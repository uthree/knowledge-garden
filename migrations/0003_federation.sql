-- Federation: this Garden's signing key, approved peers, and mirrored entries.

-- The Garden's ed25519 signing key (exactly one row).
CREATE TABLE garden_keys (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    secret_key TEXT NOT NULL,                -- base64 of the 32-byte seed
    created_at TEXT NOT NULL
) STRICT;

-- Other Gardens an admin has approved (or blocked). Approval is one-sided:
-- each Garden decides on its own whom it serves and whom it mirrors.
CREATE TABLE peers (
    id           TEXT PRIMARY KEY,
    base_url     TEXT NOT NULL UNIQUE,
    name         TEXT NOT NULL,
    public_key   TEXT NOT NULL,              -- base64, pinned when the peer was added
    status       TEXT NOT NULL CHECK (status IN ('approved', 'blocked')),
    cursor       TEXT,                       -- position in the peer's feed
    last_sync_at TEXT,
    last_error   TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
) STRICT;

-- Authors of mirrored entries are stored as users with an origin; they have
-- no tokens of their own and cannot sign in. NULL means a local user.
ALTER TABLE users ADD COLUMN origin TEXT;

-- The origin's signature over the mirrored feed item.
ALTER TABLE entries ADD COLUMN remote_signature TEXT;

-- Whether a local entry was ever shareable with peers (public or federated).
-- Only such entries produce withdrawal notices when they stop being shareable.
ALTER TABLE entries ADD COLUMN ever_shared INTEGER NOT NULL DEFAULT 0 CHECK (ever_shared IN (0, 1));
UPDATE entries SET ever_shared = 1 WHERE visibility IN ('public', 'federated');

CREATE INDEX entries_feed ON entries (is_local, updated_at, id);
