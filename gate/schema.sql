-- Schema v10, applied in full by store.init() on a fresh database only.
-- Existing databases are upgraded by the migrations in store.py; this file
-- must always describe the same end state those migrations produce.
-- journal_mode is persistent; set at creation.
PRAGMA journal_mode = WAL;

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO meta (key, value) VALUES ('schema_version', '10');

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
    resolved_at  TEXT,
    -- Plain text the user typed about this task. Never model output, and never
    -- rendered as markdown.
    notes        TEXT NOT NULL DEFAULT '',
    -- The day the task is due, 'YYYY-MM-DD' on the user's own calendar: a
    -- day, not an instant, so it never shifts with a timezone, and the fixed
    -- width makes string order date order. NULL = no due date. Notes and
    -- due_date are last because each arrived by ADD COLUMN, which appends.
    due_date     TEXT
);

-- A task can be carried into the backlog at most once, whatever the close
-- paths do: carry is INSERT OR IGNORE against this index.
CREATE UNIQUE INDEX task_carried_once
    ON task (carried_from) WHERE carried_from IS NOT NULL;

-- Every board, backlog and session-list read filters task by session_id (the
-- session list twice per row, for its label). Added by the v10 migration.
CREATE INDEX task_session ON task (session_id, position);

-- Free-text labels the user invents as they go, shared across tasks so the
-- second card can reuse the first one's tag. A label nothing wears is a
-- preset: it stays, offered to every card, until it is deleted by name from
-- the app's labels panel (task_label rows go with it by cascade).
CREATE TABLE label (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL COLLATE NOCASE UNIQUE,
    color TEXT NOT NULL           -- assigned at creation, stable thereafter
);

CREATE TABLE task_label (
    task_id  INTEGER NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    label_id INTEGER NOT NULL REFERENCES label(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, label_id)
);

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
CREATE INDEX analysis_session ON analysis (session_id);  -- v10

-- A meeting the desktop app transcribed: microphone audio captured between an
-- explicit Start and Stop, turned into text a chunk at a time, then summarized.
-- The audio itself is never stored -- each chunk is transcribed and dropped.
CREATE TABLE meeting (
    id           INTEGER PRIMARY KEY,
    session_id   INTEGER REFERENCES session(id),  -- NULL = recorded between sessions
    started_at   TEXT NOT NULL,      -- UTC, ISO-8601
    ended_at     TEXT,               -- NULL while recording, or if it ended badly
    title        TEXT NOT NULL DEFAULT '',  -- the model's, written at summarize time
    -- What the user typed during the meeting. Never model output. It is the
    -- glossary the notes read names and jargon off, and it is also where a
    -- todo nobody said out loud gets recorded.
    notes        TEXT NOT NULL DEFAULT '',
    -- Unused since the transcript-repair pass was removed: the user now edits
    -- the transcript itself (meeting_segment) before the notes are written.
    -- Kept rather than dropped so older meetings' repaired text is still
    -- recoverable; nothing reads or writes it.
    clean_transcript TEXT,
    -- The write-up: one markdown document. The model drafts it at summarize
    -- time -- an opening paragraph, then '## Key points' and, only when there
    -- is anything, '## Additional information'; fenced code and $-delimited
    -- LaTeX where the meeting had code or maths -- and the user may then edit
    -- it end to end. NULL until the model has run. Through v5 this was a
    -- paragraph beside a JSON key_points column and a details column; the v6
    -- migration folded both in.
    summary      TEXT,
    -- 'recording' while the microphone is live (and until the last chunk has
    -- landed after Stop), then 'done': stopped, with the transcript ready to
    -- review. summary IS NULL there means no notes have been written yet.
    -- 'summarizing' while Write notes runs; 'failed' means the notes never
    -- arrived -- but the transcript is still there and still worth reading,
    -- so a failed meeting is displayed, not hidden. 'cleaning' belonged to
    -- the removed repair pass and is no longer written; the startup sweep
    -- fails any row an older build left there.
    state        TEXT NOT NULL DEFAULT 'recording'
                 CHECK (state IN ('recording', 'cleaning', 'summarizing', 'done', 'failed')),
    error        TEXT,               -- why it failed, shown in the UI
    -- When the user last saved an edit to summary. NULL means the document is
    -- as the model wrote it; finish_meeting clears it, which is what lets the
    -- app ask before a re-run throws hand-written edits away. Added by the v6
    -- migration with ALTER TABLE, which appends -- hence its position here.
    summary_edited_at TEXT,
    -- When the user last edited the transcript, or a resume started growing
    -- it. NULL means the segments are as the current write-up saw them;
    -- non-NULL is exactly the claim "the transcript has changed since these
    -- notes were written", which is what marks the write-up stale and offers
    -- Re-run notes.
    --
    -- finish_meeting clears it only when it still holds the value the run
    -- started from: write_notes snapshots the transcript and then the model
    -- works for minutes, so an edit made during that run was never seen by it
    -- and must keep the mark. Appended by the v9 migration.
    transcript_edited_at TEXT
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

-- A file the user attached as context for one meeting: a deck, a spec, a
-- screenshot. The bytes live on disk under <store dir>/meetings/, never here --
-- the store is shared with the Python gate and has to stay small enough to
-- copy, back up and open in a shell.
CREATE TABLE meeting_file (
    id         INTEGER PRIMARY KEY,
    meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,     -- display order within the meeting
    name       TEXT NOT NULL,        -- the original basename, shown in the UI
    -- The copy this app owns and is free to delete. An opaque filename, never
    -- derived from `name`: the original is display metadata and nothing else.
    path       TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('text', 'pdf', 'image', 'office')),
    bytes      INTEGER NOT NULL,
    -- Text pulled out at attach time, for 'text' and 'office'. NULL for 'pdf'
    -- and 'image': those go to the model natively from the copied file.
    extracted  TEXT,
    added_at   TEXT NOT NULL
);

CREATE INDEX meeting_file_meeting ON meeting_file (meeting_id, position);
