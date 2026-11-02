//! Where a terminal or a code editor is working: its folder on disk and the
//! project that folder belongs to. A window title only hints at it -- fish
//! shortens the path to "~/D/p/i/app", Claude Code replaces it with the
//! session's topic, VS Code names the folder but not where it is -- so the
//! rest is read from the machine: the filesystem, Claude Code's own records
//! of its sessions, and VS Code's list of workspaces. Nothing here leaves the
//! machine; subject.rs decides what Jev is shown of it.
//!
//! Every answer is remembered, because `page_key` runs for every span of the
//! day: a found folder for the life of the process, a miss for a few seconds
//! (a Claude Code session's title can reach the tab a moment before it
//! reaches the session's transcript).

use crate::places::{History, Place};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::hash::Hash;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// The repository or folder a working directory belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub root: PathBuf,
    /// The root's own name: "intentionality".
    pub name: String,
    /// The README's opening, for a project whose name says little ("kiki").
    pub about: Option<String>,
}

/// What the machine can say about a window beyond its title.
pub trait Lookup: Sync {
    fn home(&self) -> &Path;
    /// A path as fish shows it, "~/D/p/i/app", made whole again.
    fn expand(&self, shown: &str) -> Option<PathBuf>;
    /// The folder of the Claude Code session titled `topic`. None asks for
    /// a session that has no title yet ("Claude Code").
    fn claude_cwd(&self, topic: Option<&str>) -> Option<PathBuf>;
    /// Where the folder a code editor names in its title is.
    fn editor_folder(&self, folder: &str) -> Option<PathBuf>;
    fn project(&self, dir: &Path) -> Option<Project>;
    /// Where a browser's tab is, from the browser's history: `title` is the
    /// tab's, cleaned of counts and the browser's name.
    fn place(&self, app: &str, title: &str) -> Option<Place>;
    /// What the installed app whose windows have this class (lowercased) is
    /// called: "md.obsidian.obsidian" is Obsidian (desktop.rs).
    fn app_name(&self, class: &str) -> Option<String>;
}

/// "~/Desktop/projects/intentionality": a path as Jev and the store see it,
/// without the user's name in it.
pub fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The lookup the app uses. Under test, one that finds nothing, so no test
/// depends on the folders of the machine it runs on.
pub fn machine() -> &'static dyn Lookup {
    #[cfg(test)]
    {
        static NOWHERE: OnceLock<Machine> = OnceLock::new();
        NOWHERE.get_or_init(|| Machine::at(PathBuf::from("/nonexistent-home")))
    }
    #[cfg(not(test))]
    {
        static MACHINE: OnceLock<Machine> = OnceLock::new();
        MACHINE.get_or_init(|| Machine::at(PathBuf::from(std::env::var("HOME").unwrap_or_default())))
    }
}

/// How long a miss is believed before it is looked for again.
const MISS_TTL: Duration = Duration::from_secs(10);
/// A memo map past this many entries starts over rather than growing.
const MEMO_MAX: usize = 4096;
/// Transcripts this recent can still be behind a title in today's counts.
const RECENT: Duration = Duration::from_secs(24 * 3600);
/// How many folders `expand` may try before calling a path ambiguous.
const EXPAND_BUDGET: usize = 64;

/// The real machine, rooted at a home directory.
pub struct Machine {
    home: PathBuf,
    proc: PathBuf,
    expand: Memo<String, Option<PathBuf>>,
    claude: Memo<Option<String>, Option<PathBuf>>,
    editor: Memo<String, Option<PathBuf>>,
    project: Memo<PathBuf, Option<Project>>,
    scans: Mutex<HashMap<PathBuf, Scan>>,
    places: Mutex<History>,
}

impl Machine {
    pub fn at(home: PathBuf) -> Self {
        Machine {
            home,
            proc: PathBuf::from("/proc"),
            expand: Memo::default(),
            claude: Memo::default(),
            editor: Memo::default(),
            project: Memo::default(),
            scans: Mutex::default(),
            places: Mutex::default(),
        }
    }
}

impl Lookup for Machine {
    fn home(&self) -> &Path {
        &self.home
    }

    fn expand(&self, shown: &str) -> Option<PathBuf> {
        self.expand.get(shown.to_string(), Option::is_some, || expand_in(&self.home, shown))
    }

