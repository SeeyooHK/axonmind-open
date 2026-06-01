import { useState, useEffect } from "react";
import type { GetEvidenceOutput, NodeId, EdgeId } from "@axonmind/types";
import { useAxonMind } from "../context";

interface State {
  data: GetEvidenceOutput | null;
  loading: boolean;
  error: string | null;
}

export function useEvidence(target: { nodeId: NodeId } | { edgeId: EdgeId } | null): State {
  const { transport } = useAxonMind();
  const [state, setState] = useState<State>({ data: null, loading: false, error: null });

  useEffect(() => {
    if (!target) {
      setState({ data: null, loading: false, error: null });
      return;
    }
    let cancelled = false;
    setState({ data: null, loading: true, error: null });
    const input = "nodeId" in target
      ? { node_id: target.nodeId }
      : { edge_id: target.edgeId };
    transport.getEvidence(input)
      .then(data => { if (!cancelled) setState({ data, loading: false, error: null }); })
      .catch(e => { if (!cancelled) setState({ data: null, loading: false, error: String(e) }); });
    return () => { cancelled = true; };
  }, [
    "nodeId" in (target ?? {}) ? (target as { nodeId: NodeId }).nodeId : null,
    "edgeId" in (target ?? {}) ? (target as { edgeId: EdgeId }).edgeId : null,
    transport,
  ]);

  return state;
}
