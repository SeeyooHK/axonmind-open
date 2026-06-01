use crate::anthropic_api::AnthropicApiProvider;
use crate::api_mod::ApiProvider;
use crate::codex_api::CodexApiProvider;
use crate::gemini_api::GeminiApiProvider;
use crate::ollama_api::OllamaApiProvider;
use crate::openai_api::OpenAiApiProvider;
use crate::types::{LlmProvider, ProxySettings, RetryConfig};
use reqwest::Client;
use std::sync::{Arc, Mutex};

pub fn get_api_provider(
    provider_type: LlmProvider,
    custom_endpoint: Option<String>,
    retry_config: RetryConfig,
    proxy_settings: Option<&ProxySettings>,
) -> Arc<dyn ApiProvider> {
    let client = build_client(proxy_settings);

    match provider_type {
        LlmProvider::Anthropic => Arc::new(AnthropicApiProvider::new(retry_config, client)),
        LlmProvider::OpenAi => {
            let is_local = custom_endpoint
                .as_deref()
                .map(|e| e.contains("localhost") || e.contains("127.0.0.1") || e.contains("[::1]"))
                .unwrap_or(false);
            Arc::new(OpenAiApiProvider::new(
                custom_endpoint,
                retry_config,
                client,
                is_local,
            ))
        }
        LlmProvider::DeepSeek => Arc::new(OpenAiApiProvider::with_identity(
            custom_endpoint.or(Some("https://api.deepseek.com/v1".into())),
            retry_config,
            client,
            LlmProvider::DeepSeek,
        )),
        LlmProvider::Groq => Arc::new(OpenAiApiProvider::with_identity(
            custom_endpoint.or(Some("https://api.groq.com/openai/v1".into())),
            retry_config,
            client,
            LlmProvider::Groq,
        )),
        LlmProvider::OpenRouter => Arc::new(OpenAiApiProvider::with_identity(
            custom_endpoint.or(Some("https://openrouter.ai/api/v1".into())),
            retry_config,
            client,
            LlmProvider::OpenRouter,
        )),
        LlmProvider::Gemini => Arc::new(GeminiApiProvider::new(retry_config, client)),
        LlmProvider::Local | LlmProvider::Ollama => Arc::new(OllamaApiProvider::new(
            custom_endpoint,
            retry_config,
            client,
        )),
        LlmProvider::Codex => Arc::new(CodexApiProvider::new(retry_config, None, None)),
        _ => Arc::new(AnthropicApiProvider::new(retry_config, client)),
    }
}

static CLIENT_CACHE: Mutex<Option<(ProxySettings, Client)>> = Mutex::new(None);

pub fn build_client(proxy_settings: Option<&ProxySettings>) -> Client {
    let current_settings = proxy_settings.cloned().unwrap_or_default();

    if let Ok(cache) = CLIENT_CACHE.lock() {
        if let Some((cached_settings, client)) = cache.as_ref() {
            if cached_settings == &current_settings {
                return client.clone();
            }
        }
    }

    let mut builder = Client::builder();

    if current_settings.enabled {
        if let Some(http_proxy) = &current_settings.http_proxy
            && !http_proxy.is_empty()
            && let Ok(proxy) = reqwest::Proxy::http(http_proxy)
        {
            builder = builder.proxy(proxy);
        }
        if let Some(https_proxy) = &current_settings.https_proxy
            && !https_proxy.is_empty()
            && let Ok(proxy) = reqwest::Proxy::https(https_proxy)
        {
            builder = builder.proxy(proxy);
        }
        if let Some(all_proxy) = &current_settings.all_proxy
            && !all_proxy.is_empty()
            && let Ok(proxy) = reqwest::Proxy::all(all_proxy)
        {
            builder = builder.proxy(proxy);
        }
    }

    let client = builder.build().unwrap_or_else(|_| Client::new());

    if let Ok(mut cache) = CLIENT_CACHE.lock() {
        *cache = Some((current_settings, client.clone()));
    }

    client
}
