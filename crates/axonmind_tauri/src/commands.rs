//! Tauri command handlers — one per `AxonMindTransport` method.
//! All commands return `Result<T, String>`; the String error is forwarded to the frontend.
use crate::lifecycle::EngineState;
use axonmind_core::NodeId;
use axonmind_engine::{
    ingest::{IngestOptions, IngestSource, IngestSummary},
    query::{
        ExplainKpiInput, ExplainKpiOutput, FocusKpiInput, FocusKpiOutput, GetEvidenceInput,
        GetEvidenceOutput, GraphExportV1, GraphSearchInput, GraphSearchOutput, ImpactRadiusInput,
        ImpactRadiusOutput, ReasoningSearchInput, ReasoningSearchOutput, SuggestActionsInput,
        SuggestActionsOutput, TraceDecisionInput, TraceDecisionOutput,
    },
    store::{
        DocumentSummary,
        generations::{GenerationId, GenerationSummary},
    },
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::State;

// ── Query commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn focus_kpi(
    state: State<'_, EngineState>,
    input: FocusKpiInput,
) -> Result<FocusKpiOutput, String> {
    state.0.focus_kpi(input).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn explain_kpi(
    state: State<'_, EngineState>,
    input: ExplainKpiInput,
) -> Result<ExplainKpiOutput, String> {
    state.0.explain_kpi(input).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_evidence(
    state: State<'_, EngineState>,
    input: GetEvidenceInput,
) -> Result<GetEvidenceOutput, String> {
    state.0.get_evidence(input).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn impact_radius(
    state: State<'_, EngineState>,
    input: ImpactRadiusInput,
) -> Result<ImpactRadiusOutput, String> {
    state
        .0
        .impact_radius(input)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn trace_decision(
    state: State<'_, EngineState>,
    input: TraceDecisionInput,
) -> Result<TraceDecisionOutput, String> {
    state
        .0
        .trace_decision(input)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn suggest_actions(
    state: State<'_, EngineState>,
    input: SuggestActionsInput,
) -> Result<SuggestActionsOutput, String> {
    state
        .0
        .suggest_actions(input)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn graph_search(
    state: State<'_, EngineState>,
    input: GraphSearchInput,
) -> Result<GraphSearchOutput, String> {
    state.0.graph_search(input).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reasoning_search(
    state: State<'_, EngineState>,
    input: ReasoningSearchInput,
) -> Result<ReasoningSearchOutput, String> {
    state
        .0
        .reasoning_search(input)
        .await
        .map_err(|e| e.to_string())
}

// ── Ingest commands ───────────────────────────────────────────────────────────

/// Index a file or directory path on disk.
#[tauri::command]
pub async fn index_path(
    state: State<'_, EngineState>,
    path: String,
    recursive: bool,
    skip_unchanged: bool,
) -> Result<IngestSummary, String> {
    let p = PathBuf::from(&path);
    let source = if p.is_dir() {
        IngestSource::Directory(p)
    } else {
        IngestSource::File(p)
    };
    let opts = IngestOptions {
        recursive,
        skip_unchanged,
        max_file_size_bytes: 50 * 1024 * 1024,
    };
    state
        .0
        .ingest_sync(source, opts)
        .await
        .map_err(|e| e.to_string())
}

/// Index pre-processed Markdown text (soverex / Next.js path).
#[tauri::command]
pub async fn index_markdown(
    state: State<'_, EngineState>,
    text: String,
    source_path: Option<String>,
    sha256: Option<String>,
) -> Result<IngestSummary, String> {
    let skip = sha256.is_some();
    let source = IngestSource::Markdown {
        text,
        source_path: source_path.map(PathBuf::from),
        sha256,
    };
    let opts = IngestOptions {
        recursive: false,
        skip_unchanged: skip,
        max_file_size_bytes: 0,
    };
    state
        .0
        .ingest_sync(source, opts)
        .await
        .map_err(|e| e.to_string())
}

// ── Document management commands ──────────────────────────────────────────────

#[tauri::command]
pub async fn list_documents(state: State<'_, EngineState>) -> Result<Vec<DocumentSummary>, String> {
    state.0.list_documents().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_document(state: State<'_, EngineState>, node_id: String) -> Result<(), String> {
    state
        .0
        .remove_document(NodeId(node_id), true)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn regenerate_document(
    state: State<'_, EngineState>,
    node_id: String,
) -> Result<IngestSummary, String> {
    state
        .0
        .regenerate_document(NodeId(node_id))
        .await
        .map_err(|e| e.to_string())
}

// ── Generation commands ───────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_generation_from_paths(
    state: State<'_, EngineState>,
    name: String,
    paths: Vec<String>,
) -> Result<String, String> {
    state
        .0
        .create_generation_from_paths(name, paths)
        .await
        .map(|id| id.0)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_generations(
    state: State<'_, EngineState>,
) -> Result<Vec<GenerationSummary>, String> {
    state.0.list_generations().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn export_generation(
    state: State<'_, EngineState>,
    gen_id: String,
) -> Result<GraphExportV1, String> {
    state
        .0
        .export_generation(GenerationId(gen_id))
        .await
        .map_err(|e| e.to_string())
}

// ── Host filesystem utilities (not part of AxonMindTransport) ────────────────

#[derive(Serialize)]
pub struct DirFileEntry {
    pub path: String,
    pub supported: bool,
    pub reject_reason: Option<String>,
}

const SUPPORTED_EXTS: &[&str] = &[
    "md", "markdown", "txt", "csv", "xlsx", "docx", "pdf", "pptx",
];

#[derive(Debug, Clone)]
enum RuntimeProviderSource {
    None,
    ApiKey {
        id: String,
    },
    Cli {
        cli: String,
        model: Option<String>,
        intelligence: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PersistedProviderSelection {
    ApiKey {
        id: String,
    },
    Cli {
        cli: String,
        model: Option<String>,
        intelligence: Option<String>,
    },
}

// Tracks which source last set the in-memory provider.
// This is process-local by design and resets on app restart.
static RUNTIME_PROVIDER_SOURCE: Mutex<RuntimeProviderSource> =
    Mutex::new(RuntimeProviderSource::None);

fn set_runtime_provider_source(source: RuntimeProviderSource) {
    if let Ok(mut guard) = RUNTIME_PROVIDER_SOURCE.lock() {
        *guard = source;
    }
}

fn get_runtime_provider_source() -> RuntimeProviderSource {
    RUNTIME_PROVIDER_SOURCE
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or(RuntimeProviderSource::None)
}
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".git",
    ".svn",
    "__pycache__",
];

fn check_ext(path: &Path) -> (bool, Option<String>) {
    match path.extension().and_then(|e| e.to_str()) {
        None => (false, Some("no file extension".into())),
        Some(ext) => {
            let low = ext.to_lowercase();
            if SUPPORTED_EXTS.contains(&low.as_str()) {
                (true, None)
            } else {
                (false, Some(format!(".{} not supported", low)))
            }
        }
    }
}

fn walk_into(dir: &Path, out: &mut Vec<DirFileEntry>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    children.sort_by_key(|e| e.file_name());
    for entry in children {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_str().unwrap_or("");
        if name_str.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if !SKIP_DIRS.contains(&name_str) {
                walk_into(&path, out);
            }
        } else if meta.is_file() {
            let (supported, reject_reason) = check_ext(&path);
            out.push(DirFileEntry {
                path: path.to_string_lossy().into_owned(),
                supported,
                reject_reason,
            });
        }
    }
}

#[tauri::command]
pub async fn list_dir_files(path: String) -> Result<Vec<DirFileEntry>, String> {
    let p = Path::new(&path);
    let mut out = Vec::new();
    if p.is_dir() {
        walk_into(p, &mut out);
    } else {
        let (supported, reject_reason) = check_ext(p);
        out.push(DirFileEntry {
            path,
            supported,
            reject_reason,
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn read_file_text(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("cannot read: {}", e))
}

// ── API key management commands ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ApiKeyConfig {
    pub id: String,
    pub name: String,
    pub provider: String, // "openai" | "ollama" | "lm_studio" | "custom"
    pub base_url: String,
    pub model: String,
    pub is_active: bool,
    pub key_masked: String,
}

const KEYRING_SERVICE: &str = "axonmind-open";
const CONFIGS_KEY_LEGACY: &str = "__api_configs__";
const CODEX_SESSION_OPTIONS_FILENAME: &str = "codex_session_options.json";
const DEFAULT_CODEX_SESSION_OPTIONS_JSON: &str =
    include_str!("../../../codex_session_options.example.json");

#[derive(Debug, Clone, Deserialize)]
struct CodexSessionOptionsConfig {
    models: Option<Vec<String>>,
    intelligence_levels: Option<Vec<String>>,
    default_model: Option<String>,
    default_intelligence: Option<String>,
}

fn config_file_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        if let Some(base) = std::env::var_os("APPDATA").map(PathBuf::from) {
            return Ok(base.join("axonmind-open").join("api_configs.json"));
        }
    }

    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        return Ok(base.join("axonmind-open").join("api_configs.json"));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        return Ok(home
            .join(".config")
            .join("axonmind-open")
            .join("api_configs.json"));
    }

    Err("cannot resolve config directory for api configs".to_string())
}

fn provider_selection_file_path() -> Result<PathBuf, String> {
    let configs = config_file_path()?;
    let parent = configs
        .parent()
        .ok_or_else(|| "cannot resolve config directory for provider selection".to_string())?;
    Ok(parent.join("provider_selection.json"))
}

fn codex_session_options_file_path() -> Result<PathBuf, String> {
    let configs = config_file_path()?;
    let parent = configs
        .parent()
        .ok_or_else(|| "cannot resolve config directory for codex options".to_string())?;
    Ok(parent.join(CODEX_SESSION_OPTIONS_FILENAME))
}

fn normalize_non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_string_list(values: Option<Vec<String>>) -> Vec<String> {
    let mut normalized = Vec::new();
    if let Some(items) = values {
        for item in items {
            if let Some(trimmed) = normalize_non_empty(item)
                && !normalized.contains(&trimmed)
            {
                normalized.push(trimmed);
            }
        }
    }
    normalized
}

fn load_codex_session_options_config() -> Option<CodexSessionOptionsConfig> {
    let path = codex_session_options_file_path().ok()?;
    if !path.exists() {
        return serde_json::from_str::<CodexSessionOptionsConfig>(
            DEFAULT_CODEX_SESSION_OPTIONS_JSON,
        )
        .ok();
    }

    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!(
                "axonmind: read codex session options config failed ({}): {e}",
                path.display()
            );
            return serde_json::from_str::<CodexSessionOptionsConfig>(
                DEFAULT_CODEX_SESSION_OPTIONS_JSON,
            )
            .ok();
        }
    };

    match serde_json::from_str::<CodexSessionOptionsConfig>(&raw) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            eprintln!(
                "axonmind: decode codex session options config failed ({}): {e}",
                path.display()
            );
            serde_json::from_str::<CodexSessionOptionsConfig>(DEFAULT_CODEX_SESSION_OPTIONS_JSON)
                .ok()
        }
    }
}

fn load_configs_from_file() -> Result<Vec<ApiKeyConfig>, String> {
    let path = config_file_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read configs: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("decode configs: {e}"))
}

fn persist_configs_to_file(configs: &[ApiKeyConfig]) -> Result<(), String> {
    let path = config_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }
    let raw = serde_json::to_string(configs).map_err(|e| e.to_string())?;
    std::fs::write(&path, raw).map_err(|e| format!("write configs: {e}"))
}

fn load_persisted_provider_selection() -> Option<PersistedProviderSelection> {
    let path = provider_selection_file_path().ok()?;
    if !path.exists() {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn persist_provider_selection(
    selection: Option<&PersistedProviderSelection>,
) -> Result<(), String> {
    let path = provider_selection_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }

    match selection {
        Some(sel) => {
            let raw = serde_json::to_string(sel).map_err(|e| e.to_string())?;
            std::fs::write(path, raw).map_err(|e| format!("write provider selection: {e}"))
        }
        None => {
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| format!("clear provider selection: {e}"))
            } else {
                Ok(())
            }
        }
    }
}

fn migrate_legacy_configs_if_needed() {
    let Ok(path) = config_file_path() else {
        return;
    };
    if path.exists() {
        return;
    }

    let mut configs_from_legacy: Vec<ApiKeyConfig> = vec![];
    let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, CONFIGS_KEY_LEGACY) else {
        let _ = persist_configs_to_file(&configs_from_legacy);
        return;
    };

    if let Ok(raw) = entry.get_password() {
        if let Ok(configs) = serde_json::from_str::<Vec<ApiKeyConfig>>(&raw) {
            configs_from_legacy = configs;
        }
    }

    if let Err(e) = persist_configs_to_file(&configs_from_legacy) {
        eprintln!("axonmind: failed to migrate legacy api configs: {e}");
        return;
    }

    // Best-effort cleanup: once local config exists, we should never read legacy metadata again.
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, CONFIGS_KEY_LEGACY) {
        let _ = entry.delete_password();
    }
}

