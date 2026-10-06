-- A template is kept whole, as the JSON document the importer made of an egg
-- (PLAN.md §5.6): nothing is ever asked of a part of one, so nothing is gained
-- by spreading it over columns. The egg itself is kept beside it, so that a
-- better importer can read it again.
CREATE TABLE templates (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL UNIQUE COLLATE NOCASE,  -- the document's name, here to be unique and sorted by
    definition TEXT    NOT NULL,                        -- JSON
    source     TEXT    NOT NULL,                        -- the egg as it was imported
    created_at INTEGER NOT NULL
) STRICT;
