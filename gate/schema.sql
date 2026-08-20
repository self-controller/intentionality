-- Applied once by store.init(). journal_mode is persistent; set at creation.
PRAGMA journal_mode = WAL;

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO meta (key, value) VALUES ('schema_version', '1');

CREATE TABLE session (
    id               INTEGER PRIMARY KEY,
    started_at       TEXT NOT NULL,      -- UTC, ISO-8601
    ended_at         TEXT,               -- NULL while open or unclosed
    close_reason     TEXT CHECK (close_reason IN ('clean', 'recovered')),
    statement        TEXT NOT NULL DEFAULT '',
    intended_minutes INTEGER,            -- NULL = open-ended, deliberately
    mode             TEXT NOT NULL CHECK (mode IN ('ai', 'manual'))
);

CREATE TABLE task (
    id          INTEGER PRIMARY KEY,
    session_id  INTEGER NOT NULL REFERENCES session(id),
    title       TEXT NOT NULL,
    position    INTEGER NOT NULL,        -- display order within the session
    status      TEXT NOT NULL DEFAULT 'planned'
                CHECK (status IN ('planned', 'done', 'dropped')),
    source      TEXT NOT NULL DEFAULT 'gate'
                CHECK (source IN ('gate', 'mid-session')),
    created_at  TEXT NOT NULL,
    resolved_at TEXT
);
