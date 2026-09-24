# intentionality

A gate that sits between login and your desktop: a welcome screen shows the
tasks you still have, you keep, finish or delete them and add what else you
intend to do, and that list becomes the session's task list — no desktop
until it holds at least one task. Inside the session, a desktop app shows
those tasks as a kanban board next to what
[ActivityWatch](https://activitywatch.net/) actually observed, and a model
periodically writes a short productivity note. It also transcribes meetings on
demand and turns them into notes with action items. All data stays in a local
SQLite database, with two exceptions spelled out below: if you configure API
keys, each analysis sends that stretch's activity summary — window titles
included — to the Anthropic API, and each meeting you record sends its audio to
the OpenAI transcription API and the resulting transcript to Anthropic.

Current status: **v0.5**. The gate is deliberately conversation-free — you
edit the list yourself (the AI chat of v0.4 is gone from the gate; git has
it). Unfinished tasks carry into a backlog the next gate's welcome screen
shows you first.
The Tauri desktop app is the in-session interface: kanban board, activity
dashboard, randomized-interval analyses, and the meeting note taker.

## Requirements

- Python 3.11+ (tested on 3.14), stdlib only — the gate has
  zero dependencies.
- For the desktop app: Rust + Node (see `app/` below), webkit2gtk, and — for
  the analysis feature only — an Anthropic API key in
  `~/.config/intentionality/api_key` (chmod 600) or `ANTHROPIC_API_KEY`.
  Without a key the app still works; analyses are skipped with a log line.
  With one, each analysis sends the board — your task list and where each
  task stands — and the observed activity — app names **and the window titles under them** — to the
  Anthropic API. Titles carry document names, page titles and URLs, which is a
  good deal more than app names do; without a key none of it leaves the
  machine.
- For the meeting note taker only: `pipewire-utils` (for `pw-record` and
  `pw-dump`, already present on a standard Fedora desktop) and an OpenAI API
  key in
  `~/.config/intentionality/openai_key` (chmod 600) or `OPENAI_API_KEY`.
  Without a key the rest of the app is unaffected and Start transcribing
  reports the missing key. See "The meeting note taker" below for what leaves
  the machine when you use it.

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

The gate is one welcome screen. It lists your **active tasks** — the backlog:
whatever earlier sessions left unfinished, plus anything you added to the
backlog in the app — and you edit that list into this session's:

- **Keep** a task by leaving it alone. Everything still on the list when you
  start goes on today's board, carried tasks first.
- **Done** (carried tasks only) records it as done in the session it came
  from, and takes it off the list.
- **Delete** removes it for good — the same as the ✕ on the app's backlog.
- **Add** a task by typing it.
- **Details** opens a panel under any task, carried or typed: a **due date**
  (typed, Today / Tomorrow, or a calendar), **notes**, and **labels** — click
  any label that exists to tag it, or type a new one to create it.

Nothing is written until you start, so Done and Delete are toggles with an
Undo, and details are written only for tasks you actually changed (the app
may be editing the same backlog while a resume gate is up). Then say how long
you will be here (blank = open-ended) and start.
**Start refuses an empty list**: there is no quit and no way to a desktop
without at least one task. On the terminal the same screen is text — type a
task to add it, `d N` / `x N` to mark task N done / deleted (again to undo),
`e N` to change task N's due date, notes (one line) and labels (blank keeps,
`-` clears), and a blank line to start. There is no "what do you want to get done?"
question — the list is that answer.

### Ending a session

By default the gate doesn't launch anything after commit — it just prints a
reminder and exits, leaving the session open. Close it (and run the debrief)
whenever your session is actually done:

```bash
python3 -m gate close
```

This is the dev-mode way to use it: run `python -m gate` in the morning,
work as usual, run `python -m gate close` when you're done. The debrief walks
through each task still open — `d` done, `n` not done, `x` drop for good —
and then whatever is still unfinished is carried into the backlog for the next
gate. `n` is a carry: the task comes back next time. `x` is the only answer
that ends a task here, and Ctrl-C bails out of the rest, carrying every task
you never answered. (With the kanban app most tasks are already resolved by
dragging, so the debrief is usually one or two keystrokes.)

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
app wasn't running), and carries what's left into the backlog — which is
the first thing that gate's welcome screen shows, with Done and Delete on each
task. It asks nothing on its own.

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

To make it automatic, add this to `~/.config/fish/config.fish`, **above**
anything that prints (a `fastfetch` or greeting would otherwise sit on screen
while the gate starts). It fires only on a tty1 *login* shell with no desktop
running, so other VTs, SSH, and terminals inside GNOME are untouched:

