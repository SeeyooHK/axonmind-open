use axonmind_engine::{
    AxonMindEngine,
    config::EngineConfig,
    ingest::{IngestOptions, IngestSource},
    query::DocumentSearchInput,
    structure::{StructurePackage, derive_identity, parse_document},
};
use tempfile::TempDir;

fn test_engine_config(dir: &TempDir) -> EngineConfig {
    EngineConfig::from_workspace_dir(dir.path().join("workspace"))
}

#[tokio::test]
async fn installs_and_lists_structure_package() {
    let temp = TempDir::new().expect("tempdir");
    let cfg = test_engine_config(&temp);
    let engine = AxonMindEngine::open(cfg).await.expect("engine");
    let package_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");

    let report = engine
        .install_structure_package_from_dir(&package_dir, "standalone")
        .await
        .expect("install");
    assert_eq!(report.package_name, "generic-manual");
    assert_eq!(report.profiles, 3);

    let packages = engine.list_structure_packages().await.expect("list");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].package_name, "generic-manual");
    assert_eq!(packages[0].sources, vec!["standalone".to_string()]);
}

/// Acceptance check for the step-5 cutover (docs/retrieve_guarantee.md item 1): a fresh
/// axonmind install — zero structure packages ever installed — must not recognize a GDPR-looking
/// document as GDPR. Before the cutover, `legal.rs::infer_document_identity` hardcoded literal
/// `"2016/679"`/`"gdpr"` string matches and fired whenever `installed_packages.is_empty()`, so a
/// fresh install with no packages would still silently identify this exact document as the GDPR
/// Regulation — the generic-claim violation the cutover was for. With `legal.rs` deleted, the
/// only remaining path is `derive_identity`'s package-driven rule loop, which has nothing to
/// iterate when `packages` is empty, so it must fall through to the untyped fallback: no
/// instrument_type, no corpus, confidence 0.35.
///
/// Content deliberately has no Markdown H1 heading (mimicking many real-world PDF renderings,
/// which don't yield a clean top-level heading) — `ingest/markdown.rs`'s generic "first H1 becomes
/// the title" heuristic is unrelated to package/domain matching, and a document WITH a heading
/// would legitimately get that heading text as its title even with zero packages installed (that
/// heuristic is format-agnostic, not a bug). Omitting it isolates the actual claim under test:
/// with no heading to derive a title from, `canonical_title` falls all the way to the filename
/// stem, proving the fallback is the generic one, not a resurrected hardcoded GDPR catalog.
#[tokio::test]
async fn fresh_install_with_zero_packages_falls_back_to_generic_identity() {
    let temp = TempDir::new().expect("tempdir");
    let cfg = test_engine_config(&temp);
    let engine = AxonMindEngine::open(cfg).await.expect("engine");
    // No install_structure_package* call anywhere above — this is the "zero packages" state.

    let content = "4.5.2016 EN Official Journal of the European Union L 119/1\n\n\
REGULATION (EU) 2016/679 OF THE EUROPEAN PARLIAMENT AND OF THE COUNCIL of 27 April 2016 on the \
protection of natural persons with regard to the processing of personal data and on the free \
movement of such data, and repealing Directive 95/46/EC (General Data Protection Regulation)\n\n\
Article 1\n\nSubject-matter and objectives\n\n\
1. This Regulation lays down rules relating to the protection of natural persons.\n";
    let doc_dir = TempDir::new().expect("doc tempdir");
    // Filename deliberately mirrors the real corpus doc's naming (legal_gdpr_Regulation_2016_679)
    // to prove the fallback is genuinely content/package-driven, not just a filename fluke.
    let doc_path = doc_dir
        .path()
        .join("legal_gdpr_Regulation_2016_679__GDPR_.md");
    std::fs::write(&doc_path, content).expect("write doc");

    engine
        .ingest_sync(
            IngestSource::File(doc_path),
            IngestOptions {
                recursive: false,
                skip_unchanged: false,
                max_file_size_bytes: 10 * 1024 * 1024,
            },
        )
        .await
        .expect("ingest failed");

    let pool = engine.db_pool();
    let conn = pool.get().await.expect("get conn");
    let (canonical_title, confidence, instrument_type, corpus): (String, f64, Option<String>, String) = conn
        .interact(|conn| {
            conn.query_row(
                "SELECT canonical_title, confidence, instrument_type, corpus FROM document_identity \
                 WHERE source_filename = 'legal_gdpr_Regulation_2016_679__GDPR_.md'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
        })
        .await
        .expect("interact")
        .expect("identity row");

    assert_eq!(
        canonical_title, "legal_gdpr_Regulation_2016_679__GDPR_",
        "with zero packages installed, canonical_title must be the filename stem, not a \
         GDPR-derived title — a non-stem title here means domain vocabulary leaked into axonmind \
         outside of package data"
    );
    assert!(
        (confidence - 0.35).abs() < 1e-6,
        "fallback confidence must be the untyped 0.35, got {confidence}"
    );
    assert_eq!(
        instrument_type, None,
        "zero packages installed means no rule could have set instrument_type"
    );
    assert_eq!(
        corpus, "[]",
        "zero packages installed means no corpus binding could have fired"
    );
}

/// Acceptance check for the step-5 cutover (docs/retrieve_guarantee.md item 1): real-GDPR-corpus
/// citation correctness. The doc's original acceptance wording asked for citations
/// "byte-identical to before" — diffing the live `doc_units.citation` values against the
/// pre-cutover `legal_units` backup (`~/.soverex/embedded_axonmind/backups/axonmind.db.20260706_192929.bak`,
/// reconstructing the deleted `build_citation()` from git history) found that goal was the wrong
/// one: of 604 real (non-EDPB) rows, 284/284 recital citations and 26/72 paragraph citations in
/// the *old* system were already wrong — the pre-cutover `structure_legacy_units` bridge mapped
/// ANY unit's own "number" capture into the legacy `article` column whenever a true "article" key
/// was absent, so a recital's own number got misread as an article number (citation said "Article
/// 33" instead of "Recital 33"), and a paragraph's own number overwrote its *parent* article's
/// number (Article 4's paragraph 7 said "Article 7(7)" instead of "Article 4(7)") — confirmed live
/// against the real GDPR text (Article 4(1) genuinely defines "personal data"; the old bridge row
/// for that unit stored article=1, not 4, purely by numeric coincidence). Matching the old output
/// would have meant re-introducing that bug. What actually needs proving instead: the new
/// per-unit `citation` template correctly threads a paragraph's *parent* article number (not its
/// own ordinal) and correctly distinguishes recital from article — using the real
/// `docs/legal_agent/structure` eu-regulation-en profile, not a synthetic one.
#[tokio::test]
async fn eu_regulation_citation_uses_parent_article_not_own_paragraph_number() {
    let pkg_dir = std::path::Path::new(
        "/Users/xuejingzhoum3/GitHub/soverex/soverex-open/docs/legal_agent/structure",
    );
    let pkg = StructurePackage::from_dir(pkg_dir).expect("load real legal-eu-privacy package");
    let profile = pkg
        .profiles
        .iter()
        .find(|p| p.profile.name == "eu-regulation-en")
        .expect("eu-regulation-en profile");

    // Recital 33 in the preamble (before the first article), then Article 4 whose own paragraph
    // number (7) deliberately differs from the article number (4) — this is exactly the
    // 26/72 buggy-old-paragraph shape (parent-number != own-number), not the 46/72 shape where
    // they coincidentally matched and would have hidden the bug.
    let markdown = "(33) A recital about scientific research purposes.\n\n\
Article 4\n\nDefinitions\n\n\
7. For the purposes of this Regulation, the following applies.\n";

    let identity = derive_identity(
        "doc.test",
        Some("Regulation (EU) 9999/9999"),
        None,
        Some(markdown),
        std::slice::from_ref(&pkg),
        None,
    )
    .identity;

    let parsed = parse_document(
        &pkg.manifest.package.name,
        profile,
        "doc.test",
        &identity,
        markdown,
    )
    .expect("parse")
    .expect("units");

    let recital = parsed
        .units
        .iter()
        .find(|u| u.label_norm == "recital.33")
        .expect("recital.33 unit");
    assert_eq!(
        recital.citation, "Regulation (EU) 9999/9999, Recital 33",
        "a recital's citation must say Recital, never Article — this is exactly what the old \
         bridge got wrong for all 284 recitals in the real corpus"
    );

    let paragraph = parsed
        .units
        .iter()
        .find(|u| u.label_norm == "art.4.p7")
        .expect("art.4.p7 unit");
    assert_eq!(
        paragraph.citation, "Regulation (EU) 9999/9999, Article 4(7)",
        "a paragraph's citation must use its PARENT article's number (4), not its own ordinal \
         (7) — the old bridge stored the paragraph's own number in both fields, so this only \
         matched by coincidence when the two happened to be numerically equal"
    );
}

/// Acceptance check for the step-5 cutover (docs/retrieve_guarantee.md item 1): "a test package
/// with a dosage capture surfaces dosage in the locator label end-to-end." The pre-cutover
/// `LegalLocator` (`query/legal.rs`) hardcoded exactly four fields (article/recital/section/
/// paragraph); a package-specific capture name like `dosage` had nowhere to go and was silently
/// dropped before it ever reached `document_search`'s output. This proves the *axonmind* half of
/// the fix: install a synthetic package whose only capture is `dosage`, ingest a document that
/// grammar matches, and confirm `document_search` surfaces `dosage` in the returned `UnitLocator`
/// map. (The *soverex* half — that `grounding_preretrieval::locator_label` renders an arbitrary
/// key like `dosage` into rider text — is covered separately by
/// `locator_label_renders_arbitrary_capture_names_in_key_order` in
/// `crates/soverex_engine/src/acp/grounding_preretrieval.rs`; that test takes exactly the
/// `UnitLocator` shape this test proves `document_search` actually produces, so together they
/// close the loop end-to-end.)
#[tokio::test]
async fn dosage_capture_surfaces_in_document_search_locator() {
    let temp = TempDir::new().expect("tempdir");
    let cfg = test_engine_config(&temp);
    let engine = AxonMindEngine::open(cfg).await.expect("engine");
    let package_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dosage_test");

    engine
        .install_structure_package_from_dir(&package_dir, "standalone")
        .await
        .expect("install dosage-test package");

    let doc_dir = TempDir::new().expect("doc tempdir");
    let doc_path = doc_dir.path().join("dosage_guide_1.md");
    std::fs::write(&doc_path, "Dosage Guide 1\n\nAmoxicillin dosage: 500mg\n").expect("write doc");

    let ingested = engine
        .ingest_file_with_content(&doc_path)
        .await
        .expect("ingest failed");

    let output = engine
        .document_search(DocumentSearchInput {
            query: "Amoxicillin".to_string(),
            doc_ids: Some(vec![ingested.doc_id.clone()]),
            corpus: None,
            unit_types: None,
            top_k: Some(5),
        })
        .await
        .expect("document_search failed");

    let hit = output
        .results
        .iter()
        .find(|r| r.doc_id == ingested.doc_id)
        .expect("expected a hit for the ingested dosage document");
    let locator = hit
        .locator
        .as_ref()
        .expect("hit must carry a locator — the dosage-en profile's only unit kind is 'entry'");
    assert_eq!(
        locator.0.get("dosage").map(String::as_str),
        Some("500mg"),
        "the package-specific 'dosage' capture must survive all the way to document_search's \
         output; before the cutover, LegalLocator's 4 hardcoded fields had no slot for it and it \
         would have been silently dropped here"
    );
    assert!(
        hit.citation_safe,
        "a unit-backed hit from an installed package must be citation_safe"
    );

    // retrieve_guarantee.md item 5: a unit-backed hit must carry the content sha256 of the
    // document it was parsed from, plus the claiming package name/version, so evidence records
    // can reconstruct exactly which document version and package rule produced a citation.
    assert_eq!(
        hit.doc_sha256, ingested.sha256,
        "doc_sha256 must be the content sha256 of the exact document version this unit was \
         parsed from"
    );
    assert_eq!(hit.package_name, "dosage-test");
    assert!(
        hit.package_version >= 1,
        "package_version must reflect the profile version in effect at parse time, not be left \
         at the zero-value default"
    );
}

/// Acceptance check for item 3 in docs/retrieve_guarantee.md: the old gate in
/// `ensure_document_grounding` was `count_doc_units_for_doc > 0 -> skip`, so any document that
/// already had a parse never re-parsed again, even after its claiming profile changed. This bit
/// the live GDPR corpus (the stale-2-unit incident) and needed manual `doc_units` row-clearing to
/// force a re-parse. This test proves the staleness key `(doc_sha256, package_name, profile_name,
/// profile_version)` makes that self-correcting: installing a version-bumped profile with a
/// changed grammar and calling `retro_apply_structure_packages` re-parses the already-ingested
/// document with the *new* grammar, producing different locator captures, with zero manual DB
/// surgery.
#[tokio::test]
async fn bumped_profile_version_forces_reparse_without_manual_clearing() {
    let temp = TempDir::new().expect("tempdir");
    let cfg = test_engine_config(&temp);
    let engine = AxonMindEngine::open(cfg).await.expect("engine");
    let package_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dosage_test");

    engine
        .install_structure_package_from_dir(&package_dir, "standalone")
        .await
        .expect("install dosage-test package v1");

    let doc_dir = TempDir::new().expect("doc tempdir");
    let doc_path = doc_dir.path().join("dosage_guide_1.md");
    std::fs::write(&doc_path, "Dosage Guide 1\n\nAmoxicillin dosage: 500mg\n").expect("write doc");

    let ingested = engine
        .ingest_file_with_content(&doc_path)
        .await
        .expect("ingest failed");

    let output_v1 = engine
        .document_search(DocumentSearchInput {
            query: "Amoxicillin".to_string(),
            doc_ids: Some(vec![ingested.doc_id.clone()]),
            corpus: None,
            unit_types: None,
            top_k: Some(5),
        })
        .await
        .expect("document_search failed (v1)");
    let locator_v1 = output_v1
        .results
        .iter()
        .find(|r| r.doc_id == ingested.doc_id)
        .expect("expected a hit for the ingested dosage document (v1)")
        .locator
        .clone()
        .expect("hit must carry a locator (v1)");
    assert_eq!(locator_v1.0.get("dosage").map(String::as_str), Some("500mg"));
    assert_eq!(locator_v1.0.get("unit"), None, "v1 grammar has no 'unit' capture");

    // Bump the package + profile version and change the grammar: the new marker splits the
    // dosage magnitude and unit into two captures instead of one combined token.
    let package_v2 = std::fs::read_to_string(package_dir.join("package.toml"))
        .expect("read package.toml")
        .replace("version = 1", "version = 2");
    let identity_v2 =
        std::fs::read_to_string(package_dir.join("identity.toml")).expect("read identity.toml");
    let profile_v2 = std::fs::read_to_string(package_dir.join("profiles/dosage-en.toml"))
        .expect("read profile")
        .replace("version = 1", "version = 2")
        .replace(
            r"marker = '(?m)^(\w[\w ]*)\s+dosage:\s*(\S+)'",
            r"marker = '(?m)^(\w[\w ]*)\s+dosage:\s*(\d+)(mg|g|mcg)'",
        )
        .replace(
            "captures = { medication = 1, dosage = 2 }",
            "captures = { medication = 1, dosage = 2, unit = 3 }",
        );

    let mut files_v2 = std::collections::BTreeMap::new();
    files_v2.insert("package.toml".to_string(), package_v2);
    files_v2.insert("identity.toml".to_string(), identity_v2);
    files_v2.insert("profiles/dosage-en.toml".to_string(), profile_v2);

    engine
        .install_structure_package_from_files(&files_v2, "standalone")
        .await
        .expect("install dosage-test package v2");

    engine
        .retro_apply_structure_packages()
        .await
        .expect("retro_apply after version bump");

    let output_v2 = engine
        .document_search(DocumentSearchInput {
            query: "Amoxicillin".to_string(),
            doc_ids: Some(vec![ingested.doc_id.clone()]),
            corpus: None,
            unit_types: None,
            top_k: Some(5),
        })
        .await
        .expect("document_search failed (v2)");
    let locator_v2 = output_v2
        .results
        .iter()
        .find(|r| r.doc_id == ingested.doc_id)
        .expect("expected a hit for the ingested dosage document (v2)")
        .locator
        .clone()
        .expect("hit must carry a locator (v2)");
    assert_eq!(
        locator_v2.0.get("dosage").map(String::as_str),
        Some("500"),
        "re-parse under the v2 grammar must split the dosage magnitude out of the combined \
         '500mg' token the v1 grammar produced — proves the document was actually re-parsed, \
         not served from the stale v1 units"
    );
    assert_eq!(
        locator_v2.0.get("unit").map(String::as_str),
        Some("mg"),
        "the 'unit' capture only exists in the v2 grammar; its presence proves retro_apply ran \
         the new profile against this already-ingested document without any manual row-clearing"
    );
}

/// Acceptance check for item 4c (`retrieve_guarantee.md`): a document misidentified by
/// best-match-by-confidence (mirroring the real Guidelines 9/2022 incident — a lower-confidence
/// rule matching the document's actual profile lost to a higher-confidence rule matching an
/// incidental mention of a different profile) can be corrected entirely via `pin_document_profile`,
/// the correction is visible immediately (no waiting for the next reconcile pass), and it
/// survives a full `retro_apply_structure_packages` re-derivation — proving `pinned_profile`
/// is no longer dead data.
#[tokio::test]
async fn pinned_profile_corrects_misidentification_and_survives_reingestion() {
    let temp = TempDir::new().expect("tempdir");
    let cfg = test_engine_config(&temp);
    let engine = AxonMindEngine::open(cfg).await.expect("engine");
    let package_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");
    engine
        .install_structure_package_from_dir(&package_dir, "standalone")
        .await
        .expect("install generic-manual package");

    let doc_dir = TempDir::new().expect("doc tempdir");
    let doc_path = doc_dir.path().join("ambiguous_doc.md");
    // Title matches two same-tier (first_chars) identity rules at once: "Acme Policy 3"
    // (handbook-en, confidence 0.9) and "Acme Field Procedure 9" (procedure-en, confidence
    // 0.85) — the document is actually a field procedure, but best-match-by-confidence picks
    // "policy". The body's "1.3.6 Escalation steps" marker (same shape as the procedure-en
    // parse fixture) gives the pin something real to re-parse into a unit once forced.
    std::fs::write(
        &doc_path,
        "Acme Policy 3 Acme Field Procedure 9\n\n1.3.6 Escalation steps\n\
         The operator must escalate to the supervisor per Chapter 3.\n",
    )
    .expect("write doc");

    let ingested = engine
        .ingest_file_with_content(&doc_path)
        .await
        .expect("ingest failed");

    let before = engine
        .list_document_identities()
        .await
        .expect("list identities")
        .into_iter()
        .find(|d| d.doc_node_id == ingested.doc_id)
        .expect("identity row exists");
    assert_eq!(
        before.instrument_type.as_deref(),
        Some("policy"),
        "sanity check: without a pin, the higher-confidence rule wins, misidentifying the doc"
    );

    engine
        .pin_document_profile(&ingested.doc_id, Some("procedure-en"))
        .await
        .expect("pin profile");

    let pinned = engine
        .list_document_identities()
        .await
        .expect("list identities")
        .into_iter()
        .find(|d| d.doc_node_id == ingested.doc_id)
        .expect("identity row exists");
    assert_eq!(
        pinned.instrument_type.as_deref(),
        Some("procedure"),
        "pin must force the correct profile's rule to win, visible immediately"
    );
    assert_eq!(pinned.profile_name.as_deref(), Some("procedure-en"));

    engine
        .retro_apply_structure_packages()
        .await
        .expect("retro_apply after pinning");

    let after_reingest = engine
        .list_document_identities()
        .await
        .expect("list identities")
        .into_iter()
        .find(|d| d.doc_node_id == ingested.doc_id)
        .expect("identity row exists");
    assert_eq!(
        after_reingest.instrument_type.as_deref(),
        Some("procedure"),
        "the pin must survive a full re-derivation pass, not just the immediate pin call"
    );
}

#[tokio::test]
async fn installs_structure_package_from_in_memory_files() {
    // Mirrors how a production caller (e.g. Soverex skill_files) installs a
    // package carried as DB rows rather than a filesystem directory.
    let temp = TempDir::new().expect("tempdir");
    let cfg = test_engine_config(&temp);
    let engine = AxonMindEngine::open(cfg).await.expect("engine");
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");

    let mut files = std::collections::BTreeMap::new();
    for name in ["package.toml", "identity.toml", "corpus.toml"] {
        files.insert(
            name.to_string(),
            std::fs::read_to_string(dir.join(name)).expect("read"),
        );
    }
    for entry in std::fs::read_dir(dir.join("profiles")).expect("read profiles dir") {
        let entry = entry.expect("dir entry");
        let file_name = entry.file_name().into_string().expect("utf8 filename");
        files.insert(
            format!("profiles/{file_name}"),
            std::fs::read_to_string(entry.path()).expect("read profile"),
        );
    }

    let report = engine
        .install_structure_package_from_files(&files, "skill:acme-ops")
        .await
        .expect("install");
    assert_eq!(report.package_name, "generic-manual");
    assert_eq!(report.profiles, 3);

    let packages = engine.list_structure_packages().await.expect("list");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].sources, vec!["skill:acme-ops".to_string()]);
}

