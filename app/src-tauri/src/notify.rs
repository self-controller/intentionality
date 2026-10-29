//! The one place that raises an OS notification.
//!
//! Absent by design until 2026-08-29 (see the note in Cargo.toml). Two callers
//! only: a check landing on its own (scheduled, or early on drift), and the
//! time-up checkpoint. The manual "Run a check now" button deliberately does
//! not notify — you are already looking at the tab that answers it.
//!
//! A notification that fails to send is a log line, never an error that
//! propagates: on Linux this is a D-Bus call to the desktop's notification
//! daemon, and a missing daemon must not take an analysis down with it.
//!
//! Sent from a thread of its own. notify-rust blocks on its D-Bus call, and
//! with zbus's tokio feature that block_on panics on an async runtime worker
//! -- which is where every caller here runs. A plain thread has no runtime.

use tauri::AppHandle;

pub fn send(_app: &AppHandle, title: &str, body: &str) {
    let (title, body) = (title.to_string(), body.to_string());
    let sent = std::thread::Builder::new().name("notify".into()).spawn(move || {
        let shown = notify_rust::Notification::new()
            .summary(&title)
            .body(&body)
            .auto_icon()
            .show();
        if let Err(err) = shown {
            eprintln!("notification not shown: {err}");
        }
    });
    if let Err(err) = sent {
        eprintln!("notification not shown: {err}");
    }
}

/// Notification bodies come from model output, so they need a ceiling. GNOME
/// truncates long bodies itself, but at an unpredictable point; cutting on a
/// word boundary keeps the first sentence readable.
pub fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    match cut.rfind(' ') {
        Some(i) if i > max / 2 => format!("{}…", &cut[..i]),
        _ => format!("{cut}…"),
    }
}