```fish
# intentionality gate — tty1 login shells only, never inside a running gate.
# Keep this above anything that prints: it would show until the gate draws.
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

### The graphical gate

The same conversation, in a window, on the same bare console. Install one
package and both launchers pick it up:

```bash
sudo dnf install cage
```

[cage](https://github.com/cage-kiosk/cage) is a Wayland kiosk compositor: it
runs one program fullscreen and exits when that program exits. No display
server exists at gate time, and nothing that draws can run without one, so
this is the smallest thing that can put a window on tty1. Under it the gate
runs with `INTENTIONALITY_GATE_UI=gtk`, and `gate/gui.py` opens one GTK
window whose only child is a WebKit view showing `gate/webui/index.html` —
the React bundle built from `app/gate-ui/`, sharing its Tailwind theme and its
checkbox with the desktop app, so the two halves look like one product.

It opens on **"Let's get to work."** in white on black, which fades into the
welcome screen after about a second (any key or click skips it). The screen is
the task list — one checkbox per task, ticked to bring it into the session and
unticked to strike it out and delete it on Start — with a Details button per
row, Done on a task carried from an earlier session, an add box, the minutes,
and a Start button that stays disabled until there is a task. The details
panel opens in place under its row, and Start saves it first.

**The React side owns no list logic.** It posts intents; `gate/webstate.py`
applies them to `ui.WelcomeList` and pushes back a whole screen. Every string
the screen shows — the greeting, each row's detail line, the status line,
every refusal — is computed in `gate/ui.py` and travels as finished text, so
the graphical gate and the terminal gate cannot disagree about what a tick
means, and `flow.py` does not know which one is on. (`gate close` still asks
its debrief questions, as a text entry or a row of buttons under a log pane.)

Because `webstate.py` is pure — no `gi`, no WebKit, no display — the welcome
screen's behaviour is finally covered by `tests/test_webstate.py`. The GTK
version's state *was* its widget tree, so none of it could be asserted
without a compositor.

**Sizing.** `INTENTIONALITY_GATE_FONT_PX` (default 20) still means what it
always did, now exactly: device pixels per `rem`. The view's zoom is set to
`(font_px / 16) / monitor_scale`, so one rem is the same *physical* size
under cage as it is in a window on the desktop. That matters because this
panel is 2880×1800 and GNOME hands a client 1728×1080 at scale 1.667 while
cage does no scaling at all — without the correction the gate renders 1.667×
smaller in real life than in any test. `INTENTIONALITY_GATE_ZOOM` overrides
the computed factor outright.

It fails open, always. No `cage`, no PyGObject, no WebKitGTK, no display — and
no `gate/webui/index.html`, which is why that file is committed rather than
built at login: the gate prints one line to stderr saying which, and runs on
the terminal exactly as before. Rebuild it with `npm run build:gate` in `app/`
whenever anything under `app/gate-ui/` or `app/src/ui/` changes, and commit the
result. A graphical gate
that could fail closed would be one that could lock the login path.

**What it runs without, and why.** `gate/gui.py` sets three things in the
environment before the webview starts — not the launchers, so a gate started
any other way is started the same way. `GTK_A11Y=none` and `GIO_USE_VFS=local`
say that the accessibility bus and gvfs are not there, instead of letting
WebKit's helpers reach for them and wait. `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1`
turns off WebKit's bubblewrap sandbox, and that one has a story: with the
sandbox on, WebKit launches `xdg-dbus-proxy` beside the web process and calls
`g_error()` if that proxy exits non-zero — `abort()`, which no `except` can
catch, so the fail-open above is powerless against it. **Every resume gate
between the webview landing and this change died that way** (`Failed to fully
launch dbus-proxy: Child process exited with code 1`, read out of the
coredumps) and fell through to the terminal gate on VT9, which is why the wake
gate looked like the old one. It never happens at a login, under cage inside
GNOME, or under a systemd user service — only in the resume unit's own
context, so *why* the proxy exits 1 there is still unknown. The gate needs
nothing the sandbox contains: one committed local bundle, no network, no
remote content, no second origin, and the only strings in it that anyone typed
go through React's escaping. Set `INTENTIONALITY_GATE_SANDBOX=1` to put the
sandbox back for a run — with the log below, that is how to catch the proxy's
own error message the next time the machine wakes.

**Why the login launcher runs two commands.** Cage holds the GPU for as long as
its client lives, so GNOME cannot be started from inside the gate the way the
terminal version does it (as a child it waits on). `bin/gate-login` runs the
conversation under cage, cage exits, and then `python3 -m gate handoff` starts
the desktop for whichever session was just committed — or nothing, if the
gate was interrupted. That is unambiguous because the recovery sweep has just closed every
other open session.

**What it locks.** Cage is started without `-s`, so `Ctrl+Alt+F2`–`F6` do
nothing until you have answered: the keyboard belongs to the gate. On a resume
gate that also means `Ctrl+Alt+F1` no longer returns you to GNOME
mid-question, which is stricter than the text gate on VT9 was. What remains:
the skip file, SSH, and the power button. Put `-s` in the `cage_opts` line of
either launcher to get the console switches back.

The window cannot be closed either. It has no title bar, a close request is
refused (Alt+F4 included, whoever binds it), and Escape does nothing. There is
no quit button: the only way off the screen is Start, and Start needs a task.
`kill -INT` on the gate's python over SSH aborts it the way Ctrl-C does on the
text gate — nothing is saved, and at login no desktop starts.

**What you see.** Between login and the gate, and again for the second or two
GNOME takes to draw its first frame, the console shows its own text buffer.
`bin/gate-login` clears that buffer, hides the cursor, and sends everything cage
and the gate print to `~/.local/state/intentionality/gate.log` instead of the
console, so both gaps are plain black. The config.fish block has to come before
anything in that file that prints, such as `fastfetch`. Otherwise that output
is on screen while cage starts.

`bin/resume-gate` does the same, into the same log — and there it is the only
record there is. The unit's `StandardOutput=`/`StandardError=` are `/dev/tty9`,
so nothing a wake gate prints ever reaches the journal; five aborted gates went
unnoticed for that reason. If the graphical gate does not complete, the
fall-through line is logged with `systemd-cat` as well, so
`journalctl -t intentionality-gate` shows it long after VT9 has gone.

For the resume gate, the unit needs one more piece — see the next section.

### Waking from sleep

Login is not the only way back to a desktop. Close the lid at 15:00, open it at
21:00, and without this you land in a session you stated six hours ago, still
counting. So the gate has a second trigger: **wake the machine after 30 minutes
or more away and it runs again**, on its own virtual terminal, and hands you
back to GNOME once you have committed a new session.

It is the same program, not an imitation — the recovery sweep closes the old
session at its last heartbeat and carries its unfinished tasks into the
backlog, and the welcome screen shows them to you first. Like the login gate
it has no quit: **you are not handed back to GNOME until the new session has
at least one task.** (Before this screen, quitting a wake gate returned you to
the desktop with no session open; that is gone.)

It is also the same *screen*: `bin/resume-gate` runs the gate under cage on
VT9 exactly as `bin/gate-login` does on tty1, so a wake shows the black
welcome screen too. It did not for a while — see "What it runs without, and
why" above for the abort that sent every wake gate to the terminal fallback,
and read `~/.local/state/intentionality/gate.log` if one ever looks like the
terminal gate again.

Two units do it, both in `systemd/`:

```bash
sudo cp systemd/intentionality-resume.service \
        systemd/intentionality-resume-gate.service /etc/systemd/system/