fn load_configs() -> Vec<ApiKeyConfig> {
    migrate_legacy_configs_if_needed();
    match load_configs_from_file() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("axonmind: load api configs failed: {e}");
            vec![]
        }
    }
}

fn persist_configs(configs: &[ApiKeyConfig]) -> Result<(), String> {
    persist_configs_to_file(configs)
}

fn store_key(id: &str, key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, id).map_err(|e| e.to_string())?;
    entry.set_password(key).map_err(|e| e.to_string())
}

fn fetch_key(id: &str) -> Result<String, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, id).map_err(|e| e.to_string())?;
    entry.get_password().map_err(|e| e.to_string())
}

fn erase_key(id: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, id) {
        let _ = entry.delete_password();
    }
}

fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len());
    }
    let prefix: String = chars[..4].iter().collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{}...{}", prefix, suffix)
}

fn build_cli_provider(
    cli: &str,
    profile: &crate::cli_auth::CliProfile,
    model: Option<String>,
    intelligence: Option<String>,
) -> std::sync::Arc<dyn axonmind_engine::extract::llm::LlmProvider> {
    if cli == "codex" {
        let selected_model = model
            .clone()
            .unwrap_or_else(|| seeyoo_llm::codex_api::default_codex_model().to_string());
        let selected_intelligence = intelligence.clone().or(Some(
            seeyoo_llm::codex_api::default_codex_intelligence().to_string(),
        ));
        let provider = seeyoo_llm::codex_api::CodexApiProvider::new(
            seeyoo_llm::types::RetryConfig::default(),
            Some(selected_model.clone()),
            selected_intelligence,
        );
        let adapter = axonmind_engine::extract::seeyoo::SeeyooAdapter::new(
            std::sync::Arc::new(provider),
            String::new(),
            Some(selected_model),
        );
        return std::sync::Arc::new(adapter);
    }

    let selected_model =
        model.unwrap_or_else(|| crate::cli_auth::default_model_for(cli).to_string());
    std::sync::Arc::new(axonmind_engine::extract::openai::OpenAiProvider::new(
        crate::cli_auth::base_url_for(cli),
        &profile.access_token,
        &selected_model,
    ))
}

