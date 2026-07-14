-- Migration 15: provenance tier per doc_unit (retrieve_guarantee.md item 10).
-- Closes residual gap 1b: a stray auto-ingested transcript was as citable as an official
-- reference document, because citation_safe had no notion of trust, only "was this unit
-- backed by a structure-package parse." Denormalized onto doc_units (not looked up via a
-- join to nodes.attrs) for the same reason doc_sha256/package_name already are: document_search
-- reads this on every candidate unit in its hot path.
--
-- Backfill is best-effort by origin, not blanket 'unknown': email_ingest.rs is the only
-- currently-identifiable auto-capture path (its Document nodes are minted as
-- "doc.email_<id>" — see crates/soverex_engine/src/workers/email_ingest.rs), so those units
-- backfill to 'auto_captured'. Every other existing unit backfills to 'user_upload', since
-- collect_ingest_files was — until this session's ca92679 follow-up fix — the only production
-- ingest path, and it was used exclusively for manual "add to Library" actions. A blanket
-- 'unknown' backfill would rank below the default `min_citation_provenance` (user_upload) and
-- flip the already-live GDPR corpus to citation_safe:false, breaking `gdpr-counsel`'s strict
-- mode — that would violate this item's own acceptance criterion ("existing GDPR corpus keeps
-- working after backfill").
ALTER TABLE doc_units ADD COLUMN provenance TEXT NOT NULL DEFAULT 'user_upload';

UPDATE doc_units SET provenance = 'auto_captured' WHERE doc_node_id GLOB 'doc.email_*';

INSERT OR IGNORE INTO schema_version (version) VALUES (15);
