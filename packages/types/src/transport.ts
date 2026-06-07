// AxonMindTransport: host-agnostic interface consumed by @axonmind/react.
// Tauri hosts implement this via invoke(); HTTP/WS hosts implement via fetch/WebSocket.
// No implementation may import from window.__TAURI__ directly inside @axonmind/react.
import type {
  Edge, EdgeId, EngineEvent, Evidence, EvidenceId, IngestSummary,
  Node, NodeId, NodeKind, KpiStatus, GraphExportV1,
} from "./index";

// ── Tool I/O types ────────────────────────────────────────────────────────────

export interface EdgeWithNodes { edge: import("./index").Edge; from: Node; to: Node; }

export interface FocusKpiInput  { kpi_id: NodeId; }
export interface FocusKpiOutput { kpi: Node; drivers: EdgeWithNodes[]; blockers: EdgeWithNodes[]; risks: EdgeWithNodes[]; owner: Node | null; evidence_count: number; }

export interface ExplainKpiInput  { kpi_id: NodeId; depth?: number; }
export interface ExplainKpiOutput { rationale: string; evidence: Evidence[]; confidence: number; }

export interface GetEvidenceInput  { edge_id?: EdgeId; node_id?: NodeId; }
export interface GetEvidenceOutput { evidence: Evidence[]; }

export interface ImpactRadiusInput  { node_id: NodeId; max_depth?: number; }
export interface ImpactRadiusOutput { affected: Array<{ node: Node; depth: number; path: NodeId[] }>; }

export interface TraceDecisionInput  { decision_node_id: NodeId; }
export interface TraceDecisionOutput { decision: Node; caused_by: EdgeWithNodes[]; evidenced_by: Evidence[]; next_actions: EdgeWithNodes[]; }

export interface SuggestActionsInput  { kpi_id: NodeId; status_filter?: KpiStatus[]; include_unreviewed?: boolean; }
export interface SuggestActionsOutput { actions: Node[]; }

// ── graph_stats ───────────────────────────────────────────────────────────────

export interface NodeKindCount { kind: string; count: number; }
export interface GraphStatsOutput {
  total_nodes: number;
  document_nodes: number;
  concept_nodes: number;
  total_edges: number;
  total_evidence: number;
  avg_confidence: number;
  tainted_nodes: number;
  tainted_edges: number;
  review_required_nodes: number;
  nodes_by_kind: NodeKindCount[];
}

// ── graph_diff ────────────────────────────────────────────────────────────────

export interface NodeChange {
  logical_key: string;
  before: Node | null;
  after: Node | null;
  changed_fields: string[];
}
export interface EdgeChange {
  logical_key: string;
  before: Edge | null;
  after: Edge | null;
  changed_fields: string[];
}
export interface DiffSection<T> { added: T[]; removed: T[]; modified: T[]; }
export interface DiffCounts {
  nodes_added: number; nodes_removed: number; nodes_modified: number;
  edges_added: number; edges_removed: number; edges_modified: number;
}
export interface GraphDiff {
  before_exported_at: string;
  after_exported_at: string;
  nodes: DiffSection<NodeChange>;
  edges: DiffSection<EdgeChange>;
  summary: DiffCounts;
  /** Non-empty when inputs were not cleanly diffable (logical-key collisions or edges
   *  whose endpoints are absent from the same export). Surfaced rather than silently dropped. */
  warnings: string[];
}

export type SearchMatchKind = "name" | "definition" | "evidence_quote";
export interface GraphSearchInput  { query: string; kinds?: NodeKind[]; limit?: number; }
export interface GraphSearchOutput { nodes: Node[]; matched_via: SearchMatchKind[][]; }

// ── PageIndex vectorless retrieval ────────────────────────────────────────────

export interface ReasoningSearchInput {
  query: string;
  /** Restrict to these document node ids. Omit or empty = whole corpus. */
  doc_node_ids?: string[];
  /** Maximum results. Default: 20. */
  max_results?: number;
}

export interface RetrievedSection {
  /** Bridges to the graph Document node. */
  doc_node_id: string;
  section_id: string;
  title: string;
  text: string;
  span_start: number;
  span_end: number;
  /** Breadcrumb from document root to this section. */
  path: string[];
}

