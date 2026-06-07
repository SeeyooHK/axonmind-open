// @axonmind/react — React hooks and provider for axonmind-open.
export { AxonMindProvider, useAxonMind } from "./context";
export type { AxonMindProviderProps } from "./context";

export { useFocusKpi } from "./hooks/useFocusKpi";
export { useGraphSearch } from "./hooks/useGraphSearch";
export { useEvidence } from "./hooks/useEvidence";
export { useImpactRadius } from "./hooks/useImpactRadius";
export { useEngineEvents } from "./hooks/useEngineEvents";
export { useGraphStats } from "./hooks/useGraphStats";
export { useGraphDiff } from "./hooks/useGraphDiff";

export { TauriTransport } from "./transport/tauri";

export { toGraphElements } from "./graph/adapter";
export type { AxonGraphNode, AxonGraphEdge, AxonGraphElements } from "./graph/adapter";
export { BrainMapView } from "./components/BrainMapView";
export type { Summary as BrainMapSummary, Category as BrainMapCategory } from "./components/BrainMapView";
export { InspectorPanel } from "./components/InspectorPanel";

// Re-export all types for convenience
export type * from "@axonmind/types";
