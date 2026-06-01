use crate::types::{LlmProvider, RetryConfig, ToolDefinition};
use crate::{
    AgentEvent,
    api_mod::{ApiProvider, MessageBlock, ProviderMessage},
    errors::LlmError,
};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::stream::StreamExt;
use rand::Rng;
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::mpsc::{Receiver, channel};
use tracing::Instrument;

#[derive(Debug)]
pub struct GeminiApiProvider {
    pub retry_config: RetryConfig,
    pub client: Client,
}

impl Default for GeminiApiProvider {
    fn default() -> Self {
        Self {
            retry_config: Default::default(),
            client: Client::new(),
        }
    }
}

impl GeminiApiProvider {
    pub fn new(retry_config: RetryConfig, client: Client) -> Self {
        Self {
            retry_config,
            client,
        }
    }
}

fn parse_gemini_event(data: &Value) -> Vec<AgentEvent> {
    let mut events = Vec::new();

    if let Some(candidates) = data["candidates"].as_array()
        && let Some(candidate) = candidates.first()
    {
        let content = &candidate["content"];
        if let Some(parts) = content["parts"].as_array() {
            for part in parts {
                if let Some(text) = part["text"].as_str() {
                    events.push(AgentEvent::TextChunk(text.to_string()));
                }

                if let Some(func_call) = part.get("functionCall") {
                    let name = func_call["name"].as_str().unwrap_or("").to_string();
                    let args = func_call["args"].clone();
                    let mut rng = rand::thread_rng();
                    let tool_use_id = format!("gemini_{:x}", rng.r#gen::<u64>());

                    events.push(AgentEvent::StatusUpdate(format!(
                        "Gemini invoking tool: {}...",
                        name
                    )));
                    events.push(AgentEvent::ToolCall {
                        tool_name: name,
                        arguments: json!({
                            "arguments": args,
                            "metadata":  { "tool_use_id": tool_use_id }
                        }),
                    });
                }
            }
        }
    }

    if let Some(usage) = data.get("usageMetadata") {
        let input = usage["promptTokenCount"].as_u64().unwrap_or(0);
        let output = usage["candidatesTokenCount"].as_u64().unwrap_or(0);
        if input > 0 || output > 0 {
            events.push(AgentEvent::TokenUsage {
                input_tokens: input,
                output_tokens: output,
            });
        }
    }

    events
}

async fn map_gemini_error(resp: reqwest::Response) -> LlmError {
    let status = resp.status();
    let provider = "gemini".to_string();

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return LlmError::RateLimited {
            provider,
            retry_after_ms: None,
        };
    }

    let message = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::BAD_REQUEST
        && (message.contains("context window")
            || message.contains("exceeded")
            || message.contains("too many tokens"))
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

impl GeminiApiProvider {
    fn build_payload(
        &self,
        system_prompt: &str,
        messages: &[ProviderMessage],
        tools: Vec<ToolDefinition>,
    ) -> Value {
        let mut contents = Vec::new();

        for m in messages {
            let (role, blocks) = match m {
                ProviderMessage::User(b) => ("user", b),
                ProviderMessage::Assistant(b) => ("model", b),
                ProviderMessage::System(b) => ("user", b),
            };

            let parts: Vec<Value> = blocks.iter().map(|block| match block {
                MessageBlock::Text { text } => json!({ "text": text }),
                MessageBlock::ToolCall { name, input, .. } => json!({
                    "functionCall": { "name": name, "args": input }
                }),
                MessageBlock::ToolResult { tool_use_id, content, .. } => json!({
                    "functionResponse": { "name": tool_use_id, "response": { "result": content } }
                }),
                MessageBlock::Artifact { id, title, artifact_type, content } => json!({
                    "text": format!("[Artifact: {} ({})]\n{}", title.as_ref().unwrap_or(id), artifact_type, content)
                }),
                MessageBlock::Image { data_base64, mime_type } => json!({
                    "inline_data": { "mime_type": mime_type, "data": data_base64 }
                }),
            }).collect();

            contents.push(json!({ "role": role, "parts": parts }));
        }

        let tools_payload: Vec<Value> = tools
            .into_iter()
            .map(|t| {
                json!({
                    "function_declarations": [{
                        "name":        t.name,
                        "description": t.description,
                        "parameters":  t.parameters
                    }]
                })
            })
            .collect();

        let mut root = json!({
            "contents": contents,
            "system_instruction": {
                "role":  "system",
                "parts": [{ "text": system_prompt }]
            },
            "generationConfig": { "maxOutputTokens": 4096, "temperature": 0.7 }
        });

        if !tools_payload.is_empty() {
            root["tools"] = json!(tools_payload);
        }

        root
    }
}

#[async_trait]
impl ApiProvider for GeminiApiProvider {
    fn id(&self) -> LlmProvider {
        LlmProvider::Gemini
    }
    fn display_name(&self) -> &'static str {
        "Google Gemini API"
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
        let model = model.unwrap_or("gemini-1.5-flash-lite");
        let payload = self.build_payload(system_prompt, &messages, tools);

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?key={}&alt=sse",
            model, api_key
        );

        let response = crate::retry::retry_operation(&self.retry_config, || async {
            let resp = self
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .json(&payload.clone())
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(map_gemini_error(resp).await);
            }
            Ok(resp)
        })
        .await?;

        let mut source = response.bytes_stream().eventsource();
        let span = tracing::Span::current();
        tokio::spawn(
            async move {
                let _ = tx
                    .send(AgentEvent::StatusUpdate("Gemini stream opened...".into()))
                    .await;
                while let Some(event) = source.next().await {
                    match event {
                        Ok(event) => {
                            let data: Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::warn!(
                                        "Failed to parse Gemini SSE data: {}. Data: {}",
                                        e,
                                        event.data
                                    );
                                    continue;
                                }
                            };
                            for e in parse_gemini_event(&data) {
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
        let model = model.unwrap_or("gemini-1.5-flash-lite");
        let payload = self.build_payload(system_prompt, &messages, tools);

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model, api_key
        );

        let response = crate::retry::retry_operation(&self.retry_config, || async {
            let resp = self
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .json(&payload.clone())
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(map_gemini_error(resp).await);
            }
            Ok(resp)
        })
        .await?;

        let data: Value = response.json().await.map_err(|e| LlmError::Api {
            provider: "gemini".into(),
            message: format!("Failed to parse Gemini response: {}", e),
            code: None,
        })?;

        if let Some(candidates) = data["candidates"].as_array()
            && let Some(candidate) = candidates.first()
            && let Some(parts) = candidate["content"]["parts"].as_array()
            && let Some(part) = parts.first()
            && let Some(text) = part["text"].as_str()
        {
            Ok(text.to_string())
        } else {
            Err(LlmError::Api {
                provider: "gemini".into(),
                message: "No text content found in Gemini response".into(),
                code: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gemini_text() {
        let data =
            json!({ "candidates": [{ "content": { "parts": [{ "text": "Hello Gemini" }] } }] });
        let events = parse_gemini_event(&data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AgentEvent::TextChunk(text) => assert_eq!(text, "Hello Gemini"),
            _ => panic!("Expected TextChunk"),
        }
    }

    #[test]
    fn test_parse_gemini_function_call() {
        let data = json!({ "candidates": [{ "content": { "parts": [{ "functionCall": { "name": "list_files", "args": { "path": "." } } }] } }] });
        let events = parse_gemini_event(&data);
        assert_eq!(events.len(), 2);
        match &events[1] {
            AgentEvent::ToolCall {
                tool_name,
                arguments,
            } => {
                assert_eq!(tool_name, "list_files");
                assert_eq!(arguments["arguments"]["path"], ".");
            }
            _ => panic!("Expected ToolCall"),
        }
    }
}