sudo cp systemd/pam.d/intentionality-gate /etc/pam.d/
sudo systemctl daemon-reload
sudo systemctl enable intentionality-resume.service   # apply
sudo systemctl disable intentionality-resume.service  # revert
```

The PAM file is for the unit's `PAMName=`: it registers a logind session on
VT9 for the run. The text gate never needed one; the graphical gate cannot do
without it, because a compositor only gets the GPU and the input devices from
logind, and logind gives them only to the active session on that VT. The stack
is `account` + `session` with `pam_systemd` and nothing else — systemd never
runs the `auth` stack for `PAMName=`, so no password is asked and none is
granted. It also sets `XDG_RUNTIME_DIR`, which cage requires; `bin/resume-gate`
treats its absence as "text gate", not "try anyway".

`intentionality-resume.service` is pulled into `suspend.target` and ordered
`After=systemd-suspend.service`, which is what makes a unit run on *resume*
rather than before sleep. It does one thing — starts the other unit
`--no-block` — because a `suspend.target` job stays open until its units exit,
and a gate you take ten minutes to answer would block the next suspend.

`intentionality-resume-gate.service` runs the gate on **VT9**. It has to be a
system unit for one reason: `/dev/tty9` is `root:tty 0600`, so nothing you run
as yourself can open it. `TTYPath=` with `StandardInput=tty-force` has systemd
open it as root and hand the fd down to `User=david`, and the `+` prefix on
`ExecStartPre=`/`ExecStopPost=` runs `chvt` with full privileges under that
same `User=`. No polkit rule, no setuid, no sudo at runtime. VT9 because
logind's `NAutoVTs=6` autospawns gettys on VT1–6 only, so 7–12 are free and the
`Ctrl+Alt+F2`–`F6` escape hatches stay exactly as they were.

Its `ExecStart=` is `/usr/bin/fish …/bin/resume-gate`, not the script's own
path, because of SELinux. Everything under your home directory is labelled
`user_home_t`, and systemd runs as `init_t`, which is not allowed to execute
`user_home_t` files: a bare path fails with `status=203/EXEC` right after the
switch to VT9, so the screen flashes and no gate appears. An interpreter in
`/usr/bin` is `bin_t`, which moves the process into `unconfined_service_t`, and
that domain may read the script. `tests/test_units.py` fails if any `Exec*=`
line points into `/home`.

The desktop app checks the install every time it starts. Its header says
**resume gate not installed** until `suspend.target.wants/` holds the unit, and
**resume units in /etc differ from systemd/** when you have edited a unit here
without copying it again.

**How long counts as away** is `now - session.last_heartbeat`: the store
already records when the machine stopped being used — it is the timestamp the
recovery sweep turns into `ended_at` — so a suspend needs no stamp of its own.
The threshold is the `meta` setting `resume_min_away_minutes`, default 30:

```bash
sqlite3 ~/.local/share/intentionality/store.db \
  "INSERT INTO meta (key, value) VALUES ('resume_min_away_minutes', '45')
   ON CONFLICT(key) DO UPDATE SET value = excluded.value;"
