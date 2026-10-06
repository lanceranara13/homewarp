-- Passkeys (PLAN.md §11, Phase 5): keys that stay on the devices they were made
-- on. What is kept of one is its public half, which is of no use to anyone who
-- reads this file.
CREATE TABLE passkeys (
    id            INTEGER PRIMARY KEY,
    user_id       INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- What the device calls the key. A browser sends it back to say which signed.
    credential_id BLOB    NOT NULL UNIQUE,
    -- A point on the P-256 curve, written out in full: 65 bytes.
    public_key    BLOB    NOT NULL,
    -- How many times the device says the key has been used. One that counts,
    -- and then counts no further, has been copied.
    sign_count    INTEGER NOT NULL,
    -- The name the key was made for, which is the only name it signs for.
    rp_id         TEXT    NOT NULL,
    -- What its owner calls it: "Laptop", "Phone".
    name          TEXT    NOT NULL,
    created_at    INTEGER NOT NULL,
    last_used_at  INTEGER
) STRICT;

CREATE INDEX passkeys_by_user ON passkeys (user_id);
