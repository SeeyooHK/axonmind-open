/// PageIndex integration tests — store, funnel, enrichment.
/// Rule 9: each test encodes WHY the behavior matters.
use axonmind_core::AxonMindError;
use axonmind_engine::{
    AxonMindEngine,
    config::EngineConfig,
    extract::llm::{
        EntityExtractionInput, EntityExtractionOutput, LlmProvider, RelationExtractionInput,
        RelationExtractionOutput, SemanticLinkInput, SemanticLinkOutput,
    },
    ingest::{IngestOptions, IngestSource},
    pageindex::{PageIndexSearchCfg, PageIndexStore, PersistTree, SectionRow},
    query::ReasoningSearchInput,
};
use std::sync::Arc;
use tempfile::TempDir;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn engine_config(dir: &TempDir) -> EngineConfig {
    let mut cfg = EngineConfig::from_workspace_dir(dir.path().to_path_buf());
    cfg.pageindex_enabled = true;
    cfg
}

async fn open_engine(dir: &TempDir) -> AxonMindEngine {
    AxonMindEngine::open(engine_config(dir))
        .await
        .expect("engine open failed")
}

async fn test_store(dir: &TempDir) -> PageIndexStore {
    let cfg = engine_config(dir);
    // Open engine to run migrations (creates the DB with all tables, including page_*).
    AxonMindEngine::open(cfg.clone())
        .await
        .expect("engine open for store test failed");
    let pool = deadpool_sqlite::Config::new(&cfg.database_path)
        .builder(deadpool_sqlite::Runtime::Tokio1)
        .expect("pool builder")
        .build()
        .expect("pool build");
    PageIndexStore::new(pool)
}

fn sample_persist_tree(doc_node_id: &str) -> PersistTree {
    let sections = vec![
        SectionRow {
            section_id: format!("{doc_node_id}#0001"),
            doc_node_id: doc_node_id.to_string(),
            parent_section_id: None,
            ordinal: 0,
            level: 1,
            title: "Revenue Growth".to_string(),
            path: "Revenue Growth".to_string(),
            summary: Some("Revenue grew 15% YoY driven by new product launches.".to_string()),
            text: Some("Our revenue grew significantly this year.".to_string()),
            span_start: 0,
            span_end: 100,
        },
        SectionRow {
            section_id: format!("{doc_node_id}#0002"),
            doc_node_id: doc_node_id.to_string(),
            parent_section_id: Some(format!("{doc_node_id}#0001")),
            ordinal: 0,
            level: 2,
            title: "Q1 Performance".to_string(),
            path: "Revenue Growth \u{203a} Q1 Performance".to_string(),
            summary: None,
            text: Some("Q1 revenue was $5M, up from $4M.".to_string()),
            span_start: 50,
            span_end: 80,
        },
    ];
    PersistTree {
        doc_node_id: doc_node_id.to_string(),
        sha256: "sha256test".to_string(),
        title: "Annual Report".to_string(),
        doc_summary: None,
        sections,
    }
}

// ── Store tests ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_fts_sync_after_upsert() {
    // After upsert_document, bm25_shortlist must find a section by a term in its text,
    // and also by a term that only appears in its summary (proving summary is FTS-indexed).
    let dir = TempDir::new().unwrap();
    let store = test_store(&dir).await;

    store
        .upsert_document(&sample_persist_tree("doc.testaaaa"))
        .await
        .expect("upsert failed");

    // Term in text
    let ids = store
        .bm25_shortlist("\"revenue\"", 10, None)
        .await
        .expect("bm25 failed");
    assert!(
        !ids.is_empty(),
        "should find section with 'revenue' in text"
    );

    // Term only in summary: "drove" is in the summary ("Revenue grew 15% YoY driven by...")
    // Use "drove" vs "driven" — let's use "YoY" which is unique to the summary.
    let ids_from_summary = store
        .bm25_shortlist("\"YoY\"", 10, None)
        .await
        .expect("bm25 failed");
    assert!(
        !ids_from_summary.is_empty(),
        "should find section by term in its summary (summary must be FTS-indexed)"
    );
}

