pub mod brain_map;
pub mod config;
pub mod events;
pub mod extract;
pub mod ingest;
pub mod mcp;
pub mod pageindex;
pub mod query;
pub mod store;
pub(crate) mod util;
pub mod workers;

use axonmind_core::{AxonMindError, EdgeKind, KpiAttrs, KpiUnit, Node, NodeId, NodeKind};
use chrono::Datelike;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};

use crate::config::EngineConfig;
use crate::events::EngineEvent;
use crate::extract::fingerprint::{
    DocFingerprint, ReextractDecision, classify, structural_signature,
};
use crate::extract::llm::LlmProvider;
use crate::ingest::{
    IngestOptions, IngestSource, IngestSummary, NormalizedDocument, dispatch_parse,
};
use crate::pageindex::{PageIndexSearchCfg, PageIndexStore};
use crate::query::{
    ExplainKpiInput, ExplainKpiOutput, FocusKpiInput, FocusKpiOutput, GetEvidenceInput,
    GetEvidenceOutput, GraphDiff, GraphSearchInput, GraphSearchOutput, GraphStatsOutput,
    ImpactRadiusInput, ImpactRadiusOutput, NodeKindCount, ReasoningSearchInput,
    ReasoningSearchOutput, SuggestActionsInput, SuggestActionsOutput, TraceDecisionInput,
    TraceDecisionOutput,
};
use crate::store::{
    DocumentSummary, GraphCache, GraphMutation, GraphStore,
    generations::{GenerationId, GenerationSummary},
};

/// Max number of existing concept-node names passed to the LLM entity extractor as the
/// cross-document "avoid duplicating" hint. Bounds prompt size as the graph grows.
const EXISTING_NAME_HINT_LIMIT: usize = 200;

/// Central engine handle. Clone-safe via `Arc` internals.
/// Obtain via `AxonMindEngine::open(config)`.
pub struct AxonMindEngine {
    pub(crate) store: Arc<GraphStore>,
    pub(crate) graph_cache: Arc<RwLock<GraphCache>>,
    pub(crate) event_tx: broadcast::Sender<EngineEvent>,
    pub(crate) config: EngineConfig,
    pub(crate) llm_provider: Arc<RwLock<Option<Arc<dyn LlmProvider>>>>,
}

/// Build the Document node for a normalized document (id derived from content sha).
fn make_document_node(doc: &NormalizedDocument) -> Node {
    let now = chrono::Utc::now();
    Node {
        id: NodeId(doc.id.clone()),
        kind: axonmind_core::NodeKind::Document,
        name: doc.title.clone().unwrap_or_else(|| {
            doc.source_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("Untitled")
                .to_owned()
        }),
        attrs: serde_json::json!({ "sha256": doc.sha256, "source_path": doc.source_path }),
        confidence: axonmind_core::Confidence::RULE,
        created_at: now,
        updated_at: now,
        is_tainted: false,
        requires_human_review: false,
    }
}

fn is_image_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "bmp" | "webp" | "tiff" | "tif" | "gif"
            )
        })
}

fn is_image_doc(doc: &NormalizedDocument) -> bool {
    doc.source_path.as_deref().is_some_and(is_image_path)
}

fn is_llm_entity_parse_error(error: &AxonMindError) -> bool {
    matches!(error, AxonMindError::LlmProvider(message) if message.starts_with("entity parse:"))
}

impl AxonMindEngine {
    /// Open (or create) a workspace. Runs migrations, rebuilds cache, starts workers.
    pub async fn open(config: EngineConfig) -> Result<Self, AxonMindError> {
        tokio::fs::create_dir_all(&config.blob_dir).await?;

        let store = Arc::new(GraphStore::open(&config.database_path).await?);

        let (event_tx, _) = broadcast::channel(config.event_buffer);

        let mut cache = GraphCache::new();
        cache.rebuild_from_db(&store.db).await?;
        let graph_cache = Arc::new(RwLock::new(cache));

        let engine = Self {
            store,
            graph_cache,
            event_tx,
            config,
            llm_provider: Arc::new(RwLock::new(None)),
        };
        engine.start_workers();
        Ok(engine)
    }

