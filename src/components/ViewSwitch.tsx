import React from "react";

export type MapView = "generation" | "all";

interface Props {
  view: MapView;
  onChange: (v: MapView) => void;
  generationName: string;
}

export function ViewSwitch({ view, onChange, generationName }: Props) {
  const options: { value: MapView; label: string }[] = [
    { value: "generation", label: generationName },
    { value: "all",        label: "All Time"      },
  ];

  return (
    <div style={{ display: "flex", background: "#1e293b", borderRadius: 8, padding: 3, gap: 2 }}>
      {options.map(o => (
        <button
          key={o.value}
          onClick={() => onChange(o.value)}
          style={{
            padding: "5px 14px", borderRadius: 6, border: "none",
            background: view === o.value ? "#3b82f6" : "transparent",
            color: view === o.value ? "#fff" : "#64748b",
            fontSize: 13, fontWeight: 500,
            cursor: "pointer",
            transition: "background 0.12s, color 0.12s",
            maxWidth: 160, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
          }}
          title={o.label}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
