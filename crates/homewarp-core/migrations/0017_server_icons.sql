-- A server's icon: a small picture its owner gives it, to tell it from the
-- others at a glance. A PNG and nothing else, made by the page from whatever
-- picture was chosen. In a table of its own, so that reading what servers there
-- are does not read their pictures with them. A server with no row here is
-- shown with the first letter of its name.
CREATE TABLE server_icons (
    server_id INTEGER PRIMARY KEY REFERENCES servers (id) ON DELETE CASCADE,
    png       BLOB    NOT NULL,
    tag       TEXT    NOT NULL   -- says which picture it is: the address it is fetched from changes with it
) STRICT;
