//! Plugin lifecycle: init, engine state, teardown.
use axonmind_engine::{AxonMindEngine, config::EngineConfig, extract::llm::LlmProvider};
use std::sync::Arc;
use tauri::{Manager, Runtime, plugin::TauriPlugin};

/// Tauri-managed engine state. Commands clone the inner Arc to call async methods.
pub struct EngineState(pub Arc<AxonMindEngine>);

/// Build the Tauri plugin. Call this in the host `tauri::Builder::plugin(axonmind_tauri::init(config, None))`.
///
/// `llm_provider` is injected before the engine is placed behind `Arc`, so any host can supply
/// its own LLM backend without bypassing the plugin abstraction. Pass `None` to let the AxonMind
/// plugin rehydrate its own persisted active provider, when one exists.
pub fn init<R: Runtime>(
    config: EngineConfig,
    llm_provider: Option<Arc<dyn LlmProvider>>,
) -> TauriPlugin<R> {
    init_inner(config, llm_provider, true)
}

/// Build the plugin without reading AxonMind-managed persisted API keys.
///
/// Hosts that own credential/auth flow should use this so AxonMind only uses an explicitly
/// injected provider and otherwise stays in rule-only mode.
pub fn init_host_managed<R: Runtime>(
    config: EngineConfig,
    llm_provider: Option<Arc<dyn LlmProvider>>,
) -> TauriPlugin<R> {
    init_inner(config, llm_provider, false)
}

fn init_inner<R: Runtime>(
    config: EngineConfig,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    allow_persisted_provider: bool,
) -> TauriPlugin<R> {
    tauri::plugin::Builder::<R>::new("axonmind")
        .setup(move |app, _api| {
            let mut engine = tauri::async_runtime::block_on(AxonMindEngine::open(config))?;

            // Prefer a host-injected provider; otherwise optionally rehydrate the persisted
            // active key so standalone AxonMind can keep LLM extraction across restarts.
            if let Some(provider) = llm_provider {
                engine.set_llm_provider(provider);
            } else if allow_persisted_provider {
                if let Some(provider) = crate::commands::build_active_provider() {
                    engine.set_llm_provider(provider);
                }
            } else {
                tracing::info!(
                    "AxonMind initialized without host LLM provider; using rule-only mode"
                );
            }

            let engine = Arc::new(engine);

            crate::events::spawn_event_forwarder(app.clone(), engine.subscribe());

            app.manage(EngineState(engine));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            crate::commands::focus_kpi,
            crate::commands::explain_kpi,
            crate::commands::get_evidence,
            crate::commands::impact_radius,
            crate::commands::trace_decision,
            crate::commands::suggest_actions,
            crate::commands::graph_search,
            crate::commands::index_path,
            crate::commands::index_markdown,
            crate::commands::list_documents,
            crate::commands::remove_document,
            crate::commands::regenerate_document,
            crate::commands::export_json,
            crate::commands::suggest_summary,
            crate::commands::get_brain_map_default_config,
            crate::commands::update_brain_map_default_config,
            crate::commands::restore_brain_map_default_config,
            crate::commands::resolve_brain_map_default_summary,
            crate::commands::resolve_brain_map_lens_children,
            crate::commands::rebuild_search_index,
            crate::commands::create_generation_from_paths,
            crate::commands::list_generations,
            crate::commands::export_generation,
            crate::commands::list_dir_files,
            crate::commands::read_file_text,
            crate::commands::list_api_keys,
            crate::commands::has_active_api_key,
            crate::commands::get_runtime_provider_status,
            crate::commands::save_api_key,
            crate::commands::delete_api_key,
            crate::commands::set_active_provider,
            crate::commands::detect_cli_sessions,
            crate::commands::get_cli_session_options,
            crate::commands::use_cli_session,
        ])
        .build()
}
