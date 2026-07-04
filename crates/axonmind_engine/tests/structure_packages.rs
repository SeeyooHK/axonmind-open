use axonmind_engine::{AxonMindEngine, config::EngineConfig};
use tempfile::TempDir;

fn test_engine_config(dir: &TempDir) -> EngineConfig {
    EngineConfig::from_workspace_dir(dir.path().join("workspace"))
}

#[tokio::test]
async fn installs_and_lists_structure_package() {
    let temp = TempDir::new().expect("tempdir");
    let cfg = test_engine_config(&temp);
    let engine = AxonMindEngine::open(cfg).await.expect("engine");
    let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../soverex-open/docs/legal_agent/structure");

    let report = engine
        .install_structure_package_from_dir(&package_dir, "standalone")
        .await
        .expect("install");
    assert_eq!(report.package_name, "legal-eu-privacy");
    assert_eq!(report.profiles, 3);

    let packages = engine.list_structure_packages().await.expect("list");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].package_name, "legal-eu-privacy");
    assert_eq!(packages[0].sources, vec!["standalone".to_string()]);
}
