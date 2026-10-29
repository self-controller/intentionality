// intentionality's at-risk timer: a giant clock at the top right of the
// screen while an app or website the user flagged is the focused window and
// no task on the board excuses the page.
//
// GNOME on Wayland gives an app no way to place its own window or keep it
// above the others, so the app (app/src-tauri/src/screen_time.rs) decides what
// to show and calls this over D-Bus; this only draws. The object is exported
// on gnome-shell's own session-bus connection, so callers address it at
// org.gnome.Shell -- the way Focused Window D-Bus works.
//
// Two things flow the other way, as signals. `Focus` reports every change of
// focused window or of its title the moment it happens, which is what lets
// the app react in milliseconds rather than wait on ActivityWatch's poll
// (Focused Window D-Bus only signals a change of *app*, so a tab switch in
// one browser never shows up there). `Appeal` carries the user's case for
// the window: the clock's arrow opens a box under the clock with the
// keyboard already in it, each Enter sends the whole case so far, and the
// app's verdict comes back through `Answer`.
//
// The clock counts by itself between calls, and hides itself when calls stop:
// a dead app, a stalled loop or a suspend can never leave a stale clock up.

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Pango from 'gi://Pango';
import Shell from 'gi://Shell';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import {GrabHelper} from 'resource:///org/gnome/shell/ui/grabHelper.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import {clock, follow, reading} from './clock.js';

// app/src-tauri/src/overlay.rs::VERSION must equal this (a Rust test reads
// it from here). Bump both when the interface or behaviour changes.
const VERSION = 3;

const OBJECT_PATH = '/org/intentionality/Timer';
const INTERFACE = `<node>
  <interface name="org.intentionality.Timer">
    <method name="Show">
      <arg type="s" direction="in" name="label"/>
      <arg type="d" direction="in" name="seconds"/>
      <arg type="s" direction="in" name="caption"/>
      <arg type="b" direction="in" name="appealable"/>
      <arg type="u" direction="out" name="version"/>
    </method>
    <method name="Hide">
      <arg type="u" direction="out" name="version"/>
    </method>
    <method name="Version">
      <arg type="u" direction="out" name="version"/>
    </method>
    <method name="Focused">
      <arg type="s" direction="out" name="app"/>
      <arg type="s" direction="out" name="title"/>
    </method>
    <method name="State">
      <arg type="s" direction="out" name="state"/>
    </method>
    <signal name="Focus">
      <arg type="s" name="app"/>
      <arg type="s" name="title"/>
    </signal>
    <method name="Answer">
      <arg type="s" direction="in" name="text"/>
      <arg type="b" direction="in" name="won"/>
      <arg type="u" direction="out" name="version"/>
    </method>
    <signal name="Appeal">
      <arg type="s" name="app"/>
      <arg type="s" name="title"/>
      <arg type="as" name="argument"/>
    </signal>
  </interface>
</node>`;

/** The app sends a Show every second while the window is at risk. */
const STALE_SECONDS = 4;
/** Redraw this often, so a new second lands within a quarter of one. */
const TICK_MS = 250;
/** Gap from the monitor's right edge and from the top bar. */
const MARGIN = 24;
/** A won case's answer stays this long after the app lets the clock go, so
 *  it can be read. */
const LINGER_SECONDS = 2.5;
/** The longest a sent case keeps the clock up waiting for its answer (the
 *  app gives Jev 10 s). */
const AWAIT_SECONDS = 15;

const monotonic = () => GLib.get_monotonic_time() / GLib.USEC_PER_SEC;

