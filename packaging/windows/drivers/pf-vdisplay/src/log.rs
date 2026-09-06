//! Minimal driver logger. Every line lands in the ring the host drains over
//! [`IOCTL_DRAIN_LOG`](pf_driver_proto::control::IOCTL_DRAIN_LOG) — the encoder runs inside
//! WUDFHost, so that ring is how its backend rejections, retargets and wedges reach `host.log`.
//! The syscall sinks stay gated on [`file_log_enabled`] (debug builds, or the `PFVD_DEBUG_LOG`
//! env var): `OutputDebugStringA` traps into a global-serializing debugger call, and the file tee
//! (WUDFHost temp dir, not world-writable — audit §4.4) takes a knob plus a device restart to
//! reach. Best-effort; ignores all errors.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use pf_driver_proto::control;

unsafe extern "system" {
    fn OutputDebugStringA(s: *const u8);
}

/// Lines the host has not drained yet. Bounded, because a per-frame failure loop must cost a
/// fixed ceiling rather than the WUDFHost: the oldest goes and `dropped` counts it, so a flood
/// reads as a flood instead of as silence. 256 lines covers several sessions' worth of open and
/// retarget chatter at the pinger's ~3.3 s cadence.
const RING_LINES: usize = 256;

/// Longest line kept. Past this a record could outgrow the host's drain buffer and never leave
/// the ring; only an `{e:#}` chain comes close.
const MAX_LINE: usize = 512;

struct Ring {
    lines: VecDeque<(u8, String)>,
    dropped: u64,
}

fn ring() -> &'static Mutex<Ring> {
    static R: OnceLock<Mutex<Ring>> = OnceLock::new();
    R.get_or_init(|| {
        Mutex::new(Ring {
            lines: VecDeque::new(),
            dropped: 0,
        })
    })
}

/// Queue one line for the host, dropping the oldest when full.
fn push(level: u8, line: &str) {
    let end = (0..=MAX_LINE.min(line.len()))
        .rev()
        .find(|&i| line.is_char_boundary(i))
        .unwrap_or(0);
    let Ok(mut r) = ring().lock() else { return };
    if r.lines.len() >= RING_LINES {
        r.lines.pop_front();
        r.dropped += 1;
    }
    r.lines.push_back((level, line[..end].to_string()));
}

/// Answer `IOCTL_DRAIN_LOG`: whole records, oldest first, up to `cap` bytes. What does not fit
/// stays queued for the next call — except a record that alone exceeds `cap`, which is dropped
/// rather than left to wedge every line behind it.
pub fn drain(cap: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let Ok(mut r) = ring().lock() else { return out };
    if r.dropped > 0 {
        let n = std::mem::take(&mut r.dropped);
        let line = format!("[pf-vd] log ring overflowed — {n} lines dropped");
        control::write_log_record(&mut out, control::LOG_WARN, &line);
    }
    while let Some((level, line)) = r.lines.pop_front() {
        let start = out.len();
        control::write_log_record(&mut out, level, &line);
        if out.len() > cap {
            out.truncate(start);
            if start == 0 {
                r.dropped += 1;
            } else {
                r.lines.push_front((level, line));
            }
            break;
        }
    }
    out
}

/// Whether the syscall sinks (debug string + bring-up file) are enabled (resolved once). Off in
/// release builds unless the `PFVD_DEBUG_LOG` knob is set. The host's drain ring does not ride
/// this gate; only these two do, and so does whether `DEBUG` events are kept at all.
pub(crate) fn file_log_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| cfg!(debug_assertions) || knob("PFVD_DEBUG_LOG").is_some())
}

/// A driver knob: the process environment first, then the MACHINE environment in the registry
/// (where `setx /M` writes). WUDFHost inherits its environment from the SCM at boot and the SCM
/// never refreshes it, so a `setx /M` set today is invisible to `std::env` until a reboot; the
/// registry read makes a device restart enough.
pub(crate) fn knob(name: &str) -> Option<String> {
    std::env::var(name).ok().or_else(|| machine_env(name))
}

/// Read a MACHINE environment variable from the registry (see [`knob`]).
fn machine_env(name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{HSTRING, PCWSTR};
    const KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
    let (subkey, value) = (HSTRING::from(KEY), HSTRING::from(name));
    let mut buf = [0u16; 256];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: both name pointers address NUL-terminated HSTRING buffers alive for the call;
    // `buf`/`size` are a matched out-buffer and its byte length. RRF_RT_REG_SZ makes the call
    // reject any non-string value rather than write a foreign type into the buffer.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if rc.is_err() {
        return None;
    }
    // `size` is bytes INCLUDING the terminator; trim to chars and drop trailing NULs.
    let chars = (size as usize / 2).min(buf.len());
    Some(
        String::from_utf16_lossy(&buf[..chars])
            .trim_end_matches('\0')
            .to_string(),
    )
}

