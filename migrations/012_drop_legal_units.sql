-- 012_drop_legal_units.sql — retire the hardcoded legal_units/legal_unit_refs tables now that
-- the store layer writes generic parsed units into doc_units/doc_unit_locators/doc_unit_refs
-- (created by 011_structure_packages.sql, previously unused). Dropping is safe: any document
-- that had legal_units rows will have 0 doc_units rows after this runs, and the existing
-- reconcile pass (retro_apply_structure_packages, already wired at startup) re-parses every
-- document automatically the next time the engine opens.
INSERT OR IGNORE INTO schema_version (version) VALUES (12);

DROP TABLE IF EXISTS legal_unit_refs;
DROP TABLE IF EXISTS legal_units;
