import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface CliProfile {
  cli: string;
  name: string | null;
  email: string | null;
}

interface CliSessions {
  codex: CliProfile | null;
  claude: CliProfile | null;
  antigravity: CliProfile | null;
}

interface RuntimeProviderStatus {
  has_provider: boolean;
  source: "none" | "api_key" | "cli" | "unknown";
  label: string | null;
}

interface CliSessionOptions {
  models: string[];
  intelligence_levels: string[];
  default_model: string | null;
  default_intelligence: string | null;
}

const CUSTOM_CODEX_MODEL_OPTION = "__custom_model__";

const CLI_META: Record<string, { label: string; model: string; experimental: boolean; note?: string }> = {
  codex:       { label: "ChatGPT (Codex CLI)",       model: "gpt-4o",         experimental: false },
  claude:      { label: "Claude (Claude Code CLI)",  model: "claude-opus-4-8", experimental: true,
                 note: "OAuth token compatibility with the Anthropic API is not yet verified — may require additional request headers. Try it and report any issues." },
  antigravity: { label: "Gemini (Antigravity CLI)",  model: "gemini-2.5-pro", experimental: true,
                 note: "OAuth token compatibility with the Google Gemini OpenAI-compatible endpoint is not yet verified. Try it and report any issues." },
};

interface ApiKeyConfig {
  id: string;
  name: string;
  provider: string;
  base_url: string;
  model: string;
  is_active: boolean;
  key_masked: string;
}

interface Props {
  onClose: () => void;
  onKeysChanged: () => void;
}

const PROVIDER_DEFAULTS: Record<string, { base_url: string; model: string; label: string; showUrl: boolean }> = {
  openai:     { base_url: "https://api.openai.com/v1",        model: "gpt-4o-mini",          label: "OpenAI",                   showUrl: false },
  openrouter: { base_url: "https://openrouter.ai/api/v1",     model: "openai/gpt-4o-mini",   label: "OpenRouter",               showUrl: true },
  deepseek:   { base_url: "https://api.deepseek.com/v1",      model: "deepseek-chat",        label: "DeepSeek",                 showUrl: true },
  ollama:     { base_url: "http://localhost:11434/v1",         model: "llama3.2",             label: "Ollama (local)",           showUrl: true },
  lm_studio:  { base_url: "http://localhost:1234/v1",         model: "local-model",          label: "LM Studio (local)",        showUrl: true },
  custom:     { base_url: "",                                  model: "",                     label: "Custom (OpenAI-compatible)", showUrl: true },
};

const EMPTY_FORM = { id: undefined as string | undefined, name: "", provider: "openai", base_url: PROVIDER_DEFAULTS.openai.base_url, model: PROVIDER_DEFAULTS.openai.model, api_key: "" };

