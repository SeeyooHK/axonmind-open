// Tauri v2 implementation of AxonMindTransport.
//
// Usage in host app:
//   import { invoke } from "@tauri-apps/api/core";
//   import { listen } from "@tauri-apps/api/event";
//   const transport = new TauriTransport(invoke, listen);
//   <AxonMindProvider transport={transport}>...</AxonMindProvider>
//
// `invoke` and `listen` are injected so this module has zero Tauri imports
// and works in any bundling context (Vite, Next.js, etc.).
import type {
  AxonMindTransport, EngineEvent, GenerationSummary, IngestedDocument, IngestSummary,
  FocusKpiInput, FocusKpiOutput,
  ExplainKpiInput, ExplainKpiOutput,
  GetEvidenceInput, GetEvidenceOutput,
  ImpactRadiusInput, ImpactRadiusOutput,
  TraceDecisionInput, TraceDecisionOutput,
  SuggestActionsInput, SuggestActionsOutput,
  GraphSearchInput, GraphSearchOutput,
  ReasoningSearchInput, ReasoningSearchOutput,
  ScopedSummaryModeInput, SuggestedSummary, SummaryResolution, LensResolution,
  SummaryConfigSnapshot, SummaryConfigEdit, DocumentSummary, DocumentVersion,
  IndexMarkdownOptions, IndexPathOptions,
  GraphExportV1, GraphStatsOutput, GraphDiff,
} from "@axonmind/types";

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type ListenFn = <T>(event: string, handler: (payload: { payload: T }) => void) => Promise<() => void>;

const CMD = (name: string) => `plugin:axonmind|${name}`;

export class TauriTransport implements AxonMindTransport {
  constructor(
    private readonly invoke: InvokeFn,
    private readonly listen?: ListenFn,
  ) {}

  private invokeWithFallback<T>(
    pluginCommand: string,
    fallbackCommand: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    return this.invoke<T>(CMD(pluginCommand), args).catch((primaryErr) => {
      console.warn(`[AxonMind] Primary command ${pluginCommand} failed:`, primaryErr);
      return this.invoke<T>(fallbackCommand, args).catch((fallbackErr) => {
        throw primaryErr; // throw the primary error so it bubbles up accurately
      });
    });
  }

  focusKpi(input: FocusKpiInput): Promise<FocusKpiOutput> {
    return this.invoke(CMD("focus_kpi"), { input });
  }

  explainKpi(input: ExplainKpiInput): Promise<ExplainKpiOutput> {
    return this.invoke(CMD("explain_kpi"), { input });
  }

  getEvidence(input: GetEvidenceInput): Promise<GetEvidenceOutput> {
    return this.invoke(CMD("get_evidence"), { input });
  }

  impactRadius(input: ImpactRadiusInput): Promise<ImpactRadiusOutput> {
    return this.invoke(CMD("impact_radius"), { input });
  }

  traceDecision(input: TraceDecisionInput): Promise<TraceDecisionOutput> {
    return this.invoke(CMD("trace_decision"), { input });
  }

  suggestActions(input: SuggestActionsInput): Promise<SuggestActionsOutput> {
    return this.invoke(CMD("suggest_actions"), { input });
  }

  graphSearch(input: GraphSearchInput): Promise<GraphSearchOutput> {
    return this.invoke(CMD("graph_search"), { input });
  }

  reasoningSearch(input: ReasoningSearchInput): Promise<ReasoningSearchOutput> {
    return this.invoke(CMD("reasoning_search"), { input });
  }

  exportJson(): Promise<GraphExportV1> {
    return this.invoke(CMD("export_json"));
  }

  graphStats(): Promise<GraphStatsOutput> {
    return this.invokeWithFallback<GraphStatsOutput>(
      "graph_stats",
      "axonmind_graph_stats",
    );
  }

  graphDiff(before: GraphExportV1, after: GraphExportV1): Promise<GraphDiff> {
    return this.invokeWithFallback<GraphDiff>(
      "graph_diff",
      "axonmind_graph_diff",
      { before, after },
    );
  }

  suggestSummary(
    doc_ids?: string[],
    scoped_mode?: ScopedSummaryModeInput,
  ): Promise<SuggestedSummary> {
    const args = {
      docIds: doc_ids ?? null,
      scopedMode: scoped_mode ?? null,
    };
    return this.invokeWithFallback<SuggestedSummary>(
      "suggest_summary",
      "axonmind_suggest_summary",
      args,
    );
  }

  resolveBrainMapDefaultSummary(doc_ids?: string[]): Promise<SummaryResolution> {
    const args = {
      docIds: doc_ids ?? null,
    };
    return this.invokeWithFallback<SummaryResolution>(
      "resolve_brain_map_default_summary",
      "axonmind_resolve_brain_map_default_summary",
      args,
    );
  }

  resolveBrainMapLensChildren(parent_lens_id: string): Promise<LensResolution[]> {
    const args = {
      parentLensId: parent_lens_id,
    };
    return this.invokeWithFallback<LensResolution[]>(
      "resolve_brain_map_lens_children",
      "axonmind_resolve_brain_map_lens_children",
      args,
    );
  }

  getBrainMapDefaultConfig(): Promise<SummaryConfigSnapshot> {
    return this.invokeWithFallback<SummaryConfigSnapshot>(
      "get_brain_map_default_config",
      "axonmind_get_brain_map_default_config",
    );
  }

