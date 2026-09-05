//! Minimal driver logger, gated as a whole on [`file_log_enabled`] (debug builds, or the
//! `PFVD_DEBUG_LOG` env var): a RELEASE build without the opt-in emits NOTHING — the
//! `OutputDebugStringA` used to fire unconditionally, a syscall + CString + `format!` alloc per
//! logged event on paths that run per IOCTL/frame. The file tee (WUDFHost temp dir, not
//! world-writable — audit §4.4) rides the same gate. Best-effort; ignores all errors. Production
//! driver-state visibility is the AU header's `driver_status` word, not this module.

unsafe extern "system" {
    fn OutputDebugStringA(s: *const u8);
}

/// Whether driver logging (debug string + bring-up file) is enabled (resolved once). Off in release
/// builds unless the `PFVD_DEBUG_LOG` knob is set. `pub(crate)` so `dbglog!` can skip its
/// `format!` too.
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

pub fn log(s: &str) {
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
        let _ = writeln!(f, "{s}");
        let _ = f.flush();
    }
}

// The `file_log_enabled()` pre-check skips the `format!` alloc too when logging is off.
macro_rules! dbglog {
    ($($a:tt)*) => { if $crate::log::file_log_enabled() { $crate::log::log(&::std::format!($($a)*)) } };
}

/// Route the encoder backends' `tracing` events into [`log`], behind the same gate: with no
/// subscriber in WUDFHost every NVENC status string and AMF rejection is dropped, and a failed
/// open reaches the host as a bare stage tag. Call from `DriverEntry`, once.
pub(crate) fn install_tracing_bridge() {
    if !file_log_enabled() {
        return;
    }
    let _ = tracing::subscriber::set_global_default(Bridge);
}

struct Bridge;

impl tracing::Subscriber for Bridge {
    fn enabled(&self, m: &tracing::Metadata<'_>) -> bool {
        *m.level() <= tracing::Level::DEBUG
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = String::new();
        event.record(&mut Line(&mut line));
        let m = event.metadata();
        dbglog!("[pf-vd] {} {}:{line}", m.level(), m.target());
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
