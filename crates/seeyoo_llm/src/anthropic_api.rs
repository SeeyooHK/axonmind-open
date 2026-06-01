use crate::types::{LlmProvider, RetryConfig, ToolDefinition};
use crate::{
    AgentEvent,
    api_mod::{ApiProvider, MessageBlock, ProviderMessage},
    errors::LlmError,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use eventsource_stream::Eventsource;
use futures_util::stream::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc::{Receiver, channel};
use tracing::Instrument;

#[derive(Debug)]
pub struct AnthropicApiProvider {
    pub retry_config: RetryConfig,
    pub client: Client,
}

impl Default for AnthropicApiProvider {
    fn default() -> Self {
        Self {
            retry_config: Default::default(),
            client: Client::new(),
        }
    }
}

impl AnthropicApiProvider {
    pub fn new(retry_config: RetryConfig, client: Client) -> Self {
        Self {
            retry_config,
            client,
        }
    }
}

fn parse_anthropic_event(
    data: &Value,
    active_tools: &mut HashMap<usize, (String, String, String)>,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    let event_type = data["type"].as_str().unwrap_or("");

    match event_type {
        "content_block_start" => {
            let index = data["index"].as_u64().unwrap_or(0) as usize;
            if let Some(blk) = data.get("content_block")
                && blk["type"] == "tool_use"
            {
                let name = blk["name"].as_str().unwrap_or("").to_string();
                let id = blk["id"].as_str().unwrap_or("").to_string();
                active_tools.insert(index, (name.clone(), String::new(), id));
                events.push(AgentEvent::StatusUpdate(format!(
                    "Anthropic starting tool: {}...",
                    name
                )));
            }
        }
        "content_block_delta" => {
            let index = data["index"].as_u64().unwrap_or(0) as usize;
            let delta = &data["delta"];
            let delta_type = delta["type"].as_str().unwrap_or("");

            if delta_type == "text_delta" {
                if let Some(text) = delta["text"].as_str() {
                    events.push(AgentEvent::TextChunk(text.to_string()));
                }
            } else if delta_type == "input_json_delta"
                && let Some(partial_json) = delta["partial_json"].as_str()
                && let Some(entry) = active_tools.get_mut(&index)
            {
                entry.1.push_str(partial_json);
            }
        }
        "content_block_stop" => {
            let index = data["index"].as_u64().unwrap_or(0) as usize;
            if let Some((name, args_str, id)) = active_tools.remove(&index) {
                let parsed_args: Value = serde_json::from_str(&args_str).unwrap_or(json!({}));
                let combined_payload = json!({
                    "arguments": parsed_args,
                    "metadata": { "tool_use_id": id }
                });
                events.push(AgentEvent::StatusUpdate(format!(
                    "Anthropic completed tool arguments: {}",
                    name
                )));
                events.push(AgentEvent::ToolCall {
                    tool_name: name,
                    arguments: combined_payload,
                });
            }
        }
        "message_delta" => {
            if let Some(usage) = data.get("usage") {
                events.push(AgentEvent::TokenUsage {
                    input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                    output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
                });
            }
        }
        "error" => {
            let err_msg = data["error"]["message"]
                .as_str()
                .unwrap_or("Unknown API error");
            events.push(AgentEvent::Error(LlmError::Api {
                provider: "anthropic".into(),
                message: err_msg.to_string(),
                code: None,
            }));
        }
        _ => {}
    }

    events
}

async fn map_anthropic_error(resp: reqwest::Response) -> LlmError {
    let status = resp.status();
    let provider = "anthropic".to_string();

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let mut retry_after_ms = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                if let Ok(s) = v.parse::<u64>() {
                    Some(s * 1000)
                } else if let Ok(dt) = DateTime::parse_from_rfc2822(v) {
                    let now = Utc::now();
                    let delta = dt.with_timezone(&Utc) - now;
                    Some(delta.num_milliseconds().max(0) as u64)
                } else {
                    None
                }
            });

        if retry_after_ms.is_none() {
            retry_after_ms = resp
                .headers()
                .get("anthropic-ratelimit-requests-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
                .map(|dt| {
                    let now = Utc::now();
                    let delta = dt.with_timezone(&Utc) - now;
                    delta.num_milliseconds().max(0) as u64
                });
        }

        return LlmError::RateLimited {
            provider,
            retry_after_ms,
        };
    }

    let message = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::BAD_REQUEST
        && (message.contains("prompt is too long") || message.contains("context_length_exceeded"))
    {
        return LlmError::ContextExceeded(message);
    }

    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return LlmError::Auth { provider };
    }

    LlmError::Api {
        provider,
        message,
        code: Some(status.as_u16()),
    }
}

