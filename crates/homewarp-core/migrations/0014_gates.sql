-- More than one VPS at once (PLAN.md §11, Phase 8): a tunnel to each, and for
-- each server the one it is reached through.

-- The one Gate there could be becomes the first of several. Its tunnel is the
-- first, which is the one it has had all along, so nothing changes for it.
CREATE TABLE gates (
    id                 INTEGER PRIMARY KEY,
    -- Which of home's tunnels leads to it: the number in the name of its
    -- interface, and what its addresses inside the tunnel follow from.
    tunnel             INTEGER NOT NULL UNIQUE CHECK (tunnel BETWEEN 0 AND 7),
    name               TEXT    NOT NULL,  -- what its owner calls it
    address            TEXT    NOT NULL,  -- the VPS's public address: what players type
    wg_port            INTEGER NOT NULL,  -- where its WireGuard listens, which home dials
    api_port           INTEGER NOT NULL,  -- where it answers, on its tunnel address
    private_key        TEXT    NOT NULL,  -- home's own, for this tunnel
    gate_public_key    TEXT    NOT NULL,
    preshared_key      TEXT    NOT NULL,
    token              TEXT    NOT NULL,  -- what the Gate asks of whoever talks to it
    next_public_key    TEXT,
    next_preshared_key TEXT,
    next_token         TEXT,
    join_token         TEXT,
    join_expires_at    INTEGER,
    mode               TEXT    NOT NULL CHECK (mode IN ('transparent', 'nat')),
    checked            TEXT    NOT NULL DEFAULT 'unchecked'
                       CHECK (checked IN ('preserved', 'hidden', 'unchecked')),
    note               TEXT,
    created_at         INTEGER NOT NULL
) STRICT;

INSERT INTO gates
    (id, tunnel, name, address, wg_port, api_port, private_key, gate_public_key, preshared_key,
     token, next_public_key, next_preshared_key, next_token, join_token, join_expires_at, mode,
     checked, note, created_at)
SELECT id, 0, address, address, wg_port, api_port, private_key, gate_public_key, preshared_key,
       token, next_public_key, next_preshared_key, next_token, join_token, join_expires_at, mode,
       checked, note, created_at
FROM gate;
DROP TABLE gate;

-- The VPS a server is reached through. None while no VPS is connected: the
-- server is then reached on the home network only, and takes the first VPS
-- that is connected.
ALTER TABLE servers ADD COLUMN gate_id INTEGER REFERENCES gates (id) ON DELETE SET NULL;
UPDATE servers SET gate_id = (SELECT id FROM gates WHERE join_token IS NULL);

-- What went through a port is counted by the Gate it went through.
CREATE TABLE traffic_by_gate (
    gate_id  INTEGER NOT NULL REFERENCES gates (id) ON DELETE CASCADE,
    port     INTEGER NOT NULL,
    protocol TEXT    NOT NULL CHECK (protocol IN ('tcp', 'udp')),
    hour     INTEGER NOT NULL,  -- Unix seconds divided by 3600
    bytes    INTEGER NOT NULL,
    PRIMARY KEY (gate_id, port, protocol, hour)
) STRICT, WITHOUT ROWID;
INSERT INTO traffic_by_gate (gate_id, port, protocol, hour, bytes)
SELECT gates.id, traffic.port, traffic.protocol, traffic.hour, traffic.bytes
FROM traffic JOIN gates ON gates.join_token IS NULL;
DROP TABLE traffic;
ALTER TABLE traffic_by_gate RENAME TO traffic;
