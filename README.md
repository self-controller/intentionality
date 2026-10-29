# intentionality

A gate that sits between login and your desktop: a welcome screen shows the
tasks you still have, you tick the ones you will do today (the rest stay in
your backlog), finish or delete others and add what else you intend to do, and
the ticked ones become the session's task list — no desktop until at least one
task is ticked. Inside the session, a desktop app shows
those tasks as a kanban board next to what
[ActivityWatch](https://activitywatch.net/) actually observed, and every so
often checks how much of a stretch went to the tasks. All data stays in a
local SQLite database, with one exception spelled out below: if you configure
a TypeSafe key, the window titles that checks and the at-risk timer judge,
your session's task titles and notes, and each new task you add without a
label (with your label names), go to TypeSafe's API.

Current status: **v0.5**. The gate is deliberately conversation-free — you
edit the list yourself (the AI chat of v0.4 is gone from the gate; git has
it). Unfinished tasks carry into a backlog the next gate's welcome screen
shows you first.
The Tauri desktop app is the in-session interface: kanban board, activity
dashboard, and randomized-interval analyses.

## Requirements

- Python 3.11+ (tested on 3.14), stdlib only — the gate has
  zero dependencies.
- For the desktop app: Rust + Node (see `app/` below), webkit2gtk, and — for
  the checks, the at-risk timer's relevance verdicts and filing new tasks
  under labels (the gate uses the same key) — a TypeSafe API key in
  `~/.config/intentionality/typesafe_key` (chmod 600) or `TYPESAFE_API_KEY`.
  Without a key the app still works; checks are skipped with a log line, the
  checkpoint still fires (unjudged), and the timer shows on every flagged page.
  With one, what is sent is listed under "What leaves the machine" below.
  Nothing goes to Anthropic any more: the Claude-written analyses are gone
  (git has `app/src-tauri/src/claude.rs`), their rows stay readable, and
  `~/.config/intentionality/api_key` is unused and can be deleted.

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

- Tasks are listed in a **dropdown per label** (click a label's header to fold
  it away), with an **Other** dropdown last for tasks that have no label. A
  task with several labels is listed under each of them — it is the same task,
  so ticking one copy ticks them all.
- **Tick** a task to do it today: ticked tasks go on today's board (To Do),
  carried tasks first. Tasks start **unticked** and stay in the backlog, so
  unfinished work carried over from earlier sessions is never put on the board
  unless you choose it. A task you type starts ticked; unticking it saves it to
  the backlog instead.
- **Done** (carried tasks only) records it as done in the session it came
  from, and takes it off the list.
- **Delete** removes it for good (struck through until you start; press it
  again to undo). The app's card editor has the same Delete button.
- **Add** a task by typing it. One added with no labels is filed under a
  label for you when Jev is sure of one (see **Filed by Jev** below): it moves
  into that label's dropdown a moment later, and **Details** takes it off.
- **Details** opens a panel under any task, carried or typed: a **due date**
  (typed, Today / Tomorrow, or a calendar), **notes**, and **labels** — click
  any label that exists to tag it, or type a new one to create it.

Nothing is written until you start, so Done and Delete are toggles with an
Undo, and details are written only for tasks you actually changed (the app
may be editing the same backlog while a resume gate is up). Then say how long
you will be here (blank = open-ended) and start.
**Start refuses until at least one task is ticked**: there is no quit and no way
to a desktop without at least one task for today. On the terminal the same
screen is text — type a task to add it (for today), `t N` to tick / untick task
N, `d N` / `x N` to mark task N done / deleted (again to undo),
`e N` to change task N's due date, notes (one line) and labels (blank keeps,
`-` clears), and a blank line to start. There is no "what do you want to get done?"
question — the list is that answer.