```

The check is the unit's `ExecCondition=`, so a short nap is a clean *skip* —
the screen never flickers to VT9 on a lid-close you did not think of as
leaving. With no open session, or one the app never heartbeated, it fires:
waking a machine with no stated intention is what the gate is for.

Escape hatches, as everywhere: `Ctrl+Alt+F1` returns to GNOME from a
half-answered gate, `touch ~/.config/intentionality/skip` disables this along
with the login gate, and disabling the unit reverts it entirely. There is no
quit: the gate ends when a session with at least one task has started. (Under
cage the `Ctrl+Alt+F1` hatch is closed too — see "What it locks".)

Test it by hand before trusting it to a real lid-close. Starting the unit by
hand still goes through `ExecCondition=`, and while the desktop app is
heartbeating you have been away for under a minute, so it skips. Drop the
threshold to 0 for the test. The recovery sweep will then close your current
session, exactly as a real wake would:

```bash
sqlite3 ~/.local/share/intentionality/store.db \
  "INSERT INTO meta (key, value) VALUES ('resume_min_away_minutes', '0')
   ON CONFLICT(key) DO UPDATE SET value = excluded.value;"
sudo systemctl start intentionality-resume-gate.service   # the whole VT dance
journalctl -u intentionality-resume-gate -b               # what it decided
```

Then a real suspend, with the threshold set to `1` the same way so a
two-minute sleep counts. Let `rtcwake` set the wake alarm only and let systemd
do the suspending. `rtcwake -m mem` suspends through `/sys/power/state` by
itself, never reaches `suspend.target`, and so never runs these units at all:

```bash
sudo rtcwake -m no -s 120 && systemctl suspend
journalctl -b | grep intentionality-resume                # both units ran
sqlite3 ~/.local/share/intentionality/store.db \
  "DELETE FROM meta WHERE key = 'resume_min_away_minutes';"   # back to 30
