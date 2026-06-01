import React from "react";
import type { AxonGraphNode, AxonGraphEdge } from "@axonmind/react";
import { KIND_COLOR } from "./graph/style";

interface Props {
  node?: AxonGraphNode;
  edge?: AxonGraphEdge;
  onClose: () => void;
}

export function InspectorPanel({ node, edge, onClose }: Props) {
  if (!node && !edge) return null;

  return (
    <div style={{
      width: 280, flexShrink: 0,
      background: "#1e293b", borderLeft: "1px solid #334155",
      display: "flex", flexDirection: "column",
      overflowY: "auto",
    }}>
      {/* Header */}
      <div style={{ padding: "14px 16px 0", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        {node && (
          <span style={{
            padding: "2px 10px", borderRadius: 9999, fontSize: 11, fontWeight: 600,
            background: KIND_COLOR[node.kind] ?? "#6b7280", color: "#fff",
          }}>
            {node.kind}
          </span>
        )}
        {edge && (
          <span style={{
            padding: "2px 10px", borderRadius: 9999, fontSize: 11, fontWeight: 600,
            background: "#334155", color: "#94a3b8",
          }}>
            Edge · {edge.kind}
          </span>
        )}
        <button
          onClick={onClose}
          style={{ background: "none", border: "none", color: "#475569", cursor: "pointer", fontSize: 20, lineHeight: 1, padding: 0 }}
        >
          ×
        </button>
      </div>

      <div style={{ padding: "12px 16px 20px", display: "flex", flexDirection: "column", gap: 12 }}>
        {node && (
          <>
            <div>
              <div style={{ color: "#f1f5f9", fontSize: 15, fontWeight: 600 }}>{node.label}</div>
              <div style={{ color: "#475569", fontSize: 11, marginTop: 2, fontFamily: "monospace" }}>{node.id}</div>
            </div>

            <Row label="Confidence" value={`${(node.confidence * 100).toFixed(0)}%`} />
            <Row label="Evidence" value={String(node.evidenceCount)} />

            {node.requiresHumanReview && (
              <div style={{ padding: "7px 10px", background: "#431407", borderRadius: 6, fontSize: 12, color: "#fb923c" }}>
                ⚠ Requires human review
              </div>
            )}
            {node.isTainted && (
              <div style={{ padding: "7px 10px", background: "#1c1917", borderRadius: 6, fontSize: 12, color: "#a8a29e" }}>
                Tainted — LLM-derived extraction (awaiting corroboration)
              </div>
            )}
          </>
        )}

        {edge && (
          <>
            <div>
              <div style={{ color: "#f1f5f9", fontSize: 15, fontWeight: 600 }}>{edge.kind}</div>
              <div style={{ color: "#475569", fontSize: 11, marginTop: 2, fontFamily: "monospace" }}>{edge.id}</div>
            </div>
            <Row label="Confidence" value={`${(edge.confidence * 100).toFixed(0)}%`} />
            <Row label="Evidence refs" value={String(edge.evidenceIds.length)} />
            {edge.confidence < 0.6 && (
              <div style={{ padding: "7px 10px", background: "#1c1917", borderRadius: 6, fontSize: 12, color: "#a8a29e" }}>
                Low confidence — dashed edge
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ display: "flex", justifyContent: "space-between", fontSize: 13 }}>
      <span style={{ color: "#64748b" }}>{label}</span>
      <span style={{ color: "#e2e8f0", fontWeight: 500 }}>{value}</span>
    </div>
  );
}
