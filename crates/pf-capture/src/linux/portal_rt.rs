//! Process-lifetime tokio runtime for every portal handshake.
//!
//! ashpd caches its D-Bus connection in a process-global `OnceLock`. The first
//! portal proxy creates it, and zbus spawns the connection's reader on
//! whichever tokio runtime is current at that moment.
//!
//! A per-session runtime that is dropped at teardown leaves that cached
//! connection with no executor. Every later portal call in the process then
//! waits for a reply nothing is left alive to read.
//!
//! Never build a per-session runtime, and never drop this one. `block_on`
//! takes `&self`, so every portal thread can park on it concurrently. A portal
//! session made here outlives the thread that made it: close it explicitly.

use ashpd::desktop::screencast::{CursorMode, Screencast};
use ashpd::enumflags2::BitFlags;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::runtime::Runtime;

/// `Result` so a failed build fails the handshake with a reason instead of aborting the process.
static PORTAL_RT: OnceLock<std::io::Result<Runtime>> = OnceLock::new();

/// Multi-thread, 2 workers: the zbus reader must run across `create_session`
/// → `select_sources` → `start` while a portal thread blocks on `block_on`.
/// A current-thread runtime cannot pump that.
pub fn portal_runtime() -> Result<&'static Runtime, String> {
    match PORTAL_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("punktfunk-portal-rt")
            .enable_all()
            .build()
    }) {
        Ok(rt) => Ok(rt),
        Err(e) => Err(format!("build the shared portal runtime: {e}")),
    }
}

/// `AvailableCursorModes`, re-read while it is empty.
///
/// A portal that the ScreenCast call itself D-Bus-activated publishes `0`
/// until its backend answers. xdg-desktop-portal validates `SelectSources`
/// against this same property, so the settled value is the one that counts.
/// Returns whatever it reads last, empty included, after 2 s.
pub async fn available_cursor_modes(proxy: &Screencast) -> ashpd::Result<BitFlags<CursorMode>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let avail = proxy.available_cursor_modes().await?;
        if !avail.is_empty() || tokio::time::Instant::now() >= deadline {
            return Ok(avail);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