export interface ReasoningSearchOutput {
  sections: RetrievedSection[];
  /** false = BM25-only (no LLM provider); true = LLM-reranked. */
  reasoning_applied: boolean;
}

export interface IndexPathOptions { recursive?: boolean; skipUnchanged?: boolean; }
export interface IndexMarkdownOptions { sourcePath?: string; sha256?: string; }

export type ScopedSummaryModeInput = "auto" | "cached_only" | "regenerate";

export interface SuggestedCategory {
  label: string;
  headline_node_id: string;
  member_node_ids: string[];
}

export interface SuggestedSummary {
  categories: SuggestedCategory[];
  source: string;
  labels: Record<string, string>;
}

export interface DocumentSummary {
  node_id: string;
  name: string;
  source_path: string | null;
  sha256: string | null;
  indexed_at: number;
  concept_count: number;
  evidence_count: number;
}

export interface SummaryConfigSnapshot {
  config_path: string;
  config_exists: boolean;
  config: Record<string, unknown>;
  effective_contexts: Array<Record<string, unknown>>;
}

export interface SummaryConfigEdit {
  summary_name?: string;
  default_period?: string;
  default_as_of?: string;
  lens_order?: string[];
  lenses?: Array<Record<string, unknown>>;
}

export interface LensResolution {
  lens_id: string;
  label: string;
  child_lens_ids: string[];
  selected_node_ids: string[];
  effective_context: Record<string, unknown>;
  measure_rule: Record<string, unknown>;
  measure: Record<string, unknown>;
  health: Record<string, unknown>;
}

export interface SummaryResolution {
  summary_id: string;
  summary_name: string;
  source: string;
  lenses: LensResolution[];
}

// ── Transport interface ───────────────────────────────────────────────────────

export interface AxonMindTransport {
  // Queries
  focusKpi(input: FocusKpiInput): Promise<FocusKpiOutput>;
  explainKpi(input: ExplainKpiInput): Promise<ExplainKpiOutput>;
  getEvidence(input: GetEvidenceInput): Promise<GetEvidenceOutput>;
  impactRadius(input: ImpactRadiusInput): Promise<ImpactRadiusOutput>;
  traceDecision(input: TraceDecisionInput): Promise<TraceDecisionOutput>;
  suggestActions(input: SuggestActionsInput): Promise<SuggestActionsOutput>;
  graphSearch(input: GraphSearchInput): Promise<GraphSearchOutput>;
  reasoningSearch(input: ReasoningSearchInput): Promise<ReasoningSearchOutput>;
  exportJson(): Promise<GraphExportV1>;
  graphStats(): Promise<GraphStatsOutput>;
  graphDiff(before: GraphExportV1, after: GraphExportV1): Promise<GraphDiff>;
  suggestSummary(doc_ids?: string[], scoped_mode?: ScopedSummaryModeInput): Promise<SuggestedSummary>;
  resolveBrainMapDefaultSummary(doc_ids?: string[]): Promise<SummaryResolution>;
  resolveBrainMapLensChildren(parent_lens_id: string): Promise<LensResolution[]>;
  getBrainMapDefaultConfig(): Promise<SummaryConfigSnapshot>;
  updateBrainMapDefaultConfig(edit: SummaryConfigEdit): Promise<SummaryConfigSnapshot>;
  restoreBrainMapDefaultConfig(): Promise<SummaryConfigSnapshot>;
  listDocuments(): Promise<DocumentSummary[]>;
  removeDocument(node_id: string): Promise<void>;
  regenerateDocument(node_id: string): Promise<IngestSummary>;

  // Ingest
  indexPath(path: string, options?: IndexPathOptions): Promise<IngestSummary>;
  indexMarkdown(text: string, options?: IndexMarkdownOptions): Promise<IngestSummary>;

  // Generations (Phase 4)
  createGenerationFromPaths(name: string, paths: string[]): Promise<string>;
  listGenerations(): Promise<import("./index").GenerationSummary[]>;
  exportGeneration(gen_id: string): Promise<GraphExportV1>;

  // Events — optional; implement for Tauri / WebSocket transports
  onEvent?(handler: (event: EngineEvent) => void): () => void;
}