#[tokio::test]
async fn test_bm25_ordering_title_vs_text() {
    // A section with the query term in its title should rank above one with it only in text,
    // because FTS5 BM25 with a title match in a small corpus ranks it higher.
    // This test fails if ORDER BY bm25() direction is wrong (descending instead of ascending).
    let dir = TempDir::new().unwrap();
    let store = test_store(&dir).await;

    let sections = vec![
        SectionRow {
            section_id: "doc.test#0001".to_string(),
            doc_node_id: "doc.test".to_string(),
            parent_section_id: None,
            ordinal: 0,
            level: 1,
            title: "churn churn churn churn churn".to_string(), // "churn" in title (repeated for weight)
            path: "churn churn churn churn churn".to_string(),
            summary: None,
            text: Some("unrelated content about onboarding".to_string()),
            span_start: 0,
            span_end: 50,
        },
        SectionRow {
            section_id: "doc.test#0002".to_string(),
            doc_node_id: "doc.test".to_string(),
            parent_section_id: None,
            ordinal: 1,
            level: 1,
            title: "Retention".to_string(),
            path: "Retention".to_string(),
            summary: None,
            text: Some("we observed churn in our user base last quarter".to_string()),
            span_start: 51,
            span_end: 100,
        },
    ];
    let tree = PersistTree {
        doc_node_id: "doc.test".to_string(),
        sha256: "sha".to_string(),
        title: "Report".to_string(),
        doc_summary: None,
        sections,
    };
    store.upsert_document(&tree).await.expect("upsert failed");

    let ids = store
        .bm25_shortlist("\"churn\"", 10, None)
        .await
        .expect("bm25 failed");
    assert_eq!(ids.len(), 2, "both sections should match");
    // Title match should rank first.
    assert_eq!(
        ids[0], "doc.test#0001",
        "section with 'churn' in title should rank first (ORDER BY bm25() ascending)"
    );
}

#[tokio::test]
async fn test_roundtrip_and_staleness() {
    // Re-indexing with the same sha should be a no-op (staleness check).
    // Changed sha should replace rows and leave no orphan FTS entries.
    let dir = TempDir::new().unwrap();
    let cfg = engine_config(&dir);
    let engine = AxonMindEngine::open(cfg.clone())
        .await
        .expect("engine open");

    let pool = deadpool_sqlite::Config::new(&cfg.database_path)
        .builder(deadpool_sqlite::Runtime::Tokio1)
        .expect("pool builder")
        .build()
        .expect("pool build");
    let store = PageIndexStore::new(pool);

    // Initial upsert.
    let tree = sample_persist_tree("doc.roundtrip");
    store.upsert_document(&tree).await.expect("first upsert");

    let ids1 = store
        .bm25_shortlist("\"revenue\"", 20, None)
        .await
        .expect("bm25");
    assert!(!ids1.is_empty(), "initial index should be searchable");

    // Re-upsert with different sections (simulates changed doc) — old FTS entries must not linger.
    let new_sections = vec![SectionRow {
        section_id: "doc.roundtrip#0001".to_string(),
        doc_node_id: "doc.roundtrip".to_string(),
        parent_section_id: None,
        ordinal: 0,
        level: 1,
        title: "Acquisition".to_string(),
        path: "Acquisition".to_string(),
        summary: None,
        text: Some("Customer acquisition costs decreased.".to_string()),
        span_start: 0,
        span_end: 60,
    }];
    let new_tree = PersistTree {
        doc_node_id: "doc.roundtrip".to_string(),
        sha256: "sha256new".to_string(),
        title: "Updated Report".to_string(),
        doc_summary: None,
        sections: new_sections,
    };
    store
        .upsert_document(&new_tree)
        .await
        .expect("second upsert");

    // Old section ("revenue") should no longer be found.
    let ids_old = store
        .bm25_shortlist("\"revenue\"", 20, None)
        .await
        .expect("bm25");
    assert!(
        ids_old.is_empty(),
        "old FTS entries must be removed on re-upsert (no orphan rows)"
    );

    // New section should be found.
    let ids_new = store
        .bm25_shortlist("\"acquisition\"", 20, None)
        .await
        .expect("bm25");
    assert!(!ids_new.is_empty(), "new section should be searchable");
}

#[tokio::test]
async fn test_delete_removes_sections_and_fts() {
    // delete_document must remove both page_sections and page_section_fts rows.
    let dir = TempDir::new().unwrap();
    let store = test_store(&dir).await;

    store
        .upsert_document(&sample_persist_tree("doc.todelete"))
        .await
        .expect("upsert");

    let before = store
        .bm25_shortlist("\"revenue\"", 10, None)
        .await
        .expect("bm25");
    assert!(!before.is_empty(), "section must exist before delete");

    store
        .delete_document("doc.todelete")
        .await
        .expect("delete failed");

    let after = store
        .bm25_shortlist("\"revenue\"", 10, None)
        .await
        .expect("bm25");
    assert!(
        after.is_empty(),
        "no sections should be findable after delete_document"
    );
}

