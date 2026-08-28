# intentionality

A gate that sits between login and your desktop: you type a short list of
what you intend to do, it becomes the session's task list, and at the end of
the session you debrief against it. Inside the session, a desktop app shows
those tasks as a kanban board next to what
[ActivityWatch](https://activitywatch.net/) actually observed, and a model
periodically writes a short productivity note. All data stays in a local
SQLite database.

Current status: **v0.5**. The gate is deliberately conversation-free — you
type the list yourself (the AI chat of v0.4 is gone from the gate; git has
it). Unfinished tasks carry into a backlog the next gate offers back to you.
The Tauri desktop app is the in-session interface: kanban board, activity
dashboard, and randomized-interval analyses.

## Requirements

- Python 3.11+ (tested on 3.14), stdlib only — the gate and CLI dashboard
  have zero dependencies.
- For the desktop app: Rust + Node (see `app/` below), webkit2gtk, and — for
  the analysis feature only — an Anthropic API key in
  `~/.config/intentionality/api_key` (chmod 600) or `ANTHROPIC_API_KEY`.
  Without a key the app still works; analyses are skipped with a log line.

## Setup

```bash
git clone <this repo> && cd intentionality
python3 -m gate            # that's the whole gate setup

# Desktop app (one-time):
sudo dnf install gcc gcc-c++ make cmake perl-core pkgconf-pkg-config \
     webkit2gtk4.1-devel javascriptcoregtk4.1-devel libsoup3-devel \
     gtk3-devel librsvg2-devel nodejs npm
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cd app && npm install && npm run tauri build
```

## Running it

From the repo root:

```bash
python3 -m gate
```

If the backlog holds tasks carried from earlier sessions, the gate offers
them first (`Pull in? (numbers, blank for none)`). Then it asks what you want
to get done, you type tasks one per line, and you confirm, revise, or quit
before anything is written. Once confirmed, the session is committed to the
local store.

### Ending a session

By default the gate doesn't launch anything after commit — it just prints a
reminder and exits, leaving the session open. Close it (and run the debrief)
whenever your session is actually done:

```bash
python3 -m gate close
```

This is the dev-mode way to use it: run `python -m gate` in the morning,
work as usual, run `python -m gate close` when you're done. The debrief walks
through each task still open — `d` done, `n` not done, `s` skip — and then
whatever is still unfinished is carried into the backlog for the next gate.
(With the kanban app most tasks are already resolved by dragging, so the
debrief is usually one or two keystrokes.)

### Full handoff (launches a program and waits on it)

If you set `INTENTIONALITY_DESKTOP_CMD`, the gate launches that command as a
child immediately after commit, blocks until it exits, and *then* runs the
debrief automatically — no need for `gate close`. This is what a real
console/login-time setup uses (eventually pointed at something like
`dbus-run-session -- gnome-session`), but it works with anything:

```bash
INTENTIONALITY_DESKTOP_CMD="xterm" python3 -m gate
```

When GNOME exits, logind tears down the whole login scope — the waiting gate
included — so in a real console login the closing write happens at the *next*
gate run: it notices the open session, stamps its end from the desktop app's
last heartbeat (or marks it `recovered` with an honest unknown end when the
app wasn't running), offers the debrief, and carries what's left into the
backlog.

### Login-time wiring (the real thing)

`bin/gate-login` is the launcher for running the gate at an actual text
console: it runs the stdlib-only gate with the system `python3` (nothing to
break at login), points `INTENTIONALITY_DESKTOP_CMD` at `bin/desktop-session`,
and declares the session type Wayland so mutter doesn't guess X11. `desktop-session` runs
GNOME 50's `gnome-session` leader over the systemd user bus with
`--no-reexec` (its login-shell re-exec would fire the gate block again,
recursively) and logs everything to
`~/.local/state/intentionality/desktop.log`, so a desktop that dies on
arrival leaves an explanation.

Launching GNOME from a tty needs one extra piece: a text login's logind
session has type `tty`, and mutter only adopts a non-graphical session via
its `XDG_SESSION_ID` environment lookup — a variable `gnome-session`
deliberately strips when uploading the environment to the systemd user
manager. The fix is a user-level drop-in at
`~/.config/systemd/user/org.gnome.Shell@.service.d/intentionality.conf`:

```ini
[Service]
EnvironmentFile=-%t/intentionality/session-env
```

`desktop-session` writes the current session ID to that runtime file before
launching and removes it after, so GDM logins (file absent) are untouched.
Run `systemctl --user daemon-reload` once after creating the drop-in.
Note: only one GNOME session per user — test from a state where you're not
also logged into a graphical session. Test it by hand first: switch to a free console
(`Ctrl+Alt+F3`), log in, run
`~/Desktop/projects/intentionality/bin/gate-login`. You should get the gate
conversation, then a real desktop, then the debrief when you log out of it.

To make it automatic, add this to `~/.config/fish/config.fish` — it fires
only on a tty1 *login* shell with no desktop running, so other VTs, SSH, and
terminals inside GNOME are untouched:

```fish
# intentionality gate — tty1 login shells only, never inside a running gate.
# Escape hatch: `touch ~/.config/intentionality/skip` disables it.
if status is-login
    and test (tty) = /dev/tty1
    and not set -q DISPLAY
    and not set -q WAYLAND_DISPLAY
    and not set -q INTENTIONALITY_GATE
    and not test -e ~/.config/intentionality/skip
    ~/Desktop/projects/intentionality/bin/gate-login
end
```

Then boot to a text console instead of GDM:

```bash
sudo systemctl set-default multi-user.target   # apply
sudo systemctl set-default graphical.target    # revert
sudo systemctl start gdm                       # one-off rescue: start GDM now
```

Escape hatches are deliberate: `Ctrl+Alt+F2`+ are normal consoles, the skip
file above bypasses the gate, and reverting the boot target restores GDM
exactly as before. This is commitment, not security.

## The desktop app

```bash
cd app && npm run tauri dev      # development
npm run tauri build              # release binary + rpm
```

Runs inside the session (it is deliberately not the gate — a window inside
GNOME can always be alt-tabbed away; the lockout lives at the console). Three
surfaces, all bare-bones for now:

- **Board** — To Do / Doing / Done lanes over the current session's tasks
  (drag to move), a collapsed dropped tray, and the backlog in a sidebar:
  pull items into today, add new ones, delete stale ones. Session cards are
  history — they can be dropped but never deleted.
- **Dashboard** — recent sessions, their task outcomes, and per-app active
  time from ActivityWatch with AFK subtracted.
- **Analyses** — at randomized intervals (mean `analysis_mean_minutes` in the
  store's `meta` table, default 60) the app sends the session statement, the
  board state, and per-app activity totals to Claude and stores a short
  observation: headline, 0-100 alignment, a sentence or two. Notification is
  a badge inside the app, never an OS notification. Quiet windows (< 5 min
  active) are skipped.

The app also writes `session.last_heartbeat` every 30 s — that is what turns
"session never closed" into an accurate end time at the next gate. Autostart
it (path must be absolute):

```ini
# ~/.config/autostart/intentionality.desktop
[Desktop Entry]
Type=Application
Name=Intentionality
Exec=/home/david/Desktop/projects/intentionality/app/src-tauri/target/release/intentionality
Terminal=false
X-GNOME-Autostart-enabled=true
```

## The CLI dashboard

```bash
python3 -m dashboard          # latest session in detail + recent list
python3 -m dashboard 3        # a specific session
python3 -m dashboard list     # recent sessions only
```

Shows each session's intention (statement, tasks, intended duration) next to
what [ActivityWatch](https://activitywatch.net/) observed in the same time
window: active time per app, minus away time. It reads ActivityWatch's local
API (`localhost:5600`, override with `INTENTIONALITY_AW_URL`) and works
without it — sessions and tasks still display, observations show as
unavailable.

> **GNOME Wayland caveat:** the window watcher bundled with ActivityWatch is
> X11-only and records nothing under GNOME Wayland. You need the
> [Focused Window D-Bus](https://extensions.gnome.org/extension/5592/) GNOME
> extension plus [awatcher](https://github.com/2e3s/awatcher) in place of the
> bundled `aw-watcher-window`/`aw-watcher-afk` — see
> [running on GNOME](https://docs.activitywatch.net/en/latest/running-on-gnome.html).

## Where your data lives

A single SQLite file: `~/.local/share/intentionality/store.db`. Tables:
`session`, `task` (rows with `session_id NULL` are the backlog), `analysis`,
and `meta` (schema version + settings). Only `gate/store.py` and the app's
`db.rs` touch this file; migrations belong to Python alone — `store.init()`
upgrades old stores (after an online self-backup to `store.db.v1.bak`), and
the app refuses politely with `python3 -m gate migrate` when the schema is
older than it understands. Inspect it anytime with the `sqlite3` CLI.

## Environment variables

| Variable | Purpose | Default |
|---|---|---|
| `INTENTIONALITY_STORE` | Path to the SQLite store | `~/.local/share/intentionality/store.db` |
| `INTENTIONALITY_DESKTOP_CMD` | Command to launch and wait on after commit | *(none — no handoff)* |
| `INTENTIONALITY_AW_URL` | ActivityWatch API base URL | `http://localhost:5600` |
| `INTENTIONALITY_SESSION_ID` | Set by the gate for the desktop; the app trusts it only if that session is still open | *(set by handoff)* |

The analysis model (`claude-opus-5`) lives in `app/src-tauri/src/claude.rs`;
the key comes from `~/.config/intentionality/api_key` or `ANTHROPIC_API_KEY`
and never reaches the webview.

## Project layout

```
bin/
├── gate-login       # console launcher: runs the gate at a text login
└── desktop-session  # starts GNOME via the systemd user bus, logs output

gate/
├── __main__.py    # entry point: gate / gate close / gate migrate
├── flow.py        # PULL -> ELICIT -> CONFIRM -> COMMIT state machine
├── manual.py      # the elicitation: type your list
├── handoff.py     # launches the desktop as a child, waits
├── debrief.py     # end-of-session per-task resolution
├── store.py       # the only Python module that touches sqlite3; owns migrations
├── schema.sql     # session / task / analysis / meta tables (v2)
├── ui.py          # terminal prompt helpers
└── config.py      # paths and env var names

dashboard/          # the CLI dashboard (stdlib only)
app/                # the Tauri desktop app
├── src/            # React frontend: Board, Dashboard, AnalysisPanel
└── src-tauri/src/  # Rust: db, aw, observed, claude, scheduler, commands

tests/
└── test_store.py   # data-layer tests: migration, carry, close idempotency
```

`flow.py` never imports `sqlite3` or the network — it only calls into
`store` and `manual`. Run the data-layer tests with
`python3 -m unittest discover tests`.

## Not yet built

- Long-range dashboard views (per-week/month trends, category mapping) —
  the `analysis` table already accumulates the data for them.
- In-column drag reordering on the board (cross-column moves work).
- Production install mode (gate as the login shell itself; needs a rescue
  account first — the fish-config wiring above is the dev mode).
