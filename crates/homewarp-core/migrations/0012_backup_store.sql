-- A store elsewhere for backups (PLAN.md §11, Phase 7): each backup that is
-- made is copied to a bucket of the owner's, where there is one. Which bucket,
-- and the key to it, are in `settings`. What a backup's copy came to is here.
ALTER TABLE backups ADD COLUMN stored TEXT;          -- null where no copy was asked for; else 'copying', 'copied' or 'failed'
ALTER TABLE backups ADD COLUMN stored_key TEXT;      -- what the copy is called in the bucket, to take it away by
ALTER TABLE backups ADD COLUMN stored_problem TEXT;  -- why it was not copied, if it was not
