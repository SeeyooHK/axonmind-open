/// Tauri v2 plugin adapter for axonmind-open.
///
/// Usage in host app:
/// ```ignore
/// # use axonmind_engine::config::EngineConfig;
/// let config = EngineConfig::from_workspace_dir("/tmp/ws".into());
/// tauri::Builder::default()
///     .plugin(axonmind_tauri::init(config, None))
///     .run(tauri::generate_context!())
///     .unwrap();
/// ```
#[cfg(feature = "tauri")]
pub mod cli_auth;
#[cfg(feature = "tauri")]
pub mod commands;
#[cfg(feature = "tauri")]
pub mod events;
#[cfg(feature = "tauri")]
pub mod lifecycle;

#[cfg(feature = "tauri")]
pub use lifecycle::{EngineState, init, init_host_managed};
