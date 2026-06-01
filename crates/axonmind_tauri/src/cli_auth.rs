//! Detect existing CLI tool sessions (Codex, Claude Code, Antigravity) so the demo
//! app can offer "use your existing account" without requiring a manual API key paste.
//!
//! Pattern: read the credential file each CLI writes on login, decode the JWT to extract
//! name/email for display, and return the raw access token for provider construction.
//! All file reads are best-effort — missing files silently return None.

use serde::{Deserialize, Serialize};
use std::{env, path::{Path, PathBuf}};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliProfile {
    /// "codex" | "claude" | "antigravity"
    pub cli: String,
    pub name: Option<String>,
    pub email: Option<String>,
    /// Raw access token — kept in memory only, never persisted by this module.
    #[serde(skip_serializing)]
    pub access_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSessions {
    pub codex: Option<CliProfile>,
    pub claude: Option<CliProfile>,
    pub antigravity: Option<CliProfile>,
}

// ── Detection ─────────────────────────────────────────────────────────────────

/// Detect all available CLI sessions. Cheap — only reads small JSON files.
pub fn detect_all() -> CliSessions {
    CliSessions {
        codex: detect_codex(),
        claude: detect_claude(),
        antigravity: detect_antigravity(),
    }
}

/// Codex CLI / ChatGPT session discovery.
pub fn detect_codex() -> Option<CliProfile> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = home_dir() {
        candidates.push(home.join(".codex").join("auth.json"));
        candidates.push(home.join(".codex").join("credentials.json"));
        candidates.push(home.join(".chatgpt").join("credentials.json"));
    }
    if let Some(codex_home) = env::var_os("CODEX_HOME").map(PathBuf::from) {
        candidates.push(codex_home.join("auth.json"));
        candidates.push(codex_home.join("credentials.json"));
    }
    candidates.push(xdg_config_dir().join("codex").join("auth.json"));
    candidates.push(xdg_config_dir().join("codex").join("credentials.json"));

    detect_from_sources(
        "codex",
        &candidates,
        &[
            ("codex-cli", "oauth"),
            ("Codex Auth", "oauth"),
            ("codex", "oauth"),
            ("openai-codex", "oauth"),
        ],
    )
}

/// Claude Code CLI session discovery.
pub fn detect_claude() -> Option<CliProfile> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = home_dir() {
        candidates.push(home.join(".claude").join(".credentials.json"));
        candidates.push(home.join(".claude").join("credentials.json"));
    }
    candidates.push(xdg_config_dir().join("claude").join(".credentials.json"));
    candidates.push(xdg_config_dir().join("claude").join("credentials.json"));
    candidates.push(xdg_config_dir().join("claude-code").join(".credentials.json"));
    candidates.push(xdg_config_dir().join("claude-code").join("credentials.json"));

    detect_from_sources(
        "claude",
        &candidates,
        &[
            ("claude-code", "oauth"),
            ("claude-cli", "oauth"),
            ("anthropic-cli", "oauth"),
            ("Claude Code", "oauth"),
        ],
    )
}

/// Antigravity CLI (Gemini OpenAI-compatible) session discovery.
pub fn detect_antigravity() -> Option<CliProfile> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = home_dir() {
        candidates.push(home.join(".antigravity").join("credentials.json"));
        candidates.push(home.join(".gemini").join("credentials.json"));
        candidates.push(home.join(".gemini").join("antigravity").join("credentials.json"));
        candidates.push(home.join(".gemini").join("antigravity-cli").join("credentials.json"));
    }
    let base = xdg_config_dir();
    candidates.push(base.join("agy").join("credentials.json"));
    candidates.push(base.join("antigravity").join("credentials.json"));
    candidates.push(base.join("antigravity-cli").join("credentials.json"));
    candidates.push(base.join("gemini").join("credentials.json"));

    detect_from_sources(
        "antigravity",
        &candidates,
        &[
            ("antigravity-cli", "oauth"),
            ("agy", "oauth"),
            ("gemini-cli", "oauth"),
            ("gemini", "oauth"),
        ],
    )
}

// ── File reading + JWT decode ─────────────────────────────────────────────────

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
        .or_else(|| {
            let drive = env::var_os("HOMEDRIVE")?;
            let path = env::var_os("HOMEPATH")?;
            let mut pb = PathBuf::from(drive);
            pb.push(path);
            Some(pb)
        })
}

fn xdg_config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            home_dir()
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".config")
        })
}

/// Read a credential file and extract an access token + profile. Tolerates different
/// nesting shapes produced by different CLI tools (flat vs. `tokens` sub-object).
fn read_profile(path: &Path, cli: &str) -> Option<CliProfile> {
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    let access_token = token_from_json_or_raw(&content)?;
    let (name, email) = decode_jwt_profile(&access_token)
        .or_else(|| {
            // Some tools store id_token separately alongside the access_token.
            let val: serde_json::Value = serde_json::from_str(&content).ok()?;
            val.get("tokens")
                .and_then(|t| t.get("id_token"))
                .and_then(|v| v.as_str())
                .and_then(|t| decode_jwt_profile(t))
        })
        .unwrap_or((None, None));

    Some(CliProfile {
        cli: cli.to_string(),
        name,
        email,
        access_token,
    })
}

