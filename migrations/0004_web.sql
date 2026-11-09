-- Browser sign-in for humans: passwords and sessions.

-- Argon2id PHC string. NULL means the user cannot sign in with a password
-- (agents' owners who only use tokens, and stand-ins for remote authors).
ALTER TABLE users ADD COLUMN password_hash TEXT;

-- A browser session. Only the SHA-256 hash of the cookie value is stored.
-- Writes made in the browser are attributed to `token_id`, the user's "web" token.
CREATE TABLE sessions (
    id_hash      TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users (id),
    token_id     TEXT NOT NULL REFERENCES tokens (id),
    csrf         TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL
) STRICT;

CREATE INDEX sessions_user ON sessions (user_id);