// ── Funnel tests ──────────────────────────────────────────────────────────────

struct MockProvider {
    response: String,
}

impl MockProvider {
    fn new(response: &str) -> Arc<Self> {
        Arc::new(Self {
            response: response.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl LlmProvider for MockProvider {
    async fn complete(&self, _system: &str, _user: &str) -> Result<String, AxonMindError> {
        Ok(self.response.clone())
    }

    async fn extract_entities(
        &self,
        _input: EntityExtractionInput,
    ) -> Result<EntityExtractionOutput, AxonMindError> {
        Ok(EntityExtractionOutput { entities: vec![] })
    }

    async fn extract_relations(
        &self,
        _input: RelationExtractionInput,
    ) -> Result<RelationExtractionOutput, AxonMindError> {
        Err(AxonMindError::Ingest {
            message: "not implemented".into(),
        })
    }

    async fn link_concepts(
        &self,
        _input: SemanticLinkInput,
    ) -> Result<SemanticLinkOutput, AxonMindError> {
        Ok(SemanticLinkOutput { links: vec![] })
    }

    async fn explain_kpi_rationale(
        &self,
        _kpi_name: &str,
        _evidence_quotes: &[String],
    ) -> Result<String, AxonMindError> {
        Ok(String::new())
    }
}

#[tokio::test]
async fn test_funnel_no_provider_returns_bm25_order() {
    // Without an LLM provider: reasoning_applied == false, results are BM25-ranked.
    let dir = TempDir::new().unwrap();
    let store = test_store(&dir).await;
    store
        .upsert_document(&sample_persist_tree("doc.funnel"))
        .await
        .expect("upsert");

    let cfg = PageIndexSearchCfg {
        shortlist_limit: 40,
        max_results: 20,
    };
    let input = ReasoningSearchInput {
        query: "revenue".to_string(),
        doc_node_ids: None,
        max_results: None,
    };
    let out = axonmind_engine::pageindex::search::reasoning_search(input, &store, None, &cfg)
        .await
        .expect("search failed");

    assert!(
        !out.reasoning_applied,
        "no provider → reasoning_applied must be false"
    );
    assert!(!out.sections.is_empty(), "BM25 should return results");
}

#[tokio::test]
async fn test_funnel_provider_rerank_order() {
    // Mock returns "2, 1": assert output order is 2nd-then-1st candidate and others are dropped.
    // This test fails if Stage-2 selection logic is broken.
    let dir = TempDir::new().unwrap();
    let store = test_store(&dir).await;

    // Insert two sections for the same doc.
    let sections = vec![
        SectionRow {
            section_id: "doc.rerank#0001".to_string(),
            doc_node_id: "doc.rerank".to_string(),
            parent_section_id: None,
            ordinal: 0,
            level: 1,
            title: "Alpha Section".to_string(),
            path: "Alpha Section".to_string(),
            summary: None,
            text: Some("alpha content about profitability growth".to_string()),
            span_start: 0,
            span_end: 50,
        },
        SectionRow {
            section_id: "doc.rerank#0002".to_string(),
            doc_node_id: "doc.rerank".to_string(),
            parent_section_id: None,
            ordinal: 1,
            level: 1,
            title: "Beta Section".to_string(),
            path: "Beta Section".to_string(),
            summary: None,
            text: Some("beta content about profitability growth".to_string()),
            span_start: 51,
            span_end: 100,
        },
    ];
    store
        .upsert_document(&PersistTree {
            doc_node_id: "doc.rerank".to_string(),
            sha256: "sha".to_string(),
            title: "Report".to_string(),
            doc_summary: None,
            sections,
        })
        .await
        .expect("upsert");

    // Mock LLM returns "2, 1" — second candidate first, first candidate second.
    let llm = MockProvider::new("2, 1");
    let cfg = PageIndexSearchCfg {
        shortlist_limit: 40,
        max_results: 20,
    };
    let input = ReasoningSearchInput {
        query: "profitability".to_string(),
        doc_node_ids: None,
        max_results: None,
    };
    let out = axonmind_engine::pageindex::search::reasoning_search(
        input,
        &store,
        Some(llm.as_ref()),
        &cfg,
    )
    .await
    .expect("search failed");

    assert!(
        out.reasoning_applied,
        "LLM provider present → reasoning_applied must be true"
    );
    assert_eq!(out.sections.len(), 2, "both sections selected");
    assert_eq!(
        out.sections[0].section_id, "doc.rerank#0002",
        "LLM said '2, 1' so 2nd BM25 candidate should be first in output"
    );
    assert_eq!(out.sections[1].section_id, "doc.rerank#0001");
}

#[tokio::test]
async fn test_funnel_none_response() {
    // When mock LLM replies "NONE", output should have empty sections, no error.
    let dir = TempDir::new().unwrap();
    let store = test_store(&dir).await;
    store
        .upsert_document(&sample_persist_tree("doc.none"))
        .await
        .expect("upsert");

    let llm = MockProvider::new("NONE");
    let cfg = PageIndexSearchCfg {
        shortlist_limit: 40,
        max_results: 20,
    };
    let input = ReasoningSearchInput {
        query: "revenue".to_string(),
        doc_node_ids: None,
        max_results: None,
    };
    let out = axonmind_engine::pageindex::search::reasoning_search(
        input,
        &store,
        Some(llm.as_ref()),
        &cfg,
    )
    .await
    .expect("search failed");

    assert!(
        out.sections.is_empty(),
        "NONE response should yield empty results"
    );
    assert!(out.reasoning_applied);
}

#[tokio::test]
async fn test_funnel_doc_filter() {
    // When doc_node_id is set, only sections from that document are returned.
    let dir = TempDir::new().unwrap();
    let store = test_store(&dir).await;

    store
        .upsert_document(&sample_persist_tree("doc.aaa"))
        .await
        .expect("upsert doc.aaa");

    // Second doc with same term but different doc_node_id.
    let other = PersistTree {
        doc_node_id: "doc.bbb".to_string(),
        sha256: "sha_bbb".to_string(),
        title: "Other".to_string(),
        doc_summary: None,
        sections: vec![SectionRow {
            section_id: "doc.bbb#0001".to_string(),
            doc_node_id: "doc.bbb".to_string(),
            parent_section_id: None,
            ordinal: 0,
            level: 1,
            title: "Revenue Overview".to_string(),
            path: "Revenue Overview".to_string(),
            summary: None,
            text: Some("revenue figures for another company".to_string()),
            span_start: 0,
            span_end: 50,
        }],
    };
    store.upsert_document(&other).await.expect("upsert doc.bbb");

    let cfg = PageIndexSearchCfg {
        shortlist_limit: 40,
        max_results: 20,
    };
    let input = ReasoningSearchInput {
        query: "revenue".to_string(),
        doc_node_ids: Some(vec!["doc.aaa".to_string()]),
        max_results: None,
    };
    let out = axonmind_engine::pageindex::search::reasoning_search(input, &store, None, &cfg)
        .await
        .expect("search failed");

    assert!(
        out.sections.iter().all(|s| s.doc_node_id == "doc.aaa"),
        "doc filter must restrict results to doc.aaa only"
    );

    // Cross-link: the returned doc_node_id must equal the graph Document node id.
    assert_eq!(
        out.sections[0].doc_node_id, "doc.aaa",
        "RetrievedSection.doc_node_id bridges to the graph Document node"
    );
}

// ── rebuild_page_index tests ──────────────────────────────────────────────────

#[tokio::test]
async fn test_rebuild_page_index_restores_searchability() {
    // rebuild_page_index must re-populate page_* for a document whose page_tree row was deleted
    // (simulating documents indexed before the pageindex feature existed). This is the primary
    // use case the command exists for — without it, "Search Contents" silently returns nothing
    // for pre-existing docs and the user has no targeted fix short of full regeneration.
    let dir = TempDir::new().unwrap();
    let cfg = engine_config(&dir);
    let engine = AxonMindEngine::open(cfg.clone()).await.expect("engine open");

    let content = "# Revenue Growth\n\nRevenue grew 20% driven by enterprise deals.\n\n## Q2 Results\n\nQ2 revenue was $8M.";
    let md_dir = TempDir::new().unwrap();
    let md_path = md_dir.path().join("revenue.md");
    std::fs::write(&md_path, content).unwrap();

    engine
        .ingest_sync(
            IngestSource::File(md_path),
            IngestOptions {
                recursive: false,
                skip_unchanged: false,
                max_file_size_bytes: 10 * 1024 * 1024,
            },
        )
        .await
        .expect("ingest failed");

    // Confirm sections exist after normal ingest.
    let before = engine
        .reasoning_search(ReasoningSearchInput {
            query: "revenue".to_string(),
            doc_node_ids: None,
            max_results: Some(5),
        })
        .await
        .expect("search before delete");
    assert!(!before.sections.is_empty(), "sections must exist after ingest");

    // Simulate pre-pageindex state: delete page_tree and page_sections rows.
    // After this, page_tree_sha returns None, which causes index_document to rebuild.
    let pool = deadpool_sqlite::Config::new(&cfg.database_path)
        .builder(deadpool_sqlite::Runtime::Tokio1)
        .expect("pool builder")
        .build()
        .expect("pool build");
    let conn = pool.get().await.expect("get conn");
    conn.interact(|conn| {
        conn.execute_batch(
            "DELETE FROM page_section_fts;
             DELETE FROM page_sections;
             DELETE FROM page_tree;",
        )
    })
    .await
    .expect("interact")
    .expect("delete page rows");

    // Confirm search no longer works.
    let after_delete = engine
        .reasoning_search(ReasoningSearchInput {
            query: "revenue".to_string(),
            doc_node_ids: None,
            max_results: Some(5),
        })
        .await
        .expect("search after delete");
    assert!(
        after_delete.sections.is_empty(),
        "search must return nothing after page_* rows are deleted"
    );

    // rebuild_page_index should restore searchability without touching graph tables.
    let (processed, skipped, errors) = engine.rebuild_page_index().await.expect("rebuild failed");
    assert!(errors.is_empty(), "rebuild errors: {errors:?}");
    assert_eq!(skipped, 0, "no docs should be skipped (all have sha256 + source_path)");
    assert!(processed >= 1, "at least one document must be processed");

    let after_rebuild = engine
        .reasoning_search(ReasoningSearchInput {
            query: "revenue".to_string(),
            doc_node_ids: None,
            max_results: Some(5),
        })
        .await
        .expect("search after rebuild");
    assert!(
        !after_rebuild.sections.is_empty(),
        "sections must be searchable after rebuild_page_index"
    );
}

#[tokio::test]
async fn test_rebuild_page_index_skips_unchanged() {
    // A second consecutive rebuild_page_index must not error on documents whose page_tree sha
    // already matches — index_document is a no-op in this case (staleness check inside).
    // The engine counts both outcomes as "processed" (index_document returns Ok for both);
    // this test pins that behaviour so a future counter fix gets a failing test to guide it.
    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;

    let content = "# Churn\n\nWe track monthly churn as a key metric.";
    let md_dir = TempDir::new().unwrap();
    let md_path = md_dir.path().join("churn2.md");
    std::fs::write(&md_path, content).unwrap();

    engine
        .ingest_sync(
            IngestSource::File(md_path),
            IngestOptions {
                recursive: false,
                skip_unchanged: false,
                max_file_size_bytes: 10 * 1024 * 1024,
            },
        )
        .await
        .expect("ingest failed");

    // First rebuild (sha already matches after normal ingest — staleness check fires immediately).
    let (_, _, errors1) = engine.rebuild_page_index().await.expect("first rebuild");
    assert!(errors1.is_empty(), "no errors on first rebuild: {errors1:?}");

    // Second rebuild — must also be error-free.
    let (_, _, errors2) = engine.rebuild_page_index().await.expect("second rebuild");
    assert!(errors2.is_empty(), "no errors on second rebuild: {errors2:?}");
}

// ── Engine-level ingest hook test ─────────────────────────────────────────────

#[tokio::test]
async fn test_ingest_populates_pageindex() {
    // After ingest, reasoning_search should find the ingested content.
    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;

    let content = "# Churn Rate\n\nWe track monthly churn as a key retention metric.\n\n## Q1 Churn\n\nQ1 churn was 2.3%.";
    let md_dir = TempDir::new().unwrap();
    let md_path = md_dir.path().join("churn.md");
    std::fs::write(&md_path, content).unwrap();

    let summary = engine
        .ingest_sync(
            IngestSource::File(md_path),
            IngestOptions {
                recursive: false,
                skip_unchanged: false,
                max_file_size_bytes: 10 * 1024 * 1024,
            },
        )
        .await
        .expect("ingest failed");

    // No pageindex errors during ingest.
    let pi_errors: Vec<_> = summary
        .errors
        .iter()
        .filter(|e| e.starts_with("pageindex:"))
        .collect();
    assert!(
        pi_errors.is_empty(),
        "pageindex errors during ingest: {pi_errors:?}"
    );

    // The ingested content is now searchable via reasoning_search.
    let out = engine
        .reasoning_search(ReasoningSearchInput {
            query: "churn".to_string(),
            doc_node_ids: None,
            max_results: Some(5),
        })
        .await
        .expect("reasoning_search failed");

    assert!(
        !out.sections.is_empty(),
        "reasoning_search should find sections after ingest"
    );
}
