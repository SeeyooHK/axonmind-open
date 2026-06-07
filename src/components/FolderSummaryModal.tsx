import React from "react";

interface RejectedEntry {
  displayPath: string;
  reason: string;
}

interface Props {
  acceptedCount: number;
  rejected: RejectedEntry[];
  onAddAttachment: () => void;
  onConfirm: () => void;
  onCancel: () => void;
}

export function FolderSummaryModal({ acceptedCount, rejected, onAddAttachment, onConfirm, onCancel }: Props) {
  return (
    <div style={{
      position: "fixed", inset: 0, zIndex: 100,
      background: "rgba(0,0,0,0.6)", display: "flex", alignItems: "center", justifyContent: "center",
    }}>
      <div style={{
        background: "#1e293b", borderRadius: 12, padding: "28px 28px 24px",
        width: 480, maxWidth: "90vw", maxHeight: "80vh", display: "flex", flexDirection: "column",
        border: "1px solid #334155",
      }}>
        <h2 style={{ fontSize: 16, fontWeight: 600, color: "#f1f5f9", margin: "0 0 16px" }}>
          Files found
        </h2>

        <div style={{ display: "flex", gap: 24, marginBottom: 16 }}>
          <div style={{ textAlign: "center" }}>
            <div style={{ fontSize: 28, fontWeight: 700, color: "#4ade80" }}>{acceptedCount}</div>
            <div style={{ fontSize: 12, color: "#64748b" }}>accepted</div>
          </div>
          {rejected.length > 0 && (
            <div style={{ textAlign: "center" }}>
              <div style={{ fontSize: 28, fontWeight: 700, color: "#f87171" }}>{rejected.length}</div>
              <div style={{ fontSize: 12, color: "#64748b" }}>skipped</div>
            </div>
          )}
        </div>

        {rejected.length > 0 && (
          <>
            <p style={{ fontSize: 12, color: "#64748b", margin: "0 0 8px" }}>Skipped files:</p>
            <div style={{ flex: 1, overflowY: "auto", marginBottom: 20 }}>
              {rejected.map((r, i) => (
                <div key={i} style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", padding: "4px 0", borderBottom: "1px solid #0f172a", gap: 12 }}>
                  <span style={{ fontSize: 12, fontFamily: "monospace", color: "#cbd5e1", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1 }}>
                    {r.displayPath}
                  </span>
                  <span style={{ fontSize: 11, color: "#64748b", flexShrink: 0 }}>{r.reason}</span>
                </div>
              ))}
            </div>
          </>
        )}

        <div style={{ display: "flex", gap: 10, justifyContent: "space-between", marginTop: rejected.length === 0 ? 16 : 0 }}>
          <button
            onClick={onAddAttachment}
            style={{ padding: "8px 14px", borderRadius: 8, border: "1px solid #334155", background: "transparent", color: "#cbd5e1", fontSize: 13, cursor: "pointer" }}
          >
            + Add Attachment
          </button>
          <div style={{ display: "flex", gap: 10 }}>
            <button
              onClick={onCancel}
              style={{ padding: "8px 18px", borderRadius: 8, border: "1px solid #334155", background: "transparent", color: "#94a3b8", fontSize: 13, cursor: "pointer" }}
            >
              Cancel
            </button>
            <button
              onClick={onConfirm}
              disabled={acceptedCount === 0}
              style={{
                padding: "8px 18px", borderRadius: 8, border: "none",
                background: acceptedCount > 0 ? "#254cff" : "#1e293b",
                color: acceptedCount > 0 ? "#fff" : "#475569",
                fontSize: 13, fontWeight: 600, cursor: acceptedCount > 0 ? "pointer" : "not-allowed",
              }}
            >
              Index {acceptedCount} {acceptedCount === 1 ? "file" : "files"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
