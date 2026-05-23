-- Network connectivity tracking. `skopos network watch` records one
-- `network_samples` row per probe tick and maintains one `network_outages`
-- row per contiguous run of unreachable ticks. `skopos network` reads both.

CREATE TABLE IF NOT EXISTS network_samples (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  ts           TEXT NOT NULL,             -- RFC3339 UTC
  status       TEXT NOT NULL,             -- 'ok' | 'degraded' | 'down'
  rtt_ms       REAL NULL,                 -- best avg RTT among responders
  loss_pct     REAL NOT NULL DEFAULT 0,
  sites_ok     INTEGER NOT NULL,
  sites_total  INTEGER NOT NULL,
  iface        TEXT NULL,
  carrier      INTEGER NULL               -- 1 = local link up, 0 = down
);

CREATE INDEX IF NOT EXISTS idx_network_samples_ts ON network_samples(ts);

CREATE TABLE IF NOT EXISTS network_outages (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at    TEXT NOT NULL,
  ended_at      TEXT NULL,                -- NULL while the outage is ongoing
  duration_secs INTEGER NULL,
  down_samples  INTEGER NOT NULL DEFAULT 0,
  cause         TEXT NULL                 -- 'unreachable' | 'dns' | 'no-carrier'
);

CREATE INDEX IF NOT EXISTS idx_network_outages_started ON network_outages(started_at);
