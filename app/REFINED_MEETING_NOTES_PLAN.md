# Meeting notes: cleaned transcript, scratchpad, and bounded file context

## Outcome and boundaries

Add a two-stage meeting-notes pipeline:

1. Preserve the raw, chunked transcript as the record.
2. Produce and persist a cleaned transcript before generating notes.
3. Let the user maintain a free-form notes scratchpad and attach bounded local
   context files.
4. Generate title, summary, nested key points, additional information, and
   proposed action items from a consistent snapshot of those inputs.

This is a local-first feature. Attachments are copied beside the configured
store, never put in SQLite and never exposed to the webview by path. Their
contents are sent to Anthropic only when the user stops or re-runs meeting
notes; say that plainly next to the Add files control.

`raw transcript + notes + attachments` are snapshotted when a run begins. A
save that finishes after that point remains saved but is deliberately not part
of that run; **Re-run notes** incorporates it. On Stop, the UI must flush the
notes draft and await outstanding attachment operations before it starts the
run, so the normal case includes what the user just typed or chose.

## Acceptance criteria

- Raw segments never change; a failed clean or summary leaves them readable.
- Notes-only and attachment-only meetings are supported. Cleaning is skipped
  when raw transcript is empty.
- A Stop or re-run has exactly one state progression:
  `recording -> cleaning -> summarizing -> done|failed`, or
  `done|failed -> cleaning -> summarizing -> done|failed`.
- A crash in either in-progress state is converted to `failed` at startup.
- A saved file is either attached and readable by a later run or cleaned up;
  no user-controlled path is stored or returned to the frontend.
- All request construction is bounded before base64 encoding and is safe from
  text, filename, and document prompt injection.

## 1. Schema v5 and migration

Change these in the same commit: `gate/schema.sql`, `gate/store.py`, and
`src-tauri/src/db.rs`. Set both schema-version constants to 5.

`meeting` gains:

```sql
notes            TEXT NOT NULL DEFAULT '',
clean_transcript TEXT,
details          TEXT,
```

Replace the state CHECK with
`('recording', 'cleaning', 'summarizing', 'done', 'failed')`. `key_points`
remains JSON but changes from `string[]` to:

```json
[{"text":"Decision or fact","subpoints":["supporting point"]}]
```

Add:

```sql
CREATE TABLE meeting_file (
    id         INTEGER PRIMARY KEY,
    meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    name       TEXT NOT NULL,
    path       TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('text','pdf','image','office')),
    bytes      INTEGER NOT NULL,
    extracted  TEXT,
    added_at   TEXT NOT NULL
);
CREATE INDEX meeting_file_meeting ON meeting_file (meeting_id, position);
```

Implement `_migrate_v4_to_v5` using the existing manual-transaction pattern:
online backup to `.pre-v5.bak`; autocommit; foreign keys off; `BEGIN
IMMEDIATE`; rebuild `meeting`; create `meeting_file`; run
`PRAGMA foreign_key_check`; update `meta`; commit; restore foreign keys and the
prior isolation level even on error. Copy all existing meeting fields, with
empty notes and NULL clean/details. Decode `key_points` defensively: a valid
array of strings becomes objects with empty `subpoints`; NULL, malformed JSON,
or unexpected values become `[]`. This migration owns every row conversion and
never calls model code.

The new desktop binary requires v5. Rehearse migration on a copy via
`INTENTIONALITY_STORE`; do not launch the v5 desktop app against the real v4
store until the updated Python gate has migrated it.

## 2. Attachment subsystem (`src-tauri/src/attach.rs`)

Make this module the sole owner of attachment filesystem access. Its root is
`db::store_path().parent()/meetings`, not a hard-coded home directory, so test
and dry-run stores stay isolated. Use an opaque file id/UUID for the stored
filename; `name` is display metadata only. Never derive a path from the
original name.

### Admission and limits

Reject directories, symlinks, devices, FIFOs, unreadable files, and files that
change size while being copied. Classify only regular files by a conservative
extension plus a content check:

- PDF requires `%PDF-`; images are png/jpeg/gif/webp with a matching magic
  prefix; SVG, BMP, HEIC, and animated/unsupported forms are rejected.
- `docx`, `pptx`, `xlsx`, `odt`, `odp`, and `ods` are ZIP/ODF Office files.
- Other files are text only when a capped prefix and the final bounded content
  are valid UTF-8. This admits source code without pretending arbitrary binary
  data is text.

