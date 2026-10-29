//! What a browser's own history says about the tab in front: the site it is
//! on and the page it sits under. A tab's title alone can be as bare as
//! "Files" -- Canvas titles a course's file list that, whatever the course
//! -- but the browser recorded the address the title came from, and the
//! title of the course's own page above it ("2026FA - INTRO DATA SCIENCE
//! 01:198:439:09"). subject.rs makes that part of what the tab is.
//!
//! Firefox and its forks (Zen among them) keep history in `places.sqlite`,
//! locked while the browser runs, with the newest visits still in its
//! write-ahead log. So the two files are copied to the user's runtime
//! folder, read into memory, and the copy deleted at once: the history is
//! never kept on disk, and nothing of it but the site's name and that one
//! title is shown to Jev. Chromium-family browsers keep history in another
//! shape and get no place.

use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Where a tab is, as its browser's history tells it.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    /// The site's host, without "www.": "rutgers.instructure.com".
    pub site: String,
    /// The title of the nearest page above it on the site, when the browser
    /// has one that says something the tab's own title does not.
    pub part_of: Option<String>,
}

/// How often the history files are looked at for a change.
const STAT_EVERY: Duration = Duration::from_secs(1);
/// The least time between two reads of the history. A read is a ~17 MB
/// copy, and a browser writes its history on every page it opens.
const READ_EVERY: Duration = Duration::from_secs(2);

/// Where each browser family keeps its profiles, from the home folder.
const ZEN: &[&str] = &[".var/app/app.zen_browser.zen/.zen", ".zen"];
const FIREFOX: &[&str] = &[
    ".mozilla/firefox",
    ".var/app/org.mozilla.firefox/.mozilla/firefox",
    ".librewolf",
    ".var/app/io.gitlab.librewolf-community/.librewolf",
    ".floorp",
    ".var/app/one.ablaze.floorp/.floorp",
    ".waterfox",
];

/// The profile folders a window class's history can be in, or None for a
/// browser whose history this does not read.
fn roots(class: &str) -> Option<&'static [&'static str]> {
    let class = class.to_lowercase();
    let words: Vec<&str> = class.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    if words.contains(&"zen") {
        Some(ZEN)
    } else if ["firefox", "librewolf", "floorp", "waterfox"].iter().any(|b| words.contains(b)) {
        Some(FIREFOX)
    } else {
        None
    }
}

/// Every browser family's history as last read.
#[derive(Default)]
pub struct History {
    books: HashMap<&'static str, Book>,
}

/// One family's history: the profile in use, what its files were when
/// read, and what was read.
#[derive(Default)]
struct Book {
    checked: Option<Instant>,
    read_at: Option<Instant>,
    stamp: Option<Stamp>,
    index: Index,
}

/// What the history files were: (modified, length) of the database and its
/// write-ahead log.
type Stamp = (PathBuf, Option<(SystemTime, u64)>, Option<(SystemTime, u64)>);

impl History {
    /// Where the tab of `class` titled `title` (already cleaned of counts
    /// and the browser's name) is. Every lookup sees the history as of a
    /// second or two ago: the same title can be a new page -- "Files" in
    /// another course -- so a found answer is not kept past a change.
    pub fn find(&mut self, home: &Path, class: &str, title: &str) -> Option<Place> {
        let family = roots(class)?;
        let book = self.books.entry(family[0]).or_default();
        if book.checked.is_none_or(|at| at.elapsed() >= STAT_EVERY) {
            book.checked = Some(Instant::now());
            let stamp = newest_profile(home, family).map(|dir| stamp(&dir));
            let due = book.read_at.is_none_or(|at| at.elapsed() >= READ_EVERY);
            if stamp != book.stamp && due {
                book.read_at = Some(Instant::now());
                match stamp.as_ref().map(|s| snapshot(&s.0)) {
                    Some(Ok(index)) => {
                        book.index = index;
                        book.stamp = stamp;
                    }
                    // Tried again when the files next change, not every
                    // READ_EVERY, so a history that cannot be read is said once.
                    Some(Err(e)) => {
                        eprintln!("intentionality: browser history unread: {e}");
                        book.stamp = stamp;
                    }
                    None => {
                        book.index = Index::default();
                        book.stamp = None;
                    }
                }
            }
        }
        book.index.find(title)
    }
}

/// The profile under `roots` whose history was written last: the one the
/// browser is running, whatever profiles.ini calls the default.
fn newest_profile(home: &Path, roots: &[&str]) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for root in roots {
        let Ok(entries) = fs::read_dir(home.join(root)) else { continue };
        for entry in entries.flatten() {
            let dir = entry.path();
            let written = ["places.sqlite", "places.sqlite-wal"]
                .iter()
                .filter_map(|f| fs::metadata(dir.join(f)).and_then(|m| m.modified()).ok())
                .max();
            if let Some(at) = written {
                if best.as_ref().is_none_or(|(b, _)| at > *b) {
                    best = Some((at, dir));
                }
            }
        }
    }
    best.map(|(_, dir)| dir)
}

