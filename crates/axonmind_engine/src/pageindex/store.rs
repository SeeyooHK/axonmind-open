use axonmind_core::AxonMindError;
use rusqlite::OptionalExtension;

use crate::store::GraphDb;

use super::tree::{PersistTree, SectionRow};

pub struct PageIndexStore {
    db: GraphDb,
}

impl PageIndexStore {
    pub fn new(pool: deadpool_sqlite::Pool) -> Self {
        Self { db: GraphDb(pool) }
    }

    /// Replace a document's whole tree atomically: delete old rows + FTS for doc_node_id,
    /// insert page_tree + all page_sections + page_section_fts.
    pub async fn upsert_document(&self, tree: &PersistTree) -> Result<(), AxonMindError> {
        let doc_node_id = tree.doc_node_id.clone();
        let sha256 = tree.sha256.clone();
        let title = tree.title.clone();
        let doc_summary = tree.doc_summary.clone();
        let sections: Vec<SectionRow> = tree.sections.clone();

        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("pageindex get conn: {e}")))?;

        conn.interact(move |conn| {
            let tx = conn.transaction()?;

            // Delete existing FTS entries for this doc.
            tx.execute(
                "DELETE FROM page_section_fts WHERE doc_node_id = ?1",
                rusqlite::params![doc_node_id],
            )?;
            // Delete existing section rows.
            tx.execute(
                "DELETE FROM page_sections WHERE doc_node_id = ?1",
                rusqlite::params![doc_node_id],
            )?;
            // Upsert page_tree row.
            let now = chrono::Utc::now().timestamp();
            tx.execute(
                "INSERT OR REPLACE INTO page_tree (doc_node_id, sha256, title, doc_summary, built_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![doc_node_id, sha256, title, doc_summary, now],
            )?;

            // Insert section rows and FTS entries.
            for row in &sections {
                tx.execute(
                    "INSERT OR REPLACE INTO page_sections
                     (section_id, doc_node_id, parent_section_id, ordinal, level, title, path, summary, text, span_start, span_end)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![
                        row.section_id,
                        row.doc_node_id,
                        row.parent_section_id,
                        row.ordinal,
                        row.level,
                        row.title,
                        row.path,
                        row.summary,
                        row.text,
                        row.span_start,
                        row.span_end,
                    ],
                )?;
                tx.execute(
                    "INSERT INTO page_section_fts (section_id, doc_node_id, title, path, summary, text)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        row.section_id,
                        row.doc_node_id,
                        row.title,
                        row.path,
                        row.summary.as_deref().unwrap_or(""),
                        row.text.as_deref().unwrap_or(""),
                    ],
                )?;
            }

            tx.commit()?;
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("pageindex interact: {e}")))?
        .map_err(|e: rusqlite::Error| AxonMindError::Database(format!("pageindex upsert: {e}")))?;

        Ok(())
    }

    /// Stage 1: BM25 shortlist. Returns section_ids ranked best-first.
    pub async fn bm25_shortlist(
        &self,
        fts_query: &str,
        limit: usize,
    ) -> Result<Vec<String>, AxonMindError> {
        let fts_query = fts_query.to_string();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("pageindex get conn: {e}")))?;

        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT section_id FROM page_section_fts
                 WHERE page_section_fts MATCH ?1
                 ORDER BY bm25(page_section_fts)
                 LIMIT ?2",
            )?;
            let ids: Vec<String> = stmt
                .query_map(rusqlite::params![fts_query, limit as i64], |row| {
                    row.get(0)
                })?
                .filter_map(|r| r.ok())
                .collect();
            Ok::<Vec<String>, rusqlite::Error>(ids)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("pageindex interact: {e}")))?
        .map_err(|e: rusqlite::Error| AxonMindError::Database(format!("pageindex bm25: {e}")))
    }

    /// Fetch full section rows by id.
    pub async fn fetch_sections(
        &self,
        ids: &[String],
    ) -> Result<Vec<SectionRow>, AxonMindError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let ids = ids.to_vec();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("pageindex get conn: {e}")))?;

        conn.interact(move |conn| {
            let placeholders: String = ids
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT section_id, doc_node_id, parent_section_id, ordinal, level, title, path,
                        summary, text, span_start, span_end
                 FROM page_sections
                 WHERE section_id IN ({placeholders})"
            );
            let mut stmt = conn.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::ToSql> =
                ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
            let rows: Vec<SectionRow> = stmt
                .query_map(params.as_slice(), |row| {
                    Ok(SectionRow {
                        section_id: row.get(0)?,
                        doc_node_id: row.get(1)?,
                        parent_section_id: row.get(2)?,
                        ordinal: row.get(3)?,
                        level: row.get(4)?,
                        title: row.get(5)?,
                        path: row.get(6)?,
                        summary: row.get(7)?,
                        text: row.get(8)?,
                        span_start: row.get(9)?,
                        span_end: row.get(10)?,
                    })
                })?
                .filter_map(|r| r.ok())
                .collect();
            Ok::<Vec<SectionRow>, rusqlite::Error>(rows)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("pageindex interact: {e}")))?
        .map_err(|e: rusqlite::Error| AxonMindError::Database(format!("pageindex fetch: {e}")))
    }

    /// Get the stored sha256 for a document (staleness check).
    pub async fn page_tree_sha(
        &self,
        doc_node_id: &str,
    ) -> Result<Option<String>, AxonMindError> {
        let doc_node_id = doc_node_id.to_string();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("pageindex get conn: {e}")))?;

        conn.interact(move |conn| {
            conn.query_row(
                "SELECT sha256 FROM page_tree WHERE doc_node_id = ?1",
                rusqlite::params![doc_node_id],
                |row| row.get(0),
            )
            .optional()
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("pageindex interact: {e}")))?
        .map_err(|e: rusqlite::Error| AxonMindError::Database(format!("pageindex sha: {e}")))
    }

    /// Delete all page_* data for a document (sections, FTS, and page_tree row).
    pub async fn delete_document(&self, doc_node_id: &str) -> Result<(), AxonMindError> {
        let doc_node_id = doc_node_id.to_string();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("pageindex get conn: {e}")))?;

        conn.interact(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "DELETE FROM page_section_fts WHERE doc_node_id = ?1",
                rusqlite::params![doc_node_id],
            )?;
            tx.execute(
                "DELETE FROM page_sections WHERE doc_node_id = ?1",
                rusqlite::params![doc_node_id],
            )?;
            tx.execute(
                "DELETE FROM page_tree WHERE doc_node_id = ?1",
                rusqlite::params![doc_node_id],
            )?;
            tx.commit()?;
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("pageindex interact: {e}")))?
        .map_err(|e: rusqlite::Error| AxonMindError::Database(format!("pageindex delete: {e}")))
    }
}
