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
    /// `"strict"` blocks install/retro-apply on eval failure; anything else (absent, typo,
    /// unrecognized value) is `"warn"` — same absent-is-safe-default convention as
    /// `grounding_mode` (retrieve_guarantee.md item 6). Enforcement of the strict path is
    /// deferred (item 7); this session parses the key and reports results either way.
    #[serde(default)]
    pub eval_policy: Option<String>,
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
    /// Optional regex matching this unit's marker glued onto the tail of a preceding line with
    /// no line-start anchor available to it (docs/retrieve_guarantee.md's Article 69/70 finding:
    /// CommonMark's lazy-paragraph-continuation rule can fold a heading with no preceding blank
    /// line into the previous block). When it matches mid-line, a line break is inserted right
    /// before the match so `marker`'s own `^`-anchored regex can fire normally. Authors should
    /// anchor this at end-of-line (`\s*$`) so it only catches a swallowed heading, not an
    /// ordinary mid-sentence cross-reference — see `split_glued_boundaries` in `structure/parse.rs`.
    #[serde(default)]
    pub split_before: Option<String>,
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
    /// Minimum provenance tier a document must carry to be `citation_safe` in this corpus
    /// (`user_upload` | `plugin_bundle` | `web_fetched` | `auto_captured`) — see
    /// `ProvenanceTier`. `None` means the item-10 default, `user_upload`, applies.
    #[serde(default)]
    pub min_citation_provenance: Option<String>,
    /// Currency review deadline for this corpus (retrieve_guarantee.md item 11) — a free-form
    /// package-declared date string (e.g. `"2026-12-31"`), surfaced by the Library review UI as
    /// "currency review overdue" once passed. `None` means no review cadence is declared.
    #[serde(default)]
    pub review_by: Option<String>,
    /// When `true`, a unit whose document `status` is `superseded` is excluded outright
    /// (`citation_safe: false`, same gate `min_citation_provenance` already uses) instead of the
    /// default behavior of staying citable with an explicit warning in the rider. `None`/`false`
    /// keeps today's behavior.
    #[serde(default)]
    pub exclude_superseded: Option<bool>,
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
    /// Currency status this bind declares for the matched instrument (`in_force` | `amended` |
    /// `superseded` | `unknown`) — retrieve_guarantee.md item 11. `None` leaves the document's
    /// existing status (default `unknown`) untouched.
    #[serde(default)]
    pub status: Option<String>,
    /// Date `status` was declared true as-of, package-declared verbatim.
    #[serde(default)]
    pub as_of: Option<String>,
    /// Declared replacement instrument, stored and rendered as the package-declared title string
    /// verbatim — never resolved to an `IdentityMatch` at render time.
    #[serde(default)]
    pub superseded_by: Option<String>,
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

