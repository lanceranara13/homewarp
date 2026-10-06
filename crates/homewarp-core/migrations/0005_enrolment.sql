-- Enrolling a VPS (PLAN.md §5.5), and more ports than one for a server.

-- The Gate's row is made anew. Until now only the lab had ever filled it, with
-- keys typed in by its script, so there is nothing to carry over.
DROP TABLE gate;
CREATE TABLE gate (
    id                 INTEGER PRIMARY KEY CHECK (id = 1),
    address            TEXT    NOT NULL,  -- the VPS's public address: what players type
    wg_port            INTEGER NOT NULL,  -- where its WireGuard listens, which home dials
    api_port           INTEGER NOT NULL,  -- where it answers, on its tunnel address
    private_key        TEXT    NOT NULL,  -- home's own
    gate_public_key    TEXT    NOT NULL,
    preshared_key      TEXT    NOT NULL,
    token              TEXT    NOT NULL,  -- what the Gate asks of whoever talks to it
    -- What the Gate will switch to once it is told that home has them: a key of
    -- its own, a new preshared key and a new token. Null but while keys change.
    next_public_key    TEXT,
    next_preshared_key TEXT,
    next_token         TEXT,
    -- Until the Gate has keys of its own: the token a VPS is enrolled with, and
    -- when it stops counting. Both null from then on.
    join_token         TEXT,
    join_expires_at    INTEGER,
    mode               TEXT    NOT NULL CHECK (mode IN ('transparent', 'nat')),
    -- Whether servers see their players' own addresses, as the self-probe found
    -- it, and what it found if that needs saying.
    checked            TEXT    NOT NULL DEFAULT 'unchecked'
                       CHECK (checked IN ('preserved', 'hidden', 'unchecked')),
    note               TEXT,
    created_at         INTEGER NOT NULL
) STRICT;

-- Which protocol a server's port speaks, and the further ports it has: voice
-- chat, a query port. The further ones as JSON, [{"port": 24454, "protocol": "udp"}].
ALTER TABLE servers ADD COLUMN protocol TEXT NOT NULL DEFAULT 'both'
    CHECK (protocol IN ('tcp', 'udp', 'both'));
ALTER TABLE servers ADD COLUMN ports TEXT NOT NULL DEFAULT '[]';
