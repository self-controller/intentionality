//! The ignore list: apps and websites the user never wants judged. An
//! ignored window is treated like this app's own: subject::of has nothing to
//! say about it, so Jev is never asked, the clock never shows on it, and its
//! time is "Not judged" on the Dashboard and in the checks.
//!
//! An app is matched by its window class (its desktop file's id or its
//! `StartupWMClass`, desktop.rs), so ignoring Zen ignores every tab in it. A
//! website is matched by the site the browser's history gives a tab
//! (places.rs) -- itself or any part of it: ignoring google.com ignores
//! docs.google.com. A tab the history does not know falls back to its title
//! naming the site, the at-risk flags' own test ("Feed | LinkedIn" names
//! linkedin.com), for a site with no part before its name.
//!
//! Kept in the store's meta as JSON, like every other setting, and in memory
//! for subject::of, which runs for every span of the day.

use crate::db;
use crate::error::{AppError, Result};
use crate::screen_time;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock, RwLock};

/// The meta key the list is kept under.
const KEY: &str = "ignore_list";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    App,
    Site,
}

/// One ignored app or website.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Rule {
    pub kind: Kind,
    /// An app: its desktop id or window class, lowercased. A site: its
    /// host without "www.": "linkedin.com".
    pub value: String,
    /// What the menu calls it: "Zen Browser", "linkedin.com".
    pub name: String,
    /// An app's window classes, lowercased (`value` among them). Empty for
    /// a site.
    #[serde(default)]
    pub classes: Vec<String>,
}

fn global() -> &'static RwLock<Arc<Vec<Rule>>> {
    static CURRENT: OnceLock<RwLock<Arc<Vec<Rule>>>> = OnceLock::new();
    CURRENT.get_or_init(RwLock::default)
}

/// The list in force. Empty until `set` -- and always under test, where no
/// test calls `set`, so no test sees another's list.
pub fn current() -> Arc<Vec<Rule>> {
    global().read().unwrap().clone()
}

/// Put a list in force.
pub fn set(rules: Vec<Rule>) {
    *global().write().unwrap() = Arc::new(rules);
}

/// The stored list. Unreadable JSON is no list (and said so on stderr): a
/// hand edit gone wrong must not stop the app.
pub fn load(conn: &Connection) -> Vec<Rule> {
    let Some(text) = db::get_setting(conn, KEY).ok().flatten() else { return Vec::new() };
    match serde_json::from_str::<Vec<Rule>>(&text) {
        Ok(rules) => rules.into_iter().filter_map(|r| normalize(r).ok()).collect(),
        Err(err) => {
            eprintln!("intentionality: meta {KEY} is not a list of rules ({err}); ignoring nothing");
            Vec::new()
        }
    }
}

fn save(conn: &Connection, rules: &[Rule]) -> Result<()> {
    let text = serde_json::to_string(rules).map_err(|e| AppError::Other(e.to_string()))?;
    conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![KEY, text])?;
    Ok(())
}

/// Add a rule, store the list and put it in force. Adding what is already
/// there replaces it (an app's classes may have grown).
pub fn add(conn: &Connection, rule: Rule) -> Result<Vec<Rule>> {
    let rules = with(load(conn), normalize(rule)?);
    save(conn, &rules)?;
    set(rules.clone());
    Ok(rules)
}

/// Remove a rule, store the list and put it in force.
pub fn remove(conn: &Connection, kind: Kind, value: &str) -> Result<Vec<Rule>> {
    let value = value.trim().to_lowercase();
    let mut rules = load(conn);
    rules.retain(|r| !(r.kind == kind && r.value == value));
    save(conn, &rules)?;
    set(rules.clone());
    Ok(rules)
}

/// `rules` with `rule` in it, once.
fn with(mut rules: Vec<Rule>, rule: Rule) -> Vec<Rule> {
    match rules.iter_mut().find(|r| r.kind == rule.kind && r.value == rule.value) {
        Some(r) => *r = rule,
        None => rules.push(rule),
    }
    rules
}

/// A rule as it is kept: a site by its host, an app by its lowercased class
/// with that class among its classes. Refused when there is nothing to
/// match by.
pub fn normalize(rule: Rule) -> Result<Rule> {
    match rule.kind {
        Kind::Site => {
            let host = host_of(&rule.value)
                .ok_or_else(|| AppError::Other(format!("\"{}\" is not a website", rule.value.trim())))?;
            Ok(Rule { kind: Kind::Site, name: host.clone(), value: host, classes: Vec::new() })
        }
        Kind::App => {
            let value = rule.value.trim().to_lowercase();
            if value.is_empty() {
                return Err(AppError::Other("no app to ignore".into()));
            }
            let mut classes = vec![value.clone()];
            for c in rule.classes {
                let c = c.trim().to_lowercase();
                if !c.is_empty() && !classes.contains(&c) {
                    classes.push(c);
                }
            }
            let name = match rule.name.trim() {
                "" => value.clone(),
                n => n.to_string(),
            };
            Ok(Rule { kind: Kind::App, value, name, classes })
        }
    }
}

