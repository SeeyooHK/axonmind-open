import { useState, useEffect } from "react";
import type { ImpactRadiusOutput, NodeId } from "@axonmind/types";
import { useAxonMind } from "../context";

interface State {
  data: ImpactRadiusOutput | null;
  loading: boolean;
  error: string | null;
}

export function useImpactRadius(nodeId: NodeId | null, maxDepth?: number): State {
  const { transport } = useAxonMind();
  const [state, setState] = useState<State>({ data: null, loading: false, error: null });

  useEffect(() => {
    if (!nodeId) {
      setState({ data: null, loading: false, error: null });
      return;
    }
    let cancelled = false;
    setState({ data: null, loading: true, error: null });
    transport.impactRadius({ node_id: nodeId, max_depth: maxDepth })
      .then(data => { if (!cancelled) setState({ data, loading: false, error: null }); })
      .catch(e => { if (!cancelled) setState({ data: null, loading: false, error: String(e) }); });
    return () => { cancelled = true; };
  }, [nodeId, maxDepth, transport]);

  return state;
}