/// One `evals/*.toml` file: `query -> expected locator(s)` retrieval regression cases
/// (retrieve_guarantee.md item 7 / Enhancement #9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalsFile {
    #[serde(default, rename = "eval")]
    pub evals: Vec<EvalCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCase {
    pub name: String,
    pub query: String,
    pub corpus: String,
    #[serde(default)]
    pub top_k: Option<usize>,
    /// Optional document-identity oracle. Locator numbers are not globally unique across a
    /// corpus, so instrument-scoping evals must assert both the locator and its document.
    #[serde(default)]
    pub expect_document: Option<IdentityMatch>,
    /// Any-of: the eval passes if any top-k hit's locator is a superset match against any one
    /// of these maps (every key in the expect map must equal the hit's locator value; extra
    /// keys on the hit are ignored). Subset matching so a grammar can add new capture keys
    /// without invalidating existing evals.
    #[serde(default, rename = "expect")]
    pub expect: Vec<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructurePackage {
    pub root_dir: PathBuf,
    pub manifest: PackageManifest,
    pub profiles: Vec<ProfileDefinition>,
    pub identity: IdentityRulesFile,
    pub corpus: Option<CorpusFile>,
    #[serde(default)]
    pub evals: Vec<EvalCase>,
    /// sha256 over the package's own raw definition files (path-sorted `(path, text)` pairs),
    /// computed once in `from_files` — the single source of truth `from_dir` delegates to, so
    /// both constructors always agree. Deliberately NOT derived from the parsed struct shape:
    /// axonmind's own schema can grow new optional fields over time with zero package content
    /// changing (root-caused 2026-07-17 — two unrelated schema additions, `min_citation_provenance`
    /// and the supersession fields, silently broke every already-installed package's stored sha
    /// the moment they shipped, because the old hash covered `serde_json::to_vec(&parsed_struct)`).
    /// `pub` (not encapsulated) to match every other field on this struct; `store::load_structure_packages`
    /// reconstructs a `StructurePackage` from DB-stored parsed fragments with no raw file bytes
    /// available and sets this to an empty string — that reconstruction path never calls
    /// `content_sha()` (only the install path via `from_files`/`from_dir` does), by design.
    #[serde(default)]
    pub raw_content_sha: String,
}

impl StructurePackage {
    pub fn from_dir(path: &Path) -> Result<Self, AxonMindError> {
        let mut files: BTreeMap<String, String> = BTreeMap::new();

        let read = |rel: &Path| -> Result<String, AxonMindError> {
            std::fs::read_to_string(rel).map_err(|e| AxonMindError::ValidationFailed {
                message: format!("read {}: {e}", rel.display()),
            })
        };

        files.insert("package.toml".to_string(), read(&path.join("package.toml"))?);
        files.insert("identity.toml".to_string(), read(&path.join("identity.toml"))?);

        let corpus_path = path.join("corpus.toml");
        if corpus_path.exists() {
            files.insert("corpus.toml".to_string(), read(&corpus_path)?);
        }

        let profiles_dir = path.join("profiles");
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
            let file_name = entry.file_name().to_string_lossy().to_string();
            files.insert(format!("profiles/{file_name}"), read(&entry.path())?);
        }

        let evals_dir = path.join("evals");
        if evals_dir.exists() {
            let entries =
                std::fs::read_dir(&evals_dir).map_err(|e| AxonMindError::ValidationFailed {
                    message: format!("read {}: {e}", evals_dir.display()),
                })?;
            for entry in entries {
                let entry = entry.map_err(|e| AxonMindError::ValidationFailed {
                    message: format!("read eval entry: {e}"),
                })?;
                if entry.path().extension().and_then(|ext| ext.to_str()) != Some("toml") {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy().to_string();
                files.insert(format!("evals/{file_name}"), read(&entry.path())?);
            }
        }

        let mut pkg = Self::from_files(&files)?;
        pkg.root_dir = path.to_path_buf();
        Ok(pkg)
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

        let mut eval_names = Vec::new();
        for eval in &self.evals {
            if eval.name.is_empty() {
                return Err(AxonMindError::ValidationFailed {
                    message: "eval case is missing a name".to_string(),
                });
            }
            if eval_names.iter().any(|existing| existing == &eval.name) {
                return Err(AxonMindError::ValidationFailed {
                    message: format!("duplicate eval name {}", eval.name),
                });
            }
            eval_names.push(eval.name.clone());
            if eval.expect.is_empty() {
                return Err(AxonMindError::ValidationFailed {
                    message: format!("eval {} has no expect entries", eval.name),
                });
            }
        }

        Ok(())
    }

    pub fn content_sha(&self) -> Result<String, AxonMindError> {
        Ok(self.raw_content_sha.clone())
    }

    /// Builds a package from in-memory file contents keyed by path relative to
    /// the package root (`"package.toml"`, `"profiles/foo.toml"`, ...), the
    /// same layout `from_dir` reads off disk. This is how production installs
    /// load a package carried as DB rows (e.g. Soverex `skill_files`) rather
    /// than a filesystem directory; callers own stripping any storage-specific
    /// path prefix before calling this.
    pub fn from_files(files: &BTreeMap<String, String>) -> Result<Self, AxonMindError> {
        let manifest: PackageManifest = parse_toml_str(get_required(files, "package.toml")?)?;
        let identity: IdentityRulesFile = parse_toml_str(get_required(files, "identity.toml")?)?;
        let corpus = match files.get("corpus.toml") {
            Some(text) => Some(parse_toml_str(text)?),
            None => None,
        };

        let mut profiles: Vec<ProfileDefinition> = Vec::new();
        for (path, text) in files {
            let Some(rest) = path.strip_prefix("profiles/") else {
                continue;
            };
            if rest.contains('/') || !rest.ends_with(".toml") {
                continue;
            }
            profiles.push(parse_toml_str(text)?);
        }
        profiles.sort_by(|a, b| a.profile.name.cmp(&b.profile.name));

        let mut evals: Vec<EvalCase> = Vec::new();
        for (path, text) in files {
            let Some(rest) = path.strip_prefix("evals/") else {
                continue;
            };
            if rest.contains('/') || !rest.ends_with(".toml") {
                continue;
            }
            let file: EvalsFile = parse_toml_str(text)?;
            evals.extend(file.evals);
        }
        evals.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(Self {
            root_dir: PathBuf::new(),
            manifest,
            profiles,
            identity,
            corpus,
            evals,
            raw_content_sha: raw_files_sha(files),
        })
    }
}

/// sha256 over path-sorted `(path, text)` pairs — the actual bytes a package author wrote, not
/// whatever shape the current parser happens to produce them into. `BTreeMap` iteration is
/// already path-sorted; a `\0` separator keeps e.g. `("a", "bc")` from hashing the same as
/// `("ab", "c")`.
fn raw_files_sha(files: &BTreeMap<String, String>) -> String {
    let mut hasher = Sha256::new();
    for (path, text) in files {
        hasher.update(path.as_bytes());
        hasher.update([0u8]);
        hasher.update(text.as_bytes());
        hasher.update([0u8]);
    }
    format!("{:x}", hasher.finalize())
}

fn get_required<'a>(
    files: &'a BTreeMap<String, String>,
    path: &str,
) -> Result<&'a str, AxonMindError> {
    files
        .get(path)
        .map(String::as_str)
        .ok_or_else(|| AxonMindError::ValidationFailed {
            message: format!("missing required file: {path}"),
        })
}

