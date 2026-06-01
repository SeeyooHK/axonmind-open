-- Backfill legacy trust flags for concept nodes created under early LLM defaults.
-- Keep taint provenance, but clear blanket review flags on tainted 0.50-confidence
-- non-KPI nodes so review warnings are reserved for explicit low-confidence/conflict paths.
UPDATE nodes
SET requires_human_review = 0
WHERE is_tainted = 1
  AND requires_human_review = 1
  AND ABS(confidence - 0.5) < 0.000001
  AND kind <> 'Kpi';

INSERT OR IGNORE INTO schema_version (version) VALUES (5);
