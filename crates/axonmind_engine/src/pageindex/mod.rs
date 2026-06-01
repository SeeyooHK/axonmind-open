pub mod enrich;
pub mod search;
pub mod store;
pub mod tree;

pub use store::PageIndexStore;
pub use tree::{PageSection, PersistTree, SectionRow, build_tree, flatten_tree};

use axonmind_core::AxonMindError;

use crate::config::EngineConfig;
use crate::extract::llm::LlmProvider;
use crate::ingest::NormalizedDocument;

/// Configuration for the BM25→LLM retrieval funnel.
pub struct PageIndexSearchCfg {
    pub shortlist_limit: usize,
    pub max_results: usize,
}

impl Default for PageIndexSearchCfg {
    fn default() -> Self {
        Self {
            shortlist_limit: 40,
            max_results: 20,
        }
    }
}

/// Build and persist the page index for a single document.
///
/// Skips if the stored sha256 matches the document's sha256 (no-op on re-ingest of unchanged doc).
/// If enrichment is enabled (`config.pageindex_enrich`) and an LLM provider is available,
/// runs bottom-up summary generation before persist (feeds FTS vocabulary + Stage-2 reranking).
pub async fn index_document(
    doc: &NormalizedDocument,
    doc_node_id: &str,
    store: &PageIndexStore,
    llm: Option<&dyn LlmProvider>,
    config: &EngineConfig,
) -> Result<(), AxonMindError> {
    if !config.pageindex_enabled {
        return Ok(());
    }

    // Staleness check: skip if the stored sha matches the current document sha.
    if let Ok(Some(stored_sha)) = store.page_tree_sha(doc_node_id).await {
        if stored_sha == doc.sha256 {
            return Ok(());
        }
    }

    let mut roots = build_tree(doc, doc_node_id);

    if roots.is_empty() {
        return Ok(());
    }

    // Optional enrichment: bottom-up LLM summaries (gated, never at query time).
    let doc_summary = if config.pageindex_enrich {
        if let Some(llm) = llm {
            match enrich::enrich_tree(
                &mut roots,
                llm,
                config.pageindex_enrich_concurrency,
            )
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("pageindex enrichment failed for {doc_node_id}: {e}");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let doc_title = doc
        .title
        .as_deref()
        .or_else(|| {
            doc.source_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
        })
        .unwrap_or("Document");

    let persist_tree = flatten_tree(&roots, doc_node_id, &doc.sha256, doc_title, doc_summary);
    store.upsert_document(&persist_tree).await
}
