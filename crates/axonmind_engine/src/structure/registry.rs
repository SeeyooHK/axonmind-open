use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallReport {
    pub package_name: String,
    pub version: i64,
    pub profiles: usize,
    pub identity_rules: usize,
    pub corpus_bindings: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    pub package_name: String,
    pub version: i64,
    pub description: Option<String>,
    pub sources: Vec<String>,
}
