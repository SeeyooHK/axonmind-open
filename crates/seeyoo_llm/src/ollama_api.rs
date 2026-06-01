use crate::types::{LlmProvider, RetryConfig, ToolDefinition};
use crate::{
    AgentEvent,
    api_mod::{ApiProvider, MessageBlock, ProviderMessage},
    errors::LlmError,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::mpsc::{Receiver, channel};
use tracing::Instrument;

#[derive(Debug)]
pub struct OllamaApiProvider {
    pub custom_endpoint: Option<String>,
    pub retry_config: RetryConfig,
    pub client: Client,
}

impl Default for OllamaApiProvider {
    fn default() -> Self {
        Self {
            custom_endpoint: None,
            retry_config: Default::default(),
            client: Client::new(),
        }
    }
}

impl OllamaApiProvider {
    pub fn new(custom_endpoint: Option<String>, retry_config: RetryConfig, client: Client) -> Self {
        Self {
            custom_endpoint,
            retry_config,
            client,
        }
    }
}

async fn map_ollama_error(resp: reqwest::Response) -> LlmError {
    let status = resp.status();
    let message = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::BAD_REQUEST
        && (message.contains("context")
            || message.contains("exceeded")
            || message.contains("length"))
    {
        return LlmError::ContextExceeded(message);
    }

    LlmError::Api {
        provider: "ollama".into(),
        message,
        code: Some(status.as_u16()),
    }
}

fn build_ollama_messages(
    system_prompt: &str,
    messages: Vec<ProviderMessage>,
) -> (Vec<Value>, Vec<Value>) {
    let mut ollama_messages = vec![json!({ "role": "system", "content": system_prompt })];

    for m in messages {
        let (role, blocks) = match m {
            ProviderMessage::User(b) => ("user", b),
            ProviderMessage::Assistant(b) => ("assistant", b),
            ProviderMessage::System(b) => ("system", b),
        };

        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut tool_results = Vec::new();

        for block in blocks {
            match block {
                MessageBlock::Text { text: t } => text.push_str(&t),
                MessageBlock::ToolCall { id, name, input } => {
                    tool_calls.push(json!({
                        "id": id, "type": "function",
                        "function": { "name": name, "arguments": input }
                    }));
                }
                MessageBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    tool_results.push(json!({
                        "role": "tool", "tool_call_id": tool_use_id, "content": content
                    }));
                }
                MessageBlock::Artifact {
                    id,
                    title,
                    artifact_type,
                    content,
                } => {
                    text.push_str(&format!(
                        "\n[Artifact: {} ({})]\n{}",
                        title.unwrap_or(id),
                        artifact_type,
                        content
                    ));
                }
                MessageBlock::Image { .. } => {
                    text.push_str("\n[Image attached]\n");
                }
            }
        }

        if !tool_results.is_empty() {
            for tr in tool_results {
                ollama_messages.push(tr);
            }
        } else if !tool_calls.is_empty() {
            ollama_messages.push(json!({
                "role": role,
                "content": if text.is_empty() { Value::Null } else { json!(text) },
                "tool_calls": tool_calls
            }));
        } else {
            ollama_messages.push(json!({ "role": role, "content": text }));
        }
    }

    (ollama_messages, Vec::new())
}

