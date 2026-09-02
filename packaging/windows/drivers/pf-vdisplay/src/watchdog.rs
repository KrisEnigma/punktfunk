//! Host-gone watchdog — a periodic WDF timer parented to the WDFDEVICE.
//!
//! Every inbound control IOCTL is host liveness ([`ping`]). A host that dies without a cooperative
//! REMOVE (crash / `TerminateProcess`) stops pinging, and its virtual monitor + swap-chain worker +
//! pooled D3D device would stay wedged in WUDFHost, the orphan sitting in the desktop topology
//! until some later host sends CLEAR_ALL (`design/windows-host-rewrite.md` §2.8). After
//! [`WATCHDOG_TIMEOUT_S`] of silence the tick departs every monitor.
//!
//! The timer is a child of the device, so the framework deletes it with the device instead of a
//! thread outliving the monitors it reaps, and [`stop`] waits out a tick that is mid-reap before
//! cleanup walks the same monitor list.
//!
//! (A WDF `EvtFileClose` on the control handle would be more immediate — the plan's preferred §3.4
//! option — but polling needs no IddCx file-object plumbing.)

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use wdk_iddcx::nt_success;
use wdk_sys::{
    NTSTATUS, ULONG, WDF_OBJECT_ATTRIBUTES, WDF_TIMER_CONFIG, WDFDEVICE, WDFTIMER,
    call_unsafe_wdf_function_binding,
};

/// The host must send an IOCTL within this window (it PINGs on a `timeout/3` timer) or the watchdog
/// treats it as gone and reaps every monitor. Reported to the host via `IOCTL_GET_INFO`.
pub const WATCHDOG_TIMEOUT_S: u32 = 10;

/// Tick period — the host's PING cadence (`timeout/3`), so one dropped PING never trips a reap.
const TICK_MS: u32 = 3000;
/// Silent ticks before a reap, rounded UP so the reap never fires short of the timeout.
const TICKS_TO_REAP: u32 = (1000 * WATCHDOG_TIMEOUT_S).div_ceil(TICK_MS);

/// Host-liveness counter: every inbound IOCTL bumps it, [`evt_timer`] samples it.
static PINGS: AtomicU64 = AtomicU64::new(0);
/// [`evt_timer`]'s own state — the previous tick's sample, and how many ticks in a row have seen it
/// unchanged. Only the tick writes them, and only one timer is ever armed.
static LAST_PINGS: AtomicU64 = AtomicU64::new(0);
static SILENT_TICKS: AtomicU32 = AtomicU32::new(0);

/// The timer handle, from [`create`] until [`stop`] hands the device back to the framework.
struct SendTimer(WDFTIMER);
// SAFETY: an opaque WDF handle, only ever passed by value to WDF DDIs (themselves the
// synchronisation point) and never dereferenced in Rust, so sharing it across threads is sound.
unsafe impl Send for SendTimer {}
static TIMER: Mutex<Option<SendTimer>> = Mutex::new(None);

/// Create the watchdog timer, stopped, as a child of `device`. Called from `driver_add` so the
/// handle exists before any IOCTL can arrive; [`start`] arms it once the adapter is up. Parenting
/// is what bounds the timer's life to the device's: a re-init gets a fresh timer, and no tick can
/// run after the framework has deleted the device.
pub fn create(device: WDFDEVICE) -> NTSTATUS {
    let mut cfg = pod_init!(WDF_TIMER_CONFIG);
    cfg.Size = core::mem::size_of::<WDF_TIMER_CONFIG>() as ULONG;
    cfg.EvtTimerFunc = Some(evt_timer);
    cfg.Period = TICK_MS;
    // AutomaticSerialization stays FALSE (the zeroed default): the tick reaps monitors, which joins
    // the swap-chain workers, and serializing that against the device's callbacks would park that
    // join in front of them.
    let mut attr = pod_init!(WDF_OBJECT_ATTRIBUTES);
    attr.Size = core::mem::size_of::<WDF_OBJECT_ATTRIBUTES>() as ULONG;
    attr.ParentObject = device.cast();
    // Zeroed leaves these at 0 (Invalid) → set them like WDF_OBJECT_ATTRIBUTES_INIT.
    attr.ExecutionLevel = wdk_sys::_WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent;
    attr.SynchronizationScope =
        wdk_sys::_WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent;
    let mut timer: WDFTIMER = core::ptr::null_mut();
    // SAFETY: cfg + attr are fully initialised locals; `timer` receives the created handle.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(WdfTimerCreate, &mut cfg, &mut attr, &mut timer)
    };
    dbglog!("[pf-vd] watchdog WdfTimerCreate -> {status:#x}");
    if nt_success(status) {
        *TIMER.lock().unwrap_or_else(PoisonError::into_inner) = Some(SendTimer(timer));
    }
    status
}

