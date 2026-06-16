import React from "react";
import type { IngestSummary } from "@axonmind/types";

export type ItemStatus = "queued" | "ingesting" | "ready" | "failed";

export interface StagedItem {
  id: string;
  path: string;
  displayPath: string;
  status: ItemStatus;
  summary?: IngestSummary;
  error?: string;
}

interface Props {
  items: StagedItem[];
  generationName: string;
  onNameChange: (name: string) => void;
  onGenerate: () => void;
  onRemove: (id: string) => void;
  onVisualize: (item: StagedItem) => void;
  hasActiveKey?: boolean;
}

const STATUS_ICON: Record<ItemStatus, string> = {
  queued: "○",
  ingesting: "◌",
  ready: "✓",
  failed: "✗",
};

const STATUS_COLOR: Record<ItemStatus, string> = {
  queued: "#475569",
  ingesting: "#3b82f6",
  ready: "#4ade80",
  failed: "#f87171",
};

function summaryLabel(s: IngestSummary): { text: string; color: string } {
  if (s.files_skipped > 0 && s.files_processed === 0) return { text: "unchanged", color: "#475569" };
  if (s.nodes_created === 0 && s.edges_created === 0) return { text: "0 nodes extracted", color: "#f59e0b" };
  const parts = [`${s.nodes_created}n`, `${s.edges_created}e`];
  if (s.evidence_created > 0) parts.push(`${s.evidence_created}ev`);
  return { text: parts.join(" · "), color: "#475569" };
}

export function GenerationStaging({ items, generationName, onNameChange, onGenerate, onRemove, onVisualize, hasActiveKey }: Props) {
  const allSettled = items.length > 0 && items.every(i => i.status === "ready" || i.status === "failed");
  const anyReady = items.some(i => i.status === "ready");
  const canGenerate = allSettled && anyReady && hasActiveKey !== false;

  const doneCount = items.filter(i => i.status === "ready" || i.status === "failed").length;
  const pct = items.length > 0 ? Math.round((doneCount / items.length) * 100) : 0;
  const isProcessing = items.some(i => i.status === "ingesting" || i.status === "queued");

  return (
    <>
      <div>
        {/* Generation name */}
        <div style={{ marginBottom: "1rem" }}>
          <label style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 4 }}>
            Generation name
          </label>
          <input
            value={generationName}
            onChange={e => onNameChange(e.target.value)}
            style={{
              width: "100%", padding: "8px 12px", borderRadius: 8,
              background: "#1e293b", border: "1px solid #334155",
              color: "#f1f5f9", fontSize: 14, boxSizing: "border-box",
            }}
          />
        </div>

        {/* Overall progress bar */}
        {isProcessing && (
          <div style={{ marginBottom: "1rem" }}>
            <div style={{ display: "flex", justifyContent: "space-between", fontSize: 11, color: "#64748b", marginBottom: 5 }}>
              <span>Indexing files…</span>
              <span>{doneCount} / {items.length} &nbsp;{pct}%</span>
            </div>
            <div style={{ height: 6, background: "#1e293b", borderRadius: 3, overflow: "hidden" }}>
              <div style={{
                height: "100%", background: "#3b82f6", borderRadius: 3,
                width: `${pct}%`, transition: "width 0.3s ease",
              }} />
            </div>
          </div>
        )}

        {/* Item list */}
        <div style={{ marginBottom: "1.5rem", display: "flex", flexDirection: "column", gap: 4, maxHeight: 320, overflowY: "auto" }}>
          {items.map(item => (
            <div key={item.id} style={{ borderRadius: 6, background: "#1e293b", overflow: "hidden" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "8px 12px" }}>

                {/* Status indicator */}
                <span style={{ fontSize: 13, flexShrink: 0, color: STATUS_COLOR[item.status], fontFamily: "monospace", width: 14, textAlign: "center" }}>
                  {STATUS_ICON[item.status]}
                </span>

                {/* Path */}
                <span style={{ flex: 1, fontSize: 12, color: "#cbd5e1", fontFamily: "monospace", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                  {item.displayPath}
                </span>

                {/* Status detail */}
                {item.status === "ingesting" && (
                  <span style={{ fontSize: 11, color: "#3b82f6", flexShrink: 0 }}>axoning…stay tuned...</span>
                )}
                {item.status === "ready" && item.summary && (() => {
                  const { text, color } = summaryLabel(item.summary);
                  return <span style={{ fontSize: 11, color, flexShrink: 0 }}>{text}</span>;
                })()}
                {item.status === "ready" && item.summary?.errors && item.summary.errors.length > 0 && (
                  <span title={item.summary.errors.join("\n")} style={{ fontSize: 11, color: "#f59e0b", flexShrink: 0, cursor: "default" }}>
                    {item.summary.errors.length} err
                  </span>
                )}
                {item.status === "failed" && item.error && (
                  <span
                    title={item.error}
                    style={{ fontSize: 11, color: "#f87171", flexShrink: 0, maxWidth: 180, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", cursor: "default" }}
                  >
                    {item.error}
                  </span>
                )}

                {/* Actions */}
                {item.status === "ready" && (
                  <button
                    onClick={() => onVisualize(item)}
                    title="View original and extracted data"
                    style={{ background: "none", border: "1px solid #334155", borderRadius: 4, color: "#94a3b8", cursor: "pointer", fontSize: 11, padding: "2px 7px", flexShrink: 0 }}
                  >
                    view
                  </button>
                )}
                <button
                  onClick={() => onRemove(item.id)}
                  title="Remove from this batch only"
                  style={{ background: "none", border: "none", color: "#334155", cursor: "pointer", fontSize: 16, padding: "0 2px", flexShrink: 0, lineHeight: 1 }}
                >
                  ×
                </button>
              </div>

            </div>
          ))}
        </div>

        <div style={{ marginTop: "-0.75rem", marginBottom: "1rem", fontSize: 11, color: "#64748b" }}>
          Removing an item here only clears it from this batch. To delete indexed concepts and evidence, use Remove in Processed Files.
        </div>

        {/* Generate button */}
        <button
          onClick={canGenerate ? onGenerate : undefined}
          disabled={!canGenerate}
          style={{
            width: "100%", padding: "13px", borderRadius: 10, border: "none",
            background: canGenerate ? "#3b82f6" : "#1e293b",
            color: canGenerate ? "#fff" : "#475569",
            fontSize: 15, fontWeight: 600,
            cursor: canGenerate ? "pointer" : "not-allowed",
            transition: "background 0.15s, color 0.15s",
          }}
        >
          {!allSettled
            ? `Processing… (${items.filter(i => i.status === "ready" || i.status === "failed").length}/${items.length})`
            : hasActiveKey === false
              ? "⚙ Set an API key to Generate"
              : "Generate Brain Map"}
        </button>
      </div>
    </>
  );
}
