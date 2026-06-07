import React, { useState } from "react";
import type { GraphDiff, NodeChange, EdgeChange } from "@axonmind/types";

interface GraphDiffModalProps {
  fileName: string;
  completedAt: string;
  beforeCount: { nodes: number; edges: number };
  afterCount: { nodes: number; edges: number };
  diff: GraphDiff;
  onClose: () => void;
}

export function GraphDiffModal({
  fileName,
  completedAt,
  beforeCount,
  afterCount,
  diff,
  onClose,
}: GraphDiffModalProps) {
  const [tab, setTab] = useState<"overview" | "nodes" | "edges" | "warnings">("overview");

  const s = diff.summary;
  const nodesTouched = s.nodes_added + s.nodes_removed + s.nodes_modified;
  const edgesTouched = s.edges_added + s.edges_removed + s.edges_modified;
  const hasChanges = nodesTouched > 0 || edgesTouched > 0 || diff.warnings.length > 0;

  function renderOverview() {
    if (!hasChanges) {
      return (
        <div style={{ padding: 20, color: "#94a3b8", fontSize: 13, textAlign: "center" }}>
          No graph changes after regeneration.
        </div>
      );
    }
    return (
      <div style={{ padding: 16, color: "#cbd5e1", fontSize: 13, display: "flex", flexDirection: "column", gap: 12 }}>
        <div>
          <span style={{ color: "#94a3b8" }}>nodes: </span>
          {afterCount.nodes} total
          <span style={{ color: "#10b981", marginLeft: 8 }}>+{s.nodes_added} added</span>
          <span style={{ color: "#eab308", marginLeft: 8 }}>~{s.nodes_modified} modified</span>
          <span style={{ color: "#ef4444", marginLeft: 8 }}>-{s.nodes_removed} removed</span>
        </div>
        <div>
          <span style={{ color: "#94a3b8" }}>edges: </span>
          {afterCount.edges} total
          <span style={{ color: "#10b981", marginLeft: 8 }}>+{s.edges_added} added</span>
          <span style={{ color: "#eab308", marginLeft: 8 }}>~{s.edges_modified} modified</span>
          <span style={{ color: "#ef4444", marginLeft: 8 }}>-{s.edges_removed} removed</span>
        </div>
      </div>
    );
  }

  function renderNodes() {
    return (
      <div style={{ padding: 16, overflowY: "auto" }}>
        <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 12 }}>
          <thead>
            <tr style={{ color: "#475569", textAlign: "left" }}>
              <th style={{ padding: "4px 8px" }}>Status</th>
              <th style={{ padding: "4px 8px" }}>Logical Key / Name</th>
              <th style={{ padding: "4px 8px" }}>Kind</th>
              <th style={{ padding: "4px 8px" }}>Changed Fields</th>
            </tr>
          </thead>
          <tbody>
            {diff.nodes.added.map((n, i) => (
              <tr key={`add-${i}`} style={{ borderTop: "1px solid #1e293b" }}>
                <td style={{ padding: "6px 8px", color: "#10b981" }}>Added</td>
                <td style={{ padding: "6px 8px", color: "#e2e8f0" }}>{n.after?.name || n.logical_key}</td>
                <td style={{ padding: "6px 8px", color: "#94a3b8" }}>{n.after?.kind}</td>
                <td style={{ padding: "6px 8px", color: "#64748b" }}>—</td>
              </tr>
            ))}
            {diff.nodes.removed.map((n, i) => (
              <tr key={`rem-${i}`} style={{ borderTop: "1px solid #1e293b" }}>
                <td style={{ padding: "6px 8px", color: "#ef4444" }}>Removed</td>
                <td style={{ padding: "6px 8px", color: "#94a3b8", textDecoration: "line-through" }}>{n.before?.name || n.logical_key}</td>
                <td style={{ padding: "6px 8px", color: "#94a3b8" }}>{n.before?.kind}</td>
                <td style={{ padding: "6px 8px", color: "#64748b" }}>—</td>
              </tr>
            ))}
            {diff.nodes.modified.map((n, i) => (
              <tr key={`mod-${i}`} style={{ borderTop: "1px solid #1e293b" }}>
                <td style={{ padding: "6px 8px", color: "#eab308" }}>Modified</td>
                <td style={{ padding: "6px 8px", color: "#e2e8f0" }}>{n.after?.name || n.logical_key}</td>
                <td style={{ padding: "6px 8px", color: "#94a3b8" }}>{n.after?.kind}</td>
                <td style={{ padding: "6px 8px", color: "#cbd5e1" }}>{n.changed_fields.join(", ")}</td>
              </tr>
            ))}
            {s.nodes_added + s.nodes_removed + s.nodes_modified === 0 && (
              <tr>
                <td colSpan={4} style={{ padding: "16px", textAlign: "center", color: "#64748b" }}>No nodes changed</td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    );
  }

  function renderEdges() {
    return (
      <div style={{ padding: 16, overflowY: "auto" }}>
        <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 12 }}>
          <thead>
            <tr style={{ color: "#475569", textAlign: "left" }}>
              <th style={{ padding: "4px 8px" }}>Status</th>
              <th style={{ padding: "4px 8px" }}>Logical Key</th>
              <th style={{ padding: "4px 8px" }}>Kind</th>
              <th style={{ padding: "4px 8px" }}>Changed Fields</th>
            </tr>
          </thead>
          <tbody>
            {diff.edges.added.map((e, i) => (
              <tr key={`add-${i}`} style={{ borderTop: "1px solid #1e293b" }}>
                <td style={{ padding: "6px 8px", color: "#10b981" }}>Added</td>
                <td style={{ padding: "6px 8px", color: "#e2e8f0" }}>{e.logical_key}</td>
                <td style={{ padding: "6px 8px", color: "#94a3b8" }}>{e.after?.kind}</td>
                <td style={{ padding: "6px 8px", color: "#64748b" }}>—</td>
              </tr>
            ))}
            {diff.edges.removed.map((e, i) => (
              <tr key={`rem-${i}`} style={{ borderTop: "1px solid #1e293b" }}>
                <td style={{ padding: "6px 8px", color: "#ef4444" }}>Removed</td>
                <td style={{ padding: "6px 8px", color: "#94a3b8", textDecoration: "line-through" }}>{e.logical_key}</td>
                <td style={{ padding: "6px 8px", color: "#94a3b8" }}>{e.before?.kind}</td>
                <td style={{ padding: "6px 8px", color: "#64748b" }}>—</td>
              </tr>
            ))}
            {diff.edges.modified.map((e, i) => (
              <tr key={`mod-${i}`} style={{ borderTop: "1px solid #1e293b" }}>
                <td style={{ padding: "6px 8px", color: "#eab308" }}>Modified</td>
                <td style={{ padding: "6px 8px", color: "#e2e8f0" }}>{e.logical_key}</td>
                <td style={{ padding: "6px 8px", color: "#94a3b8" }}>{e.after?.kind}</td>
                <td style={{ padding: "6px 8px", color: "#cbd5e1" }}>{e.changed_fields.join(", ")}</td>
              </tr>
            ))}
            {s.edges_added + s.edges_removed + s.edges_modified === 0 && (
              <tr>
                <td colSpan={4} style={{ padding: "16px", textAlign: "center", color: "#64748b" }}>No edges changed</td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    );
  }

  function renderWarnings() {
    return (
      <div style={{ padding: 16, overflowY: "auto", color: "#f87171", fontSize: 13, lineHeight: 1.5 }}>
        {diff.warnings.length === 0 ? (
          <div style={{ textAlign: "center", color: "#64748b" }}>No warnings</div>
        ) : (
          <ul style={{ margin: 0, paddingLeft: 20 }}>
            {diff.warnings.map((w, i) => (
              <li key={i}>{w}</li>
            ))}
          </ul>
        )}
      </div>
    );
  }

  function handleCopySummary() {
    const summaryText =
      `Graph Diff for ${fileName}\n` +
      `nodes: +${s.nodes_added} ~${s.nodes_modified} -${s.nodes_removed} (total: ${afterCount.nodes})\n` +
      `edges: +${s.edges_added} ~${s.edges_modified} -${s.edges_removed} (total: ${afterCount.edges})\n` +
      `warnings: ${diff.warnings.length}`;
    navigator.clipboard.writeText(summaryText).catch(() => {});
  }

  function handleExportJson() {
    const blob = new Blob([JSON.stringify(diff, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.style.display = "none";
    a.href = url;
    a.download = `graph_diff_${fileName.replace(/[^a-z0-9]/gi, '_').toLowerCase()}.json`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  return (
    <div
      style={{ position: "fixed", inset: 0, zIndex: 400, background: "rgba(0,0,0,0.6)", display: "flex", alignItems: "center", justifyContent: "center" }}
      onClick={(e) => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div style={{ background: "#0f172a", border: "1px solid #334155", borderRadius: 10, width: 720, maxWidth: "94vw", maxHeight: "85vh", display: "flex", flexDirection: "column" }}>
        {/* Header */}
        <div style={{ padding: "12px 16px", borderBottom: "1px solid #1e293b", display: "flex", alignItems: "center", gap: 12 }}>
          <span style={{ color: "#f1f5f9", fontWeight: 600, fontSize: 15 }}>Graph Diff</span>
          <span style={{ color: "#cbd5e1", fontSize: 13, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{fileName}</span>
          <div style={{ flex: 1 }} />
          <span style={{ color: "#64748b", fontSize: 11 }}>{new Date(completedAt).toLocaleString()}</span>
        </div>

        {/* Tabs */}
        {hasChanges && (
          <div style={{ display: "flex", borderBottom: "1px solid #1e293b", padding: "0 16px" }}>
            {(["overview", "nodes", "edges", "warnings"] as const).map(t => (
              <button
                key={t}
                onClick={() => setTab(t)}
                style={{
                  padding: "10px 16px", border: "none", background: "transparent",
                  color: tab === t ? "#60a5fa" : "#94a3b8",
                  borderBottom: tab === t ? "2px solid #60a5fa" : "2px solid transparent",
                  fontSize: 13, fontWeight: 500, cursor: "pointer", textTransform: "capitalize"
                }}
              >
                {t}
                {t === "warnings" && diff.warnings.length > 0 && ` (${diff.warnings.length})`}
              </button>
            ))}
          </div>
        )}

        {/* Content */}
        <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
          {tab === "overview" && renderOverview()}
          {tab === "nodes" && hasChanges && renderNodes()}
          {tab === "edges" && hasChanges && renderEdges()}
          {tab === "warnings" && hasChanges && renderWarnings()}
        </div>

        {/* Footer */}
        <div style={{ padding: "12px 16px", borderTop: "1px solid #1e293b", display: "flex", justifyContent: "flex-end", gap: 10 }}>
          {hasChanges && (
            <>
              <button onClick={handleCopySummary} style={actionBtnStyle}>Copy summary</button>
              <button onClick={handleExportJson} style={actionBtnStyle}>Export JSON</button>
            </>
          )}
          <button onClick={onClose} style={{ ...actionBtnStyle, background: "#1e293b", color: "#f1f5f9" }}>Close</button>
        </div>
      </div>
    </div>
  );
}

const actionBtnStyle: React.CSSProperties = {
  padding: "6px 12px",
  borderRadius: 6,
  border: "1px solid #334155",
  background: "transparent",
  color: "#94a3b8",
  fontSize: 12,
  cursor: "pointer",
};