#[async_trait]
impl ApiProvider for OllamaApiProvider {
    fn id(&self) -> LlmProvider {
        LlmProvider::Local
    }
    fn display_name(&self) -> &'static str {
        "Ollama (Local)"
    }
    fn retry_config(&self) -> &RetryConfig {
        &self.retry_config
    }
    fn requires_api_key(&self) -> bool {
        false
    }

    async fn stream_completion(
        &self,
        system_prompt: &str,
        messages: Vec<ProviderMessage>,
        tools: Vec<ToolDefinition>,
        _api_key: &str,
        model: Option<&str>,
    ) -> Result<Receiver<AgentEvent>, LlmError> {
        let (tx, rx) = channel(100);
        let model = model.unwrap_or("llama3.1");
        let base_url = self
            .custom_endpoint
            .as_deref()
            .unwrap_or("http://localhost:11434");
        let url = format!("{}/api/chat", base_url);

        let (ollama_messages, _) = build_ollama_messages(system_prompt, messages);

        let mut payload = json!({ "model": model, "messages": ollama_messages, "stream": true });
        if !tools.is_empty() {
            payload["tools"] = json!(tools.into_iter().map(|t| json!({
                "type": "function",
                "function": { "name": t.name, "description": t.description, "parameters": t.parameters }
            })).collect::<Vec<_>>());
        }

        let response = crate::retry::retry_operation(&self.retry_config, || async {
            let res = self
                .client
                .post(&url)
                .json(&payload.clone())
                .send()
                .await
                .map_err(|e| {
                    if e.is_connect()
                        && (base_url.contains("localhost") || base_url.contains("127.0.0.1"))
                    {
                        LlmError::Api {
                            provider: "ollama".into(),
                            message: format!(
                                "Ollama is not running on {}. Please start it first.",
                                base_url
                            ),
                            code: None,
                        }
                    } else {
                        e.into()
                    }
                })?;
            if !res.status().is_success() {
                return Err(map_ollama_error(res).await);
            }
            Ok(res)
        })
        .await?;

        let mut stream = response.bytes_stream();
        let span = tracing::Span::current();
        tokio::spawn(
            async move {
                let mut buffer = Vec::new();
                while let Some(chunk) = stream.next().await {
                    let bytes = match chunk {
                        Ok(b) => b,
                        Err(e) => {
                            let _ = tx
                                .send(AgentEvent::Error(LlmError::StreamError(e.to_string())))
                                .await;
                            break;
                        }
                    };
                    buffer.extend_from_slice(&bytes);

                    while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                        let line_bytes = buffer.drain(..=pos).collect::<Vec<_>>();
                        let data: Value = match serde_json::from_slice(&line_bytes) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        if let Some(content) = data["message"]["content"].as_str()
                            && !content.is_empty()
                        {
                            let _ = tx.send(AgentEvent::TextChunk(content.to_string())).await;
                        }

                        if let Some(tool_calls) = data["message"]["tool_calls"].as_array() {
                            for tc in tool_calls {
                                let name = tc["function"]["name"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string();
                                let args = tc["function"]["arguments"].clone();
                                let _ = tx
                                    .send(AgentEvent::ToolCall {
                                        tool_name: name,
                                        arguments: args,
                                    })
                                    .await;
                            }
                        }

                        if data["done"].as_bool().unwrap_or(false) {
                            let _ = tx
                                .send(AgentEvent::StatusUpdate("Stream completed".into()))
                                .await;
                            return;
                        }
                    }
                }
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
        _api_key: &str,
        model: Option<&str>,
    ) -> Result<String, LlmError> {
        let model = model.unwrap_or("llama3.1");
        let base_url = self
            .custom_endpoint
            .as_deref()
            .unwrap_or("http://localhost:11434");
        let url = format!("{}/api/chat", base_url);

        let (ollama_messages, _) = build_ollama_messages(system_prompt, messages);

        let mut payload = json!({ "model": model, "messages": ollama_messages, "stream": false });
        if !tools.is_empty() {
            payload["tools"] = json!(tools.into_iter().map(|t| json!({
                "type": "function",
                "function": { "name": t.name, "description": t.description, "parameters": t.parameters }
            })).collect::<Vec<_>>());
        }

        let response = crate::retry::retry_operation(&self.retry_config, || async {
            let res = self
                .client
                .post(&url)
                .json(&payload.clone())
                .send()
                .await
                .map_err(|e| {
                    if e.is_connect()
                        && (base_url.contains("localhost") || base_url.contains("127.0.0.1"))
                    {
                        LlmError::Api {
                            provider: "ollama".into(),
                            message: format!(
                                "Ollama is not running on {}. Please start it first.",
                                base_url
                            ),
                            code: None,
                        }
                    } else {
                        e.into()
                    }
                })?;
            if !res.status().is_success() {
                return Err(map_ollama_error(res).await);
            }
            Ok(res)
        })
        .await?;

        let data: Value = response.json().await.map_err(|e| LlmError::Api {
            provider: "ollama".into(),
            message: format!("Failed to parse Ollama response: {}", e),
            code: None,
        })?;
        Ok(data["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    async fn fetch_context_window(
        &self,
        model: &str,
        endpoint_url: Option<&str>,
    ) -> Result<Option<usize>, LlmError> {
        let base_url = endpoint_url.unwrap_or("http://localhost:11434");
        let url = format!("{}/api/show", base_url);

        let response = crate::retry::retry_operation(&self.retry_config, || async {
            let res = self
                .client
                .post(&url)
                .json(&json!({ "name": model }))
                .send()
                .await
                .map_err(|e| {
                    if e.is_connect()
                        && (base_url.contains("localhost") || base_url.contains("127.0.0.1"))
                    {
                        LlmError::Api {
                            provider: "ollama".into(),
                            message: format!(
                                "Ollama is not running on {}. Please start it first.",
                                base_url
                            ),
                            code: None,
                        }
                    } else {
                        e.into()
                    }
                })?;
            if !res.status().is_success() {
                return Err(map_ollama_error(res).await);
            }
            Ok(res)
        })
        .await?;

        let data: Value = response.json().await.unwrap_or_default();
        Ok(data
            .pointer("/model_info/llama.context_length")
            .and_then(|v| v.as_u64())
            .map(|c| c as usize))
    }

    async fn list_models(&self, endpoint_url: Option<&str>) -> Result<Vec<String>, LlmError> {
        let base = endpoint_url
            .or(self.custom_endpoint.as_deref())
            .unwrap_or("http://localhost:11434");
        let url = format!("{}/api/tags", base.trim_end_matches('/'));

        let Ok(resp) = self.client.get(&url).send().await else {
            return Ok(vec![]);
        };
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let data: Value = resp.json().await.unwrap_or_default();

        Ok(data["models"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }
}