    fn claude_cwd(&self, topic: Option<&str>) -> Option<PathBuf> {
        let key = topic.map(norm);
        // Which sessions have no title yet changes as they start and get
        // named, so that answer is never kept for long.
        let keep = |found: &Option<PathBuf>| key.is_some() && found.is_some();
        self.claude.get(key.clone(), keep, || match &key {
            Some(topic) => self.named_cwd(topic),
            None => self.unnamed_cwd(),
        })
    }

    fn editor_folder(&self, folder: &str) -> Option<PathBuf> {
        self.editor.get(folder.to_string(), Option::is_some, || editor_folder_in(&self.home, folder))
    }

    fn project(&self, dir: &Path) -> Option<Project> {
        self.project.get(dir.to_path_buf(), Option::is_some, || project_in(&self.home, dir))
    }

    fn place(&self, app: &str, title: &str) -> Option<Place> {
        self.places.lock().unwrap().find(&self.home, app, title)
    }

    fn app_name(&self, class: &str) -> Option<String> {
        crate::desktop::machine().of_class(class).map(|a| a.name)
    }
}

/// Answers remembered by key: those `keep` says are settled for good, the
/// rest for MISS_TTL.
struct Memo<K, V>(Mutex<HashMap<K, (V, Option<Instant>)>>);

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Memo(Mutex::new(HashMap::new()))
    }
}

impl<K: Eq + Hash, V: Clone> Memo<K, V> {
    fn get(&self, key: K, keep: impl Fn(&V) -> bool, find: impl FnOnce() -> V) -> V {
        if let Some((value, expires)) = self.0.lock().unwrap().get(&key) {
            if expires.is_none_or(|at| Instant::now() < at) {
                return value.clone();
            }
        }
        // Looked up without the lock: it reads files.
        let value = find();
        let expires = (!keep(&value)).then(|| Instant::now() + MISS_TTL);
        let mut map = self.0.lock().unwrap();
        if map.len() >= MEMO_MAX {
            map.clear();
        }
        map.insert(key, (value.clone(), expires));
        value
    }
}

/// A title as compared: case and runs of whitespace do not matter.
fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

// ---- fish's shortened paths ------------------------------------------------

/// fish's prompt_pwd keeps a path's last folder whole and every folder
/// before it to its first letter (two for a dot-folder): "~/D/p/i/app",
/// "~/.c/intentionality". Each short folder is matched as a prefix, the last
/// exactly. One match is the path; none, or more than one, is None.
fn expand_in(home: &Path, shown: &str) -> Option<PathBuf> {
    let shown = shown.trim();
    let (base, rest) = if shown == "~" {
        return Some(home.to_path_buf());
    } else if let Some(rest) = shown.strip_prefix("~/") {
        (home.to_path_buf(), rest)
    } else if let Some(rest) = shown.strip_prefix('/') {
        (PathBuf::from("/"), rest)
    } else {
        return None;
    };
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return base.is_dir().then_some(base);
    }
    let mut found = Vec::new();
    let mut budget = EXPAND_BUDGET;
    walk(&base, &parts, &mut found, &mut budget);
    if found.len() == 1 && budget > 0 {
        found.pop()
    } else {
        None
    }
}

fn walk(dir: &Path, parts: &[&str], found: &mut Vec<PathBuf>, budget: &mut usize) {
    let Some((part, rest)) = parts.split_first() else { return };
    if rest.is_empty() {
        let path = dir.join(part);
        if path.is_dir() {
            found.push(path);
        }
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if found.len() > 1 || *budget == 0 {
            return;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let path = entry.path();
        if !name.starts_with(part) || !path.is_dir() {
            continue;
        }
        *budget -= 1;
        walk(&path, rest, found, budget);
    }
}

// ---- projects --------------------------------------------------------------

/// The project a folder is in: the nearest folder above it (itself
/// included, home and / not) that is a git repository, else the nearest
/// that has a README.
fn project_in(home: &Path, dir: &Path) -> Option<Project> {
    let chain: Vec<&Path> = dir
        .ancestors()
        .take_while(|d| *d != home && d.parent().is_some())
        .collect();
    let root = chain
        .iter()
        .find(|d| d.join(".git").exists())
        .or_else(|| chain.iter().find(|d| readme(d).is_some()))?;
    let name = root.file_name()?.to_string_lossy().into_owned();
    let about = readme(root).and_then(|p| fs::read_to_string(p).ok()).and_then(|t| opening(&t));
    Some(Project { root: root.to_path_buf(), name, about })
}

/// A folder's README, README.md first.
fn readme(dir: &Path) -> Option<PathBuf> {
    let mut names: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().to_lowercase().starts_with("readme"))
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    names.sort_by_key(|p| (p.file_name().map_or(true, |n| n != "README.md"), p.clone()));
    names.into_iter().next()
}