    /// Subscribe to engine events. Each call returns an independent receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.event_tx.subscribe()
    }

    /// Inject an LLM provider at startup (before Arc wrapping). Use `update_llm_provider` at runtime.
    pub fn set_llm_provider(&mut self, provider: Arc<dyn LlmProvider>) {
        *self.llm_provider.try_write().expect("llm_provider lock") = Some(provider);
    }

    /// Update the active LLM provider at runtime. Pass `None` to disable LLM extraction.
    pub async fn update_llm_provider(&self, provider: Option<Arc<dyn LlmProvider>>) {
        *self.llm_provider.write().await = provider;
    }

    /// Returns true if an LLM provider is currently configured.
    pub async fn has_llm_provider(&self) -> bool {
        self.llm_provider.read().await.is_some()
    }

    /// Spawn background workers if enabled in config. Called automatically by `open`.
    pub fn start_workers(&self) {
        workers::start_workers(self);
    }

    // ── Ingest ───────────────────────────────────────────────────────────────

    /// Synchronous ingest (waits for completion). Suitable for CLI use.
    pub async fn ingest_sync(
        &self,
        source: IngestSource,
        options: IngestOptions,
    ) -> Result<IngestSummary, AxonMindError> {
        match source {
            IngestSource::File(path) => self.ingest_file(&path, &options).await,
            IngestSource::Directory(dir) => {
                let mut summary = IngestSummary {
                    files_processed: 0,
                    nodes_created: 0,
                    edges_created: 0,
                    evidence_created: 0,
                    files_skipped: 0,
                    errors: vec![],
                };
                let mut stack = vec![dir];
                while let Some(current) = stack.pop() {
                    let mut entries =
                        tokio::fs::read_dir(&current)
                            .await
                            .map_err(|e| AxonMindError::Ingest {
                                message: e.to_string(),
                            })?;
                    while let Some(entry) =
                        entries
                            .next_entry()
                            .await
                            .map_err(|e| AxonMindError::Ingest {
                                message: e.to_string(),
                            })?
                    {
                        let path = entry.path();
                        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if name.starts_with('.') {
                            continue;
                        } // skip hidden
                        if path.is_dir() && options.recursive {
                            stack.push(path);
                        } else if path.is_file() {
                            match self.ingest_file(&path, &options).await {
                                Ok(s) => {
                                    summary.files_processed += s.files_processed;
                                    summary.nodes_created += s.nodes_created;
                                    summary.edges_created += s.edges_created;
                                    summary.evidence_created += s.evidence_created;
                                    summary.files_skipped += s.files_skipped;
                                    summary.errors.extend(s.errors);
                                }
                                Err(e) => {
                                    summary.errors.push(format!("{}: {e}", path.display()));
                                }
                            }
                        }
                    }
                }
                Ok(summary)
            }
            IngestSource::Markdown {
                text,
                source_path,
                sha256,
            } => {
                use sha2::Digest as _;
                let content_sha256 = sha256.unwrap_or_else(|| {
                    sha2::Sha256::digest(text.as_bytes())
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect()
                });
                let path = source_path.unwrap_or_else(|| std::path::PathBuf::from("inline.md"));
                let doc = ingest::markdown::parse_text(&path, &text, content_sha256.clone())
                    .map_err(|e| AxonMindError::Ingest {
                        message: e.to_string(),
                    })?;
                let structural_sha256 = structural_signature(&doc);
                let fp = DocFingerprint {
                    content_sha256,
                    structural_sha256,
                };
                self.ingest_normalized(doc, fp, false).await
            }
            IngestSource::PreParsed(doc) => {
                let structural_sha256 = structural_signature(&doc);
                let fp = DocFingerprint {
                    content_sha256: doc.sha256.clone(),
                    structural_sha256,
                };
                self.ingest_normalized(doc, fp, false).await
            }
            IngestSource::ManualJson(_) => Err(AxonMindError::Ingest {
                message: "ManualJson ingest not implemented in Phase 1".into(),
            }),
        }
    }

    async fn ingest_file(
        &self,
        path: &std::path::Path,
        options: &IngestOptions,
    ) -> Result<IngestSummary, AxonMindError> {
        let bytes = tokio::fs::read(path).await?;

        if bytes.len() as u64 > options.max_file_size_bytes && options.max_file_size_bytes > 0 {
            return Ok(IngestSummary {
                files_skipped: 1,
                errors: vec![format!("{}: file too large", path.display())],
                ..Default::default()
            });
        }

        use sha2::Digest as _;
        let content_sha256: String = sha2::Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        // Copy to blobs/<sha256>
        let blob_path = self.config.blob_dir.join(&content_sha256);
        if !blob_path.exists() {
            tokio::fs::write(&blob_path, &bytes).await?;
        }

        // Parse to compute structural fingerprint.
        // Images go through the async LLM-vision path first (falls back to Tesseract on failure).
        let doc = {
            #[cfg(feature = "llm")]
            {
                if is_image_path(path) {
                    let llm_guard = self.llm_provider.read().await;
                    if let Some(llm) = llm_guard.as_deref() {
                        ingest::image::parse_with_llm(path, &bytes, content_sha256.clone(), llm)
                            .await?
                    } else {
                        dispatch_parse(path, &bytes).map_err(|_| AxonMindError::Ingest {
                            message: "image ingest requires an active LLM provider (configure \
                                      one in Settings) or rebuild with `--features ocr` for \
                                      Tesseract OCR"
                                .into(),
                        })?
                    }
                } else {
                    dispatch_parse(path, &bytes)?
                }
            }
            #[cfg(not(feature = "llm"))]
            dispatch_parse(path, &bytes)?
        };
        let structural_sha256 = structural_signature(&doc);
        let next_fp = DocFingerprint {
            content_sha256,
            structural_sha256,
        };

        let skip_llm = if options.skip_unchanged {
            let path_str = path.to_string_lossy().to_string();
            let cached = self.store.fetch_document_fingerprint(&path_str).await?;
            match classify(cached.as_ref(), &next_fp) {
                ReextractDecision::Skip => {
                    return Ok(IngestSummary {
                        files_skipped: 1,
                        ..Default::default()
                    });
                }
                ReextractDecision::CosmeticRefresh => true,
                ReextractDecision::FullReextract => false,
            }
        } else {
            false
        };

        self.ingest_normalized(doc, next_fp, skip_llm).await
    }

    async fn ingest_normalized(
        &self,
        doc: NormalizedDocument,
        fingerprint: DocFingerprint,
        skip_llm: bool,
    ) -> Result<IngestSummary, AxonMindError> {
        let doc_node = make_document_node(&doc);
        let doc_node_id = doc_node.id.clone();

        let mutations = self
            .build_ingest_mutations(&doc, &doc_node, skip_llm, &Default::default())
            .await?;

        // Apply mutations and track summary
        let mut summary = IngestSummary {
            files_processed: 1,
            ..Default::default()
        };

        for mutation in mutations {
            match &mutation {
                GraphMutation::UpsertNode { .. } => summary.nodes_created += 1,
                GraphMutation::UpsertEdge { .. } => summary.edges_created += 1,
                GraphMutation::UpsertEvidence { .. } => summary.evidence_created += 1,
                _ => {}
            }
            if let Err(e) = self
                .store
                .apply_mutation(mutation, &self.graph_cache, &self.event_tx)
                .await
            {
                summary.errors.push(e.to_string());
            }
        }

        self.run_ingest_tail(&doc, &fingerprint, &doc_node_id, &mut summary)
            .await;

        Ok(summary)
    }

    /// Shared tail for all ingest paths: upsert_document_cache + pageindex hook.
    /// Graph mutations must already be applied before calling this.
    /// Errors from pageindex are non-fatal: they are pushed to `summary.errors`.
    async fn run_ingest_tail(
        &self,
        doc: &NormalizedDocument,
        fingerprint: &extract::fingerprint::DocFingerprint,
        doc_node_id: &NodeId,
        summary: &mut IngestSummary,
    ) {
        if let Some(path) = &doc.source_path {
            let _ = self
                .store
                .upsert_document_cache(
                    &path.to_string_lossy(),
                    &fingerprint.content_sha256,
                    &fingerprint.structural_sha256,
                    doc_node_id,
                )
                .await;
        }

        let page_store = PageIndexStore::new(self.store.db.0.clone());
        let llm = self.llm_provider.read().await.clone();
        if let Err(e) = pageindex::index_document(
            doc,
            &doc_node_id.0,
            &page_store,
            llm.as_deref(),
            &self.config,
        )
        .await
        {
            summary.errors.push(format!("pageindex: {e}"));
        }
    }

    // ── Document management ─────────────────────────────────────────────────────

    /// List every processed document with extraction counts, for the file-list UI.
    pub async fn list_documents(&self) -> Result<Vec<DocumentSummary>, AxonMindError> {
        self.store.list_document_summaries().await
    }

    /// Build (but do not apply) all extraction mutations for a document: the Document node, rule
    /// extraction, LLM entity+relation extraction, cross-document semantic links, and the
    /// deterministic bridge. Kept separate from application so callers can apply atomically.
    ///
    /// `exclude_from_existing`: concept node ids to exclude from the "existing concepts" view fed
    /// to the bridge and semantic linker. Used by `regenerate_document` to prevent the bridge from
    /// creating edges to concepts that are about to be deleted in the same batch — which would
    /// produce `NodeNotFound` when those edge insertions are processed after the deletions.
    async fn build_ingest_mutations(
        &self,
        doc: &NormalizedDocument,
        doc_node: &Node,
        skip_llm: bool,
        exclude_from_existing: &std::collections::HashSet<String>,
    ) -> Result<Vec<GraphMutation>, AxonMindError> {
        use axonmind_core::NodeKind;

        let mut mutations = vec![GraphMutation::UpsertNode {
            node: doc_node.clone(),
        }];

        // Rule extraction
        let rule_mutations = extract::rules::extract(doc, doc_node);
        let rule_node_ids: std::collections::HashSet<String> = rule_mutations
            .iter()
            .filter_map(|m| match m {
                GraphMutation::UpsertNode { node } => Some(node.id.0.clone()),
                _ => None,
            })
            .collect();
        mutations.extend(rule_mutations);

        // Phase 8 (open-repo): toy insurance-table enrichment seam.
        // Intentionally non-production; only obvious policy/claim table shapes are handled.
        let toy_insurance_mutations = extract::insurance_toy::extract(doc, doc_node);
        mutations.extend(toy_insurance_mutations);

        // Concept nodes already in the graph from other documents — drives cross-document dedup
        // (LLM "avoid duplicating" hint) and the deterministic bridge.
        // Concepts in `exclude_from_existing` are about to be deleted in the same batch (regenerate
        // path) and must not appear as bridge/linker targets or they cause NodeNotFound.
        let existing_concepts: Vec<(NodeId, String)> = self
            .store
            .fetch_concept_node_id_names(EXISTING_NAME_HINT_LIMIT)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|(id, _)| !exclude_from_existing.contains(&id.0))
            .collect();

        // Resolve the LLM provider once so we don't hold the lock across awaits.
        let llm_opt = if self.config.enable_llm_extraction && !skip_llm {
            self.llm_provider.read().await.clone()
        } else {
            None
        };

        if let Some(llm) = &llm_opt {
            let rule_edge_pairs: std::collections::HashSet<(String, String)> = mutations
                .iter()
                .filter_map(|m| match m {
                    GraphMutation::UpsertEdge { edge, .. } => {
                        Some((edge.from.0.clone(), edge.to.0.clone()))
                    }
                    _ => None,
                })
                .collect();
            let existing_graph_names: Vec<String> = existing_concepts
                .iter()
                .map(|(_, name)| name.clone())
                .collect();
            let llm_muts = extract::relation::run_llm_extraction(
                llm.as_ref(),
                doc,
                doc_node,
                rule_node_ids,
                existing_graph_names,
                rule_edge_pairs,
            )
            .await;
            match llm_muts {
                Ok(llm_muts) => mutations.extend(llm_muts),
                Err(e) if is_image_doc(doc) && is_llm_entity_parse_error(&e) => {
                    tracing::warn!("skipping image LLM entity extraction after parse error: {e}");
                }
                Err(e) => {
                    return Err(AxonMindError::Ingest {
                        message: format!("LLM extraction failed: {e}"),
                    });
                }
            }
        }

        let new_concepts: Vec<(NodeId, String)> = mutations
            .iter()
            .filter_map(|m| match m {
                GraphMutation::UpsertNode { node } if node.kind != NodeKind::Document => {
                    Some((node.id.clone(), node.name.clone()))
                }
                _ => None,
            })
            .collect();

        if let Some(llm) = &llm_opt {
            let sem_muts = extract::semantic::run_semantic_linking(
                llm.as_ref(),
                doc_node,
                &doc.sha256,
                &new_concepts,
                &existing_concepts,
            )
            .await
            .map_err(|e| AxonMindError::Ingest {
                message: format!("semantic linking failed: {e}"),
            })?;
            mutations.extend(sem_muts);
        }

        let bridge_muts = extract::bridge::build_cross_document_bridges(
            doc_node,
            &doc.sha256,
            &new_concepts,
            &existing_concepts,
        );
        mutations.extend(bridge_muts);

        Ok(mutations)
    }

    /// Build the mutations that delete a document and the concepts that belong only to it. A
    /// concept referenced by exactly one document (this one) is orphaned by the removal and
    /// included; shared concepts are left untouched. Errors if `doc_id` isn't a Document.
    async fn build_removal_mutations(
        &self,
        doc_id: &NodeId,
    ) -> Result<Vec<GraphMutation>, AxonMindError> {
        let node = self
            .store
            .fetch_node(doc_id)
            .await?
            .ok_or_else(|| AxonMindError::Ingest {
                message: format!("document not found: {}", doc_id.0),
            })?;
        if node.kind != axonmind_core::NodeKind::Document {
            return Err(AxonMindError::Ingest {
                message: format!("{} is not a document", doc_id.0),
            });
        }

        let mentioned = self.store.fetch_mentioned_node_ids(doc_id).await?;
        let mut mutations = Vec::new();
        for concept_id in mentioned {
            // Exactly one source document (this one) → the concept becomes an orphan on removal.
            if self
                .store
                .count_source_documents_for_node(&concept_id)
                .await?
                == 1
            {
                mutations.push(GraphMutation::DeleteNode {
                    node_id: concept_id,
                });
            }
        }
        mutations.push(GraphMutation::DeleteNode {
            node_id: doc_id.clone(),
        });
        Ok(mutations)
    }

    /// Remove a document and everything derived solely from it, atomically. Deleting a node
    /// cascades (via SQLite FK) its edges, evidence, and `document_cache` row; orphaned concepts
    /// are deleted in the same transaction. When `delete_blob` is set, the blob is removed too —
    /// but only if no other document still references that content hash.
    pub async fn remove_document(
        &self,
        node_id: NodeId,
        delete_blob: bool,
    ) -> Result<(), AxonMindError> {
        // Capture the blob hash before removal (for optional cleanup).
        let sha = self.store.fetch_node(&node_id).await?.and_then(|n| {
            n.attrs
                .get("sha256")
                .and_then(|v| v.as_str().map(str::to_owned))
        });

        let removal = self.build_removal_mutations(&node_id).await?;
        self.store
            .apply_batch(removal, &self.graph_cache, &self.event_tx)
            .await?;

        PageIndexStore::new(self.store.db.0.clone())
            .delete_document(&node_id.0)
            .await?;

        if delete_blob {
            if let Some(sha) = sha {
                if self.store.count_documents_with_sha(&sha).await? == 0 {
                    let _ = tokio::fs::remove_file(self.config.blob_dir.join(&sha)).await;
                }
            }
        }
        Ok(())
    }

    /// Remove a document's derived data and re-extract it from scratch, atomically.
    ///
    /// The new extraction (including the slow LLM passes) is computed FIRST, with no database
    /// writes — so if parsing or extraction fails, the existing graph is left completely intact.
    /// Only once the new mutations are ready are the old data's removal and the new data applied
    /// together in a single transaction. Recomputation reads the retained blob, never the original
    /// path, so it works even if the source file moved or was deleted.
    pub async fn regenerate_document(
        &self,
        node_id: NodeId,
    ) -> Result<IngestSummary, AxonMindError> {
        let node = self
            .store
            .fetch_node(&node_id)
            .await?
            .ok_or_else(|| AxonMindError::Ingest {
                message: format!("document not found: {}", node_id.0),
            })?;
        if node.kind != axonmind_core::NodeKind::Document {
            return Err(AxonMindError::Ingest {
                message: format!("{} is not a document", node_id.0),
            });
        }
        let sha = node
            .attrs
            .get("sha256")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AxonMindError::Ingest {
                message: format!("{} has no stored blob to regenerate from", node_id.0),
            })?
            .to_owned();
        let source_path = node
            .attrs
            .get("source_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AxonMindError::Ingest {
                message: format!("{} has no source path to reparse with", node_id.0),
            })?
            .to_owned();

        // Read the blob, parse, and run the full extraction — no DB writes yet, so a failure here
        // leaves the existing graph untouched.
        let bytes = tokio::fs::read(self.config.blob_dir.join(&sha))
            .await
            .map_err(|e| AxonMindError::Ingest {
                message: format!("blob read failed for {}: {e}", node_id.0),
            })?;
        let path = std::path::PathBuf::from(&source_path);
        let doc = {
            #[cfg(feature = "llm")]
            {
                if is_image_path(&path) {
                    let llm_guard = self.llm_provider.read().await;
                    if let Some(llm) = llm_guard.as_deref() {
                        ingest::image::parse_with_llm(&path, &bytes, sha.clone(), llm).await?
                    } else {
                        dispatch_parse(&path, &bytes).map_err(|_| AxonMindError::Ingest {
                            message: "image ingest requires an active LLM provider (configure \
                                      one in Settings) or rebuild with `--features ocr` for \
                                      Tesseract OCR"
                                .into(),
                        })?
                    }
                } else {
                    dispatch_parse(&path, &bytes)?
                }
            }
            #[cfg(not(feature = "llm"))]
            dispatch_parse(&path, &bytes)?
        };
        let fingerprint = DocFingerprint {
            content_sha256: sha,
            structural_sha256: structural_signature(&doc),
        };
        let doc_node = make_document_node(&doc);
        let doc_node_id = doc_node.id.clone();

        // Build removal mutations first (read-only) so we know which concepts are about to be
        // deleted. This is still safe: no DB writes happen until apply_batch below.
        // Concepts in the removal set must be excluded from the bridge/linker's "existing" view —
        // they will be deleted in the same batch, so edges TO them would produce NodeNotFound.
        let removal = self.build_removal_mutations(&node_id).await?;
        let to_delete: std::collections::HashSet<String> = removal
            .iter()
            .filter_map(|m| match m {
                GraphMutation::DeleteNode { node_id } => Some(node_id.0.clone()),
                _ => None,
            })
            .collect();

        let new_mutations = self
            .build_ingest_mutations(&doc, &doc_node, false, &to_delete)
            .await?;

        let mut summary = IngestSummary {
            files_processed: 1,
            ..Default::default()
        };
        for m in &new_mutations {
            match m {
                GraphMutation::UpsertNode { .. } => summary.nodes_created += 1,
                GraphMutation::UpsertEdge { .. } => summary.edges_created += 1,
                GraphMutation::UpsertEvidence { .. } => summary.evidence_created += 1,
                _ => {}
            }
        }

        // Apply the old data's removal and the new data together: all-or-nothing.
        let mut batch = removal;
        batch.extend(new_mutations);
        self.store
            .apply_batch(batch, &self.graph_cache, &self.event_tx)
            .await?;

        self.run_ingest_tail(&doc, &fingerprint, &doc_node_id, &mut summary)
            .await;

        Ok(summary)
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    pub async fn focus_kpi(&self, input: FocusKpiInput) -> Result<FocusKpiOutput, AxonMindError> {
        query::focus::focus_kpi(input, &self.store, &self.graph_cache).await
    }

    pub async fn explain_kpi(
        &self,
        input: ExplainKpiInput,
    ) -> Result<ExplainKpiOutput, AxonMindError> {
        let guard = self.llm_provider.read().await;
        let llm = (*guard).as_deref();
        query::evidence::explain_kpi(input, &self.store, &self.graph_cache, llm).await
    }

    pub async fn get_evidence(
        &self,
        input: GetEvidenceInput,
    ) -> Result<GetEvidenceOutput, AxonMindError> {
        query::evidence::get_evidence(input, &self.store).await
    }

    pub async fn impact_radius(
        &self,
        input: ImpactRadiusInput,
    ) -> Result<ImpactRadiusOutput, AxonMindError> {
        query::impact::impact_radius(input, &self.store, &self.graph_cache).await
    }

    pub async fn trace_decision(
        &self,
        input: TraceDecisionInput,
    ) -> Result<TraceDecisionOutput, AxonMindError> {
        query::impact::trace_decision(input, &self.store, &self.graph_cache).await
    }

    pub async fn suggest_actions(
        &self,
        input: SuggestActionsInput,
    ) -> Result<SuggestActionsOutput, AxonMindError> {
        query::impact::suggest_actions(input, &self.store, &self.graph_cache).await
    }

    pub async fn graph_search(
        &self,
        input: GraphSearchInput,
    ) -> Result<GraphSearchOutput, AxonMindError> {
        query::search::graph_search(input, &self.store).await
    }

    /// Vectorless two-stage retrieval: BM25 recall → LLM reasoning precision.
    /// Returns ranked sections with raw passage text; degrades gracefully to BM25-only
    /// when no LLM provider is configured (`reasoning_applied = false`).
    pub async fn reasoning_search(
        &self,
        input: ReasoningSearchInput,
    ) -> Result<ReasoningSearchOutput, AxonMindError> {
        let store = PageIndexStore::new(self.store.db.0.clone());
        let llm = self.llm_provider.read().await.clone();
        let cfg = PageIndexSearchCfg {
            shortlist_limit: self.config.pageindex_shortlist_limit,
            ..Default::default()
        };
        pageindex::search::reasoning_search(input, &store, llm.as_deref(), &cfg).await
    }

    pub async fn graph_stats(&self) -> Result<GraphStatsOutput, AxonMindError> {
        let conn = self
            .store
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(|conn| -> Result<GraphStatsOutput, AxonMindError> {
            let total_nodes: i64 = conn
                .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let document_nodes: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM nodes WHERE kind = 'Document'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let total_edges: i64 = conn
                .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let total_evidence: i64 = conn
                .query_row("SELECT COUNT(*) FROM evidence", [], |r| r.get(0))
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let avg_confidence: f64 = conn
                .query_row(
                    "SELECT COALESCE(AVG(confidence), 0.0) FROM nodes WHERE kind != 'Document'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let tainted_nodes: i64 = conn
                .query_row("SELECT COUNT(*) FROM nodes WHERE is_tainted = 1", [], |r| {
                    r.get(0)
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let tainted_edges: i64 = conn
                .query_row("SELECT COUNT(*) FROM edges WHERE is_tainted = 1", [], |r| {
                    r.get(0)
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let review_required_nodes: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM nodes WHERE requires_human_review = 1",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let mut stmt = conn
                .prepare("SELECT kind, COUNT(*) FROM nodes GROUP BY kind ORDER BY kind")
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let nodes_by_kind: Vec<NodeKindCount> = stmt
                .query_map([], |row| {
                    let kind_str: String = row.get(0)?;
                    let count: i64 = row.get(1)?;
                    Ok((kind_str, count))
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .filter_map(|r| r.ok())
                .filter_map(|(kind_str, count)| {
                    let kind = serde_json::from_str::<axonmind_core::NodeKind>(&format!(
                        "\"{}\"",
                        kind_str
                    ))
                    .ok()?;
                    Some(NodeKindCount {
                        kind,
                        count: count as usize,
                    })
                })
                .collect();

            Ok(GraphStatsOutput {
                total_nodes: total_nodes as usize,
                document_nodes: document_nodes as usize,
                concept_nodes: (total_nodes - document_nodes) as usize,
                total_edges: total_edges as usize,
                total_evidence: total_evidence as usize,
                avg_confidence,
                tainted_nodes: tainted_nodes as usize,
                tainted_edges: tainted_edges as usize,
                review_required_nodes: review_required_nodes as usize,
                nodes_by_kind,
            })
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub fn graph_diff(
        &self,
        before: &query::GraphExportV1,
        after: &query::GraphExportV1,
    ) -> GraphDiff {
        query::diff::diff_exports(before, after)
    }

    // ── Export / import ───────────────────────────────────────────────────────

    /// Import a `GraphExportV1` into this workspace.
    ///
    /// Replays all rows as `GraphMutation`s in dependency order so the evidence invariant
    /// and FTS5 sync are enforced on import exactly as on any write. Rejects exports with
    /// a mismatched `schema_version` before writing anything.
    pub async fn import_export(
        &self,
        export: query::GraphExportV1,
    ) -> Result<IngestSummary, AxonMindError> {
        const SUPPORTED_VERSION: u32 = 1;
        if export.schema_version != SUPPORTED_VERSION {
            return Err(AxonMindError::Ingest {
                message: format!(
                    "unsupported schema_version {}: expected {SUPPORTED_VERSION}",
                    export.schema_version
                ),
            });
        }

        // Build edge_id → evidence_ids index for O(1) lookup during edge replay.
        let mut ev_by_edge: std::collections::HashMap<String, Vec<axonmind_core::EvidenceId>> =
            std::collections::HashMap::new();
        for (edge_id, ev_id) in &export.edge_evidence {
            ev_by_edge
                .entry(edge_id.0.clone())
                .or_default()
                .push(ev_id.clone());
        }

        let mut summary = IngestSummary::default();

        // 1. Nodes
        for node in export.nodes {
            match self
                .store
                .apply_mutation(
                    GraphMutation::UpsertNode { node },
                    &self.graph_cache,
                    &self.event_tx,
                )
                .await
            {
                Ok(_) => summary.nodes_created += 1,
                Err(e) => summary.errors.push(format!("node: {e}")),
            }
        }

        // 2. Evidence
        for ev in export.evidence {
            match self
                .store
                .apply_mutation(
                    GraphMutation::UpsertEvidence { evidence: ev },
                    &self.graph_cache,
                    &self.event_tx,
                )
                .await
            {
                Ok(_) => summary.evidence_created += 1,
                Err(e) => summary.errors.push(format!("evidence: {e}")),
            }
        }

        // 3. Edges (with evidence_ids derived from edge_evidence)
        for edge in export.edges {
            let evidence_ids = ev_by_edge.get(&edge.id.0).cloned().unwrap_or_default();
            match self
                .store
                .apply_mutation(
                    GraphMutation::UpsertEdge { edge, evidence_ids },
                    &self.graph_cache,
                    &self.event_tx,
                )
                .await
            {
                Ok(_) => summary.edges_created += 1,
                Err(e) => summary.errors.push(format!("edge: {e}")),
            }
        }

        // 4. Metric values
        for mv in export.metric_values {
            if let Err(e) = self
                .store
                .apply_mutation(
                    GraphMutation::RecordMetricValue { value: mv },
                    &self.graph_cache,
                    &self.event_tx,
                )
                .await
            {
                summary.errors.push(format!("metric_value: {e}"));
            }
        }

        // 5. KPI candidates
        for candidate in export.kpi_candidates {
            if let Err(e) = self
                .store
                .apply_mutation(
                    GraphMutation::ProposeKpiCandidate { candidate },
                    &self.graph_cache,
                    &self.event_tx,
                )
                .await
            {
                summary.errors.push(format!("kpi_candidate: {e}"));
            }
        }

        summary.files_processed = 1;
        Ok(summary)
    }

    // ── Generations ───────────────────────────────────────────────────────────

    pub async fn create_generation(&self, name: String) -> Result<GenerationId, AxonMindError> {
        self.store.create_generation(name).await
    }

    pub async fn list_generations(&self) -> Result<Vec<GenerationSummary>, AxonMindError> {
        self.store.list_generations().await
    }

    /// Create a generation from paths already indexed in this workspace.
    ///
    /// Looks up each path in `document_cache` to get its current sha256, then records
    /// `generation_source` and `source_version` entries. Paths not found in the cache
    /// (not yet ingested) are skipped silently — the caller must ingest first.
    pub async fn create_generation_from_paths(
        &self,
        name: String,
        paths: Vec<String>,
    ) -> Result<GenerationId, AxonMindError> {
        let gen_id = self.store.create_generation(name).await?;
        for path in paths {
            // Look up sha256 from document_cache
            let fingerprint = self.store.fetch_document_fingerprint(&path).await?;
            if let Some(fp) = fingerprint {
                let _ = self
                    .store
                    .record_generation_source(&gen_id, path, fp.content_sha256)
                    .await;
            }
        }
        Ok(gen_id)
    }

    pub async fn export_generation(
        &self,
        gen_id: GenerationId,
    ) -> Result<query::GraphExportV1, AxonMindError> {
        use crate::config::WorkspaceManifest;
        let workspace_id =
            tokio::fs::read_to_string(self.config.workspace_dir.join("workspace.json"))
                .await
                .ok()
                .and_then(|s| serde_json::from_str::<WorkspaceManifest>(&s).ok())
                .map(|m| m.id)
                .unwrap_or_default();

        let (nodes, edges, evidence, edge_evidence) =
            self.store.fetch_export_for_generation(&gen_id).await?;

        Ok(query::GraphExportV1 {
            schema_version: 1,
            exported_at: chrono::Utc::now(),
            workspace_id,
            nodes,
            edges,
            evidence,
            edge_evidence,
            metric_values: vec![], // scoped export omits metric_values (not evidence-tagged)
            kpi_candidates: vec![], // scoped export omits candidates (not evidence-tagged)
        })
    }

    /// Phase 1 brain map: organize the graph into ≤10 categories for the radial summary.
    ///
    /// `doc_ids = Some([..])` scopes the summary to the concepts of those documents (their
    /// `MentionedIn` members and the edges among them); `None`/empty uses the whole graph.
    /// LLM-suggested when a provider is active (system prompt assembled from `extract::prompts`,
    /// honoring override files under `<workspace>/prompts/`); deterministic group-by-kind fallback
    /// otherwise. For unscoped summaries (`doc_ids = None`), persists/loads
    /// `<workspace>/summaries/default.json`. Scoped summaries are cached per file selection under
    /// `<workspace>/summaries/scoped/`.
    pub async fn suggest_summary(
        &self,
        doc_ids: Option<Vec<NodeId>>,
    ) -> Result<extract::summarize::SuggestedSummary, AxonMindError> {
        self.suggest_summary_with_mode(doc_ids, brain_map::ScopedSummaryMode::Auto)
            .await
    }

    /// Same as `suggest_summary`, but allows caller control over scoped cache behavior.
    pub async fn suggest_summary_with_mode(
        &self,
        doc_ids: Option<Vec<NodeId>>,
        scoped_mode: brain_map::ScopedSummaryMode,
    ) -> Result<extract::summarize::SuggestedSummary, AxonMindError> {
        let export = self.export_json().await?;
        let scoped_doc_ids = doc_ids.unwrap_or_default();
        let is_scoped = !scoped_doc_ids.is_empty();
        let provider = self.llm_provider.read().await.clone();
        let library =
            extract::prompts::PromptLibrary::new(Some(self.config.workspace_dir.join("prompts")));
        let scoped_cache_meta = if is_scoped {
            let scope_doc_id_strings = scoped_doc_ids
                .iter()
                .map(|id| id.0.clone())
                .collect::<Vec<_>>();
            let scope_key = brain_map::scoped_summary_scope_key(&scope_doc_id_strings);
            let node_by_id: std::collections::HashMap<&str, &Node> =
                export.nodes.iter().map(|n| (n.id.0.as_str(), n)).collect();
            let doc_signatures = scope_doc_id_strings
                .iter()
                .map(|id| {
                    let node = node_by_id.get(id.as_str()).copied();
                    let sha = node
                        .and_then(|n| n.attrs.get("sha256"))
                        .and_then(|v| v.as_str())
                        .map(ToOwned::to_owned);
                    let source_path = node
                        .and_then(|n| n.attrs.get("source_path"))
                        .and_then(|v| v.as_str())
                        .map(ToOwned::to_owned);
                    (id.clone(), sha, source_path)
                })
                .collect::<Vec<_>>();
            let input_signature =
                brain_map::scoped_summary_input_signature(&doc_signatures, provider.is_some());
            Some((scope_key, input_signature))
        } else {
            None
        };

        let (nodes, edges) = if is_scoped {
            // Union of concept nodes belonging to the selected documents; then keep only the
            // edges whose both endpoints are in that set (drops cross-doc and MentionedIn).
            let mut members = std::collections::HashSet::new();
            for id in &scoped_doc_ids {
                for concept in self.store.fetch_mentioned_node_ids(id).await? {
                    members.insert(concept.0);
                }
            }
            let nodes = export
                .nodes
                .iter()
                .filter(|n| members.contains(&n.id.0))
                .cloned()
                .collect::<Vec<_>>();
            let edges = export
                .edges
                .iter()
                .filter(|e| members.contains(&e.from.0) && members.contains(&e.to.0))
                .cloned()
                .collect::<Vec<_>>();
            (nodes, edges)
        } else {
            (export.nodes, export.edges)
        };

        // Scoped summaries are cached by selected-doc scope + source-signature.
        if is_scoped {
            let (scope_key, input_signature) =
                scoped_cache_meta.ok_or_else(|| AxonMindError::ValidationFailed {
                    message: "missing scoped cache metadata".to_string(),
                })?;

            if scoped_mode != brain_map::ScopedSummaryMode::Regenerate {
                if let Some(entry) =
                    brain_map::load_scoped_summary_cache(&self.config.workspace_dir, &scope_key)
                        .await?
                {
                    if entry.input_signature == input_signature {
                        let mut cached = entry.summary;
                        cached.source = "cache".to_string();
                        return Ok(cached);
                    }
                    if scoped_mode == brain_map::ScopedSummaryMode::CachedOnly {
                        let mut cached = entry.summary;
                        cached.source = "cache".to_string();
                        return Ok(cached);
                    }
                } else if scoped_mode == brain_map::ScopedSummaryMode::CachedOnly {
                    return Err(AxonMindError::ValidationFailed {
                        message: "no cached scoped summary exists yet for this selected file set"
                            .to_string(),
                    });
                }
            }

            let mut suggested =
                extract::summarize::suggest_summary(provider.as_deref(), &library, &nodes, &edges)
                    .await?;
            let now = chrono::Utc::now().timestamp();
            let created_at =
                brain_map::load_scoped_summary_cache(&self.config.workspace_dir, &scope_key)
                    .await?
                    .map(|e| e.created_at)
                    .unwrap_or(now);
            brain_map::save_scoped_summary_cache(
                &self.config.workspace_dir,
                &brain_map::ScopedSummaryCacheEntry {
                    scope_key,
                    input_signature,
                    summary: suggested.clone(),
                    created_at,
                    updated_at: now,
                },
            )
            .await?;
            suggested.source = "llm".to_string();
            return Ok(suggested);
        }

        if let Some(cfg) = brain_map::load_default_summary(&self.config.workspace_dir).await? {
            cfg.validate_and_compute_effective_contexts()?;
            return Ok(cfg.to_suggested_summary(&nodes, "config".to_string()));
        }

        let suggested =
            extract::summarize::suggest_summary(provider.as_deref(), &library, &nodes, &edges)
                .await?;
        let source = suggested.source.clone();
        let cfg = brain_map::SummaryConfig::from_suggested(suggested, &nodes);
        cfg.validate_and_compute_effective_contexts()?;
        brain_map::save_default_summary(&self.config.workspace_dir, &cfg).await?;
        Ok(cfg.to_suggested_summary(&nodes, source))
    }

    /// Return the persisted default Brain Map config plus computed effective contexts.
    pub async fn get_brain_map_default_config(
        &self,
    ) -> Result<brain_map::SummaryConfigSnapshot, AxonMindError> {
        // Ensure the default config exists by materializing the unscoped summary once.
        let _ = self.suggest_summary(None).await?;

        let path = brain_map::default_summary_path(&self.config.workspace_dir);
        let cfg = brain_map::load_default_summary(&self.config.workspace_dir)
            .await?
            .ok_or_else(|| AxonMindError::ValidationFailed {
                message: format!("default summary config not found at '{}'", path.display()),
            })?;
        let effective_contexts = cfg.validate_and_compute_effective_contexts()?;
        Ok(brain_map::SummaryConfigSnapshot {
            config_path: path.display().to_string(),
            config_exists: true,
            config: cfg,
            effective_contexts,
        })
    }

    /// Apply safe user edits to the persisted default Brain Map config.
    pub async fn update_brain_map_default_config(
        &self,
        edit: brain_map::SummaryConfigEdit,
    ) -> Result<brain_map::SummaryConfigSnapshot, AxonMindError> {
        // Ensure config exists.
        let _ = self.suggest_summary(None).await?;
        let path = brain_map::default_summary_path(&self.config.workspace_dir);
        let mut cfg = brain_map::load_default_summary(&self.config.workspace_dir)
            .await?
            .ok_or_else(|| AxonMindError::ValidationFailed {
                message: format!("default summary config not found at '{}'", path.display()),
            })?;

        brain_map::apply_summary_config_edit(&mut cfg, edit)?;
        let effective_contexts = cfg.validate_and_compute_effective_contexts()?;
        brain_map::save_default_summary(&self.config.workspace_dir, &cfg).await?;

        Ok(brain_map::SummaryConfigSnapshot {
            config_path: path.display().to_string(),
            config_exists: true,
            config: cfg,
            effective_contexts,
        })
    }

    /// Restore the persisted default summary config by deleting it and regenerating from graph.
    pub async fn restore_brain_map_default_config(
        &self,
    ) -> Result<brain_map::SummaryConfigSnapshot, AxonMindError> {
        let path = brain_map::default_summary_path(&self.config.workspace_dir);
        if tokio::fs::try_exists(&path).await? {
            tokio::fs::remove_file(&path).await?;
        }
        let _ = self.suggest_summary(None).await?;
        self.get_brain_map_default_config().await
    }

    /// Resolve top-level lens headline measures for the persisted default Brain Map summary.
    pub async fn resolve_brain_map_default_summary(
        &self,
        doc_ids: Option<Vec<NodeId>>,
    ) -> Result<brain_map::SummaryResolution, AxonMindError> {
        if matches!(doc_ids.as_ref(), Some(ids) if !ids.is_empty()) {
            let suggested = self.suggest_summary(doc_ids).await?;
            let lenses = suggested
                .categories
                .into_iter()
                .map(|c| {
                    let count = c.member_node_ids.len() as f64;
                    brain_map::LensResolution {
                        lens_id: c.label.to_ascii_lowercase().replace(' ', "_"),
                        label: c.label,
                        child_lens_ids: vec![],
                        selected_node_ids: c.member_node_ids.clone(),
                        effective_context: brain_map::EffectiveLensContext {
                            lens_id: "scoped".to_string(),
                            effective_selector: serde_json::json!({ "type": "saved_query", "query": "scoped_members" }),
                            effective_period: "latest".to_string(),
                            effective_as_of: "latest".to_string(),
                        },
                        measure_rule: serde_json::json!({
                            "type": "count",
                            "period": "latest",
                            "as_of": "latest"
                        }),
                        measure: brain_map::MeasureResolution {
                            measure_type: "count".to_string(),
                            state: brain_map::MeasureState::Resolved,
                            value: Some(count),
                            unit: Some("count".to_string()),
                            confidence: None,
                            observed_at: None,
                            explanation: Some("scoped summary uses member-count measure".to_string()),
                            evidence_ids: vec![],
                            evidence_lineage: vec![],
                            lineage_gaps: vec![lineage_gap(
                                "aggregate_not_evidence_backed",
                                "scoped count is an aggregate and does not map to a single evidence record in v1",
                            )],
                            supporting_nodes: vec![],
                        },
                        health: brain_map::HealthResolution {
                            state: brain_map::HealthState::Good,
                            explanation: Some("scoped summary uses presence-based health".to_string()),
                        },
                    }
                })
                .collect::<Vec<_>>();
            return Ok(brain_map::SummaryResolution {
                summary_id: "scoped".to_string(),
                summary_name: "Scoped Summary".to_string(),
                source: suggested.source,
                lenses,
            });
        }

        let source = self.suggest_summary(None).await?.source;
        let snapshot = self.get_brain_map_default_config().await?;
        let cfg = snapshot.config;
        let contexts = snapshot.effective_contexts;
        let export = self.export_json().await?;

        let mut nodes_by_id = std::collections::HashMap::<String, &Node>::new();
        for n in &export.nodes {
            nodes_by_id.insert(n.id.0.clone(), n);
        }

        let mut metric_values_by_kpi =
            std::collections::HashMap::<String, Vec<&store::MetricValue>>::new();
        for mv in &export.metric_values {
            metric_values_by_kpi
                .entry(mv.kpi_node_id.0.clone())
                .or_default()
                .push(mv);
        }
        let evidence_by_id: std::collections::HashMap<String, &axonmind_core::Evidence> = export
            .evidence
            .iter()
            .map(|e| (e.id.0.clone(), e))
            .collect();

        let context_by_lens: std::collections::HashMap<String, brain_map::EffectiveLensContext> =
            contexts
                .into_iter()
                .map(|c| (c.lens_id.clone(), c))
                .collect();
        let lens_by_id: std::collections::HashMap<String, &brain_map::LensDefinition> =
            cfg.lenses.iter().map(|l| (l.id.clone(), l)).collect();

        let lenses = resolve_lens_ids(
            &cfg.summary.lenses,
            &lens_by_id,
            &context_by_lens,
            &export.nodes,
            &export.edges,
            &nodes_by_id,
            &metric_values_by_kpi,
            &evidence_by_id,
        );

        Ok(brain_map::SummaryResolution {
            summary_id: cfg.summary.id,
            summary_name: cfg.summary.name,
            source,
            lenses,
        })
    }

    /// Resolve direct child lenses for a given parent lens id (unscoped/default summary only).
    pub async fn resolve_brain_map_lens_children(
        &self,
        parent_lens_id: String,
    ) -> Result<Vec<brain_map::LensResolution>, AxonMindError> {
        let snapshot = self.get_brain_map_default_config().await?;
        let cfg = snapshot.config;
        let contexts = snapshot.effective_contexts;
        let export = self.export_json().await?;

        let mut nodes_by_id = std::collections::HashMap::<String, &Node>::new();
        for n in &export.nodes {
            nodes_by_id.insert(n.id.0.clone(), n);
        }
        let mut metric_values_by_kpi =
            std::collections::HashMap::<String, Vec<&store::MetricValue>>::new();
        for mv in &export.metric_values {
            metric_values_by_kpi
                .entry(mv.kpi_node_id.0.clone())
                .or_default()
                .push(mv);
        }
        let evidence_by_id: std::collections::HashMap<String, &axonmind_core::Evidence> = export
            .evidence
            .iter()
            .map(|e| (e.id.0.clone(), e))
            .collect();

        let context_by_lens: std::collections::HashMap<String, brain_map::EffectiveLensContext> =
            contexts
                .into_iter()
                .map(|c| (c.lens_id.clone(), c))
                .collect();
        let lens_by_id: std::collections::HashMap<String, &brain_map::LensDefinition> =
            cfg.lenses.iter().map(|l| (l.id.clone(), l)).collect();

        let parent =
            lens_by_id
                .get(&parent_lens_id)
                .ok_or_else(|| AxonMindError::ValidationFailed {
                    message: format!("parent lens '{}' not found", parent_lens_id),
                })?;

        Ok(resolve_lens_ids(
            &parent.children,
            &lens_by_id,
            &context_by_lens,
            &export.nodes,
            &export.edges,
            &nodes_by_id,
            &metric_values_by_kpi,
            &evidence_by_id,
        ))
    }

    pub async fn export_json(&self) -> Result<query::GraphExportV1, AxonMindError> {
        use crate::config::WorkspaceManifest;
        let workspace_id =
            tokio::fs::read_to_string(self.config.workspace_dir.join("workspace.json"))
                .await
                .ok()
                .and_then(|s| serde_json::from_str::<WorkspaceManifest>(&s).ok())
                .map(|m| m.id)
                .unwrap_or_default();

        let (nodes, edges, evidence, edge_evidence, metric_values, kpi_candidates) =
            self.store.fetch_export().await?;

        Ok(query::GraphExportV1 {
            schema_version: 1,
            exported_at: chrono::Utc::now(),
            workspace_id,
            nodes,
            edges,
            evidence,
            edge_evidence,
            metric_values,
            kpi_candidates,
        })
    }

    /// Apply a single mutation. Exposed for CLI `import-json`.
    pub async fn apply_mutation(&self, m: GraphMutation) -> Result<(), AxonMindError> {
        self.store
            .apply_mutation(m, &self.graph_cache, &self.event_tx)
            .await
    }

    /// Rebuild the FTS5 search index from scratch. CLI command: `rebuild-search-index`.
    pub async fn rebuild_search_index(&self) -> Result<(), AxonMindError> {
        let conn = self
            .store
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(|conn| store::sqlite::rebuild_full_fts(conn))
            .await
            .map_err(|e| AxonMindError::Database(format!("interact: {e}")))??;
        Ok(())
    }

    /// Walk `document_cache`, re-parse each blob, and repopulate the `page_*` tables without
    /// touching graph tables. Skips documents whose page-index sha already matches.
    /// Returns (processed, skipped, errors).
    pub async fn rebuild_page_index(&self) -> Result<(usize, usize, Vec<String>), AxonMindError> {
        let docs = self.store.list_document_summaries().await?;
        let page_store = PageIndexStore::new(self.store.db.0.clone());
        let llm = self.llm_provider.read().await.clone();

        let mut processed = 0usize;
        let mut skipped = 0usize;
        let mut errors: Vec<String> = Vec::new();

        for doc in docs {
            let (sha, src) = match (doc.sha256, doc.source_path) {
                (Some(s), Some(p)) => (s, p),
                _ => {
                    skipped += 1;
                    continue;
                }
            };

            let blob_path = self.config.blob_dir.join(&sha);
            let bytes = match tokio::fs::read(&blob_path).await {
                Ok(b) => b,
                Err(e) => {
                    errors.push(format!("{}: blob read failed: {e}", doc.node_id));
                    continue;
                }
            };

            let path = std::path::PathBuf::from(&src);
            let normalized = match dispatch_parse(&path, &bytes) {
                Ok(d) => d,
                Err(e) => {
                    errors.push(format!("{}: parse failed: {e}", doc.node_id));
                    continue;
                }
            };

            match pageindex::index_document(
                &normalized,
                &doc.node_id,
                &page_store,
                llm.as_deref(),
                &self.config,
            )
            .await
            {
                Ok(()) => processed += 1,
                Err(e) => errors.push(format!("{}: index failed: {e}", doc.node_id)),
            }
        }

        Ok((processed, skipped, errors))
    }
}

fn resolve_lens_measure(
    measure: &serde_json::Value,
    selected_node_ids: &[String],
    nodes_by_id: &std::collections::HashMap<String, &Node>,
    export_edges: &[axonmind_core::Edge],
    metric_values_by_kpi: &std::collections::HashMap<String, Vec<&store::MetricValue>>,
    evidence_by_id: &std::collections::HashMap<String, &axonmind_core::Evidence>,
) -> brain_map::MeasureResolution {
    let measure_type = measure
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    match measure_type.as_str() {
        "count" => {
            let scoped =
                match apply_temporal_scope_to_nodes(measure, selected_node_ids, nodes_by_id) {
                    Ok(v) => v,
                    Err(msg) => {
                        return brain_map::MeasureResolution {
                            measure_type,
                            state: brain_map::MeasureState::Unknown,
                            value: None,
                            unit: Some("count".to_string()),
                            confidence: None,
                            observed_at: None,
                            explanation: Some(msg),
                            evidence_ids: vec![],
                            evidence_lineage: vec![],
                            lineage_gaps: vec![lineage_gap(
                                "invalid_as_of",
                                "cannot build lineage because the as_of value is invalid",
                            )],
                            supporting_nodes: vec![],
                        };
                    }
                };
            let (evidence_ids, evidence_lineage) =
                collect_member_evidence(&scoped.ids, export_edges, evidence_by_id, nodes_by_id, 24);
            let mut lineage_gaps = Vec::new();
            if evidence_ids.is_empty() {
                lineage_gaps.push(lineage_gap(
                    "aggregate_not_evidence_backed",
                    "count is aggregate lens math; no per-member source evidence could be resolved in current scope",
                ));
            }
            if scoped.missing_timestamp > 0 {
                lineage_gaps.push(lineage_gap(
                    "temporal_scope_partial",
                    "one or more members lacked timestamps and were excluded by period/as_of filtering",
                ));
            }
            brain_map::MeasureResolution {
                measure_type,
                state: brain_map::MeasureState::Resolved,
                value: Some(scoped.ids.len() as f64),
                unit: Some("count".to_string()),
                confidence: None,
                observed_at: None,
                explanation: scoped.explanation,
                evidence_ids,
                evidence_lineage,
                lineage_gaps,
                supporting_nodes: scoped
                    .ids
                    .iter()
                    .take(20)
                    .filter_map(|id| {
                        nodes_by_id
                            .get(id)
                            .map(|n| support_node_ref(n, "member_counted"))
                    })
                    .collect(),
            }
        }
        "sum" => resolve_sum_measure(
            measure,
            selected_node_ids,
            nodes_by_id,
            export_edges,
            evidence_by_id,
        ),
        "ref_kpi" => {
            resolve_ref_kpi_measure(measure, nodes_by_id, metric_values_by_kpi, evidence_by_id)
        }
        "derived" => resolve_derived_measure(
            measure,
            nodes_by_id,
            export_edges,
            metric_values_by_kpi,
            evidence_by_id,
        ),
        other => brain_map::MeasureResolution {
            measure_type: if other.is_empty() {
                "unknown".to_string()
            } else {
                other.to_string()
            },
            state: brain_map::MeasureState::Unknown,
            value: None,
            unit: None,
            confidence: None,
            observed_at: None,
            explanation: Some("unsupported measure type in v1 resolver".to_string()),
            evidence_ids: vec![],
            evidence_lineage: vec![],
            lineage_gaps: vec![lineage_gap(
                "unsupported_measure_lineage",
                "lineage unavailable because the measure type is not supported by the v1 resolver",
            )],
            supporting_nodes: vec![],
        },
    }
}

fn resolve_lens_ids(
    lens_ids: &[String],
    lens_by_id: &std::collections::HashMap<String, &brain_map::LensDefinition>,
    context_by_lens: &std::collections::HashMap<String, brain_map::EffectiveLensContext>,
    export_nodes: &[Node],
    export_edges: &[axonmind_core::Edge],
    nodes_by_id: &std::collections::HashMap<String, &Node>,
    metric_values_by_kpi: &std::collections::HashMap<String, Vec<&store::MetricValue>>,
    evidence_by_id: &std::collections::HashMap<String, &axonmind_core::Evidence>,
) -> Vec<brain_map::LensResolution> {
    let mut out = Vec::new();
    for lens_id in lens_ids {
        let Some(lens) = lens_by_id.get(lens_id) else {
            continue;
        };
        if brain_map::is_hidden_lens(lens) {
            continue;
        }
        let Some(context) = context_by_lens.get(lens_id) else {
            continue;
        };

        let selected_node_ids = brain_map::filter_node_ids_for_selector_with_edges(
            export_nodes,
            export_edges,
            &context.effective_selector,
        );
        let mut measure = resolve_lens_measure(
            &lens.measure,
            &selected_node_ids,
            nodes_by_id,
            export_edges,
            metric_values_by_kpi,
            evidence_by_id,
        );
        enrich_with_facet_membership_evidence(
            &mut measure,
            &selected_node_ids,
            &context.effective_selector,
            export_edges,
            evidence_by_id,
            nodes_by_id,
        );
        let health = evaluate_lens_health(&measure, lens.health.as_ref());

        out.push(brain_map::LensResolution {
            lens_id: lens.id.clone(),
            label: lens.label.clone(),
            child_lens_ids: lens
                .children
                .iter()
                .filter(|child_id| {
                    lens_by_id
                        .get((*child_id).as_str())
                        .map(|l| !brain_map::is_hidden_lens(l))
                        .unwrap_or(false)
                })
                .cloned()
                .collect(),
            selected_node_ids,
            effective_context: context.clone(),
            measure_rule: lens.measure.clone(),
            measure,
            health,
        });
    }
    out
}

fn resolve_sum_measure(
    measure: &serde_json::Value,
    selected_node_ids: &[String],
    nodes_by_id: &std::collections::HashMap<String, &Node>,
    export_edges: &[axonmind_core::Edge],
    evidence_by_id: &std::collections::HashMap<String, &axonmind_core::Evidence>,
) -> brain_map::MeasureResolution {
    let measure_type = "sum".to_string();
    let field = measure
        .get("field")
        .or_else(|| measure.get("attr"))
        .and_then(|v| v.as_str());
    let Some(field) = field else {
        return brain_map::MeasureResolution {
            measure_type,
            state: brain_map::MeasureState::Unknown,
            value: None,
            unit: None,
            confidence: None,
            observed_at: None,
            explanation: Some("sum measure requires 'field'".to_string()),
            evidence_ids: vec![],
            evidence_lineage: vec![],
            lineage_gaps: vec![lineage_gap(
                "missing_sum_field",
                "sum lineage unavailable because the measure rule is missing the numeric field",
            )],
            supporting_nodes: vec![],
        };
    };

    let scoped = match apply_temporal_scope_to_nodes(measure, selected_node_ids, nodes_by_id) {
        Ok(v) => v,
        Err(msg) => {
            return brain_map::MeasureResolution {
                measure_type,
                state: brain_map::MeasureState::Unknown,
                value: None,
                unit: None,
                confidence: None,
                observed_at: None,
                explanation: Some(msg),
                evidence_ids: vec![],
                evidence_lineage: vec![],
                lineage_gaps: vec![lineage_gap(
                    "invalid_as_of",
                    "cannot build lineage because the as_of value is invalid",
                )],
                supporting_nodes: vec![],
            };
        }
    };

    let mut total = 0.0f64;
    let mut found = 0usize;
    let mut supporting_nodes = Vec::new();
    for id in &scoped.ids {
        let Some(node) = nodes_by_id.get(id) else {
            continue;
        };
        if let Some(v) = numeric_attr(&node.attrs, field) {
            total += v;
            found += 1;
            if supporting_nodes.len() < 20 {
                supporting_nodes.push(support_node_ref(node, "sum_input"));
            }
        }
    }

    if found == 0 {
        return brain_map::MeasureResolution {
            measure_type,
            state: brain_map::MeasureState::Unknown,
            value: None,
            unit: None,
            confidence: None,
            observed_at: None,
            explanation: Some(format!(
                "no numeric values found for field '{}' in selected scope",
                field
            )),
            evidence_ids: vec![],
            evidence_lineage: vec![],
            lineage_gaps: vec![
                lineage_gap(
                    "sum_no_numeric_values",
                    "sum lineage unavailable because no numeric values were resolved in the selected scope",
                ),
                lineage_gap(
                    "aggregate_not_evidence_backed",
                    "sum could not map any numeric members to source evidence in current scope",
                ),
            ],
            supporting_nodes: vec![],
        };
    }

    let (evidence_ids, evidence_lineage) =
        collect_member_evidence(&scoped.ids, export_edges, evidence_by_id, nodes_by_id, 32);
    let mut lineage_gaps = Vec::new();
    if evidence_ids.is_empty() {
        lineage_gaps.push(lineage_gap(
            "aggregate_not_evidence_backed",
            "sum resolved as aggregate math; no per-input source evidence was found in current scope",
        ));
    }
    if scoped.missing_timestamp > 0 {
        lineage_gaps.push(lineage_gap(
            "temporal_scope_partial",
            "one or more members lacked timestamps and were excluded by period/as_of filtering",
        ));
    }

    brain_map::MeasureResolution {
        measure_type,
        state: brain_map::MeasureState::Resolved,
        value: Some(total),
        unit: measure
            .get("unit")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned),
        confidence: None,
        observed_at: None,
        explanation: scoped.explanation,
        evidence_ids,
        evidence_lineage,
        lineage_gaps,
        supporting_nodes,
    }
}

fn resolve_ref_kpi_measure(
    measure: &serde_json::Value,
    nodes_by_id: &std::collections::HashMap<String, &Node>,
    metric_values_by_kpi: &std::collections::HashMap<String, Vec<&store::MetricValue>>,
    evidence_by_id: &std::collections::HashMap<String, &axonmind_core::Evidence>,
) -> brain_map::MeasureResolution {
    let measure_type = "ref_kpi".to_string();
    let Some(kpi_key) = measure.get("kpi").and_then(|v| v.as_str()) else {
        return brain_map::MeasureResolution {
            measure_type,
            state: brain_map::MeasureState::Unknown,
            value: None,
            unit: None,
            confidence: None,
            observed_at: None,
            explanation: Some("ref_kpi measure requires 'kpi'".to_string()),
            evidence_ids: vec![],
            evidence_lineage: vec![],
            lineage_gaps: vec![lineage_gap(
                "missing_ref_kpi",
                "no KPI reference configured for ref_kpi measure",
            )],
            supporting_nodes: vec![],
        };
    };

    let Some(kpi_node) = find_kpi_node(kpi_key, nodes_by_id) else {
        return brain_map::MeasureResolution {
            measure_type,
            state: brain_map::MeasureState::Unknown,
            value: None,
            unit: None,
            confidence: None,
            observed_at: None,
            explanation: Some(format!("KPI '{}' not found", kpi_key)),
            evidence_ids: vec![],
            evidence_lineage: vec![],
            lineage_gaps: vec![lineage_gap(
                "kpi_not_found",
                "cannot build lineage because the referenced KPI node was not found",
            )],
            supporting_nodes: vec![],
        };
    };

    let period = measure
        .get("period")
        .and_then(|v| v.as_str())
        .unwrap_or("latest");
    let as_of = measure
        .get("as_of")
        .and_then(|v| v.as_str())
        .unwrap_or("latest");
    let cutoff = parse_as_of_cutoff(as_of);
    if as_of != "latest" && cutoff.is_none() {
        return brain_map::MeasureResolution {
            measure_type,
            state: brain_map::MeasureState::Unknown,
            value: None,
            unit: None,
            confidence: None,
            observed_at: None,
            explanation: Some(format!(
                "invalid as_of '{}' (expected 'latest', YYYY-MM-DD, or RFC3339 timestamp)",
                as_of
            )),
            evidence_ids: vec![],
            evidence_lineage: vec![],
            lineage_gaps: vec![lineage_gap(
                "invalid_as_of",
                "cannot build lineage because the as_of value is invalid",
            )],
            supporting_nodes: vec![],
        };
    }

    if let Some(values) = metric_values_by_kpi.get(&kpi_node.id.0) {
        let anchor = cutoff.unwrap_or_else(chrono::Utc::now);
        let filtered_values = values
            .iter()
            .copied()
            .filter(|mv| {
                cutoff
                    .map(|c| metric_business_timestamp(mv) <= c)
                    .unwrap_or(true)
            })
            .filter(|mv| match period {
                "latest" => true,
                "MTD" => {
                    let ts = metric_business_timestamp(mv);
                    ts.year() == anchor.year() && ts.month() == anchor.month()
                }
                "YTD" => metric_business_timestamp(mv).year() == anchor.year(),
                _ => true,
            })
            .collect::<Vec<_>>();
        let latest = filtered_values
            .iter()
            .copied()
            .max_by_key(|mv| (metric_business_timestamp(mv), mv.observed_at));
        let conflict_detected = detect_metric_value_conflict(&filtered_values);

        if let Some(mv) = latest {
            let mut lineage_gaps = if evidence_by_id.contains_key(&mv.evidence_id.0) {
                vec![]
            } else {
                vec![lineage_gap(
                    "evidence_record_missing",
                    "metric value points to an evidence id that is not present in the export payload",
                )]
            };
            if conflict_detected {
                lineage_gaps.push(lineage_gap(
                    "conflicting_observations",
                    "multiple observations for the same timestamp have conflicting values",
                ));
            }
            let mut supporting_nodes = vec![support_node_ref(kpi_node, "kpi_target")];
            if let Some(metric_node) = nodes_by_id.get(&mv.metric_node_id.0) {
                supporting_nodes.push(support_node_ref(metric_node, "metric_observation"));
            }
            if let Some(source_node) = evidence_by_id
                .get(&mv.evidence_id.0)
                .and_then(|e| nodes_by_id.get(&e.source_node_id.0).copied())
            {
                supporting_nodes.push(support_node_ref(source_node, "evidence_source"));
            }
            return brain_map::MeasureResolution {
                measure_type,
                state: brain_map::MeasureState::Resolved,
                value: Some(mv.value),
                unit: Some(mv.unit.clone()),
                confidence: evidence_by_id
                    .get(&mv.evidence_id.0)
                    .map(|e| e.confidence.0),
                observed_at: Some(metric_business_timestamp(mv).to_rfc3339()),
                explanation: None,
                evidence_ids: vec![mv.evidence_id.0.clone()],
                evidence_lineage: evidence_by_id
                    .get(&mv.evidence_id.0)
                    .map(|e| vec![lineage_from_evidence(e, nodes_by_id)])
                    .unwrap_or_default(),
                lineage_gaps,
                supporting_nodes,
            };
        }
    }

    if kpi_node.kind == NodeKind::Kpi {
        if let Ok(attrs) = serde_json::from_value::<KpiAttrs>(kpi_node.attrs.clone()) {
            if let Some(v) = attrs.value {
                return brain_map::MeasureResolution {
                    measure_type,
                    state: brain_map::MeasureState::Resolved,
                    value: Some(v),
                    unit: Some(kpi_unit_to_string(attrs.unit)),
                    confidence: Some(kpi_node.confidence.0),
                    observed_at: None,
                    explanation: Some(
                        "resolved from KPI attrs (no metric_values row found)".to_string(),
                    ),
                    evidence_ids: vec![],
                    evidence_lineage: vec![],
                    lineage_gaps: vec![lineage_gap(
                        "resolved_from_attrs_without_observation",
                        "value came from KPI attrs fallback; observed metric-value lineage is unavailable",
                    )],
                    supporting_nodes: vec![support_node_ref(kpi_node, "kpi_target")],
                };
            }
        }
    }

    brain_map::MeasureResolution {
        measure_type,
        state: brain_map::MeasureState::Unknown,
        value: None,
        unit: None,
        confidence: None,
        observed_at: None,
        explanation: Some(format!(
            "KPI '{}' exists but has no resolvable value",
            kpi_key
        )),
        evidence_ids: vec![],
        evidence_lineage: vec![],
        lineage_gaps: vec![lineage_gap(
            "no_resolvable_kpi_value",
            "KPI exists but no observed value/evidence could be resolved for the requested period/as_of",
        )],
        supporting_nodes: vec![support_node_ref(kpi_node, "kpi_target")],
    }
}

fn resolve_derived_measure(
    measure: &serde_json::Value,
    nodes_by_id: &std::collections::HashMap<String, &Node>,
    export_edges: &[axonmind_core::Edge],
    metric_values_by_kpi: &std::collections::HashMap<String, Vec<&store::MetricValue>>,
    evidence_by_id: &std::collections::HashMap<String, &axonmind_core::Evidence>,
) -> brain_map::MeasureResolution {
    let measure_type = "derived".to_string();
    let kpi_key = measure.get("kpi").and_then(|v| v.as_str());
    let period = measure
        .get("period")
        .and_then(|v| v.as_str())
        .unwrap_or("latest");
    let as_of = measure
        .get("as_of")
        .and_then(|v| v.as_str())
        .unwrap_or("latest");

    let target_kpi = kpi_key.and_then(|k| find_kpi_node(k, nodes_by_id));
    let input_keys = derived_input_keys(measure, target_kpi, nodes_by_id, export_edges);
    if input_keys.is_empty() {
        return brain_map::MeasureResolution {
            measure_type,
            state: brain_map::MeasureState::Unknown,
            value: None,
            unit: None,
            confidence: None,
            observed_at: None,
            explanation: Some(
                "derived measure requires explicit inputs or a derived_from graph path from target KPI"
                    .to_string(),
            ),
            evidence_ids: vec![],
            evidence_lineage: vec![],
            lineage_gaps: vec![lineage_gap(
                "missing_derived_inputs",
                "no derived inputs were configured and no derived_from input edges were found",
            )],
            supporting_nodes: target_kpi
                .map(|n| vec![support_node_ref(n, "derived_target")])
                .unwrap_or_default(),
        };
    }

    let mut unresolved_inputs = Vec::<String>::new();
    let mut propagated_confidences = Vec::<f32>::new();
    let mut merged_evidence_ids = std::collections::HashSet::<String>::new();
    let mut evidence_ids = Vec::<String>::new();
    let mut evidence_lineage = Vec::<brain_map::EvidenceLineageItem>::new();
    let mut lineage_gaps = Vec::<brain_map::LineageGap>::new();
    let mut supporting_nodes = Vec::<brain_map::SupportingNodeRef>::new();

    if let Some(target) = target_kpi {
        supporting_nodes.push(support_node_ref(target, "derived_target"));
    }

    for input_key in &input_keys {
        let input_measure = serde_json::json!({
            "type": "ref_kpi",
            "kpi": input_key,
            "period": period,
            "as_of": as_of,
        });
        let input_res = resolve_ref_kpi_measure(
            &input_measure,
            nodes_by_id,
            metric_values_by_kpi,
            evidence_by_id,
        );

        if matches!(input_res.state, brain_map::MeasureState::Unknown) {
            unresolved_inputs.push(input_key.clone());
            lineage_gaps.extend(input_res.lineage_gaps);
        } else if let Some(c) = input_res.confidence {
            propagated_confidences.push(c);
        }

        if let Some(input_node) = find_kpi_node(input_key, nodes_by_id) {
            supporting_nodes.push(support_node_ref(input_node, "derived_input"));
        }

        for eid in input_res.evidence_ids {
            if merged_evidence_ids.insert(eid.clone()) {
                evidence_ids.push(eid);
            }
        }
        for item in input_res.evidence_lineage {
            if !evidence_lineage
                .iter()
                .any(|x| x.evidence_id == item.evidence_id)
            {
                evidence_lineage.push(item);
            }
        }
    }

    if !unresolved_inputs.is_empty() {
        unresolved_inputs.sort();
        unresolved_inputs.dedup();
        let listed = unresolved_inputs
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if unresolved_inputs.len() > 4 {
            ", …"
        } else {
            ""
        };
        lineage_gaps.push(lineage_gap(
            "derived_input_missing_or_unknown",
            "one or more derived inputs could not be resolved for the requested period/as_of",
        ));
        return brain_map::MeasureResolution {
            measure_type,
            state: brain_map::MeasureState::Unknown,
            value: None,
            unit: None,
            confidence: propagated_confidences.into_iter().reduce(f32::min),
            observed_at: None,
            explanation: Some(format!("derived inputs unresolved: {listed}{suffix}")),
            evidence_ids,
            evidence_lineage,
            lineage_gaps,
            supporting_nodes,
        };
    }

    let mut out = if let Some(kpi_key) = kpi_key {
        let target_measure = serde_json::json!({
            "type": "ref_kpi",
            "kpi": kpi_key,
            "period": period,
            "as_of": as_of,
        });
        resolve_ref_kpi_measure(
            &target_measure,
            nodes_by_id,
            metric_values_by_kpi,
            evidence_by_id,
        )
    } else {
        brain_map::MeasureResolution {
            measure_type: "derived".to_string(),
            state: brain_map::MeasureState::Unknown,
            value: None,
            unit: None,
            confidence: None,
            observed_at: None,
            explanation: Some("derived measure requires target KPI in open-repo v1".to_string()),
            evidence_ids: vec![],
            evidence_lineage: vec![],
            lineage_gaps: vec![lineage_gap(
                "missing_derived_target",
                "cannot resolve derived headline value because no target KPI was configured",
            )],
            supporting_nodes: vec![],
        }
    };

    out.measure_type = "derived".to_string();
    out.confidence = propagated_confidences
        .into_iter()
        .reduce(f32::min)
        .or(out.confidence);

    for eid in out.evidence_ids.clone() {
        merged_evidence_ids.insert(eid);
    }
    for eid in evidence_ids {
        if merged_evidence_ids.insert(eid.clone()) {
            out.evidence_ids.push(eid);
        }
    }
    for item in evidence_lineage {
        if !out
            .evidence_lineage
            .iter()
            .any(|x| x.evidence_id == item.evidence_id)
        {
            out.evidence_lineage.push(item);
        }
    }
    out.lineage_gaps.extend(lineage_gaps);
    for node in supporting_nodes {
        if !out
            .supporting_nodes
            .iter()
            .any(|x| x.node_id == node.node_id && x.role == node.role)
        {
            out.supporting_nodes.push(node);
        }
    }

    out
}

fn derived_input_keys(
    measure: &serde_json::Value,
    target_kpi: Option<&Node>,
    nodes_by_id: &std::collections::HashMap<String, &Node>,
    export_edges: &[axonmind_core::Edge],
) -> Vec<String> {
    let from_rule = measure
        .get("inputs")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !from_rule.is_empty() {
        return from_rule;
    }

    let Some(target) = target_kpi else {
        return vec![];
    };
    let mut out = Vec::<String>::new();
    for edge in export_edges {
        if edge.kind != EdgeKind::DerivedFrom || edge.from != target.id {
            continue;
        }
        if let Some(input_node) = nodes_by_id.get(&edge.to.0) {
            out.push(input_node.id.0.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn parse_as_of_cutoff(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if raw.eq_ignore_ascii_case("latest") {
        return None;
    }
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(ts.with_timezone(&chrono::Utc));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let dt = date.and_hms_opt(23, 59, 59)?;
        return Some(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
            dt,
            chrono::Utc,
        ));
    }
    None
}

fn find_kpi_node<'a>(
    kpi_key: &str,
    nodes_by_id: &'a std::collections::HashMap<String, &Node>,
) -> Option<&'a Node> {
    if let Some(node) = nodes_by_id.get(kpi_key) {
        return Some(node);
    }

    let slug = kpi_key
        .to_ascii_lowercase()
        .replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    let canonical = format!("kpi.{}", slug.trim_matches('_'));
    if let Some(node) = nodes_by_id.get(&canonical) {
        return Some(node);
    }

    nodes_by_id.values().copied().find(|n| {
        n.kind == NodeKind::Kpi
            && (n.id.0.eq_ignore_ascii_case(kpi_key) || n.name.eq_ignore_ascii_case(kpi_key))
    })
}

fn kpi_unit_to_string(unit: KpiUnit) -> String {
    match unit {
        KpiUnit::Percent => "percent".to_string(),
        KpiUnit::Currency(code) => code,
        KpiUnit::Count => "count".to_string(),
        KpiUnit::Ratio => "ratio".to_string(),
        KpiUnit::Duration => "duration".to_string(),
        KpiUnit::Custom(v) => v,
    }
}

fn lineage_gap(code: &str, message: &str) -> brain_map::LineageGap {
    brain_map::LineageGap {
        code: code.to_string(),
        message: message.to_string(),
    }
}

fn support_node_ref(node: &Node, role: &str) -> brain_map::SupportingNodeRef {
    brain_map::SupportingNodeRef {
        node_id: node.id.0.clone(),
        label: node.name.clone(),
        kind: format!("{:?}", node.kind),
        role: role.to_string(),
    }
}

fn lineage_from_evidence(
    evidence: &axonmind_core::Evidence,
    nodes_by_id: &std::collections::HashMap<String, &Node>,
) -> brain_map::EvidenceLineageItem {
    let source_node_name = nodes_by_id
        .get(&evidence.source_node_id.0)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| evidence.source_node_id.0.clone());
    let source_path = nodes_by_id.get(&evidence.source_node_id.0).and_then(|n| {
        n.attrs
            .get("source_path")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
    });

    brain_map::EvidenceLineageItem {
        evidence_id: evidence.id.0.clone(),
        source_node_id: evidence.source_node_id.0.clone(),
        source_node_name,
        source_type: format!("{:?}", evidence.source_type),
        source_path,
        row_ref: evidence.row_ref.clone(),
        quote: evidence.quote.clone(),
        timestamp: evidence.timestamp.as_ref().map(|ts| ts.to_rfc3339()),
    }
}

fn numeric_attr(attrs: &serde_json::Value, field: &str) -> Option<f64> {
    let mut cur = attrs;
    for part in field.split('.') {
        cur = cur.get(part)?;
    }
    match cur {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

struct TemporalScopeResult {
    ids: Vec<String>,
    missing_timestamp: usize,
    explanation: Option<String>,
}

fn apply_temporal_scope_to_nodes(
    measure: &serde_json::Value,
    selected_node_ids: &[String],
    nodes_by_id: &std::collections::HashMap<String, &Node>,
) -> Result<TemporalScopeResult, String> {
    let period = measure
        .get("period")
        .and_then(|v| v.as_str())
        .unwrap_or("latest");
    let as_of = measure
        .get("as_of")
        .and_then(|v| v.as_str())
        .unwrap_or("latest");
    let cutoff = parse_as_of_cutoff(as_of);
    if as_of != "latest" && cutoff.is_none() {
        return Err(format!(
            "invalid as_of '{}' (expected 'latest', YYYY-MM-DD, or RFC3339 timestamp)",
            as_of
        ));
    }
    let anchor = cutoff.unwrap_or_else(chrono::Utc::now);

    let needs_temporal_filter = period != "latest" || as_of != "latest";
    if !needs_temporal_filter {
        return Ok(TemporalScopeResult {
            ids: selected_node_ids.to_vec(),
            missing_timestamp: 0,
            explanation: None,
        });
    }

    let mut kept = Vec::<String>::new();
    let mut missing_timestamp = 0usize;
    for id in selected_node_ids {
        let Some(node) = nodes_by_id.get(id) else {
            continue;
        };
        let Some(ts) = node_temporal_timestamp(node) else {
            missing_timestamp += 1;
            continue;
        };
        if cutoff.map(|c| ts > c).unwrap_or(false) {
            continue;
        }
        let in_period = match period {
            "latest" => true,
            "MTD" => ts.year() == anchor.year() && ts.month() == anchor.month(),
            "YTD" => ts.year() == anchor.year(),
            _ => true,
        };
        if in_period {
            kept.push(id.clone());
        }
    }

    let explanation = Some(format!(
        "temporal scope applied: period={} as_of={} (kept {} of {} members)",
        period,
        as_of,
        kept.len(),
        selected_node_ids.len()
    ));

    Ok(TemporalScopeResult {
        ids: kept,
        missing_timestamp,
        explanation,
    })
}

fn node_temporal_timestamp(node: &Node) -> Option<chrono::DateTime<chrono::Utc>> {
    node_timestamp_from_attrs(&node.attrs)
}

fn node_timestamp_from_attrs(attrs: &serde_json::Value) -> Option<chrono::DateTime<chrono::Utc>> {
    for key in ["as_of", "observed_at", "timestamp", "date", "updated_at"] {
        let Some(value) = attrs.get(key) else {
            continue;
        };
        if let Some(ts) = parse_attr_timestamp_value(value) {
            return Some(ts);
        }
    }
    None
}

fn parse_attr_timestamp_value(value: &serde_json::Value) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Some(s) = value.as_str() {
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(s) {
            return Some(ts.with_timezone(&chrono::Utc));
        }
        if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            let dt = date.and_hms_opt(23, 59, 59)?;
            return Some(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
                dt,
                chrono::Utc,
            ));
        }
    }
    if let Some(epoch) = value.as_i64() {
        return chrono::DateTime::from_timestamp(epoch, 0);
    }
    None
}

fn collect_member_evidence(
    selected_node_ids: &[String],
    edges: &[axonmind_core::Edge],
    evidence_by_id: &std::collections::HashMap<String, &axonmind_core::Evidence>,
    nodes_by_id: &std::collections::HashMap<String, &Node>,
    limit: usize,
) -> (Vec<String>, Vec<brain_map::EvidenceLineageItem>) {
    let selected: std::collections::HashSet<&str> =
        selected_node_ids.iter().map(String::as_str).collect();
    let mut evidence_ids = Vec::<String>::new();
    let mut evidence_lineage = Vec::<brain_map::EvidenceLineageItem>::new();
    let mut seen = std::collections::HashSet::<String>::new();

    for edge in edges {
        if edge.kind != EdgeKind::MentionedIn {
            continue;
        }
        if !selected.contains(edge.from.0.as_str()) && !selected.contains(edge.to.0.as_str()) {
            continue;
        }
        for eid in &edge.evidence {
            if !seen.insert(eid.0.clone()) {
                continue;
            }
            evidence_ids.push(eid.0.clone());
            if let Some(ev) = evidence_by_id.get(&eid.0) {
                evidence_lineage.push(lineage_from_evidence(ev, nodes_by_id));
            }
            if evidence_ids.len() >= limit {
                return (evidence_ids, evidence_lineage);
            }
        }
    }

    (evidence_ids, evidence_lineage)
}

fn detect_metric_value_conflict(values: &[&store::MetricValue]) -> bool {
    for i in 0..values.len() {
        for j in (i + 1)..values.len() {
            let left = values[i];
            let right = values[j];
            if metric_business_timestamp(left) == metric_business_timestamp(right)
                && left.unit.eq_ignore_ascii_case(&right.unit)
                && (left.value - right.value).abs() > f64::EPSILON
            {
                return true;
            }
        }
    }
    false
}

fn metric_business_timestamp(mv: &store::MetricValue) -> chrono::DateTime<chrono::Utc> {
    mv.as_of.unwrap_or(mv.observed_at)
}

fn enrich_with_facet_membership_evidence(
    measure: &mut brain_map::MeasureResolution,
    selected_node_ids: &[String],
    selector: &serde_json::Value,
    edges: &[axonmind_core::Edge],
    evidence_by_id: &std::collections::HashMap<String, &axonmind_core::Evidence>,
    nodes_by_id: &std::collections::HashMap<String, &Node>,
) {
    if !selector_contains_facet(selector) || selected_node_ids.is_empty() {
        return;
    }

    let selected: std::collections::HashSet<&str> =
        selected_node_ids.iter().map(String::as_str).collect();
    let mut seen_ids: std::collections::HashSet<String> =
        measure.evidence_ids.iter().cloned().collect();

    for edge in edges {
        if !matches!(edge.kind, EdgeKind::InFunction | EdgeKind::ForProduct) {
            continue;
        }
        if !selected.contains(edge.from.0.as_str()) && !selected.contains(edge.to.0.as_str()) {
            continue;
        }
        for eid in &edge.evidence {
            if seen_ids.insert(eid.0.clone()) {
                measure.evidence_ids.push(eid.0.clone());
            }
            if let Some(ev) = evidence_by_id.get(&eid.0) {
                let lineage = lineage_from_evidence(ev, nodes_by_id);
                if !measure
                    .evidence_lineage
                    .iter()
                    .any(|x| x.evidence_id == lineage.evidence_id)
                {
                    measure.evidence_lineage.push(lineage);
                }
            }
        }
    }
}

fn selector_contains_facet(selector: &serde_json::Value) -> bool {
    let Some(obj) = selector.as_object() else {
        return false;
    };
    if obj.get("type").and_then(|v| v.as_str()) == Some("facet") {
        return true;
    }
    obj.get("and")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(selector_contains_facet))
        .unwrap_or(false)
        || obj
            .get("or")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(selector_contains_facet))
            .unwrap_or(false)
}

fn evaluate_lens_health(
    measure: &brain_map::MeasureResolution,
    health_rule: Option<&serde_json::Value>,
) -> brain_map::HealthResolution {
    if matches!(measure.state, brain_map::MeasureState::Unknown) {
        return brain_map::HealthResolution {
            state: brain_map::HealthState::Unknown,
            explanation: measure
                .explanation
                .clone()
                .or_else(|| Some("measure is unknown".to_string())),
        };
    }

    let Some(rule) = health_rule else {
        return brain_map::HealthResolution {
            state: brain_map::HealthState::Unknown,
            explanation: Some("no health rule configured".to_string()),
        };
    };
    let health_type = rule
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    if let Some(reason) = evaluate_health_unknown_reason(measure, rule) {
        return brain_map::HealthResolution {
            state: brain_map::HealthState::Unknown,
            explanation: Some(reason),
        };
    }

    match health_type {
        "presence" => brain_map::HealthResolution {
            state: brain_map::HealthState::Good,
            explanation: None,
        },
        "threshold" => {
            let value = measure.value;
            let Some(v) = value else {
                return brain_map::HealthResolution {
                    state: brain_map::HealthState::Unknown,
                    explanation: Some("threshold health requires a numeric value".to_string()),
                };
            };

            let green_lt = rule.get("green_lt").and_then(|x| x.as_f64());
            let watch_lt = rule.get("watch_lt").and_then(|x| x.as_f64());
            let red_gte = rule.get("red_gte").and_then(|x| x.as_f64());

            if let Some(g) = green_lt {
                if v < g {
                    return brain_map::HealthResolution {
                        state: brain_map::HealthState::Good,
                        explanation: Some(format!(
                            "good: value {} is below green threshold {}",
                            v, g
                        )),
                    };
                }
            }
            if let Some(w) = watch_lt {
                if v < w {
                    return brain_map::HealthResolution {
                        state: brain_map::HealthState::Watch,
                        explanation: Some(format!(
                            "watch: value {} is below watch threshold {}",
                            v, w
                        )),
                    };
                }
            }
            if let Some(r) = red_gte {
                if v >= r {
                    return brain_map::HealthResolution {
                        state: brain_map::HealthState::AtRisk,
                        explanation: Some(format!(
                            "at risk: value {} is above red threshold {}",
                            v, r
                        )),
                    };
                }
            }

            brain_map::HealthResolution {
                state: brain_map::HealthState::Unknown,
                explanation: Some("value does not match configured threshold bands".to_string()),
            }
        }
        "confidence" => {
            let min_confidence = rule
                .get("min_confidence")
                .and_then(|x| x.as_f64())
                .map(|x| x as f32)
                .unwrap_or(0.7);
            let Some(c) = measure.confidence else {
                return brain_map::HealthResolution {
                    state: brain_map::HealthState::Unknown,
                    explanation: Some("confidence health requires measure confidence".to_string()),
                };
            };
            if c < min_confidence {
                return brain_map::HealthResolution {
                    state: brain_map::HealthState::Unknown,
                    explanation: Some(format!(
                        "confidence {} is below minimum {}",
                        c, min_confidence
                    )),
                };
            }
            brain_map::HealthResolution {
                state: brain_map::HealthState::Good,
                explanation: Some(format!("confidence {} meets minimum {}", c, min_confidence)),
            }
        }
        "freshness" | "staleness" => {
            let max_age_days = rule
                .get("max_age_days")
                .and_then(|x| x.as_i64())
                .unwrap_or(30);
            let Some(observed_at) = parse_measure_observed_at(measure) else {
                return brain_map::HealthResolution {
                    state: brain_map::HealthState::Unknown,
                    explanation: Some(
                        "freshness health requires an observation timestamp".to_string(),
                    ),
                };
            };
            let age_days = (chrono::Utc::now() - observed_at).num_days();
            if age_days > max_age_days {
                return brain_map::HealthResolution {
                    state: brain_map::HealthState::Unknown,
                    explanation: Some(format!(
                        "source is stale: {} days old (max {} days)",
                        age_days, max_age_days
                    )),
                };
            }
            brain_map::HealthResolution {
                state: brain_map::HealthState::Good,
                explanation: Some(format!(
                    "source freshness is within policy: {} days old (max {} days)",
                    age_days, max_age_days
                )),
            }
        }
        "conflict" => {
            let has_conflict = measure
                .lineage_gaps
                .iter()
                .any(|g| g.code.contains("conflict"));
            if has_conflict {
                return brain_map::HealthResolution {
                    state: brain_map::HealthState::Unknown,
                    explanation: Some(
                        "conflicting observations detected for the resolved value".to_string(),
                    ),
                };
            }
            brain_map::HealthResolution {
                state: brain_map::HealthState::Good,
                explanation: Some("no conflict markers detected in lineage".to_string()),
            }
        }
        _ => brain_map::HealthResolution {
            state: brain_map::HealthState::Unknown,
            explanation: Some("unsupported health rule type in v1 evaluator".to_string()),
        },
    }
}

fn evaluate_health_unknown_reason(
    measure: &brain_map::MeasureResolution,
    rule: &serde_json::Value,
) -> Option<String> {
    let unknown_when = parse_unknown_when(rule);

    if unknown_when.contains("missing") && measure.value.is_none() {
        return Some("required value is missing".to_string());
    }
    if unknown_when.contains("missing_evidence") && measure.evidence_ids.is_empty() {
        return Some("evidence is required by health rule but none was resolved".to_string());
    }
    if unknown_when.contains("low_confidence") {
        let min_confidence = rule
            .get("min_confidence")
            .and_then(|x| x.as_f64())
            .map(|x| x as f32)
            .unwrap_or(0.7);
        let Some(c) = measure.confidence else {
            return Some("confidence is required by health rule but missing".to_string());
        };
        if c < min_confidence {
            return Some(format!(
                "value confidence {} is below minimum {}",
                c, min_confidence
            ));
        }
    }
    if unknown_when.contains("stale") || unknown_when.contains("stale_source") {
        let max_age_days = rule
            .get("max_age_days")
            .and_then(|x| x.as_i64())
            .unwrap_or(30);
        let Some(observed_at) = parse_measure_observed_at(measure) else {
            return Some("freshness rule requires an observation timestamp".to_string());
        };
        let age_days = (chrono::Utc::now() - observed_at).num_days();
        if age_days > max_age_days {
            return Some(format!(
                "source is stale: {} days old (max {} days)",
                age_days, max_age_days
            ));
        }
    }
    if unknown_when.contains("conflicted")
        && measure
            .lineage_gaps
            .iter()
            .any(|g| g.code.contains("conflict"))
    {
        return Some("value is conflicted by lineage signals".to_string());
    }

    None
}

fn parse_unknown_when(rule: &serde_json::Value) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::<String>::new();
    match rule.get("unknown_when") {
        Some(serde_json::Value::String(s)) => {
            let normalized = s.trim().to_ascii_lowercase();
            if normalized == "missing_or_low_confidence" {
                out.insert("missing_evidence".to_string());
                out.insert("low_confidence".to_string());
                return out;
            }
            for token in normalized
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                out.insert(token.to_string());
            }
        }
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                if let Some(s) = item.as_str() {
                    out.insert(s.to_ascii_lowercase());
                }
            }
        }
        _ => {}
    }
    out
}