/// Rebuild the active LLM provider from persisted config — called at startup so the engine's
/// in-memory provider matches the `is_active` config that the UI shows. Returns `None` when no
/// key is active. When an active key exists but can't be read from the keyring, this logs rather
/// than failing silently, so a missing-provider state is never invisible.
pub(crate) fn build_active_provider()
-> Option<std::sync::Arc<dyn axonmind_engine::extract::llm::LlmProvider>> {
    if let Some(selection) = load_persisted_provider_selection() {
        match selection {
            PersistedProviderSelection::ApiKey { id } => {
                if let Some(config) = load_configs().into_iter().find(|c| c.id == id) {
                    match fetch_key(&id) {
                        Ok(key) => {
                            set_runtime_provider_source(RuntimeProviderSource::ApiKey {
                                id: id.clone(),
                            });
                            return Some(std::sync::Arc::new(
                                axonmind_engine::extract::openai::OpenAiProvider::new(
                                    &config.base_url,
                                    &key,
                                    &config.model,
                                ),
                            ));
                        }
                        Err(e) => {
                            eprintln!(
                                "axonmind: persisted API key '{}' could not be loaded from keyring: {e}",
                                config.name
                            );
                        }
                    }
                }
            }
            PersistedProviderSelection::Cli {
                cli,
                model,
                intelligence,
            } => {
                let sessions = crate::cli_auth::detect_all();
                let profile = match cli.as_str() {
                    "codex" => sessions.codex,
                    "claude" => sessions.claude,
                    "antigravity" => sessions.antigravity,
                    _ => None,
                };
                if let Some(profile) = profile {
                    let provider =
                        build_cli_provider(&cli, &profile, model.clone(), intelligence.clone());
                    set_runtime_provider_source(RuntimeProviderSource::Cli {
                        cli,
                        model,
                        intelligence,
                    });
                    return Some(provider);
                }
            }
        }
    }

    let config = load_configs().into_iter().find(|c| c.is_active)?;
    match fetch_key(&config.id) {
        Ok(key) => {
            set_runtime_provider_source(RuntimeProviderSource::ApiKey {
                id: config.id.clone(),
            });
            Some(std::sync::Arc::new(
                axonmind_engine::extract::openai::OpenAiProvider::new(
                    &config.base_url,
                    &key,
                    &config.model,
                ),
            ))
        }
        Err(e) => {
            eprintln!(
                "axonmind: active API key '{}' could not be loaded from keyring: {e}",
                config.name
            );
            set_runtime_provider_source(RuntimeProviderSource::None);
            None
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveKeyStatus {
    pub has_active_key: bool,
    pub active_key_id: Option<String>,
}

#[tauri::command]
pub async fn has_active_api_key(state: State<'_, EngineState>) -> Result<ActiveKeyStatus, String> {
    let active = load_configs().into_iter().find(|c| c.is_active);
    Ok(ActiveKeyStatus {
        has_active_key: state.0.has_llm_provider().await,
        active_key_id: active.map(|c| c.id),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeProviderStatus {
    pub has_provider: bool,
    pub source: String, // "none" | "api_key" | "cli" | "unknown"
    pub label: Option<String>,
}

#[tauri::command]
pub async fn get_runtime_provider_status(
    state: State<'_, EngineState>,
) -> Result<RuntimeProviderStatus, String> {
    let has_provider = state.0.has_llm_provider().await;
    if !has_provider {
        return Ok(RuntimeProviderStatus {
            has_provider: false,
            source: "none".to_string(),
            label: None,
        });
    }

    let status = match get_runtime_provider_source() {
        RuntimeProviderSource::ApiKey { id } => {
            let label = load_configs()
                .into_iter()
                .find(|c| c.id == id)
                .map(|c| c.name)
                .or(Some(id));
            RuntimeProviderStatus {
                has_provider: true,
                source: "api_key".to_string(),
                label,
            }
        }
        RuntimeProviderSource::Cli {
            cli,
            model,
            intelligence,
        } => {
            let base = match cli.as_str() {
                "codex" => "ChatGPT (Codex CLI)".to_string(),
                "claude" => "Claude (Claude Code CLI)".to_string(),
                "antigravity" => "Gemini (Antigravity CLI)".to_string(),
                _ => cli,
            };
            let mut suffix = String::new();
            if let Some(m) = model.as_deref() {
                suffix.push_str(&format!(" · {m}"));
            }
            if let Some(i) = intelligence.as_deref() {
                suffix.push_str(&format!(" · {i}"));
            }
            let label = Some(format!("{base}{suffix}"));
            RuntimeProviderStatus {
                has_provider: true,
                source: "cli".to_string(),
                label,
            }
        }
        RuntimeProviderSource::None => RuntimeProviderStatus {
            has_provider: true,
            source: "unknown".to_string(),
            label: None,
        },
    };
    Ok(status)
}

#[tauri::command]
pub async fn list_api_keys() -> Result<Vec<ApiKeyConfig>, String> {
    Ok(load_configs())
}

#[tauri::command]
pub async fn save_api_key(
    id: Option<String>,
    name: String,
    provider: String,
    base_url: String,
    model: String,
    api_key: String,
) -> Result<ApiKeyConfig, String> {
    let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    store_key(&id, &api_key)?;

    let mut configs = load_configs();
    let masked = mask_key(&api_key);

    if let Some(existing) = configs.iter_mut().find(|c| c.id == id) {
        existing.name = name.clone();
        existing.provider = provider.clone();
        existing.base_url = base_url.clone();
        existing.model = model.clone();
        existing.key_masked = masked.clone();
        let result = existing.clone();
        persist_configs(&configs)?;
        Ok(result)
    } else {
        let config = ApiKeyConfig {
            id,
            name,
            provider,
            base_url,
            model,
            is_active: false,
            key_masked: masked,
        };
        configs.push(config.clone());
        persist_configs(&configs)?;
        Ok(config)
    }
}

#[tauri::command]
pub async fn delete_api_key(id: String, state: State<'_, EngineState>) -> Result<(), String> {
    let mut configs = load_configs();
    let was_active = configs.iter().any(|c| c.id == id && c.is_active);
    configs.retain(|c| c.id != id);
    erase_key(&id);
    persist_configs(&configs)?;
    if was_active {
        state.0.update_llm_provider(None).await;
        set_runtime_provider_source(RuntimeProviderSource::None);
        let _ = persist_provider_selection(None);
    } else if matches!(
        load_persisted_provider_selection(),
        Some(PersistedProviderSelection::ApiKey { id: ref selected_id }) if selected_id == &id
    ) {
        let _ = persist_provider_selection(None);
    }
    Ok(())
}

#[tauri::command]
pub async fn set_active_provider(
    id: Option<String>,
    state: State<'_, EngineState>,
) -> Result<(), String> {
    let mut configs = load_configs();
    for c in &mut configs {
        c.is_active = false;
    }

    if let Some(ref target_id) = id {
        let config = configs
            .iter_mut()
            .find(|c| &c.id == target_id)
            .ok_or_else(|| format!("API key '{}' not found", target_id))?;
        config.is_active = true;

        let key = fetch_key(target_id)?;
        let provider = axonmind_engine::extract::openai::OpenAiProvider::new(
            &config.base_url,
            &key,
            &config.model,
        );
        persist_configs(&configs)?;
        state
            .0
            .update_llm_provider(Some(std::sync::Arc::new(provider)))
            .await;
        set_runtime_provider_source(RuntimeProviderSource::ApiKey {
            id: target_id.clone(),
        });
        let _ = persist_provider_selection(Some(&PersistedProviderSelection::ApiKey {
            id: target_id.clone(),
        }));
    } else {
        persist_configs(&configs)?;
        state.0.update_llm_provider(None).await;
        set_runtime_provider_source(RuntimeProviderSource::None);
        let _ = persist_provider_selection(None);
    }
    Ok(())
}

// ── CLI session detection ─────────────────────────────────────────────────────

/// Return detected CLI sessions (Codex, Claude Code, Antigravity) so the UI can
/// offer "use your existing account" without requiring a manual API key paste.
/// Safe to call at startup — only reads small local files, never makes network calls.
#[tauri::command]
pub async fn detect_cli_sessions() -> Result<crate::cli_auth::CliSessions, String> {
    Ok(crate::cli_auth::detect_all())
}

#[derive(Debug, Clone, Serialize)]
pub struct CliSessionOptions {
    pub models: Vec<String>,
    pub intelligence_levels: Vec<String>,
    pub default_model: Option<String>,
    pub default_intelligence: Option<String>,
}

/// Return optional model/intelligence choices for each CLI provider.
#[tauri::command]
pub async fn get_cli_session_options(cli: String) -> Result<CliSessionOptions, String> {
    if cli == "codex" {
        let cfg = load_codex_session_options_config();
        let models = normalize_string_list(cfg.as_ref().and_then(|c| c.models.clone()));

        let intelligence_levels = {
            let configured =
                normalize_string_list(cfg.as_ref().and_then(|c| c.intelligence_levels.clone()));
            if configured.is_empty() {
                seeyoo_llm::codex_api::CODEX_INTELLIGENCE_LEVELS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            } else {
                configured
            }
        };

        let default_model = cfg
            .as_ref()
            .and_then(|c| c.default_model.clone())
            .and_then(normalize_non_empty)
            .or(Some(
                seeyoo_llm::codex_api::default_codex_model().to_string(),
            ));

        let default_intelligence = cfg
            .as_ref()
            .and_then(|c| c.default_intelligence.clone())
            .and_then(normalize_non_empty)
            .filter(|v| intelligence_levels.iter().any(|level| level == v))
            .or(Some(
                seeyoo_llm::codex_api::default_codex_intelligence().to_string(),
            ));

        return Ok(CliSessionOptions {
            models,
            intelligence_levels,
            default_model,
            default_intelligence,
        });
    }

    Ok(CliSessionOptions {
        models: vec![crate::cli_auth::default_model_for(&cli).to_string()],
        intelligence_levels: vec![],
        default_model: Some(crate::cli_auth::default_model_for(&cli).to_string()),
        default_intelligence: None,
    })
}

/// Activate a detected CLI session as the current LLM provider.
/// `cli` must be "codex", "claude", or "antigravity".
#[tauri::command]
pub async fn use_cli_session(
    cli: String,
    model: Option<String>,
    intelligence: Option<String>,
    state: State<'_, EngineState>,
) -> Result<(), String> {
    let sessions = crate::cli_auth::detect_all();
    let profile = match cli.as_str() {
        "codex" => sessions.codex,
        "claude" => sessions.claude,
        "antigravity" => sessions.antigravity,
        other => return Err(format!("unknown cli: {other}")),
    };
    let profile = profile.ok_or_else(|| format!("no active {cli} session found"))?;

    if cli == "codex" {
        let selected_model =
            model.unwrap_or_else(|| seeyoo_llm::codex_api::default_codex_model().to_string());
        let selected_intelligence = intelligence.or(Some(
            seeyoo_llm::codex_api::default_codex_intelligence().to_string(),
        ));
        let adapter = build_cli_provider(
            "codex",
            &profile,
            Some(selected_model.clone()),
            selected_intelligence.clone(),
        );
        state.0.update_llm_provider(Some(adapter)).await;
        set_runtime_provider_source(RuntimeProviderSource::Cli {
            cli: cli.clone(),
            model: Some(selected_model.clone()),
            intelligence: selected_intelligence.clone(),
        });
        let _ = persist_provider_selection(Some(&PersistedProviderSelection::Cli {
            cli,
            model: Some(selected_model.clone()),
            intelligence: selected_intelligence,
        }));
        return Ok(());
    }

    let selected_model =
        model.unwrap_or_else(|| crate::cli_auth::default_model_for(&cli).to_string());
    let provider = build_cli_provider(&cli, &profile, Some(selected_model.clone()), None);
    state.0.update_llm_provider(Some(provider)).await;
    set_runtime_provider_source(RuntimeProviderSource::Cli {
        cli: cli.clone(),
        model: Some(selected_model.clone()),
        intelligence: None,
    });
    let _ = persist_provider_selection(Some(&PersistedProviderSelection::Cli {
        cli,
        model: Some(selected_model),
        intelligence: None,
    }));
    Ok(())
}

// ── Maintenance commands ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn export_json(state: State<'_, EngineState>) -> Result<GraphExportV1, String> {
    state.0.export_json().await.map_err(|e| e.to_string())
}

/// Phase 1 brain map: ≤10 LLM-suggested categories (or kind-based fallback) over the live graph.
/// `doc_ids` scopes the summary to those documents' concepts; omit/empty for the whole graph.
/// `scoped_mode` controls scoped cache usage: auto (default), cached_only, regenerate.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopedSummaryModeInput {
    Auto,
    CachedOnly,
    Regenerate,
}

#[tauri::command]
pub async fn suggest_summary(
    state: State<'_, EngineState>,
    doc_ids: Option<Vec<String>>,
    scoped_mode: Option<ScopedSummaryModeInput>,
) -> Result<axonmind_engine::extract::summarize::SuggestedSummary, String> {
    let ids = doc_ids.map(|v| v.into_iter().map(NodeId).collect());
    let mode = match scoped_mode.unwrap_or(ScopedSummaryModeInput::Auto) {
        ScopedSummaryModeInput::Auto => axonmind_engine::brain_map::ScopedSummaryMode::Auto,
        ScopedSummaryModeInput::CachedOnly => {
            axonmind_engine::brain_map::ScopedSummaryMode::CachedOnly
        }
        ScopedSummaryModeInput::Regenerate => {
            axonmind_engine::brain_map::ScopedSummaryMode::Regenerate
        }
    };
    state
        .0
        .suggest_summary_with_mode(ids, mode)
        .await
        .map_err(|e| e.to_string())
}

/// Return persisted default brain-map config and computed effective contexts.
#[tauri::command]
pub async fn get_brain_map_default_config(
    state: State<'_, EngineState>,
) -> Result<axonmind_engine::brain_map::SummaryConfigSnapshot, String> {
    state
        .0
        .get_brain_map_default_config()
        .await
        .map_err(|e| e.to_string())
}

/// Apply safe user edits to the persisted default brain-map config.
#[tauri::command]
pub async fn update_brain_map_default_config(
    state: State<'_, EngineState>,
    edit: axonmind_engine::brain_map::SummaryConfigEdit,
) -> Result<axonmind_engine::brain_map::SummaryConfigSnapshot, String> {
    state
        .0
        .update_brain_map_default_config(edit)
        .await
        .map_err(|e| e.to_string())
}

/// Restore default brain-map config by regenerating it from the current graph.
#[tauri::command]
pub async fn restore_brain_map_default_config(
    state: State<'_, EngineState>,
) -> Result<axonmind_engine::brain_map::SummaryConfigSnapshot, String> {
    state
        .0
        .restore_brain_map_default_config()
        .await
        .map_err(|e| e.to_string())
}

/// Resolve top-level lens headline values for the persisted default brain-map summary.
#[tauri::command]
pub async fn resolve_brain_map_default_summary(
    state: State<'_, EngineState>,
    doc_ids: Option<Vec<String>>,
) -> Result<axonmind_engine::brain_map::SummaryResolution, String> {
    let ids = doc_ids.map(|v| v.into_iter().map(NodeId).collect());
    state
        .0
        .resolve_brain_map_default_summary(ids)
        .await
        .map_err(|e| e.to_string())
}

/// Resolve direct child lenses for a parent lens id in the default (unscoped) summary.
#[tauri::command]
pub async fn resolve_brain_map_lens_children(
    state: State<'_, EngineState>,
    parent_lens_id: String,
) -> Result<Vec<axonmind_engine::brain_map::LensResolution>, String> {
    state
        .0
        .resolve_brain_map_lens_children(parent_lens_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rebuild_search_index(state: State<'_, EngineState>) -> Result<(), String> {
    state
        .0
        .rebuild_search_index()
        .await
        .map_err(|e| e.to_string())
}