/// Longest opening a README gives Jev.
const OPENING_CHARS: usize = 240;
/// A first sentence shorter than this ("A personal reading room.") takes
/// the next one with it.
const OPENING_MIN: usize = 60;

/// What a README says the project is: its first paragraph of prose -- not a
/// heading, a badge, a code block or a lone link -- as plain text, cut to
/// its first sentence or two.
fn opening(readme: &str) -> Option<String> {
    // Under four words is a name or a lone link, not a description.
    let prose = |para: &[String]| para.iter().map(|l| l.split_whitespace().count()).sum::<usize>() >= 4;
    let mut para: Vec<String> = Vec::new();
    let mut fenced = false;
    // The blank line chained on closes a paragraph the file ends in.
    for line in readme.lines().chain(std::iter::once("")) {
        let l = line.trim();
        let fence = l.starts_with("```") || l.starts_with("~~~");
        if fence {
            fenced = !fenced;
        }
        let skip = fence
            || fenced
            || l.starts_with('#')
            || l.starts_with("---")
            || l.starts_with("===")
            || l.starts_with('|')
            || l.starts_with("![")
            || l.starts_with("[![")
            || is_html_heading(l);
        let text = if skip { String::new() } else { plain(l) };
        if !text.is_empty() {
            para.push(text);
            continue;
        }
        // A paragraph ends at a blank line, a heading, a fence, or an HTML
        // line with no words of its own (kiki's `</p>`).
        if prose(&para) {
            break;
        }
        para.clear();
    }
    prose(&para).then(|| sentences(&para.join(" ")))
}

fn is_html_heading(l: &str) -> bool {
    let l = l.to_lowercase();
    (1..=6).any(|n| l.starts_with(&format!("<h{n}")))
}

/// A line of markdown or HTML as the words it shows: tags, images, link
/// targets, emphasis marks and a quote's `>` taken off.
fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line.trim_start_matches('>').trim();
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            // A tag goes; the text between tags stays.
            match rest.find('>') {
                Some(end) => rest = &rest[end + 1..],
                None => rest = "",
            }
            continue;
        }
        if rest.starts_with("![") {
            // An image goes whole.
            rest = link(&rest[1..]).map_or("", |(_, after)| after);
            continue;
        }
        if c == '[' {
            if let Some((text, after)) = link(rest) {
                out.push_str(text);
                rest = after;
                continue;
            }
        }
        if matches!(c, '*' | '`') {
            rest = &rest[1..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    // Emphasis underscores at a word's edges ("_Guided … reason_"), not
    // inside one (cs439_m2_team).
    out.split_whitespace()
        .map(|w| w.trim_matches('_'))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A `[text](target)` at the start of `s`: its text, and what follows it.
fn link(s: &str) -> Option<(&str, &str)> {
    let close = s.find("](")?;
    let end = s[close..].find(')')? + close;
    Some((&s[1..close], &s[end + 1..]))
}

/// The first sentence, and the next while the first is short; never past
/// OPENING_CHARS, which cuts at a word and says so.
fn sentences(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() && out.chars().count() < OPENING_MIN {
        let end = sentence_end(rest).unwrap_or(rest.len());
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(rest[..end].trim());
        rest = rest[end..].trim_start();
    }
    if out.chars().count() <= OPENING_CHARS {
        return out;
    }
    let cut: String = out.chars().take(OPENING_CHARS).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ';', ':', ' ']))
}

/// Just past a ". " that a capital follows.
fn sentence_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    (0..bytes.len().saturating_sub(2))
        .find(|&i| {
            // Both bytes are ASCII, so i + 2 is a character boundary.
            matches!(bytes[i], b'.' | b'!' | b'?')
                && bytes[i + 1] == b' '
                && text[i + 2..].chars().next().is_some_and(char::is_uppercase)
        })
        .map(|i| i + 1)
}