  updateBrainMapDefaultConfig(edit: SummaryConfigEdit): Promise<SummaryConfigSnapshot> {
    return this.invokeWithFallback<SummaryConfigSnapshot>(
      "update_brain_map_default_config",
      "axonmind_update_brain_map_default_config",
      { edit },
    );
  }

  restoreBrainMapDefaultConfig(): Promise<SummaryConfigSnapshot> {
    return this.invokeWithFallback<SummaryConfigSnapshot>(
      "restore_brain_map_default_config",
      "axonmind_restore_brain_map_default_config",
    );
  }

  listDocuments(): Promise<DocumentSummary[]> {
    return this.invokeWithFallback<DocumentSummary[]>(
      "list_documents",
      "axonmind_list_documents",
    );
  }

  listIngestStatus(): Promise<import("@axonmind/types").IngestStatusRow[]> {
    return this.invokeWithFallback<import("@axonmind/types").IngestStatusRow[]>(
      "list_ingest_status",
      "axonmind_list_ingest_status",
    );
  }

  listTrash(): Promise<import("@axonmind/types").TrashRow[]> {
    return this.invokeWithFallback<import("@axonmind/types").TrashRow[]>(
      "list_trash",
      "axonmind_list_trash",
    );
  }

  listDocumentVersions(logical_doc_id: string): Promise<DocumentVersion[]> {
    const args = { logicalDocId: logical_doc_id };
    return this.invokeWithFallback<DocumentVersion[]>(
      "list_document_versions",
      "axonmind_list_document_versions",
      args,
    );
  }

  getDocumentContent(node_id: string): Promise<string> {
    const args = { nodeId: node_id };
    return this.invokeWithFallback<string>(
      "get_document_content",
      "axonmind_get_document_content",
      args,
    );
  }

  removeDocument(node_id: string): Promise<void> {
    const args = { nodeId: node_id };
    return this.invokeWithFallback<void>(
      "remove_document",
      "axonmind_remove_document",
      args,
    );
  }

  regenerateDocument(node_id: string): Promise<IngestSummary> {
    const args = { nodeId: node_id };
    return this.invokeWithFallback<IngestSummary>(
      "regenerate_document",
      "axonmind_regenerate_document",
      args,
    );
  }

  trashDocument(source_path: string): Promise<void> {
    const args = { sourcePath: source_path };
    return this.invokeWithFallback<void>(
      "trash_document",
      "axonmind_trash_document",
      args,
    );
  }

  restoreDocument(source_path: string): Promise<void> {
    const args = { sourcePath: source_path };
    return this.invokeWithFallback<void>(
      "restore_document",
      "axonmind_restore_document",
      args,
    );
  }

  deleteDocumentPermanently(source_path: string): Promise<void> {
    const args = { sourcePath: source_path };
    return this.invokeWithFallback<void>(
      "delete_document_permanently",
      "axonmind_delete_document_permanently",
      args,
    );
  }

  emptyTrash(): Promise<void> {
    return this.invokeWithFallback<void>(
      "empty_trash",
      "axonmind_empty_trash",
    );
  }

  startIngest(paths: string[], options?: IndexPathOptions): Promise<string> {
    return this.invokeWithFallback<string>(
      "start_ingest",
      "axonmind_start_ingest",
      {
        paths,
        recursive: options?.recursive ?? true,
        skipUnchanged: options?.skipUnchanged ?? false,
      },
    );
  }

  cancelIngest(job_id: string): Promise<void> {
    return this.invokeWithFallback<void>(
      "cancel_ingest",
      "axonmind_cancel_ingest",
      { jobId: job_id },
    );
  }

  cancelIngestFile(job_id: string, source_path: string): Promise<void> {
    return this.invokeWithFallback<void>(
      "cancel_ingest_file",
      "axonmind_cancel_ingest_file",
      { jobId: job_id, sourcePath: source_path },
    );
  }

  indexPath(path: string, options?: IndexPathOptions): Promise<IngestSummary> {
    return this.invoke(CMD("index_path"), {
      path,
      recursive: options?.recursive ?? true,
      skipUnchanged: options?.skipUnchanged ?? false,
    });
  }

  indexMarkdown(text: string, options?: IndexMarkdownOptions): Promise<IngestSummary> {
    return this.invoke(CMD("index_markdown"), {
      text,
      sourcePath: options?.sourcePath ?? null,
      sha256: options?.sha256 ?? null,
    });
  }

  parseAndIndex(path: string): Promise<IngestedDocument> {
    return this.invoke(CMD("parse_and_index"), { path });
  }

  createGenerationFromPaths(name: string, paths: string[]): Promise<string> {
    return this.invokeWithFallback<string>(
      "create_generation_from_paths",
      "axonmind_create_generation_from_paths",
      { name, paths },
    );
  }

  listGenerations(): Promise<GenerationSummary[]> {
    return this.invokeWithFallback<GenerationSummary[]>(
      "list_generations",
      "axonmind_list_generations",
    );
  }

  exportGeneration(gen_id: string): Promise<GraphExportV1> {
    return this.invokeWithFallback<GraphExportV1>(
      "export_generation",
      "axonmind_export_generation",
      { genId: gen_id },
    );
  }

  onEvent(handler: (event: EngineEvent) => void): () => void {
    if (!this.listen) return () => {};
    let unlisten: (() => void) | undefined;
    this.listen<EngineEvent>("axonmind://event", ({ payload }) => handler(payload))
      .then(fn => { unlisten = fn; });
    return () => { unlisten?.(); };
  }
}
