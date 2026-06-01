use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Root directory for all workspace data (DB, blobs, manifest).
    /// Default: `~/.axonmind/workspaces/<workspace_id>/`
    pub workspace_dir: PathBuf,
    /// Path to the SQLite database file.
    /// Default: `<workspace_dir>/axonmind.db`
    pub database_path: PathBuf,
    /// Content-addressed blob store for all ingested files.
    /// Default: `<workspace_dir>/blobs/`
    /// Files are stored as `<blob_dir>/<sha256>`. Never user-controlled paths.
    pub blob_dir: PathBuf,
    pub enable_llm_extraction: bool,
    pub llm: LlmConfig,
    pub event_buffer: usize,
    pub workers: WorkerConfig,
}

impl EngineConfig {
    /// Construct a config rooted at the given workspace directory.
    pub fn from_workspace_dir(workspace_dir: PathBuf) -> Self {
        Self {
            database_path: workspace_dir.join("axonmind.db"),
            blob_dir: workspace_dir.join("blobs"),
            workspace_dir,
            enable_llm_extraction: false,
            llm: LlmConfig::default(),
            event_buffer: 1024,
            workers: WorkerConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LlmConfig {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

/// Worker on/off toggles and intervals.
///
/// Default policy:
/// - CLI: workers off unless a command explicitly invokes them.
/// - Tauri/minimal host: workers on.
/// - Tests: workers off unless `WorkerConfig::for_tests()` is used.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub enable_discovery_worker: bool,
    pub enable_recompute_worker: bool,
    pub discovery_interval: Duration,
    pub recompute_interval: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            enable_discovery_worker: false,
            enable_recompute_worker: false,
            discovery_interval: Duration::from_secs(86_400),
            recompute_interval: Duration::from_secs(300),
        }
    }
}

impl WorkerConfig {
    pub fn for_host() -> Self {
        Self {
            enable_discovery_worker: true,
            enable_recompute_worker: true,
            ..Default::default()
        }
    }
}

/// Persisted at `<workspace_dir>/workspace.json`.
/// Read on `AxonMindEngine::open`; written by `axonmind init`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub engine_version: String,
    pub schema_version: u32,
}

impl WorkspaceManifest {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            created_at: Utc::now(),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: 1,
        }
    }
}