// ---- code editors ----------------------------------------------------------

/// Where VS Code and its kin keep one folder per workspace they opened.
const WORKSPACE_STORES: &[&str] = &[
    ".config/Code/User/workspaceStorage",
    ".config/Code - OSS/User/workspaceStorage",
    ".config/VSCodium/User/workspaceStorage",
    ".config/Cursor/User/workspaceStorage",
    ".var/app/com.visualstudio.code/config/Code/User/workspaceStorage",
    ".var/app/com.vscodium.codium/config/VSCodium/User/workspaceStorage",
];

/// The opened folder named `folder`, from each workspace's workspace.json
/// (`{"folder": "file:///home/…/cereal"}`); the most recently used when two
/// share a name.
fn editor_folder_in(home: &Path, folder: &str) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for store in WORKSPACE_STORES {
        let Ok(entries) = fs::read_dir(home.join(store)) else { continue };
        for entry in entries.flatten() {
            let Ok(text) = fs::read_to_string(entry.path().join("workspace.json")) else { continue };
            let Some(path) = workspace_folder(&text) else { continue };
            if path.file_name().is_none_or(|n| n != folder) || !path.is_dir() {
                continue;
            }
            let used = entry.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            if best.as_ref().is_none_or(|(t, _)| used > *t) {
                best = Some((used, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

/// A workspace.json's folder as a path; a multi-root workspace has none.
fn workspace_folder(json: &str) -> Option<PathBuf> {
    let v: Value = serde_json::from_str(json).ok()?;
    let uri = v["folder"].as_str()?.strip_prefix("file://")?;
    Some(PathBuf::from(percent_decode(uri)))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---- Claude Code sessions --------------------------------------------------

/// One running Claude Code session, from `~/.claude/sessions/<pid>.json`.
#[derive(Debug, Clone, PartialEq)]
struct Live {
    session_id: String,
    cwd: PathBuf,
    /// What it is called now and was called before.
    names: Vec<String>,
}

/// What a transcript says so far: every title the session has had (a tab
/// seen this morning may carry one it has since replaced), the folder it
/// started in -- later lines follow the shell's `cd`s -- and how far it has
/// been read. Transcripts only grow, so a changed one is read on from there.
#[derive(Debug, Clone, Default, PartialEq)]
struct Scan {
    titles: Vec<String>,
    cwd: Option<PathBuf>,
    read_to: u64,
}

impl Machine {
    /// The folder of the session titled `topic`: a running one first, then
    /// any whose transcript changed in the last day.
    fn named_cwd(&self, topic: &str) -> Option<PathBuf> {
        let titled = |scan: &Scan| scan.titles.iter().any(|t| norm(t) == topic);
        for live in self.live_sessions() {
            let scanned = || self.transcript(&live.session_id).map(|p| self.scan(&p));
            if live.names.iter().any(|n| norm(n) == topic) || scanned().is_some_and(|s| titled(&s)) {
                return Some(live.cwd);
            }
        }
        let mut recent = self.recent_transcripts();
        recent.sort_by(|a, b| b.0.cmp(&a.0));
        recent.into_iter().find_map(|(_, path)| {
            let scan = self.scan(&path);
            if titled(&scan) {
                scan.cwd
            } else {
                None
            }
        })
    }

    /// The folder of the running sessions that have no title yet, if they
    /// are all in one.
    fn unnamed_cwd(&self) -> Option<PathBuf> {
        let mut cwds = self.live_sessions().into_iter().filter_map(|live| {
            let scan = self.transcript(&live.session_id).map(|p| self.scan(&p));
            scan.is_none_or(|s| s.titles.is_empty()).then_some(live.cwd)
        });
        let first = cwds.next()?;
        cwds.all(|c| c == first).then_some(first)
    }

    /// The running sessions. The registry's `.key` files are never opened;
    /// a record whose process has gone, or whose pid now belongs to another
    /// process, is passed over.
    fn live_sessions(&self) -> Vec<Live> {
        let Ok(entries) = fs::read_dir(self.home.join(".claude/sessions")) else { return Vec::new() };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Some((pid, started, live)) = registry_entry(&text) else { continue };
            if self.alive(pid, started.as_deref()) {
                out.push(live);
            }
        }
        out
    }

    /// The pid is running, and is the process the record was written by:
    /// its start time (/proc/<pid>/stat field 22) matches `procStart`.
    fn alive(&self, pid: u32, started: Option<&str>) -> bool {
        let Ok(stat) = fs::read_to_string(self.proc.join(pid.to_string()).join("stat")) else {
            return false;
        };
        let Some(started) = started else { return true };
        // Fields after the command name, which may hold spaces, start at 3.
        let after = stat.rsplit_once(')').map_or("", |(_, rest)| rest);
        after.split_whitespace().nth(22 - 3) == Some(started)
    }

    /// `~/.claude/projects/<folder>/<session id>.jsonl`.
    fn transcript(&self, session_id: &str) -> Option<PathBuf> {
        let file = format!("{session_id}.jsonl");
        fs::read_dir(self.home.join(".claude/projects"))
            .ok()?
            .flatten()
            .map(|e| e.path().join(&file))
            .find(|p| p.is_file())
    }

    /// Transcripts changed in the last day, with when.
    fn recent_transcripts(&self) -> Vec<(SystemTime, PathBuf)> {
        let since = SystemTime::now() - RECENT;
        let Ok(dirs) = fs::read_dir(self.home.join(".claude/projects")) else { return Vec::new() };
        let mut out = Vec::new();
        for dir in dirs.flatten() {
            let Ok(files) = fs::read_dir(dir.path()) else { continue };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                let Ok(modified) = file.metadata().and_then(|m| m.modified()) else { continue };
                if modified >= since {
                    out.push((modified, path));
                }
            }
        }
        out
    }

    /// A transcript as far as it goes now: the first look reads it whole,
    /// later ones only what was appended since.
    fn scan(&self, path: &Path) -> Scan {
        let len = fs::metadata(path).map_or(0, |m| m.len());
        let mut scan = self.scans.lock().unwrap().get(path).cloned().unwrap_or_default();
        if len < scan.read_to {
            scan = Scan::default(); // rewritten, not appended to
        }
        if len > scan.read_to {
            read_on(path, &mut scan);
            let mut scans = self.scans.lock().unwrap();
            if scans.len() >= MEMO_MAX {
                scans.clear();
            }
            scans.insert(path.to_path_buf(), scan.clone());
        }
        scan
    }
}

/// A registry record: its pid, its process's start time, and the session.
fn registry_entry(json: &str) -> Option<(u32, Option<String>, Live)> {
    let v: Value = serde_json::from_str(json).ok()?;
    let pid = u32::try_from(v["pid"].as_u64()?).ok()?;
    let started = match &v["procStart"] {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    let mut names: Vec<String> = v["name"].as_str().map(String::from).into_iter().collect();
    if let Some(former) = v["formerNames"].as_array() {
        names.extend(former.iter().filter_map(|f| f["name"].as_str().map(String::from)));
    }
    let live = Live {
        session_id: v["sessionId"].as_str()?.to_string(),
        cwd: PathBuf::from(v["cwd"].as_str()?),
        names,
    };
    Some((pid, started, live))
}

/// Read a transcript on from `scan.read_to` through its last whole line,
/// noting what it says.
fn read_on(path: &Path, scan: &mut Scan) {
    let Ok(mut file) = fs::File::open(path) else { return };
    if file.seek(SeekFrom::Start(scan.read_to)).is_err() {
        return;
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return;
    }
    let Some(end) = bytes.iter().rposition(|&b| b == b'\n') else { return };
    note(&String::from_utf8_lossy(&bytes[..=end]), scan);
    scan.read_to += end as u64 + 1;
}

/// The titles and the first folder in a run of transcript lines, found by
/// their keys rather than by parsing every line: a transcript is mostly
/// megabytes of tool output. A key quoted inside that output is escaped
/// (`\"cwd\":`), so only a line's own keys match.
fn note(text: &str, scan: &mut Scan) {
    for title in string_values(text, "\"aiTitle\":") {
        if !title.trim().is_empty() && !scan.titles.contains(&title) {
            scan.titles.push(title);
        }
    }
    if scan.cwd.is_none() {
        scan.cwd = string_values(text, "\"cwd\":").next().map(PathBuf::from);
    }
}

/// Every JSON string that follows `key` in `text`, unescaped.
fn string_values<'a>(text: &'a str, key: &'a str) -> impl Iterator<Item = String> + 'a {
    text.match_indices(key).filter_map(move |(at, _)| {
        let rest = text[at + key.len()..].trim_start();
        let body = rest.strip_prefix('"')?;
        let mut escaped = false;
        let (close, _) = body.char_indices().find(|&(_, c)| {
            let closes = c == '"' && !escaped;
            escaped = c == '\\' && !escaped;
            closes
        })?;
        serde_json::from_str(&rest[..close + 2]).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    /// A directory of our own per test, as in resume_units.rs: no tempfile crate.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
            let dir = std::env::temp_dir().join(format!("folder-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Tmp(dir)
        }

        fn dir(&self, rel: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(&p).unwrap();
            p
        }

        fn file(&self, rel: &str, text: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, text).unwrap();
            p
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_fish_path_is_made_whole_when_only_one_folder_fits() {
        let t = Tmp::new("expand");
        let app = t.dir("Desktop/projects/intentionality/app");
        t.dir("Documents/papers");
        t.dir("Downloads");
        t.dir(".config/intentionality");
        t.dir(".cache/intentionality");
        let home = &t.0;
        assert_eq!(expand_in(home, "~/D/p/i/app"), Some(app));
        assert_eq!(expand_in(home, "~/D/p/intentionality"), Some(home.join("Desktop/projects/intentionality")));
        assert_eq!(expand_in(home, "~/D/projects"), Some(home.join("Desktop/projects")));
        // A dot-folder keeps two characters, and .c is not .cache.
        assert_eq!(expand_in(home, "~/.co/intentionality"), Some(home.join(".config/intentionality")));
        assert_eq!(expand_in(home, "~/.c/intentionality"), None); // .config or .cache: ambiguous
        assert_eq!(expand_in(home, "~"), Some(home.clone()));
        assert_eq!(expand_in(home, "~/Downloads/"), Some(home.join("Downloads")));
        assert_eq!(expand_in(home, "~/D/p/nothing"), None);
        assert_eq!(expand_in(home, "not a path"), None);
        // Two folders under ~/D both hold a "p…/x": no guess.
        t.dir("Desktop/pa/x");
        t.dir("Documents/pb/x");
        assert_eq!(expand_in(home, "~/D/p/x"), None);
    }

    #[test]
    fn a_project_is_the_nearest_repository_above_else_the_nearest_readme() {
        let t = Tmp::new("project");
        let home = &t.0;
        t.dir("Desktop/projects/intentionality/.git");
        t.file("Desktop/projects/intentionality/README.md", "# intentionality\n\nA gate that sits between login and your desktop.\n");
        t.file("Desktop/projects/intentionality/app/README.md", "# app\n\nThe Tauri app inside the gate's repository.\n");
        let app = t.dir("Desktop/projects/intentionality/app/src");
        let p = project_in(home, &app).unwrap();
        assert_eq!(p.name, "intentionality"); // the repository beats app's own README
        assert_eq!(p.about.as_deref(), Some("A gate that sits between login and your desktop."));
        // No repository: the README marks it.
        t.file("Desktop/projects/Reader/README.md", "# Reader\n\nA personal reading room. Your books live as files.\n");
        let reader = t.dir("Desktop/projects/Reader/server");
        assert_eq!(project_in(home, &reader).unwrap().name, "Reader");
        // Neither, or home itself: no project.
        assert_eq!(project_in(home, &t.dir("Desktop/projects")), None);
        assert_eq!(project_in(home, home), None);
    }

    /// Real openings from the projects on this machine.
    #[test]
    fn a_readme_opening_is_its_first_prose() {
        let kiki = "<p align=\"center\">\n  <img src=\"docs/mascot.svg\" width=\"96\" alt=\"Kiki\">\n</p>\n\n\
<h1 align=\"center\">Kiki</h1>\n\n<p align=\"center\">\n  Highlight text in any app, press <b>Ctrl+Alt+K</b>, and ask about it.\n</p>\n";
        assert_eq!(opening(kiki).as_deref(), Some("Highlight text in any app, press Ctrl+Alt+K, and ask about it."));
        let pie = "# InterviewPie\n\n> _Guided interview practice rooted in questions, speech, and reason_\n\n[InterviewPie](https://interviewpie.com)\n";
        assert_eq!(opening(pie).as_deref(), Some("Guided interview practice rooted in questions, speech, and reason"));
        let cereal = "# Cereal\n\nAn AI meeting notepad for Linux. You jot down the few things that matter during a meeting.\n";
        // Short first sentence: the next comes with it.
        assert_eq!(
            opening(cereal).as_deref(),
            Some("An AI meeting notepad for Linux. You jot down the few things that matter during a meeting.")
        );
        let fervor = "# Fervor\n\nGuitar isolation research pipeline (`python -m fervor --help`), and a\nlow-latency chord recogniser.\n\n## Chords\n";
        assert_eq!(
            opening(fervor).as_deref(),
            Some("Guitar isolation research pipeline (python -m fervor --help), and a low-latency chord recogniser.")
        );
        let linked = "# x\n\nA web app for [fervor](../fervor)'s **ChordNet**, a chord recogniser for guitar.\n";
        assert_eq!(opening(linked).as_deref(), Some("A web app for fervor's ChordNet, a chord recogniser for guitar."));
        let long = format!("# x\n\n{}\n", "word ".repeat(80));
        let cut = opening(&long).unwrap();
        assert!(cut.ends_with('…') && cut.chars().count() <= OPENING_CHARS + 1, "{cut}");
        assert_eq!(opening("# Title only\n\n```sh\nnpm install\n```\n"), None);
        assert_eq!(opening(""), None);
        // An underscore inside a word is the word's.
        assert_eq!(plain("_the cs439_m2_team repo_"), "the cs439_m2_team repo");
    }

    #[test]
    fn a_workspace_names_its_folder() {
        let t = Tmp::new("editor");
        let cereal = t.dir("Desktop/projects/cereal");
        t.file(
            ".var/app/com.visualstudio.code/config/Code/User/workspaceStorage/aa/workspace.json",
            &format!("{{\"folder\": \"file://{}\"}}", cereal.display()),
        );
        t.file(
            ".config/Code/User/workspaceStorage/bb/workspace.json",
            "{\"workspace\": \"file:///x/agent-sessions.code-workspace\"}",
        );
        assert_eq!(editor_folder_in(&t.0, "cereal"), Some(cereal));
        assert_eq!(editor_folder_in(&t.0, "kiki"), None);
        assert_eq!(
            workspace_folder("{\"folder\": \"file:///home/d/My%20Project\"}"),
            Some(PathBuf::from("/home/d/My Project"))
        );
    }

    #[test]
    fn a_transcript_gives_every_title_and_the_folder_it_started_in() {
        let t = Tmp::new("scan");
        let lines = [
            r#"{"type":"user","cwd":"/home/d","message":"how do I free memory?"}"#,
            r#"{"type":"ai-title","aiTitle":"Allocating more memory on Linux","sessionId":"s"}"#,
            // Later lines follow a `cd`; the session still started in ~.
            r#"{"type":"user","cwd":"/home/d/projects/fervor","message":"delete fervor's data"}"#,
            // A title quoted in tool output is not the session's.
            r#"{"type":"user","toolUseResult":"{\"aiTitle\":\"quoted\",\"cwd\":\"/x\"}"}"#,
            r#"{"type":"ai-title","aiTitle":"Clean up \"projects\"","sessionId":"s"}"#,
            r#"{"type":"ai-title","aiTitle":"Allocating more memory on Linux","sessionId":"s"}"#,
        ];
        let path = t.file("s.jsonl", &format!("{}\n", lines[..3].join("\n")));
        let m = Machine::at(t.0.clone());
        let first = m.scan(&path);
        assert_eq!(first.titles, ["Allocating more memory on Linux"]);
        assert_eq!(first.cwd, Some(PathBuf::from("/home/d")));
        // Appended to, half a line at the end: read on, the half left for later.
        fs::write(&path, format!("{}\n{}", lines.join("\n"), r#"{"type":"ai-ti"#)).unwrap();
        let more = m.scan(&path);
        assert_eq!(more.titles, ["Allocating more memory on Linux", "Clean up \"projects\""]);
        assert_eq!(more.cwd, Some(PathBuf::from("/home/d")));
        assert!(more.read_to < fs::metadata(&path).unwrap().len());
        assert_eq!(Machine::at(t.0.clone()).scan(&t.0.join("missing.jsonl")), Scan::default());
    }

    /// A fake /proc and ~/.claude: two live sessions and one whose pid now
    /// belongs to another process.
    fn claude_home(t: &Tmp) -> Machine {
        let intent = t.dir("Desktop/projects/intentionality");
        let projects = t.dir("Desktop/projects");
        t.file("proc/100/stat", "100 (claude) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 1000 20\n");
        t.file("proc/200/stat", "200 (claude code) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 2000 20\n");
        t.file("proc/300/stat", "300 (bash) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 9999 20\n");
        let entry = |pid: u32, start: u32, id: &str, cwd: &Path, name: &str| {
            format!(
                "{{\"pid\":{pid},\"procStart\":\"{start}\",\"sessionId\":\"{id}\",\"cwd\":\"{}\",\"name\":\"{name}\",\
\"formerNames\":[{{\"name\":\"old-{name}\"}}]}}",
                cwd.display()
            )
        };
        t.file(".claude/sessions/100.json", &entry(100, 1000, "s1", &intent, "remove-wordmark-title-bar"));
        t.file(".claude/sessions/100.0123abcd.key", "secret");
        t.file(".claude/sessions/200.json", &entry(200, 2000, "s2", &intent, "intentionality-8e"));
        t.file(".claude/sessions/300.json", &entry(300, 3000, "s3", &projects, "stale"));
        t.file(
            ".claude/projects/-intent/s2.jsonl",
            &format!(
                "{{\"type\":\"user\",\"cwd\":\"{}\"}}\n{{\"type\":\"ai-title\",\"aiTitle\":\"Jev terminal window relevance matching\"}}\n\
{{\"type\":\"ai-title\",\"aiTitle\":\"give-jev-terminal-folder-context\"}}\n",
                intent.display()
            ),
        );
        t.file(
            ".claude/projects/-home/s9.jsonl",
            &format!(
                "{{\"type\":\"user\",\"cwd\":\"{}\"}}\n{{\"type\":\"ai-title\",\"aiTitle\":\"Allocating more memory on Linux\"}}\n",
                t.0.display()
            ),
        );
        let mut m = Machine::at(t.0.clone());
        m.proc = t.0.join("proc");
        m
    }

    #[test]
    fn a_claude_code_title_finds_its_session_folder() {
        let t = Tmp::new("claude");
        let m = claude_home(&t);
        let intent = t.0.join("Desktop/projects/intentionality");
        // By the registry's name, a former name, or the transcript's title.
        assert_eq!(m.claude_cwd(Some("remove-wordmark-title-bar")), Some(intent.clone()));
        assert_eq!(m.claude_cwd(Some("Old-Remove-Wordmark-Title-Bar")), Some(intent.clone()));
        assert_eq!(m.claude_cwd(Some("give-jev-terminal-folder-context")), Some(intent.clone()));
        // A title it has since replaced: a tab seen earlier still carried it.
        assert_eq!(m.claude_cwd(Some("jev terminal  window relevance matching")), Some(intent.clone()));
        // A session that has ended: its transcript still knows.
        assert_eq!(m.claude_cwd(Some("Allocating more memory on Linux")), Some(t.0.clone()));
        // The stale record's pid is another process now.
        assert_eq!(m.claude_cwd(Some("stale")), None);
        assert_eq!(m.claude_cwd(Some("never heard of it")), None);
        // s1 has no transcript yet, s2 is titled: the one unnamed session is s1.
        assert_eq!(m.claude_cwd(None), Some(intent));
    }

    #[test]
    fn unnamed_sessions_in_two_folders_are_no_answer() {
        let t = Tmp::new("unnamed");
        let m = claude_home(&t);
        // A second untitled session, in another folder.
        let other = t.dir("Desktop/projects/kiki");
        t.file("proc/400/stat", "400 (claude) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 4000 20\n");
        t.file(
            ".claude/sessions/400.json",
            &format!("{{\"pid\":400,\"procStart\":\"4000\",\"sessionId\":\"s4\",\"cwd\":\"{}\",\"name\":\"kiki-1f\"}}", other.display()),
        );
        assert_eq!(m.unnamed_cwd(), None);
    }

    #[test]
    fn tilde_hides_the_home_folder() {
        let home = Path::new("/home/d");
        assert_eq!(tilde(Path::new("/home/d/Desktop/projects/x"), home), "~/Desktop/projects/x");
        assert_eq!(tilde(Path::new("/home/d"), home), "~");
        assert_eq!(tilde(Path::new("/tmp/x"), home), "/tmp/x");
    }
}
