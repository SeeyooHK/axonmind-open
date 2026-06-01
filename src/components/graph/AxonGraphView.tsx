import React, { useRef, useEffect } from "react";
import cytoscape, { type Core } from "cytoscape";
import fcose from "cytoscape-fcose";
import type { AxonGraphElements, AxonGraphNode, AxonGraphEdge } from "@axonmind/react";
import { stylesheet } from "./style";

cytoscape.use(fcose);

interface Props {
  elements: AxonGraphElements;
  onSelectNode?: (node: AxonGraphNode) => void;
  onSelectEdge?: (edge: AxonGraphEdge) => void;
  style?: React.CSSProperties;
  onGoHome?: () => void;
}

export function AxonGraphView({ elements, onSelectNode, onSelectEdge, style, onGoHome }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<Core | null>(null);

  // callbacks in refs so the tap listeners don't go stale
  const onSelectNodeRef = useRef(onSelectNode);
  const onSelectEdgeRef = useRef(onSelectEdge);
  useEffect(() => { onSelectNodeRef.current = onSelectNode; }, [onSelectNode]);
  useEffect(() => { onSelectEdgeRef.current = onSelectEdge; }, [onSelectEdge]);

  // Initialize Cytoscape once
  useEffect(() => {
    if (!containerRef.current) return;
    const cy = cytoscape({
      container: containerRef.current,
      style: stylesheet,
      elements: [],
      minZoom: 0.1,
      maxZoom: 5,
    });
    cyRef.current = cy;

    cy.on("tap", "node", (evt) => {
      const n = evt.target;
      const connectedEvidence = new Set<string>();
      n.connectedEdges().forEach((edge) => {
        const ids = edge.data("evidenceIds");
        if (Array.isArray(ids)) {
          for (const id of ids) connectedEvidence.add(String(id));
        }
      });
      const directEvidence = Number(n.data("evidenceCount") ?? 0);
      onSelectNodeRef.current?.({
        id: n.id(),
        label: n.data("label"),
        kind: n.data("kind"),
        confidence: n.data("confidence"),
        isTainted: n.data("isTainted"),
        requiresHumanReview: n.data("requiresHumanReview"),
        evidenceCount: Math.max(directEvidence, connectedEvidence.size),
      });
    });

    cy.on("tap", "edge", (evt) => {
      const e = evt.target;
      onSelectEdgeRef.current?.({
        id: e.id(),
        source: e.source().id(),
        target: e.target().id(),
        kind: e.data("kind"),
        confidence: e.data("confidence"),
        evidenceIds: e.data("evidenceIds"),
      });
    });

    cy.on("tap", (evt) => {
      if (evt.target === cy) {
        onSelectNodeRef.current?.(undefined as unknown as AxonGraphNode);
        onSelectEdgeRef.current?.(undefined as unknown as AxonGraphEdge);
      }
    });

    return () => { cy.destroy(); cyRef.current = null; };
  }, []);

  // Patch elements incrementally; relayout when the set changes
  useEffect(() => {
    const cy = cyRef.current;
    if (!cy) return;

    const incomingNodeIds = new Set(elements.nodes.map(n => n.id));
    const incomingEdgeIds = new Set(elements.edges.map(e => e.id));

    cy.batch(() => {
      // Remove stale
      cy.nodes().forEach(n => { if (!incomingNodeIds.has(n.id())) n.remove(); });
      cy.edges().forEach(e => { if (!incomingEdgeIds.has(e.id())) e.remove(); });

      // Upsert nodes
      for (const n of elements.nodes) {
        const existing = cy.getElementById(n.id);
        if (existing.length) {
          existing.data(n);
        } else {
          cy.add({ group: "nodes", data: n });
        }
      }

      // Upsert edges
      for (const e of elements.edges) {
        const existing = cy.getElementById(e.id);
        if (existing.length) {
          existing.data(e);
        } else {
          cy.add({ group: "edges", data: { ...e, source: e.source, target: e.target } });
        }
      }
    });

    if (elements.nodes.length > 0) {
      cy.layout({ name: "fcose", animate: true, animationDuration: 400 } as never).run();
    }
  }, [elements]);

  return (
    <div style={{ position: "relative", width: "100%", height: "100%", ...style }}>
      <div
        ref={containerRef}
        style={{ width: "100%", height: "100%", background: "#0f172a" }}
      />
      {onGoHome && (
        <button
          onClick={onGoHome}
          style={{
            position: "absolute",
            top: "16px",
            left: "16px",
            zIndex: 10,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            width: "40px",
            height: "40px",
            borderRadius: "50%",
            background: "#1e293b",
            border: "1px solid #334155",
            color: "#94a3b8",
            cursor: "pointer",
            boxShadow: "0 4px 12px rgba(0, 0, 0, 0.3)",
            transition: "all 0.2s ease-in-out",
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.background = "#254cff";
            e.currentTarget.style.color = "#ffffff";
            e.currentTarget.style.transform = "scale(1.08)";
            e.currentTarget.style.boxShadow = "0 0 15px rgba(37, 76, 255, 0.5)";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = "#1e293b";
            e.currentTarget.style.color = "#94a3b8";
            e.currentTarget.style.transform = "scale(1)";
            e.currentTarget.style.boxShadow = "0 4px 12px rgba(0, 0, 0, 0.3)";
          }}
          title="Go to Home Page"
        >
          {/* Home Icon */}
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
            <polyline points="9 22 9 12 15 12 15 22" />
          </svg>
        </button>
      )}
    </div>
  );
}