export function SettingsModal({ onClose, onKeysChanged }: Props) {
  const [keys, setKeys] = useState<ApiKeyConfig[]>([]);
  const [form, setForm] = useState({ ...EMPTY_FORM });
  const [saving, setSaving] = useState(false);
  const [activating, setActivating] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [cliSessions, setCliSessions] = useState<CliSessions | null>(null);
  const [cliUsing, setCliUsing] = useState<string | null>(null);
  const [cliError, setCliError] = useState<string | null>(null);
  const [cliSuccess, setCliSuccess] = useState<string | null>(null);
  const [runtimeStatus, setRuntimeStatus] = useState<RuntimeProviderStatus | null>(null);
  const [cliOptions, setCliOptions] = useState<Record<string, CliSessionOptions>>({});
  const [codexModel, setCodexModel] = useState("gpt-5.4-mini");
  const [codexModelChoice, setCodexModelChoice] = useState("gpt-5.4-mini");
  const [codexIntelligence, setCodexIntelligence] = useState("low");

  useEffect(() => {
    reload();
    invoke<CliSessions>("plugin:axonmind|detect_cli_sessions")
      .then(setCliSessions)
      .catch(() => setCliSessions({ codex: null, claude: null, antigravity: null }));
    invoke<RuntimeProviderStatus>("plugin:axonmind|get_runtime_provider_status")
      .then(setRuntimeStatus)
      .catch(() => setRuntimeStatus(null));
    Promise.all(
      (["codex", "claude", "antigravity"] as const).map(async (cli) => {
        try {
          const opts = await invoke<CliSessionOptions>("plugin:axonmind|get_cli_session_options", { cli });
          return [cli, opts] as const;
        } catch {
          return [cli, { models: [], intelligence_levels: [], default_model: null, default_intelligence: null }] as const;
        }
      })
    ).then((entries) => {
      const next = Object.fromEntries(entries);
      setCliOptions(next);
      const codex = next.codex;
      if (codex?.default_model) {
        const defaultModel = codex.default_model;
        setCodexModel(defaultModel);
        const options = codex.models ?? [];
        setCodexModelChoice(
          options.includes(defaultModel) ? defaultModel : CUSTOM_CODEX_MODEL_OPTION
        );
      }
      if (codex?.default_intelligence) setCodexIntelligence(codex.default_intelligence);
    });
  }, []);

  useEffect(() => {
    const options = cliOptions.codex?.models ?? [];
    if (codexModelChoice !== CUSTOM_CODEX_MODEL_OPTION && !options.includes(codexModelChoice)) {
      setCodexModelChoice(
        options.includes(codexModel) ? codexModel : CUSTOM_CODEX_MODEL_OPTION
      );
    }
  }, [cliOptions, codexModelChoice, codexModel]);

  async function reload() {
    try {
      const result = await invoke<ApiKeyConfig[]>("plugin:axonmind|list_api_keys");
      // Auto-activate the first key if none is active
      if (result.length > 0 && !result.some(k => k.is_active)) {
        await invoke("plugin:axonmind|set_active_provider", { id: result[0].id });
        const updated = await invoke<ApiKeyConfig[]>("plugin:axonmind|list_api_keys");
        setKeys(updated);
      } else {
        setKeys(result);
      }
      const runtime = await invoke<RuntimeProviderStatus>("plugin:axonmind|get_runtime_provider_status");
      setRuntimeStatus(runtime);
      onKeysChanged();
    } catch {
      setKeys([]);
    }
  }

  function handleProviderChange(provider: string) {
    const defaults = PROVIDER_DEFAULTS[provider] ?? PROVIDER_DEFAULTS.custom;
    setForm(f => ({ ...f, provider, base_url: defaults.base_url, model: defaults.model }));
  }

  function startEdit(key: ApiKeyConfig) {
    setForm({ id: key.id, name: key.name, provider: key.provider, base_url: key.base_url, model: key.model, api_key: "" });
    setFormError(null);
  }

  function resetForm() {
    setForm({ ...EMPTY_FORM });
    setFormError(null);
  }

  async function handleSave() {
    if (!form.name.trim()) { setFormError("Name is required"); return; }
    if (!form.api_key.trim() && !form.id) { setFormError("API key is required"); return; }
    if (!form.model.trim()) { setFormError("Model is required"); return; }
    setSaving(true);
    setFormError(null);
    try {
      const saved = await invoke<ApiKeyConfig>("plugin:axonmind|save_api_key", {
        id: form.id ?? null,
        name: form.name.trim(),
        provider: form.provider,
        baseUrl: form.base_url.trim(),
        model: form.model.trim(),
        apiKey: form.api_key,
      });
      // Auto-activate if this is the first key or nothing is currently active
      const noActiveYet = !keys.some(k => k.is_active);
      if (noActiveYet) {
        await invoke("plugin:axonmind|set_active_provider", { id: saved.id });
      }
      resetForm();
      await reload();
    } catch (e) {
      setFormError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete(id: string) {
    try {
      await invoke("plugin:axonmind|delete_api_key", { id });
      await reload();
    } catch (e) {
      setFormError(String(e));
    }
  }

  async function handleSetActive(id: string | null) {
    setActivating(id ?? "__none__");
    try {
      await invoke("plugin:axonmind|set_active_provider", { id });
      await reload();
    } catch (e) {
      setFormError(String(e));
    } finally {
      setActivating(null);
    }
  }

  const providerMeta = PROVIDER_DEFAULTS[form.provider] ?? PROVIDER_DEFAULTS.custom;
  const activeKey = keys.find(k => k.is_active);
  const runtimeLabel = runtimeStatus?.source === "none"
    ? "None"
    : runtimeStatus?.label ?? "Unknown";
  const runtimeHint = runtimeStatus?.source === "cli"
    ? "CLI session"
    : runtimeStatus?.source === "api_key"
      ? "Saved API key"
      : runtimeStatus?.source === "none"
        ? "No provider selected"
        : "Provider in memory";

  return (
    <div
      style={{ position: "fixed", inset: 0, zIndex: 300, background: "rgba(0,0,0,0.7)", display: "flex", alignItems: "center", justifyContent: "center" }}
      onClick={e => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div style={{
        background: "#0f172a", border: "1px solid #1e293b", borderRadius: 12,
        width: 520, maxWidth: "95vw", maxHeight: "90vh", display: "flex", flexDirection: "column",
        overflow: "hidden",
      }}>
        {/* Header */}
        <div style={{ display: "flex", alignItems: "center", padding: "16px 20px", borderBottom: "1px solid #1e293b", flexShrink: 0 }}>
          <span style={{ fontWeight: 600, fontSize: 15, color: "#f1f5f9", flex: 1 }}>LLM Settings</span>
          <div style={{ display: "flex", flexDirection: "column", alignItems: "flex-end", marginRight: 12, gap: 3 }}>
            <span style={{ fontSize: 11, color: "#4ade80", background: "rgba(74,222,128,0.1)", padding: "2px 8px", borderRadius: 4 }}>
              Runtime: {runtimeLabel}
            </span>
            <span style={{ fontSize: 10, color: "#64748b" }}>
              {runtimeHint}
              {activeKey ? ` · Saved default: ${activeKey.name}` : ""}
            </span>
          </div>
          <button onClick={onClose} style={{ background: "none", border: "none", color: "#64748b", cursor: "pointer", fontSize: 20, lineHeight: 1, padding: 0 }}>×</button>
        </div>

        <div style={{ flex: 1, overflowY: "auto", padding: "20px" }}>

          {/* ── Use existing CLI account ────────────────────────────────── */}
          <div style={{ marginBottom: 24 }}>
            <div style={{ fontSize: 11, color: "#475569", fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.05em", marginBottom: 12 }}>
              Use existing account
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {(["codex", "claude", "antigravity"] as const).map(cli => {
                const profile = cliSessions?.[cli] ?? null;
                const meta = CLI_META[cli];
                const detected = profile !== null;
                const displayModel = cli === "codex" ? codexModel : meta.model;
                return (
                  <div key={cli} style={{
                    padding: "10px 12px", borderRadius: 8,
                    background: detected ? "#1e293b" : "#0f172a",
                    border: `1px solid ${detected ? "#334155" : "#1e293b"}`,
                    display: "flex", flexDirection: "column", gap: 6,
                    opacity: detected ? 1 : 0.5,
                  }}>
                    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
                          <span style={{ fontSize: 13, color: "#e2e8f0", fontWeight: 500 }}>{meta.label}</span>
                          {meta.experimental && detected && (
                            <span style={{ fontSize: 10, color: "#fb923c", background: "rgba(251,146,60,0.12)", padding: "1px 6px", borderRadius: 3, flexShrink: 0 }}>
                              experimental
                            </span>
                          )}
                        </div>
                        <div style={{ fontSize: 11, color: "#64748b", marginTop: 2 }}>
                          {detected
                            ? `${profile.name ?? profile.email ?? "logged in"}${profile.name && profile.email ? ` · ${profile.email}` : ""} · ${displayModel}`
                            : "Not detected — run the CLI and log in first"}
                        </div>
                      </div>
                      <button
                        onClick={async () => {
                          setCliUsing(cli);
                          setCliError(null);
                          setCliSuccess(null);
                          try {
                            if (cli === "codex" && !codexModel.trim()) {
                              setCliError("Codex model is required.");
                              return;
                            }
                            const payload = cli === "codex"
                              ? {
                                  cli,
                                  model: codexModel.trim(),
                                  intelligence: codexIntelligence.trim() || null,
                                }
                              : { cli };
                            await invoke("plugin:axonmind|use_cli_session", payload);
                            await reload();
                            onKeysChanged();
                            const suffix = cli === "codex"
                              ? ` (${codexModel}, ${codexIntelligence})`
                              : "";
                            setCliSuccess(`${meta.label}${suffix} connected. You can generate now.`);
                          } catch (e) {
                            setCliError(`${meta.label}: ${String(e)}`);
                          } finally {
                            setCliUsing(null);
                          }
                        }}
                        disabled={!detected || cliUsing === cli}
                        style={{
                          fontSize: 12, padding: "5px 14px", borderRadius: 6, flexShrink: 0,
                          border: `1px solid ${detected ? "#3b82f6" : "#1e293b"}`,
                          background: "transparent",
                          color: !detected || cliUsing === cli ? "#475569" : "#60a5fa",
                          cursor: !detected || cliUsing === cli ? "not-allowed" : "pointer",
                        }}
                      >
                        {cliUsing === cli ? "Connecting…" : "Use this account"}
                      </button>
                    </div>
                    {cli === "codex" && detected && (
                      <div style={{ display: "flex", gap: 8 }}>
                        <select
                          value={codexModelChoice}
                          onChange={e => {
                            const next = e.target.value;
                            setCodexModelChoice(next);
                            if (next !== CUSTOM_CODEX_MODEL_OPTION) {
                              setCodexModel(next);
                            }
                          }}
                          style={{ ...inputStyle, fontSize: 12, padding: "4px 8px", flex: 1 }}
                        >
                          {(cliOptions.codex?.models ?? []).map(model => (
                            <option key={model} value={model}>{model}</option>
                          ))}
                          <option value={CUSTOM_CODEX_MODEL_OPTION}>Custom model…</option>
                        </select>
                        {codexModelChoice === CUSTOM_CODEX_MODEL_OPTION && (
                          <input
                            value={codexModel}
                            onChange={e => setCodexModel(e.target.value)}
                            placeholder="Enter Codex model"
                            style={{ ...inputStyle, fontSize: 12, padding: "4px 8px", flex: 1 }}
                          />
                        )}
                        <select
                          value={codexIntelligence}
                          onChange={e => setCodexIntelligence(e.target.value)}
                          style={{ ...inputStyle, fontSize: 12, padding: "4px 8px", flex: 1 }}
                        >
                          {(cliOptions.codex?.intelligence_levels.length ? cliOptions.codex.intelligence_levels : [codexIntelligence]).map(level => (
                            <option key={level} value={level}>{level}</option>
                          ))}
                        </select>
                      </div>
                    )}
                    {meta.experimental && detected && (
                      <div style={{ fontSize: 11, color: "#78716c", lineHeight: 1.5, borderTop: "1px solid #334155", paddingTop: 6 }}>
                        {meta.note}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
            {cliError && (
              <div style={{ fontSize: 12, color: "#f87171", marginTop: 8 }}>{cliError}</div>
            )}
            {cliSuccess && (
              <div style={{ fontSize: 12, color: "#4ade80", marginTop: 8 }}>{cliSuccess}</div>
            )}
            <div style={{ height: 1, background: "#1e293b", margin: "20px 0" }} />
          </div>

          {/* Add/Edit form */}
          <div style={{ marginBottom: 24 }}>
            <div style={{ fontSize: 11, color: "#475569", fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.05em", marginBottom: 12 }}>
              {form.id ? "Edit Key" : "Add Key"}
            </div>

            {/* Display name */}
            <div style={{ marginBottom: 10 }}>
              <label style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 4 }}>Display name</label>
              <input
                value={form.name}
                onChange={e => setForm(f => ({ ...f, name: e.target.value }))}
                placeholder="My OpenAI Key"
                style={inputStyle}
              />
            </div>

            {/* Provider */}
            <div style={{ marginBottom: 10 }}>
              <label style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 4 }}>Provider</label>
              <select
                value={form.provider}
                onChange={e => handleProviderChange(e.target.value)}
                style={{ ...inputStyle, appearance: "none" as const }}
              >
                {Object.entries(PROVIDER_DEFAULTS).map(([key, val]) => (
                  <option key={key} value={key}>{val.label}</option>
                ))}
              </select>
            </div>

            {/* Base URL (shown for non-OpenAI) */}
            {providerMeta.showUrl && (
              <div style={{ marginBottom: 10 }}>
                <label style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 4 }}>Base URL</label>
                <input
                  value={form.base_url}
                  onChange={e => setForm(f => ({ ...f, base_url: e.target.value }))}
                  placeholder="http://localhost:11434/v1"
                  style={inputStyle}
                />
              </div>
            )}

            {/* Model */}
            <div style={{ marginBottom: 10 }}>
              <label style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 4 }}>Model</label>
              <input
                value={form.model}
                onChange={e => setForm(f => ({ ...f, model: e.target.value }))}
                placeholder="gpt-4o-mini"
                style={inputStyle}
              />
            </div>

            {/* API key */}
            <div style={{ marginBottom: 14 }}>
              <label style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 4 }}>
                API Key{form.id ? " (leave blank to keep existing)" : ""}
              </label>
              <input
                type="password"
                value={form.api_key}
                onChange={e => setForm(f => ({ ...f, api_key: e.target.value }))}
                placeholder={form.id ? "••••••••" : "sk-..."}
                style={inputStyle}
                autoComplete="off"
              />
            </div>

            {formError && (
              <div style={{ fontSize: 12, color: "#f87171", marginBottom: 10 }}>{formError}</div>
            )}

            <div style={{ display: "flex", gap: 8 }}>
              <button
                onClick={handleSave}
                disabled={saving}
                style={{
                  flex: 1, padding: "8px 0", borderRadius: 8, border: "none",
                  background: saving ? "#1e293b" : "#3b82f6",
                  color: saving ? "#475569" : "#fff",
                  fontSize: 13, fontWeight: 600, cursor: saving ? "not-allowed" : "pointer",
                }}
              >
                {saving ? "Saving…" : form.id ? "Update" : "Save"}
              </button>
              {form.id && (
                <button
                  onClick={resetForm}
                  style={{ padding: "8px 16px", borderRadius: 8, border: "1px solid #334155", background: "transparent", color: "#94a3b8", fontSize: 13, cursor: "pointer" }}
                >
                  Cancel
                </button>
              )}
            </div>
          </div>

          {/* Saved keys list */}
          {keys.length > 0 && (
            <div>
              <div style={{ fontSize: 11, color: "#475569", fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.05em", marginBottom: 10 }}>
                Saved Keys
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {keys.map(key => (
                  <div key={key.id} style={{
                    padding: "10px 12px", borderRadius: 8,
                    background: key.is_active ? "rgba(59,130,246,0.1)" : "#1e293b",
                    border: `1px solid ${key.is_active ? "#3b82f6" : "#334155"}`,
                    display: "flex", alignItems: "center", gap: 10,
                  }}>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 2 }}>
                        <span style={{ fontSize: 13, color: "#e2e8f0", fontWeight: 500, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                          {key.name}
                        </span>
                        <span style={{ fontSize: 10, color: "#64748b", background: "#0f172a", padding: "1px 6px", borderRadius: 3, flexShrink: 0 }}>
                          {PROVIDER_DEFAULTS[key.provider]?.label ?? key.provider}
                        </span>
                        {key.is_active && (
                          <span style={{ fontSize: 10, color: "#4ade80", background: "rgba(74,222,128,0.1)", padding: "1px 6px", borderRadius: 3, flexShrink: 0 }}>
                            active
                          </span>
                        )}
                      </div>
                      <div style={{ fontSize: 11, color: "#475569", fontFamily: "monospace" }}>
                        {key.model} · {key.key_masked}
                      </div>
                    </div>

                    {!key.is_active && (
                      <button
                        onClick={() => handleSetActive(key.id)}
                        disabled={activating === key.id}
                        style={{ fontSize: 11, padding: "3px 10px", borderRadius: 5, border: "1px solid #334155", background: "transparent", color: "#94a3b8", cursor: "pointer", flexShrink: 0 }}
                      >
                        {activating === key.id ? "…" : "Activate"}
                      </button>
                    )}
                    {key.is_active && (
                      <button
                        onClick={() => handleSetActive(null)}
                        disabled={activating === "__none__"}
                        style={{ fontSize: 11, padding: "3px 10px", borderRadius: 5, border: "1px solid #334155", background: "transparent", color: "#64748b", cursor: "pointer", flexShrink: 0 }}
                      >
                        Deactivate
                      </button>
                    )}
                    <button
                      onClick={() => startEdit(key)}
                      style={{ fontSize: 11, padding: "3px 8px", borderRadius: 5, border: "1px solid #334155", background: "transparent", color: "#94a3b8", cursor: "pointer", flexShrink: 0 }}
                    >
                      Edit
                    </button>
                    <button
                      onClick={() => handleDelete(key.id)}
                      style={{ fontSize: 13, padding: "2px 6px", borderRadius: 5, border: "none", background: "transparent", color: "#475569", cursor: "pointer", flexShrink: 0, lineHeight: 1 }}
                      title="Delete"
                    >
                      ×
                    </button>
                  </div>
                ))}
              </div>

              {runtimeStatus?.has_provider && (
                <p style={{ fontSize: 11, color: "#475569", marginTop: 12, lineHeight: 1.5 }}>
                  LLM extraction is enabled. Runtime provider: {runtimeLabel}.
                </p>
              )}
              {!runtimeStatus?.has_provider && (
                <p style={{ fontSize: 11, color: "#64748b", marginTop: 12, lineHeight: 1.5 }}>
                  No active provider — Brain Map will use rule-based extraction only (limited to financial KPIs).
                  Activate a key above for full entity extraction on any document type.
                </p>
              )}
            </div>
          )}

          {keys.length === 0 && (
            <p style={{ fontSize: 12, color: "#475569", lineHeight: 1.6 }}>
              No API keys saved yet. Add one above to enable AI-powered entity extraction.<br />
              Keys are stored securely in your OS keychain.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

const inputStyle: React.CSSProperties = {
  width: "100%", padding: "8px 10px", borderRadius: 6,
  background: "#1e293b", border: "1px solid #334155",
  color: "#f1f5f9", fontSize: 13, boxSizing: "border-box",
  outline: "none",
};