fn detect_from_sources(
    cli: &str,
    file_candidates: &[PathBuf],
    keyring_candidates: &[(&str, &str)],
) -> Option<CliProfile> {
    for path in file_candidates {
        if let Some(profile) = read_profile(path, cli) {
            return Some(profile);
        }
    }
    read_profile_from_keyring(cli, keyring_candidates)
}

fn read_profile_from_keyring(
    cli: &str,
    keyring_candidates: &[(&str, &str)],
) -> Option<CliProfile> {
    for (service, account) in keyring_candidates {
        let entry = match keyring::Entry::new(service, account) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let secret = match entry.get_password() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let access_token = match token_from_json_or_raw(&secret) {
            Some(v) => v,
            None => continue,
        };
        let (name, email) = decode_jwt_profile(&access_token).unwrap_or((None, None));
        return Some(CliProfile {
            cli: cli.to_string(),
            name,
            email,
            access_token,
        });
    }
    None
}

fn token_from_json_or_raw(content: &str) -> Option<String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
        return extract_token(&val);
    }
    let raw = content.trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

/// Walk common nesting patterns to find an access token string.
fn extract_token(val: &serde_json::Value) -> Option<String> {
    // Flat: { "access_token": "..." }
    if let Some(t) = val.get("access_token").and_then(|v| v.as_str()) {
        return Some(t.to_string());
    }
    // Nested: { "tokens": { "access_token": "..." } }
    if let Some(t) = val
        .get("tokens")
        .and_then(|tokens| tokens.get("access_token"))
        .and_then(|v| v.as_str())
    {
        return Some(t.to_string());
    }
    // Legacy Codex shape: { "token": "..." }
    if let Some(t) = val.get("token").and_then(|v| v.as_str()) {
        return Some(t.to_string());
    }
    // camelCase variant
    if let Some(t) = val.get("accessToken").and_then(|v| v.as_str()) {
        return Some(t.to_string());
    }
    // nested auth shapes
    if let Some(t) = val
        .get("auth")
        .and_then(|a| a.get("access_token"))
        .and_then(|v| v.as_str())
    {
        return Some(t.to_string());
    }
    if let Some(t) = val
        .get("auth")
        .and_then(|a| a.get("accessToken"))
        .and_then(|v| v.as_str())
    {
        return Some(t.to_string());
    }
    if let Some(t) = val
        .get("oauth")
        .and_then(|a| a.get("access_token"))
        .and_then(|v| v.as_str())
    {
        return Some(t.to_string());
    }
    if let Some(t) = val
        .get("oauth")
        .and_then(|a| a.get("accessToken"))
        .and_then(|v| v.as_str())
    {
        return Some(t.to_string());
    }
    None
}

/// Decode the payload segment of a JWT and return (name, email) when present.
fn decode_jwt_profile(token: &str) -> Option<(Option<String>, Option<String>)> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&bytes).ok()?;

    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let email = payload
        .get("email")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        // OpenAI sometimes nests email under a profile claim
        .or_else(|| {
            payload
                .get("https://api.openai.com/profile")
                .and_then(|p| p.get("email"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });

    if name.is_some() || email.is_some() {
        Some((name, email))
    } else {
        None
    }
}

// ── Provider config helpers ───────────────────────────────────────────────────

/// OpenAI-compatible base URLs for each CLI's backing API.
pub fn base_url_for(cli: &str) -> &'static str {
    match cli {
        "codex" => "https://api.openai.com/v1",
        "claude" => "https://api.anthropic.com/v1",
        "antigravity" => "https://generativelanguage.googleapis.com/v1beta/openai",
        _ => "https://api.openai.com/v1",
    }
}

/// Sensible default model for each CLI provider.
pub fn default_model_for(cli: &str) -> &'static str {
    match cli {
        "codex" => "gpt-4o",
        "claude" => "claude-opus-4-8",
        "antigravity" => "gemini-2.5-pro",
        _ => "gpt-4o",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_standard_jwt_profile() {
        // Payload: {"name":"Test User","email":"test@example.com"}
        let token = "header.eyJuYW1lIjoiVGVzdCBVc2VyIiwiZW1haWwiOiJ0ZXN0QGV4YW1wbGUuY29tIn0.sig";
        let (name, email) = decode_jwt_profile(token).unwrap();
        assert_eq!(name.as_deref(), Some("Test User"));
        assert_eq!(email.as_deref(), Some("test@example.com"));
    }

    #[test]
    fn extract_token_flat() {
        let val = serde_json::json!({"access_token": "tok123"});
        assert_eq!(extract_token(&val).as_deref(), Some("tok123"));
    }

    #[test]
    fn extract_token_nested() {
        let val = serde_json::json!({"tokens": {"access_token": "tok456"}});
        assert_eq!(extract_token(&val).as_deref(), Some("tok456"));
    }

    #[test]
    fn extract_token_legacy_codex() {
        let val = serde_json::json!({"token": "tok789"});
        assert_eq!(extract_token(&val).as_deref(), Some("tok789"));
    }
}
