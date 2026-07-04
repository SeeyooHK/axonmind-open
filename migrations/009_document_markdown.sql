BEGIN;

CREATE TABLE IF NOT EXISTS document_markdown (
    sha256    TEXT PRIMARY KEY,   -- content hash of the source blob
    markdown  TEXT NOT NULL,      -- render_markdown(&NormalizedDocument) output
    built_at  INTEGER NOT NULL
);

INSERT INTO schema_version(version) VALUES (9);

COMMIT;
