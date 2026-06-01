import { useState, useEffect } from "react";
import type { GraphSearchOutput, NodeKind } from "@axonmind/types";
import { useAxonMind } from "../context";

interface Options { kinds?: NodeKind[]; limit?: number; }
interface State {
  data: GraphSearchOutput | null;
  loading: boolean;
  error: string | null;
}

export function useGraphSearch(query: string, options?: Options): State {
  const { transport } = useAxonMind();
  const [state, setState] = useState<State>({ data: null, loading: false, error: null });

  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setState({ data: null, loading: false, error: null });
      return;
    }
    let cancelled = false;
    setState({ data: null, loading: true, error: null });
    transport.graphSearch({ query: q, kinds: options?.kinds, limit: options?.limit })
      .then(data => { if (!cancelled) setState({ data, loading: false, error: null }); })
      .catch(e => { if (!cancelled) setState({ data: null, loading: false, error: String(e) }); });
    return () => { cancelled = true; };
  }, [query, options?.kinds, options?.limit, transport]);

  return state;
}
