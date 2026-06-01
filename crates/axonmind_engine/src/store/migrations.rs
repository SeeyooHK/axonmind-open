use axonmind_core::AxonMindError;
/// Runs all SQL migrations on a raw rusqlite connection.
/// Called once on `GraphDb::new` inside a `conn.interact` closure.
/// SQL lives in `migrations/001_initial.sql` and `002_structural_sha256.sql`.
///
/// Each migration is guarded by a version check so re-opening an existing DB
/// only applies the migrations that have not yet run. `ALTER TABLE` statements
/// are not idempotent, so skipping already-applied migrations is required.
use rusqlite::Connection;

const MIGRATION_001: &str = include_str!("../../../../migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../../../../migrations/002_structural_sha256.sql");
const MIGRATION_003: &str = include_str!("../../../../migrations/003_generations.sql");
const MIGRATION_004: &str = include_str!("../../../../migrations/004_metric_values_as_of.sql");
const MIGRATION_005: &str = include_str!("../../../../migrations/005_llm_review_flag_backfill.sql");
const MIGRATION_006: &str = include_str!("../../../../migrations/006_page_index.sql");

pub fn run_migrations(conn: &Connection) -> Result<(), AxonMindError> {
    conn.execute_batch(MIGRATION_001)
        .map_err(|e| AxonMindError::Database(format!("migration 001 failed: {e}")))?;

    let max_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .map_err(|e| AxonMindError::Database(e.to_string()))?;

    if max_version < 2 {
        conn.execute_batch(MIGRATION_002)
            .map_err(|e| AxonMindError::Database(format!("migration 002 failed: {e}")))?;
    }

    if max_version < 3 {
        conn.execute_batch(MIGRATION_003)
            .map_err(|e| AxonMindError::Database(format!("migration 003 failed: {e}")))?;
    }

    if max_version < 4 {
        conn.execute_batch(MIGRATION_004)
            .map_err(|e| AxonMindError::Database(format!("migration 004 failed: {e}")))?;
    }
    if max_version < 5 {
        conn.execute_batch(MIGRATION_005)
            .map_err(|e| AxonMindError::Database(format!("migration 005 failed: {e}")))?;
    }

    if max_version < 6 {
        conn.execute_batch(MIGRATION_006)
            .map_err(|e| AxonMindError::Database(format!("migration 006 failed: {e}")))?;
    }

    Ok(())
}
