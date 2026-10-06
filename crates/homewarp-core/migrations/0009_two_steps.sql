-- A second step for a sign-in (PLAN.md §11, Phase 5): a code from an
-- authenticator app, which changes every half minute.
ALTER TABLE users ADD COLUMN totp_secret TEXT;   -- in base32; null while the second step is off
ALTER TABLE users ADD COLUMN totp_pending TEXT;  -- a secret that has been shown and not yet proven
-- The last half minute a code was taken for. A code is good once, so that one
-- read over a shoulder is of no use a moment later.
ALTER TABLE users ADD COLUMN totp_step INTEGER NOT NULL DEFAULT 0;
-- JSON: the hashes of the recovery codes that have not been used.
ALTER TABLE users ADD COLUMN recovery_codes TEXT NOT NULL DEFAULT '[]';
