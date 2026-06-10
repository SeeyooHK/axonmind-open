import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAxonMind } from "@axonmind/react";
import type { Node } from "@axonmind/types";
import type { StagedItem } from "./GenerationStaging";

interface Props {
  item: StagedItem;
  onClose: () => void;
  zIndex?: number;
}

export function FileVisualizationModal({ item, onClose, zIndex = 200 }: Props) {
  const { transport } = useAxonMind();
  const [rawText, setRawText] = useState<string | null>(null);
  const [nodes, setNodes] = useState<Node[] | null>(null);
  const [rawErr, setRawErr] = useState<string | null>(null);

  useEffect(() => {
    setRawText(null);
    setRawErr(null);
    setNodes(null);

    // Load parsed file content
    invoke<string>("plugin:axonmind|read_file_text", { path: item.path, nodeId: item.id })
      .then(setRawText)
      .catch(e => setRawErr(String(e)));

    // Load extracted nodes via graph search by filename (without extension)
    const base = item.displayPath.split("/").pop() ?? "";
    const query = base.replace(/\.[^.]+$/, "").trim();
    if (query) {
      transport.graphSearch({ query, limit: 50 })
        .then(r => setNodes(r.nodes))
        .catch(() => setNodes([]));
    } else {
      setNodes([]);
    }
  }, [item.path, item.displayPath, transport]);

  return (
    <div
      style={{ position: "fixed", inset: 0, zIndex, background: "rgba(0,0,0,0.75)", display: "flex", alignItems: "stretch" }}
      onClick={e => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div style={{
        flex: 1, margin: 32, background: "#0f172a", borderRadius: 12,
        border: "1px solid #1e293b", display: "flex", flexDirection: "column", overflow: "hidden",
      }}>
        {/* Header */}
        <div style={{
          display: "flex", alignItems: "center", padding: "12px 20px",
          borderBottom: "1px solid #1e293b", gap: 12, flexShrink: 0,
        }}>
          <span style={{ fontFamily: "monospace", fontSize: 13, color: "#94a3b8", flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {item.displayPath}
          </span>
          <button
            onClick={onClose}
            style={{ background: "none", border: "none", color: "#64748b", cursor: "pointer", fontSize: 20, lineHeight: 1, padding: 0 }}
          >
            ×
          </button>
        </div>

        {/* Body: two columns */}
        <div style={{ flex: 1, display: "flex", overflow: "hidden", gap: 1 }}>
          {/* Original */}
          <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }}>
            <div style={{ padding: "10px 16px", borderBottom: "1px solid #1e293b", flexShrink: 0 }}>
              <span style={{ fontSize: 11, color: "#475569", fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.05em" }}>Original</span>
            </div>
            <pre style={{
              flex: 1, margin: 0, padding: 16, overflowY: "auto",
              fontSize: 12, lineHeight: 1.6, fontFamily: "monospace",
              color: "#cbd5e1", whiteSpace: "pre-wrap", wordBreak: "break-word",
            }}>
              {rawErr
                ? <span style={{ color: "#64748b", fontStyle: "italic" }}>{rawErr}</span>
                : rawText === null
                  ? <span style={{ color: "#475569" }}>Loading…</span>
                  : rawText}
            </pre>
          </div>

          <div style={{ width: 1, background: "#1e293b", flexShrink: 0 }} />

          {/* Extracted */}
          <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }}>
            <div style={{ padding: "10px 16px", borderBottom: "1px solid #1e293b", flexShrink: 0, display: "flex", alignItems: "center", gap: 8 }}>
              <span style={{ fontSize: 11, color: "#475569", fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.05em" }}>Extracted</span>
              {nodes && <span style={{ fontSize: 11, color: "#334155" }}>{nodes.length} nodes</span>}
            </div>
            <pre style={{
              flex: 1, margin: 0, padding: 16, overflowY: "auto",
              fontSize: 12, lineHeight: 1.6, fontFamily: "monospace",
              color: "#cbd5e1", whiteSpace: "pre-wrap", wordBreak: "break-word",
            }}>
              {nodes === null
                ? <span style={{ color: "#475569" }}>Loading…</span>
                : nodes.length === 0
                  ? <span style={{ color: "#475569", fontStyle: "italic" }}>No nodes extracted yet</span>
                  : JSON.stringify(nodes, null, 2)}
            </pre>
          </div>
        </div>
      </div>
    </div>
  );
}
