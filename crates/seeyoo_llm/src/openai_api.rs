use crate::types::{LlmProvider, RetryConfig, ToolDefinition};
use crate::{
    AgentEvent,
    api_mod::{ApiProvider, MessageBlock, ProviderMessage},
    errors::LlmError,
};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::stream::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc::{Receiver, channel};

#[derive(Debug)]
pub struct OpenAiApiProvider {
    pub base_url: Option<String>,
    pub retry_config: RetryConfig,
    pub client: Client,
    /// True when targeting a local server (LM Studio, llama.cpp, Jan, vLLM, etc.).
    pub is_local: bool,
    /// Overrides id() for OpenAI-compatible cloud providers that have distinct API keys
    /// (DeepSeek, Groq, OpenRouter). None means use the is_local/OpenAi default.
    pub provider_id: Option<LlmProvider>,
}

impl Default for OpenAiApiProvider {
    fn default() -> Self {
        Self {
            base_url: None,
            retry_config: Default::default(),
            client: Client::new(),
            is_local: false,
            provider_id: None,
        }
    }
}

impl OpenAiApiProvider {
    pub fn new(
        base_url: Option<String>,
        retry_config: RetryConfig,
        client: Client,
        is_local: bool,
    ) -> Self {
        Self {
            base_url,
            retry_config,
            client,
            is_local,
            provider_id: None,
        }
    }

    pub fn with_identity(
        base_url: Option<String>,
        retry_config: RetryConfig,
        client: Client,
        id: LlmProvider,
    ) -> Self {
        Self {
            base_url,
            retry_config,
            client,
            is_local: false,
            provider_id: Some(id),
        }
    }
}

fn parse_openai_event(
    data: &Value,
    active_tools: &mut HashMap<u64, (String, String, String)>,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();

    if let Some(choices) = data["choices"].as_array()
        && let Some(choice) = choices.first()
    {
        if let Some(content) = choice["delta"]["content"].as_str() {
            events.push(AgentEvent::TextChunk(content.to_string()));
        }

        if let Some(delta_tool_calls) = choice["delta"].get("tool_calls").and_then(|t| t.as_array())
        {
            for tc in delta_tool_calls {
                let index = tc["index"].as_u64().unwrap_or(0);
                let entry = active_tools
                    .entry(index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));

                if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                    entry.2.push_str(id);
                }
                if let Some(name) = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                {
                    entry.0.push_str(name);
                    events.push(AgentEvent::StatusUpdate(format!(
                        "Preparing tool: {}...",
                        entry.0
                    )));
                }
                if let Some(args) = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|a| a.as_str())
                {
                    entry.1.push_str(args);
                }
            }
        }

        if let Some(finish_reason) = choice.get("finish_reason").and_then(|f| f.as_str())
            && finish_reason == "tool_calls"
        {
            for (_, (name, args_str, id)) in active_tools.drain() {
                let parsed_args: Value = serde_json::from_str(&args_str).unwrap_or(json!({}));
                events.push(AgentEvent::ToolCall {
                    tool_name: name,
                    arguments: json!({
                        "arguments": parsed_args,
                        "metadata":  { "tool_use_id": id }
                    }),
                });
            }
        }
    }

    if let Some(usage) = data.get("usage") {
        events.push(AgentEvent::TokenUsage {
            input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
        });
    }

    events
}

async fn map_openai_error(resp: reqwest::Response) -> LlmError {
    let status = resp.status();
    let provider = "openai".to_string();

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|s| s * 1000)
            .or_else(|| {
                resp.headers()
                    .get("x-ratelimit-reset-requests")
                    .and_then(|v| v.to_str().ok())
                    .map(parse_openai_duration)
            });
        return LlmError::RateLimited {
            provider,
            retry_after_ms: retry_after,
        };
    }

    let message = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::BAD_REQUEST
        && (message.contains("context_length_exceeded") || message.contains("string_too_long"))
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

fn parse_openai_duration(v: &str) -> u64 {
    if v.ends_with("ms") {
        v.trim_end_matches("ms").parse::<u64>().unwrap_or(0)
    } else if v.ends_with("s") {
        v.trim_end_matches("s").parse::<u64>().unwrap_or(0) * 1000
    } else if v.ends_with("m") {
        v.trim_end_matches("m").parse::<u64>().unwrap_or(0) * 60_000
    } else {
        v.parse::<u64>().unwrap_or(0) * 1000
    }
}

