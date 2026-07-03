BEGIN;

CREATE TABLE IF NOT EXISTS document_ingest_status (
    source_path    TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    content_sha256 TEXT,
    job_id         TEXT NOT NULL,
    status         TEXT NOT NULL,
    phase          TEXT NOT NULL,
    error          TEXT,
    started_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    trashed_at     INTEGER
);

CREATE INDEX IF NOT EXISTS idx_document_ingest_status_trashed_at
    ON document_ingest_status(trashed_at);

CREATE TABLE IF NOT EXISTS document_trash (
    source_path  TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    head_sha256  TEXT NOT NULL,
    trashed_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS document_trash_blob (
    source_path TEXT NOT NULL REFERENCES document_trash(source_path) ON DELETE CASCADE,
    sha256      TEXT NOT NULL,
    PRIMARY KEY (source_path, sha256)
);

CREATE INDEX IF NOT EXISTS idx_document_trash_trashed_at
    ON document_trash(trashed_at);

INSERT INTO schema_version(version) VALUES (8);

COMMIT;
