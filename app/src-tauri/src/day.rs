//! Where a day starts. A day runs from a chosen hour to the same hour the
//! next morning -- 4 am unless Settings says otherwise -- so an evening that
//! runs past midnight stays on the day it began. It is named by the date it
//! began on: 02:00 on Nov 1 is still Oct 31.
//!
//! The Dashboard's days and the off-task clock's daily total (screen_time.rs)
//! both go by it, so the two never disagree about which day a minute was.
//! Task due dates are calendar days and do not.
//!
//! The hour lives in meta `day_start_hour` and is held in a process-global,
//! loaded once at startup (ignore.rs's pattern), so nothing needs the store
//! to know it.

use crate::db;
use crate::error::{AppError, Result};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use rusqlite::{params, Connection};
use std::sync::atomic::{AtomicU32, Ordering};

const KEY: &str = "day_start_hour";
pub const DEFAULT_HOUR: u32 = 4;

static HOUR: AtomicU32 = AtomicU32::new(DEFAULT_HOUR);

/// The stored hour, 0-23. Missing, junk or out of range is the default: a
/// hand edit gone wrong must not move the day somewhere strange.
pub fn load(conn: &Connection) -> u32 {
    db::get_setting(conn, KEY).ok().flatten().and_then(|v| parse(&v)).unwrap_or(DEFAULT_HOUR)
}

fn parse(value: &str) -> Option<u32> {
    value.trim().parse::<u32>().ok().filter(|h| *h < 24)
}

/// The hour in force.
pub fn hour() -> u32 {
    HOUR.load(Ordering::Relaxed)
}

/// Put an hour in force without storing it (startup).
pub fn put(hour: u32) {
    HOUR.store(hour, Ordering::Relaxed);
}

/// Store an hour and put it in force. The clock picks it up on its next
/// pass: a new day key is a new day's total.
pub fn set(conn: &Connection, hour: u32) -> Result<()> {
    if hour >= 24 {
        return Err(AppError::Other(format!("a day starts at an hour from 0 to 23, not {hour}")));
    }
    conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![KEY, hour.to_string()])?;
    put(hour);
    Ok(())
}

/// The day a moment belongs to, when days start at `hour`. Local wall time,
/// so a DST change moves nothing: 03:59 is the day before, 04:00 is its own.
pub fn of(now: DateTime<Local>, hour: u32) -> NaiveDate {
    (now.naive_local() - Duration::hours(hour.into())).date()
}

/// Where a day starts, when days start at `hour`.
pub fn start(date: NaiveDate, hour: u32) -> DateTime<Utc> {
    let at = date.and_hms_opt(hour, 0, 0).expect("an hour under 24 exists");
    Local
        .from_local_datetime(&at)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        // A zone that skips that hour for DST: count from the naive time.
        .unwrap_or_else(|| at.and_utc())
}

/// A day as a window: today runs to now, a past day to the next one's
/// start. A day that has not begun has no window.
pub fn window(date: NaiveDate, today: NaiveDate, now: DateTime<Utc>, hour: u32) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    if date > today {
        return Err(AppError::Other(format!("{date} has not begun")));
    }
    let end = if date == today { now } else { start(date + Duration::days(1), hour) };
    Ok((start(date, hour), end))
}

/// Today, by the hour in force.
pub fn today() -> NaiveDate {
    of(Local::now(), hour())
}

/// Where today started, by the hour in force.
pub fn today_start() -> DateTime<Utc> {
    start(today(), hour())
}

/// A day as the frontend names it: "YYYY-MM-DD", and only that spelling
/// (chrono would also take "2026-9-1").
pub fn parse_date(day: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .ok()
        .filter(|d| d.format("%Y-%m-%d").to_string() == day)
        .ok_or_else(|| AppError::Other(format!("a day is YYYY-MM-DD, not {day:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn a_day_turns_over_at_its_hour() {
        assert_eq!(of(local(2026, 11, 1, 3, 59), 4), date(2026, 10, 31));
        assert_eq!(of(local(2026, 11, 1, 4, 0), 4), date(2026, 11, 1));
        assert_eq!(of(local(2026, 11, 1, 23, 59), 4), date(2026, 11, 1));
        assert_eq!(start(date(2026, 10, 31), 4).with_timezone(&Local), local(2026, 10, 31, 4, 0));
    }

    #[test]
    fn hour_zero_is_local_midnight() {
        let now = local(2026, 10, 19, 15, 30);
        assert_eq!(of(now, 0), date(2026, 10, 19));
        assert_eq!(start(of(now, 0), 0).with_timezone(&Local), local(2026, 10, 19, 0, 0));
        assert_eq!(of(local(2026, 10, 19, 0, 0), 0), date(2026, 10, 19));
    }

    #[test]
    fn today_runs_to_now_and_a_past_day_to_the_next_start() {
        let now = local(2026, 11, 1, 15, 0).with_timezone(&Utc);
        let today = date(2026, 11, 1);
        let (s, e) = window(today, today, now, 4).unwrap();
        assert_eq!((s, e), (start(today, 4), now));
        let (s, e) = window(date(2026, 10, 30), today, now, 4).unwrap();
        assert_eq!(s.with_timezone(&Local), local(2026, 10, 30, 4, 0));
        assert_eq!(e.with_timezone(&Local), local(2026, 10, 31, 4, 0));
        assert!(window(date(2026, 11, 2), today, now, 4).is_err());
    }

    #[test]
    fn a_stored_hour_out_of_range_or_junk_is_the_default() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT)").unwrap();
        assert_eq!(load(&conn), DEFAULT_HOUR);
        for (stored, want) in [("6", 6), (" 0 ", 0), ("23", 23), ("24", 4), ("-1", 4), ("4.5", 4), ("noon", 4)] {
            conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![KEY, stored]).unwrap();
            assert_eq!(load(&conn), want, "{stored:?}");
        }
    }

    #[test]
    fn only_the_canonical_spelling_names_a_day() {
        assert_eq!(parse_date("2026-10-31").unwrap(), date(2026, 10, 31));
        for junk in ["2026-9-1", "2026-10-31T00:00", "", "yesterday", "2026-02-30"] {
            assert!(parse_date(junk).is_err(), "{junk:?}");
        }
    }
}
