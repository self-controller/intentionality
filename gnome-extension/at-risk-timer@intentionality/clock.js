// The timer's arithmetic, apart from the shell so gjs can test it alone
// (tests/test_extension.py). No gi:// imports here.

/** How far a Show may run ahead of the clock's own count before the clock
 *  jumps to it. Below this the clock keeps its own steady count: the app's
 *  numbers arrive once a second with jitter, and following each one would
 *  make the seconds stutter. */
export const REBASE_SLACK = 2;

/** h:mm:ss, always all three, so the clock never changes shape mid-stretch
 *  except at 10 h. */
export function clock(seconds) {
    const s = Math.max(0, Math.floor(seconds));
    const pad = n => String(n).padStart(2, '0');
    return `${Math.floor(s / 3600)}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}`;
}

/**
 * Where the clock counts from after a Show of `seconds` for `label` at
 * monotonic time `now` (seconds). `stretch` is the current one, or null when
 * the clock is hidden. A new label starts a new stretch at the value given.
 * Within a stretch the clock only ever moves forward: it jumps ahead when the
 * app's count is well ahead of its own, and never back -- the next stretch
 * starts from the true value anyway.
 */
export function follow(stretch, label, seconds, now) {
    if (!stretch || stretch.label !== label)
        return {label, base: seconds, at: now};
    const predicted = stretch.base + (now - stretch.at);
    if (seconds > predicted + REBASE_SLACK)
        return {label, base: seconds, at: now};
    return stretch;
}

/** The count a stretch shows at monotonic time `now`. */
export function reading(stretch, now) {
    return stretch.base + (now - stretch.at);
}
