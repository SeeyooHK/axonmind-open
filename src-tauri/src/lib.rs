use axonmind_engine::config::{EngineConfig, WorkerConfig, WorkspaceManifest};
use std::path::PathBuf;
use tauri::Manager;

fn ensure_workspace(workspace_dir: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(workspace_dir)?;
    let manifest_path = workspace_dir.join("workspace.json");
    if !manifest_path.exists() {
        let manifest =
            WorkspaceManifest::new(uuid::Uuid::new_v4().to_string(), "default".to_string());
        std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let workspace_dir = app.path().app_data_dir()?.join("axonmind");
            ensure_workspace(&workspace_dir)?;

            let config = EngineConfig {
                workers: WorkerConfig::for_host(),
                enable_llm_extraction: true,
                ..EngineConfig::from_workspace_dir(workspace_dir)
            };

            app.handle().plugin(axonmind_tauri::init(config, None))?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
