# The graphical gate: one window, under sway, on a bare console.
#
# Sourced by bin/gate-login and bin/resume-gate, never run on its own, so that
# the two launchers start the compositor the same way and cannot drift apart
# -- the same reason recover_open_sessions is shared in python. What differs
# between them stays in them: what happens after the gate (a handoff to
# GNOME, or back to the VT it came from) and what they say when it did not
# complete.
#
# sway, run as a kiosk by gate-sway.conf.in (next to this file): one
# fullscreen window and nothing else, no key bindings, the touchpad set the
# way GNOME has it (natural scrolling, tap to click), and the pointer drawn by
# sway itself from the shapes GTK names over cursor-shape-v1. If sway is not
# installed, or nothing gave this login a runtime directory, or sway ends
# before the gate has finished, the launchers run the text gate.

set -g __graphical_gate_dir (path resolve (status filename) | path dirname)

function graphical_gate_possible --description 'Is there a compositor to draw the gate?'
    # XDG_RUNTIME_DIR comes with a logind session, which is also what lets a
    # compositor open the GPU and the input devices -- so its absence means
    # "text gate", not "try anyway". sway refuses to start without it, and
    # its status file lives in it.
    set -q XDG_RUNTIME_DIR
    and command -q sway
end

function __graphical_gate_gnome --argument-names schema key fallback
    # enabled or disabled, as GNOME has it. Read-only, so it works at a
    # console with no session bus: dconf reads its database file directly. A
    # setting that cannot be read in two seconds is GNOME's default instead;
    # the login path never waits on it.
    set -l value (timeout 2 gsettings get org.gnome.desktop.peripherals.$schema $key 2>/dev/null)
    switch "$value"
        case true
            echo enabled
        case false
            echo disabled
        case '*'
            echo $fallback
    end
end

function graphical_gate_sway_config --argument-names status_file
    # graphical_gate_sway_config STATUS_FILE COMMAND...
    # The sway config that runs COMMAND as the gate and writes its exit status
    # to STATUS_FILE, on stdout. Every value lands inside a quoted sh -c, so
    # anything that would need quoting there is refused rather than escaped:
    # the function then prints nothing and fails, and the launcher runs the
    # text gate.
    set -l command $argv[2..]
    set -l repo (path dirname $__graphical_gate_dir)
    set -l template $__graphical_gate_dir/gate-sway.conf.in
    test -n "$status_file"; and test (count $command) -gt 0; and test -r $template
    or return 1
    for word in $repo $status_file $command
        string match -qr '^[[:alnum:]_./+-]+$' -- $word
        or return 1
    end
    # GNOME's own defaults are the fallbacks: natural scrolling on a
    # touchpad, not on a mouse, and tap to click on.
    set -l text (string replace -a -- @REPO@ $repo < $template \
        | string replace -a -- @STATUS@ $status_file \
        | string replace -a -- @COMMAND@ "$command" \
        | string replace -a -- @TOUCHPAD_NATURAL@ (__graphical_gate_gnome touchpad natural-scroll enabled) \
        | string replace -a -- @TOUCHPAD_TAP@ (__graphical_gate_gnome touchpad tap-to-click enabled) \
        | string replace -a -- @MOUSE_NATURAL@ (__graphical_gate_gnome mouse natural-scroll disabled))
    # A name the template gained and this function never learned.
    string match -qr '@[A-Z_]+@' -- $text
    and return 1
    printf '%s\n' $text
end

function graphical_gate --argument-names log
    # graphical_gate LOG COMMAND...
    # Runs COMMAND (the gate) under sway and returns the gate's own exit
    # status: 0 means it finished. Non-zero means it failed, or sway could not
    # run it. Everything sway and the gate print is appended to LOG.
    set -l command $argv[2..]

    # The environment the gate itself needs -- no accessibility bus, no gvfs,
    # no WebKit sandbox -- is set in gate/gui.py, so that a gate started any
    # other way gets it too.
    #
    # The escape hatch for a first run on real hardware: sway on a bare
    # console drives KMS directly, and WebKit's DMA-BUF path between its GPU
    # and web processes is the likeliest thing to come up black there. Set
    # INTENTIONALITY_GATE_SOFTWARE=1 to take the slow, certain route. (Scoped
    # to this function: the desktop started after the gate never sees them.)
    if set -q INTENTIONALITY_GATE_SOFTWARE
        set -fx WEBKIT_DISABLE_DMABUF_RENDERER 1
        set -fx LIBGL_ALWAYS_SOFTWARE 1
    end

    # A blank console with no cursor. Whenever sway lets go of the screen --
    # while it starts, and for the seconds GNOME takes to draw after it exits
    # -- the console shows its text buffer again, so that buffer must hold
    # nothing: not the login text, and not the gate's own output. The
    # launchers show the cursor again once they are done with the console.
    isatty stdout; and printf '\e[2J\e[3J\e[H\e[?25l'
    mkdir -p (path dirname $log)
    echo "=== "(date -Is)" ===" >> $log

    if not command -q sway
        echo "gate: sway is not installed." >> $log
        return 1
    end

    set -l dir $XDG_RUNTIME_DIR/intentionality
    set -l conf $dir/sway-$fish_pid.conf
    set -l rc $dir/sway-$fish_pid.status
    # A status left by an earlier run must never be read as this one's. Each
    # file is named for its launcher; a gate that outlives a crashed sway
    # writes its status after that launcher has moved on, so sweep up after
    # launchers that are gone, too.
    rm -f $rc
    for stale in $dir/sway-*.status $dir/sway-*.conf
        set -l pid (string match -r -- '/sway-(\d+)\.\w+$' $stale)[2]
        test -n "$pid"; and not test -d /proc/$pid; and rm -f $stale
    end
    # Validated first (see gate-sway.conf.in for why). sway 1.11's -C builds
    # a whole compositor before it reads the file, so it gets the headless
    # backend and the CPU renderer: ~40 ms, and it never touches the GPU, the
    # seat or the console.
    if mkdir -p $dir
        and graphical_gate_sway_config $rc $command > $conf
        and WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 sway -C -c $conf >> $log 2>&1
        sway -c $conf >> $log 2>&1
        set -l sway_status $status
        set -l code (cat $rc 2>/dev/null)
        rm -f $rc $conf
        # The gate finished: its status is the answer, 0 or not. A failed
        # gate is not run again here; it falls through to the text gate.
        if string match -qr '^[0-9]+$' -- "$code"
            return $code
        end
        echo "gate: sway exited $sway_status before the gate finished." >> $log
    else
        rm -f $rc $conf
        echo "gate: no usable sway config; not starting sway." >> $log
    end
    return 1
end
