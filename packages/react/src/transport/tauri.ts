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
  AxonMindTransport, EngineEvent, GenerationSummary, IngestSummary,
  FocusKpiInput, FocusKpiOutput,
  ExplainKpiInput, ExplainKpiOutput,
  GetEvidenceInput, GetEvidenceOutput,
  ImpactRadiusInput, ImpactRadiusOutput,
  TraceDecisionInput, TraceDecisionOutput,
  SuggestActionsInput, SuggestActionsOutput,
  GraphSearchInput, GraphSearchOutput,
  ReasoningSearchInput, ReasoningSearchOutput,
  ScopedSummaryModeInput, SuggestedSummary, SummaryResolution, LensResolution,
  SummaryConfigSnapshot, SummaryConfigEdit, DocumentSummary,
  IndexMarkdownOptions, IndexPathOptions,
  GraphExportV1,
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
    return this.invoke<T>(CMD(pluginCommand), args).catch(() =>
      this.invoke<T>(fallbackCommand, args),
    );
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
