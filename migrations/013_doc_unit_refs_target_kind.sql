-- 013_doc_unit_refs_target_kind.sql — add target_kind to doc_unit_refs so ambiguous
-- cross-reference resolution (resolving a ref whose to_doc_node_id isn't known yet) can filter
-- by the package-declared [[ref]] target_kind (e.g. "article", "recital") directly, instead of
-- guessing it back from target_label_norm's string shape. Backing out document_search's
-- hardcoded profile-name + corpus + canonical_title special case (see
-- docs/retrieve_guarantee.md, 2026-07-09) in favor of consulting corpus.toml's [[xref]] rules
-- generically.
INSERT OR IGNORE INTO schema_version (version) VALUES (13);

ALTER TABLE doc_unit_refs ADD COLUMN target_kind TEXT NOT NULL DEFAULT '';
