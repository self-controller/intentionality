//! The one place that raises an OS notification.
//!
//! Absent by design until 2026-08-29 (see the note in Cargo.toml). Two callers
//! only: a scheduled analysis landing, and the time-up checkpoint. The manual
//! "Run a check now" button deliberately does not notify — you are already
//! looking at the tab that answers it.
//!
//! A notification that fails to send is a log line, never an error that
//! propagates: on Linux this is a D-Bus call to the desktop's notification
//! daemon, and a missing daemon must not take an analysis down with it.

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

pub fn send(app: &AppHandle, title: &str, body: &str) {
    if let Err(err) = app.notification().builder().title(title).body(body).show() {
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
