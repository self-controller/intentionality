-- Schema v4, applied in full by store.init() on a fresh database only.
-- Existing databases are upgraded by the migrations in store.py; this file
-- must always describe the same end state those migrations produce.
-- journal_mode is persistent; set at creation.
PRAGMA journal_mode = WAL;

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO meta (key, value) VALUES ('schema_version', '4');

CREATE TABLE session (
    id                INTEGER PRIMARY KEY,
    started_at        TEXT NOT NULL,      -- UTC, ISO-8601
    ended_at          TEXT,               -- NULL while open or unclosed
    close_reason      TEXT CHECK (close_reason IN ('clean', 'recovered')),
    statement         TEXT NOT NULL DEFAULT '',
    intended_minutes  INTEGER,            -- NULL = open-ended, deliberately
    mode              TEXT NOT NULL CHECK (mode IN ('ai', 'manual')),
    last_heartbeat    TEXT,               -- written ~30s by the desktop app;
                                          -- becomes ended_at on recovery
    -- When the time-up checkpoint fires. Set at commit to started_at +
    -- intended_minutes; NULL means none is pending -- an open-ended session,
    -- one already fired, or one the user acknowledged. "+15 min" writes a new
    -- time here. Living in the store (not in memory) is what makes a
    -- checkpoint that came due while the app was down fire on its next start.
    checkpoint_due_at TEXT
);

CREATE TABLE task (
    id           INTEGER PRIMARY KEY,
    session_id   INTEGER REFERENCES session(id),  -- NULL = in the backlog
    title        TEXT NOT NULL,
    position     INTEGER NOT NULL,       -- display order within its board
    status       TEXT NOT NULL DEFAULT 'planned'
                 CHECK (status IN ('planned', 'doing', 'done', 'dropped')),
    source       TEXT NOT NULL DEFAULT 'gate'
                 CHECK (source IN ('gate', 'mid-session', 'meeting')),
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
    id                  INTEGER PRIMARY KEY,
    session_id          INTEGER NOT NULL REFERENCES session(id),
    created_at          TEXT NOT NULL,
    window_start        TEXT NOT NULL,   -- the stretch this analysis covers
    window_end          TEXT NOT NULL,
    headline            TEXT NOT NULL,
    alignment           INTEGER,         -- 0-100; NULL if the model declined
    body                TEXT NOT NULL,
    observed_json       TEXT NOT NULL,   -- the AW breakdown that fed it
    seen_at             TEXT,            -- NULL until opened; the unread badge
    -- 'check' = the randomized background observation. 'checkpoint' = the one
    -- that fires when the intended time runs out and is shown immediately.
    kind                TEXT NOT NULL DEFAULT 'check'
                        CHECK (kind IN ('check', 'checkpoint')),
    -- The id of an entry in the desktop app's recommendation catalog, never
    -- its text: improving the wording later then applies to every past
    -- checkpoint. NULL on plain checks.
    recommendation_id   TEXT,
    recommendation_note TEXT             -- one sentence tying the advice to
                                         -- what was actually observed
);

-- A meeting the desktop app transcribed: microphone audio captured between an
-- explicit Start and Stop, turned into text a chunk at a time, then summarized.
-- The audio itself is never stored -- each chunk is transcribed and dropped.
CREATE TABLE meeting (
    id           INTEGER PRIMARY KEY,
    session_id   INTEGER REFERENCES session(id),  -- NULL = recorded between sessions
    started_at   TEXT NOT NULL,      -- UTC, ISO-8601
    ended_at     TEXT,               -- NULL while recording, or if it ended badly
    title        TEXT NOT NULL DEFAULT '',  -- the model's, written at summarize time
    summary      TEXT,               -- NULL until the model has run
    key_points   TEXT,               -- JSON array of strings; NULL until then
    -- 'recording' while the microphone is live, 'summarizing' between Stop and
    -- the model's reply, 'done' once notes landed. 'failed' means the notes
    -- never arrived -- but the transcript is still there and still worth reading,
    -- so a failed meeting is displayed, not hidden.
    state        TEXT NOT NULL DEFAULT 'recording'
                 CHECK (state IN ('recording', 'summarizing', 'done', 'failed')),
    error        TEXT                -- why it failed, shown in the UI
);

-- One transcribed chunk of audio. Rows land while the meeting is still running,
-- which is what makes a meeting that ended badly (app killed, machine slept)
-- still hold everything transcribed up to that point.
CREATE TABLE meeting_segment (
    id         INTEGER PRIMARY KEY,
    meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,     -- 0-based; ordering within the meeting
    started_at TEXT NOT NULL,        -- wall clock at the head of this chunk
    text       TEXT NOT NULL         -- '' when that chunk's transcription failed
);

-- Retrying a chunk must not duplicate it.
CREATE UNIQUE INDEX meeting_segment_seq ON meeting_segment (meeting_id, seq);

-- An action item the model pulled out of the transcript. It is only a proposal
-- until the user ticks it; task_id is the record that it became a real task.
CREATE TABLE meeting_action (
    id         INTEGER PRIMARY KEY,
    meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,     -- display order within the meeting
    text       TEXT NOT NULL,
    -- NULL until approved. Non-NULL is the record that this action already
    -- became a backlog task, so approving twice cannot double-insert.
    task_id    INTEGER REFERENCES task(id) ON DELETE SET NULL
);