/// Process-lifetime append handle to the bring-up log, opened ONCE (by whichever thread logs first) and
/// shared via a `Mutex` — so the swap-chain WORKER thread's writes land too. Per-call open/append raced
/// the control thread and/or could fail under the worker's restricted token, hiding exactly the
/// swap-chain-processor lines a game-break repro needs (game-capture bug S3). `flush` after each line so a
/// crash/stall doesn't lose the tail.
fn file_appender() -> Option<&'static std::sync::Mutex<std::fs::File>> {
    use std::sync::OnceLock;
    static APPENDER: OnceLock<Option<std::sync::Mutex<std::fs::File>>> = OnceLock::new();
    APPENDER
        .get_or_init(|| {
            if !file_log_enabled() {
                return None;
            }
            // WUDFHost's own (LocalService) temp dir — NOT world-writable/readable `C:\Users\Public`,
            // where a non-admin could pre-create/hold the file or read the diagnostics
            // (security-review 2026-07-17). Opt-in/debug only.
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::env::temp_dir().join("pfvd-driver.log"))
                .ok()
                .map(std::sync::Mutex::new)
        })
        .as_ref()
}

/// One line at `INFO`; see [`log_at`].
pub fn log(s: &str) {
    log_at(control::LOG_INFO, s);
}

/// Queue one line for the host and, when [`file_log_enabled`], tee it to the debugger and the
/// bring-up file.
pub(crate) fn log_at(level: u8, s: &str) {
    push(level, s);
    if !file_log_enabled() {
        return;
    }
    if let Ok(c) = std::ffi::CString::new(s) {
        // SAFETY: `c` is a valid NUL-terminated string for the duration of the call.
        unsafe { OutputDebugStringA(c.as_ptr().cast()) };
    }
    use std::io::Write;
    if let Some(m) = file_appender()
        && let Ok(mut f) = m.lock()
    {
        let _ = writeln!(f, "{} {s}", utc_hms_millis());
        let _ = f.flush();
    }
}

/// `HH:MM:SS.mmm` UTC for the file line. The host logs RFC3339 UTC, and without a shared clock
/// on both sides a driver line cannot be placed against the host event it explains — which is
/// the whole question when frames stop. Date-free: same-day alignment is what a session needs.
fn utc_hms_millis() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
        now.subsec_millis()
    )
}

// Always formats: the line is the host's only view of this process. One `String` per event costs
// nothing beside the encode it describes, and the ring bounds what a failure loop can hold.
macro_rules! dbglog {
    ($($a:tt)*) => { $crate::log::log(&::std::format!($($a)*)) };
}

/// Route the encoder backends' `tracing` events into [`log_at`]: with no subscriber in WUDFHost
/// every NVENC status string and AMF rejection is dropped, and a failed open reaches the host as
/// a bare stage tag. Call from `DriverEntry`, once.
pub(crate) fn install_tracing_bridge() {
    let _ = tracing::subscriber::set_global_default(Bridge);
}

struct Bridge;

impl tracing::Subscriber for Bridge {
    /// `DEBUG` only under the local sinks — a backend may emit one per submitted frame, and the
    /// host's ring is for the session story, not a per-frame trace.
    fn enabled(&self, m: &tracing::Metadata<'_>) -> bool {
        *m.level()
            <= if file_log_enabled() {
                tracing::Level::DEBUG
            } else {
                tracing::Level::INFO
            }
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    /// The severity travels as the record's byte, so the host re-emits at the level the backend
    /// chose instead of flattening every encoder warning into an info line.
    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = String::new();
        event.record(&mut Line(&mut line));
        let m = event.metadata();
        let level = match *m.level() {
            tracing::Level::ERROR => control::LOG_ERROR,
            tracing::Level::WARN => control::LOG_WARN,
            tracing::Level::INFO => control::LOG_INFO,
            _ => control::LOG_DEBUG,
        };
        log_at(level, &format!("[pf-vd] {}:{line}", m.target()));
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// `message` first as written, every other field as `name=value`.
struct Line<'a>(&'a mut String);

impl tracing::field::Visit for Line<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn core::fmt::Debug) {
        use core::fmt::Write;
        let _ = if field.name() == "message" {
            write!(self.0, " {value:?}")
        } else {
            write!(self.0, " {}={value:?}", field.name())
        };
    }
}

/// Zero-initialise a C POD struct (windows-rs / WDK / IddCx). These are `#[repr(C)]` framework structs
/// whose all-zero bit pattern is a valid zero-initialised value; the caller stamps the required
/// `.Size`/etc fields immediately after. Centralises the `unsafe { core::mem::zeroed() }` the IddCx/WDF
/// bring-up needs — pass the type EXPLICITLY (`pod_init!(T)`) so it works without a binding annotation.
/// Made crate-visible by the same `#[macro_use] mod log;` in `lib.rs` that exports `dbglog!`.
macro_rules! pod_init {
    ($t:ty) => {{
        // SAFETY: $t is a C POD (windows-rs/WDK/IddCx struct); its all-zero bit pattern is a valid
        // zero-initialised value and the caller sets the required .Size/etc fields immediately after.
        // `unused_unsafe`: pod_init! is also expanded at call sites already inside an `unsafe` block
        // (where this `unsafe` is redundant), but it IS required at the non-unsafe sites — so allow it.
        #[allow(unused_unsafe)]
        let zeroed = unsafe { ::core::mem::zeroed::<$t>() };
        zeroed
    }};
}
