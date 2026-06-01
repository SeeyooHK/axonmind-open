import React, { useEffect, useRef, useState } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

interface Props {
  onPaths: (paths: string[]) => void;
}

export function DropZone({ onPaths }: Props) {
  const [hovering, setHovering] = useState(false);
  const onPathsRef = useRef(onPaths);
  useEffect(() => { onPathsRef.current = onPaths; }, [onPaths]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWebviewWindow().onDragDropEvent((evt) => {
      const { type } = evt.payload;
      if (type === "enter" || type === "over") {
        setHovering(true);
      } else if (type === "leave") {
        setHovering(false);
      } else if (type === "drop") {
        setHovering(false);
        const paths = (evt.payload as { type: "drop"; paths: string[] }).paths;
        if (paths.length > 0) onPathsRef.current(paths);
      }
    }).then(fn => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, []);

  return (
    <div
      style={{
        border: `2px dashed ${hovering ? "#3b82f6" : "#334155"}`,
        borderRadius: 12,
        padding: "3rem 2rem",
        textAlign: "center",
        background: hovering ? "rgba(59,130,246,0.08)" : "transparent",
        transition: "border-color 0.15s, background 0.15s",
        userSelect: "none",
      }}
    >
      <div style={{ fontSize: 40, marginBottom: 12 }}>🗂</div>
      <p style={{ color: hovering ? "#93c5fd" : "#64748b", margin: 0, fontSize: 15 }}>
        Drag files or folders here
      </p>
      <p style={{ color: "#475569", margin: "6px 0 0", fontSize: 12 }}>
        md · txt · csv · xlsx · docx · pdf · pptx
      </p>
    </div>
  );
}
