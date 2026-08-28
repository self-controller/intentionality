"""Minimal ActivityWatch REST client. Stdlib only.

The dashboard never talks to the tracker process — it reads AW's local API
after the fact, joining on the session's time window. AW being down is a
normal condition (dashboard still shows sessions and tasks), so every failure
surfaces as AWUnavailable rather than a traceback.
"""

import json
import os
import urllib.error
import urllib.parse
import urllib.request

BASE_URL = os.environ.get("INTENTIONALITY_AW_URL", "http://localhost:5600")
TIMEOUT = 3.0


class AWUnavailable(Exception):
    pass


def _get(path: str, params: dict | None = None):
    url = f"{BASE_URL}/api/0/{path}"
    if params:
        url += "?" + urllib.parse.urlencode(params)
    try:
        with urllib.request.urlopen(url, timeout=TIMEOUT) as resp:
            return json.load(resp)
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError) as exc:
        raise AWUnavailable(f"ActivityWatch not reachable at {BASE_URL} ({exc})") from exc


def find_buckets() -> tuple[str | None, str | None]:
    """Return (window_bucket_id, afk_bucket_id); either may be None."""
    buckets = _get("buckets/")
    window = afk = None
    for bucket_id, bucket in buckets.items():
        if bucket.get("type") == "currentwindow":
            window = bucket_id
        elif bucket.get("type") == "afkstatus":
            afk = bucket_id
    return window, afk


def events(bucket_id: str, start_iso: str, end_iso: str) -> list[dict]:
    """Events overlapping [start, end], oldest first. Timestamps are ISO UTC."""
    result = _get(
        f"buckets/{bucket_id}/events",
        {"start": start_iso, "end": end_iso, "limit": -1},
    )
    return sorted(result, key=lambda e: e["timestamp"])
