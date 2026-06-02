import React, { useEffect, useRef, useState } from "react";
import { useAxonMind } from "@axonmind/react";
import type { RetrievedSection } from "@axonmind/types";
import { FileVisualizationModal } from "./FileVisualizationModal";
import type { StagedItem } from "./GenerationStaging";

interface DocInfo {
  node_id: string;
  name: string;
  source_path: string | null;
}

interface Props {
  docs: DocInfo[];
  initialDocIds: string[];
  onClose: () => void;
}

export function ContentSearchModal({ docs, initialDocIds, onClose }: Props) {
  const { transport } = useAxonMind();
  const [query, setQuery] = useState("");
  const [scopedIds, setScopedIds] = useState<string[]>(initialDocIds);
  const [results, setResults] = useState<RetrievedSection[] | null>(null);
  const [reasoningApplied, setReasoningApplied] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [viewingItem, setViewingItem] = useState<StagedItem | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  async function runSearch() {
    const q = query.trim();
    if (!q) return;
    setLoading(true);
    setError(null);
    try {
      const out = await transport.reasoningSearch({
        query: q,
        doc_node_ids: scopedIds.length > 0 ? scopedIds : undefined,
        max_results: 20,
      });
      setResults(out.sections);
      setReasoningApplied(out.reasoning_applied);
    } catch (e) {
      setError(String(e));
      setResults(null);
    } finally {
      setLoading(false);
    }
  }

  function openDoc(docNodeId: string) {
    const doc = docs.find(d => d.node_id === docNodeId);
    if (!doc?.source_path) return;
    setViewingItem({ id: doc.node_id, path: doc.source_path, displayPath: doc.name, status: "ready" });
  }

  function removeScope(id: string) {
    setScopedIds(prev => prev.filter(x => x !== id));
  }

  const isWholeLibrary = scopedIds.length === 0;
  const scopedDocs = docs.filter(d => scopedIds.includes(d.node_id));

  return (
    <>
      {/* Overlay */}
      <div
        style={{ position: "fixed", inset: 0, zIndex: 315, background: "rgba(0,0,0,0.6)", display: "flex", alignItems: "center", justifyContent: "center" }}
        onClick={e => { if (e.target === e.currentTarget) onClose(); }}
      >
        <div style={{ background: "#0f172a", border: "1px solid #334155", borderRadius: 10, width: 760, maxWidth: "94vw", maxHeight: "82vh", display: "flex", flexDirection: "column" }}>
          {/* Header */}
          <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "12px 16px", borderBottom: "1px solid #1e293b", flexShrink: 0 }}>
            <span style={{ color: "#f1f5f9", fontWeight: 700, fontSize: 14 }}>Search Contents</span>
            <div style={{ flex: 1 }} />
            <button onClick={onClose} style={{ background: "none", border: "none", color: "#64748b", cursor: "pointer", fontSize: 20, lineHeight: 1, padding: 0 }}>×</button>
          </div>

          {/* Scope row */}
          <div style={{ display: "flex", alignItems: "center", flexWrap: "wrap", gap: 6, padding: "8px 16px", borderBottom: "1px solid #1e293b", flexShrink: 0, minHeight: 38 }}>
            <span style={{ fontSize: 11, color: "#475569", whiteSpace: "nowrap" }}>
              {isWholeLibrary ? `Searching all ${docs.length} document${docs.length === 1 ? "" : "s"}` : "Scope:"}
            </span>
            {!isWholeLibrary && scopedDocs.map(d => (
              <span key={d.node_id} style={{ display: "inline-flex", alignItems: "center", gap: 4, padding: "2px 8px", background: "rgba(59,130,246,0.12)", border: "1px solid #1d4ed8", borderRadius: 4, fontSize: 11, color: "#93c5fd", maxWidth: 200, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{d.name}</span>
                <button onClick={() => removeScope(d.node_id)} style={{ background: "none", border: "none", color: "#60a5fa", cursor: "pointer", fontSize: 12, lineHeight: 1, padding: 0, flexShrink: 0 }}>✕</button>
              </span>
            ))}
            {!isWholeLibrary && (
              <button onClick={() => setScopedIds([])} style={{ fontSize: 11, color: "#475569", background: "none", border: "none", cursor: "pointer", padding: 0, textDecoration: "underline", whiteSpace: "nowrap" }}>
                search all instead
              </button>
            )}
          </div>

          {/* Query input */}
          <div style={{ display: "flex", gap: 8, padding: "12px 16px", flexShrink: 0 }}>
            <input
              ref={inputRef}
              value={query}
              onChange={e => setQuery(e.target.value)}
              onKeyDown={e => { if (e.key === "Enter") void runSearch(); }}
              placeholder="Type a question or keywords…"
              style={{ flex: 1, padding: "8px 12px", borderRadius: 6, background: "#1e293b", border: "1px solid #334155", color: "#f1f5f9", fontSize: 13, outline: "none" }}
            />
            <button
              onClick={() => void runSearch()}
              disabled={!query.trim() || loading}
              style={{ padding: "8px 18px", borderRadius: 6, border: "none", background: !query.trim() || loading ? "#1e293b" : "#1d4ed8", color: !query.trim() || loading ? "#475569" : "#fff", fontSize: 13, fontWeight: 600, cursor: !query.trim() || loading ? "default" : "pointer", whiteSpace: "nowrap" }}
            >
              {loading ? "Searching…" : "Search"}
            </button>
          </div>

          {/* Results area */}
          <div style={{ flex: 1, overflowY: "auto", minHeight: 0 }}>
            {loading && (
              <div style={{ padding: "0 16px 12px" }}>
                <IndeterminateBar />
              </div>
            )}

            {error && (
              <div style={{ margin: "0 16px 12px", padding: "8px 12px", background: "rgba(248,113,113,0.1)", border: "1px solid #7f1d1d", borderRadius: 6, fontSize: 12, color: "#fca5a5" }}>
                {error}
              </div>
            )}

            {!loading && results !== null && results.length === 0 && (
              <p style={{ padding: "0 16px", color: "#475569", fontSize: 13 }}>No matching sections found.</p>
            )}

            {results !== null && results.length > 0 && (
              <div style={{ padding: "0 16px 4px" }}>
                <div style={{ fontSize: 11, color: "#475569", marginBottom: 6 }}>{results.length} result{results.length === 1 ? "" : "s"}</div>
                {results.map(section => {
                  const sourceDoc = docs.find(d => d.node_id === section.doc_node_id);
                  const canOpen = !!sourceDoc?.source_path;
                  const breadcrumb = section.path.length > 1 ? section.path.join(" › ") : null;
                  const excerpt = section.text.length > 220
                    ? section.text.slice(0, 220).trimEnd() + "…"
                    : section.text;

                  return (
                    <div
                      key={section.section_id}
                      onClick={() => canOpen && openDoc(section.doc_node_id)}
                      style={{
                        padding: "10px 12px", marginBottom: 6, borderRadius: 6,
                        background: "#0b1120", border: "1px solid #1e293b",
                        cursor: canOpen ? "pointer" : "default",
                        transition: "border-color 0.12s",
                      }}
                      onMouseEnter={e => { if (canOpen) (e.currentTarget as HTMLDivElement).style.borderColor = "#334155"; }}
                      onMouseLeave={e => { (e.currentTarget as HTMLDivElement).style.borderColor = "#1e293b"; }}
                    >
                      <div style={{ display: "flex", alignItems: "baseline", gap: 8, marginBottom: 4 }}>
                        <span style={{ fontSize: 13, fontWeight: 600, color: "#e2e8f0" }}>{section.title}</span>
                        {breadcrumb && (
                          <span style={{ fontSize: 11, color: "#475569", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{breadcrumb}</span>
                        )}
                        <div style={{ flex: 1 }} />
                        <span style={{ fontSize: 11, color: "#334155", whiteSpace: "nowrap", flexShrink: 0 }}>{sourceDoc?.name ?? section.doc_node_id}</span>
                      </div>
                      {excerpt && (
                        <div style={{ fontSize: 12, color: "#64748b", lineHeight: 1.5 }}>{excerpt}</div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}

            {/* No-provider note — shown once at the bottom, not stamped on each row */}
            {results !== null && results.length > 0 && !reasoningApplied && (
              <div style={{ padding: "4px 16px 12px", fontSize: 11, color: "#475569" }}>
                ℹ Keyword-ranked — connect an AI provider in Settings for smarter results.
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Document viewer — opened on result click */}
      {viewingItem && (
        <FileVisualizationModal item={viewingItem} onClose={() => setViewingItem(null)} zIndex={330} />
      )}
    </>
  );
}

const KEYFRAMES = `@keyframes axonmind-indeterminate { 0% { left: -40%; } 100% { left: 100%; } }`;

function IndeterminateBar() {
  return (
    <>
      <style>{KEYFRAMES}</style>
      <div style={{ position: "relative", height: 3, background: "#1e293b", borderRadius: 2, overflow: "hidden" }}>
        <div style={{ position: "absolute", top: 0, height: "100%", width: "40%", borderRadius: 2, background: "#3b82f6", animation: "axonmind-indeterminate 1.1s ease-in-out infinite" }} />
      </div>
    </>
  );
}