/// The website typed or pasted, as a host without "www.":
/// "https://www.linkedin.com/feed" -> "linkedin.com". None for text that
/// names no site.
pub fn host_of(text: &str) -> Option<String> {
    screen_time::host(text)
}

/// Is a window of this class (lowercased) ignored as an app?
pub fn hides_app(rules: &[Rule], class: &str) -> bool {
    rules.iter().any(|r| r.kind == Kind::App && r.classes.iter().any(|c| c == class))
}

/// Is a tab on this site ignored -- the site itself, or a site it is part of?
pub fn hides_site(rules: &[Rule], host: &str) -> bool {
    let host = host.to_lowercase();
    rules.iter().any(|r| {
        r.kind == Kind::Site
            && (host == r.value || host.strip_suffix(&r.value).is_some_and(|rest| rest.ends_with('.')))
    })
}

/// For a tab the history has no site for: does its title name an ignored
/// site? Only a site with nothing before its name ("linkedin.com", not
/// "docs.google.com") -- a title saying "Google" is no sign of Docs.
pub fn hides_title(rules: &[Rule], title: &str) -> bool {
    rules.iter().any(|r| {
        let keyword = screen_time::keyword(&r.value);
        r.kind == Kind::Site
            && r.value.starts_with(&format!("{keyword}."))
            && screen_time::contains_word(title, &keyword)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(host: &str) -> Rule {
        normalize(Rule { kind: Kind::Site, value: host.into(), name: String::new(), classes: Vec::new() }).unwrap()
    }

    fn app(value: &str, classes: &[&str]) -> Rule {
        normalize(Rule {
            kind: Kind::App,
            value: value.into(),
            name: String::new(),
            classes: classes.iter().map(|c| c.to_string()).collect(),
        })
        .unwrap()
    }

    #[test]
    fn a_typed_site_is_its_host() {
        assert_eq!(host_of("https://www.linkedin.com/feed/").as_deref(), Some("linkedin.com"));
        assert_eq!(host_of("LinkedIn.com").as_deref(), Some("linkedin.com"));
        assert_eq!(host_of("docs.google.com/document/d/1").as_deref(), Some("docs.google.com"));
        assert_eq!(host_of("http://bbc.co.uk?x=1").as_deref(), Some("bbc.co.uk"));
        assert_eq!(host_of("discord"), None);
        assert_eq!(host_of(""), None);
        assert!(normalize(Rule { kind: Kind::Site, value: "discord".into(), name: String::new(), classes: Vec::new() }).is_err());
        assert_eq!(site("https://www.linkedin.com/in/someone").name, "linkedin.com");
    }

    #[test]
    fn an_app_is_its_classes() {
        let code = app("com.visualstudio.code", &["Code", "com.visualstudio.code"]);
        assert_eq!(code.classes, ["com.visualstudio.code", "code"]);
        assert!(hides_app(&[code.clone()], "code"));
        assert!(hides_app(&[code.clone()], "com.visualstudio.code"));
        assert!(!hides_app(&[code], "codium"));
        assert!(normalize(Rule { kind: Kind::App, value: " ".into(), name: String::new(), classes: Vec::new() }).is_err());
        // A site rule never hides an app.
        assert!(!hides_app(&[site("zen.com")], "zen"));
    }

    #[test]
    fn a_site_hides_itself_and_its_parts() {
        let rules = [site("google.com"), site("linkedin.com")];
        assert!(hides_site(&rules, "google.com"));
        assert!(hides_site(&rules, "docs.google.com"));
        assert!(hides_site(&rules, "LinkedIn.com"));
        assert!(!hides_site(&rules, "notgoogle.com"));
        assert!(!hides_site(&rules, "google.com.evil.net"));
        // A part ignored is only that part.
        let docs = [site("docs.google.com")];
        assert!(hides_site(&docs, "docs.google.com"));
        assert!(!hides_site(&docs, "mail.google.com"));
        assert!(!hides_site(&docs, "google.com"));
    }

    #[test]
    fn a_title_names_a_site_only_by_its_whole_name() {
        let rules = [site("linkedin.com"), site("docs.google.com"), site("x.com")];
        assert!(hides_title(&rules, "Feed | LinkedIn"));
        assert!(hides_title(&rules, "Home / X"));
        assert!(!hides_title(&rules, "xdg-desktop-portal"));
        // "Google" in a title is no sign of Docs.
        assert!(!hides_title(&rules, "why do warheads taste fishy - Google Search"));
        assert!(!hides_title(&[app("zen", &[])], "Zen"));
    }

    #[test]
    fn the_list_is_kept_in_meta_once_per_thing() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT)").unwrap();
        assert!(load(&conn).is_empty());
        let rules = with(with(Vec::new(), site("linkedin.com")), app("zen", &[]));
        let rules = with(rules, app("zen", &["app.zen_browser.zen"]));
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[1].classes, ["zen", "app.zen_browser.zen"]);
        save(&conn, &rules).unwrap();
        assert_eq!(load(&conn), rules);
        // A hand edit gone wrong reads as nothing ignored.
        conn.execute("UPDATE meta SET value = 'nope' WHERE key = ?1", [KEY]).unwrap();
        assert!(load(&conn).is_empty());
    }
}
