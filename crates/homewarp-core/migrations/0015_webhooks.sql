-- Webhooks: several addresses that are told what happens to this Homewarp,
-- each of what its owner chose for it. Before this there was the one, kept
-- among the settings, and told either of what happens by itself or of
-- everything.

CREATE TABLE webhooks (
    id           INTEGER PRIMARY KEY,
    name         TEXT    NOT NULL,  -- what its owner calls it
    url          TEXT    NOT NULL,  -- a secret of the site's making: never given back whole
    -- Told of every line of the Activity page, whatever a later Homewarp adds
    -- to what is written down there.
    everything   INTEGER NOT NULL DEFAULT 0 CHECK (everything IN (0, 1)),
    events       TEXT    NOT NULL,  -- JSON: the names of what it is told of, where not of everything
    last_at      INTEGER,           -- when it was last sent something, in Unix seconds
    last_problem TEXT,              -- why the site did not take that. Nothing if it did
    created_at   INTEGER NOT NULL
) STRICT;

-- The one address there could be becomes the first of several, told of what
-- it was told of: everything, or what happens with nobody at the panel.
INSERT INTO webhooks (name, url, everything, events, last_at, last_problem, created_at)
SELECT 'Notices',
       json_extract(value, '$.url'),
       CASE WHEN json_extract(value, '$.everything') THEN 1 ELSE 0 END,
       '["server.crash","server.gave_up","server.sleep","server.wake","backup.copy_failed","schedule.ran","gate.lost","gate.back","panel.certificate","update.available","update.done","update.failed"]',
       (SELECT json_extract(value, '$.at') FROM settings WHERE key = 'webhook_last'),
       (SELECT json_extract(value, '$.problem') FROM settings WHERE key = 'webhook_last'),
       CAST(strftime('%s', 'now') AS INTEGER)
FROM settings
WHERE key = 'webhook';

DELETE FROM settings WHERE key IN ('webhook', 'webhook_last');