fn messages_to_json(messages: Vec<ProviderMessage>) -> Vec<Value> {
    messages.into_iter().map(|m| {
        let (role, blocks) = match m {
            ProviderMessage::User(b)      => ("user",      b),
            ProviderMessage::Assistant(b) => ("assistant", b),
            ProviderMessage::System(b)    => ("user",      b),
        };

        let content: Vec<Value> = blocks.into_iter().map(|b| match b {
            MessageBlock::Text { text } => json!({ "type": "text", "text": text }),
            MessageBlock::ToolCall { id, name, input } => json!({
                "type":  "tool_use",
                "id":    id,
                "name":  name,
                "input": input
            }),
            MessageBlock::ToolResult { tool_use_id, content, is_error } => json!({
                "type":        "tool_result",
                "tool_use_id": tool_use_id,
                "content":     content,
                "is_error":    is_error
            }),
            MessageBlock::Artifact { id, title, artifact_type, content } => json!({
                "type": "text",
                "text": format!("[Artifact: {} ({})]\n{}", title.unwrap_or(id), artifact_type, content)
            }),
            MessageBlock::Image { data_base64, mime_type } => json!({
                "type":   "image",
                "source": { "type": "base64", "media_type": mime_type, "data": data_base64 }
            }),
        }).collect();

        json!({ "role": role, "content": content })
    }).collect()
}

