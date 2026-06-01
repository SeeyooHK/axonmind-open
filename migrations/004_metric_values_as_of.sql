-- Phase 11 (open): add business reporting timestamp hook for metric values.
-- Existing rows keep NULL as_of; resolver falls back to observed_at.
ALTER TABLE metric_values ADD COLUMN as_of INTEGER;
CREATE INDEX IF NOT EXISTS idx_metric_kpi_as_of_observed
  ON metric_values(kpi_node_id, as_of DESC, observed_at DESC);
INSERT OR IGNORE INTO schema_version (version) VALUES (4);
