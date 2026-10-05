CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT    NOT NULL,              -- an Argon2id PHC string
    created_at    INTEGER NOT NULL               -- Unix seconds, as every time here
) STRICT;

-- The browser holds a random token; this holds its SHA-256, so that a copy of
-- the database file is not a set of working sessions.
CREATE TABLE sessions (
    token_hash BLOB    PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX sessions_by_user ON sessions (user_id);
