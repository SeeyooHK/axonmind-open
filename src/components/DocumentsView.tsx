import React, { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BrainMapView, type Summary } from "./graph/BrainMapView";
import { InspectorPanel } from "./InspectorPanel";
import { ContentSearchModal } from "./ContentSearchModal";
import { FileVisualizationModal } from "./FileVisualizationModal";
import type { StagedItem } from "./GenerationStaging";
import { toGraphElements } from "@axonmind/react";
import type { AxonGraphElements, AxonGraphNode } from "@axonmind/react";

// Mirrors the Rust `DocumentSummary` (serde snake_case).
interface DocumentSummary {
  node_id: string;
  name: string;
  source_path: string | null;
  sha256: string | null;
  indexed_at: number; // unix seconds
  concept_count: number;
  evidence_count: number;
}

interface Props {
  onBack: () => void;
  /** Called after a remove/regenerate so the host can refresh the map. */
  onChanged?: () => void;
  elements?: AxonGraphElements;
}

const PAGE_SIZE = 50;

export function DocumentsView({ onBack, onChanged, elements }: Props) {
  const [docs, setDocs] = useState<DocumentSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(0);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [busyId, setBusyId] = useState<string | null>(null);
  const [busyKind, setBusyKind] = useState<"removing" | "regenerating" | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ message: string; run: () => Promise<void> } | null>(null);
  const [summaryData, setSummaryData] = useState<Summary | null>(null);
  const [summaryView, setSummaryView] = useState<"graph" | "json">("graph");
  const [summaryBusy, setSummaryBusy] = useState(false);
  const [summarySelectedNode, setSummarySelectedNode] = useState<AxonGraphNode | undefined>();
  const [showContentSearch, setShowContentSearch] = useState(false);
  const [visualizingItem, setVisualizingItem] = useState<StagedItem | null>(null);
  const [summaryElements, setSummaryElements] = useState<AxonGraphElements>(
    elements ?? { nodes: [], edges: [] }
  );

  useEffect(() => { void reload(); }, []);
  useEffect(() => {
    if (elements) setSummaryElements(elements);
  }, [elements]);

  async function ensureSummaryElements() {
    if (summaryElements.nodes.length > 0) return;
    try {
      const exp = await invoke<any>("plugin:axonmind|export_json");
      setSummaryElements(toGraphElements(exp));
    } catch {
      // Keep the summary usable even if graph export fails; node-select actions may be disabled.
    }
  }

  async function reload() {
    setLoading(true);
    try {
      const result = await invoke<DocumentSummary[]>("plugin:axonmind|list_documents");
      setDocs(result);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return docs;
    return docs.filter(d =>
      d.name.toLowerCase().includes(q) || (d.source_path ?? "").toLowerCase().includes(q));
  }, [docs, search]);

  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const safePage = Math.min(page, pageCount - 1);
  const pageRows = filtered.slice(safePage * PAGE_SIZE, safePage * PAGE_SIZE + PAGE_SIZE);
  const selectedDocIds = useMemo(
    () => (selected.size > 0 ? [...selected].sort() : undefined),
    [selected]
  );

  const allFilteredSelected = filtered.length > 0 && filtered.every(d => selected.has(d.node_id));

  function toggleSelect(id: string) {
    setSelected(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }

  function toggleSelectAll() {
    setSelected(prev => {
      if (filtered.every(d => prev.has(d.node_id))) {
        const next = new Set(prev);
        for (const d of filtered) next.delete(d.node_id);
        return next;
      }
      const next = new Set(prev);
      for (const d of filtered) next.add(d.node_id);
      return next;
    });
  }

  async function removeOne(id: string) {
    await invoke("plugin:axonmind|remove_document", { nodeId: id });
  }

  async function regenerateOne(id: string) {
    await invoke("plugin:axonmind|regenerate_document", { nodeId: id });
  }

  function askRemove(id: string, label: string) {
    setConfirm({
      message: `Remove "${label}" and everything derived only from it? Shared concepts are kept. This cannot be undone.`,
      run: async () => {
        setBusyId(id); setBusyKind("removing");
        try {
          await removeOne(id);
          setSelected(prev => { const n = new Set(prev); n.delete(id); return n; });
          await reload();
          onChanged?.();
        } catch (e) { setError(String(e)); } finally { setBusyId(null); setBusyKind(null); }
      },
    });
  }

  function askRegenerate(id: string, label: string) {
    setConfirm({
      message: `Regenerate "${label}"? Its current extracted data is removed and the file is reprocessed from scratch.`,
      run: async () => {
        setBusyId(id); setBusyKind("regenerating");
        try {
          await regenerateOne(id);
          await reload();
          onChanged?.();
        } catch (e) { setError(String(e)); } finally { setBusyId(null); setBusyKind(null); }
      },
    });
  }

  function askRemoveSelected() {
    const ids = [...selected];
    setConfirm({
      message: `Remove ${ids.length} selected document${ids.length === 1 ? "" : "s"} and data derived only from them? Shared concepts are kept. This cannot be undone.`,
      run: async () => {
        setBusyId("__bulk__"); setBusyKind("removing");
        try {
          for (const id of ids) await removeOne(id);
          setSelected(new Set());
          await reload();
          onChanged?.();
        } catch (e) { setError(String(e)); } finally { setBusyId(null); setBusyKind(null); }
      },
    });
  }

  function askRegenerateSelected() {
    const ids = [...selected];
    setConfirm({
      message: `Regenerate ${ids.length} selected document${ids.length === 1 ? "" : "s"}? Each is reprocessed from scratch with AI extraction — this can take a while.`,
      run: async () => {
        setBusyId("__bulk__"); setBusyKind("regenerating");
        try {
          for (const id of ids) await regenerateOne(id);
          await reload();
          onChanged?.();
        } catch (e) { setError(String(e)); } finally { setBusyId(null); setBusyKind(null); }
      },
    });
  }

  async function revealPath(d: DocumentSummary) {
    if (!d.source_path) return;
    try {
      await navigator.clipboard.writeText(d.source_path);
      setCopiedId(d.node_id);
      setTimeout(() => setCopiedId(c => (c === d.node_id ? null : c)), 1500);
    } catch { /* clipboard unavailable */ }
  }

  // Build the brain-map summary (≤10 categories) from the already-indexed graph. This does NOT
  // re-extract documents — it groups the existing nodes/edges via the LLM (or kind fallback).
  async function generateBrainMap(scopedMode: "auto" | "regenerate" = "auto") {
    setSummaryBusy(true);
    setError(null);
    try {
      // Scope to the checked files when any are selected; otherwise the whole indexed graph.
      const docIds = selectedDocIds;
      const res = await invoke<Summary>("plugin:axonmind|suggest_summary", {
        docIds,
        scopedMode: docIds && docIds.length > 0 ? scopedMode : undefined,
      });
      await ensureSummaryElements();
      setSummaryData(res);
      setSummaryView("graph");
      setSummarySelectedNode(undefined);
    } catch (e) {
      setError(String(e));
    } finally {
      setSummaryBusy(false);
    }
  }

  return (
    <div style={{ height: "100vh", display: "flex", flexDirection: "column", background: "#020617" }}>
      <style>{INDETERMINATE_KEYFRAMES}</style>
      {/* Header */}
      <div style={{ flexShrink: 0, padding: "10px 18px", display: "flex", alignItems: "center", gap: 14, borderBottom: "1px solid #1e293b" }}>
        <button onClick={onBack} style={{ padding: "5px 12px", borderRadius: 8, border: "1px solid #334155", background: "transparent", color: "#94a3b8", fontSize: 13, cursor: "pointer" }}>‹ Back</button>
        <span style={{ color: "#f1f5f9", fontWeight: 700, fontSize: 16 }}>Processed Files</span>
        <span style={{ color: "#475569", fontSize: 12 }}>{filtered.length} of {docs.length}</span>
        <div style={{ flex: 1 }} />
        <input
          value={search}
          onChange={e => { setSearch(e.target.value); setPage(0); }}
          placeholder="Search name or path…"
          style={{ ...inputStyle, width: 240 }}
        />
        <button
          onClick={() => setShowContentSearch(true)}
          disabled={docs.length === 0}
          title={selected.size > 0
            ? `Search inside the ${selected.size} selected file(s)`
            : "Search inside all indexed documents"}
          style={{ padding: "6px 14px", borderRadius: 8, border: "1px solid #334155", background: "transparent", color: docs.length === 0 ? "#334155" : "#94a3b8", fontSize: 13, fontWeight: 600, cursor: docs.length === 0 ? "default" : "pointer", whiteSpace: "nowrap" }}
        >
          {selected.size > 0 ? `Search Contents (${selected.size})` : "Search Contents"}
        </button>
        <button
          onClick={() => void generateBrainMap("auto")}
          disabled={summaryBusy || docs.length === 0}
          title={selected.size > 0
            ? `Open the cached brain-map summary for the ${selected.size} selected file(s), or build and cache it if missing/outdated`
            : "Build or open the persisted workspace brain-map summary — no re-extraction"}
          style={{ padding: "6px 14px", borderRadius: 8, border: "1px solid #1d4ed8", background: summaryBusy ? "#1e293b" : "#1d4ed8", color: "#fff", fontSize: 13, fontWeight: 600, cursor: summaryBusy ? "default" : "pointer", whiteSpace: "nowrap" }}
        >
          {summaryBusy ? "Generating…" : selected.size > 0 ? `Open Brain Map (${selected.size})` : "Open Brain Map"}
        </button>
        <button
          onClick={() => void generateBrainMap("regenerate")}
          disabled={summaryBusy || docs.length === 0}
          title={selected.size > 0
            ? `Force a fresh re-categorization for the ${selected.size} selected file(s), then replace cache`
            : "Regenerate the persisted workspace summary categories from current graph state"}
          style={{ padding: "6px 12px", borderRadius: 8, border: "1px solid #334155", background: "transparent", color: "#94a3b8", fontSize: 12, fontWeight: 600, cursor: summaryBusy ? "default" : "pointer", whiteSpace: "nowrap" }}
        >
          {selected.size > 0 ? `Regenerate Map (${selected.size})` : "Regenerate Map"}
        </button>
      </div>

      {/* Bulk bar */}
      {selected.size > 0 && (
        <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "8px 18px", background: "rgba(59,130,246,0.08)", borderBottom: "1px solid #1e293b", flexShrink: 0 }}>
          <span style={{ fontSize: 12, color: "#94a3b8" }}>{selected.size} selected</span>
          <button
            onClick={askRegenerateSelected}
            disabled={busyId === "__bulk__"}
            title="Reprocess selected files from scratch with AI extraction"
            style={{ fontSize: 12, padding: "4px 12px", borderRadius: 6, border: "1px solid #1d4ed8", background: "transparent", color: "#60a5fa", cursor: "pointer" }}
          >
            {busyId === "__bulk__" && busyKind === "regenerating" ? "Regenerating…" : "Regenerate selected"}
          </button>
          <button
            onClick={askRemoveSelected}
            disabled={busyId === "__bulk__"}
            style={{ fontSize: 12, padding: "4px 12px", borderRadius: 6, border: "1px solid #7f1d1d", background: "transparent", color: "#f87171", cursor: "pointer" }}
          >
            {busyId === "__bulk__" && busyKind === "removing" ? "Removing…" : "Remove selected"}
          </button>
          <button onClick={() => setSelected(new Set())} disabled={busyId === "__bulk__"} style={{ fontSize: 12, padding: "4px 10px", borderRadius: 6, border: "1px solid #334155", background: "transparent", color: "#94a3b8", cursor: "pointer" }}>
            Clear
          </button>
          {busyId === "__bulk__" && <div style={{ flex: 1, maxWidth: 280 }}><IndeterminateBar label={busyKind === "regenerating" ? "Regenerating… (AI, may take a while)" : "Removing…"} /></div>}
        </div>
      )}

      {/* Body */}
      <div style={{ flex: 1, overflowY: "auto", padding: "8px 18px 16px" }}>
        {error && <div style={{ fontSize: 12, color: "#f87171", margin: "8px 0" }}>{error}</div>}
        {loading ? (
          <p style={{ color: "#475569", fontSize: 13 }}>Loading…</p>
        ) : filtered.length === 0 ? (
          <p style={{ color: "#475569", fontSize: 13 }}>{docs.length === 0 ? "No documents processed yet." : "No files match your search."}</p>
        ) : (
          <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13 }}>
            <thead>
              <tr style={{ color: "#475569", fontSize: 11, textTransform: "uppercase", letterSpacing: "0.04em" }}>
                <th style={{ ...th, width: 28 }}>
                  <input type="checkbox" checked={allFilteredSelected} onChange={toggleSelectAll} title="Select all (filtered)" />
                </th>
                <th style={{ ...th, textAlign: "left" }}>Name</th>
                <th style={th}>Indexed</th>
                <th style={th}>Concepts</th>
                <th style={th}>Evidence</th>
                <th style={{ ...th, textAlign: "right" }}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {pageRows.map(d => {
                const busy = busyId === d.node_id;
                return (
                  <React.Fragment key={d.node_id}>
                    <tr style={{ borderTop: "1px solid #1e293b" }}>
                      <td style={td}><input type="checkbox" checked={selected.has(d.node_id)} onChange={() => toggleSelect(d.node_id)} /></td>
                      <td style={{ ...td, textAlign: "left", maxWidth: 420 }}>
                        <div style={{ color: "#e2e8f0", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{d.name}</div>
                        {d.source_path && <div style={{ color: "#475569", fontSize: 11, fontFamily: "monospace", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{d.source_path}</div>}
                      </td>
                      <td style={{ ...td, color: "#94a3b8", fontSize: 11, whiteSpace: "nowrap" }}>{new Date(d.indexed_at * 1000).toLocaleDateString()}</td>
                      <td style={{ ...td, color: "#94a3b8" }}>{d.concept_count}</td>
                      <td style={{ ...td, color: "#94a3b8" }}>{d.evidence_count}</td>
                      <td style={{ ...td, textAlign: "right", whiteSpace: "nowrap" }}>
                        {busy ? (
                          <div style={{ display: "inline-block", minWidth: 200, textAlign: "left" }}>
                            <IndeterminateBar label={busyKind === "regenerating" ? "Regenerating… (AI, may take a while)" : "Removing…"} />
                          </div>
                        ) : (
                          <>
                            <button
                              onClick={() => {
                                if (d.source_path) {
                                  setVisualizingItem({ id: d.node_id, path: d.source_path, displayPath: d.name, status: "ready" });
                                } else {
                                  setExpandedId(id => id === d.node_id ? null : d.node_id);
                                }
                              }}
                              style={actionBtn}
                              title="Inspect"
                            >Inspect</button>
                            <button onClick={() => void revealPath(d)} disabled={!d.source_path} style={actionBtn} title="Copy file path">{copiedId === d.node_id ? "Copied" : "Reveal"}</button>
                            <button onClick={() => askRegenerate(d.node_id, d.name)} style={actionBtn} title="Reprocess from scratch">Regenerate</button>
                            <button onClick={() => askRemove(d.node_id, d.name)} style={{ ...actionBtn, color: "#f87171", borderColor: "#7f1d1d" }} title="Remove">Remove</button>
                          </>
                        )}
                      </td>
                    </tr>
                    {expandedId === d.node_id && (
                      <tr style={{ background: "#0b1120" }}>
                        <td />
                        <td colSpan={5} style={{ padding: "8px 8px 12px", color: "#94a3b8", fontSize: 12 }}>
                          <div><span style={{ color: "#475569" }}>Path: </span><span style={{ fontFamily: "monospace" }}>{d.source_path ?? "—"}</span></div>
                          <div><span style={{ color: "#475569" }}>SHA-256: </span><span style={{ fontFamily: "monospace" }}>{d.sha256 ?? "—"}</span></div>
                          <div><span style={{ color: "#475569" }}>Indexed: </span>{new Date(d.indexed_at * 1000).toLocaleString()}</div>
                          <div><span style={{ color: "#475569" }}>Concepts: </span>{d.concept_count} · <span style={{ color: "#475569" }}>Evidence: </span>{d.evidence_count}</div>
                        </td>
                      </tr>
                    )}
                  </React.Fragment>
                );
              })}
            </tbody>
          </table>
        )}
      </div>

      {/* Pagination */}
      {pageCount > 1 && (
        <div style={{ display: "flex", alignItems: "center", justifyContent: "center", gap: 14, padding: "10px", borderTop: "1px solid #1e293b", flexShrink: 0 }}>
          <button onClick={() => setPage(p => Math.max(0, p - 1))} disabled={safePage === 0} style={pageBtn}>‹ Prev</button>
          <span style={{ fontSize: 12, color: "#64748b" }}>Page {safePage + 1} of {pageCount}</span>
          <button onClick={() => setPage(p => Math.min(pageCount - 1, p + 1))} disabled={safePage >= pageCount - 1} style={pageBtn}>Next ›</button>
        </div>
      )}

      {/* Confirm dialog */}
      {confirm && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 310, background: "rgba(0,0,0,0.6)", display: "flex", alignItems: "center", justifyContent: "center" }}
          onClick={e => { if (e.target === e.currentTarget) setConfirm(null); }}
        >
          <div style={{ background: "#0f172a", border: "1px solid #334155", borderRadius: 10, padding: 20, width: 420, maxWidth: "92vw" }}>
            <p style={{ color: "#e2e8f0", fontSize: 13, lineHeight: 1.5, margin: "0 0 16px" }}>{confirm.message}</p>
            <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
              <button onClick={() => setConfirm(null)} style={{ padding: "7px 14px", borderRadius: 7, border: "1px solid #334155", background: "transparent", color: "#94a3b8", fontSize: 13, cursor: "pointer" }}>Cancel</button>
              <button
                onClick={() => { const c = confirm; setConfirm(null); void c.run(); }}
                style={{ padding: "7px 14px", borderRadius: 7, border: "none", background: "#dc2626", color: "#fff", fontSize: 13, fontWeight: 600, cursor: "pointer" }}
              >
                Confirm
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Content search modal */}
      {showContentSearch && (
        <ContentSearchModal
          docs={docs}
          initialDocIds={[...selected]}
          onClose={() => setShowContentSearch(false)}
        />
      )}

      {/* Side-by-side original / extracted view */}
      {visualizingItem && (
        <FileVisualizationModal
          item={visualizingItem}
          onClose={() => setVisualizingItem(null)}
          zIndex={320}
        />
      )}

      {/* Brain-map summary — radial graph (default) with a JSON toggle. */}
      {summaryData !== null && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 320, background: "rgba(0,0,0,0.6)", display: "flex", alignItems: "center", justifyContent: "center" }}
          onClick={e => {
            if (e.target === e.currentTarget) {
              setSummaryData(null);
              setSummarySelectedNode(undefined);
            }
          }}
        >
          <div style={{ background: "#0f172a", border: "1px solid #334155", borderRadius: 10, padding: 16, width: 880, maxWidth: "94vw", height: "82vh", display: "flex", flexDirection: "column" }}>
            <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
              <span style={{ color: "#f1f5f9", fontWeight: 700, fontSize: 14 }}>Brain Map Summary</span>
              <span style={{ color: "#475569", fontSize: 11 }}>source: {summaryData.source}</span>
              <div style={{ flex: 1 }} />
              <div style={{ display: "flex", border: "1px solid #334155", borderRadius: 7, overflow: "hidden" }}>
                {(["graph", "json"] as const).map(v => (
                  <button
                    key={v}
                    onClick={() => setSummaryView(v)}
                    style={{ padding: "4px 12px", border: "none", background: summaryView === v ? "#1d4ed8" : "transparent", color: summaryView === v ? "#fff" : "#94a3b8", fontSize: 12, cursor: "pointer" }}
                  >
                    {v === "graph" ? "Graph" : "JSON"}
                  </button>
                ))}
              </div>
              {summaryView === "json" && (
                <button onClick={() => void navigator.clipboard.writeText(JSON.stringify(summaryData, null, 2))} style={pageBtn}>Copy</button>
              )}
              <button
                onClick={() => {
                  setSummaryData(null);
                  setSummarySelectedNode(undefined);
                }}
                style={pageBtn}
              >
                Close
              </button>
            </div>
            <div style={{ flex: 1, minHeight: 0, borderRadius: 8, overflow: "hidden", border: "1px solid #1e293b" }}>
              {summaryView === "graph" ? (
                <div style={{ height: "100%", display: "flex" }}>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <BrainMapView
                      elements={summaryElements}
                      initialSummary={summaryData}
                      scopeDocIds={selectedDocIds}
                      onSelectNode={setSummarySelectedNode}
                      style={{ height: "100%" }}
                    />
                  </div>
                  <InspectorPanel
                    node={summarySelectedNode}
                    onClose={() => setSummarySelectedNode(undefined)}
                  />
                </div>
              ) : (
                <pre style={{ margin: 0, height: "100%", overflow: "auto", background: "#020617", padding: 12, color: "#cbd5e1", fontSize: 12, lineHeight: 1.5 }}>{JSON.stringify(summaryData, null, 2)}</pre>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// Indeterminate progress bar — the regenerate step is opaque (LLM), so an animated bar with a
// label is the honest signal that work is in progress, rather than a misleading percentage.
const INDETERMINATE_KEYFRAMES = `@keyframes axonmind-indeterminate { 0% { left: -40%; } 100% { left: 100%; } }`;

function IndeterminateBar({ label }: { label?: string }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4, width: "100%" }}>
      {label && <span style={{ fontSize: 11, color: "#3b82f6" }}>{label}</span>}
      <div style={{ position: "relative", height: 4, background: "#1e293b", borderRadius: 2, overflow: "hidden" }}>
        <div style={{ position: "absolute", top: 0, height: "100%", width: "40%", borderRadius: 2, background: "#3b82f6", animation: "axonmind-indeterminate 1.1s ease-in-out infinite" }} />
      </div>
    </div>
  );
}

const inputStyle: React.CSSProperties = {
  padding: "7px 10px", borderRadius: 6, background: "#1e293b", border: "1px solid #334155",
  color: "#f1f5f9", fontSize: 13, boxSizing: "border-box", outline: "none",
};

const th: React.CSSProperties = { padding: "6px 8px", textAlign: "center", fontWeight: 600 };
const td: React.CSSProperties = { padding: "8px", textAlign: "center", verticalAlign: "top" };
const actionBtn: React.CSSProperties = {
  fontSize: 11, padding: "3px 8px", marginLeft: 4, borderRadius: 5,
  border: "1px solid #334155", background: "transparent", color: "#94a3b8", cursor: "pointer",
};
const pageBtn: React.CSSProperties = {
  fontSize: 12, padding: "4px 12px", borderRadius: 6, border: "1px solid #334155",
  background: "transparent", color: "#94a3b8", cursor: "pointer",
};
