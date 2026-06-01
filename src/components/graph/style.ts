import type cytoscape from "cytoscape";
import type { NodeKind } from "@axonmind/types";

const KIND_COLOR: Record<NodeKind, string> = {
  Kpi:         "#3b82f6",
  Metric:      "#8b5cf6",
  Objective:   "#06b6d4",
  Initiative:  "#0ea5e9",
  Risk:        "#ef4444",
  Opportunity: "#22c55e",
  Decision:    "#f59e0b",
  Insight:     "#a855f7",
  Document:    "#6b7280",
  Person:      "#84cc16",
  Team:        "#14b8a6",
  Customer:    "#f97316",
  Product:     "#ec4899",
  Market:      "#64748b",
  Process:     "#0891b2",
  System:      "#7c3aed",
  Action:      "#dc2626",
};

export { KIND_COLOR };

export const stylesheet: cytoscape.StylesheetCSS[] = [
  {
    selector: "node",
    css: {
      label: "data(label)",
      "background-color": (ele: cytoscape.NodeSingular) =>
        KIND_COLOR[ele.data("kind") as NodeKind] ?? "#6b7280",
      color: "#fff",
      "font-size": 11,
      "text-wrap": "wrap",
      "text-max-width": "80px",
      "text-valign": "center",
      "text-halign": "center",
      width: (ele: cytoscape.NodeSingular) => 30 + (ele.data("confidence") as number) * 30,
      height: (ele: cytoscape.NodeSingular) => 30 + (ele.data("confidence") as number) * 30,
      "border-width": (ele: cytoscape.NodeSingular) =>
        (ele.data("requiresHumanReview") as boolean) ? 3 : 0,
      "border-color": "#f97316",
      opacity: (ele: cytoscape.NodeSingular) =>
        (ele.data("isTainted") as boolean) ? 0.5 : 1,
    },
  },
  {
    selector: "node[kind = 'Document']",
    css: { shape: "rectangle" },
  },
  {
    selector: "node[kind = 'Person'], node[kind = 'Team']",
    css: { shape: "round-rectangle" },
  },
  {
    selector: "edge",
    css: {
      label: "data(kind)",
      "font-size": 9,
      color: "#94a3b8",
      "curve-style": "bezier",
      "target-arrow-shape": "triangle",
      "arrow-scale": 0.8,
      "line-color": "#334155",
      "target-arrow-color": "#334155",
      "line-style": (ele: cytoscape.EdgeSingular) =>
        (ele.data("confidence") as number) < 0.6 ? "dashed" : "solid",
      "text-background-color": "#0f172a",
      "text-background-opacity": 0.7,
      "text-background-padding": "2px",
    },
  },
  {
    selector: ":selected",
    css: {
      "border-width": 3,
      "border-color": "#60a5fa",
      "line-color": "#60a5fa",
      "target-arrow-color": "#60a5fa",
    },
  },
];
