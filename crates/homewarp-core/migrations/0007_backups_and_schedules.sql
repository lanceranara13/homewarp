-- Backups of a server's files, and what a server does by the clock
-- (PLAN.md §11, Phase 4).

-- A backup is one file beside the server's: backups/<server uuid>/<id>.tar.zst
-- under the data directory. The row is made when the backup is begun and says
-- how it went.
CREATE TABLE backups (
    id          INTEGER PRIMARY KEY,
    server_id   INTEGER NOT NULL REFERENCES servers (id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,
    state       TEXT    NOT NULL CHECK (state IN ('running', 'done', 'failed')),
    size_bytes  INTEGER NOT NULL DEFAULT 0,
    problem     TEXT,              -- why it failed, if it did
    created_at  INTEGER NOT NULL,
    finished_at INTEGER
) STRICT;

CREATE INDEX backups_by_server ON backups (server_id, id);
-- One at a time for a server: two would read the same files and fight for the disk.
CREATE UNIQUE INDEX backups_one_at_a_time ON backups (server_id) WHERE state = 'running';

-- How many finished backups of a server are kept. When one more is done, the
-- oldest go. Few by default: a home's disk is not large.
ALTER TABLE servers ADD COLUMN backups_kept INTEGER NOT NULL DEFAULT 3;

-- Something a server does by the clock: a line typed into its console, a start
-- or a stop, a backup, or several of those one after another.
CREATE TABLE schedules (
    id           INTEGER PRIMARY KEY,
    server_id    INTEGER NOT NULL REFERENCES servers (id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    cron         TEXT    NOT NULL,            -- five fields: minute hour day month weekday
    utc_offset   INTEGER NOT NULL,            -- minutes east of UTC: the clock those fields are read on
    enabled      INTEGER NOT NULL DEFAULT 1,
    only_running INTEGER NOT NULL DEFAULT 0,  -- 1 to pass over a time at which the server is not running
    tasks        TEXT    NOT NULL,            -- JSON: [{"action": "command", "command": "say hi", "wait_seconds": 0}, ...]
    next_run_at  INTEGER,                     -- null while it is not enabled
    last_run_at  INTEGER,
    last_result  TEXT,                        -- what the last run came to, in a sentence
    created_at   INTEGER NOT NULL
) STRICT;

CREATE INDEX schedules_by_server ON schedules (server_id);
