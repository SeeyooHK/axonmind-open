pub mod api_mod;
pub mod errors;
pub mod factory;
pub mod local_detect;
pub mod retry;
pub mod types;

pub mod anthropic_api;
pub mod codex_api;
pub mod gemini_api;
pub mod ollama_api;
pub mod openai_api;

pub use factory as provider_factory;
pub use local_detect::{LocalServerInfo, detect_local_server, probe_all_local_servers};
pub use types::{LlmProvider, ProxySettings, RetryConfig, ToolDefinition};

use crate::api_mod::ApiProvider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: String,
    pub model: Option<String>,
    pub custom_endpoint: Option<String>,
    pub retry_config: Option<types::RetryConfig>,
    pub proxy_settings: Option<types::ProxySettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum AgentEvent {
    TextChunk(String),
    StatusUpdate(String),
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
    },
    RawOutput(String),
    Error(errors::LlmError),
    ProcessExit {
        code: i32,
    },
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
    },
}

#[derive(Clone, Debug)]
pub enum AgentProvider {
    Api(Arc<dyn ApiProvider + Send + Sync>),
}

impl AgentProvider {
    pub fn id(&self) -> LlmProvider {
        match self {
            AgentProvider::Api(p) => p.id(),
        }
    }

    pub fn requires_api_key(&self) -> bool {
        match self {
            AgentProvider::Api(p) => p.requires_api_key(),
        }
    }

    pub async fn list_models(
        &self,
        endpoint_url: Option<&str>,
    ) -> Result<Vec<String>, errors::LlmError> {
        match self {
            AgentProvider::Api(p) => p.list_models(endpoint_url).await,
        }
    }
}

/// Maps a `provider_type` string to an `LlmProvider` enum value and injects
/// the default localhost endpoint for named local tools when no `custom_endpoint`
/// is provided by the caller.
fn resolve_provider(
    type_str: &str,
    custom_endpoint: Option<String>,
) -> (LlmProvider, Option<String>) {
    match type_str {
        "anthropic-api" | "anthropic" | "claude-code" => (LlmProvider::Anthropic, custom_endpoint),
        "gemini-api" | "gemini" | "gemini-cli" => (LlmProvider::Gemini, custom_endpoint),
        "openai-api" | "openai" => (LlmProvider::OpenAi, custom_endpoint),
        "openai-compatible" => (LlmProvider::OpenAi, custom_endpoint),
        "deepseek" => (LlmProvider::DeepSeek, custom_endpoint),
        "groq" => (LlmProvider::Groq, custom_endpoint),
        "openrouter" => (LlmProvider::OpenRouter, custom_endpoint),
        "ollama-api" | "ollama" => (
            LlmProvider::Local,
            custom_endpoint.or(Some("http://localhost:11434".into())),
        ),
        "lmstudio" | "lm-studio" => (
            LlmProvider::OpenAi,
            custom_endpoint.or(Some("http://localhost:1234/v1".into())),
        ),
        "llamacpp" | "llama-cpp" | "llama.cpp" => (
            LlmProvider::OpenAi,
            custom_endpoint.or(Some("http://localhost:8080/v1".into())),
        ),
        "jan" | "jan-ai" => (
            LlmProvider::OpenAi,
            custom_endpoint.or(Some("http://localhost:1337/v1".into())),
        ),
        "vllm" => (
            LlmProvider::OpenAi,
            custom_endpoint.or(Some("http://localhost:8000/v1".into())),
        ),
        "codex" | "codex-cli" => (LlmProvider::Codex, custom_endpoint),
        _ => (LlmProvider::Anthropic, custom_endpoint),
    }
}

pub fn provider_from_config(config: ProviderConfig) -> AgentProvider {
    let retry_config = config.retry_config.unwrap_or_default();
    let (provider_type, custom_endpoint) =
        resolve_provider(&config.provider_type, config.custom_endpoint);

    let provider = crate::factory::get_api_provider(
        provider_type,
        custom_endpoint,
        retry_config,
        config.proxy_settings.as_ref(),
    );

    AgentProvider::Api(provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_from_config_api_variants() {
        let p_anthropic = provider_from_config(ProviderConfig {
            provider_type: "anthropic-api".into(),
            model: None,
            custom_endpoint: None,
            retry_config: None,
            proxy_settings: None,
        });
        assert_eq!(p_anthropic.id(), LlmProvider::Anthropic);

        let p_gemini = provider_from_config(ProviderConfig {
            provider_type: "gemini-api".into(),
            model: None,
            custom_endpoint: None,
            retry_config: None,
            proxy_settings: None,
        });
        assert_eq!(p_gemini.id(), LlmProvider::Gemini);

        let p_openai = provider_from_config(ProviderConfig {
            provider_type: "openai-api".into(),
            model: None,
            custom_endpoint: None,
            retry_config: None,
            proxy_settings: None,
        });
        assert_eq!(p_openai.id(), LlmProvider::OpenAi);

        let p_ollama = provider_from_config(ProviderConfig {
            provider_type: "ollama-api".into(),
            model: None,
            custom_endpoint: None,
            retry_config: None,
            proxy_settings: None,
        });
        assert_eq!(p_ollama.id(), LlmProvider::Local);
    }
}