fn build_openai_messages(system_prompt: &str, messages: Vec<ProviderMessage>) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": system_prompt })];

    out.extend(messages.into_iter().map(|m| {
        let (role, blocks) = match m {
            ProviderMessage::User(b) => ("user", b),
            ProviderMessage::Assistant(b) => ("assistant", b),
            ProviderMessage::System(b) => ("system", b),
        };

        let mut text_content = String::new();
        let mut has_image = false;
        let mut content_array: Vec<Value> = Vec::new();
        let mut tool_calls = Vec::new();
        let mut tool_result: Option<Value> = None;

        for block in blocks {
            match block {
                MessageBlock::Text { text } => {
                    text_content.push_str(&text);
                    content_array.push(json!({ "type": "text", "text": text }));
                }
                MessageBlock::ToolCall { id, name, input } => {
                    tool_calls.push(json!({
                        "id": id, "type": "function",
                        "function": { "name": name, "arguments": input.to_string() }
                    }));
                }
                MessageBlock::ToolResult {
                    tool_use_id,
                    content: tool_content,
                    ..
                } => {
                    tool_result = Some(json!({
                        "role": "tool", "tool_call_id": tool_use_id, "content": tool_content
                    }));
                    break;
                }
                MessageBlock::Artifact {
                    id,
                    title,
                    artifact_type,
                    content: artifact_content,
                } => {
                    let formatted = format!(
                        "\n[Artifact: {} ({})]\n{}",
                        title.unwrap_or(id),
                        artifact_type,
                        artifact_content
                    );
                    text_content.push_str(&formatted);
                    content_array.push(json!({ "type": "text", "text": formatted }));
                }
                MessageBlock::Image {
                    data_base64,
                    mime_type,
                } => {
                    has_image = true;
                    content_array.push(json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{};base64,{}", mime_type, data_base64) }
                    }));
                }
            }
        }

        let final_content = if has_image {
            json!(content_array)
        } else if text_content.is_empty() {
            Value::Null
        } else {
            json!(text_content)
        };

        if let Some(tr) = tool_result {
            tr
        } else if !tool_calls.is_empty() {
            json!({ "role": role, "content": final_content, "tool_calls": tool_calls })
        } else {
            json!({ "role": role, "content": final_content })
        }
    }));

    out
}

fn build_openai_tools(tools: Vec<ToolDefinition>) -> Vec<Value> {
    tools.into_iter().map(|t| json!({
        "type": "function",
        "function": { "name": t.name, "description": t.description, "parameters": t.parameters }
    })).collect()
}

