import { useState, useEffect } from "react";
import type { GraphStatsOutput } from "@axonmind/types";
import { useAxonMind } from "../context";

interface State {
  data: GraphStatsOutput | null;
  loading: boolean;
  error: string | null;
}

export function useGraphStats(): State {
  const { transport } = useAxonMind();
  const [state, setState] = useState<State>({ data: null, loading: false, error: null });

  useEffect(() => {
    let cancelled = false;
    setState({ data: null, loading: true, error: null });
    transport.graphStats()
      .then(data => { if (!cancelled) setState({ data, loading: false, error: null }); })
      .catch(e => { if (!cancelled) setState({ data: null, loading: false, error: String(e) }); });
    return () => { cancelled = true; };
  }, [transport]);

  return state;
}
