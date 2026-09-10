"""Whether waking the machine should re-open the gate.

The threshold question only — everything the resume gate then *does* is the
ordinary gate, recovery sweep and all (see __main__.resume_cmd). Imports no
sqlite3 and nothing network-facing, like flow.py.
"""

from datetime import datetime, timezone

from . import config, store

# Under this many minutes away, waking is just coming back to your desk.
DEFAULT_MIN_AWAY_MINUTES = 30.0
SETTING = "resume_min_away_minutes"


def away_minutes(conn) -> float | None:
    """How long the machine went unattended, measured from the open session's
    last heartbeat.

    The heartbeat is already this store's record of when the machine stopped
    being used — mark_recovered turns it into ended_at for exactly that
    reason — so a suspend needs no separate stamp of its own. Accurate to the
    desktop app's 30s tick.

    None when there is nothing to measure: no open session, or one the
    desktop app never heartbeated.
    """
    row = store.latest_open_session(conn)
    if row is None or row["last_heartbeat"] is None:
        return None
    beat = datetime.fromisoformat(row["last_heartbeat"])
    return (datetime.now(timezone.utc) - beat).total_seconds() / 60


def threshold_minutes(conn) -> float:
    # A setting, not a constant: tunable without touching the units.
    raw = store.get_setting(conn, SETTING)
    try:
        return float(raw)
    except (TypeError, ValueError):
        return DEFAULT_MIN_AWAY_MINUTES


def should_gate(conn) -> bool:
    if config.SKIP_PATH.exists():
        return False
    away = away_minutes(conn)
    # Nothing to measure means nothing to protect: waking a machine that has
    # no stated intention is precisely what the gate is for.
    if away is None:
        return True
    return away >= threshold_minutes(conn)
