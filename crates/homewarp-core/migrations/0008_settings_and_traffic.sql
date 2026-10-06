-- What is set for this Homewarp as a whole, and how much has gone through each
-- forwarded port (PLAN.md §11, Phase 4).

-- One row for each thing that has been set. What has no row is as Homewarp
-- would have it by itself.
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL  -- JSON
) STRICT, WITHOUT ROWID;

-- What the Gate counted through each port, both ways together, an hour at a
-- time. The Gate counts from when it last set its rules; Core adds up what is
-- new each time it asks, so that the count outlives a change of rules.
CREATE TABLE traffic (
    port     INTEGER NOT NULL,
    protocol TEXT    NOT NULL CHECK (protocol IN ('tcp', 'udp')),
    hour     INTEGER NOT NULL,  -- Unix seconds divided by 3600
    bytes    INTEGER NOT NULL,
    PRIMARY KEY (port, protocol, hour)
) STRICT, WITHOUT ROWID;
