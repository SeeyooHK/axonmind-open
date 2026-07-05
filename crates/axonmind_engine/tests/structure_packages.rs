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
    let package_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");

    let report = engine
        .install_structure_package_from_dir(&package_dir, "standalone")
        .await
        .expect("install");
    assert_eq!(report.package_name, "generic-manual");
    assert_eq!(report.profiles, 3);

    let packages = engine.list_structure_packages().await.expect("list");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].package_name, "generic-manual");
    assert_eq!(packages[0].sources, vec!["standalone".to_string()]);
}

#[tokio::test]
async fn installs_structure_package_from_in_memory_files() {
    // Mirrors how a production caller (e.g. Soverex skill_files) installs a
    // package carried as DB rows rather than a filesystem directory.
    let temp = TempDir::new().expect("tempdir");
    let cfg = test_engine_config(&temp);
    let engine = AxonMindEngine::open(cfg).await.expect("engine");
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");

    let mut files = std::collections::BTreeMap::new();
    for name in ["package.toml", "identity.toml", "corpus.toml"] {
        files.insert(
            name.to_string(),
            std::fs::read_to_string(dir.join(name)).expect("read"),
        );
    }
    for entry in std::fs::read_dir(dir.join("profiles")).expect("read profiles dir") {
        let entry = entry.expect("dir entry");
        let file_name = entry.file_name().into_string().expect("utf8 filename");
        files.insert(
            format!("profiles/{file_name}"),
            std::fs::read_to_string(entry.path()).expect("read profile"),
        );
    }

    let report = engine
        .install_structure_package_from_files(&files, "skill:acme-ops")
        .await
        .expect("install");
    assert_eq!(report.package_name, "generic-manual");
    assert_eq!(report.profiles, 3);

    let packages = engine.list_structure_packages().await.expect("list");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].sources, vec!["skill:acme-ops".to_string()]);
}
