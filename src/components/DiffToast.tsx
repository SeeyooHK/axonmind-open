import React, { useEffect } from "react";

interface Props {
  message: string;
  onClose: () => void;
  /** Auto-dismiss after this many ms. Default 7000. */
  durationMs?: number;
}

// One-line "what changed" toast shown after a re-index batch. Self-dismissing.
export function DiffToast({ message, onClose, durationMs = 7000 }: Props) {
  useEffect(() => {
    const t = setTimeout(onClose, durationMs);
    return () => clearTimeout(t);
  }, [message, durationMs, onClose]);

  return (
    <div style={{
      position: "fixed", bottom: 20, left: "50%", transform: "translateX(-50%)",
      zIndex: 200, display: "flex", alignItems: "center", gap: 12,
      padding: "10px 14px", borderRadius: 10,
      background: "#1e293b", border: "1px solid #334155",
      boxShadow: "0 6px 24px rgba(0,0,0,0.4)",
      maxWidth: "90vw",
    }}>
      <span style={{ fontSize: 14 }}>🔄</span>
      <span style={{ fontSize: 13, color: "#e2e8f0", fontFamily: "monospace", whiteSpace: "nowrap" }}>
        {message}
      </span>
      <button
        onClick={onClose}
        title="Dismiss"
        style={{
          background: "none", border: "none", cursor: "pointer", padding: 2,
          color: "#64748b", display: "flex", alignItems: "center", lineHeight: 1,
        }}
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </button>
    </div>
  );
}