export default class AtRiskTimer extends Extension {
    enable() {
        this._stretch = null;
        this._lastShow = 0;
        this._tick = 0;
        this._focusWindow = null;
        this._titleId = 0;
        // What the app last asked for under the clock.
        this._appCaption = '';
        this._appealable = false;
        // The case being argued: {app, title, monitor} while the box is open.
        this._asking = null;
        this._argument = [];
        this._awaitingSince = 0; // when the last case was sent, 0 once answered
        this._reply = '';
        this._lingerUntil = 0;
        this._hidePending = false;

        this._label = new St.Label({style_class: 'at-risk-label'});
        this._clock = new St.Label({style_class: 'at-risk-clock'});
        this._caption = new St.Label({style_class: 'at-risk-caption', x_expand: true});
        // A reply can be longer than the clock is wide: wrap it (CSS caps the
        // width) rather than cut it.
        this._caption.clutter_text.line_wrap = true;
        this._caption.clutter_text.ellipsize = Pango.EllipsizeMode.NONE;
        // The one thing on the clock that takes a click: it is reactive while
        // the box around it is not, so the rest stays click-through.
        this._appeal = new St.Button({
            style_class: 'at-risk-appeal',
            reactive: true,
            can_focus: false,
            track_hover: true,
            child: new St.Icon({icon_name: 'go-next-symbolic', style_class: 'at-risk-appeal-icon'}),
        });
        this._appeal.connect('clicked', () => this._onAppeal());
        this._row = new St.BoxLayout({
            style_class: 'at-risk-row',
            orientation: Clutter.Orientation.HORIZONTAL,
            reactive: false,
            x_expand: true,
        });
        this._row.add_child(this._caption);
        this._row.add_child(this._appeal);
        // Nothing here but the arrow is reactive, so the pointer goes straight
        // through to the window underneath: the timer can be seen but never
        // dragged or dismissed.
        this._box = new St.BoxLayout({
            style_class: 'at-risk-timer',
            orientation: Clutter.Orientation.VERTICAL,
            reactive: false,
            can_focus: false,
            track_hover: false,
            visible: false,
        });
        // Where the case is argued. Reactive and focusable inside the
        // click-through box, like the arrow; shown only while asking.
        this._entry = new St.Entry({
            style_class: 'at-risk-entry',
            hint_text: 'Why does this serve a task? Enter sends · Esc closes',
            can_focus: true,
            reactive: true,
            visible: false,
            x_expand: true,
        });
        this._entry.clutter_text.connect('activate', () => this._send());

        this._box.add_child(this._label);
        this._box.add_child(this._clock);
        this._box.add_child(this._row);
        this._box.add_child(this._entry);
        // While the box is open the shell holds the keyboard, the way a popup
        // menu does: on GNOME 50 key focus alone leaves it with the window.
        // Esc or a click outside the clock lets go, and the box closes.
        this._grab = new GrabHelper(this._box, {actionMode: Shell.ActionMode.POPUP});
        // Top chrome: above every window, fullscreen ones included -- a
        // fullscreen video is the case this exists for.
        Main.layoutManager.addTopChrome(this._box, {trackFullscreen: false});

        Main.overview.connectObject(
            'showing', () => this._render(),
            'hidden', () => this._render(),
            this);

        this._service = Gio.DBusExportedObject.wrapJSObject(INTERFACE, this);
        this._service.export(Gio.DBus.session, OBJECT_PATH);

        global.display.connectObject('notify::focus-window', () => this._onFocus(), this);
        this._onFocus();
    }

    disable() {
        this._closeAsk();
        global.display.disconnectObject(this);
        this._watchTitle(null);
        this._service.unexport();
        this._service = null;
        Main.overview.disconnectObject(this);
        this._stopTicking();
        this._box.destroy();
        this._box = this._label = this._clock = this._caption = this._row = this._appeal = null;
        this._entry = this._grab = null;
        this._stretch = null;
    }

    // --- D-Bus methods -----------------------------------------------------

    Show(label, seconds, caption, appealable) {
        const now = monotonic();
        this._stretch = follow(this._stretch, label, seconds, now);
        this._lastShow = now;
        this._hidePending = false;
        this._label.text = `${label.toUpperCase()} · TODAY`;
        this._appCaption = caption;
        this._appealable = appealable;
        this._syncCaption();
        this._startTicking();
        this._render();
        return VERSION;
    }

    Hide() {
        // A won case's answer, or one still on its way, is read first: the
        // tick hides the clock once it is.
        if (this._holding()) {
            this._hidePending = true;
            return VERSION;
        }
        this._clear();
        return VERSION;
    }

    /** The app's verdict on the case. Lost: the box takes the next line.
     *  Won: the box closes and the answer stays up a moment. */
    Answer(text, won) {
        this._awaitingSince = 0;
        if (won) {
            this._reply = text;
            this._lingerUntil = monotonic() + LINGER_SECONDS;
            this._closeAsk(true);
        } else if (this._asking) {
            this._reply = text;
            this._entry.clutter_text.editable = true;
            this._syncCaption();
        }
        this._render();
        return VERSION;
    }

    Version() {
        return VERSION;
    }

    /** The window in front: [window class, title], both '' for none. */
    Focused() {
        return this._focused();
    }

    /** What is on screen, for checking from a shell: GNOME refuses screenshots
     *  to other programs, so this is how the timer can be seen without eyes. */
    State() {
        const [x, y] = this._box.get_transformed_position();
        const [width, height] = this._box.get_transformed_size();
        const [app, title] = this._focused();
        return JSON.stringify({
            version: VERSION,
            visible: this._box.visible,
            label: this._label.text,
            clock: this._clock.text,
            caption: this._caption.visible ? this._caption.text : '',
            appealable: this._appeal.visible,
            asking: this._asking !== null,
            entry: this._entry.visible ? this._entry.text : null,
            keyFocus: global.stage.get_key_focus() === this._entry.clutter_text,
            grabbed: this._grab.grabbed,
            focus: {app, title},
            x: Math.round(x),
            y: Math.round(y),
            width: Math.round(width),
            height: Math.round(height),
            monitor: this._monitor()?.index ?? null,
        });
    }

    // --- the focused window ------------------------------------------------

    /** Should the box ever leave no window focused, the window being argued
     *  about is still the one in front. */
    _focused() {
        const w = global.display.focus_window;
        if (!w && this._asking)
            return [this._asking.app, this._asking.title];
        return [w?.get_wm_class() ?? '', w?.get_title() ?? ''];
    }