```

## The desktop app

```bash
cd app && npm run tauri dev      # development
npm run tauri build              # release binary + rpm
```

Runs inside the session (it is deliberately not the gate — a window inside
GNOME can always be alt-tabbed away; the lockout lives at the console). Three
surfaces, all bare-bones for now:

- **Board** — To Do / Doing / Done lanes over the current session's tasks
  (drag to move), a dropped tray that is a fourth drop target, and the backlog
  in a sidebar: pull items into today, add new ones, delete stale ones.
  Session cards are history — drag one into the tray to drop it, and drag it
  back out to undo that, but they are never deleted. Clicking any card (or a
  backlog row) opens it: rename it, write plain-text **notes** on it, give it
  a **due date** — typed, or picked from the month grid behind
  **Calendar**, and shown on the card in words ("Due tomorrow", "Overdue ·
  Sep 9") and beside the task on the gate's welcome screen — and **labels**: every label there is shows as a chip to click, and
  typing a new name makes a new one. **Details…** beside either add box opens
  the same editor for a task that doesn't exist yet, so it starts with all of
  that; Enter still adds the bare title. The **Labels** panel under the
  backlog lists every label with how many tasks wear it. Make presets there
  before anything uses them; a label stays until you delete it (one still in
  use asks first, and deleting takes it off every card).
  Notes, labels and due dates travel with a task when the gate carries it into
  the backlog.
- **Dashboard** — recent sessions, their task outcomes, and per-app active
  time from ActivityWatch with AFK subtracted, each app expandable to the
  window titles that made up its time.
- **Analyses** — at randomized intervals (mean `analysis_mean_minutes` in the
  store's `meta` table, default 60) the app sends the board state and per-app
  activity totals — the busiest 8 apps, each with
  its 5 longest-running window titles — to Claude and stores a short
  observation: headline, 0-100 alignment, a sentence or two. Titles are what
  let it tell research from drift; on app names alone a browser is
  unreadable. It lands as an
  unread badge in the app *and* a desktop notification. Quiet windows (< 5 min
  active) are skipped. "Run a check now" deliberately does not notify — you
  are already looking at the tab that answers it.
- **Checkpoint** — when the time you said you would be here runs out
  (`intended_minutes`, set at the gate), the app runs an analysis of its own,
  raises a desktop notification, and puts a full-screen checkpoint in front of
  you: what the last stretch looked like, and one recommendation for what to
  do from here. `Got it` dismisses it; `+15 min` / `+30 min` re-arm it. It
  never closes the session — that stays the gate's job.

  Recommendations come from a fixed catalog in
  `src-tauri/src/recommendations.rs`; the model picks one id from it and
  writes a sentence saying why it fits. Only the id is stored, so rewording an
  entry applies to every checkpoint already recorded. Every entry's `source`
  is currently `None` and the screen says so — these are sensible defaults,
  not research findings, and that is the slot real citations go in.

  Open-ended sessions (blank minutes at the gate) get no checkpoint: a NULL
  `intended_minutes` is already you saying not to hold you to a clock. The
  due time lives in `session.checkpoint_due_at`, so a checkpoint that came due
  while the app was closed fires on its next start rather than being lost.

  Two notifications, in total, are all this app ever raises. It was built with
  none at all on the rule that an OS notification should be *structurally*
  impossible; the checkpoint is why that changed, and the plugin is still
  reachable from exactly one file (`src-tauri/src/notify.rs`).

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

Note what that `Exec` points at: a **build artifact**. It changes only when you
run `npm run tauri build`, so an old binary keeps autostarting at login until
you rebuild — and on 2026-08-29 that meant a day-old app started at login with
nothing on screen to say so.

So the app names its own build. The header always shows
`v0.5.0 · built 21:44 Aug 29`, and when any file under `app/src`,
`app/src-tauri/src`, `app/src-tauri/Cargo.toml`, `app/index.html` or
`app/package.json` is newer than the binary it adds
`build is behind your source — run npm run tauri build`. The same line goes to
stderr at startup, which at autostart means the journal:

```bash
journalctl --user -b | grep '^intentionality v'
```

The comparison is file timestamps, not the git commit: this tree is dirty most
of the time, and a commit check would report "up to date" all the way through.
`gate/` is deliberately not watched — editing `store.py` or `schema.sql` needs
no app rebuild, and warning about it would only teach you to ignore the warning.

On GNOME 50 the autostart entry is launched by `gnome-session-service` itself,
as a transient scope (`app-gnome-intentionality-NNNN.scope`), **not** through
systemd's `xdg-desktop-autostart.target`. That target has `RefuseManualStart=yes`
and nothing pulls it in, so every generated `app-*@autostart.service` on the
machine sits inactive. That is normal here, not a fault to chase.

Autostart fires **once**, at session start, and closing the window quits the
app — `main.rs` sets no `on_window_event` and there is no tray icon, so Tauri's
default applies and the last window closing ends the process. Nothing in
`~/.config/autostart` shows up in the GNOME overview, so for a while that
combination meant a closed window was gone until the next login. Hence a second
entry, this one an ordinary launcher:

```ini
# ~/.local/share/applications/intentionality.desktop
[Desktop Entry]
Type=Application
Name=Intentionality
Comment=Session board, activity dashboard, and heartbeats
Exec=/home/david/Desktop/projects/intentionality/app/src-tauri/target/release/intentionality
Icon=/home/david/Desktop/projects/intentionality/app/src-tauri/icons/icon.png
StartupWMClass=intentionality
Terminal=false
Categories=Utility;
```

Two files, two jobs: `~/.config/autostart` handles login, this one handles the
rest of the session (Super, type "Intentionality"). GNOME scans the two
directories separately, so nothing launches twice. `StartupWMClass` is what
makes the running window bind to this entry in the dash instead of appearing
beside it as a second, iconless item. gnome-shell watches the directory, so the
entry appears without a relog.

Worth knowing what a closed window costs, since it is easy to do by reflex: the
30 s heartbeat stops with it, and `last_heartbeat` is what the next gate reads
to infer `ended_at`. Close the window at 19:07 and keep working until the next
day, and the session is recorded as ending at 19:07. That is not a bug — the app
genuinely stopped watching — but it is a quiet way to lose an evening of
session history, and it happened on 2026-08-31 (session 16). Clicking the app
open again while it is already running starts a *second* process, with its own
analysis timer; if that ever becomes a real annoyance the fix is
`tauri-plugin-single-instance`.

## The meeting note taker

The Meetings tab records a meeting, transcribes it as it goes, and turns the
transcript into a write-up and action items. The write-up is one markdown
document — an opening paragraph, key points, and additional information when
there is any, with fenced code blocks and `$…$` LaTeX where the meeting had
code or maths. It is the model's draft until you press **Edit**; then it is
yours, and **Re-run notes** asks before replacing a document you have changed.
After a meeting the detail pane has three tabs: **Summary** (the write-up and
the action items), **Notes** (your scratchpad and context files) and
**Transcript** (cleaned, with the raw segments behind a toggle).

Pick a microphone, press **Start transcribing** and it opens; press **Stop**
and it shuts at once. The notes are written behind you — a line under the
button says whether it is still cleaning the transcript or writing the notes
— and you can **start the next meeting straight away**: the microphone was
free the moment Stop returned, and only the model calls are queued, so a
second wrap-up waits for the first rather than competing with it. Nothing
else starts a recording — there is no timer, no scheduler and no startup path
that can, which is the one hard rule this feature has.

### Correcting the transcript

Click any block in the raw transcript to fix what was heard: a mangled name,
a piece of jargon, or a stretch that failed to transcribe at all, which shows
as "(this stretch could not be transcribed)" and opens an empty box you can
type into. It works while the meeting is still recording, while the notes are
being written, and long afterwards — there is no state in which the record of
what was said is frozen against you.

Corrections go into the raw segments rather than the cleaned text, because
the raw segments are what everything else is built from: **Re-run notes**
re-cleans and re-summarizes from them, so a fix reaches the next write-up.
The cleaned transcript and the write-up both say when they predate your
corrections, and offer the re-run rather than taking it — the repair pass is
minutes of model time, and when to spend it is your call. Editing one block
never rewrites another, and the timestamps stay put, so the meeting is still
navigable after a correction.
While the microphone is open the tab shows a red pulsing indicator, the
elapsed time, and the level meter described below.

How it works: one `pw-record` process writes raw 16 kHz mono PCM to its stdout,
Rust processes that stream 20 ms at a time and slices it into two-minute
chunks, and each chunk is sent to OpenAI's transcription API as it is cut. That
is why the transcript appears during the meeting, and why Stop has only the
final part-chunk left to transcribe before the notes can be written. Stop
returns as soon as the microphone is shut, and that last chunk and the summary
are finished behind it — which is what the **Writing the notes…** button is
reporting. Each chunk's audio is held in memory, uploaded, and dropped; **no
audio is ever written to disk or to the store.** That is unchanged by
everything below: the meter and the source selector add no file, no recording
and no new row.

### Choosing a microphone

The dropdown above Start lists PipeWire's capture sources; **System default
microphone** is the entry that changes nothing. Your choice is remembered
across restarts by node *name*, not by PipeWire's object serial, because a
serial is reassigned when a device is unplugged and plugged back in. If the
remembered device is not there when the tab loads — or has gone by the time you
press Start — the selector falls back to the system default and says so. It
does not forget your choice: the backend reports a `pw-dump` that failed and a
`pw-dump` that found nothing the same way, so a momentary hiccup would
otherwise throw away a deliberate pick. Plug the device back in and it is
selected again; only choosing something yourself changes what is remembered.

That check is the app's own, and it has to be: `pw-record` does **not** refuse
an unknown `--target`. It falls back to the default source, streams happily and
reports nothing until it exits, so an unplugged microphone would otherwise
produce a recording from the wrong device rather than an error. The
selector is disabled while a recording is running — `pw-record` is given its
source when it starts, so changing it mid-meeting could only mislead.

An empty list is not a failure. If PipeWire is unavailable or `pw-dump` says
nothing, only System default is offered and the detail goes to the journal.

### Reading the level meter

The bars beside the recording clock are the **raw** microphone level, before
any gain: 24 bars, one per 100 ms window, scrolling right to left over the last
2.4 seconds. Height is mapped from dBFS, not linear amplitude — speech across a
room is invisible on a linear scale. The small line under them reports input,
output and peak levels plus the gain currently applied.

Nothing about the strip is decorative. Every bar is a measurement that arrived
from the capture thread; there is no timer and no interpolation, so a strip
that stops moving means the levels stopped, and a strip resting at its floor
means the microphone is hearing nothing. After about four seconds of sustained
low *raw* level it says **very quiet — check the mic or move closer**. That
warning tracks the raw level only, never the gain. Under
`prefers-reduced-motion` the bars lose their smoothing between values but keep
updating at the same rate.

The red indicator, not the meter, remains the answer to "is this recording?".

### Automatic gain, and what it cannot do

A speaker across a room arrives far below what the transcription model wants,
and the internal laptop microphone is already at its hardware ceiling
(`amixer -c0 sget Dmic0` reads `Capture 70 [100%] [20.00dB]` against a limit of
70; `pw-record --volume` caps at 1.0). So the capture path applies bounded
digital gain: up to +30 dB toward a conservative -20 dBFS target, held rather
than raised whenever the raw input is below a noise floor, with a soft limiter
before the 16-bit conversion so a sudden loud frame compresses instead of
clipping.

**Gain increases amplitude, not information.** If distant speech and room noise
arrive at the same signal-to-noise ratio, both are raised together, and
over-amplified room noise is a known way to make these models hallucinate. A
USB boundary microphone, a lavalier, or anything else closer to the speaker is
the real fix for a lecture; this is what makes the room *audible*, not what
makes it *clear*.

Set `INTENTIONALITY_AGC=off` to capture without it. The meter still runs, so
this is a controlled A/B and not a different program — which matters precisely
because no audio is kept to compare afterwards.

### Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `INTENTIONALITY_AGC` | on | `off` disables automatic gain; metering is unaffected. |
| `INTENTIONALITY_TRANSCRIBE_MODEL` | `gpt-4o-transcribe` | Also accepts `whisper-1`. Anything else is refused at Start. |
| `INTENTIONALITY_TRANSCRIBE_LANGUAGE` | `en` | An ISO-639-1 code. **Recording another language needs this set.** |

Both transcription variables are validated before the microphone opens, so a
typo costs a Start error rather than a recorded meeting that turns out to have
been rejected chunk by chunk. The allowed model list is short on purpose:
diarization models are rejected even though the endpoint takes them, because
their request and response shape differs from the carry-forward `prompt` this
app uses to stitch chunk boundaries. A model override is not a language
setting; the two are separate for that reason.

Recording lives in Rust, not in the React component, so switching to the Board
and back does not stop it. If a chunk fails to transcribe the meeting carries
on and the gap is shown in the transcript rather than hidden. If the app is
killed mid-meeting, everything transcribed up to that point is already saved
and the meeting is marked failed on the next start rather than left recording
forever. If the model call fails, the transcript is still there and the detail
pane offers a retry that costs no audio.

Action items are proposals, not tasks. Each gets a checkbox, and **Add N to
backlog** inserts the ticked ones as backlog tasks (`source = 'meeting'`) that
the next gate offers you. Approving twice cannot insert twice.

**What leaves the machine.** This is the largest exception to "all data stays
local" in the project. The analysis call sends app and window names; this sends
what was said in a room, and it can capture people who never agreed to it. The
audio goes to OpenAI and the transcript then goes to Anthropic for the notes.
Both happen only between your Start and your Stop.

Level data stays on the machine and is never stored: the meter's events are
ephemeral, and the journal gets one summary line per two-minute chunk (raw and
output RMS, peak, median and maximum gain) and never a sample. That line still
reveals when a room was acoustically active, which is why it is a summary.

The transcript is treated as untrusted input, like window titles: it is wrapped
in delimiters the transcript itself cannot forge, and the model is told it is a
recording of what people said and never instructions to follow. A participant
saying "ignore your previous instructions" is recorded, not obeyed.

## ActivityWatch

The app's Dashboard tab and the analyses read
[ActivityWatch](https://activitywatch.net/)'s local API (`localhost:5600`,
override with `INTENTIONALITY_AW_URL`) and work without it: sessions and tasks
still display, and observations show as unavailable.

> **GNOME Wayland caveat:** the window watcher bundled with ActivityWatch is
> X11-only and records nothing under GNOME Wayland. You need the
> [Focused Window D-Bus](https://extensions.gnome.org/extension/5592/) GNOME
> extension plus [awatcher](https://github.com/2e3s/awatcher) in place of the
> bundled `aw-watcher-window`/`aw-watcher-afk` — see
> [running on GNOME](https://docs.activitywatch.net/en/latest/running-on-gnome.html).

## Where your data lives

A single SQLite file: `~/.local/share/intentionality/store.db`. Tables:
`session`, `task` (rows with `session_id NULL` are the backlog),
`label` + `task_label` (the tags a task wears), `analysis`,
`meeting` + `meeting_segment` + `meeting_action` (the note taker), and `meta`
(schema version + settings). Recorded audio is **not** among them — each chunk
is transcribed and dropped, so only text is ever stored. Only `gate/store.py` and the app's
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
| `INTENTIONALITY_GATE_UI` | `gtk` asks for the graphical front end; anything else, or no display, is the terminal | *(terminal)* |
| `INTENTIONALITY_GATE_FONT_PX` | Device pixels per `rem` in the graphical gate — its size knob, on any display | `20` |
| `INTENTIONALITY_GATE_ZOOM` | Overrides the computed webview zoom outright, when the rule is wrong for a monitor | *(computed)* |
| `INTENTIONALITY_GATE_SOFTWARE` | Draws the gate without GPU acceleration; pair with the launchers, which also set `WEBKIT_DISABLE_DMABUF_RENDERER` | *(off)* |
| `INTENTIONALITY_GATE_INSPECT` | Enables the WebKit inspector on the gate, for design work | *(off)* |
| `INTENTIONALITY_GATE_SANDBOX` | Puts WebKit's sandbox back for one run — a diagnostic, see "The graphical gate" | *(sandbox off)* |

Two keys, two files, two providers: the Anthropic key at
`~/.config/intentionality/api_key` (analyses, checkpoints and meeting notes)
and the OpenAI key at `~/.config/intentionality/openai_key` (transcription
only). Each is read by exactly one module and neither reaches the webview.

Meeting notes (the transcript repair and the write-up) run on `claude-sonnet-5`;
analyses and checkpoints stay on `claude-opus-5`. Meeting attachments are
capped at 25 MB per file and 50 MB per meeting. PDFs and images are uploaded
to the Anthropic Files API when a notes run starts, referenced by id, and
deleted when the run ends however it ends. Each upload also carries a 4-hour
expiry in case the app dies mid-run. Text and Office files send only their
extracted text.

The analysis model (`claude-opus-5`) lives in `app/src-tauri/src/claude.rs`;
the key comes from `~/.config/intentionality/api_key` or `ANTHROPIC_API_KEY`
and never reaches the webview. Window titles are the least trustworthy input in
that prompt — a page can title itself anything, including something addressed
to the model — so each one is flattened to a single line, quoted, and
length-capped before it goes in, and the system prompt says outright that
titles are data to judge and never instructions to follow.

## Project layout

```
bin/
├── gate-login       # console launcher: runs the gate at a text login
├── resume-gate      # the gate again, on a spare VT after the machine wakes
└── desktop-session  # starts GNOME via the systemd user bus, logs output

