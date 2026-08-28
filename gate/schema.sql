-- Schema v2, applied in full by store.init() on a fresh database only.
-- Existing databases are upgraded by the migrations in store.py; this file
-- must always describe the same end state those migrations produce.
-- journal_mode is persistent; set at creation.
PRAGMA journal_mode = WAL;

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO meta (key, value) VALUES ('schema_version', '2');

CREATE TABLE session (
    id               INTEGER PRIMARY KEY,
    started_at       TEXT NOT NULL,      -- UTC, ISO-8601
    ended_at         TEXT,               -- NULL while open or unclosed
    close_reason     TEXT CHECK (close_reason IN ('clean', 'recovered')),
    statement        TEXT NOT NULL DEFAULT '',
    intended_minutes INTEGER,            -- NULL = open-ended, deliberately
    mode             TEXT NOT NULL CHECK (mode IN ('ai', 'manual')),
    last_heartbeat   TEXT                -- written ~60s by the desktop app;
                                         -- becomes ended_at on recovery
);

CREATE TABLE task (
    id           INTEGER PRIMARY KEY,
    session_id   INTEGER REFERENCES session(id),  -- NULL = in the backlog
    title        TEXT NOT NULL,
    position     INTEGER NOT NULL,       -- display order within its board
    status       TEXT NOT NULL DEFAULT 'planned'
                 CHECK (status IN ('planned', 'doing', 'done', 'dropped')),
    source       TEXT NOT NULL DEFAULT 'gate'
                 CHECK (source IN ('gate', 'mid-session')),
    -- Carry chain: closing a session inserts a NEW backlog row for each
    -- unfinished task, pointing back at the session row it copies. Session
    -- rows are immutable history and are never moved to the backlog.
    carried_from INTEGER REFERENCES task(id) ON DELETE SET NULL,
    created_at   TEXT NOT NULL,
    started_at   TEXT,                   -- first time the task entered 'doing'
    resolved_at  TEXT
);

-- A task can be carried into the backlog at most once, whatever the close
-- paths do: carry is INSERT OR IGNORE against this index.
CREATE UNIQUE INDEX task_carried_once
    ON task (carried_from) WHERE carried_from IS NOT NULL;

CREATE TABLE analysis (
    id            INTEGER PRIMARY KEY,
    session_id    INTEGER NOT NULL REFERENCES session(id),
    created_at    TEXT NOT NULL,
    window_start  TEXT NOT NULL,         -- the stretch this analysis covers
    window_end    TEXT NOT NULL,
    headline      TEXT NOT NULL,
    alignment     INTEGER,               -- 0-100; NULL if the model declined
    body          TEXT NOT NULL,
    observed_json TEXT NOT NULL,         -- the AW breakdown that fed it
    seen_at       TEXT                   -- NULL until opened; the unread badge
);
