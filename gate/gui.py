"""A graphical front end for the gate, meant to run under a kiosk compositor.

Same gate as the terminal: flow still asks ui.welcome for the session's
tasks, and debrief (for `gate close`) still calls ui.ask / ui.confirm_choice;
install() makes those land here instead. The screen is a WebKit view showing
the React bundle built from app/gate-ui into gate/webui/index.html, but the
list it edits is still ui.WelcomeList, the same model the terminal uses, so
the two cannot disagree about what a tick or a Done means. Everything the
gate print()s reaches the log pane, because install() also points sys.stdout
at this object.

One thread, on purpose, and unchanged by the move to a webview. The view is a
GTK widget in this process on this process's default GLib main context, so a
postMessage from the page arrives exactly the way a button's "clicked" used
to: each question pushes its state and then spins the main context until an
answer arrives. The store connection never crosses a thread and flow stays the
plain sequential code it is. No local server, no subprocess. The one
exception is Jev filing a typed task under a label (autolabel.py): that
request waits on a thread of its own, and its answer comes back to this one
through the main context like any other event.

The window cannot be closed: it has no decorations, a close request (the X a
compositor would draw, Alt+F4, GTK's own close action) is refused, and Escape
does nothing. There is no quit button either: the only way off the welcome
screen is Start, and Start needs a task. SIGINT is the one abort left --
unreachable from the keyboard under cage, but `kill -INT` over SSH still
works -- and it counts as Ctrl-C did on the terminal: the question being asked
raises GateAborted, and every question after it does too.

Importing this module needs nothing; constructing GtkUI needs PyGObject,
GTK 4, WebKitGTK 6 and a display, and says which is missing.
"""

import json
import os
import signal
import sys
import threading
from collections.abc import Mapping
from pathlib import Path

from . import autolabel, ui, webstate
from .ui import GateAborted

# The debrief names its keys once, in an earlier print, not in every
# prompt. These are those keys' meanings.
_KNOWN_LABELS = {
    "dnx": {"d": "Done", "n": "Not done", "x": "Drop for good"},
}

# Device pixels per CSS rem. The bundle authors everything in rem against the
# browser default of 16, and the webview's zoom is set so that one rem is this
# many *physical* pixels -- see _zoom(). That is what makes the gate the same
# size under cage as it is when tested nested inside GNOME, and it is the same
# knob the console font took in /etc/vconsole.conf.
DEFAULT_FONT_PX = 26
REFERENCE_REM_PX = 16

# The cursor theme size the gate asks GTK for: exactly the width of GTK's own
# built-in cursor images. cage has no cursor-shape protocol, so GTK 4.22 draws
# the pointer itself from 32x32 textures (/org/gtk/libgdk/cursor/*) whose
# hotspots are in texture pixels (pointer 14,9; text 14,15). It scales each
# one by theme_size / 32 but hands the compositor the hotspot unscaled, so at
# any size but 32 the drawn tip and the real pointer part company: at GNOME's
# 24 the hand and the I-beam sat ~4.5 px down-right of where a click landed.
# Under GNOME, mutter draws the cursor and this setting changes nothing.
CURSOR_PX = 32

# The built bundle. Committed, because login cannot depend on an npm build.
WEBUI = Path(__file__).resolve().parent / "webui" / "index.html"
# A synthetic base URI, so the document gets a real origin instead of an
# opaque one. Verified on this machine: with it, inline module scripts run and
# storage works; without it the origin is "null" and storage throws.
BASE_URI = "gate://app/"

# The page's background, for the window as well. The window shows through for
# the first few hundred ms, until WebKit's web process hands over its first
# frame; matching it to the page (gate.css --color-bg) means that gap is
# invisible rather than a flash of whatever GTK falls back to.
BACKGROUND = "#0c0c0c"

# How long the page gets to say hello before we give up and let the terminal
# gate run instead.
READY_SECONDS = 10.0

# After Start, how long answers about typed tasks still get to land. Jev
# answers in well under a second; this is only the most a slow network can
# hold the session back.
SORT_WAIT = 3.0

