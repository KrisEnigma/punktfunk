//! The Windows-only halves of the host: the SCM service, the per-user tray, the installer
//! work (`driver`, `web`), the interactive-session spawn, the seat contract, and the process
//! entry hooks ([`entry`]). `main.rs` re-imports the flat modules so `crate::install::*`
//! keeps its historic path; `entry` is the one seam `main` calls through.

// The Windows-only devtests; the cross-platform ones stay in `crate::devtest`.
pub(crate) mod devtest;
pub(crate) mod entry;
// The IDD-push manager's session touch points; every other OS gets the no-op twin in `main.rs`.
pub(crate) mod idd;
// WM_CLOSE on the interactive desktop, then TerminateProcess.
pub(crate) mod game_term;
pub(crate) mod install;
pub(crate) mod interactive;
// What this host reads of the multi-seat contract; unset means the console host.
pub(crate) mod seat;
pub(crate) mod service;
// Per-user tray start/stop/status — the only recovery path after a crash or upgrade.
pub(crate) mod tray;

/// A secret the host did not write is not a credential. Only Windows can be pre-planted:
/// `%ProgramData%` grants Users create, while the Unix config dir is 0700 from birth.
pub(crate) mod planted {
    pub(crate) use crate::install::quarantine_planted_secret;
}
