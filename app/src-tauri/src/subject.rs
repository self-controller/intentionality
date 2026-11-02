//! What a focused window is about, for the relevance check, the clock's
//! count and its label. A browser's window is the page in its tab; a code
//! editor is the project it has open, whatever file is in front; a terminal
//! is the project it is working in, or failing one its folder or session;
//! any other app is its window title. The parts of a title that change while
//! the thing stays the same -- unread counts, a dirty marker, a spinner, the
//! browser's or the running process's name -- come off, so one thing is
//! judged once.
//!
//! A browser tab's title can be as bare as "Files", which every course on
//! Canvas has. places.rs reads the browser's history for the page above it
//! ("2026FA - INTRO DATA SCIENCE 01:198:439:09"), and when that says what
//! the title does not, it is part of the tab: Jev sees it, and the Files of
//! one course is judged apart from another's.
//!
//! A terminal's or an editor's title rarely says where it is: fish shortens
//! the path, Claude Code puts its session's topic there instead, VS Code
//! names the folder alone. folder.rs finds the rest on the machine, so Jev
//! is shown the folder and the project, and one verdict covers every
//! terminal and every file of that project.
//!
//! Each subject also says what it is part of, for the Dashboard's culprits
//! chart (`source`): a tab its site ("linkedin.com", the feed and every
//! profile alike), a terminal its tool and project ("Claude Code:
//! intentionality", "Terminal: Pager"), an editor its project, any other
//! app the app ("Discord", every channel alike).
//!
//! An app or site on the ignore list (ignore.rs) is never judged: `of` has
//! nothing to say about it, as about this app's own window.

use crate::folder::{self, Lookup, Project};
use crate::ignore::{self, Rule};
use crate::places::Place;
use crate::screen_time::is_browser;
use std::path::Path;

/// One thing a window can be about.
#[derive(Debug, Clone, PartialEq)]
pub struct Subject {
    /// (window class lowercased, the thing lowercased): what verdicts are
    /// cached under and today's count is kept by. A terminal or editor in a
    /// project is keyed by the project's path: "~/desktop/projects/kiki".
    pub key: (String, String),
    /// What it is called, in the title's own case: "Feed | LinkedIn",
    /// "VS Code: cereal", "Terminal: kiki". `label()` is the clock's cut.
    pub name: String,
    /// What Jev is shown as the window's title. For an editor or terminal
    /// that is the project, not the file or the session -- the verdict is
    /// cached for the project, and file names stay on the machine.
    pub about: String,
    /// What sort of window it is, for Jev: "browser tab", "code editor",
    /// "terminal" or "app window".
    pub kind: &'static str,
    /// The folder it is working in, from `~`, when the machine could tell.
    pub folder: Option<String>,
    /// The project that folder is part of.
    pub project: Option<Project>,
    /// For a browser tab, its site and the page it sits under -- set only
    /// when that page is part of the key, so what Jev sees of it is what the
    /// verdict is cached under.
    pub place: Option<Place>,
    /// What the Dashboard's culprits chart counts it under: a tab's site,
    /// "Claude Code: <project>" / "Terminal: <project>", "VS Code:
    /// <project>", or the app's name.
    pub source: String,
    /// A tab's site, whenever the browser's history knows the title (unlike
    /// `place`, which is only kept when the page above says more).
    pub site: Option<String>,
}

/// What every Claude Code session outside a project is counted under: one
/// bar for all the one-off chats, not one each.
pub const CLAUDE_ELSEWHERE: &str = "Claude Code: other";

/// Longest label the clock shows before cutting it with "…".
const LABEL_CHARS: usize = 32;

/// Windows that are never judged: this app's own (where an appeal is
/// argued) and the file dialogs a browser or editor opens for a moment.
pub(crate) const NEVER: &[&str] = &["intentionality", "xdg-desktop-portal-gtk", "xdg-desktop-portal-gnome"];

/// Code editors whose titles end "<file> - <folder> - <name>": (window
/// class, the name its titles end with, what the clock calls it).
const EDITORS: &[(&str, &str, &str)] = &[
    ("code", "Visual Studio Code", "VS Code"),
    ("com.visualstudio.code", "Visual Studio Code", "VS Code"),
    ("code-oss", "Code - OSS", "Code - OSS"),
    ("codium", "VSCodium", "VSCodium"),
    ("vscodium", "VSCodium", "VSCodium"),
    ("com.vscodium.codium", "VSCodium", "VSCodium"),
    ("cursor", "Cursor", "Cursor"),
];

/// Terminals, by window class: their titles name a directory, or whatever
/// the program inside set (Claude Code: its session's topic).
const TERMINALS: &[&str] = &[
    "org.gnome.ptyxis", "ptyxis", "org.gnome.terminal", "gnome-terminal-server",
    "org.gnome.console", "kgx", "kitty", "alacritty", "foot", "org.wezfurlong.wezterm",
    "wezterm", "org.kde.konsole", "konsole", "com.mitchellh.ghostty", "ghostty",
];

/// A browser's own name, as its titles end: "… — Zen Browser",
/// "… - Google Chrome", "… — Mozilla Firefox Private Browsing".
const BROWSER_NAMES: &[&str] = &[
    "zen", "mozilla firefox", "firefox", "librewolf", "floorp", "waterfox", "chromium",
    "google chrome", "chrome", "brave", "vivaldi", "microsoft edge", "opera",
];

/// What the window (AW's or the shell's `app` and `title`) is about, or None
/// for a window that is never judged: this app's own, a file dialog, or
/// anything on the ignore list.
pub fn of(app: &str, title: &str) -> Option<Subject> {
    of_with(app, title, folder::machine(), &ignore::current())
}

/// `of`, with what the machine says about folders given, ignoring nothing.
pub fn of_in(app: &str, title: &str, look: &dyn Lookup) -> Option<Subject> {
    of_with(app, title, look, &[])
}