/// Cleanup for the "document_search GDPR special-case" gap flagged in docs/retrieve_guarantee.md:
/// `document_search`'s ambiguous-cross-reference resolution used to hardcode
/// `source_unit.profile_name == "edpb-guidance-en"` plus a `corpus == "gdpr"` /
/// `canonical_title.contains("2016/679")` text search — domain vocabulary baked into axonmind's
/// "generic" code, the same category of issue item 1 removed elsewhere. The real
/// `docs/legal_agent/structure/corpus.toml` already declares this exact rule generically via
/// `[[xref]] from = { corpus = "gdpr", instrument_type = "guidelines" } to = { corpus = "gdpr",
/// canonical_title = 'Regulation \(EU\) 2016/679' } kinds = ["article", "recital"]` — it was
/// parsed and stored but never consulted by any retrieval code path (the same "write-only xref
/// graph" gap named in the doc's Enhancement #6). This test ingests a real regulation doc and a
/// real guidance doc side by side, searches only the guidance doc for text that cites "Article
/// 33" (present in the regulation, absent from the guidance doc itself), and confirms
/// `document_search` follows the cross-reference to the regulation doc — driven entirely by the
/// corpus.toml declaration, with no profile name or corpus string hardcoded in the resolution
/// path anymore.
#[tokio::test]
async fn ambiguous_xref_resolves_via_declared_corpus_rule_not_hardcoded_profile() {
    let temp = TempDir::new().expect("tempdir");
    let cfg = test_engine_config(&temp);
    let engine = AxonMindEngine::open(cfg).await.expect("engine");
    let package_dir = std::path::Path::new(
        "/Users/xuejingzhoum3/GitHub/soverex/soverex-open/docs/legal_agent/structure",
    );
    engine
        .install_structure_package_from_dir(package_dir, "standalone")
        .await
        .expect("install real legal-eu-privacy package");

    let doc_dir = TempDir::new().expect("doc tempdir");

    let regulation_path = doc_dir.path().join("regulation.md");
    std::fs::write(
        &regulation_path,
        "Regulation (EU) 2016/679 of the European Parliament and of the Council\n\n\
Article 33\n\nNotification of a personal data breach to the supervisory authority\n\n\
1. In the case of a personal data breach, the controller shall without undue delay notify the \
supervisory authority.\n",
    )
    .expect("write regulation doc");
    let regulation = engine
        .ingest_file_with_content(&regulation_path)
        .await
        .expect("ingest regulation");

    let guidance_path = doc_dir.path().join("guidance.md");
    std::fs::write(
        &guidance_path,
        "Guidelines 99/2099 on data breach notification\n\n\
1.1 Scope\n\nThis guidance discusses the reporting obligations under Article 33 of the \
Regulation.\n",
    )
    .expect("write guidance doc");
    let guidance = engine
        .ingest_file_with_content(&guidance_path)
        .await
        .expect("ingest guidance");

    let output = engine
        .document_search(DocumentSearchInput {
            query: "reporting obligations".to_string(),
            doc_ids: Some(vec![guidance.doc_id.clone()]),
            corpus: None,
            unit_types: None,
            top_k: Some(5),
        })
        .await
        .expect("document_search failed");

    let direct_hit = output
        .results
        .iter()
        .find(|r| r.doc_id == guidance.doc_id)
        .expect("expected a direct hit in the guidance document itself");
    assert_eq!(direct_hit.score_source, "bm25");

    let cross_ref_hit = output
        .results
        .iter()
        .find(|r| r.score_source == "cross_reference")
        .expect(
            "expected the Article 33 reference in the guidance doc to resolve via the \
             corpus.toml [[xref]] rule",
        );
    assert_eq!(
        cross_ref_hit.doc_id, regulation.doc_id,
        "the ambiguous 'Article 33' reference must resolve to the regulation doc, not stay \
         self-referential to the guidance doc — this is exactly what the old hardcoded \
         profile-name check did"
    );
    let locator = cross_ref_hit
        .locator
        .as_ref()
        .expect("cross-reference hit must carry a locator");
    assert_eq!(locator.0.get("article").map(String::as_str), Some("33"));
    assert!(cross_ref_hit.citation_safe);
}