#[async_trait]
impl ApiProvider for AnthropicApiProvider {
    fn id(&self) -> LlmProvider {
        LlmProvider::Anthropic
    }
    fn display_name(&self) -> &'static str {
        "Anthropic API"
    }
    fn retry_config(&self) -> &RetryConfig {
        &self.retry_config
    }

    async fn stream_completion(
        &self,
        system_prompt: &str,
        messages: Vec<ProviderMessage>,
        tools: Vec<ToolDefinition>,
        api_key: &str,
        model: Option<&str>,
    ) -> Result<Receiver<AgentEvent>, LlmError> {
        let (tx, rx) = channel(100);
        let model = model.unwrap_or("claude-3-5-sonnet-20240620");

        let anthropic_tools: Vec<Value> = tools
            .into_iter()
            .map(|t| {
                json!({
                    "name":         t.name,
                    "description":  t.description,
                    "input_schema": t.parameters
                })
            })
            .collect();

        let mut payload = json!({
            "model":    model,
            "max_tokens": 4096,
            "system":   system_prompt,
            "messages": messages_to_json(messages),
            "stream":   true
        });
        if !anthropic_tools.is_empty() {
            payload["tools"] = json!(anthropic_tools);
        }

        let response = crate::retry::retry_operation(&self.retry_config, || async {
            let resp = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&payload.clone())
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(map_anthropic_error(resp).await);
            }
            Ok(resp)
        })
        .await?;

        let mut source = response.bytes_stream().eventsource();
        let span = tracing::Span::current();
        tokio::spawn(
            async move {
                let mut active_tools: HashMap<usize, (String, String, String)> = HashMap::new();
                let _ = tx
                    .send(AgentEvent::StatusUpdate("Connection opened...".into()))
                    .await;

                while let Some(event) = source.next().await {
                    match event {
                        Ok(event) => {
                            let data: Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            for e in parse_anthropic_event(&data, &mut active_tools) {
                                let _ = tx.send(e).await;
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send(AgentEvent::Error(LlmError::StreamError(e.to_string())))
                                .await;
                            break;
                        }
                    }
                }
                let _ = tx
                    .send(AgentEvent::StatusUpdate("Stream completed".into()))
                    .await;
            }
            .instrument(span),
        );

        Ok(rx)
    }

    async fn complete(
        &self,
        system_prompt: &str,
        messages: Vec<ProviderMessage>,
        tools: Vec<ToolDefinition>,
        api_key: &str,
        model: Option<&str>,
    ) -> Result<String, LlmError> {
        let model = model.unwrap_or("claude-3-5-sonnet-20240620");

        let anthropic_tools: Vec<Value> = tools
            .into_iter()
            .map(|t| {
                json!({
                    "name":         t.name,
                    "description":  t.description,
                    "input_schema": t.parameters
                })
            })
            .collect();

        let mut payload = json!({
            "model":    model,
            "max_tokens": 4096,
            "system":   system_prompt,
            "messages": messages_to_json(messages),
            "stream":   false
        });
        if !anthropic_tools.is_empty() {
            payload["tools"] = json!(anthropic_tools);
        }

        let response = crate::retry::retry_operation(&self.retry_config, || async {
            let resp = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&payload.clone())
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(map_anthropic_error(resp).await);
            }
            Ok(resp)
        })
        .await?;

        let data: Value = response.json().await.map_err(|e| LlmError::Api {
            provider: "anthropic".into(),
            message: format!("Failed to parse Anthropic response: {}", e),
            code: None,
        })?;

        Ok(data["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_anthropic_text() {
        let mut active_tools = HashMap::new();
        let data = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "Hello" }
        });
        let events = parse_anthropic_event(&data, &mut active_tools);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AgentEvent::TextChunk(text) => assert_eq!(text, "Hello"),
            _ => panic!("Expected TextChunk"),
        }
    }

    #[test]
    fn test_parse_anthropic_tool_call_assembly() {
        let mut active_tools = HashMap::new();

        let start = json!({
            "type": "content_block_start", "index": 1,
            "content_block": { "type": "tool_use", "id": "tu_123", "name": "read_file" }
        });
        let events_start = parse_anthropic_event(&start, &mut active_tools);
        assert_eq!(events_start.len(), 1);
        assert!(matches!(events_start[0], AgentEvent::StatusUpdate(_)));
        assert!(active_tools.contains_key(&1));

        let delta = json!({
            "type": "content_block_delta", "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "{\"path\": \"main.rs\"}" }
        });
        let events_delta = parse_anthropic_event(&delta, &mut active_tools);
        assert!(events_delta.is_empty());
        assert_eq!(active_tools.get(&1).unwrap().1, "{\"path\": \"main.rs\"}");

        let stop = json!({ "type": "content_block_stop", "index": 1 });
        let events_stop = parse_anthropic_event(&stop, &mut active_tools);
        assert_eq!(events_stop.len(), 2);
        match &events_stop[1] {
            AgentEvent::ToolCall {
                tool_name,
                arguments,
            } => {
                assert_eq!(tool_name, "read_file");
                assert_eq!(arguments["arguments"]["path"], "main.rs");
                assert_eq!(arguments["metadata"]["tool_use_id"], "tu_123");
            }
            _ => panic!("Expected ToolCall"),
        }
    }

    #[test]
    fn test_parse_anthropic_usage() {
        let mut active_tools = HashMap::new();
        let data = json!({ "type": "message_delta", "usage": { "input_tokens": 50, "output_tokens": 20 } });
        let events = parse_anthropic_event(&data, &mut active_tools);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AgentEvent::TokenUsage {
                input_tokens,
                output_tokens,
            } => {
                assert_eq!(*input_tokens, 50);
                assert_eq!(*output_tokens, 20);
            }
            _ => panic!("Expected TokenUsage"),
        }
    }

    #[test]
    fn test_parse_anthropic_error() {
        let mut active_tools = HashMap::new();
        let data = json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } });
        let events = parse_anthropic_event(&data, &mut active_tools);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AgentEvent::Error(LlmError::Api { message, .. }) => assert_eq!(message, "Overloaded"),
            _ => panic!("Expected Error"),
        }
    }
}
