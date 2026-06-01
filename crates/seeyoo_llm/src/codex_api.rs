use crate::types::{LlmProvider, RetryConfig, ToolDefinition};
use crate::{
    AgentEvent,
    api_mod::{MessageBlock, ApiProvider, ProviderMessage},
    errors::LlmError,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::Receiver;

pub const CODEX_MODELS: &[&str] = &[
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.3-codex",
    "gpt-5.2",
    "gpt-5.1-codex",
    "gpt-5.1",
    "gpt-5",
];

pub const CODEX_INTELLIGENCE_LEVELS: &[&str] = &["low", "medium", "high", "extra_high"];

pub fn default_codex_model() -> &'static str {
    "gpt-5.3-codex"
}

pub fn default_codex_intelligence() -> &'static str {
    "medium"
}

#[derive(Debug, Clone, Default)]
pub struct CodexApiProvider {
    pub retry_config: RetryConfig,
    pub model: Option<String>,
    pub intelligence: Option<String>,
}

impl CodexApiProvider {
    pub fn new(
        retry_config: RetryConfig,
        model: Option<String>,
        intelligence: Option<String>,
    ) -> Self {
        Self {
            retry_config,
            model,
            intelligence,
        }
    }
}

#[derive(Debug)]
struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn xdg_config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn find_auth_files() -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    let mut homes = Vec::new();

    if let Some(codex_home) = std::env::var_os("CODEX_HOME").map(PathBuf::from) {
        homes.push(codex_home);
    }
    if let Some(home) = home_dir() {
        homes.push(home.join(".codex"));
        homes.push(home.join(".chatgpt"));
    }
    homes.push(xdg_config_dir().join("codex"));

    for filename in ["auth.json", "credentials.json", "config.toml"] {
        if let Some(path) = homes.iter().map(|h| h.join(filename)).find(|p| p.exists()) {
            files.push((filename.to_string(), path));
        }
    }
    files
}

fn lookup_in_path(bin: &str, path_env: &str) -> Option<PathBuf> {
    for dir in path_env.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn lookup_in_common_dirs(bin: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ];
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".bun").join("bin"));
        dirs.push(home.join(".npm-global").join("bin"));
    }

    for dir in dirs {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn lookup_in_known_app_locations(platform_bin: &str) -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
        PathBuf::from("/Applications/Codex.app/Contents/Resources").join(platform_bin),
    ];

    if let Some(home) = home_dir() {
        candidates.push(home.join("Applications/Codex.app/Contents/Resources/codex"));
        candidates.push(home.join("Applications/Codex.app/Contents/Resources").join(platform_bin));
    }

    candidates.into_iter().find(|p| p.is_file())
}

