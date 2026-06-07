import { useState, useEffect } from "react";
import type { GraphDiff, GraphExportV1 } from "@axonmind/types";
import { useAxonMind } from "../context";

interface State {
  data: GraphDiff | null;
  loading: boolean;
  error: string | null;
}

export function useGraphDiff(
  before: GraphExportV1 | null,
  after: GraphExportV1 | null
): State {
  const { transport } = useAxonMind();
  const [state, setState] = useState<State>({ data: null, loading: false, error: null });

  useEffect(() => {
    if (!before || !after) {
      setState({ data: null, loading: false, error: null });
      return;
    }
    let cancelled = false;
    setState({ data: null, loading: true, error: null });
    transport.graphDiff(before, after)
      .then(data => { if (!cancelled) setState({ data, loading: false, error: null }); })
      .catch(e => { if (!cancelled) setState({ data: null, loading: false, error: String(e) }); });
    return () => { cancelled = true; };
  }, [before, after, transport]);

  return state;
}