fn parse_measure_observed_at(
    measure: &brain_map::MeasureResolution,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Some(ts) = &measure.observed_at {
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(ts) {
            return Some(parsed.with_timezone(&chrono::Utc));
        }
    }
    let newest_lineage_ts = measure
        .evidence_lineage
        .iter()
        .filter_map(|item| item.timestamp.as_deref())
        .filter_map(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .map(|ts| ts.with_timezone(&chrono::Utc))
        .max();
    newest_lineage_ts
}

#[cfg(test)]
mod tests {
    use super::*;
    use axonmind_core::{
        Confidence, Edge, EdgeId, Evidence, EvidenceId, ExtractorKind, NodeId, SourceType,
    };
    use chrono::{Duration, Utc};

    fn kpi_node(id: &str, name: &str) -> Node {
        let now = Utc::now();
        Node {
            id: NodeId(id.to_string()),
            kind: NodeKind::Kpi,
            name: name.to_string(),
            created_at: now,
            updated_at: now,
            attrs: serde_json::json!({}),
            confidence: Confidence::RULE,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    fn metric_node(id: &str) -> Node {
        let now = Utc::now();
        Node {
            id: NodeId(id.to_string()),
            kind: NodeKind::Metric,
            name: id.to_string(),
            created_at: now,
            updated_at: now,
            attrs: serde_json::json!({}),
            confidence: Confidence::RULE,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    fn document_node(id: &str) -> Node {
        let now = Utc::now();
        Node {
            id: NodeId(id.to_string()),
            kind: NodeKind::Document,
            name: id.to_string(),
            created_at: now,
            updated_at: now,
            attrs: serde_json::json!({"source_path":"fixtures/source.md"}),
            confidence: Confidence::RULE,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    fn evidence(
        id: &str,
        source_node_id: &str,
        confidence: f32,
        ts: chrono::DateTime<Utc>,
    ) -> Evidence {
        Evidence {
            id: EvidenceId(id.to_string()),
            source_node_id: NodeId(source_node_id.to_string()),
            source_type: SourceType::Document,
            quote: Some(format!("quote:{id}")),
            row_ref: None,
            blob_sha256: None,
            timestamp: Some(ts),
            extractor: ExtractorKind::Rule,
            confidence: Confidence(confidence),
            is_tainted: false,
            requires_human_review: false,
        }
    }

    #[test]
    fn derived_measure_propagates_input_confidence_and_lineage() {
        let now = Utc::now();
        let nodes = vec![
            kpi_node("kpi.loss_ratio", "Loss Ratio"),
            kpi_node("kpi.incurred_claims", "Incurred Claims"),
            kpi_node("kpi.earned_premium", "Earned Premium"),
            metric_node("metric.incurred_claims"),
            metric_node("metric.earned_premium"),
            metric_node("metric.loss_ratio"),
            document_node("doc.source"),
        ];
        let nodes_by_id = nodes
            .iter()
            .map(|n| (n.id.0.clone(), n))
            .collect::<std::collections::HashMap<_, _>>();

        let ev1 = evidence("ev.incurred", "doc.source", 0.9, now - Duration::days(2));
        let ev2 = evidence("ev.premium", "doc.source", 0.7, now - Duration::days(1));
        let ev3 = evidence("ev.loss_ratio", "doc.source", 0.8, now);
        let evidence = vec![ev1, ev2, ev3];
        let evidence_by_id = evidence
            .iter()
            .map(|e| (e.id.0.clone(), e))
            .collect::<std::collections::HashMap<_, _>>();

        let metric_values = vec![
            store::MetricValue {
                id: "mv.incurred".to_string(),
                kpi_node_id: NodeId("kpi.incurred_claims".to_string()),
                metric_node_id: NodeId("metric.incurred_claims".to_string()),
                value: 40.0,
                unit: "USD".to_string(),
                period_start: None,
                period_end: None,
                as_of: None,
                observed_at: now - Duration::days(2),
                evidence_id: EvidenceId("ev.incurred".to_string()),
            },
            store::MetricValue {
                id: "mv.premium".to_string(),
                kpi_node_id: NodeId("kpi.earned_premium".to_string()),
                metric_node_id: NodeId("metric.earned_premium".to_string()),
                value: 100.0,
                unit: "USD".to_string(),
                period_start: None,
                period_end: None,
                as_of: None,
                observed_at: now - Duration::days(1),
                evidence_id: EvidenceId("ev.premium".to_string()),
            },
            store::MetricValue {
                id: "mv.loss_ratio".to_string(),
                kpi_node_id: NodeId("kpi.loss_ratio".to_string()),
                metric_node_id: NodeId("metric.loss_ratio".to_string()),
                value: 0.4,
                unit: "ratio".to_string(),
                period_start: None,
                period_end: None,
                as_of: None,
                observed_at: now,
                evidence_id: EvidenceId("ev.loss_ratio".to_string()),
            },
        ];
        let mut metric_values_by_kpi =
            std::collections::HashMap::<String, Vec<&store::MetricValue>>::new();
        for mv in &metric_values {
            metric_values_by_kpi
                .entry(mv.kpi_node_id.0.clone())
                .or_default()
                .push(mv);
        }

        let measure = serde_json::json!({
            "type":"derived",
            "kpi":"loss_ratio",
            "formula":"incurred_claims / earned_premium",
            "inputs":["incurred_claims","earned_premium"],
            "period":"latest",
            "as_of":"latest"
        });
        let out = resolve_derived_measure(
            &measure,
            &nodes_by_id,
            &[],
            &metric_values_by_kpi,
            &evidence_by_id,
        );

        assert!(matches!(out.state, brain_map::MeasureState::Resolved));
        assert_eq!(out.value, Some(0.4));
        assert_eq!(out.confidence, Some(0.7));
        assert!(
            out.supporting_nodes
                .iter()
                .any(|n| n.role == "derived_target")
        );
        assert!(
            out.supporting_nodes
                .iter()
                .any(|n| n.role == "derived_input")
        );
        assert!(
            out.evidence_lineage
                .iter()
                .any(|x| x.evidence_id == "ev.incurred")
        );
        assert!(
            out.evidence_lineage
                .iter()
                .any(|x| x.evidence_id == "ev.premium")
        );
    }

    #[test]
    fn derived_measure_becomes_unknown_when_an_input_is_unresolved() {
        let now = Utc::now();
        let nodes = vec![
            kpi_node("kpi.loss_ratio", "Loss Ratio"),
            kpi_node("kpi.incurred_claims", "Incurred Claims"),
            kpi_node("kpi.earned_premium", "Earned Premium"),
            metric_node("metric.incurred_claims"),
            metric_node("metric.loss_ratio"),
            document_node("doc.source"),
        ];
        let nodes_by_id = nodes
            .iter()
            .map(|n| (n.id.0.clone(), n))
            .collect::<std::collections::HashMap<_, _>>();
        let edges = vec![
            Edge {
                id: EdgeId("edge.derived.1".to_string()),
                from: NodeId("kpi.loss_ratio".to_string()),
                to: NodeId("kpi.incurred_claims".to_string()),
                kind: EdgeKind::DerivedFrom,
                evidence: vec![EvidenceId("ev.derived.1".to_string())],
                confidence: Confidence::RULE,
                created_at: now,
                created_by: ExtractorKind::Rule,
                is_tainted: false,
                requires_human_review: false,
            },
            Edge {
                id: EdgeId("edge.derived.2".to_string()),
                from: NodeId("kpi.loss_ratio".to_string()),
                to: NodeId("kpi.earned_premium".to_string()),
                kind: EdgeKind::DerivedFrom,
                evidence: vec![EvidenceId("ev.derived.2".to_string())],
                confidence: Confidence::RULE,
                created_at: now,
                created_by: ExtractorKind::Rule,
                is_tainted: false,
                requires_human_review: false,
            },
        ];

        let ev1 = evidence("ev.incurred", "doc.source", 0.8, now);
        let evidence = vec![ev1];
        let evidence_by_id = evidence
            .iter()
            .map(|e| (e.id.0.clone(), e))
            .collect::<std::collections::HashMap<_, _>>();
        let metric_values = vec![
            store::MetricValue {
                id: "mv.incurred".to_string(),
                kpi_node_id: NodeId("kpi.incurred_claims".to_string()),
                metric_node_id: NodeId("metric.incurred_claims".to_string()),
                value: 40.0,
                unit: "USD".to_string(),
                period_start: None,
                period_end: None,
                as_of: None,
                observed_at: now,
                evidence_id: EvidenceId("ev.incurred".to_string()),
            },
            store::MetricValue {
                id: "mv.loss_ratio".to_string(),
                kpi_node_id: NodeId("kpi.loss_ratio".to_string()),
                metric_node_id: NodeId("metric.loss_ratio".to_string()),
                value: 0.4,
                unit: "ratio".to_string(),
                period_start: None,
                period_end: None,
                as_of: None,
                observed_at: now,
                evidence_id: EvidenceId("ev.incurred".to_string()),
            },
        ];
        let mut metric_values_by_kpi =
            std::collections::HashMap::<String, Vec<&store::MetricValue>>::new();
        for mv in &metric_values {
            metric_values_by_kpi
                .entry(mv.kpi_node_id.0.clone())
                .or_default()
                .push(mv);
        }

        let measure = serde_json::json!({
            "type":"derived",
            "kpi":"loss_ratio",
            "formula":"incurred_claims / earned_premium",
            "period":"latest",
            "as_of":"latest"
        });
        let out = resolve_derived_measure(
            &measure,
            &nodes_by_id,
            &edges,
            &metric_values_by_kpi,
            &evidence_by_id,
        );

        assert!(matches!(out.state, brain_map::MeasureState::Unknown));
        assert!(
            out.lineage_gaps
                .iter()
                .any(|g| g.code == "derived_input_missing_or_unknown")
        );
        assert!(
            out.explanation
                .as_deref()
                .unwrap_or_default()
                .contains("earned_premium")
        );
    }

    #[test]
    fn ref_kpi_prefers_business_as_of_over_observed_at_for_latest() {
        let now = Utc::now();
        let nodes = vec![
            kpi_node("kpi.loss_ratio", "Loss Ratio"),
            metric_node("metric.loss_ratio"),
            document_node("doc.source"),
        ];
        let nodes_by_id = nodes
            .iter()
            .map(|n| (n.id.0.clone(), n))
            .collect::<std::collections::HashMap<_, _>>();
        let ev1 = evidence("ev.old_asof", "doc.source", 0.8, now - Duration::days(2));
        let ev2 = evidence("ev.new_asof", "doc.source", 0.9, now - Duration::days(1));
        let evidence = vec![ev1, ev2];
        let evidence_by_id = evidence
            .iter()
            .map(|e| (e.id.0.clone(), e))
            .collect::<std::collections::HashMap<_, _>>();

        let metric_values = vec![
            store::MetricValue {
                id: "mv1".to_string(),
                kpi_node_id: NodeId("kpi.loss_ratio".to_string()),
                metric_node_id: NodeId("metric.loss_ratio".to_string()),
                value: 0.1,
                unit: "ratio".to_string(),
                period_start: None,
                period_end: None,
                as_of: Some(now - Duration::days(10)),
                observed_at: now,
                evidence_id: EvidenceId("ev.old_asof".to_string()),
            },
            store::MetricValue {
                id: "mv2".to_string(),
                kpi_node_id: NodeId("kpi.loss_ratio".to_string()),
                metric_node_id: NodeId("metric.loss_ratio".to_string()),
                value: 0.2,
                unit: "ratio".to_string(),
                period_start: None,
                period_end: None,
                as_of: Some(now - Duration::days(1)),
                observed_at: now - Duration::days(2),
                evidence_id: EvidenceId("ev.new_asof".to_string()),
            },
        ];
        let mut metric_values_by_kpi =
            std::collections::HashMap::<String, Vec<&store::MetricValue>>::new();
        for mv in &metric_values {
            metric_values_by_kpi
                .entry(mv.kpi_node_id.0.clone())
                .or_default()
                .push(mv);
        }

        let out = resolve_ref_kpi_measure(
            &serde_json::json!({"type":"ref_kpi","kpi":"loss_ratio","period":"latest","as_of":"latest"}),
            &nodes_by_id,
            &metric_values_by_kpi,
            &evidence_by_id,
        );
        assert!(matches!(out.state, brain_map::MeasureState::Resolved));
        assert_eq!(out.value, Some(0.2));
    }

    #[test]
    fn count_measure_applies_period_as_of_scope_for_timestamped_nodes() {
        let now = Utc::now();
        let nodes = vec![
            Node {
                id: NodeId("n.within".to_string()),
                kind: NodeKind::Metric,
                name: "Within".to_string(),
                created_at: now,
                updated_at: now,
                attrs: serde_json::json!({"as_of": now.to_rfc3339()}),
                confidence: Confidence::RULE,
                is_tainted: false,
                requires_human_review: false,
            },
            Node {
                id: NodeId("n.outside".to_string()),
                kind: NodeKind::Metric,
                name: "Outside".to_string(),
                created_at: now,
                updated_at: now,
                attrs: serde_json::json!({"as_of": (now - Duration::days(80)).to_rfc3339()}),
                confidence: Confidence::RULE,
                is_tainted: false,
                requires_human_review: false,
            },
            Node {
                id: NodeId("n.missing_ts".to_string()),
                kind: NodeKind::Metric,
                name: "Missing".to_string(),
                created_at: now,
                updated_at: now,
                attrs: serde_json::json!({}),
                confidence: Confidence::RULE,
                is_tainted: false,
                requires_human_review: false,
            },
        ];
        let nodes_by_id = nodes
            .iter()
            .map(|n| (n.id.0.clone(), n))
            .collect::<std::collections::HashMap<_, _>>();
        let selected = vec![
            "n.within".to_string(),
            "n.outside".to_string(),
            "n.missing_ts".to_string(),
        ];
        let out = resolve_lens_measure(
            &serde_json::json!({
                "type":"count",
                "period":"MTD",
                "as_of": now.to_rfc3339()
            }),
            &selected,
            &nodes_by_id,
            &[],
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        );
        assert!(matches!(out.state, brain_map::MeasureState::Resolved));
        assert_eq!(out.value, Some(1.0));
        assert!(
            out.lineage_gaps
                .iter()
                .any(|g| g.code == "temporal_scope_partial")
        );
    }
}
