use reqwest::Client;
use std::time::Duration;

/// A local LLM server discovered by probing well-known ports.
#[derive(Debug, Clone)]
pub struct LocalServerInfo {
    /// Pass this as `ProviderConfig::provider_type`.
    pub provider_type: &'static str,
    pub display_name: &'static str,
    /// Resolved base endpoint to pass as `ProviderConfig::custom_endpoint`.
    pub endpoint: &'static str,
}

const PROBE_TIMEOUT_MS: u64 = 300;

struct Candidate {
    provider_type: &'static str,
    display_name: &'static str,
    /// Value to use as `ProviderConfig::custom_endpoint`.
    endpoint: &'static str,
    /// URL we GET to confirm the server is live.
    probe_url: &'static str,
}

static CANDIDATES: &[Candidate] = &[
    Candidate {
        provider_type: "ollama",
        display_name: "Ollama",
        endpoint: "http://localhost:11434",
        probe_url: "http://localhost:11434/api/tags",
    },
    Candidate {
        provider_type: "lmstudio",
        display_name: "LM Studio",
        endpoint: "http://localhost:1234/v1",
        probe_url: "http://localhost:1234/v1/models",
    },
    Candidate {
        provider_type: "llamacpp",
        display_name: "llama.cpp",
        endpoint: "http://localhost:8080/v1",
        probe_url: "http://localhost:8080/v1/models",
    },
    Candidate {
        provider_type: "jan",
        display_name: "Jan",
        endpoint: "http://localhost:1337/v1",
        probe_url: "http://localhost:1337/v1/models",
    },
    Candidate {
        provider_type: "vllm",
        display_name: "vLLM",
        endpoint: "http://localhost:8000/v1",
        probe_url: "http://localhost:8000/v1/models",
    },
];

async fn probe(client: &Client, c: &'static Candidate) -> Option<LocalServerInfo> {
    let ok = client
        .get(c.probe_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    if ok {
        Some(LocalServerInfo {
            provider_type: c.provider_type,
            display_name: c.display_name,
            endpoint: c.endpoint,
        })
    } else {
        None
    }
}

/// Returns the first live local LLM server found, in priority order:
/// Ollama → LM Studio → llama.cpp → Jan → vLLM.
///
/// Use the returned `LocalServerInfo` to build a `ProviderConfig`:
/// ```ignore
/// if let Some(s) = detect_local_server().await {
///     let provider = provider_from_config(ProviderConfig {
///         provider_type:   s.provider_type.to_string(),
///         custom_endpoint: Some(s.endpoint.to_string()),
///         ..Default::default()
///     });
/// }
/// ```
pub async fn detect_local_server() -> Option<LocalServerInfo> {
    probe_all_local_servers().await.into_iter().next()
}

/// Probes all well-known local server ports concurrently and returns every live server.
/// Order of results follows `CANDIDATES` order.
pub async fn probe_all_local_servers() -> Vec<LocalServerInfo> {
    let client = Client::builder()
        .timeout(Duration::from_millis(PROBE_TIMEOUT_MS))
        .build()
        .unwrap_or_default();

    futures_util::future::join_all(CANDIDATES.iter().map(|c| probe(&client, c)))
        .await
        .into_iter()
        .flatten()
        .collect()
}