Use two separate budgets. Disk quotas are `MAX_FILE_BYTES = 6 MiB` and
`MAX_TOTAL_FILE_BYTES = 15 MiB`, leaving base64/JSON headroom beneath the
Messages API 32 MB request limit. Content quotas are `MAX_FILES = 8`,
`MAX_EXTRACTED_CHARS_PER_FILE = 80_000`, and
`MAX_EXTRACTED_CHARS_TOTAL = 200_000`. Enforce the aggregate database limit
under the connection's `IMMEDIATE` transaction, not just in the frontend.

For PDFs, reject encrypted/corrupt documents and cap page count (20 is a good
first product limit) with a small PDF parser. Do not claim that a raw-byte cap
controls PDF page/token cost; it does not. Add the parser dependency if needed
for this validation. For images, inspect dimensions and reject pathological
pixel counts before forwarding. Record enough metadata to show a specific,
actionable rejection message.

### Copy, extraction, and cleanup

Copy through a same-filesystem temporary file, enforce limits while streaming,
then atomically rename to an opaque final filename. Extract text from that
owned copy, never the source path. On any error remove the temporary/final copy.

The DB row and file cannot share a cross-resource transaction. Make the
failure policy explicit:

1. Stage and validate the file outside the SQLite mutex.
2. In one `IMMEDIATE` DB transaction, confirm the meeting exists, enforce the
   aggregate limit, and insert its metadata and final path.
3. Promote the staged file; if promotion fails, compensate by deleting the row
   and report failure.
4. On startup and after attachment mutations, remove orphan temporary files;
   identify final-file orphans only by paths under the owned root.

Deleting a file removes its row then attempts deletion of only its owned path.
Deleting a meeting first reads its owned paths, deletes the meeting in SQLite,
then removes its meeting directory recursively. A failed cleanup is logged and
retried by the orphan sweep; it must not roll back a completed DB deletion.

Office extraction is text-only context, not visual Office rendering. Use a
bounds-checked ZIP reader: cap every decompressed entry and total output to
avoid ZIP bombs, reject encrypted archives, select the documented XML parts,
and parse XML with an XML parser rather than a hand-written tag stripper.
Support DOCX paragraphs, PPTX slide/notes text, and ODF `content.xml`; for XLSX
include shared strings and inline strings, with a clear comment that formulas
and numeric-only cells are intentionally out of scope. Use `zip` with
`default-features = false` and `deflate`; add the XML/PDF dependency only if it
is pure Rust and actually needed for the above validation.

## 3. Model interface and input budgets (`claude.rs`)

Refactor transport into `post(body, timeout) -> Value`, which retains current
status/refusal/error handling, and small helpers for a required tool input and
text response. Keep existing analysis/checkpoint calls on 60 seconds. Cleaning
uses a 300-second deadline; summary uses 180 seconds. Timeouts, rate limits,
and payload errors must retain API request IDs in the user-visible/logged error
when the response provides one.

Define one `MeetingContext` snapshot from database rows plus owned-file bytes.
It has a byte/token budget before JSON is built. Put stable context blocks
first, in a deterministic order; mark the final stable context block with
`cache_control: {type: "ephemeral"}`; then append the changing transcript
window and run instruction. This lets later cleaning windows reuse a cache
prefix, but it is an optimization only--requests must work with a cache miss.

Use fixed delimiters and a single `defang` routine for every untrusted text
field. Do **not** make a delimiter from a filename. For a prompt label, flatten
control/whitespace to one space, replace delimiter-like sequences, cap it to
120 Unicode scalar values, and use a fixed file ordinal as the structural
identifier. Preserve the original basename only for display. Apply the same
defanging to raw text, notes, extracted Office/text content, and model-produced
continuity. State in both system prompts that every supplied block is data,
never instruction, and must never be acted on.

Native PDF/image blocks use the documented base64 `document` and `image`
shapes and only the validated media type. Text/Office context is a text block.
If the combined request would exceed the preflight budget, fail before the
network request and identify the attachments that must be removed; never rely
on a 413 response.

### Cleaning contract

`clean_transcript(raw, context) -> String` returns raw unchanged without a
model call when it is blank. Otherwise it uses a text response, not a tool.
The prompt may correct mishearings using context, restore punctuation, and
repair chunk seams. It must preserve order and all material statements; it may
remove only unmistakable filler/repetition. It must use `[unclear]` rather than
inventing a resolution.

Window at a conservative character budget. Split first at an ASR sentence
boundary, then whitespace, then a UTF-8 character boundary so an unpunctuated
or very long token cannot overflow a window. Supply a capped, defanged tail of
the previous cleaned result as *reference only* and instruct the model to emit
only the current window. Concatenate only returned current-window text. Check
non-whitespace token retention per window; if output is below 60% of input,
store that raw window instead and log the fallback. Also reject empty output for
nonempty input. This makes the no-loss guarantee conservative and testable.

