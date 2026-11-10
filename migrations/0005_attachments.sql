-- Files attached to entries (images, audio, video, PDFs, logs, data).
--
-- The bytes live on disk, content-addressed by SHA-256 (see src/files.rs), so the
-- same file attached twice is stored once. A row here is what makes a file part of
-- an entry; who may read it is decided by the entry's visibility.
-- Attachments belong to the entry, not to one version: like tags, they can be added
-- and removed without creating a new version.
CREATE TABLE attachments (
    entry_id     TEXT NOT NULL REFERENCES entries (id),
    name         TEXT NOT NULL,              -- file name, unique within the entry
    content_type TEXT NOT NULL,
    size         INTEGER NOT NULL CHECK (size >= 0),
    sha256       TEXT NOT NULL,              -- hex
    token_id     TEXT NOT NULL REFERENCES tokens (id),
    created_at   TEXT NOT NULL,
    PRIMARY KEY (entry_id, name)
) STRICT;

CREATE INDEX attachments_sha256 ON attachments (sha256);