#[async_trait]
impl ApiProvider for OpenAiApiProvider {
    fn id(&self) -> LlmProvider {
        self.provider_id.unwrap_or_else(|| {
            if self.is_local {
                LlmProvider::Local
            } else {
                LlmProvider::OpenAi
            }
        })
    }
    fn display_name(&self) -> &'static str {
        match self.provider_id {
            Some(LlmProvider::DeepSeek) => "DeepSeek API",
            Some(LlmProvider::Groq) => "Groq API",
            Some(LlmProvider::OpenRouter) => "OpenRouter API",
            _ if self.is_local => "Local (OpenAI-compatible)",
            _ => "OpenAI API",
        }
    }
    fn retry_config(&self) -> &RetryConfig {
        &self.retry_config
    }
    fn requires_api_key(&self) -> bool {
        !self.is_local
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
        let model = model.unwrap_or("gpt-4o-mini");

        let openai_messages = build_openai_messages(system_prompt, messages);
        let openai_tools = build_openai_tools(tools);

        let mut payload = json!({
            "model": model,
            "messages": openai_messages,
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        if !openai_tools.is_empty() {
            payload["tools"] = json!(openai_tools);
        }

        let base = self
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");
        let url = format!("{}/chat/completions", base.trim_end_matches('/'));

        let response = crate::retry::retry_operation(&self.retry_config, || async {
            let resp = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&payload.clone())
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(map_openai_error(resp).await);
            }
            Ok(resp)
        })
        .await?;

        let mut source = response.bytes_stream().eventsource();
        use tracing::Instrument;
        let span = tracing::Span::current();
        tokio::spawn(
            async move {
                let mut active_tools: HashMap<u64, (String, String, String)> = HashMap::new();
                let _ = tx
                    .send(AgentEvent::StatusUpdate(
                        "Connection to OpenAI opened...".into(),
                    ))
                    .await;

                while let Some(event) = source.next().await {
                    match event {
                        Ok(event) => {
                            if event.data == "[DONE]" {
                                break;
                            }
                            let data: Value = match serde_json::from_str(&event.data) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            for e in parse_openai_event(&data, &mut active_tools) {
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
        let model = model.unwrap_or("gpt-4o-mini");

        let openai_messages = build_openai_messages(system_prompt, messages);
        let openai_tools = build_openai_tools(tools);

        let mut payload = json!({
            "model": model,
            "messages": openai_messages,
            "stream": false
        });
        if !openai_tools.is_empty() {
            payload["tools"] = json!(openai_tools);
        }

        let base = self
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");
        let url = format!("{}/chat/completions", base.trim_end_matches('/'));

        let response = crate::retry::retry_operation(&self.retry_config, || async {
            let resp = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&payload.clone())
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(map_openai_error(resp).await);
            }
            Ok(resp)
        })
        .await?;

        let data: Value = response.json().await.map_err(|e| LlmError::Api {
            provider: "openai".into(),
            message: format!("Failed to parse OpenAI response: {}", e),
            code: None,
        })?;

        Ok(data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    async fn fetch_context_window(
        &self,
        model: &str,
        endpoint_url: Option<&str>,
    ) -> Result<Option<usize>, LlmError> {
        if !self.is_local {
            return Ok(None);
        }
        let base = endpoint_url
            .or(self.base_url.as_deref())
            .unwrap_or("http://localhost:1234/v1");
        let url = format!("{}/models/{}", base.trim_end_matches('/'), model);

        let Ok(resp) = self.client.get(&url).send().await else {
            return Ok(None);
        };
        if !resp.status().is_success() {
            return Ok(None);
        }
        let data: Value = resp.json().await.unwrap_or_default();

        // LM Studio exposes max_context_length; some others expose context_window
        let ctx = data
            .get("max_context_length")
            .or_else(|| data.get("context_window"))
            .and_then(|v| v.as_u64())
            .map(|c| c as usize);
        Ok(ctx)
    }

    async fn list_models(&self, endpoint_url: Option<&str>) -> Result<Vec<String>, LlmError> {
        let base = endpoint_url
            .or(self.base_url.as_deref())
            .unwrap_or("https://api.openai.com/v1");
        let url = format!("{}/models", base.trim_end_matches('/'));

        let Ok(resp) = self.client.get(&url).send().await else {
            return Ok(vec![]);
        };
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let data: Value = resp.json().await.unwrap_or_default();

        Ok(data["data"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m["id"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_openai_text() {
        let mut active_tools = HashMap::new();
        let data = json!({ "choices": [{ "delta": { "content": "Hello" }, "index": 0 }] });
        let events = parse_openai_event(&data, &mut active_tools);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AgentEvent::TextChunk(text) => assert_eq!(text, "Hello"),
            _ => panic!("Expected TextChunk"),
        }
    }

    #[test]
    fn test_parse_openai_tool_call_accumulation() {
        let mut active_tools = HashMap::new();

        let chunk1 = json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "call_1", "function": { "name": "read" } }] }, "index": 0 }] });
        let events1 = parse_openai_event(&chunk1, &mut active_tools);
        assert_eq!(events1.len(), 1);
        assert!(matches!(events1[0], AgentEvent::StatusUpdate(_)));

        let chunk2 = json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "{\"path\":" } }] }, "index": 0 }] });
        parse_openai_event(&chunk2, &mut active_tools);

        let chunk3 = json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": " \"lib.rs\"}" } }] }, "index": 0 }] });
        parse_openai_event(&chunk3, &mut active_tools);

        let chunk4 =
            json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls", "index": 0 }] });
        let events4 = parse_openai_event(&chunk4, &mut active_tools);
        assert_eq!(events4.len(), 1);
        match &events4[0] {
            AgentEvent::ToolCall {
                tool_name,
                arguments,
            } => {
                assert_eq!(tool_name, "read");
                assert_eq!(arguments["arguments"]["path"], "lib.rs");
                assert_eq!(arguments["metadata"]["tool_use_id"], "call_1");
            }
            _ => panic!("Expected ToolCall"),
        }
    }

    #[test]
    fn test_parse_openai_usage() {
        let mut active_tools = HashMap::new();
        let data = json!({ "usage": { "prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150 } });
        let events = parse_openai_event(&data, &mut active_tools);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AgentEvent::TokenUsage {
                input_tokens,
                output_tokens,
            } => {
                assert_eq!(*input_tokens, 100);
                assert_eq!(*output_tokens, 50);
            }
            _ => panic!("Expected TokenUsage"),
        }
    }
}