**Filed by Jev.** A task added with no labels — typed at the gate (either
front end) or saved on the app's board — is shown to TypeSafe's Jev beside
every label there is and up to six titles already filed under each, and gets
at most one of those labels. Never a new one. In one request (about 0.2 s),
Jev picks the label that fits best and, separately for each label, says
whether the task belongs there at all. The label goes on only when both
answers are at least 0.7 (`MIN` in `gate/autolabel.py` and
`app/src-tauri/src/autolabel.rs`, kept equal by a test). So when two labels
both fit about equally ("School" and a course's own label), or nothing fits
well, the task is left as it was. When two labels both fit, Jev is told to
prefer the more specific one. A label you choose by hand always wins: a task
you labelled, a panel you saved, or a card you relabelled before the answer
arrived is left alone. At the gate, Start waits up to 3 s for an answer still
out. With no key or no network nothing is sent, and the task simply has no
label. Each decision is logged (`gate.log` for the gate, the journal for the
app). `cargo test live_sort -- --ignored --nocapture` (in `app/src-tauri`)
asks the real Jev about a few titles against your live labels, read-only, and
prints what each would be filed under (billed).

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
the first thing that gate's welcome screen shows (unticked, so it stays in the
backlog unless you tick it for today), with Done and Delete on each task. It
asks nothing on its own.

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
the task list, grouped into label dropdowns, on the desktop app's black — one
checkbox per task, ticked to bring it into today's board — with Details and
Delete buttons per row, Done on a task carried from an earlier session, an add
box, and a footer holding the minutes and a Start button that stays disabled
until a task is ticked. The details panel opens in place under the row it was
opened from, and Start saves it first. The page itself scrolls, and the footer
stays pinned to the bottom, so a long list or an open panel never hides Start.

**The React side owns no list logic.** It posts intents; `gate/webstate.py`
applies them to `ui.WelcomeList` and pushes back a whole screen. Every string
the screen shows — the greeting, each row's detail line, every refusal — is computed in `gate/ui.py` and travels as finished text, so
the graphical gate and the terminal gate cannot disagree about what a tick
means, and `flow.py` does not know which one is on. (`gate close` still asks
its debrief questions, as a text entry or a row of buttons under a log pane.)

Because `webstate.py` is pure — no `gi`, no WebKit, no display — the welcome
screen's behaviour is finally covered by `tests/test_webstate.py`. The GTK
version's state *was* its widget tree, so none of it could be asserted
without a compositor.

**Sizing.** `INTENTIONALITY_GATE_FONT_PX` (default 26) still means what it
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

It also skips while **another gate is already on screen**. Close the lid on
the login gate's welcome screen and there is no open session either (the
recovery sweep has closed the old one, and nothing is committed yet), so on
its own that would fire a second gate on VT9. Answering both used to split
your tasks over two sessions and start the desktop for only one of them.
Every gate now holds an `flock` on `gate.lock`, next to the store, while its
welcome screen is up, and the check stands down while one is held, so you
wake up on the gate you left. The kernel drops the lock whenever the holder
exits, however it exits, so a crashed gate can never keep a later wake from
firing.

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

**Ctrl+Tab** / **Ctrl+Shift+Tab** step through the tabs.

- **Board** — To Do / Doing / Done lanes over the current session's tasks
  (drag to move), a dropped tray that is a fourth drop target, and the backlog
  in a sidebar: pull items into today, add new ones, delete stale ones.
  Drag a session card into the tray to drop it (history stays), and back out
  to undo that. To remove a task completely, open it and press **Delete** in
  its editor (it asks once; works for session cards and backlog rows alike).
  Clicking any card (or a backlog row) opens it: rename it, write plain-text **notes** on it, give it
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
  the backlog. A card saved with no labels may get one a moment later (see
  **Filed by Jev** above).
- **Dashboard** — **today**, since local midnight. **Time active** and
  **Time away** lead as large figures (ActivityWatch, AFK subtracted past a
  5-minute grace — see the timer below). A pie
  splits the active time into **Relevant to a task**, **Not relevant** and
  **Not judged**, by the at-risk timer's own rule: each page of today against
  the tasks in **Doing** right now, read from Jev's cached verdicts, so the
  Dashboard and the clock never disagree about a page. While nothing is in
  Doing, every page of the day counts as not relevant, as the clock shows it;
  time in no session counts as not judged. Reading costs nothing; **Judge N pages with
  Jev** asks about the busiest unjudged pages (up to 30, like a check), which
  is billed and sends those titles to TypeSafe. Under the pie, relevant time
  by task; beside it, the at-risk apps panel. **Recent sessions** — each
  session's task outcomes and per-window active time — is folded away
  underneath, and the build line sits at the foot of the tab.
