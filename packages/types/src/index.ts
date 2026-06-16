// Public TypeScript surface for @axonmind/types — hand-maintained to mirror the
// axonmind_core / axonmind_engine Rust types. The canonical ts-rs-generated bindings
// live in crates/*/bindings/; this package curates and re-states them (plus the
// host-agnostic transport types) as the published API. Keep in sync with Rust by hand.

export type NodeId = string;
export type EdgeId = string;
export type EvidenceId = string;

export type NodeKind =
  | "Kpi" | "Metric" | "Objective" | "Initiative"
  | "Risk" | "Opportunity" | "Decision" | "Insight"
  | "Document" | "Person" | "Team" | "Customer"
  | "Function" | "Product" | "Market" | "Process" | "System" | "Action";

export type EdgeKind =
  | "Influences" | "Causes" | "CorrelatesWith" | "DependsOn" | "DerivedFrom"
  | "Blocks" | "Improves" | "Degrades" | "OwnedBy"
  | "MeasuredBy" | "EvidencedBy" | "MentionedIn" | "DecidedBy"
  | "AssignedTo" | "InFunction" | "ForProduct" | "Impacts" | "NextAction"
  | "Contradicts" | "Corroborates";

export type ExtractorKind = "Manual" | "Connector" | "Rule" | "Llm" | "Calculated";
export type KpiStatus = "Healthy" | "AtRisk" | "Critical" | "Unknown";
export type KpiTrend = "Up" | "Down" | "Flat" | "Unknown";

export interface Node {
  id: NodeId;
  kind: NodeKind;
  name: string;
  created_at: string;
  updated_at: string;
  attrs: Record<string, unknown>;
  confidence: number;
  is_tainted: boolean;
  requires_human_review: boolean;
}

export interface Edge {
  id: EdgeId;
  from: NodeId;
  to: NodeId;
  kind: EdgeKind;
  evidence: EvidenceId[];
  confidence: number;
  created_at: string;
  created_by: ExtractorKind;
  is_tainted: boolean;
  requires_human_review: boolean;
}

export interface Evidence {
  id: EvidenceId;
  source_node_id: NodeId;
  source_type: string;
  quote: string | null;
  row_ref: string | null;
  blob_sha256: string | null;
  timestamp: string | null;
  extractor: ExtractorKind;
  confidence: number;
  is_tainted: boolean;
  requires_human_review: boolean;
}

export interface IngestSummary {
  files_processed: number;
  nodes_created: number;
  edges_created: number;
  evidence_created: number;
  files_skipped: number;
  errors: string[];
}

/** Returned by `parseAndIndex`: ingest stats + provenance + the document rendered to markdown. */
export interface IngestedDocument {
  summary: IngestSummary;
  /** Graph node id (`doc.<sha256[..8]>`) — stable handle for provenance and audit trails. */
  doc_id: string;
  sha256: string;
  title: string | null;
  markdown: string;
}

export type EngineEvent =
  | { type: "node_upserted"; node_id: NodeId }
  | { type: "node_deleted"; node_id: NodeId }
  | { type: "edge_upserted"; edge_id: EdgeId }
  | { type: "edge_deleted"; edge_id: EdgeId }
  | { type: "evidence_added"; evidence_id: EvidenceId }
  | { type: "kpi_candidate_proposed"; candidate_id: string }
  | { type: "kpi_candidate_resolved"; candidate_id: string; status: string }
  | { type: "ingest_started"; job_id: string; path: string }
  | { type: "ingest_progress"; job_id: string; processed: number; total: number | null }
  | { type: "ingest_completed"; job_id: string; summary: IngestSummary }
  | { type: "ingest_failed"; job_id: string; error: string }
  | { type: "cache_rebuilt" };

export type CandidateStatus = "Pending" | "Approved" | "Rejected" | "Merged";

export interface KpiCandidate {
  id: string;
  name: string;
  definition: string | null;
  detected_in: NodeId[];
  confidence: number;
  proposed_at: string;
  status: CandidateStatus;
  merged_into: NodeId | null;
}

export interface MetricValue {
  id: string;
  kpi_node_id: NodeId;
  metric_node_id: NodeId;
  value: number;
  unit: string;
  period_start: string | null;
  period_end: string | null;
  observed_at: string;
  evidence_id: EvidenceId;
}

export interface GraphExportV1 {
  schema_version: number;
  exported_at: string;
  workspace_id: string;
  nodes: Node[];
  edges: Edge[];
  evidence: Evidence[];
  edge_evidence: [EdgeId, EvidenceId][];
  metric_values: MetricValue[];
  kpi_candidates: KpiCandidate[];
}

export interface GenerationSummary {
  id: string;
  name: string;
  created_at: number;   // Unix timestamp (seconds)
  file_count: number;
}

export * from "./transport";
