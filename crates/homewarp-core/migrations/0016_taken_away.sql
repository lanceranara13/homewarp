-- What an account is let do, held to when it is taken away again, and a
-- sign-in that others' wrong tries do not shut (the audit of 2026-10-07).

-- Whose schedule each one is: the account that made it, or changed it last. A
-- schedule does what its tasks say with that account's leave, with nobody at
-- the panel. So when the account is taken out of the server, or loses what a
-- task needs, the schedule is switched off. None for a schedule made before
-- this was kept, and for one whose account is gone.
ALTER TABLE schedules ADD COLUMN user_id INTEGER REFERENCES users (id) ON DELETE SET NULL;

-- The browsers an account has signed in from, each by the hash of a token it
-- keeps in a cookie. Wrong tries at an account, from wherever, make every
-- sign-in at it wait: but not one from a browser that is known, which is
-- counted by itself. Otherwise anybody could keep an account's owner out.
CREATE TABLE known_devices (
    token_hash BLOB    PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