/// `of`, with the machine and the ignore list given.
pub fn of_with(app: &str, title: &str, look: &dyn Lookup, ignored: &[Rule]) -> Option<Subject> {
    let class = app.trim().to_lowercase();
    if class.is_empty() || NEVER.contains(&class.as_str()) || ignore::hides_app(ignored, &class) {
        return None;
    }
    let subject = judged(app, title, class, look)?;
    if subject.kind == "browser tab" && !ignored.is_empty() {
        let hidden = match &subject.site {
            Some(site) => ignore::hides_site(ignored, site),
            None => ignore::hides_title(ignored, &clean(app, title)),
        };
        if hidden {
            return None;
        }
    }
    Some(subject)
}

/// What a window is about, the ignore list aside.
fn judged(app: &str, title: &str, class: String, look: &dyn Lookup) -> Option<Subject> {
    if let Some(subject) = editor(&class, title, look) {
        return Some(subject);
    }
    if TERMINALS.contains(&class.as_str()) {
        return terminal(class, title, look);
    }
    let shown = clean(&class, title);
    let mut name = if shown.is_empty() { short_name(app).to_string() } else { shown.clone() };
    let mut key = shown.to_lowercase();
    let browser = is_browser(app);
    let found = (browser && !shown.is_empty()).then(|| look.place(&class, &shown)).flatten();
    let site = found.as_ref().map(|p| p.site.clone()).filter(|s| !s.is_empty());
    // The page above, where it says what the title does not ("Files", not
    // "Assignments: 2026FA - APOCALYPSE NOW 01:840:305:90").
    let place = found.filter(|p| p.part_of.as_deref().is_some_and(|above| adds_to(&shown, above)));
    let source = if browser {
        site.clone().unwrap_or_else(|| title_site(&name))
    } else {
        look.app_name(&class).unwrap_or_else(|| short_name(app).to_string())
    };
    if let Some(above) = place.as_ref().and_then(|p| p.part_of.as_deref()) {
        key = format!("{key} · {}", above.to_lowercase());
        name = format!("{name} · {above}");
    }
    Some(Subject {
        key: (class, key),
        name,
        about: title.trim().to_string(),
        kind: if browser { "browser tab" } else { "app window" },
        folder: None,
        project: None,
        place,
        source,
        site,
    })
}