- **Analyses** — at randomized intervals (mean `analysis_mean_minutes` in the
  store's `meta` table, default 60) the app runs a **check** on the stretch
  since the last one. Every page of it (the 30 busiest, each window title
  normalised the way the timer does it) is judged against every task of the
  session that was not dropped — done ones too, since time spent finishing a
  task was time on it — by the at-risk timer's own question to
  [Jev](https://docs.typesafe.ai), through the timer's own cache and bar
  (below). The **alignment** is the share of judged active time on pages some
  task excuses; time past the 30 pages, an app's "(other)" rollup, and pages
  Jev could not be asked about count as *not judged*, and under half judged
  means no number at all. Jev answers with probabilities, never prose, so the
  headline and the sentences are written by the app (`check.rs`). The tab
  shows where the time went by task, the at-risk apps' time in the stretch,
  and every page with its verdict; a page no task needed carries the same
  **Appeal** as the clock's arrow, and a win there keeps the timer off that
  page too (it counts from the next check — a row is a snapshot). Rows the
  Claude-era analysis wrote stay readable, shown the way they were written.
  A check lands as an unread badge in the app *and* a desktop notification.
  Quiet windows (< 5 min active) are skipped. "Run a check now" deliberately
  does not notify — you are already looking at the tab that answers it.

  **Early check on drift.** While the timer is up captioned *Not relevant to
  any task* during a session, that time is counted; at `drift_check_minutes`
  (a `meta` setting, default 10, 0 = off) since the last check, a check runs
  early instead of waiting for its draw, and its notification says so.
- **Checkpoint** — when the time you said you would be here runs out
  (`intended_minutes`, set at the gate), the app runs an analysis of its own,
  raises a desktop notification, and puts a full-screen checkpoint in front of
  you: what the last stretch looked like, and one recommendation for what to
  do from here. `Got it` dismisses it; `+15 min` / `+30 min` re-arm it. It
  never closes the session — that stays the gate's job.

  Recommendations come from a fixed catalog in
  `src-tauri/src/recommendations.rs`; Jev picks one id from it with a single
  choice question, matching the stretch (the clock, the board, the check's
  numbers) against the situation each entry `fits`, and the app writes the
  sentence under it. Only the id is stored, so rewording an entry applies to
  every checkpoint already recorded. Every entry's `source`
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

### At-risk apps: the on-screen timer

Flag an app or a website you lose time to — `linkedin.com`, `youtube.com`,
`discord` — from the **At-risk apps** panel on the Dashboard. While a
flagged one is the window in front, a giant clock sits at the top right of the
screen with today's **off-task total**, counting up. There is one clock,
shared by every app and site: it counts today's time on every page that
would show it now (a flagged page no Doing task excuses, or any page Jev found
relevant to none of them), each moment once. So a new off-task site picks up
from the day's total rather than from zero, and switching between two of them
never starts it over. Switch to something relevant and it goes; come back and
it carries on from the total. An appeal you win takes that page's time back
out. It shows over fullscreen windows too, and cannot be dragged or dismissed.
It hides in the overview, on the lock screen, and once you are away. **Away**
means 5 minutes with no keyboard or mouse input (GNOME's screen-blank delay).
ActivityWatch calls you AFK after 3, but until the screen goes dark you may
be reading or watching, so the first 5 minutes of every idle stretch count as
present everywhere: in the clock, on the Dashboard and in the checks
(`observed::IDLE_GRACE_SECS`). The panel lists today's time on every flag,
with the total counted once even where two flags overlap.

**Unless the page is relevant to a task.** Before the clock appears, the page
is judged against every task in the board's **Doing** column by
[Jev](https://docs.typesafe.ai), TypeSafe's System One model: one request,
one yes/no question per task, each answered with a probability. A page that
at least one task can plausibly need — the job post for *apply to 3 roles* —
gets no clock at all; the feed gets the clock, captioned **Not relevant to any
task**, with a small arrow beside the caption. Jev sees each task's title
and its notes, so a note that names the sites or folders a task uses counts.
Click the arrow and a box opens right under the clock with the cursor already
in it: say why the page serves a task and press Enter, and Jev re-judges it
with your explanation (the whole case so far, every time). The answer
replaces the caption. A verdict at or over the bar (70 %, `meta` setting
`relevance_min`) stops the clock for that page and is remembered; short of
it, the box waits for more. Esc, or a click on any window, closes it. Jev
answers with probabilities, not prose, so the "chat" is you making the case
and the app wording the verdict.

**A case you win is remembered beyond its page.** Every later judgement is
shown your newest won appeals (up to 12: the window, the task, what you
said), and the rule tells Jev that the same reasoning holds for a window like
it — so the next Stack Overflow tab for the same bug needs no new appeal. A
win also clears Jev's earlier "not relevant" verdicts for that task, so each
of those pages is asked again, with your case, the next time it is in front;
until then its time is out of today's off-task total.

**Every other window is judged too.** Any tab or app that stays in front for
1.5 s is put to Jev the same way, flagged or not. One that is relevant to none
of the Doing tasks gets the same clock, the same caption and arrow, and the
same shared count. **With nothing in Doing**, no open session included,
nothing is relevant: every window gets the clock, captioned **Nothing in
Doing** and without an arrow, and all of that time is off task. Nothing else
ever shows on an unflagged window: with no key or TypeSafe unreachable it
looks exactly as it did before. A flagged window is still judged the moment
it comes to the front.
What counts as one page, so the same thing is judged once
(`app/src-tauri/src/subject.rs`):

- **A browser tab** is its title, without unread counts or the browser's name.
  When the title alone says little, it is also the page it sits under: Canvas
  calls every course's file list *Files*, so a Files tab is *Files · 2026FA -
  INTRO DATA SCIENCE …*, judged apart from another course's Files. That page
  comes from the browser's own history (`app/src-tauri/src/places.rs`): the
  tab's address is the one last visited under its title, and the page above
  is the nearest one on the same site with a title of its own, skipping the
  site's front page and one-word listings such as *Courses*. It is used only
  when at least half its words are not already in the tab's title. Firefox
  and its forks (Zen among them) only; the history is copied to
  `$XDG_RUNTIME_DIR`, read into memory and the copy deleted at once. Two
  open tabs with the same bare title in different courses both read as the
  one visited last, and today's earlier time on a bare title is counted
  under what it means now.
- **VS Code** (and Code - OSS, VSCodium, Cursor) is the project folder it has
  open: every file of `cereal` is *VS Code: cereal*, and Jev is told the
  project, never the file name.
- **A terminal** is the project it is working in: every terminal in
  `intentionality` is *Terminal: intentionality*, whether it sits at a prompt
  in `~/D/p/i/app`, runs `npm run tauri build`, or runs a Claude Code session
  titled `remove-wordmark-title-bar`. Outside any project it is its folder,
  except that a Claude Code session there is its own page, since `~` holds
  sessions about anything. A spinner frame or a `— claude --resume …` tail
  does not make a new page.
- **The project** behind an editor or terminal is found on the machine
  (`app/src-tauri/src/folder.rs`), because titles rarely say where they are.
  fish's shortened path is matched against the folders that exist. A Claude
  Code session's folder comes from Claude Code's own records: the running
  sessions in `~/.claude/sessions/*.json` (never the `.key` files beside them)
  and the titles in its transcripts. VS Code's folder comes from its
  `workspaceStorage/*/workspace.json`. The project is the nearest git
  repository above that folder, or failing one the nearest folder with a
  README. Jev is shown the folder, the project's name and the README's
  opening sentence, so a task worded by purpose ("fix the meeting notepad")
  can still match a folder called `cereal`. A Claude Code session that has no
  title yet, and whose folder cannot be told, is not judged at all.
- **Any other app** is its window title, so each Discord channel and each PDF
  is its own page.
- **Never judged:** this app's own window (so the board stays reachable
  without a clock) and file dialogs (`xdg-desktop-portal-*`).

Verdicts are cached in the store (`relevance` table, keyed by the page as
above and the task's text), so a page seen before is decided without
a request and an edited task is judged afresh. The at-risk panel's footer
counts them and offers **Forget**, which costs nothing but the requests to ask
again.
With no TypeSafe key the header says so and
every flagged page shows the clock captioned **Relevance unchecked**; put the key at
`~/.config/intentionality/typesafe_key` (chmod 600) or in `TYPESAFE_API_KEY`.

The checks (above) ask this same question, through this same cache and bar,
about every page of their stretch, so the timer and the Analyses tab cannot
disagree about a page: a page a check judged is already decided when the
timer meets it, and an appeal won in either place holds in both.

**What leaves the machine:** for the timer, every window you stay on for
1.5 s, flagged or not: a browser's tab title, another app's window title, an
editor's project folder name (not its file names), a terminal's directory or
session name. For an editor or terminal whose folder is found, that folder's
path from `~` (never your user name), the project's name and the first
sentence of its README go too; Claude Code's records, its transcripts and
VS Code's workspace list are only read locally, never sent. For a browser
tab whose title alone says little, the site's name (not the address) and the
title of the page it sits under go too; the browser's history is only read
locally, never sent. With them go the titles and notes of the session's Doing
tasks, the window, task and text of your newest won appeals (up to 12), and,
on an appeal, what you typed. This app's own window and file
dialogs send nothing, and with no open session nothing is sent at all. For each check, the titles of up to 30 windows of its
stretch — flagged or not — with the titles and notes of the session's tasks;
pages already in the cache are not sent again. For the checkpoint, the
board's task titles and the check's numbers. For each task added with no
labels, at the gate or in the app: its title and notes, every label's name and
up to six titles filed under each. App names travel with the titles. Nothing
else does, and nothing goes anywhere without a key.

How a window is matched, because ActivityWatch records an app and a window
title but never a URL:

- **A website** is looked for by its name: `linkedin.com` matches a browser
  window whose title contains the word *LinkedIn* ("Feed | LinkedIn — Zen
  Browser"). `www.`, `https://` and any path are ignored, and `bbc.co.uk` looks
  for *bbc*. A site whose tab titles never name it will not match; flag the
  word its titles do use instead.
- **Anything else** is looked for as typed, in the app's id (`discord`,
  `spotify`) or, in a browser, in the title.
- Whole words only, case-insensitive, so `x.com` does not match
  `xdg-desktop-portal-gtk`. Titles count only inside a browser: an editor with
  `linkedin.py` open is not LinkedIn time.

The time comes from ActivityWatch and the verdict cache, never a model call
(focused-window time, minus AFK past the grace — the same arithmetic as the
Dashboard), so removing a flag loses nothing and adding one
counts the whole of today straight away. *Which* window is in front comes
from the shell extension below, the moment it changes — a tab switch shows
up in milliseconds, and a page already judged is decided in the same instant.
Without the extension (or with the v1 copy still loaded) the app falls back to
polling ActivityWatch's newest event, a second or two behind.

**The clock is drawn by a GNOME Shell extension.** On GNOME/Wayland an app
cannot place its own window or keep it above the others; only the compositor
can. So the app decides what to show and `gnome-extension/at-risk-timer@intentionality`
only draws it, over D-Bus; it also reports every change of focused window or
title back, and holds the appeal box the arrow opens, sending your case to
the app and showing the answer it gets back. It hides
itself within 4 s if the app stops talking to it. Install it once — a symlink,
so later repo edits need only another re-login, never a re-copy:

```bash
ln -s "$PWD/gnome-extension/at-risk-timer@intentionality" ~/.local/share/gnome-shell/extensions/
# then log out and back in -- on Wayland the shell only looks for new
# extensions at login, so `gnome-extensions enable` says "does not exist"
# until then -- and after logging back in:
gnome-extensions enable at-risk-timer@intentionality
```

The header says **timer overlay not installed** until that is done, and
**timer overlay is out of date** when the extension's code changed and the
shell is still running the old copy (log out and back in). It also says
**window tracking is off** when the Focused Window D-Bus extension is
disabled — without it awatcher sees only XWayland apps, so the timer, the
Dashboard and every analysis go quietly blind (this happened for two days in
October 2026, when another extension's setup rewrote the enabled list; the
fix is `gnome-extensions enable focused-window-dbus@flexagoon.com`).

To check the timer from a terminal (GNOME refuses screenshots to other
programs): `gdbus call --session --dest org.gnome.Shell --object-path
/org/intentionality/Timer --method org.intentionality.Timer.State` — it
reports the caption, whether the arrow is up, and the focused window as the
shell sees it. The app logs every change of what is on screen, and every page
a task excused: `journalctl --user -b | grep 'at risk'`.

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
`label` + `task_label` (the tags a task wears), `risk_app` (the flagged
at-risk apps), `relevance` (Jev's cached page-vs-task verdicts), `analysis`,
and `meta` (schema version + settings). `meeting`, `meeting_segment`,
`meeting_action` and `meeting_file` are dormant: they hold what the removed
meeting note taker recorded, and nothing reads or writes them any more.
Only `gate/store.py` and the app's
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
| `INTENTIONALITY_GATE_FONT_PX` | Device pixels per `rem` in the graphical gate — its size knob, on any display | `26` |
| `INTENTIONALITY_GATE_ZOOM` | Overrides the computed webview zoom outright, when the rule is wrong for a monitor | *(computed)* |
| `INTENTIONALITY_GATE_SOFTWARE` | Draws the gate without GPU acceleration; pair with the launchers, which also set `WEBKIT_DISABLE_DMABUF_RENDERER` | *(off)* |
| `INTENTIONALITY_GATE_INSPECT` | Enables the WebKit inspector on the gate, for design work | *(off)* |
| `INTENTIONALITY_GATE_SANDBOX` | Puts WebKit's sandbox back for one run — a diagnostic, see "The graphical gate" | *(sandbox off)* |

The TypeSafe client is `app/src-tauri/src/jev.rs` (model alias `jev-latest`;
the versioned model that answered is stored with every verdict and every
check). The key comes from `~/.config/intentionality/typesafe_key` or
`TYPESAFE_API_KEY` and never reaches the webview; `TYPESAFE_API_URL` points
the client at a local stub for testing. A window title is the least
trustworthy input — a page can title itself anything — but it travels as a
JSON field, not inside a prompt, and Jev answers only with probabilities, so
the worst a title can do is sway its own page's verdict.

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
├── lock.py        # one gate on screen at a time (an flock the resume check honours)
├── handoff.py     # launches the desktop as a child, waits
├── debrief.py     # end-of-session per-task resolution
├── store.py       # the only Python module that touches sqlite3; owns migrations
├── schema.sql     # session / task / analysis / meta tables (v14)
├── ui.py          # the welcome list + questions; terminal by default, or a backend
├── gui.py         # the WebKit backend, for running under cage
├── webstate.py    # the welcome screen as data -- pure, no gi, so it is testable
├── autolabel.py   # Jev files a typed, unlabelled task under a label (the gate's one request)
├── config.py      # paths and env var names
└── webui/         # the built React bundle (committed: login cannot run npm)

app/                # the Tauri desktop app, and the gate's front end
├── src/            # React + Tailwind: Board, Analyses, Dashboard
│   └── ui/         # the shared theme and components, used by both bundles
├── gate-ui/        # the gate's React, built into gate/webui/ by build:gate
└── src-tauri/src/  # Rust: db, aw, observed, scheduler, commands,
                    #       screen_time + overlay (the at-risk timer),
                    #       relevance + jev (is the page relevant to a task?),
                    #       analysis + check (the checks, judged by Jev),
                    #       autolabel (a new card filed under a label by Jev)

gnome-extension/
└── at-risk-timer@intentionality/  # draws the at-risk timer, reports the focused
                                   # window (symlinked into ~/.local/share/gnome-shell/extensions)

tests/
├── test_store.py   # data-layer tests: migration, carry, close idempotency
├── test_debrief.py # what d/n/x do, and that recovery strands nothing
├── test_flow.py    # the welcome screen's plan landing, the no-empty-session rule
├── test_welcome.py # the shared list model and the terminal welcome
├── test_gui.py     # the ui backend seam, button labels, `gate handoff`
├── test_webstate.py # the welcome screen's state, with no display and no gi
├── test_autolabel.py # filing by Jev: policy, the request (local stub), the app's twin
├── test_resume.py  # the away-time threshold that decides a resume gate
├── test_units.py   # the systemd units and launchers: no Exec into /home, PAM
└── test_extension.py # the timer extension: clock arithmetic (under gjs), layout
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
