-- Sleep (PLAN.md §11, Phase 7): a server that nobody has been on for a while is
-- stopped, and started again when a player joins. It needs a server that says
-- who is on it, which Homewarp asks as a game's list of servers does.
ALTER TABLE servers ADD COLUMN sleep_minutes INTEGER NOT NULL DEFAULT 0;  -- 0 is never
-- 1 while it is stopped for that reason and something listens in its place, so
-- that a Homewarp that starts again finds it so. What it is doing otherwise is
-- not written down.
ALTER TABLE servers ADD COLUMN asleep INTEGER NOT NULL DEFAULT 0;
