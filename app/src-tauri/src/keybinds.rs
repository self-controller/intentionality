//! Keyboard shortcuts the user has changed, for the app and the gate both.
//!
//! Meta `keybinds` holds a JSON object of overrides only -- action id to
//! combo string ("Ctrl+KeyN", "Super", "" for unbound). Anything not in it
//! is the default. The list of actions, their defaults and what a combo means
//! live in one place, app/src/keybinds.ts; this file only stores the object,
//! and the gate (gate/flow.py) only passes it through to its page.

use crate::db;
use crate::error::Result;
use rusqlite::{params, Connection};
use std::collections::BTreeMap;

const KEY: &str = "keybinds";

pub type Keybinds = BTreeMap<String, String>;

/// The stored overrides. Missing or junk is none: a hand edit gone wrong
/// must leave the defaults working, not the keys dead.
pub fn load(conn: &Connection) -> Keybinds {
    db::get_setting(conn, KEY).ok().flatten().map(|v| parse(&v)).unwrap_or_default()
}

fn parse(value: &str) -> Keybinds {
    serde_json::from_str(value).unwrap_or_default()
}

/// Store the overrides whole, replacing what was there.
pub fn set(conn: &Connection, keybinds: &Keybinds) -> Result<()> {
    let json = serde_json::to_string(keybinds).map_err(|e| crate::error::AppError::Other(e.to_string()))?;
    conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![KEY, json])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)", []).unwrap();
        conn
    }

    #[test]
    fn junk_reads_as_no_overrides() {
        assert!(parse("not json").is_empty());
        assert!(parse("[1, 2]").is_empty());
        assert!(parse("{\"app.newTask\": 3}").is_empty());
        assert!(load(&store()).is_empty());
    }

    #[test]
    fn overrides_round_trip() {
        let conn = store();
        let mut k = Keybinds::new();
        k.insert("app.newTask".into(), "Ctrl+KeyK".into());
        k.insert("gate.focusAdd".into(), "".into());
        set(&conn, &k).unwrap();
        assert_eq!(load(&conn), k);
        set(&conn, &Keybinds::new()).unwrap();
        assert!(load(&conn).is_empty());
    }
}
