//! Centralized prompt fragments + caller-ordered assembly.
//!
//! Prompts are stored as small composable **fragments**, each addressed by a key
//! (e.g. `categorize.system`). A caller assembles the final prompt by listing the fragment keys
//! in the order it wants them; the library loads each and joins them with a blank line.
//!
//! Each fragment has a built-in default embedded at compile time. A workspace may **override** any
//! fragment by placing `<key>.md` in its prompts directory; the override file wins. Delete the
//! file to revert to the built-in. This keeps prompt tuning out of the Rust source and out of the
//! per-provider duplication that the inline prompts in `openai.rs` / `seeyoo.rs` currently suffer.
//!
//! The library only *loads and assembles* the prompt text. Sending it to the model stays with the
//! provider, which holds the credentials and HTTP client.

use std::path::PathBuf;

/// Loads and assembles prompt fragments, preferring per-workspace overrides over built-ins.
pub struct PromptLibrary {
    /// Directory searched for `<key>.md` override files. `None` = built-ins only.
    overrides_dir: Option<PathBuf>,
}

impl PromptLibrary {
    pub fn new(overrides_dir: Option<PathBuf>) -> Self {
        Self { overrides_dir }
    }

    /// Resolve a single fragment by key: the workspace override file if present and readable,
    /// otherwise the built-in default. `None` when the key is unknown and no override exists.
    pub fn fragment(&self, key: &str) -> Option<String> {
        if let Some(dir) = &self.overrides_dir {
            if let Ok(text) = std::fs::read_to_string(dir.join(format!("{key}.md"))) {
                return Some(text);
            }
        }
        builtin(key).map(str::to_string)
    }

    /// Assemble fragments in the caller's order into one prompt, joined by blank lines.
    /// Unknown keys (no override, no built-in) are skipped so a typo degrades rather than panics;
    /// use [`Self::resolve_all`] when you need to verify every key resolved.
    pub fn assemble(&self, keys: &[&str]) -> String {
        keys.iter()
            .filter_map(|k| self.fragment(k))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Like [`Self::assemble`] but returns `Err(key)` on the first key that resolves to nothing,
    /// for callers that want a hard guarantee the prompt is complete.
    pub fn resolve_all(&self, keys: &[&str]) -> Result<String, String> {
        let mut parts = Vec::with_capacity(keys.len());
        for &k in keys {
            match self.fragment(k) {
                Some(t) => parts.push(t),
                None => return Err(k.to_string()),
            }
        }
        Ok(parts.join("\n\n"))
    }
}

/// Built-in fragment defaults, embedded at compile time from the sibling `.md` files.
/// Adding a fragment = drop a `<key>.md` here and add one match arm.
fn builtin(key: &str) -> Option<&'static str> {
    Some(match key {
        "categorize.system" => include_str!("categorize.system.md"),
        "categorize.rules" => include_str!("categorize.rules.md"),
        "categorize.optimization" => include_str!("categorize.optimization.md"),
        "categorize.output" => include_str!("categorize.output.md"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_builtins_in_caller_order() {
        // Intent: the assembled prompt must contain each requested fragment, in the order asked —
        // ordering is the contract the caller relies on to compose system prompts.
        let lib = PromptLibrary::new(None);
        let out = lib.assemble(&["categorize.system", "categorize.output"]);
        let sys = out.find("brain map").expect("system fragment present");
        let outp = out.find("ONLY JSON").expect("output fragment present");
        assert!(
            sys < outp,
            "fragments must appear in caller-specified order"
        );
    }

    #[test]
    fn unknown_key_is_skipped_not_fatal() {
        // A typo'd key should degrade to the remaining fragments, not blow up the whole prompt.
        let lib = PromptLibrary::new(None);
        let out = lib.assemble(&["categorize.system", "does.not.exist"]);
        assert_eq!(out, lib.fragment("categorize.system").unwrap());
    }

    #[test]
    fn resolve_all_reports_the_missing_key() {
        let lib = PromptLibrary::new(None);
        assert_eq!(
            lib.resolve_all(&["categorize.system", "nope"]),
            Err("nope".to_string())
        );
    }

    #[test]
    fn override_file_wins_over_builtin() {
        // Intent: a workspace can tune a prompt by dropping a file, without touching Rust — and a
        // key it did NOT override still falls back to the built-in.
        let dir = std::env::temp_dir().join(format!("axonmind_prompts_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("categorize.system.md"), "OVERRIDDEN SYSTEM").unwrap();

        let lib = PromptLibrary::new(Some(dir.clone()));
        assert_eq!(
            lib.fragment("categorize.system").as_deref(),
            Some("OVERRIDDEN SYSTEM")
        );
        assert!(
            lib.fragment("categorize.rules")
                .unwrap()
                .contains("AT MOST 10 categories"),
            "non-overridden key still resolves to the built-in"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
