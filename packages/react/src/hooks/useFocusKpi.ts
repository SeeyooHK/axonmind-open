import { useState, useEffect } from "react";
import type { FocusKpiOutput } from "@axonmind/types";
import { useAxonMind } from "../context";

interface State {
  data: FocusKpiOutput | null;
  loading: boolean;
  error: string | null;
}

export function useFocusKpi(kpiId: string | null): State {
  const { transport } = useAxonMind();
  const [state, setState] = useState<State>({ data: null, loading: false, error: null });

  useEffect(() => {
    if (!kpiId) {
      setState({ data: null, loading: false, error: null });
      return;
    }
    let cancelled = false;
    setState({ data: null, loading: true, error: null });
    transport.focusKpi({ kpi_id: kpiId })
      .then(data => { if (!cancelled) setState({ data, loading: false, error: null }); })
      .catch(e => { if (!cancelled) setState({ data: null, loading: false, error: String(e) }); });
    return () => { cancelled = true; };
  }, [kpiId, transport]);

  return state;
}