systemd/
├── intentionality-resume.service       # fires on resume, starts the next unit
├── intentionality-resume-gate.service  # the gate on VT9 (TTYPath + chvt + PAMName)
└── pam.d/intentionality-gate           # the logind session that unit's PAMName= needs

gate/
├── __main__.py    # entry point: gate / close / migrate / resume / resume-needed / handoff
├── flow.py        # WELCOME -> COMMIT: the list in, one session out
├── resume.py      # was the machine away long enough to re-gate?
├── handoff.py     # launches the desktop as a child, waits
├── debrief.py     # end-of-session per-task resolution
├── store.py       # the only Python module that touches sqlite3; owns migrations
├── schema.sql     # session / task / analysis / meeting / meta tables (v10)
├── ui.py          # the welcome list + questions; terminal by default, or a backend
├── gui.py         # the WebKit backend, for running under cage
├── webstate.py    # the welcome screen as data -- pure, no gi, so it is testable
├── config.py      # paths and env var names
└── webui/         # the built React bundle (committed: login cannot run npm)

app/                # the Tauri desktop app, and the gate's front end
├── src/            # React + Tailwind: Board, Analyses, Meetings, Dashboard
│   └── ui/         # the shared theme and components, used by both bundles
├── gate-ui/        # the gate's React, built into gate/webui/ by build:gate
└── src-tauri/src/  # Rust: db, aw, observed, claude, scheduler, commands,
                    #       meeting + record + gain + level + audio
                    #       + transcribe (the note taker)

tests/
├── test_store.py   # data-layer tests: migration, carry, close idempotency
├── test_debrief.py # what d/n/x do, and that recovery strands nothing
├── test_flow.py    # the welcome screen's plan landing, the no-empty-session rule
├── test_welcome.py # the shared list model and the terminal welcome
├── test_gui.py     # the ui backend seam, button labels, `gate handoff`
├── test_webstate.py # the welcome screen's state, with no display and no gi
├── test_resume.py  # the away-time threshold that decides a resume gate
└── test_units.py   # the systemd units and launchers: no Exec into /home, PAM
```

`flow.py` never imports `sqlite3` or the network — it only calls into
`store` and `ui`. Run the data-layer tests with
`python3 -m unittest discover tests`.

## Not yet built

- Long-range dashboard views (per-week/month trends, category mapping) —
  the `analysis` table already accumulates the data for them.
- In-column drag reordering on the board (cross-column moves work).
- Production install mode (gate as the login shell itself; needs a rescue
  account first — the fish-config wiring above is the dev mode).
