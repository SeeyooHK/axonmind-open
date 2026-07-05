use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axonmind_core::AxonMindError;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    pub package: PackageMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMeta {
    pub name: String,
    pub version: i64,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileDefinition {
    pub profile: ProfileMeta,
    #[serde(default)]
    pub region: Vec<RegionDefinition>,
    #[serde(default)]
    pub toc: Option<TocPolicy>,
    #[serde(default, rename = "unit")]
    pub units: Vec<StructureUnit>,
    #[serde(default, rename = "ref")]
    pub refs: Vec<StructureUnitRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMeta {
    pub name: String,
    pub version: i64,
    pub description: Option<String>,
    #[serde(default)]
    pub language: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionDefinition {
    pub name: String,
    pub from: String,
    pub until: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocPolicy {
    #[serde(default)]
    pub strip_lines: Vec<String>,
    #[serde(default)]
    pub strip_headings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureUnit {
    pub kind: String,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    pub marker: String,
    #[serde(default)]
    pub captures: BTreeMap<String, CaptureSelector>,
    #[serde(default)]
    pub title: Option<String>,
    pub label: String,
    pub label_norm: String,
    pub citation: String,
    pub level: LevelSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureUnitRef {
    pub target_kind: String,
    pub pattern: String,
    pub target_label_norm: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CaptureSelector {
    Index(usize),
    FirstOf(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LevelSpec {
    Fixed(i64),
    Depth(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityRulesFile {
    #[serde(default, rename = "rule")]
    pub rules: Vec<IdentityRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityRule {
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(rename = "match")]
    pub pattern: String,
    pub canonical_title: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub set: IdentitySet,
    pub profile: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdentitySet {
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub jurisdiction: Vec<String>,
    #[serde(default)]
    pub domain: Vec<String>,
    #[serde(default)]
    pub instrument_type: Option<String>,
    #[serde(default)]
    pub corpus: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusFile {
    pub corpus: Option<CorpusMeta>,
    #[serde(default, rename = "bind")]
    pub binds: Vec<CorpusBinding>,
    #[serde(default, rename = "term_map")]
    pub term_maps: Vec<TermMapBinding>,
    #[serde(default, rename = "xref")]
    pub xrefs: Vec<XrefBinding>,
    #[serde(default)]
    pub enrichment: Option<EnrichmentBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusMeta {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusBinding {
    #[serde(rename = "match")]
    pub matcher: IdentityMatch,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub corpus: Vec<String>,
    #[serde(default)]
    pub domain: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityMatch {
    #[serde(default)]
    pub canonical_title: Option<String>,
    #[serde(default)]
    pub corpus: Option<String>,
    #[serde(default)]
    pub instrument_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermMapBinding {
    pub from_lang: String,
    pub to_lang: String,
    #[serde(default)]
    pub terms: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XrefBinding {
    pub from: IdentityMatch,
    pub to: IdentityMatch,
    #[serde(default)]
    pub kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichmentBinding {
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructurePackage {
    pub root_dir: PathBuf,
    pub manifest: PackageManifest,
    pub profiles: Vec<ProfileDefinition>,
    pub identity: IdentityRulesFile,
    pub corpus: Option<CorpusFile>,
}

impl StructurePackage {
    pub fn from_dir(path: &Path) -> Result<Self, AxonMindError> {
        let manifest: PackageManifest = parse_toml_file(&path.join("package.toml"))?;
        let identity: IdentityRulesFile = parse_toml_file(&path.join("identity.toml"))?;
        let corpus_path = path.join("corpus.toml");
        let corpus = if corpus_path.exists() {
            Some(parse_toml_file(&corpus_path)?)
        } else {
            None
        };

        let profiles_dir = path.join("profiles");
        let mut profiles: Vec<ProfileDefinition> = Vec::new();
        let entries =
            std::fs::read_dir(&profiles_dir).map_err(|e| AxonMindError::ValidationFailed {
                message: format!("read {}: {e}", profiles_dir.display()),
            })?;
        for entry in entries {
            let entry = entry.map_err(|e| AxonMindError::ValidationFailed {
                message: format!("read profile entry: {e}"),
            })?;
            let file_type = entry
                .file_type()
                .map_err(|e| AxonMindError::ValidationFailed {
                    message: format!("profile file type: {e}"),
                })?;
            if !file_type.is_file() {
                continue;
            }
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            profiles.push(parse_toml_file(&entry.path())?);
        }
        profiles.sort_by(|a, b| a.profile.name.cmp(&b.profile.name));

        Ok(Self {
            root_dir: path.to_path_buf(),
            manifest,
            profiles,
            identity,
            corpus,
        })
    }

    pub fn validate(&self) -> Result<(), AxonMindError> {
        let name_re = regex::Regex::new(r"^[a-z0-9-]{1,64}$").expect("hardcoded regex");
        if !name_re.is_match(&self.manifest.package.name) {
            return Err(AxonMindError::ValidationFailed {
                message: format!("invalid package name {}", self.manifest.package.name),
            });
        }

        let mut profile_names = Vec::new();
        for profile in &self.profiles {
            if !name_re.is_match(&profile.profile.name) {
                return Err(AxonMindError::ValidationFailed {
                    message: format!("invalid profile name {}", profile.profile.name),
                });
            }
            if profile_names
                .iter()
                .any(|existing| existing == &profile.profile.name)
            {
                return Err(AxonMindError::ValidationFailed {
                    message: format!("duplicate profile {}", profile.profile.name),
                });
            }
            profile_names.push(profile.profile.name.clone());
            validate_regex(
                &profile.profile.name,
                "profile unit marker",
                profile.units.iter().map(|unit| unit.marker.as_str()),
            )?;
            validate_regex(
                &profile.profile.name,
                "profile ref",
                profile
                    .refs
                    .iter()
                    .map(|reference| reference.pattern.as_str()),
            )?;
        }

        for rule in &self.identity.rules {
            if !profile_names.iter().any(|name| name == &rule.profile) {
                return Err(AxonMindError::ValidationFailed {
                    message: format!("identity rule references missing profile {}", rule.profile),
                });
            }
            validate_regex(
                &self.manifest.package.name,
                "identity rule",
                std::iter::once(rule.pattern.as_str()),
            )?;
        }

        Ok(())
    }

    pub fn content_sha(&self) -> Result<String, AxonMindError> {
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(&self.manifest).map_err(json_error)?);
        hasher.update(serde_json::to_vec(&self.profiles).map_err(json_error)?);
        hasher.update(serde_json::to_vec(&self.identity).map_err(json_error)?);
        hasher.update(serde_json::to_vec(&self.corpus).map_err(json_error)?);
        Ok(format!("{:x}", hasher.finalize()))
    }
}

fn parse_toml_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, AxonMindError> {
    let text = std::fs::read_to_string(path).map_err(|e| AxonMindError::ValidationFailed {
        message: format!("read {}: {e}", path.display()),
    })?;
    toml::from_str(&text).map_err(|e| AxonMindError::ValidationFailed {
        message: format!("parse {}: {e}", path.display()),
    })
}

fn validate_regex<'a>(
    scope: &str,
    label: &str,
    patterns: impl Iterator<Item = &'a str>,
) -> Result<(), AxonMindError> {
    for pattern in patterns {
        RegexBuilder::new(pattern)
            .size_limit(256 * 1024)
            .dfa_size_limit(256 * 1024)
            .build()
            .map_err(|e| AxonMindError::ValidationFailed {
                message: format!("{scope} {label} regex {pattern:?}: {e}"),
            })?;
    }
    Ok(())
}

fn json_error(error: serde_json::Error) -> AxonMindError {
    AxonMindError::ValidationFailed {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_validates_generic_package() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");
        let pkg = StructurePackage::from_dir(&dir).expect("package loads");
        pkg.validate().expect("package validates");
        assert_eq!(pkg.manifest.package.name, "generic-manual");
        assert_eq!(pkg.profiles.len(), 3);
        assert_eq!(pkg.identity.rules.len(), 4);
    }
}