# The environment the webview runs in. It lives here, not in the launchers, so
# that every entry point gets the same one: the login gate under cage, the
# resume gate under cage on VT9, and a bin/gate-login run by hand from another
# console while GNOME is up.
#
# The sandbox switch is the one that matters. WebKit wraps its web process in
# bubblewrap and launches xdg-dbus-proxy beside it, and when that proxy exits
# non-zero WebKit calls g_error() -- abort(), which no `except` can catch, so
# select_ui's fail-open cannot help and the whole gate dies. It did: every
# resume gate from the v1.5 webview until this change aborted in
# XDGDBusProxy::launch ("Failed to fully launch dbus-proxy: Child process
# exited with code 1", read out of the coredumps) and fell through to the
# terminal gate on VT9. It never happens at a login, under cage inside GNOME,
# or under a systemd user service -- only in the resume unit's own context,
# so the proxy's own reason is still unknown. What is certain is that the gate
# needs nothing the sandbox is there to contain: one committed local bundle,
# no network, no remote content, no second origin, and the only strings in it
# that anyone typed go through React's escaping.
#
# INTENTIONALITY_GATE_SANDBOX=1 puts the sandbox back for a run. That is how
# to catch the proxy's error message next time the machine wakes, now that
# bin/resume-gate keeps a log.
SANDBOX_OFF = "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS"
KEEP_SANDBOX = "INTENTIONALITY_GATE_SANDBOX"
# Optional services a bare console does not have. Saying so beats letting
# WebKit's helpers reach for them and wait.
DEFAULTS = {"GTK_A11Y": "none", "GIO_USE_VFS": "local"}


def webkit_env(env: Mapping[str, str]) -> dict[str, str]:
    """What to add to the inherited environment before the webview starts.

    Pure, so the contract is testable without a display, and it never
    overrides a value the caller set on purpose."""
    extra = {key: value for key, value in DEFAULTS.items() if not env.get(key)}
    if not env.get(KEEP_SANDBOX) and not env.get(SANDBOX_OFF):
        extra[SANDBOX_OFF] = "1"
    return extra


# Scrolling. Cage has no natural-scrolling setting, and a page that reverses
# wheel events itself cannot win: WebKitGTK coasts after the fingers lift
# (kinetic scrolling) without sending the page a single wheel event, so that
# coast cannot be prevented or reversed. Measured with a virtual touchpad: the
# page moved the right way while the fingers did, then jumped back and coasted
# the wrong way -- the "jitter". So the window takes every scroll before
# WebKit sees it, and tells the page how far to move (window.__gateScroll).
#
# CSS px per GDK unit, matching what WebKit itself scrolls (measured on
# WebKitGTK 2.52; the same at every zoom level): a touchpad's surface units,
# and one notch of a mouse wheel.
TOUCHPAD_PX = 2.5
WHEEL_PX = 105.0
# Where the natural-scrolling choices live. Read-only, so it works at a
# console with no session bus: dconf reads its database file directly.
NATURAL_SCHEMAS = ("org.gnome.desktop.peripherals.touchpad", "org.gnome.desktop.peripherals.mouse")
# GNOME's own defaults, for when the settings cannot be read.
NATURAL_DEFAULTS = (True, False)


def scroll_step(dx: float, dy: float, wheel: bool, natural: bool, inverted: bool) -> tuple[float, float]:
    """CSS px to scroll the page for one GDK scroll event.

    `natural` is the desktop's setting for this kind of device; `inverted`
    says the compositor has applied it already. GNOME's mutter does, so a gate
    run inside GNOME passes deltas through; cage never does, so at the console
    the gate reverses them itself. Pure, so it is testable without a display."""
    scale = WHEEL_PX if wheel else TOUCHPAD_PX
    sign = -scale if natural and not inverted else scale
    return dx * sign, dy * sign


def parse_choices(prompt: str, choices: str) -> list[tuple[str, str]]:
    """(key, label) per choice: from the known key sets, else the key."""
    known = _KNOWN_LABELS.get(choices, {})
    return [(k, known.get(k) or k) for k in choices]


def question_text(prompt: str) -> str:
    """The prompt without its trailing '> ', for a heading."""
    return prompt.replace(">", "").strip()


