-- 014_doc_units_staleness_key.sql — add the document-content half of the staleness key.
-- doc_units already carried package_name/profile_name/profile_version per unit; this adds
-- doc_sha256 so a re-parse can be triggered when the document's content changed even if the
-- claiming package/profile didn't (previously only a package/profile edit could be detected,
-- and only via a full content_sha bump on the package itself).
INSERT OR IGNORE INTO schema_version (version) VALUES (14);

ALTER TABLE doc_units ADD COLUMN doc_sha256 TEXT NOT NULL DEFAULT '';
