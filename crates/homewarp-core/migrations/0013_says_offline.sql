-- A stopped server that says so (PLAN.md §11, Phase 7): where its owner asks
-- for it, something small listens on a stopped Minecraft server's port and
-- tells the game's list, and a player who joins, that it is offline.
ALTER TABLE servers ADD COLUMN says_offline INTEGER NOT NULL DEFAULT 0;