    _onFocus() {
        // Another window came to the front (a new one opened): the box goes.
        const w = global.display.focus_window;
        if (this._asking && w && w !== this._asking.window)
            this._closeAsk();
        this._watchTitle(global.display.focus_window);
        this._emitFocus();
    }

    /** A browser's title changes with every tab and page while the window
     *  stays the same, so the focused window's title is watched too. */
    _watchTitle(window) {
        if (this._focusWindow && this._titleId) {
            if (!this._focusWindow.is_destroyed?.())
                this._focusWindow.disconnect(this._titleId);
        }
        this._focusWindow = window;
        this._titleId = window ? window.connect('notify::title', () => this._emitFocus()) : 0;
    }

    _emitFocus() {
        const [app, title] = this._focused();
        this._service?.emit_signal('Focus', new GLib.Variant('(ss)', [app, title]));
        if (this._stretch !== null)
            this._render(); // the clock follows the window across monitors
    }

    // --- the appeal --------------------------------------------------------

    /** The arrow: open the box under the clock, keyboard in it. */
    _onAppeal() {
        const [app, title] = this._focused();
        if (!app || this._asking)
            return;
        const window = global.display.focus_window;
        this._asking = {app, title, window, monitor: window?.get_monitor() ?? -1};
        this._argument = [];
        this._reply = '';
        this._entry.text = '';
        this._entry.clutter_text.editable = true;
        this._entry.show();
        this._syncCaption();
        this._render();
        // Reactive while open, so a click on the clock itself stays inside.
        this._box.reactive = true;
        this._grab.grab({
            actor: this._box,
            focus: this._entry.clutter_text,
            onUngrab: () => this._closeAsk(),
        });
    }

    /** Enter: the whole case so far goes to the app. */
    _send() {
        const line = this._entry.text.trim();
        if (!this._asking || !line || !this._entry.clutter_text.editable)
            return;
        this._argument.push(line);
        this._entry.text = '';
        this._entry.clutter_text.editable = false;
        this._awaitingSince = monotonic();
        this._reply = 'Asking Jev…';
        this._syncCaption();
        this._service?.emit_signal('Appeal', new GLib.Variant('(ssas)',
            [this._asking.app, this._asking.title, this._argument]));
    }

    /** Close the box and hand the keyboard back to the window in front.
     *  `keepReply`: a won case's answer stays under the clock. */
    _closeAsk(keepReply = false) {
        if (!this._asking)
            return;
        this._asking = null; // first: letting go below calls back here
        this._argument = [];
        if (!keepReply)
            this._reply = '';
        if (this._grab.grabbed)
            this._grab.ungrab({actor: this._box});
        this._box.reactive = false;
        this._entry.hide();
        this._syncCaption();
        this._render();
    }

    /** Waiting on an answer, or showing a won one: Hide holds off. */
    _holding() {
        const now = monotonic();
        return (this._awaitingSince && now - this._awaitingSince < AWAIT_SECONDS) ||
            now < this._lingerUntil;
    }

    _syncCaption() {
        const text = this._reply || this._appCaption;
        this._caption.text = text;
        this._caption.visible = text !== '';
        this._appeal.visible = this._appealable && !this._asking && !this._reply;
        this._row.visible = this._caption.visible || this._appeal.visible;
    }

    // --- drawing -----------------------------------------------------------

    _startTicking() {
        if (this._tick)
            return;
        this._tick = GLib.timeout_add(GLib.PRIORITY_DEFAULT, TICK_MS, () => {
            const done = this._hidePending || monotonic() - this._lastShow > STALE_SECONDS;
            if (done && !this._holding()) {
                this._tick = 0;
                this._clear();
                return GLib.SOURCE_REMOVE;
            }
            this._render();
            return GLib.SOURCE_CONTINUE;
        });
    }

    _stopTicking() {
        if (this._tick) {
            GLib.source_remove(this._tick);
            this._tick = 0;
        }
    }

    /** Off the screen, and everything about the case with it. */
    _clear() {
        this._closeAsk();
        this._reply = '';
        this._lingerUntil = 0;
        this._awaitingSince = 0;
        this._hidePending = false;
        this._stretch = null;
        this._stopTicking();
        this._syncCaption();
        this._render();
    }

    /** The monitor the focused window is on: on a second screen, the timer
     *  belongs beside the window it is about. */
    _monitor() {
        const layout = Main.layoutManager;
        const index = this._asking?.monitor ?? global.display.focus_window?.get_monitor() ?? -1;
        return layout.monitors[index] ?? layout.primaryMonitor;
    }

    _render() {
        const show = this._stretch !== null && !Main.overview.visible;
        if (!show) {
            this._box.hide();
            return;
        }
        this._clock.text = clock(reading(this._stretch, monotonic()));
        const monitor = this._monitor();
        if (!monitor)
            return;
        const [, width] = this._box.get_preferred_width(-1);
        const top = monitor.index === Main.layoutManager.primaryIndex
            ? Main.layoutManager.panelBox.height
            : 0;
        this._box.set_position(
            Math.round(monitor.x + monitor.width - width - MARGIN),
            Math.round(monitor.y + top + MARGIN));
        this._box.show();
    }
}