/// Arm the watchdog from `adapter_init_finished` — the point where monitors become reachable.
/// Idempotent across re-entrant adapter inits: re-arming a queued timer only re-bases its due time,
/// and the sample is reset so the first tick after a re-arm never counts as silence.
pub fn start() {
    let timer = TIMER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|t| t.0);
    let Some(timer) = timer else { return };
    LAST_PINGS.store(PINGS.load(Ordering::Relaxed), Ordering::Relaxed);
    SILENT_TICKS.store(0, Ordering::Relaxed);
    // SAFETY: `timer` is the live device-parented handle from `create`; a negative due time is
    // relative (100 ns units), so the first tick lands one period out.
    let _armed = unsafe {
        call_unsafe_wdf_function_binding!(WdfTimerStart, timer, -(i64::from(TICK_MS) * 10_000))
    };
}

/// Disarm the watchdog from device cleanup: the device — and with it every monitor — is going away,
/// and a reap running into `cleanup_for_device_removal` would fight it over the same monitor list.
/// Takes the handle, so the slot never holds one the framework has since deleted.
pub fn stop() {
    let taken = TIMER.lock().unwrap_or_else(PoisonError::into_inner).take();
    let Some(SendTimer(timer)) = taken else {
        return;
    };
    // SAFETY: `timer` is still live — the device's EvtCleanup runs before the framework deletes its
    // children. `1` is Wait=TRUE, so a tick that is mid-reap has returned by the time this does.
    let _was_queued = unsafe { call_unsafe_wdf_function_binding!(WdfTimerStop, timer, 1) };
    dbglog!("[pf-vd] watchdog: device cleanup — timer stopped");
}

/// Record host liveness. Called for EVERY inbound IOCTL, not just PING: an ADD/REMOVE/GET_INFO
/// proves the host is alive just as well.
pub fn ping() {
    PINGS.fetch_add(1, Ordering::Relaxed);
}

/// One tick: depart every monitor once the ping counter has stood still for [`WATCHDOG_TIMEOUT_S`].
///
/// A live host PINGs every `timeout/3`, so the counter only stalls this long when the host is truly
/// gone. The reap runs here on a framework thread and joins the swap-chain workers, which is why
/// cleanup stops the timer with Wait=TRUE.
unsafe extern "C" fn evt_timer(_timer: WDFTIMER) {
    let pings = PINGS.load(Ordering::Relaxed);
    if LAST_PINGS.swap(pings, Ordering::Relaxed) != pings {
        SILENT_TICKS.store(0, Ordering::Relaxed);
        return;
    }
    // Only reap when there is something to reap.
    if SILENT_TICKS.fetch_add(1, Ordering::Relaxed) + 1 < TICKS_TO_REAP
        || !crate::monitor::has_monitors()
    {
        return;
    }
    SILENT_TICKS.store(0, Ordering::Relaxed); // don't re-reap every tick
    let n = crate::monitor::reap_orphaned(Duration::from_secs(3));
    if n > 0 {
        dbglog!(
            "[pf-vd] watchdog: no host IOCTL in {WATCHDOG_TIMEOUT_S}s — host gone, departed {n} monitor(s)"
        );
    }
}
