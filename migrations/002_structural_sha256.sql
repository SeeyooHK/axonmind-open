-- Migration 2: add structural_sha256 to document_cache for change-classification.
-- NULL on existing rows is intentional — treated as FullReextract until re-indexed.
ALTER TABLE document_cache ADD COLUMN structural_sha256 TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (2);