class GtkUI:
    """The webview front end. The name is historical -- tests pin it, and the
    window really is still a GTK window; only its one child changed."""

    def __init__(self, font_px: int = DEFAULT_FONT_PX):
        if not WEBUI.exists():
            raise RuntimeError(f"{WEBUI} is missing -- run `npm run build:gate` in app/")
        html = WEBUI.read_text(encoding="utf-8")
        # Before gi, and well before load_html(): WebKit reads these when it
        # launches the network and web processes, not when it is imported.
        os.environ.update(webkit_env(os.environ))

        import gi

        gi.require_version("Gtk", "4.0")
        gi.require_version("Gdk", "4.0")
        gi.require_version("WebKit", "6.0")
        from gi.repository import Gdk, GLib, Gtk, WebKit

        self.Gtk, self.Gdk, self.GLib, self.WebKit = Gtk, Gdk, GLib, WebKit
        # init_check() returns True with no display at all, so ask Gdk.
        if not Gtk.init_check() or Gdk.Display.get_default() is None:
            raise RuntimeError("no display to open a window on")
        # Before the window exists, so its very first frame is black too.
        gtk_settings = Gtk.Settings.get_default()
        gtk_settings.set_property("gtk-application-prefer-dark-theme", True)
        # Size, then name: GTK passes the size on only when it has a theme
        # name, and under cage nothing else may give it one. See CURSOR_PX.
        gtk_settings.set_property("gtk-cursor-theme-size", CURSOR_PX)
        gtk_settings.set_property("gtk-cursor-theme-name", "Adwaita")
        css = Gtk.CssProvider()
        css.load_from_string(f"window {{ background-color: {BACKGROUND}; }}")
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )

        self.closed = False
        self.font_px = font_px
        self._real_stdout = sys.stdout
        self._log: list[str] = []
        self._log_pending = None
        self._ready = False
        self._fatal: str | None = None
        self._queue: list[dict] = []
        self._last: dict | None = None
        # Set for the duration of a question; every page message goes here.
        self._handler = None

        ucm = WebKit.UserContentManager()
        if not ucm.register_script_message_handler("gate", None):
            raise RuntimeError("could not register the gate message handler")
        ucm.connect("script-message-received::gate", self._on_message)

        self.web = WebKit.WebView(user_content_manager=ucm, hexpand=True, vexpand=True)
        bg = Gdk.RGBA()
        bg.parse(BACKGROUND)
        self.web.set_background_color(bg)
        settings = self.web.get_settings()
        # The web process writes straight to the real fd, which bin/gate-login
        # has already pointed at gate.log -- a JS error is diagnosable.
        settings.set_property("enable-write-console-messages-to-stdout", True)
        settings.set_property("enable-back-forward-navigation-gestures", False)
        settings.set_property("enable-developer-extras", bool(os.environ.get("INTENTIONALITY_GATE_INSPECT")))
        if os.environ.get("INTENTIONALITY_GATE_SOFTWARE"):
            # Pair with the launchers' WEBKIT_DISABLE_DMABUF_RENDERER: slow,
            # but it draws where the accelerated path might not.
            settings.set_property("hardware-acceleration-policy",
                                  WebKit.HardwareAccelerationPolicy.NEVER)
        self.web.connect("load-failed", self._on_load_failed)
        self.web.connect("web-process-terminated", self._on_web_gone)

        self.window = Gtk.Window(title="intentionality")
        # GTK 4 draws its own titlebar on Wayland whatever the compositor
        # does, so this is what removes the close button under cage.
        self.window.set_decorated(False)
        self.window.set_default_size(1200, 800)
        self.window.set_child(self.web)
        self.window.connect("close-request", self._on_close)
        keys = Gtk.EventControllerKey()
        # Bubble phase, deliberately: the focused webview sees keys first, so
        # typing in the add box is never swallowed. The page answers a choice
        # itself; this controller is the backstop for when it cannot.
        keys.connect("key-pressed", self._on_key)
        self.window.add_controller(keys)

        # Capture phase, the opposite of the keys and just as deliberate: the
        # window sees each scroll before WebKit does, and claiming it keeps
        # WebKit from scrolling or coasting at all. See TOUCHPAD_PX.
        self._natural = self._natural_scroll()
        self._inverted = False
        scroll = Gtk.EventControllerScroll.new(
            Gtk.EventControllerScrollFlags.BOTH_AXES | Gtk.EventControllerScrollFlags.KINETIC
        )
        scroll.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
        scroll.connect("scroll-begin", self._on_scroll_begin)
        scroll.connect("scroll", self._on_scroll)
        scroll.connect("decelerate", self._on_decelerate)
        self.window.add_controller(scroll)

        GLib.unix_signal_add(GLib.PRIORITY_HIGH, signal.SIGINT, self._abort)
        self.window.present()
        self._apply_zoom()
        self.window.connect("notify::scale-factor", self._apply_zoom)

        self.web.load_html(html, BASE_URI)
        self._await_ready()

    # ------------------------------------------------------------- sizing

    def _device_scale(self) -> float:
        """Physical pixels per logical pixel, as the compositor reports it.
        GNOME hands this window 1728x1080 at 1.667; cage does no scaling at
        all and hands it 2880x1800 at 1."""
        display = self.Gdk.Display.get_default()
        monitor = None
        surface = self.window.get_surface()
        if surface is not None:
            monitor = display.get_monitor_at_surface(surface)
        if monitor is None:
            monitors = display.get_monitors()
            monitor = monitors.get_item(0) if monitors.get_n_items() else None
        if monitor is None:
            return 1.0
        scale = 0.0
        if hasattr(monitor, "get_scale"):  # GTK 4.14+, the fractional one
            scale = float(monitor.get_scale() or 0)
        return scale or float(monitor.get_scale_factor() or 1) or 1.0

    def _zoom(self) -> float:
        override = os.environ.get("INTENTIONALITY_GATE_ZOOM")
        if override:
            try:
                return float(override)
            except ValueError:
                pass
        # Hold (zoom x scale) constant and one rem is font_px physical pixels
        # everywhere: 0.975 x 1.667 under GNOME, 1.625 x 1 under cage.
        return (self.font_px / REFERENCE_REM_PX) / self._device_scale()

    def _apply_zoom(self, *_args) -> None:
        self.web.set_zoom_level(self._zoom())

    # -------------------------------------------------------------- bridge

    def _on_message(self, _ucm, value) -> None:
        # WebKit 6.0 hands the JSCValue itself; WebKitJavascriptResult is gone.
        try:
            msg = json.loads(value.to_string())
        except (AttributeError, TypeError, ValueError):
            return
        if not isinstance(msg, dict):
            return
        if msg.get("op") == "ready":
            self._on_ready()
            return
        if self._handler is not None:
            self._handler(msg)

    def _on_ready(self) -> None:
        self._ready = True
        pending, self._queue = self._queue, []
        for state in pending:
            self._push(state)

    def _push(self, state: dict) -> None:
        if not self._ready:
            # Keep only the latest: each state is a whole screen, not a delta.
            self._queue = [state]
            return
        self._last = state
        self.web.evaluate_javascript(
            f"window.__gate && window.__gate.push({json.dumps(state)})", -1
        )

    def _on_load_failed(self, _web, _event, uri, error) -> bool:
        self._fatal = f"the gate page failed to load ({error.message})"
        self._wake()
        return True

    def _on_web_gone(self, _web, reason) -> bool:
        self._fatal = f"the gate page crashed (reason {reason})"
        self._wake()
        return True

    def _wake(self) -> None:
        # A blocked iteration(True) needs an event to notice a flag changed.
        self.GLib.idle_add(lambda: False)

    def _await_ready(self) -> None:
        """Block until the page says hello, or give up. The timeout source is
        what makes giving up possible: iteration(True) blocks forever when
        nothing is pending, so without a timer a page that never loads would
        hang the login rather than fall back to the terminal."""
        context = self.GLib.MainContext.default()
        expired: list = []
        tid = self.GLib.timeout_add(
            int(READY_SECONDS * 1000), lambda: (expired.append(True), False)[1]
        )
        try:
            while not self._ready and not expired and self._fatal is None:
                if self.closed:
                    raise GateAborted
                context.iteration(True)
        finally:
            try:
                self.GLib.Source.remove(tid)
            except Exception:
                pass
        if self._fatal:
            raise RuntimeError(self._fatal)
        if not self._ready:
            raise RuntimeError(f"the gate page did not load within {READY_SECONDS:g}s")

    # ----------------------------------------------------------- the log

    def write(self, text: str) -> int:
        # The real stdout first, always: under cage that is gate.log and under
        # the resume unit it is the journal. A dead page must not cost a line.
        self._real_stdout.write(text)
        self._log.append(text)
        if self._ready and self._log_pending is None:
            self._log_pending = self.GLib.idle_add(self._flush_log)
        return len(text)

    def _flush_log(self) -> bool:
        """Re-push the current screen with the log it should now show. The
        debrief prints a burst of lines at once; idle_add collapses the burst
        into one call."""
        self._log_pending = None
        if self._last is not None and "log" in self._last:
            self._push({**self._last, "log": "".join(self._log)})
        return False

    def flush(self) -> None:
        self._real_stdout.flush()

    # ------------------------------------------------------ the questions

    def confirm_choice(self, prompt: str, choices: str) -> str:
        answer: list = []
        pairs = parse_choices(prompt, choices)

        def handle(msg: dict) -> None:
            if msg.get("op") == "choose" and msg.get("key") in choices:
                answer.append(str(msg["key"]))

        self._handler = handle
        # The backstop path: if the page cannot answer, a keypress still can.
        self._keys = {key: (lambda k=key: answer.append(k)) for key, _ in pairs}
        self._push(
            {
                "screen": "choice",
                "question": question_text(prompt),
                "choices": [{"key": k, "label": label} for k, label in pairs],
                "log": "".join(self._log),
            }
        )
        try:
            self._wait(answer)
        finally:
            self._handler = None
            self._keys = {}
        return answer[0]

    def welcome(
        self,
        tasks: list,
        notes: list,
        error: str,
        labels: list = (),
    ) -> ui.Plan:
        screen = webstate.WelcomeScreen(tasks, list(notes), error, list(labels))
        answer: list = []

        # Typed rows Jev is being asked about, until each answer lands.
        out: list = []

        def handle(msg: dict) -> None:
            screen.handle(msg)
            if screen.plan is not None:
                answer.append(screen.plan)
            else:
                self._sort(screen, out)
                self._push(screen.state())

        self._handler = handle
        self._push(screen.state())
        try:
            # after=noop: the welcome screen is torn down by whatever comes
            # next, and blanking it here would flash the page black first.
            self._wait(answer, after=lambda: None)
        finally:
            self._handler = None
        # A task typed and started in one go is asked about only now, and
        # every answer still out gets a moment, so the session starts with
        # the labels Jev gives.
        self._sort(screen, out)
        self._settle(out)
        return screen.replan()

    def _sort(self, screen: webstate.WelcomeScreen, out: list) -> None:
        """Ask Jev about each newly typed, unlabelled row, one thread each. The
        thread only waits on the network; the answer comes back through the
        main context, so the screen and the store stay on this thread."""
        rows = screen.to_sort()
        if not rows:
            return
        candidates = screen.candidates()

        def landed(row, label) -> bool:
            out[:] = [r for r in out if r is not row]
            if screen.sorted(row, label) and screen.plan is None:
                self._push(screen.state())
            return False

        for row in rows:
            out.append(row)

            def work(row=row, title=row.title, notes=row.details.notes) -> None:
                label = autolabel.ask(title, notes, candidates)
                self.GLib.idle_add(landed, row, label)

            threading.Thread(target=work, daemon=True).start()

    def _settle(self, out: list) -> None:
        """Spin until every answer in `out` has landed or SORT_WAIT is up. An
        answer later than that is dropped with the screen."""
        if not out:
            return
        context = self.GLib.MainContext.default()
        expired: list = []
        tid = self.GLib.timeout_add(
            int(SORT_WAIT * 1000), lambda: (expired.append(True), False)[1]
        )
        try:
            while out and not expired and not self.closed and self._fatal is None:
                context.iteration(True)
        finally:
            try:
                self.GLib.Source.remove(tid)
            except Exception:
                pass

    def _wait(self, answer: list, after=None) -> None:
        """Spin the main loop until `answer` has an element, then clear the
        question away unless `after` says otherwise."""
        context = self.GLib.MainContext.default()
        while not answer:
            if self.closed:
                raise GateAborted
            if self._fatal:
                self.restore()
                raise RuntimeError(self._fatal)
            context.iteration(True)
        if after is None:
            # Whatever answered is gone now; a stale row must not catch a key.
            self._push({"screen": "blank", "log": "".join(self._log)})
        else:
            after()

    _keys: dict = {}

    def _on_key(self, _controller, keyval, _keycode, state) -> bool:
        Gdk = self.Gdk
        if state & (Gdk.ModifierType.CONTROL_MASK | Gdk.ModifierType.ALT_MASK):
            return False
        # 0 for keys with no character (arrows, F-keys); chr(0) matches nothing.
        handler = self._keys.get(chr(Gdk.keyval_to_unicode(keyval)).lower())
        if handler:
            handler()
            return True
        return False

    # ------------------------------------------------------------ scrolling

    def _natural_scroll(self) -> tuple[bool, bool]:
        """(touchpad, mouse): whether the desktop scrolls naturally."""
        from gi.repository import Gio

        source = Gio.SettingsSchemaSource.get_default()
        out = []
        for schema, default in zip(NATURAL_SCHEMAS, NATURAL_DEFAULTS):
            try:
                found = source.lookup(schema, True) if source else None
                out.append(Gio.Settings.new(schema).get_boolean("natural-scroll") if found else default)
            except Exception:
                out.append(default)
        return out[0], out[1]

    def _scroll_page(self, call: str) -> None:
        self.web.evaluate_javascript(f"window.__gateScroll && window.__gateScroll.{call}", -1)

    def _on_scroll_begin(self, _controller) -> None:
        # Fingers on the touchpad again: catch a coast that is still running.
        self._scroll_page("stop()")

    def _on_scroll(self, controller, dx: float, dy: float) -> bool:
        event = controller.get_current_event()
        if event is not None and event.get_modifier_state() & self.Gdk.ModifierType.CONTROL_MASK:
            # Ctrl+scroll would zoom the page; the zoom is the gate's to set.
            return True
        wheel = controller.get_unit() == self.Gdk.ScrollUnit.WHEEL
        inverted = False
        if event is not None and hasattr(event, "get_relative_direction"):
            inverted = event.get_relative_direction() == self.Gdk.ScrollRelativeDirection.INVERTED
        if not wheel:
            self._inverted = inverted  # for the coast that may follow
        x, y = scroll_step(dx, dy, wheel, self._natural[1 if wheel else 0], inverted)
        self._scroll_page(f"by({x:.2f}, {y:.2f})")
        return True

    def _on_decelerate(self, _controller, vx: float, vy: float) -> None:
        # Only a touchpad flick ends this way. Velocity is in surface units a
        # second, so it converts exactly like a delta.
        x, y = scroll_step(vx, vy, False, self._natural[0], self._inverted)
        self._scroll_page(f"fling({x:.2f}, {y:.2f})")

    def _on_close(self, _window) -> bool:
        # Every close arrives here, whoever asked for it: True refuses it.
        return True

    def _abort(self) -> bool:
        # SIGINT only; see the module docstring.
        self.closed = True
        # A blocked iteration(True) needs a wakeup to notice.
        self.GLib.idle_add(lambda: False)
        return False

    def restore(self) -> None:
        """Hand stdout and the ui backend back, so a failure after install()
        still prints where someone can read it."""
        sys.stdout = self._real_stdout
        ui.backend = None


def install(font_px: int | None = None) -> GtkUI:
    """Make the graphical front end the gate's UI, or raise saying why not."""
    if font_px is None:
        try:
            font_px = int(os.environ.get("INTENTIONALITY_GATE_FONT_PX", DEFAULT_FONT_PX))
        except ValueError:
            font_px = DEFAULT_FONT_PX
    front = GtkUI(font_px)
    ui.backend = front
    sys.stdout = front
    return front
