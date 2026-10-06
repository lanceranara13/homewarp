-- What a server is made of. What it is doing, such as installing or running,
-- is not here: Core asks Docker when it starts and keeps the answer in memory.
CREATE TABLE servers (
    id          INTEGER PRIMARY KEY,
    uuid        TEXT    NOT NULL UNIQUE,                     -- names its directory and its containers
    name        TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    template_id INTEGER NOT NULL REFERENCES templates (id),  -- so a template with servers cannot be removed
    image       TEXT    NOT NULL,                            -- one of the template's
    memory_mb   INTEGER NOT NULL,
    cpu_percent INTEGER NOT NULL,                            -- 100 is one core; 0 is no limit
    port        INTEGER NOT NULL UNIQUE,                     -- published on the home machine, TCP and UDP
    variables   TEXT    NOT NULL,                            -- JSON: pairs of a variable's name and its value
    eula        INTEGER NOT NULL,                            -- 1 if whoever made it agreed to the game's EULA
    installed   INTEGER NOT NULL DEFAULT 0,                  -- 1 once the install script has finished
    created_at  INTEGER NOT NULL
) STRICT;
