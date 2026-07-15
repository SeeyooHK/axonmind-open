-- Migration 16: currency & supersession fields on document_identity (retrieve_guarantee.md item 11).
-- Closes residual gap 2a: nothing told the user the ingested text is still the law. Package-declared
-- data only (corpus.toml `[[bind]]` status/as_of/superseded_by), never derived by code — see
-- structure/identity.rs's apply_corpus_bindings.
--
-- Default is 'unknown'/NULL, not a blanket "in_force" guess — mirrors migration 015's backfill
-- lesson (item 10): a package with no currency declarations must behave exactly as today, and
-- 'unknown' must stay silent (no warning rendered), matching this item's own acceptance criterion.
ALTER TABLE document_identity ADD COLUMN status TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE document_identity ADD COLUMN as_of TEXT;
ALTER TABLE document_identity ADD COLUMN superseded_by TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (16);
