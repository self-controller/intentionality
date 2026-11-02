//! The apps installed on the machine, from their .desktop files: what each
//! is called ("Obsidian", not "md.obsidian.Obsidian") and which window
//! classes are its windows. The Dashboard's culprits chart names an app's
//! time with it, and its ignore list offers every installed app to pick.
//!
//! A window's class is usually the desktop file's id (flatpaks:
//! "app.zen_browser.zen"), but not always: the VS Code flatpak is
//! "com.visualstudio.code" with windows of class "code", which its
//! `StartupWMClass=Code` says. Both are kept, lowercased.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// One installed app.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct App {
    /// The desktop file's name without ".desktop": "md.obsidian.Obsidian".
    pub id: String,
    /// Its `Name=`: "Obsidian".
    pub name: String,
    /// The window classes that are its windows, lowercased: the id, and its
    /// `StartupWMClass` when it has one.
    pub classes: Vec<String>,
}

/// How long a scan is believed. Installing an app is rare; the ignore menu
/// asks for a fresh scan anyway when it opens.
const KEEP: Duration = Duration::from_secs(300);

/// The `[Desktop Entry]` of one desktop file as an app, or None for one the
/// app grid would not show: not an application, `NoDisplay`, `Hidden`, or
/// nameless. Only that group is read -- an action's group has a `Name=` of
/// its own ("New Private Window").
pub fn parse(id: &str, text: &str) -> Option<App> {
    let mut in_entry = false;
    let (mut name, mut class, mut kind) = (None, None, None);
    let mut shown = true;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        match key.trim() {
            "Name" if name.is_none() => name = Some(value.to_string()),
            "StartupWMClass" => class = Some(value.to_lowercase()),
            "Type" => kind = Some(value.to_string()),
            "NoDisplay" | "Hidden" if value.eq_ignore_ascii_case("true") => shown = false,
            _ => {}
        }
    }
    let name = name.filter(|n| !n.is_empty())?;
    if !shown || kind.as_deref() != Some("Application") {
        return None;
    }
    let mut classes = vec![id.to_lowercase()];
    if let Some(class) = class.filter(|c| !c.is_empty() && !classes.contains(c)) {
        classes.push(class);
    }
    Some(App { id: id.to_string(), name, classes })
}

/// Where desktop files are, most personal first (a user's own copy of an
/// app's file wins over the system's): XDG_DATA_HOME, then XDG_DATA_DIRS,
/// then both flatpak export folders -- an app started outside GNOME may not
/// have them in its XDG_DATA_DIRS.
pub fn dirs(home: &Path) -> Vec<PathBuf> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut roots: Vec<PathBuf> = vec![env("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from)];
    let system = env("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    roots.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    roots.push(home.join(".local/share/flatpak/exports/share"));
    roots.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    let mut seen = HashSet::new();
    roots.into_iter().map(|r| r.join("applications")).filter(|d| seen.insert(d.clone())).collect()
}

/// Every app in `dirs`, A to Z. An id met twice is the first one's.
pub fn scan(dirs: &[PathBuf]) -> Vec<App> {
    let mut ids = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".desktop"))
            else {
                continue;
            };
            if !ids.insert(id.to_string()) {
                continue;
            }
            if let Some(app) = fs::read_to_string(&path).ok().and_then(|text| parse(id, &text)) {
                apps.push(app);
            }
        }
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
    apps
}

/// The apps of the machine the app runs on. Under test, none, so no test
/// depends on what is installed where it runs.
pub fn machine() -> &'static Installed {
    static APPS: OnceLock<Installed> = OnceLock::new();
    APPS.get_or_init(|| {
        #[cfg(test)]
        {
            Installed::new(Vec::new())
        }
        #[cfg(not(test))]
        {
            Installed::new(dirs(Path::new(&std::env::var("HOME").unwrap_or_default())))
        }
    })
}

/// The installed apps as last scanned.
pub struct Installed {
    dirs: Vec<PathBuf>,
    last: Mutex<Option<(Instant, Arc<Vec<App>>)>>,
}

