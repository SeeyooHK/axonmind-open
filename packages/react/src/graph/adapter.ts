import type { GraphExportV1, NodeKind, EdgeKind } from "@axonmind/types";

export interface AxonGraphNode {
  id: string;
  label: string;
  kind: NodeKind;
  confidence: number;
  isTainted: boolean;
  requiresHumanReview: boolean;
  evidenceCount: number;
}

export interface AxonGraphEdge {
  id: string;
  source: string;
  target: string;
  kind: EdgeKind;
  confidence: number;
  evidenceIds: string[];
}

export interface AxonGraphElements {
  nodes: AxonGraphNode[];
  edges: AxonGraphEdge[];
}

export interface ToGraphElementsOptions {
  /**
   * Edge kinds to omit from the rendered graph. Defaults to ["MentionedIn"], which is
   * document→concept provenance — it's what document each concept came from, not a business
   * relationship between concepts. It dominates the edge count and turns the map into a hairball
   * of spokes radiating from document hubs. The provenance is still available per node via
   * evidence (evidenceCount) and the underlying data, so hiding it here is view-only.
   * Pass [] to render every edge kind.
   */
  hideEdgeKinds?: EdgeKind[];
}

export function toGraphElements(
  g: GraphExportV1,
  options: ToGraphElementsOptions = {},
): AxonGraphElements {
  const hidden = new Set<EdgeKind>(options.hideEdgeKinds ?? ["MentionedIn"]);
  const evidenceById = new Map(g.evidence.map((ev) => [ev.id, ev]));
  const evidenceIdsByNode = new Map<string, Set<string>>();
  const edgeReviewByNode = new Map<string, boolean>();

  function addEvidence(nodeId: string, evidenceId: string) {
    const set = evidenceIdsByNode.get(nodeId) ?? new Set<string>();
    set.add(evidenceId);
    evidenceIdsByNode.set(nodeId, set);
  }
  function setEdgeReview(nodeId: string, requiresReview: boolean) {
    edgeReviewByNode.set(
      nodeId,
      (edgeReviewByNode.get(nodeId) ?? false) || requiresReview,
    );
  }

  for (const ev of g.evidence) {
    addEvidence(ev.source_node_id, ev.id);
  }
  for (const edge of g.edges) {
    for (const evidenceId of edge.evidence) {
      addEvidence(edge.from, evidenceId);
      addEvidence(edge.to, evidenceId);
    }
    setEdgeReview(edge.from, edge.requires_human_review);
    setEdgeReview(edge.to, edge.requires_human_review);
  }

  return {
    nodes: g.nodes.map(n => ({
      // Display confidence as evidence-backed noisy-OR when evidence is present, otherwise
      // preserve the stored node confidence.
      confidence: (() => {
        const evIds = evidenceIdsByNode.get(n.id);
        if (!evIds || evIds.size === 0) return n.confidence;
        const confs: number[] = [];
        for (const evId of evIds) {
          const ev = evidenceById.get(evId);
          if (ev) confs.push(ev.confidence);
        }
        if (confs.length === 0) return n.confidence;
        const product = confs.reduce((acc, c) => acc * (1 - c), 1);
        const combined = 1 - product;
        return Math.max(0, Math.min(1, combined));
      })(),
      id: n.id,
      label: n.name,
      kind: n.kind,
      isTainted: n.is_tainted,
      requiresHumanReview: (() => {
        const evIds = evidenceIdsByNode.get(n.id);
        let evidenceRequiresReview = false;
        if (evIds) {
          for (const evId of evIds) {
            const ev = evidenceById.get(evId);
            if (ev?.requires_human_review) {
              evidenceRequiresReview = true;
              break;
            }
          }
        }
        const reviewFromEdges = edgeReviewByNode.get(n.id) ?? false;
        const reviewFromNode =
          n.requires_human_review && (n.kind === "Kpi" || n.confidence < 0.5);
        return reviewFromNode || reviewFromEdges || evidenceRequiresReview;
      })(),
      evidenceCount: evidenceIdsByNode.get(n.id)?.size ?? 0,
    })),
    edges: g.edges
      .filter(e => !hidden.has(e.kind))
      .map(e => ({
        id: e.id,
        source: e.from,
        target: e.to,
        kind: e.kind,
        confidence: e.confidence,
        evidenceIds: e.evidence,
      })),
  };
}
