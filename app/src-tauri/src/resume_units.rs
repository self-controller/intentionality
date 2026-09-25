//! Is the resume gate actually installed?
//!
//! The gate that fires on wake lives in two system units under `systemd/`, and
//! installing them is a sudo step this app cannot take. On 2026-09-11 they had
//! been sitting uninstalled for nine days of lid-closes, with nothing on screen
//! to say why no gate ever appeared. So the app checks, the way build_info
//! checks the binary: every start, read-only, and silent when all is well.

use std::fs;
use std::path::Path;

use crate::build_info::REPO;

const ETC: &str = "/etc/systemd/system";
const UNITS: &[&str] = &[
    "intentionality-resume.service",
    "intentionality-resume-gate.service",
];
/// What `systemctl enable` creates. Its absence covers both "never copied" and
/// "copied but never enabled" — either way nothing hangs off suspend.target.
const WANTS: &str = "suspend.target.wants/intentionality-resume.service";

pub fn warning() -> Option<String> {
    check(Path::new(ETC), &Path::new(REPO).join("systemd"))
}

fn check(etc: &Path, repo_units: &Path) -> Option<String> {
    // A binary moved away from its repo has nothing to compare against: skip
    // the check rather than fail it, like build_info's staleness.
    if !repo_units.is_dir() {
        return None;
    }
    // symlink_metadata, not metadata: a dangling link still counts as enabled
    // here, and then fails the comparison below with the right advice.
    if fs::symlink_metadata(etc.join(WANTS)).is_err() {
        return Some("resume gate not installed — sudo steps in README \"Waking from sleep\"".into());
    }
    // Bytes, not mtimes: `cp` without -p stamps the copy with the install time.
    let differs = UNITS
        .iter()
        .any(|unit| fs::read(etc.join(unit)).ok() != fs::read(repo_units.join(unit)).ok());
    differs.then(|| "resume units in /etc differ from systemd/ — re-copy them".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A directory of our own per test, as in attach.rs: no tempfile crate.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
            let dir = std::env::temp_dir()
                .join(format!("resume-units-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Tmp(dir)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A repo `systemd/` holding both units, and an empty `/etc` stand-in.
    fn layout(tag: &str) -> (Tmp, PathBuf, PathBuf) {
        let tmp = Tmp::new(tag);
        let (etc, repo) = (tmp.0.join("etc"), tmp.0.join("repo"));
        fs::create_dir_all(etc.join("suspend.target.wants")).unwrap();
        fs::create_dir_all(&repo).unwrap();
        for unit in UNITS {
            fs::write(repo.join(unit), format!("# {unit}\n")).unwrap();
        }
        (tmp, etc, repo)
    }

    fn copy_units(etc: &Path, repo: &Path) {
        for unit in UNITS {
            fs::copy(repo.join(unit), etc.join(unit)).unwrap();
        }
    }

    fn enable(etc: &Path) {
        std::os::unix::fs::symlink(etc.join(UNITS[0]), etc.join(WANTS)).unwrap();
    }

    #[test]
    fn nothing_installed() {
        let (_tmp, etc, repo) = layout("none");
        assert!(check(&etc, &repo).unwrap().contains("not installed"));
    }

    #[test]
    fn copied_but_never_enabled() {
        let (_tmp, etc, repo) = layout("copied");
        copy_units(&etc, &repo);
        assert!(check(&etc, &repo).unwrap().contains("not installed"));
    }

    #[test]
    fn installed_and_identical() {
        let (_tmp, etc, repo) = layout("ok");
        copy_units(&etc, &repo);
        enable(&etc);
        assert_eq!(check(&etc, &repo), None);
    }

    #[test]
    fn edited_after_install() {
        let (_tmp, etc, repo) = layout("edited");
        copy_units(&etc, &repo);
        enable(&etc);
        fs::write(repo.join(UNITS[1]), "# changed\n").unwrap();
        assert!(check(&etc, &repo).unwrap().contains("differ"));
    }

    #[test]
    fn enabled_but_unit_file_gone() {
        let (_tmp, etc, repo) = layout("dangling");
        copy_units(&etc, &repo);
        enable(&etc);
        fs::remove_file(etc.join(UNITS[0])).unwrap();
        assert!(check(&etc, &repo).unwrap().contains("differ"));
    }

    #[test]
    fn no_repo_to_compare_against() {
        let (tmp, etc, _repo) = layout("moved");
        assert_eq!(check(&etc, &tmp.0.join("nowhere")), None);
    }
}