fn parse_toml_str<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, AxonMindError> {
    toml::from_str(text).map_err(|e| AxonMindError::ValidationFailed {
        message: format!("parse toml: {e}"),
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

    #[test]
    fn from_files_matches_from_dir_for_the_same_package() {
        // Production installs (Soverex skill_files) carry package files as DB
        // rows, not a filesystem directory. from_files must build an identical
        // package to from_dir given the same content, keyed the same way.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");
        let from_dir_pkg = StructurePackage::from_dir(&dir).expect("from_dir");

        let mut files = BTreeMap::new();
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

        let from_files_pkg = StructurePackage::from_files(&files).expect("from_files");
        from_files_pkg.validate().expect("validates");
        assert_eq!(
            from_files_pkg.content_sha().expect("sha"),
            from_dir_pkg.content_sha().expect("sha"),
        );
    }

    #[test]
    fn content_sha_equals_the_raw_files_hash_not_a_function_of_the_parsed_struct_shape() {
        // Regression pin for the 2026-07-17 live bug: content_sha() used to hash
        // `serde_json::to_vec(&parsed_struct)`, so any axonmind schema addition (a new
        // optional field on PackageManifest/ProfileDefinition/etc.) changed every
        // already-installed package's stored sha with zero actual package content
        // change (`install_structure_package`'s "changed without version bump" guard
        // then false-positives on every subsequent reconcile). Asserting content_sha()
        // equals `raw_files_sha` directly — not just that two call sites agree — proves
        // the value can never again depend on how the parser happens to shape its output.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");
        let mut files = BTreeMap::new();
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

        let pkg = StructurePackage::from_files(&files).expect("from_files");
        assert_eq!(pkg.content_sha().expect("sha"), raw_files_sha(&files));
    }

    #[test]
    fn content_sha_is_insensitive_to_extra_unknown_toml_keys_but_sensitive_to_real_edits() {
        // A package author's own edit changes the sha (guard fires correctly); this doesn't
        // test schema growth directly (that needs a second axonmind version to compare against),
        // but pins that the sha tracks real file content changes, the property the guard needs.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");
        let mut files = BTreeMap::new();
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
        let baseline = StructurePackage::from_files(&files)
            .expect("from_files")
            .content_sha()
            .expect("sha");

        let mut edited = files.clone();
        let package_toml = edited.get_mut("package.toml").expect("package.toml");
        package_toml.push_str("\n# a real edit\n");
        let edited_sha = StructurePackage::from_files(&edited)
            .expect("from_files")
            .content_sha()
            .expect("sha");

        assert_ne!(baseline, edited_sha);
    }
}
