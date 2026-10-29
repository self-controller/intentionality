//! The at-risk timer's screen half: a GNOME Shell extension
//! (gnome-extension/at-risk-timer@intentionality) that draws the clock.
//!
//! On GNOME/Wayland a client cannot place its own window or keep it above the
//! others -- only the compositor can draw over everything -- so this app
//! decides what to show and the shell shows it. They meet on the session bus,
//! at an object the extension exports on gnome-shell's own connection: the
//! pattern Focused Window D-Bus already uses, which is why the destination is
//! org.gnome.Shell.
//!
//! Since v2 the traffic runs both ways: the extension signals every change
//! of focused window or title (`Focus`), which is how the timer reacts in
//! milliseconds instead of waiting on ActivityWatch's one-second poll. Since
//! v3 the clock's arrow opens a box under it where the user argues the page's
//! case: each send is an `Appeal` signal with the whole case so far, and
//! Jev's verdict goes back as `Answer`.

use std::future::Future;
use std::time::Duration;
use tokio::sync::OnceCell;
use zbus::{proxy, proxy::CacheProperties, Connection};

/// The extension's `const VERSION`, which a test keeps equal to this. The
/// shell loads an extension's code once per login, so a mismatch means the
/// running copy is older than the repo's.
pub const VERSION: u32 = 3;

/// A hung gnome-shell must not stall the timer loop or the header's health
/// read; a healthy one answers in well under a millisecond.
const CALL_BUDGET: Duration = Duration::from_secs(1);

#[proxy(
    interface = "org.intentionality.Timer",
    default_service = "org.gnome.Shell",
    default_path = "/org/intentionality/Timer"
)]
// pub: the generated FocusStream / AppealStream types take this visibility,
// and screen_time holds one.
pub trait Timer {
    fn show(&self, label: &str, seconds: f64, caption: &str, appealable: bool) -> zbus::Result<u32>;
    fn hide(&self) -> zbus::Result<u32>;
    fn version(&self) -> zbus::Result<u32>;
    /// The window in front as the shell sees it: (window class, title). Both
    /// empty when nothing has focus. v2 only.
    fn focused(&self) -> zbus::Result<(String, String)>;
    #[zbus(signal)]
    fn focus(&self, app: &str, title: &str) -> zbus::Result<()>;
    /// The user's case for the window, every line so far. v3.
    #[zbus(signal)]
    fn appeal(&self, app: &str, title: &str, argument: Vec<String>) -> zbus::Result<()>;
    /// Jev's verdict on the case, in a line under the clock. v3.
    fn answer(&self, text: &str, won: bool) -> zbus::Result<u32>;
}

#[proxy(
    interface = "org.gnome.shell.extensions.FocusedWindow",
    default_service = "org.gnome.Shell",
    default_path = "/org/gnome/shell/extensions/FocusedWindow"
)]
trait FocusedWindow {
    fn get(&self) -> zbus::Result<String>;
}

static BUS: OnceCell<Connection> = OnceCell::const_new();
static TIMER: OnceCell<TimerProxy<'static>> = OnceCell::const_new();
static FOCUSED: OnceCell<FocusedWindowProxy<'static>> = OnceCell::const_new();

async fn bus() -> zbus::Result<&'static Connection> {
    BUS.get_or_try_init(Connection::session).await
}

// No property cache: neither interface has properties, and a cache would
// fetch them and subscribe to their changes for nothing.
async fn timer() -> zbus::Result<&'static TimerProxy<'static>> {
    TIMER
        .get_or_try_init(|| async {
            TimerProxy::builder(bus().await?).cache_properties(CacheProperties::No).build().await
        })
        .await
}

async fn focused_window() -> zbus::Result<&'static FocusedWindowProxy<'static>> {
    FOCUSED
        .get_or_try_init(|| async {
            FocusedWindowProxy::builder(bus().await?)
                .cache_properties(CacheProperties::No)
                .build()
                .await
        })
        .await
}

async fn call<T>(f: impl Future<Output = zbus::Result<T>>) -> zbus::Result<T> {
    tokio::time::timeout(CALL_BUDGET, f)
        .await
        .unwrap_or_else(|_| Err(zbus::Error::Failure("gnome-shell did not answer".into())))
}

/// Put the timer on screen, or keep it there: `seconds` is today's total for
/// `label` as of now, `caption` the line under the clock (empty for none) and
/// `appealable` whether to draw the arrow that opens the appeal. Sent every
/// tick, which doubles as the keepalive -- the extension hides the clock by
/// itself when these stop.
pub async fn show(label: &str, seconds: f64, caption: &str, appealable: bool) -> zbus::Result<u32> {
    call(async { timer().await?.show(label, seconds, caption, appealable).await }).await
}

pub async fn hide() -> zbus::Result<u32> {
    call(async { timer().await?.hide().await }).await
}

/// The focused window right now, from a v2 extension. An older one has no
/// such method, and that error is how the timer loop learns to poll instead.
pub async fn focused() -> zbus::Result<(String, String)> {
    call(async { timer().await?.focused().await }).await
}

/// Every change of focused window or of its title, as it happens.
pub async fn focus_stream() -> zbus::Result<FocusStream> {
    call(async { timer().await?.receive_focus().await }).await
}

/// Every case sent from the box under the clock.
pub async fn appeal_stream() -> zbus::Result<AppealStream> {
    call(async { timer().await?.receive_appeal().await }).await
}

/// The verdict on a case, shown under the clock. `won` closes the box.
pub async fn answer(text: &str, won: bool) -> zbus::Result<u32> {
    call(async { timer().await?.answer(text, won).await }).await
}

/// Why the timer cannot reach the screen, or None when it can.
pub async fn warning() -> Option<String> {
    match call(async { timer().await?.version().await }).await {
        Ok(v) if v == VERSION => None,
        Ok(v) if v < VERSION => Some("timer overlay is out of date — log out and back in".into()),
        Ok(_) => Some("timer overlay is newer than this build".into()),
        Err(_) => Some("timer overlay not installed — see README".into()),
    }
}

/// Set when ActivityWatch cannot see Wayland windows. awatcher reads the
/// focused window from the Focused Window D-Bus extension; with it off,
/// awatcher falls back to X11 and sees only XWayland apps, so every
/// observation goes quietly blind (it did, for two days, when another
/// extension's setup rewrote the enabled list).
pub async fn window_tracking_warning() -> Option<String> {
    call(async { focused_window().await?.get().await })
        .await
        .err()
        .map(|_| "window tracking is off — enable the Focused Window D-Bus extension".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extension's VERSION is what `warning` compares against, and the two
    /// live in different languages, so read it out of the source.
    #[test]
    fn the_extension_declares_the_version_this_app_expects() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../gnome-extension/at-risk-timer@intentionality/extension.js");
        let source = std::fs::read_to_string(path).unwrap();
        let declared: u32 = source
            .lines()
            .find_map(|l| l.trim().strip_prefix("const VERSION = "))
            .and_then(|rest| rest.trim_end_matches(';').trim().parse().ok())
            .expect("extension.js declares const VERSION = <n>;");
        assert_eq!(declared, VERSION);
    }
}
