# intentionality

A gate that sits between login and your desktop: you have a short conversation
about what you intend to do, it becomes a task list, and at the end of the
session you debrief against it. All data stays in a local SQLite database.

Current status: **v0.4**. The gate can elicit intent (via Claude, or manually
when offline), commit a session, hand off to your desktop, wait, and run a
debrief when it exits — and it can be wired into a real console login (see
"Login-time wiring" below).

## Requirements

- Python 3.11+ (tested on 3.14)
- No dependencies for the manual-only path — everything is stdlib.
- For the AI conversation: a virtualenv with the `anthropic` package, and an
  API key.

## Setup

```bash
git clone <this repo> && cd intentionality

# Optional — only needed for the AI conversation, not for manual entry:
python3 -m venv .venv
.venv/bin/pip install anthropic
mkdir -p ~/.config/intentionality
echo "sk-ant-..." > ~/.config/intentionality/api_key
chmod 600 ~/.config/intentionality/api_key
```

No `pip install` step is required to just try it — see below.

**You need your own Anthropic API key** to use the AI conversation — one
isn't bundled with this repo. Get one from
[console.anthropic.com](https://console.anthropic.com), then paste it in
place of `sk-ant-...` above. The key file lives outside the repo (`~/.config/intentionality/api_key`, per
`gate/config.py`) and is read only by `gate/claude.py` — it's never part of
this checkout and won't get committed by accident. Keep it `chmod 600`; it's
the only credential the gate stores. Without a key, the gate still works
fully via manual entry (`python3 -m gate`, no venv needed).

## Running it

From the repo root:

```bash
# Manual entry, no dependencies, no key needed:
python3 -m gate

# AI-driven conversation (falls back to manual automatically if the key
# is missing, the network is down, or the model is unreachable):
.venv/bin/python -m gate
```

The gate asks what you want to get done, turns it into a numbered task list,
and lets you confirm, revise, or quit before anything is written. Once
confirmed, the session is committed to the local store.

### Ending a session

By default the gate doesn't launch anything after commit — it just prints a
reminder and exits, leaving the session open. Close it (and run the debrief)
whenever your session is actually done:

```bash
python3 -m gate close
```

This is the normal way to use it today: run `python -m gate` in the morning,
work as usual, run `python -m gate close` when you're done. The debrief walks
through each planned task — `d` done, `n` not done, `s` skip — then shows how
long the session ran versus what you intended.

### Full handoff (launches a program and waits on it)

If you set `INTENTIONALITY_DESKTOP_CMD`, the gate launches that command as a
child immediately after commit, blocks until it exits, and *then* runs the
debrief automatically — no need for `gate close`. This is what a real
console/login-time setup uses (eventually pointed at something like
`dbus-run-session -- gnome-session`), but it works with anything:

```bash
INTENTIONALITY_DESKTOP_CMD="xterm" python3 -m gate
```

If the session was left open by a crash or a hard reboot, the next `gate` run
notices, marks it `recovered`, and offers to resolve its tasks before
starting a new one.

### Login-time wiring (the real thing)

`bin/gate-login` is the launcher for running the gate at an actual text
console: it picks the venv Python if present, points
`INTENTIONALITY_DESKTOP_CMD` at `bin/desktop-session`, and declares the
session type Wayland so mutter doesn't guess X11. `desktop-session` runs
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

## The dashboard

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

A single SQLite file: `~/.local/share/intentionality/store.db`. Two tables —
`session` and `task` — plus a `meta` table for schema versioning. Nothing
else touches this file; inspect it anytime with the `sqlite3` CLI or Python's
built-in `sqlite3` module.

## Environment variables

| Variable | Purpose | Default |
|---|---|---|
| `INTENTIONALITY_STORE` | Path to the SQLite store | `~/.local/share/intentionality/store.db` |
| `INTENTIONALITY_DESKTOP_CMD` | Command to launch and wait on after commit | *(none — no handoff)* |

The API key path (`~/.config/intentionality/api_key`) and model
(`claude-opus-5`) are not environment-configurable yet — see `gate/config.py`
if you need to change them.

## Project layout

```
bin/
├── gate-login       # console launcher: runs the gate at a text login
└── desktop-session  # starts GNOME via the systemd user bus, logs output

gate/
├── __main__.py    # entry point: gate / gate close
├── flow.py        # ELICIT -> CONFIRM -> COMMIT state machine
├── manual.py      # dependency-free elicitation (also the AI fallback path)
├── provider.py    # the model boundary (Protocol) — what claude.py implements
├── claude.py       # AI elicitation via the Claude API, strict tool-use
├── handoff.py      # launches the desktop as a child, waits
├── debrief.py      # end-of-session summary + per-task resolution
├── store.py        # the only module that touches sqlite3
├── schema.sql      # session / task / meta tables
├── ui.py           # terminal prompt helpers
└── config.py       # paths, env var names, model id, timeouts

dashboard/
├── __main__.py     # entry point: dashboard / dashboard N / dashboard list
├── aw.py           # ActivityWatch REST client (stdlib urllib)
└── report.py       # intention-vs-observed rendering, AFK subtraction
```

`flow.py` never imports `sqlite3` or the network — it only calls into
`store` and the elicitation source. That boundary is what keeps the manual
path, the AI path, and the storage layer independently testable.

## Not yet built

- Mid-session task additions.
- Long-range dashboard views (per-week/month trends, category mapping).
- Production install mode (gate as the login shell itself; needs a rescue
  account first — the fish-config wiring above is the dev mode).
