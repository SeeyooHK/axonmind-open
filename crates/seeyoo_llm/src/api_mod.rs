use crate::AgentEvent;
use crate::errors::LlmError;
use crate::types::{LlmProvider, ToolDefinition};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Receiver;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageBlock {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    Artifact {
        id: String,
        title: Option<String>,
        artifact_type: String,
        content: String,
    },
    Image {
        data_base64: String,
        mime_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", content = "content", rename_all = "snake_case")]
pub enum ProviderMessage {
    User(Vec<MessageBlock>),
    Assistant(Vec<MessageBlock>),
    System(Vec<MessageBlock>),
}

impl ProviderMessage {
    pub fn user(text: String) -> Self {
        Self::User(vec![MessageBlock::Text { text }])
    }
    pub fn assistant(text: String) -> Self {
        Self::Assistant(vec![MessageBlock::Text { text }])
    }
    pub fn system(text: String) -> Self {
        Self::System(vec![MessageBlock::Text { text }])
    }
}

#[async_trait]
pub trait ApiProvider: Send + Sync + std::fmt::Debug {
    fn id(&self) -> LlmProvider;
    fn display_name(&self) -> &'static str;
    fn retry_config(&self) -> &crate::types::RetryConfig;

    async fn stream_completion(
        &self,
        system_prompt: &str,
        messages: Vec<ProviderMessage>,
        tools: Vec<ToolDefinition>,
        api_key: &str,
        model: Option<&str>,
    ) -> Result<Receiver<AgentEvent>, LlmError>;

    async fn complete(
        &self,
        system_prompt: &str,
        messages: Vec<ProviderMessage>,
        tools: Vec<ToolDefinition>,
        api_key: &str,
        model: Option<&str>,
    ) -> Result<String, LlmError>;

    async fn fetch_context_window(
        &self,
        _model: &str,
        _endpoint_url: Option<&str>,
    ) -> Result<Option<usize>, LlmError> {
        Ok(None)
    }

    /// Whether this provider requires a valid API key to operate.
    /// Local servers (Ollama, LM Studio, llama.cpp, Jan, vLLM) return false.
    fn requires_api_key(&self) -> bool {
        true
    }

    /// List model identifiers available on this provider.
    /// Returns an empty vec if the provider does not support listing or the server is unreachable.
    async fn list_models(&self, _endpoint_url: Option<&str>) -> Result<Vec<String>, LlmError> {
        Ok(vec![])
    }
}