fn lookup_via_shell(bin: &str, platform_bin: &str) -> Option<PathBuf> {
    // GUI-launched apps often inherit a stripped PATH; a login shell can recover user PATH.
    let cmd = format!("command -v {bin} || command -v {platform_bin}");
    let output = std::process::Command::new("/bin/zsh")
        .arg("-lc")
        .arg(cmd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = stdout
        .lines()
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let resolved = PathBuf::from(path);
    if resolved.is_file() {
        Some(resolved)
    } else {
        None
    }
}

fn find_codex_binary() -> Option<PathBuf> {
    let path_env = std::env::var("PATH").unwrap_or_default();
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    let platform = format!("codex-{}-{}", arch, os);
    lookup_in_path("codex", &path_env)
        .or_else(|| lookup_in_path(&platform, &path_env))
        .or_else(|| lookup_in_common_dirs("codex"))
        .or_else(|| lookup_in_common_dirs(&platform))
        .or_else(|| lookup_in_known_app_locations(&platform))
        .or_else(|| lookup_via_shell("codex", &platform))
}

fn merge_prompt(system_prompt: &str, messages: &[ProviderMessage]) -> String {
    let mut parts = Vec::new();
    let system = system_prompt.trim();
    if !system.is_empty() {
        parts.push(system.to_string());
    }

    let text_from_blocks = |blocks: &[MessageBlock]| -> String {
        blocks
            .iter()
            .filter_map(|b| match b {
                MessageBlock::Text { text } => Some(text.as_str()),
                MessageBlock::ToolResult { content, .. } => Some(content.as_str()),
                MessageBlock::Artifact { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    for msg in messages {
        match msg {
            ProviderMessage::User(blocks) => {
                let text = text_from_blocks(blocks);
                if !text.trim().is_empty() {
                    parts.push(text);
                }
            }
            ProviderMessage::Assistant(blocks) | ProviderMessage::System(blocks) => {
                let text = text_from_blocks(blocks);
                if !text.trim().is_empty() {
                    parts.push(text);
                }
            }
        }
    }

    parts.join("\n\n")
}

fn reasoning_effort(intelligence: Option<&str>) -> Option<&'static str> {
    match intelligence {
        Some("low") => Some("low"),
        Some("medium") => Some("medium"),
        Some("high") => Some("high"),
        Some("extra_high") | Some("xhigh") => Some("xhigh"),
        _ => None,
    }
}

fn parse_text_value(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(parse_text_value)
                .collect::<Vec<_>>()
                .join("");
            if text.is_empty() { None } else { Some(text) }
        }
        Value::Object(map) => map
            .get("text")
            .and_then(parse_text_value)
            .or_else(|| map.get("content").and_then(parse_text_value))
            .or_else(|| map.get("message").and_then(parse_text_value)),
        _ => None,
    }
}

fn line_to_text(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or_default();

    if event_type == "error" {
        return v
            .get("message")
            .and_then(|m| m.as_str())
            .map(|m| format!("Codex error: {m}"));
    }

    v.get("message")
        .and_then(parse_text_value)
        .or_else(|| v.get("content").and_then(parse_text_value))
        .or_else(|| v.get("text").and_then(parse_text_value))
        .or_else(|| v.get("output").and_then(parse_text_value))
}

fn codex_isolation_dir() -> Result<(PathBuf, TempDirGuard), LlmError> {
    let mut dir = std::env::temp_dir();
    let suffix: u64 = rand::random();
    dir.push(format!("seeyoo-codex-{}-{suffix:x}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|e| LlmError::Config(format!("Failed to create Codex isolation dir: {e}")))?;
    Ok((dir.clone(), TempDirGuard(dir)))
}

fn copy_auth_files(dest_dir: &Path) -> Result<(), LlmError> {
    let files = find_auth_files();
    if files.is_empty() {
        return Err(LlmError::Config(
            "No Codex auth files found. Run `codex login` first.".to_string(),
        ));
    }
    for (name, src) in files {
        let dest = dest_dir.join(name);
        std::fs::copy(&src, &dest).map_err(|e| {
            LlmError::Config(format!("Failed copying Codex auth file {}: {e}", src.display()))
        })?;
    }
    Ok(())
}

#[async_trait]
impl ApiProvider for CodexApiProvider {
    fn id(&self) -> LlmProvider {
        LlmProvider::Codex
    }
    fn display_name(&self) -> &'static str {
        "Codex Sidecar"
    }
    fn retry_config(&self) -> &RetryConfig {
        &self.retry_config
    }

    async fn stream_completion(
        &self,
        _system_prompt: &str,
        _messages: Vec<ProviderMessage>,
        _tools: Vec<ToolDefinition>,
        _api_key: &str,
        _model: Option<&str>,
    ) -> Result<Receiver<AgentEvent>, LlmError> {
        Err(LlmError::Config(
            "Codex CLI provider supports non-streaming completion only in this host".into(),
        ))
    }

    async fn complete(
        &self,
        system_prompt: &str,
        messages: Vec<ProviderMessage>,
        tools: Vec<ToolDefinition>,
        _api_key: &str,
        model: Option<&str>,
    ) -> Result<String, LlmError> {
        if !tools.is_empty() {
            return Err(LlmError::Config(
                "Codex CLI provider does not support tool calls in this path".to_string(),
            ));
        }

        let binary = find_codex_binary().ok_or_else(|| {
            LlmError::Config(
                "Unable to find `codex` binary. Checked PATH, common bin directories, and Codex.app locations.".to_string(),
            )
        })?;
        let prompt = merge_prompt(system_prompt, &messages);
        if prompt.trim().is_empty() {
            return Err(LlmError::Config("Codex prompt is empty".to_string()));
        }

        let (isolated_home, _guard) = codex_isolation_dir()?;
        copy_auth_files(&isolated_home)?;

        let mut cmd = tokio::process::Command::new(binary);
        cmd.arg("--ask-for-approval")
            .arg("never")
            .arg("exec")
            .arg("--json")
            .arg("--skip-git-repo-check")
            .arg("--sandbox")
            .arg("workspace-write")
            .env("CODEX_HOME", &isolated_home);

        if let Ok(cwd) = std::env::current_dir() {
            cmd.arg("-C").arg(cwd);
        }

        if let Some(selected_model) = model.or(self.model.as_deref()) {
            cmd.arg("-m").arg(selected_model);
        }
        if let Some(effort) = reasoning_effort(self.intelligence.as_deref()) {
            cmd.arg("-c")
                .arg(format!("model_reasoning_effort=\"{effort}\""));
        }

        let output = cmd
            .arg(prompt)
            .output()
            .await
            .map_err(|e| LlmError::RequestFailed(format!("Failed to run codex exec: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            return Err(LlmError::Api {
                provider: "codex".to_string(),
                message: if detail.is_empty() {
                    format!("codex exited with status {}", output.status)
                } else {
                    detail
                },
                code: output.status.code().map(|c| c as u16),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let texts = stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .filter_map(line_to_text)
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>();

        if texts.is_empty() {
            return Err(LlmError::StreamError(
                "Codex returned no parseable text output".to_string(),
            ));
        }

        Ok(texts.join("\n"))
    }
}