impl Subject {
    /// The name, cut to fit the clock.
    pub fn label(&self) -> String {
        if self.name.chars().count() <= LABEL_CHARS {
            return self.name.clone();
        }
        let cut: String = self.name.chars().take(LABEL_CHARS - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

/// What a window is cached and counted under. A window that is never judged
/// still has one -- its title, cleaned -- so an appeal over it reads the
/// store like any other.
pub fn page_key(app: &str, title: &str) -> (String, String) {
    of(app, title).map(|s| s.key).unwrap_or_else(|| {
        (app.trim().to_lowercase(), page_title(app, title).to_lowercase())
    })
}

/// What a window is called, before lowercasing: the subject's name, or for
/// a window never judged its title cleaned the same way. What a check
/// shows for a page.
pub fn page_title(app: &str, title: &str) -> String {
    match of(app, title) {
        Some(s) => s.name,
        None => clean(app, title),
    }
}

/// A tab's site as its title gives it, for a tab the history does not know:
/// the title's last part when it is short enough to be a site's name
/// ("Feed | LinkedIn" -> "LinkedIn", "Untitled document - Google Docs" ->
/// "Google Docs"), else the title.
fn title_site(shown: &str) -> String {
    let tail = [" | ", " — ", " – ", " - ", " · "]
        .iter()
        .filter_map(|sep| shown.rfind(sep).map(|at| &shown[at + sep.len()..]))
        .min_by_key(|tail| tail.len())
        .map(str::trim)
        .filter(|t| !t.is_empty() && t.split_whitespace().count() <= 3 && t.chars().count() <= 30);
    tail.unwrap_or(shown).to_string()
}

/// The title with what changes without the page changing taken off --
/// unread counts, status glyphs, a browser's name -- and runs of whitespace
/// made one space. Case kept. "(3) Feed | LinkedIn — Zen Browser" and
/// "Feed | LinkedIn — Zen Browser" are one page.
pub(crate) fn clean(app: &str, title: &str) -> String {
    let mut t = title.trim();
    loop {
        let before = t;
        t = strip_count(t);
        t = t
            .strip_prefix(is_status_glyph)
            .map(str::trim_start)
            .filter(|rest| !rest.is_empty())
            .unwrap_or(t);
        if t == before {
            break;
        }
    }
    if is_browser(app) {
        t = strip_browser(t);
    }
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A mark a program puts in front of its title to say something about its
/// state, not its page: an editor's unsaved dot, a terminal's spinner frames
/// ("◐ build — claude", "⠹ npm run dev"), a done star. Bullets and the
/// geometric-shape, symbol, dingbat and braille blocks.
fn is_status_glyph(c: char) -> bool {
    matches!(
        c,
        '•' | '*' | '·' | '\u{25A0}'..='\u{25FF}' | '\u{2600}'..='\u{27BF}' | '\u{2800}'..='\u{28FF}'
    )
}

/// "(3) Inbox" -> "Inbox"; "(1,204) Feed" and "(99+) Feed" too.
fn strip_count(t: &str) -> &str {
    let Some(rest) = t.strip_prefix('(') else { return t };
    let Some(end) = rest.find(')') else { return t };
    let inner = &rest[..end];
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '+') {
        return t;
    }
    rest[end + 1..].trim_start()
}

/// The last " — …" / " - …" segment, when it names a browser.
fn strip_browser(t: &str) -> &str {
    for sep in [" — ", " – ", " - "] {
        if let Some(at) = t.rfind(sep) {
            let tail = t[at + sep.len()..].trim().to_lowercase();
            if BROWSER_NAMES.iter().any(|b| tail.starts_with(b)) {
                return t[..at].trim_end();
            }
        }
    }
    t
}

/// A code editor's window is its project folder. "calendar.ts - cereal -
/// Visual Studio Code" and ".env - cereal - Visual Studio Code" are both
/// cereal; "report.ipynb - Visual Studio Code" (a file opened on its own, or
/// a folder with nothing open) is what is left. A title that does not end
/// with the editor's name (the empty window, a custom title) is None, and
/// the window is judged by its title like any other app. Where the editor's
/// own records say the folder is, it is keyed by its project's path.
fn editor(class: &str, title: &str, look: &dyn Lookup) -> Option<Subject> {
    let (_, name, short) = EDITORS.iter().find(|(id, _, _)| *id == class)?;
    let t = clean(class, title);
    let rest = t.strip_suffix(name)?.strip_suffix(" - ")?;
    let folder = rest.rsplit(" - ").next().unwrap_or(rest).trim();
    let folder = folder.strip_suffix(" (Workspace)").unwrap_or(folder).trim();
    if folder.is_empty() {
        return None;
    }
    let about = format!("{name} with the project folder \"{folder}\" open");
    let found = look.editor_folder(folder);
    let project = found.as_deref().and_then(|dir| look.project(dir));
    let home = look.home();
    let key = match (&project, &found) {
        (Some(p), _) => folder::tilde(&p.root, home).to_lowercase(),
        (None, Some(dir)) => folder::tilde(dir, home).to_lowercase(),
        (None, None) => folder.to_lowercase(),
    };
    let shown = project.as_ref().map_or(folder, |p| p.name.as_str());
    let name = format!("{short}: {shown}");
    Some(Subject {
        key: (class.to_string(), key),
        source: name.clone(),
        name,
        about,
        kind: "code editor",
        folder: found.map(|dir| folder::tilde(&dir, home)),
        project,
        place: None,
        site: None,
    })
}

/// A terminal's window is the project it is working in. Ptyxis appends
/// " — <process> <args>", and Claude Code animates a spinner in front;
/// both come off. What is left is fish's title -- the folder, shortened
/// ("~/D/p/i/app"), after the command running if there is one -- or, when
/// Claude Code is the process, its session's topic, whose folder Claude
/// Code's own records give. Where neither can be found, the window is what
/// it says: the last folder of its path, or its session's topic, so
/// "✳ Orchestrator review — claude --resume 72f8…" and "✶ Orchestrator
/// review — claude" are still one session.
fn terminal(class: String, title: &str, look: &dyn Lookup) -> Option<Subject> {
    let t = clean(&class, title);
    let (t, process) = match t.rfind(" — ") {
        Some(at) => (t[..at].trim_end(), Some(t[at + " — ".len()..].trim())),
        None => (t.as_str(), None),
    };
    // fish's title holds until the program sets its own: "claude
    // ~/D/p/intentionality" is a session starting there.
    let path = t.rsplit(' ').next().filter(|w| w.starts_with('~') || w.starts_with('/'));
    let claude = process.is_some_and(|p| p.split_whitespace().next() == Some("claude"));
    if let Some(dir) = path.and_then(|p| look.expand(p)) {
        return Some(in_folder(class, &dir, None, claude, look));
    }
    if claude {
        return claude_session(class, t, look);
    }
    let (name, about) = if t.starts_with('~') || t.starts_with('/') {
        let dir = t.trim_end_matches('/').rsplit('/').next().filter(|d| !d.is_empty());
        let dir = dir.unwrap_or(if t.starts_with('~') { "~" } else { "/" }).to_string();
        let about = format!("A terminal in the directory \"{dir}\"");
        (dir, about)
    } else {
        let about = format!("A terminal window titled \"{t}\"");
        (t.to_string(), about)
    };
    let shown = format!("Terminal: {name}");
    Some(Subject {
        key: (class, name.to_lowercase()),
        source: shown.clone(),
        name: shown,
        about,
        kind: "terminal",
        folder: None,
        project: None,
        place: None,
        site: None,
    })
}

/// A terminal running Claude Code, titled with its session's topic. A
/// session with no topic yet ("Claude Code") whose folder cannot be told is
/// not judged: its title says nothing, and every such session would share
/// one verdict.
fn claude_session(class: String, topic: &str, look: &dyn Lookup) -> Option<Subject> {
    let named = !is_untitled(topic);
    match look.claude_cwd(named.then_some(topic)) {
        Some(dir) => Some(in_folder(class, &dir, named.then_some(topic), true, look)),
        None if named => Some(Subject {
            key: (class, topic.to_lowercase()),
            name: format!("Terminal: {topic}"),
            about: claude_about(topic),
            kind: "terminal",
            folder: None,
            project: None,
            place: None,
            source: CLAUDE_ELSEWHERE.into(),
            site: None,
        }),
        None => None,
    }
}

/// What Claude Code titles a session before it has a topic, its resume
/// picker ("claude · resume"), and Ptyxis's own "Terminal" before either.
fn is_untitled(topic: &str) -> bool {
    let t = topic.to_lowercase();
    ["", "claude", "claude code", "terminal"].contains(&t.as_str()) || t.starts_with("claude ·")
}

fn claude_about(topic: &str) -> String {
    format!("Claude Code, an AI coding assistant, in a session titled \"{topic}\"")
}

/// A terminal working in `dir`: the project's when the folder is in one,
/// whatever the session or command, so one verdict holds for every terminal
/// on that project. Outside a project it is the folder's, except that a
/// Claude Code session there is its own -- a home folder holds sessions
/// about anything. `claude`: Claude Code is what runs in it, which the
/// culprits chart tells apart from the shell: "Claude Code: Pager" and
/// "Terminal: Pager" share a verdict but not a bar.
fn in_folder(class: String, dir: &Path, topic: Option<&str>, claude: bool, look: &dyn Lookup) -> Subject {
    let home = look.home();
    let shown = folder::tilde(dir, home);
    if let Some(project) = look.project(dir) {
        let tool = if claude { "Claude Code" } else { "Terminal" };
        return Subject {
            key: (class, folder::tilde(&project.root, home).to_lowercase()),
            name: format!("Terminal: {}", project.name),
            about: format!("A terminal working in the project \"{}\"", project.name),
            kind: "terminal",
            folder: Some(shown),
            source: format!("{tool}: {}", project.name),
            project: Some(project),
            place: None,
            site: None,
        };
    }
    let (key, name, about) = match topic {
        Some(topic) => (
            format!("{} · {}", shown.to_lowercase(), topic.to_lowercase()),
            topic.to_string(),
            claude_about(topic),
        ),
        None => {
            let last = dir.file_name().map(|n| n.to_string_lossy().into_owned());
            let last = if shown == "~" { None } else { last };
            (shown.to_lowercase(), last.unwrap_or_else(|| shown.clone()), format!("A terminal in the folder \"{shown}\""))
        }
    };
    let name = format!("Terminal: {name}");
    Subject {
        key: (class, key),
        source: if claude { CLAUDE_ELSEWHERE.into() } else { name.clone() },
        name,
        about,
        kind: "terminal",
        folder: Some(shown),
        project: None,
        place: None,
        site: None,
    }
}

/// Whether the page above says something the tab's own title does not: at
/// least half its words are new. "Quizzes: 2026FA - INTRO DATA SCIENCE …"
/// over "Quiz 3: 2026FA - INTRO DATA SCIENCE …" adds one word of nine.
fn adds_to(title: &str, above: &str) -> bool {
    let words = |t: &str| -> Vec<String> {
        t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
    };
    let own = words(title);
    let above = words(above);
    let new = above.iter().filter(|w| !own.contains(w)).count();
    !above.is_empty() && new * 2 >= above.len()
}

/// An app id's last part, for a window with no title: "org.gnome.Papers"
/// is Papers.
pub(crate) fn short_name(app: &str) -> &str {
    app.trim().rsplit('.').next().unwrap_or(app)
}


#[cfg(test)]
mod tests {
    use super::*;

    const ZEN: &str = "app.zen_browser.zen";
    const PTYXIS: &str = "org.gnome.Ptyxis";

    fn key(app: &str, title: &str) -> String {
        of(app, title).unwrap().key.1
    }

    #[test]
    fn a_browser_title_loses_its_count_and_its_browser() {
        assert_eq!(
            page_key(ZEN, "(3) Feed | LinkedIn — Zen Browser"),
            (ZEN.into(), "feed | linkedin".into())
        );
        assert_eq!(page_key(ZEN, "Feed | LinkedIn — Zen Browser").1, "feed | linkedin");
        assert_eq!(page_key("Google-chrome", "(99+) Inbox - Google Chrome").1, "inbox");
        assert_eq!(
            page_key("org.mozilla.firefox", "● Draft - Mozilla Firefox Private Browsing").1,
            "draft"
        );
        // Only the browser's own segment goes: a title with its own dash keeps it.
        assert_eq!(
            page_key(ZEN, "Senior Engineer - Acme | LinkedIn — Zen Browser").1,
            "senior engineer - acme | linkedin"
        );
        assert_eq!(page_key(ZEN, "Home / X — Zen Browser").1, "home / x");
        // The clock keeps the title's case; Jev sees the title as it was.
        let s = of(ZEN, "(3) Feed | LinkedIn — Zen Browser").unwrap();
        assert_eq!(s.label(), "Feed | LinkedIn");
        assert_eq!(s.about, "(3) Feed | LinkedIn — Zen Browser");
    }

    /// One terminal running one job is one page, whatever frame its spinner
    /// was on; and a title's own symbols past the front are left alone.
    #[test]
    fn status_glyphs_in_front_of_a_title_are_not_the_page() {
        let key = |t: &str| page_key(PTYXIS, t).1;
        assert_eq!(key("◐ Jev run tab and app checks — claude"), "jev run tab and app checks");
        assert_eq!(key("◑ Jev run tab and app checks — claude"), key("◐ Jev run tab and app checks — claude"));
        assert_eq!(key("✳ Jev run tab"), "jev run tab");
        assert_eq!(key("⠹ npm run dev"), "npm run dev");
        assert_eq!(key("● main.rs - code"), "main.rs - code");
        assert_eq!(key("#general ★ - Discord"), "#general ★ - discord");
        assert_eq!(key("★"), "★"); // nothing else: the glyph is all there is to key on
        let key = |t: &str| page_key("discord", t).1;
        assert_eq!(key("◐ #general - Discord"), "#general - discord");
    }

    #[test]
    fn an_app_title_is_kept_whole() {
        assert_eq!(page_key("discord", "(2) #general - Discord").1, "#general - discord");
        assert_eq!(page_key(" Spotify ", "  Spotify   Premium ").1, "spotify premium");
        assert_eq!(page_key("x", "(not a count) Thing").1, "(not a count) thing");
        // Each channel and each document is its own thing.
        assert_ne!(key("discord", "#general | fruity tartz - Discord"), key("discord", "totss - Discord"));
        assert_ne!(
            key("org.gnome.Papers", "Assignment 3-1.pdf"),
            key("org.gnome.Papers", "regression_methods_exam_cheat_sheet.pdf")
        );
    }

    /// Real titles from two weeks of ActivityWatch: every file in a folder
    /// is the folder.
    #[test]
    fn an_editor_is_the_project_folder_it_has_open() {
        for title in [
            "calendar.ts - cereal - Visual Studio Code",
            ".env - cereal - Visual Studio Code",
            "Welcome - cereal - Visual Studio Code",
            "Extension: vscode-pets - cereal - Visual Studio Code",
            "Cereal-0.1.0.flatpak - cereal - Visual Studio Code",
        ] {
            assert_eq!(page_key("code", title), ("code".into(), "cereal".into()), "{title}");
        }
        assert_eq!(key("code", "● train.ipynb - fervor - Visual Studio Code"), "fervor");
        assert_eq!(key("code", "config.py - fervor - Visual Studio Code"), "fervor");
        assert_eq!(key("code", "Preview README.md - kiki - Visual Studio Code"), "kiki");
        assert_eq!(key("code", "● Q1.ipynb - assignment_3 - Visual Studio Code"), "assignment_3");
        // A file whose own name has a dash still names its folder last.
        assert_eq!(key("code", "a - b.md - Reader - Visual Studio Code"), "reader");
        assert_eq!(key("code", "main.rs - mono (Workspace) - Visual Studio Code"), "mono");
        // Nothing but a file: the file is what there is to go on.
        assert_eq!(key("code", "report.ipynb - Visual Studio Code"), "report.ipynb");
        assert_eq!(key("code-oss", "lib.rs - kiki - Code - OSS"), "kiki");
        let s = of("code", "calendar.ts - cereal - Visual Studio Code").unwrap();
        assert_eq!(s.label(), "VS Code: cereal");
        assert_eq!(s.about, "Visual Studio Code with the project folder \"cereal\" open");
        assert!(!s.about.contains("calendar.ts"));
        // The empty window has no folder: judged by its title.
        assert_eq!(key("code", "Visual Studio Code"), "visual studio code");
    }

    #[test]
    fn a_terminal_is_its_directory_or_its_session() {
        assert_eq!(key(PTYXIS, "~/D/p/kiki"), "kiki");
        assert_eq!(key(PTYXIS, "~/D/projects"), "projects");
        assert_eq!(key(PTYXIS, "~"), "~");
        assert_eq!(key(PTYXIS, "/"), "/");
        assert_eq!(key(PTYXIS, "~/Downloads/"), "downloads");
        // Claude Code: one session whatever the spinner frame or the arguments.
        let resumed = key(
            PTYXIS,
            "✳ Orchestrator agent planning and review — claude --resume 72f8de96-0dd4-43a5-b0cd-70583f30452a --permission-mode auto ",
        );
        assert_eq!(resumed, "orchestrator agent planning and review");
        assert_eq!(key(PTYXIS, "✶ Orchestrator agent planning and review — claude "), resumed);
        assert_eq!(key(PTYXIS, "· Orchestrator agent planning and review"), resumed);
        assert_ne!(key(PTYXIS, "✳ Daily water usage — claude "), resumed);
        let s = of(PTYXIS, "~/D/p/little-orchestra").unwrap();
        assert_eq!(s.label(), "Terminal: little-orchestra");
        assert_eq!(s.about, "A terminal in the directory \"little-orchestra\"");
        let s = of(PTYXIS, "✳ Jev run tab and app checks — claude ").unwrap();
        assert_eq!(s.name, "Terminal: Jev run tab and app checks");
        assert_eq!(s.label(), "Terminal: Jev run tab and app c…");
        assert_eq!(
            s.about,
            "Claude Code, an AI coding assistant, in a session titled \"Jev run tab and app checks\""
        );
        assert_eq!((s.kind, s.folder, s.project), ("terminal", None, None));
        // Untitled, and no way to tell where: not judged at all.
        assert_eq!(of(PTYXIS, "✳ Claude Code — claude "), None);
        assert_eq!(of(PTYXIS, "claude · resume — claude -resume "), None);
    }

    /// A machine that knows one project, intentionality (a repository with a
    /// README), the folders above it, a Claude Code session in it, one in
    /// the home folder, and VS Code's record of a folder.
    struct Fake {
        home: std::path::PathBuf,
        /// Whether the untitled sessions are all in intentionality.
        untitled_here: bool,
    }

    impl Fake {
        fn new(untitled_here: bool) -> Self {
            Fake { home: "/home/d".into(), untitled_here }
        }
        fn at(&self, rel: &str) -> std::path::PathBuf {
            self.home.join(rel)
        }
    }

    impl Lookup for Fake {
        fn home(&self) -> &Path {
            &self.home
        }
        fn expand(&self, shown: &str) -> Option<std::path::PathBuf> {
            match shown {
                "~/D/p/intentionality" => Some(self.at("Desktop/projects/intentionality")),
                "~/D/p/i/app" => Some(self.at("Desktop/projects/intentionality/app")),
                "~/D/projects" => Some(self.at("Desktop/projects")),
                "~" => Some(self.home.clone()),
                _ => None,
            }
        }
        fn claude_cwd(&self, topic: Option<&str>) -> Option<std::path::PathBuf> {
            match topic {
                None => self.untitled_here.then(|| self.at("Desktop/projects/intentionality")),
                Some("remove-wordmark-title-bar") => Some(self.at("Desktop/projects/intentionality/app")),
                Some("Allocating more memory on Linux") | Some("Plan a trip") => Some(self.home.clone()),
                Some(_) => None,
            }
        }
        fn editor_folder(&self, folder: &str) -> Option<std::path::PathBuf> {
            (folder == "intentionality").then(|| self.at("Desktop/projects/intentionality"))
        }
        fn project(&self, dir: &Path) -> Option<Project> {
            let root = self.at("Desktop/projects/intentionality");
            dir.starts_with(&root).then(|| Project {
                root,
                name: "intentionality".into(),
                about: Some("A gate that sits between login and your desktop.".into()),
            })
        }
        /// The browser's history knows two Canvas pages, two LinkedIn ones
        /// and a Google Doc.
        fn place(&self, _app: &str, title: &str) -> Option<Place> {
            let site = |site: &str| Some(Place { site: site.into(), part_of: None });
            let part_of = match title {
                "Feed | LinkedIn" | "Ada Lovelace | LinkedIn" => return site("linkedin.com"),
                "Untitled document - Google Docs" => return site("docs.google.com"),
                "Files" => "2026FA - INTRO DATA SCIENCE 01:198:439:09",
                "Assignments: 2026FA - APOCALYPSE NOW 01:840:305:90" => "2026FA - APOCALYPSE NOW 01:840:305:90",
                "Quiz 3: 2026FA - INTRO DATA SCIENCE 01:198:439:09" => "Quizzes: 2026FA - INTRO DATA SCIENCE 01:198:439:09",
                "Mid-Term Take Home Essay Exam" => "Assignments: 2026FA - APOCALYPSE NOW 01:840:305:90",
                _ => return None,
            };
            Some(Place { site: "rutgers.instructure.com".into(), part_of: Some(part_of.into()) })
        }
        fn app_name(&self, class: &str) -> Option<String> {
            (class == "com.discordapp.discord").then(|| "Discord".into())
        }
    }

    /// The flagged case: Canvas titles a course's files "Files", and Jev,
    /// shown only that, found it unrelated to "Study for Intro to DS".
    #[test]
    fn a_bare_tab_title_is_the_course_page_it_sits_under() {
        let look = Fake::new(false);
        let s = of_in(ZEN, "Files — Zen Browser", &look).unwrap();
        assert_eq!(s.key, (ZEN.into(), "files · 2026fa - intro data science 01:198:439:09".into()));
        assert_eq!(s.name, "Files · 2026FA - INTRO DATA SCIENCE 01:198:439:09");
        assert_eq!(s.label(), "Files · 2026FA - INTRO DATA SCI…");
        assert_eq!(s.about, "Files — Zen Browser");
        assert_eq!(s.place.unwrap().site, "rutgers.instructure.com");
        // A title that already names its course is keyed as before.
        let s = of_in(ZEN, "Assignments: 2026FA - APOCALYPSE NOW 01:840:305:90 — Zen Browser", &look).unwrap();
        assert_eq!(s.key.1, "assignments: 2026fa - apocalypse now 01:840:305:90");
        assert_eq!(s.place, None);
        // So is one whose page above only repeats it.
        let s = of_in(ZEN, "Quiz 3: 2026FA - INTRO DATA SCIENCE 01:198:439:09 — Zen Browser", &look).unwrap();
        assert_eq!(s.key.1, "quiz 3: 2026fa - intro data science 01:198:439:09");
        // A specific title still gains the course it is in.
        assert_eq!(
            of_in(ZEN, "Mid-Term Take Home Essay Exam — Zen Browser", &look).unwrap().key.1,
            "mid-term take home essay exam · assignments: 2026fa - apocalypse now 01:840:305:90"
        );
        // Not in the history, or not a browser: the title alone.
        assert_eq!(of_in(ZEN, "Feed | LinkedIn — Zen Browser", &look).unwrap().key.1, "feed | linkedin");
        assert_eq!(of_in("org.gnome.Nautilus", "Files", &look).unwrap().key.1, "files");
    }

    /// Today's windows from ActivityWatch, as (app, title) -> seconds,
    /// for those `keep` says. Read from this machine; sends nothing.
    fn todays_windows(keep: impl Fn(&str) -> bool) -> std::collections::BTreeMap<(String, String), f64> {
        use std::io::{Read as _, Write as _};
        let day = chrono::Local::now().format("%Y-%m-%d");
        let path = format!(
            "/api/0/buckets/aw-watcher-window_{}/events?start={day}T00:00:00&limit=20000",
            std::fs::read_to_string("/etc/hostname").unwrap_or_default().trim()
        );
        let mut aw = std::net::TcpStream::connect("127.0.0.1:5600").expect("ActivityWatch");
        write!(aw, "GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").unwrap();
        let mut reply = String::new();
        aw.read_to_string(&mut reply).unwrap();
        let body = reply.split_once("\r\n\r\n").map_or("[]", |(_, b)| b);
        let events: Vec<serde_json::Value> = serde_json::from_str(body).unwrap_or_default();
        let mut seen = std::collections::BTreeMap::new();
        for e in &events {
            let (app, title) = (e["data"]["app"].as_str().unwrap_or(""), e["data"]["title"].as_str().unwrap_or(""));
            if keep(app) {
                *seen.entry((app.to_string(), title.to_string())).or_insert(0.0) += e["duration"].as_f64().unwrap_or(0.0);
            }
        }
        seen
    }

    /// What Jev would be shown for every browser tab ActivityWatch saw
    /// today: the site and the page above, from the browser's history.
    /// Sends nothing. `cargo test live_tabs -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_tabs() {
        let seen = todays_windows(is_browser);
        let machine = folder::Machine::at(std::env::var("HOME").unwrap().into());
        println!("{} browser titles today", seen.len());
        for ((app, title), secs) in seen {
            if let Some(s) = of_in(&app, &title, &machine) {
                let place = s.place.as_ref().map(|p| (&p.site, &p.part_of));
                println!("{secs:>6.0}s  {title:?}\n        key {:?}  place {place:?}", s.key.1);
            }
        }
    }

    /// What Jev would be shown for every terminal and editor window
    /// ActivityWatch saw today, read from this machine. Sends nothing.
    /// `cargo test live_windows -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_windows() {
        let seen = todays_windows(|app| {
            let class = app.to_lowercase();
            TERMINALS.contains(&class.as_str()) || EDITORS.iter().any(|(id, _, _)| *id == class)
        });
        let machine = folder::Machine::at(std::env::var("HOME").unwrap().into());
        println!("{} terminal/editor titles today", seen.len());
        for ((app, title), secs) in seen {
            match of_in(&app, &title, &machine) {
                Some(s) => println!(
                    "{secs:>6.0}s  {title:?}\n        key {:?}  name {:?}\n        folder {:?}  project {:?}\n        about {:?}",
                    s.key.1,
                    s.name,
                    s.folder,
                    s.project.as_ref().map(|p| (&p.name, &p.about)),
                    s.about
                ),
                None => println!("{secs:>6.0}s  {title:?}\n        not judged"),
            }
        }
    }

    /// The case that was flagged: "Work on Intentionality" against a Claude
    /// Code tab whose title only gave its topic.
    #[test]
    fn every_terminal_in_a_project_is_the_project() {
        let look = Fake::new(true);
        let key = |t: &str| of_in(PTYXIS, t, &look).map(|s| s.key.1);
        let project = Some("~/desktop/projects/intentionality".to_string());
        for title in [
            "~/D/p/intentionality",
            "~/D/p/i/app",
            "npm run tauri build ~/D/p/i/app — npm run tauri build                ",
            "✳ remove-wordmark-title-bar — claude ",
            "◐ remove-wordmark-title-bar — claude --resume 82ff",
            "✳ Claude Code — claude ",
            // fish's title, before Claude Code set its own.
            "claude ~/D/p/intentionality — claude ",
        ] {
            assert_eq!(key(title), project, "{title}");
        }
        let s = of_in(PTYXIS, "✳ remove-wordmark-title-bar — claude ", &look).unwrap();
        assert_eq!(s.name, "Terminal: intentionality");
        assert_eq!(s.about, "A terminal working in the project \"intentionality\"");
        assert_eq!(s.folder.as_deref(), Some("~/Desktop/projects/intentionality/app"));
        assert_eq!(s.project.as_ref().unwrap().about.as_deref(), Some("A gate that sits between login and your desktop."));
        // The topic is not what is judged: the project's verdict is every session's.
        assert!(!s.about.contains("wordmark"));
    }

    #[test]
    fn outside_a_project_a_session_is_its_own_and_a_shell_its_folder() {
        let look = Fake::new(false);
        let s = of_in(PTYXIS, "✳ Allocating more memory on Linux — claude ", &look).unwrap();
        assert_eq!(s.key.1, "~ · allocating more memory on linux");
        assert_eq!(s.name, "Terminal: Allocating more memory on Linux");
        assert_eq!(s.folder.as_deref(), Some("~"));
        assert!(s.about.contains("Allocating more memory on Linux"));
        let trip = of_in(PTYXIS, "✳ Plan a trip — claude ", &look).unwrap();
        assert_ne!(trip.key, s.key);
        let shell = of_in(PTYXIS, "~/D/projects", &look).unwrap();
        assert_eq!((shell.key.1.as_str(), shell.name.as_str()), ("~/desktop/projects", "Terminal: projects"));
        assert_eq!(shell.about, "A terminal in the folder \"~/Desktop/projects\"");
        let home = of_in(PTYXIS, "~", &look).unwrap();
        assert_eq!((home.key.1.as_str(), home.name.as_str()), ("~", "Terminal: ~"));
        // Untitled sessions in more than one folder: nothing to go on.
        assert_eq!(of_in(PTYXIS, "✳ Claude Code — claude ", &look), None);
        // A session the records do not know is judged by its topic, as before.
        let unknown = of_in(PTYXIS, "✳ Rust backend rewrite — claude ", &look).unwrap();
        assert_eq!((unknown.key.1.as_str(), unknown.folder), ("rust backend rewrite", None));
    }

    #[test]
    fn an_editor_found_on_disk_is_its_projects_path() {
        let look = Fake::new(false);
        let s = of_in("code", "main.rs - intentionality - Visual Studio Code", &look).unwrap();
        assert_eq!(s.key, ("code".into(), "~/desktop/projects/intentionality".into()));
        assert_eq!(s.name, "VS Code: intentionality");
        assert_eq!(s.kind, "code editor");
        assert_eq!(s.folder.as_deref(), Some("~/Desktop/projects/intentionality"));
        assert!(s.project.is_some());
        // A folder VS Code has no record of keeps its name.
        let s = of_in("code", "calendar.ts - cereal - Visual Studio Code", &look).unwrap();
        assert_eq!((s.key.1.as_str(), s.folder), ("cereal", None));
    }

    fn source(app: &str, title: &str) -> String {
        of_in(app, title, &Fake::new(false)).unwrap().source
    }

    /// The culprits chart's names: a site whatever the page, a tool and its
    /// project whatever the session, an app whatever the channel.
    #[test]
    fn what_a_window_is_part_of() {
        // Every LinkedIn page is linkedin.com; the history says so.
        assert_eq!(source(ZEN, "(3) Feed | LinkedIn — Zen Browser"), "linkedin.com");
        assert_eq!(source(ZEN, "Ada Lovelace | LinkedIn — Zen Browser"), "linkedin.com");
        let s = of_in(ZEN, "Feed | LinkedIn — Zen Browser", &Fake::new(false)).unwrap();
        assert_eq!((s.site.as_deref(), s.place), (Some("linkedin.com"), None));
        assert_eq!(source(ZEN, "Untitled document - Google Docs — Zen Browser"), "docs.google.com");
        assert_eq!(source(ZEN, "Files — Zen Browser"), "rutgers.instructure.com");
        // The history does not know it: the title's short last part, or the title.
        assert_eq!(source(ZEN, "Grace Hopper | LinkedIn — Zen Browser"), "LinkedIn");
        assert_eq!(source(ZEN, "why do warheads taste fishy - Google Search — Zen Browser"), "Google Search");
        assert_eq!(source(ZEN, "Some Long Essay - by a writer with many names — Zen Browser"), "Some Long Essay - by a writer with many names");
        assert_eq!(source(ZEN, "Substack — Zen Browser"), "Substack");

        // Claude Code and the shell in one project: one verdict, two bars.
        let look = Fake::new(true);
        let src = |t: &str| of_in(PTYXIS, t, &look).unwrap().source;
        assert_eq!(src("✳ remove-wordmark-title-bar — claude "), "Claude Code: intentionality");
        assert_eq!(src("✳ Claude Code — claude "), "Claude Code: intentionality");
        assert_eq!(src("claude ~/D/p/intentionality — claude "), "Claude Code: intentionality");
        assert_eq!(src("~/D/p/i/app"), "Terminal: intentionality");
        assert_eq!(src("npm run tauri build ~/D/p/i/app — npm run tauri build "), "Terminal: intentionality");
        assert_eq!(
            of_in(PTYXIS, "✳ remove-wordmark-title-bar — claude ", &look).unwrap().key,
            of_in(PTYXIS, "~/D/p/i/app", &look).unwrap().key
        );
        // The clock's label is unchanged.
        assert_eq!(of_in(PTYXIS, "✳ remove-wordmark-title-bar — claude ", &look).unwrap().name, "Terminal: intentionality");
        // Outside a project every Claude Code session is one bar; a shell is its folder.
        assert_eq!(source(PTYXIS, "✳ Allocating more memory on Linux — claude "), CLAUDE_ELSEWHERE);
        assert_eq!(source(PTYXIS, "✳ Rust backend rewrite — claude "), CLAUDE_ELSEWHERE);
        assert_eq!(source(PTYXIS, "~/D/projects"), "Terminal: projects");
        assert_eq!(source(PTYXIS, "htop"), "Terminal: htop");

        assert_eq!(source("code", "main.rs - intentionality - Visual Studio Code"), "VS Code: intentionality");
        assert_eq!(source("code", "calendar.ts - cereal - Visual Studio Code"), "VS Code: cereal");
        // An app is the app, by its installed name when it has one.
        assert_eq!(source("com.discordapp.Discord", "#general | fruity tartz - Discord"), "Discord");
        assert_eq!(source("com.discordapp.Discord", "totss - Discord"), "Discord");
        assert_eq!(source("org.gnome.Papers", "Assignment 3-1.pdf"), "Papers");
    }

    fn rule(kind: ignore::Kind, value: &str, classes: &[&str]) -> Rule {
        ignore::normalize(Rule {
            kind,
            value: value.into(),
            name: String::new(),
            classes: classes.iter().map(|c| c.to_string()).collect(),
        })
        .unwrap()
    }

    #[test]
    fn an_ignored_app_or_site_is_never_judged() {
        let look = Fake::new(false);
        let of = |app: &str, title: &str, rules: &[Rule]| of_with(app, title, &look, rules).map(|s| s.source);
        let code = [rule(ignore::Kind::App, "com.visualstudio.code", &["Code"])];
        assert_eq!(of("code", "main.rs - intentionality - Visual Studio Code", &code), None);
        assert_eq!(of("Code", "main.rs - intentionality - Visual Studio Code", &code), None);
        assert!(of(PTYXIS, "~/D/projects", &code).is_some());
        // A browser ignored is every tab in it.
        let zen = [rule(ignore::Kind::App, ZEN, &[])];
        assert_eq!(of(ZEN, "Feed | LinkedIn — Zen Browser", &zen), None);
        // A site: every page of it, by the history -- and by the title when
        // the history does not know the page.
        let linkedin = [rule(ignore::Kind::Site, "https://www.linkedin.com/feed", &[])];
        assert_eq!(of(ZEN, "Feed | LinkedIn — Zen Browser", &linkedin), None);
        assert_eq!(of(ZEN, "Grace Hopper | LinkedIn — Zen Browser", &linkedin), None);
        assert!(of(ZEN, "Files — Zen Browser", &linkedin).is_some());
        // Only browsers have sites: an app titled LinkedIn is not the site.
        assert!(of("org.gnome.Papers", "LinkedIn.pdf", &linkedin).is_some());
        // A part of a site is that part only.
        let google = [rule(ignore::Kind::Site, "google.com", &[])];
        assert_eq!(of(ZEN, "Untitled document - Google Docs — Zen Browser", &google), None);
        let docs = [rule(ignore::Kind::Site, "docs.google.com", &[])];
        assert_eq!(of(ZEN, "Untitled document - Google Docs — Zen Browser", &docs), None);
        assert!(of(ZEN, "why do warheads taste fishy - Google Search — Zen Browser", &docs).is_some());
        // Ignoring nothing changes nothing.
        assert_eq!(of_with(ZEN, "Files — Zen Browser", &look, &[]), of_in(ZEN, "Files — Zen Browser", &look));
    }

    #[test]
    fn this_app_and_file_dialogs_are_never_judged() {
        assert_eq!(of("intentionality", "intentionality"), None);
        assert_eq!(of("xdg-desktop-portal-gtk", "Save As - lecture01.pdf — Zen Browser"), None);
        assert_eq!(of("", ""), None);
        assert_eq!(of("  ", "Something"), None);
        // An appeal over one still reads the store under a key.
        assert_eq!(page_key("intentionality", "intentionality").1, "intentionality");
    }

    #[test]
    fn a_long_label_is_cut_and_an_untitled_window_is_its_app() {
        let s = of(ZEN, "Senior Software Engineer, Platform Infrastructure | LinkedIn — Zen Browser").unwrap();
        assert_eq!(s.label().chars().count(), LABEL_CHARS);
        assert!(s.label().ends_with('…'));
        assert!(!s.name.ends_with('…')); // a check shows it whole
        let s = of("org.gnome.Papers", "").unwrap();
        assert_eq!((s.key.1.as_str(), s.name.as_str()), ("", "Papers"));
    }
}