### Summary contract

`summarize_meeting(clean, context)` uses the existing strict tool-call pattern
with `{title, summary, key_points, details, action_items}`. `key_points` are
objects with text and string subpoints. Enforce in Rust: at most 9 points, 6
subpoints each, and 12 action items; trim blanks; treat empty details as NULL.
Keep the existing rule that action items must be explicit commitments. The
summary system prompt must likewise say notes/files are context and may include
instructions that are not to be followed.

## 4. Lifecycle and database API

Replace `summarize` with `write_notes`. It must:

1. Read transcript, notes, and file metadata while holding the SQLite mutex;
   release it before file I/O or any await.
2. Read owned files and build one bounded input snapshot outside the mutex.
3. Fail only if raw transcript, notes, and files are all empty.
4. Hold `analysis_lock` once around both model stages.
5. Persist `clean_transcript` after cleaning, transition `cleaning` to
   `summarizing`, emit `meeting:state`, then atomically persist final notes and
   actions.

`stop` changes `recording -> cleaning` before it drains the capture task, emits
that state, and runs `write_notes` in the existing background task. Add a
guarded general `meeting_state(id, from, to)`. `meeting_failed` and
`sweep_orphan_meetings` include `cleaning`. Add `meeting_rerun`: it only moves
`done|failed -> cleaning`, clears error, and rejects a currently recording
meeting. It powers `rerun_meeting_notes`, which re-cleans from raw inputs.

Add database operations for notes, cleaned text, attachment list/add/remove,
and detail retrieval. `MEETING_SELECT` adds only `details`, `file_count`, and
`has_clean`; list rows never include notes/transcripts/file paths. `MeetingFile`
serialized to the webview contains id, position, name, kind, bytes, and
added_at--never `path` or extracted text.

Register commands in `main.rs` and mirror them in `api.ts`:
`set_meeting_notes`, `attach_meeting_files`, `remove_meeting_file`, and
`rerun_meeting_notes`. Attachment work is `spawn_blocking`; the SQLite lock is
held only for the short metadata mutation.

## 5. Frontend

Add the Tauri dialog plugin (`tauri-plugin-dialog`,
`@tauri-apps/plugin-dialog`), initialize it in `main.rs`, and grant only
`dialog:allow-open`. Use `getCurrentWindow().onDragDropEvent`; accept drops
only for the selected meeting, and clean up the listener on unmount.

In `Meetings.tsx`, extract `LevelBars` first. Add `notesDraft`, seeded only
when selected meeting id changes. Debounce autosave (~600 ms), save on blur,
and provide `flushNotes()` for Stop and unmount. Never overwrite the draft from
the detail object on segment/state refresh. Track `pendingAttachments`; Stop
first awaits `flushNotes()` and all attachment promises, then invokes Stop.
Disable Add/remove and Stop while an attachment mutation is pending. During
cleaning/summarizing show the saved snapshot as read-only; edits afterward are
allowed only after completion and are labelled as requiring Re-run notes.

Render: transcript + scratchpad columns while selected; file chips and Add
files; Summary; nested Key points; conditional Additional information; action
items; then cleaned transcript open by default with a raw-segments toggle.
`notesPending` includes both cleaning and summarizing. Show a Re-run notes
button for done and failed meetings. Update types to add `cleaning`, `KeyPoint`,
`MeetingFile`, notes/clean text/files, details/file counts, and the widened key
point representation.

## 6. Tests and release checks

Python tests cover v4-to-v5 backup/end state, state constraints, key-point
conversion including NULL/malformed values, and file cascade. Rust tests cover
classification/magic checks, UTF-8 and quota rejection, ZIP expansion caps,
Office extraction, filename/notes marker defanging, window splitting and
shrink fallback, structured-output caps, notes/file round trips, rerun guards,
and orphan sweep for cleaning. Add cleanup-failure and Stop-flush/attachment
barrier tests at the smallest layer that can exercise each condition.

Before release:

1. `python3 -m unittest discover tests`
2. `cd app/src-tauri && cargo test`
3. `cd app && npx tsc --noEmit && npm run build`
4. Migrate a copied store, then run `integrity_check`, `foreign_key_check`, and
   row-count comparisons before migrating the real store.
5. `npm run tauri build`, then manually verify: a deliberately misheard name
   corrected in the scratchpad; PDF, PPTX, PNG, and text attachment; raw/clean
   transcript visibility; nested points; action approval; a notes-only run.
6. Exercise negative paths: over-size/over-total files, ZIP bomb, encrypted or
   oversized-page PDF, binary file, network loss during cleaning, app kill in
   cleaning/summarizing, and an immediate type-and-Stop sequence.

