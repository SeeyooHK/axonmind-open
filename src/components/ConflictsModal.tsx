import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Evidence {
  id: string;
  quote: string | null;
  confidence: number;
  source_node_id: string;
}

interface EdgeWithEvidence {
  edge: {
    id: string;
    from: string;
    to: string;
    kind: string;
    confidence: number;
  };
  evidence: Evidence[];
}

interface ConflictPair {
  node_a: { id: string; name: string; kind: string };
  node_b: { id: string; name: string; kind: string };
  positive: EdgeWithEvidence[];
  negative: EdgeWithEvidence[];
  max_confidence: number;
}

interface FindConflictsOutput {
  conflicts: ConflictPair[];
}

interface Props {
  onClose: () => void;
}

export function ConflictsModal({ onClose }: Props) {
  const [data, setData] = useState<FindConflictsOutput | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);

  useEffect(() => {
    invoke<FindConflictsOutput>("plugin:axonmind|find_conflicts", { input: { node_id: null, limit: 50 } })
      .then(setData)
      .catch(e => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  function pairKey(p: ConflictPair) {
    return `${p.node_a.id}::${p.node_b.id}`;
  }

  return (
    <div
      style={{ position: "fixed", inset: 0, zIndex: 300, background: "rgba(0,0,0,0.7)", display: "flex", alignItems: "center", justifyContent: "center" }}
      onClick={e => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div style={{
        background: "#0f172a", border: "1px solid #1e293b", borderRadius: 12,
        width: 620, maxWidth: "95vw", maxHeight: "80vh", display: "flex", flexDirection: "column",
        overflow: "hidden",
      }}>
        {/* Header */}
        <div style={{ display: "flex", alignItems: "center", padding: "14px 20px", borderBottom: "1px solid #1e293b", flexShrink: 0 }}>
          <span style={{ fontWeight: 600, fontSize: 15, color: "#f1f5f9", flex: 1 }}>
            Conflicts
            {data && (
              <span style={{ fontWeight: 400, fontSize: 12, color: "#475569", marginLeft: 8 }}>
                {data.conflicts.length} pair{data.conflicts.length !== 1 ? "s" : ""} with contradictory claims
              </span>
            )}
          </span>
          <button onClick={onClose} style={{ background: "none", border: "none", color: "#64748b", cursor: "pointer", fontSize: 20, lineHeight: 1, padding: 0 }}>×</button>
        </div>

        {/* Body */}
        <div style={{ flex: 1, overflowY: "auto", padding: 16 }}>
          {loading && (
            <div style={{ color: "#475569", fontSize: 13, textAlign: "center", padding: 24 }}>Scanning graph…</div>
          )}
          {error && (
            <div style={{ color: "#f87171", fontSize: 13, padding: 12 }}>{error}</div>
          )}
          {data && data.conflicts.length === 0 && (
            <div style={{ color: "#475569", fontSize: 13, textAlign: "center", padding: 24 }}>
              No contradictions found in the graph.
            </div>
          )}
          {data && data.conflicts.map(pair => {
            const key = pairKey(pair);
            const isOpen = expanded === key;
            return (
              <div key={key} style={{ marginBottom: 8, borderRadius: 8, border: "1px solid #1e293b", overflow: "hidden" }}>
                {/* Pair header */}
                <button
                  onClick={() => setExpanded(isOpen ? null : key)}
                  style={{
                    width: "100%", display: "flex", alignItems: "center", gap: 10,
                    padding: "10px 14px", background: "#0f172a", border: "none", cursor: "pointer", textAlign: "left",
                  }}
                >
                  <span style={{ flex: 1, fontSize: 13, color: "#e2e8f0", fontWeight: 500 }}>
                    {pair.node_a.name}
                    <span style={{ color: "#475569", margin: "0 6px" }}>↔</span>
                    {pair.node_b.name}
                  </span>
                  <span style={{ fontSize: 11, color: "#4ade80", background: "rgba(74,222,128,0.08)", padding: "2px 7px", borderRadius: 4 }}>
                    {pair.positive.length} for
                  </span>
                  <span style={{ fontSize: 11, color: "#f87171", background: "rgba(248,113,113,0.08)", padding: "2px 7px", borderRadius: 4 }}>
                    {pair.negative.length} against
                  </span>
                  <span style={{ fontSize: 11, color: "#475569" }}>
                    {Math.round(pair.max_confidence * 100)}%
                  </span>
                  <span style={{ color: "#475569", fontSize: 12 }}>{isOpen ? "▲" : "▼"}</span>
                </button>

                {/* Expanded evidence */}
                {isOpen && (
                  <div style={{ padding: "0 14px 12px", background: "#080f1e" }}>
                    {pair.positive.length > 0 && (
                      <Section label="Supporting" color="#4ade80" edges={pair.positive} />
                    )}
                    {pair.negative.length > 0 && (
                      <Section label="Contradicting" color="#f87171" edges={pair.negative} />
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function Section({ label, color, edges }: { label: string; color: string; edges: EdgeWithEvidence[] }) {
  return (
    <div style={{ marginTop: 10 }}>
      <div style={{ fontSize: 11, color, fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.05em", marginBottom: 6 }}>
        {label}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        {edges.map(ewe => (
          <div key={ewe.edge.id} style={{ fontSize: 12, color: "#94a3b8", padding: "6px 10px", background: "#0f172a", borderRadius: 6 }}>
            <span style={{ color: "#64748b", marginRight: 6 }}>{ewe.edge.kind}</span>
            <span style={{ color: "#475569", fontSize: 11, marginRight: 8 }}>
              {Math.round(ewe.edge.confidence * 100)}%
            </span>
            {ewe.evidence.map(ev => ev.quote && (
              <div key={ev.id} style={{ marginTop: 4, fontStyle: "italic", color: "#64748b", lineHeight: 1.5 }}>
                "{ev.quote}"
              </div>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}
