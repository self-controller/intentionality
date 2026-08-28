"""Turn a session's time window into an intention-vs-observed report.

Observed time is interpretation, not fact: window events are clipped to the
session window and AFK spans are subtracted, all at read time. The raw events
stay in ActivityWatch; nothing here writes anywhere.
"""

from datetime import datetime, timedelta, timezone

from . import aw

STATUS_MARK = {"done": "✓", "dropped": "✗", "planned": "·"}
BAR_WIDTH = 24


def parse_ts(iso: str) -> datetime:
    dt = datetime.fromisoformat(iso)
    return dt if dt.tzinfo else dt.replace(tzinfo=timezone.utc)


def observed(start_iso: str, end_iso: str) -> tuple[dict[str, float], float, float]:
    """Per-app active seconds within the window, plus (active, afk) totals."""
    window_bucket, afk_bucket = aw.find_buckets()
    if window_bucket is None:
        raise aw.AWUnavailable("no window-watcher bucket found")

    start, end = parse_ts(start_iso), parse_ts(end_iso)

    afk_spans = []
    if afk_bucket is not None:
        for event in aw.events(afk_bucket, start_iso, end_iso):
            if event["data"].get("status") == "afk":
                span_start = parse_ts(event["timestamp"])
                afk_spans.append((span_start, span_start + _dur(event)))

    per_app: dict[str, float] = {}
    afk_total = 0.0
    for event in aw.events(window_bucket, start_iso, end_iso):
        ev_start = parse_ts(event["timestamp"])
        ev_end = ev_start + _dur(event)
        clipped_start, clipped_end = max(ev_start, start), min(ev_end, end)
        if clipped_end <= clipped_start:
            continue
        seconds = (clipped_end - clipped_start).total_seconds()
        for span_start, span_end in afk_spans:
            overlap = (
                min(clipped_end, span_end) - max(clipped_start, span_start)
            ).total_seconds()
            if overlap > 0:
                seconds -= overlap
                afk_total += overlap
        if seconds > 0:
            app = event["data"].get("app") or "unknown"
            per_app[app] = per_app.get(app, 0.0) + seconds

    return per_app, sum(per_app.values()), afk_total


def print_session(session, tasks) -> None:
    start = parse_ts(session["started_at"])
    end_iso = session["ended_at"]
    open_note = ""
    if end_iso is None:
        end_iso = datetime.now(timezone.utc).isoformat(timespec="seconds")
        open_note = " (still open)" if session["close_reason"] is None else " (unclosed)"

    minutes = round((parse_ts(end_iso) - start).total_seconds() / 60)
    intended = (
        f", intended {session['intended_minutes']}"
        if session["intended_minutes"] is not None
        else ""
    )
    print(f"\nSession {session['id']} — {session['statement']}")
    print(f"  {start.astimezone():%a %b %d %H:%M} · {minutes} min{intended}{open_note}")

    done = sum(1 for t in tasks if t["status"] == "done")
    print(f"  Tasks ({done}/{len(tasks)} done):")
    for task in tasks:
        print(f"    {STATUS_MARK[task['status']]} {task['title']}")

    try:
        per_app, active, afk_total = observed(session["started_at"], end_iso)
    except aw.AWUnavailable as exc:
        print(f"  Observed: unavailable — {exc}")
        return

    print(f"  Observed ({_fmt(active)} active, {_fmt(afk_total)} away):")
    if not per_app:
        print("    no window events in this session's window")
    top = sorted(per_app.items(), key=lambda kv: -kv[1])
    width = max(len(app) for app, _ in top[:8]) if top else 0
    for app, seconds in top[:8]:
        bar = "█" * max(1, round(seconds / active * BAR_WIDTH)) if active else ""
        print(f"    {app:<{width}}  {_fmt(seconds):>7}  {bar}")
    if len(top) > 8:
        rest = sum(s for _, s in top[8:])
        print(f"    {'(other)':<{width}}  {_fmt(rest):>7}")


def print_session_list(rows) -> None:
    print("recent sessions:")
    for s in rows:
        start = parse_ts(s["started_at"])
        state = s["close_reason"] or "open"
        print(f"  {s['id']:>3}  {start.astimezone():%b %d %H:%M}  [{state:<9}]  {s['statement']}")


def _dur(event) -> timedelta:
    return timedelta(seconds=float(event.get("duration", 0) or 0))


def _fmt(seconds: float) -> str:
    minutes = int(seconds // 60)
    if minutes >= 60:
        return f"{minutes // 60}h {minutes % 60:02d}m"
    if minutes:
        return f"{minutes}m"
    return f"{int(seconds)}s"
