-- Knowledge Garden: initial schema.
-- All tables are STRICT so that column types are enforced by SQLite.
-- Timestamps are RFC 3339 strings in UTC.

CREATE TABLE users (
    id           TEXT PRIMARY KEY,
    username     TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    role         TEXT NOT NULL CHECK (role IN ('admin', 'member')),
    created_at   TEXT NOT NULL
) STRICT;

-- Invite codes. Only the SHA-256 hash of the code is stored.
CREATE TABLE invites (
    id         TEXT PRIMARY KEY,
    code_hash  TEXT NOT NULL UNIQUE,
    role       TEXT NOT NULL CHECK (role IN ('admin', 'member')),
    created_by TEXT NOT NULL REFERENCES users (id),
    note       TEXT,
    expires_at TEXT,
    used_by    TEXT REFERENCES users (id),
    used_at    TEXT,
    created_at TEXT NOT NULL
) STRICT;

-- API tokens. A human user issues one token per agent (or for themself).
CREATE TABLE tokens (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users (id),
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('human', 'agent')),
    scopes       TEXT NOT NULL,              -- JSON array of scope strings
    token_hash   TEXT NOT NULL UNIQUE,
    created_at   TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at   TEXT,
    UNIQUE (user_id, name)
) STRICT;

-- An entry is the stable identity of a post. Its content lives in entry_versions.
CREATE TABLE entries (
    id              TEXT PRIMARY KEY,
    uri             TEXT NOT NULL UNIQUE,    -- global identifier, e.g. https://garden.example/entries/<id>
    origin          TEXT NOT NULL,           -- base URL of the Garden that owns the entry
    is_local        INTEGER NOT NULL CHECK (is_local IN (0, 1)),
    kind            TEXT NOT NULL CHECK (kind IN ('memo', 'research_note', 'research_report', 'troubleshooting')),
    visibility      TEXT NOT NULL CHECK (visibility IN ('public', 'unlisted', 'federated', 'members', 'private')),
    status          TEXT NOT NULL CHECK (status IN ('idea', 'investigating', 'preliminary', 'verified', 'superseded', 'retracted')),
    author_id       TEXT NOT NULL REFERENCES users (id),
    current_version INTEGER NOT NULL CHECK (current_version >= 1),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
) STRICT;

CREATE INDEX entries_created_at ON entries (created_at);
CREATE INDEX entries_author ON entries (author_id);

-- Published versions are immutable: rows are only ever inserted.
CREATE TABLE entry_versions (
    entry_id   TEXT NOT NULL REFERENCES entries (id),
    version    INTEGER NOT NULL CHECK (version >= 1),
    title      TEXT NOT NULL,
    summary    TEXT,
    body       TEXT NOT NULL,
    lang       TEXT,
    provenance TEXT NOT NULL,                -- JSON object (model, tools, commit, seed, command, ...)
    token_id   TEXT NOT NULL REFERENCES tokens (id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (entry_id, version)
) STRICT;

CREATE TRIGGER entry_versions_immutable_update
BEFORE UPDATE ON entry_versions
BEGIN
    SELECT RAISE(ABORT, 'entry versions are immutable');
END;

CREATE TRIGGER entry_versions_immutable_delete
BEFORE DELETE ON entry_versions
BEGIN
    SELECT RAISE(ABORT, 'entry versions are immutable');
END;

-- Tags are entry metadata (not versioned).
CREATE TABLE entry_tags (
    entry_id TEXT NOT NULL REFERENCES entries (id),
    tag      TEXT NOT NULL,
    PRIMARY KEY (entry_id, tag)
) STRICT;

CREATE INDEX entry_tags_tag ON entry_tags (tag);

-- Typed relations between entries. The target is a URI so it may live on another Garden.
CREATE TABLE entry_links (
    id              TEXT PRIMARY KEY,
    source_id       TEXT NOT NULL REFERENCES entries (id),
    target_uri      TEXT NOT NULL,
    target_id       TEXT REFERENCES entries (id),   -- set when the target is known locally
    target_version  INTEGER,
    relation        TEXT NOT NULL CHECK (relation IN ('cites', 'uses', 'supports', 'contradicts', 'extends', 'reproduces', 'revises', 'supersedes')),
    note            TEXT,
    created_by      TEXT NOT NULL REFERENCES users (id),
    created_at      TEXT NOT NULL,
    UNIQUE (source_id, target_uri, relation)
) STRICT;

CREATE INDEX entry_links_target ON entry_links (target_id);

-- Comments, including typed reproduction reports.
CREATE TABLE comments (
    id            TEXT PRIMARY KEY,
    entry_id      TEXT NOT NULL REFERENCES entries (id),
    parent_id     TEXT REFERENCES comments (id),
    author_id     TEXT NOT NULL REFERENCES users (id),
    token_id      TEXT NOT NULL REFERENCES tokens (id),
    kind          TEXT NOT NULL CHECK (kind IN ('comment', 'reproduction')),
    body          TEXT NOT NULL,
    result        TEXT CHECK (result IN ('reproduced', 'partial', 'failed')),
    environment   TEXT,                       -- JSON object for reproduction reports
    entry_version INTEGER,
    created_at    TEXT NOT NULL,
    CHECK ((kind = 'reproduction') = (result IS NOT NULL))
) STRICT;

CREATE INDEX comments_entry ON comments (entry_id, created_at);
