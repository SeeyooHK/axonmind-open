import React from "react";
import type { NodeKind } from "@axonmind/types";
import type { AxonGraphNode, AxonGraphEdge } from "../graph/adapter";

const KIND_COLOR: Record<NodeKind, string> = {
  Kpi: "#3b82f6",
  Metric: "#8b5cf6",
  Objective: "#06b6d4",
  Initiative: "#0ea5e9",
  Risk: "#ef4444",
  Opportunity: "#22c55e",
  Decision: "#f59e0b",
  Insight: "#a855f7",
  Document: "#6b7280",
  Person: "#84cc16",
  Team: "#14b8a6",
  Customer: "#f97316",
  Product: "#ec4899",
  Market: "#64748b",
  Process: "#0891b2",
  System: "#7c3aed",
  Action: "#dc2626",
};

const THEME = {
  panelBg: "var(--axonmind-panel-bg, #1e293b)",
  panelBorder: "var(--axonmind-panel-border, #334155)",
  text: "var(--axonmind-text, #e2e8f0)",
  textStrong: "var(--axonmind-text-strong, #f1f5f9)",
  textMuted: "var(--axonmind-text-muted, #94a3b8)",
  textSubtle: "var(--axonmind-text-subtle, #64748b)",
  close: "var(--axonmind-close, #475569)",
  edgeBadgeBg: "var(--axonmind-edge-badge-bg, #334155)",
  edgeBadgeText: "var(--axonmind-edge-badge-text, #94a3b8)",
  reviewBg: "var(--axonmind-review-bg, #431407)",
  reviewText: "var(--axonmind-review-text, #fb923c)",
  taintedBg: "var(--axonmind-tainted-bg, #1c1917)",
  taintedText: "var(--axonmind-tainted-text, #a8a29e)",
};

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
      background: THEME.panelBg, borderLeft: `1px solid ${THEME.panelBorder}`,
      display: "flex", flexDirection: "column",
      overflowY: "auto",
    }}>
      {/* Header */}
      <div style={{ padding: "14px 16px 0", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        {node && (
          <span style={{
            padding: "2px 10px", borderRadius: 9999, fontSize: 11, fontWeight: 600,
            background: KIND_COLOR[node.kind] ?? "var(--axonmind-kind-fallback, #6b7280)", color: "var(--axonmind-kind-text, #fff)",
          }}>
            {node.kind}
          </span>
        )}
        {edge && (
          <span style={{
            padding: "2px 10px", borderRadius: 9999, fontSize: 11, fontWeight: 600,
            background: THEME.edgeBadgeBg, color: THEME.edgeBadgeText,
          }}>
            Edge · {edge.kind}
          </span>
        )}
        <button
          onClick={onClose}
          style={{ background: "none", border: "none", color: THEME.close, cursor: "pointer", fontSize: 20, lineHeight: 1, padding: 0 }}
        >
          ×
        </button>
      </div>

      <div style={{ padding: "12px 16px 20px", display: "flex", flexDirection: "column", gap: 12 }}>
        {node && (
          <>
            <div>
              <div style={{ color: THEME.textStrong, fontSize: 15, fontWeight: 600 }}>{node.label}</div>
              <div style={{ color: THEME.close, fontSize: 11, marginTop: 2, fontFamily: "monospace" }}>{node.id}</div>
            </div>

            <Row label="Confidence" value={`${(node.confidence * 100).toFixed(0)}%`} />
            <Row label="Evidence" value={String(node.evidenceCount)} />

            {node.requiresHumanReview && (
              <div style={{ padding: "7px 10px", background: THEME.reviewBg, borderRadius: 6, fontSize: 12, color: THEME.reviewText }}>
                ⚠ Requires human review
              </div>
            )}
            {node.isTainted && (
              <div style={{ padding: "7px 10px", background: THEME.taintedBg, borderRadius: 6, fontSize: 12, color: THEME.taintedText }}>
                Tainted — LLM-derived extraction (awaiting corroboration)
              </div>
            )}
          </>
        )}

        {edge && (
          <>
            <div>
              <div style={{ color: THEME.textStrong, fontSize: 15, fontWeight: 600 }}>{edge.kind}</div>
              <div style={{ color: THEME.close, fontSize: 11, marginTop: 2, fontFamily: "monospace" }}>{edge.id}</div>
            </div>
            <Row label="Confidence" value={`${(edge.confidence * 100).toFixed(0)}%`} />
            <Row label="Evidence refs" value={String(edge.evidenceIds.length)} />
            {edge.confidence < 0.6 && (
              <div style={{ padding: "7px 10px", background: THEME.taintedBg, borderRadius: 6, fontSize: 12, color: THEME.taintedText }}>
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
      <span style={{ color: THEME.textSubtle }}>{label}</span>
      <span style={{ color: THEME.text, fontWeight: 500 }}>{value}</span>
    </div>
  );
}
