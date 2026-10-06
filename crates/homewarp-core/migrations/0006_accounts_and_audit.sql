-- More accounts than one, what each may do with which server, and a record of
-- what was done (PLAN.md §11, Phase 4).

-- The account made at setup owns this Homewarp: it may do everything, and it is
-- the one that makes the others. An account that is not the owner sees only the
-- servers it has been let into.
ALTER TABLE users ADD COLUMN owner INTEGER NOT NULL DEFAULT 0;
UPDATE users SET owner = 1 WHERE id = (SELECT MIN(id) FROM users);

-- Who else is let into a server, and what they may do there beyond looking at it.
CREATE TABLE server_users (
    server_id   INTEGER NOT NULL REFERENCES servers (id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    permissions TEXT    NOT NULL,  -- JSON: ["console", "power", "files", ...]
    PRIMARY KEY (server_id, user_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX server_users_by_user ON server_users (user_id);

-- What was done through the panel, by whom and to which server. A row outlives
-- the account and the server it is about, so it keeps their names as they were
-- and holds on to neither.
CREATE TABLE audit_log (
    id        INTEGER PRIMARY KEY,
    at        INTEGER NOT NULL,
    user_id   INTEGER,           -- null for what nobody signed in did: a sign-in that failed
    username  TEXT    NOT NULL,
    server_id INTEGER,           -- null for what is about no one server
    server    TEXT,              -- its name at the time
    action    TEXT    NOT NULL,  -- 'server.start', 'files.remove', ...
    detail    TEXT    NOT NULL   -- a path, a name, a command: a line at the most
) STRICT;

CREATE INDEX audit_log_by_server ON audit_log (server_id, id);
CREATE INDEX audit_log_by_user ON audit_log (user_id, id);