fn stamp(profile: &Path) -> Stamp {
    let of = |f: &str| fs::metadata(profile.join(f)).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
    (profile.to_path_buf(), of("places.sqlite"), of("places.sqlite-wal"))
}

/// The profile's history, read from a private copy that is gone again
/// before this returns.
fn snapshot(profile: &Path) -> Result<Index, String> {
    let base = std::env::var_os("XDG_RUNTIME_DIR").ok_or("no XDG_RUNTIME_DIR")?;
    let dir = PathBuf::from(base).join("intentionality");
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| e.to_string())?;
    // Per process: a second copy of the app must not read this one's copy.
    let db = dir.join(format!("places-{}.sqlite", std::process::id()));
    let side = |suffix: &str| PathBuf::from(format!("{}{suffix}", db.display()));
    let (wal, shm) = (side("-wal"), side("-shm"));
    let clear = || {
        for f in [&db, &wal, &shm] {
            let _ = fs::remove_file(f);
        }
    };
    clear();
    let read = (|| {
        fs::copy(profile.join("places.sqlite"), &db).map_err(|e| e.to_string())?;
        let log = profile.join("places.sqlite-wal");
        if log.exists() {
            fs::copy(&log, &wal).map_err(|e| e.to_string())?;
        }
        // Read-write: opening a database with a write-ahead log replays it,
        // and only this copy is ever written.
        let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|e| e.to_string())?;
        Index::read(&conn).map_err(|e| e.to_string())
    })();
    clear();
    read
}

/// A history held in memory: every titled web page the browser kept.
#[derive(Default)]
pub struct Index {
    /// A title as compared -> the page last visited under it: (url, when).
    by_title: HashMap<String, (String, i64)>,
    /// A page's address with no query, fragment or trailing slash -> its
    /// title as written. What a tab's parent pages are looked up in.
    titles: HashMap<String, String>,
}

impl Index {
    /// `moz_places`: one row per address, with its last title and visit.
    pub fn read(conn: &Connection) -> rusqlite::Result<Index> {
        let mut stmt = conn.prepare(
            "SELECT url, title, COALESCE(last_visit_date, 0) FROM moz_places
             WHERE title IS NOT NULL AND title != ''
               AND (url LIKE 'https://%' OR url LIKE 'http://%')",
        )?;
        let mut index = Index::default();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
        for row in rows {
            let (url, title, visited) = row?;
            index.add(&url, &title, visited);
        }
        Ok(index)
    }

    fn add(&mut self, url: &str, title: &str, visited: i64) {
        let compared = compared(title);
        if compared.is_empty() {
            return;
        }
        let newer = self.by_title.get(&compared).is_none_or(|(_, at)| visited > *at);
        if newer {
            self.by_title.insert(compared, (url.to_string(), visited));
        }
        if !url.contains(['?', '#']) {
            self.titles.insert(url.trim_end_matches('/').to_string(), title.to_string());
        }
    }

    /// The page last visited under this title: its site, and the nearest
    /// page above it with a title of its own. The site's front page is not
    /// one -- it is a dashboard or the site's name, which the site says --
    /// and nor is a one-word title above ("Courses", "Jobs", "Recipes"): a
    /// listing of the site's sections says no more than the site.
    pub fn find(&self, title: &str) -> Option<Place> {
        let own = compared(title);
        let (url, _) = self.by_title.get(&own)?;
        let (site, _) = split(url)?;
        let part_of = ancestors(url)
            .iter()
            .filter_map(|a| self.titles.get(a))
            .map(|t| crate::subject::clean("", t))
            .find(|t| t.split_whitespace().count() > 1 && t.to_lowercase() != own);
        let site = site.strip_prefix("www.").unwrap_or(site).to_string();
        Some(Place { site, part_of })
    }
}

/// A title as compared: the counts and marks in front, case and runs of
/// whitespace do not matter -- as subject.rs cleans a window's title.
fn compared(title: &str) -> String {
    crate::subject::clean("", title).to_lowercase()
}

/// An http(s) address's host (lowercased, no user or port) and path (no
/// query or fragment).
fn split(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let rest = &rest[..rest.find(['?', '#']).unwrap_or(rest.len())];
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    (!host.is_empty()).then_some((host, path))
}