impl Installed {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Installed { dirs, last: Mutex::new(None) }
    }

    /// Every app, from a scan at most KEEP old, or a new one when `fresh`.
    pub fn all(&self, fresh: bool) -> Arc<Vec<App>> {
        let mut last = self.last.lock().unwrap();
        if let Some((at, apps)) = last.as_ref() {
            if !fresh && at.elapsed() < KEEP {
                return apps.clone();
            }
        }
        let apps = Arc::new(scan(&self.dirs));
        *last = Some((Instant::now(), apps.clone()));
        apps
    }

    /// The app whose windows have this class.
    pub fn of_class(&self, class: &str) -> Option<App> {
        let class = class.trim().to_lowercase();
        self.all(false).iter().find(|a| a.classes.contains(&class)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZEN: &str = "[Desktop Entry]\nVersion=1.0\nName=Zen Browser\nExec=zen %u\nType=Application\n\
        StartupWMClass=zen\n\n[Desktop Action new-private-window]\nName=New Private Window\nExec=zen --private-window\n";

    #[test]
    fn an_entry_is_its_own_group_only() {
        let app = parse("app.zen_browser.zen", ZEN).unwrap();
        assert_eq!(app.name, "Zen Browser");
        assert_eq!(app.classes, ["app.zen_browser.zen", "zen"]);
        // The window class differs from the id: both are its windows.
        let code = parse("com.visualstudio.code", "[Desktop Entry]\nName=Visual Studio Code\nType=Application\nStartupWMClass=Code\n").unwrap();
        assert_eq!(code.classes, ["com.visualstudio.code", "code"]);
        // A StartupWMClass that is the id is not listed twice.
        let one = parse("kiki", "[Desktop Entry]\nName=Kiki\nType=Application\nStartupWMClass=Kiki\n").unwrap();
        assert_eq!(one.classes, ["kiki"]);
    }

    #[test]
    fn what_the_app_grid_hides_is_not_an_app() {
        assert_eq!(parse("x", "[Desktop Entry]\nName=X\nType=Application\nNoDisplay=true\n"), None);
        assert_eq!(parse("x", "[Desktop Entry]\nName=X\nType=Application\nHidden=true\n"), None);
        assert_eq!(parse("x", "[Desktop Entry]\nName=X\nType=Link\n"), None);
        assert_eq!(parse("x", "[Desktop Entry]\nType=Application\n"), None);
        // A name only an action has is no name.
        assert_eq!(parse("x", "[Desktop Action a]\nName=A\n[Desktop Entry]\nType=Application\n"), None);
        assert!(parse("x", "[Desktop Entry]\nName=X\nType=Application\nNoDisplay=false\n").is_some());
    }

    #[test]
    fn the_users_own_folder_comes_first_and_flatpaks_are_always_looked_in() {
        let found = dirs(Path::new("/home/d"));
        assert!(found.iter().all(|d| d.ends_with("applications")));
        assert!(found.contains(&PathBuf::from("/var/lib/flatpak/exports/share/applications")));
        assert!(found.contains(&PathBuf::from("/home/d/.local/share/flatpak/exports/share/applications")));
        let unique: HashSet<_> = found.iter().collect();
        assert_eq!(unique.len(), found.len());
    }

    #[test]
    fn a_scan_lists_each_id_once_from_the_first_folder() {
        let root = std::env::temp_dir().join(format!("intentionality-desktop-{}", std::process::id()));
        let (mine, system) = (root.join("mine"), root.join("system"));
        fs::create_dir_all(&mine).unwrap();
        fs::create_dir_all(&system).unwrap();
        fs::write(mine.join("b.desktop"), "[Desktop Entry]\nName=Beta (mine)\nType=Application\n").unwrap();
        fs::write(system.join("b.desktop"), "[Desktop Entry]\nName=Beta\nType=Application\n").unwrap();
        fs::write(system.join("a.desktop"), "[Desktop Entry]\nName=alpha\nType=Application\n").unwrap();
        fs::write(system.join("hidden.desktop"), "[Desktop Entry]\nName=H\nType=Application\nNoDisplay=true\n").unwrap();
        fs::write(system.join("notes.txt"), "not a desktop file").unwrap();
        let apps = scan(&[mine, system, root.join("missing")]);
        let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Beta (mine)"]);
        let installed = Installed { dirs: Vec::new(), last: Mutex::new(Some((Instant::now(), Arc::new(apps)))) };
        assert_eq!(installed.of_class(" B ").map(|a| a.name), Some("Beta (mine)".into()));
        assert_eq!(installed.of_class("c"), None);
        fs::remove_dir_all(root).unwrap();
    }
}
