use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum LlmError {
    #[error("API error from {provider}: {message} (code: {code:?})")]
    Api {
        provider: String,
        message: String,
        code: Option<u16>,
    },

    #[error("Rate limited by {provider}. Retry after {retry_after_ms:?}ms")]
    RateLimited {
        provider: String,
        retry_after_ms: Option<u64>,
    },

    #[error("Context window exceeded: {0}")]
    ContextExceeded(String),

    #[error("Authentication failed for {provider}")]
    Auth { provider: String },

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Network/Request failed: {0}")]
    RequestFailed(String),

    #[error("Stream error: {0}")]
    StreamError(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Provider not found: {0}")]
    ProviderNotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl LlmError {
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::RateLimited { .. } => true,
            LlmError::RequestFailed(_) => true,
            LlmError::Api {
                code: Some(code), ..
            } => (500..=504).contains(code) || *code == 429,
            _ => false,
        }
    }
}

impl From<reqwest::Error> for LlmError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() || err.is_connect() {
            LlmError::RequestFailed(format!("Network timeout/connection error: {}", err))
        } else {
            if err.is_decode() {
                return LlmError::Serialization(err.to_string());
            }
            LlmError::RequestFailed(err.to_string())
        }
    }
}

impl From<serde_json::Error> for LlmError {
    fn from(err: serde_json::Error) -> Self {
        LlmError::Serialization(err.to_string())
    }
}
