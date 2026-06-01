use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "bindings", derive(ts_rs::TS))]
pub enum LlmProvider {
    Anthropic,
    OpenAi,
    Gemini,
    Groq,
    DeepSeek,
    OpenRouter,
    Local,
    Ollama,
    Mock,
    Codex,
}

impl fmt::Display for LlmProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anthropic => write!(f, "Anthropic"),
            Self::OpenAi => write!(f, "OpenAI"),
            Self::Gemini => write!(f, "Gemini"),
            Self::Groq => write!(f, "Groq"),
            Self::DeepSeek => write!(f, "DeepSeek"),
            Self::OpenRouter => write!(f, "OpenRouter"),
            Self::Local => write!(f, "Local"),
            Self::Ollama => write!(f, "Ollama"),
            Self::Mock => write!(f, "Mock"),
            Self::Codex => write!(f, "Codex"),
        }
    }
}

impl From<&str> for LlmProvider {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "anthropic" | "claude" | "anthropic-api" | "claude-code" => Self::Anthropic,
            "openai" | "openai-api" => Self::OpenAi,
            "codex" | "codex-cli" => Self::Codex,
            "gemini" | "gemini-api" | "gemini-cli" => Self::Gemini,
            "groq" => Self::Groq,
            "deepseek" => Self::DeepSeek,
            "openrouter" => Self::OpenRouter,
            "local" | "ollama" => Self::Local,
            "mock" => Self::Mock,
            _ => Self::Mock,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "bindings", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RetryConfig {
    pub max_retries: usize,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 30_000,
            backoff_multiplier: 2.0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[cfg_attr(feature = "bindings", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ProxySettings {
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub no_proxy: Option<String>,
    pub all_proxy: Option<String>,
    pub enabled: bool,
}