/// The pages above an address on its site, nearest first, as `titles`
/// keys them: "https://h/courses/405053/files" -> "https://h/courses/405053",
/// "https://h/courses". Not the site's front page, and not the page itself.
fn ancestors(url: &str) -> Vec<String> {
    let Some((_, path)) = split(url) else { return Vec::new() };
    let scheme = if url.starts_with("https://") { "https" } else { "http" };
    // The host as written, so the key matches the stored address.
    let rest = &url[scheme.len() + 3..];
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    (1..parts.len())
        .rev()
        .map(|n| format!("{scheme}://{authority}/{}", parts[..n].join("/")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const COURSE: &str = "https://rutgers.instructure.com/courses/405053";
    const OTHER: &str = "https://rutgers.instructure.com/courses/408293";

    fn index(rows: &[(&str, &str, i64)]) -> Index {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE moz_places (url TEXT, title TEXT, last_visit_date INTEGER)").unwrap();
        for (url, title, at) in rows {
            conn.execute("INSERT INTO moz_places VALUES (?1, NULLIF(?2, ''), ?3)", rusqlite::params![url, title, at])
                .unwrap();
        }
        Index::read(&conn).unwrap()
    }

    #[test]
    fn the_pages_above_an_address_are_nearest_first_without_the_front_page() {
        assert_eq!(
            ancestors("https://rutgers.instructure.com/courses/405053/files?x=1#top"),
            vec![COURSE.to_string(), "https://rutgers.instructure.com/courses".into()]
        );
        assert_eq!(ancestors("https://www.youtube.com/watch?v=abc"), Vec::<String>::new());
        assert_eq!(ancestors("https://github.com/"), Vec::<String>::new());
        assert_eq!(ancestors("about:newtab"), Vec::<String>::new());
        assert_eq!(split("https://me:pw@Example.org:8443/a/b?c"), Some(("Example.org", "/a/b")));
        assert_eq!(split("moz-extension://abc/page.html"), None);
    }

    /// The flagged case: "Files" means the course it was last opened in.
    #[test]
    fn a_bare_title_is_the_page_last_visited_under_it_and_the_page_above_that() {
        let ix = index(&[
            (COURSE, "2026FA - INTRO DATA SCIENCE 01:198:439:09", 10),
            (&format!("{COURSE}/files"), "Files", 20),
            (OTHER, "2026FA - APOCALYPSE NOW 01:840:305:90", 5),
            (&format!("{OTHER}/files"), "Files", 8),
            ("https://rutgers.instructure.com/", "Dashboard", 30),
            ("https://rutgers.instructure.com/courses/405053/quizzes/1055113", "Quiz 3: 2026FA - INTRO DATA SCIENCE 01:198:439:09", 3),
            ("https://rutgers.instructure.com/courses/405053/quizzes/1055113/take/questions/1", "Quiz: Quiz 3", 4),
            ("https://www.linkedin.com/feed/", "(3) Feed | LinkedIn", 9),
            ("https://www.linkedin.com/", "Feed | LinkedIn", 7),
            ("https://example.org/untitled", "", 50),
        ]);
        let course = Some("2026FA - INTRO DATA SCIENCE 01:198:439:09".to_string());
        let files = ix.find("Files").unwrap();
        assert_eq!(files, Place { site: "rutgers.instructure.com".into(), part_of: course });
        // A page whose parent folder has no title is under the next one up.
        assert_eq!(
            ix.find("quiz: quiz 3").unwrap().part_of.as_deref(),
            Some("Quiz 3: 2026FA - INTRO DATA SCIENCE 01:198:439:09")
        );
        // The front page is never the page above, nor a one-word listing;
        // counts never matter.
        assert_eq!(ix.find("Dashboard").unwrap().part_of, None);
        let listed = index(&[
            ("https://rutgers.instructure.com/courses", "Courses", 1),
            (COURSE, "2026FA - INTRO DATA SCIENCE 01:198:439:09", 2),
        ]);
        assert_eq!(listed.find("2026FA - INTRO DATA SCIENCE 01:198:439:09").unwrap().part_of, None);
        assert_eq!(ix.find("Feed | LinkedIn"), Some(Place { site: "linkedin.com".into(), part_of: None }));
        assert_eq!(ix.find("Nowhere"), None);

        // Opened later in the other course, "Files" is that course's.
        let later = index(&[
            (COURSE, "2026FA - INTRO DATA SCIENCE 01:198:439:09", 10),
            (&format!("{COURSE}/files"), "Files", 20),
            (OTHER, "2026FA - APOCALYPSE NOW 01:840:305:90", 25),
            (&format!("{OTHER}/files"), "Files", 30),
        ]);
        assert_eq!(later.find("Files").unwrap().part_of.as_deref(), Some("2026FA - APOCALYPSE NOW 01:840:305:90"));
    }

    #[test]
    fn only_firefox_family_histories_are_read() {
        assert_eq!(roots("app.zen_browser.zen"), Some(ZEN));
        assert_eq!(roots("org.mozilla.firefox"), Some(FIREFOX));
        assert_eq!(roots("firefox"), Some(FIREFOX));
        assert_eq!(roots("google-chrome"), None);
        assert_eq!(roots("org.gnome.Citizen"), None);
        // No profile at all: no place, and nothing copied.
        let mut h = History::default();
        assert_eq!(h.find(Path::new("/nonexistent-home"), "app.zen_browser.zen", "Files"), None);
    }
}
