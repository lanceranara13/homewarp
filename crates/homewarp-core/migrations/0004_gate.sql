-- The Gate this home is connected to, and the keys of the tunnel to it. There is
-- one: the check keeps it so until more than one is something Homewarp can do.
CREATE TABLE gate (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    address         TEXT    NOT NULL,  -- the VPS's public address: what players type
    wg_port         INTEGER NOT NULL,  -- where its WireGuard listens, which home dials
    private_key     TEXT    NOT NULL,  -- home's own
    gate_public_key TEXT    NOT NULL,
    preshared_key   TEXT    NOT NULL,
    token           TEXT    NOT NULL,  -- what the Gate asks of whoever talks to it
    api_port        INTEGER NOT NULL,  -- where it answers, on its tunnel address
    mode            TEXT    NOT NULL,  -- 'transparent' or 'nat'
    created_at      INTEGER NOT NULL
) STRICT;
