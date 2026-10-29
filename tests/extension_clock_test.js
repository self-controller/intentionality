// Run by tests/test_extension.py as `gjs -m`: the at-risk timer's arithmetic,
// which is the only part of the GNOME Shell extension that can run outside
// gnome-shell. Throws (exit status 1) on the first failure.

import {clock, follow, reading, REBASE_SLACK} from '../gnome-extension/at-risk-timer@intentionality/clock.js';

function eq(actual, expected, what) {
    const a = JSON.stringify(actual), e = JSON.stringify(expected);
    if (a !== e)
        throw new Error(`${what}: expected ${e}, got ${a}`);
}

eq(clock(0), '0:00:00', 'zero');
eq(clock(59.9), '0:00:59', 'seconds are floored, never rounded up');
eq(clock(754), '0:12:34', 'minutes');
eq(clock(3600 * 4 + 20 * 60 + 15), '4:20:15', 'hours');
eq(clock(-3), '0:00:00', 'never negative');

// A first Show, and a new label, start a stretch at the value given.
let s = follow(null, 'linkedin.com', 100, 10);
eq(s, {label: 'linkedin.com', base: 100, at: 10}, 'first show');
eq(reading(s, 12.5), 102.5, 'counts on by itself');
eq(follow(s, 'discord', 7, 11), {label: 'discord', base: 7, at: 11}, 'new label');

// Ordinary jitter is ignored: the clock keeps its own steady count.
eq(follow(s, 'linkedin.com', 101.8, 11), s, 'jitter ahead');
eq(follow(s, 'linkedin.com', 100.4, 11), s, 'jitter behind');
// Well ahead: jump to it.
eq(follow(s, 'linkedin.com', 101 + REBASE_SLACK + 0.5, 11),
    {label: 'linkedin.com', base: 101 + REBASE_SLACK + 0.5, at: 11}, 'far ahead');
// Well behind: never step back within a stretch.
eq(follow(s, 'linkedin.com', 50, 11), s, 'far behind');

print('ok');
