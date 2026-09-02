//! The `pf-driver-proto` control plane (`EvtIddCxDeviceIoControl`). The host opens the device
//! interface (`PF_VDISPLAY_INTERFACE_GUID`) and drives the low-frequency IOCTLs: GET_INFO (version
//! handshake), PING (watchdog keepalive), ADD/REMOVE/CLEAR_ALL (virtual monitors),
//! SET_RENDER_ADAPTER, UPDATE_MODES, and the frame/cursor channel deliveries.
//!
//! [`dispatch`] wraps the raw `WDFREQUEST` in a [`Request`] token once and hands it to a handler BY
//! VALUE; completing consumes the token, so "every path completes exactly once" (the
//! `EVT_IDD_CX_DEVICE_IO_CONTROL` shape returns `()`, leaving the framework no status to act on) is
//! a type-level fact. Buffer I/O rides the token's methods over `bytemuck` casts of the Pod wire
//! structs — which leaves the control plane one `unsafe` block, the token construction.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytemuck::Pod;
use pf_driver_proto::control;
use pf_umdf_util::wdf::Request;
use wdk_sys::WDFREQUEST;

use crate::{STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_PARAMETER, STATUS_NOT_FOUND, STATUS_SUCCESS};

/// The host must send an IOCTL within this window (it PINGs on a `timeout/3` timer) or the watchdog
/// treats it as gone and reaps every monitor. Reported to the host via [`control::IOCTL_GET_INFO`].
const WATCHDOG_TIMEOUT_S: u32 = 10;

/// Host-liveness counter — EVERY inbound IOCTL bumps it; [`start_watchdog`]'s thread samples it.
static WATCHDOG_PINGS: AtomicU64 = AtomicU64::new(0);
/// Spawns the watchdog thread exactly once (idempotent across re-entrant adapter inits).
static WATCHDOG_STARTED: AtomicBool = AtomicBool::new(false);
/// Asks the watchdog thread to exit ([`stop_watchdog`], from device cleanup). The thread consumes
/// the flag (swap) and clears [`WATCHDOG_STARTED`] on the way out, so a later adapter init on a
/// fresh device can re-arm.
static WATCHDOG_STOP: AtomicBool = AtomicBool::new(false);

/// Start the host-liveness watchdog (once, from `adapter_init_finished`).
///
/// Previously [`WATCHDOG_PINGS`] was bumped but NEVER sampled (no thread existed) — so a host that died
/// without a cooperative REMOVE (crash / `TerminateProcess`) left its virtual monitor + swap-chain
/// worker + pooled D3D device wedged in WUDFHost until the next host start's CLEAR_ALL, and a
/// not-restarted host left the orphan monitor in the desktop topology indefinitely
/// (`design/windows-host-rewrite.md` §2.8). This thread closes that: if no IOCTL arrives for
/// `WATCHDOG_TIMEOUT_S` while monitors exist, it departs them all.
///
/// (A WDF `EvtFileClose` on the control handle would be more immediate — the plan's preferred §3.4
/// option — but the polling watchdog matches the proven oracle and needs no IddCx file-object plumbing.)
pub fn start_watchdog() {
    if WATCHDOG_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let tick = Duration::from_secs(u64::from((WATCHDOG_TIMEOUT_S / 3).max(1)));
    let timeout = Duration::from_secs(u64::from(WATCHDOG_TIMEOUT_S));
    std::thread::spawn(move || {
        let mut last = WATCHDOG_PINGS.load(Ordering::Relaxed);
        let mut last_change = Instant::now();
        loop {
            std::thread::sleep(tick);
            // Device cleanup asked us to stop: the WDFDEVICE (and with it every monitor) is going
            // away — a reap fired after that point would race `cleanup_for_device_removal` over
            // the same monitor list. Consume the flag and un-mark STARTED so a fresh device's
            // adapter init can re-arm. (Previously this thread ran forever: it outlived the
            // device and was reaped only with the WUDFHost process.)
            if WATCHDOG_STOP.swap(false, Ordering::SeqCst) {
                WATCHDOG_STARTED.store(false, Ordering::SeqCst);
                dbglog!("[pf-vd] watchdog: device cleanup — thread exiting");
                return;
            }
            let cur = WATCHDOG_PINGS.load(Ordering::Relaxed);
            if cur != last {
                last = cur;
                last_change = Instant::now();
                continue;
            }
            // No IOCTL since `last_change`. A live host PINGs every `timeout/3`, so this only trips once
            // the host is truly gone; only reap when there's something to reap.
            if last_change.elapsed() >= timeout && crate::monitor::has_monitors() {
                let n = crate::monitor::reap_orphaned(Duration::from_secs(3));
                if n > 0 {
                    dbglog!(
                        "[pf-vd] watchdog: no host IOCTL in {WATCHDOG_TIMEOUT_S}s — host gone, departed {n} monitor(s)"
                    );
                }
                last_change = Instant::now(); // don't re-reap every tick
            }
        }
    });
}

/// Ask the watchdog thread to exit (device cleanup). Takes effect within one tick (~3 s); the
/// narrow window where a cleanup-then-re-add lands between the flag and the thread noticing it
/// is unreachable in practice — `ProcessSharingDisabled` gives each device its own WUDFHost, so
/// a new device means a new process with fresh statics.
pub fn stop_watchdog() {
    if WATCHDOG_STARTED.load(Ordering::SeqCst) {
        WATCHDOG_STOP.store(true, Ordering::SeqCst);
    }
}

/// Dispatch one control IOCTL and complete the request.
///
/// # Safety
/// `request` is the framework-provided `WDFREQUEST` for an `EvtIddCxDeviceIoControl` call.
pub unsafe fn dispatch(request: WDFREQUEST, ioctl_code: u32) {
    // Every inbound IOCTL is host liveness (the host PINGs on a timer, plus ADD/REMOVE/GET_INFO/…) —
    // bump the watchdog at the top so it only fires once the host has gone truly silent. See
    // [`start_watchdog`].
    WATCHDOG_PINGS.fetch_add(1, Ordering::Relaxed);
    // SAFETY: `request` is the live request for THIS EvtIddCxDeviceIoControl invocation — exactly
    // the contract `Request::new` requires. Everything below is safe: the token owns completion.
    let request = unsafe { Request::new(request) };
    match ioctl_code {
        control::IOCTL_GET_INFO => {
            let reply = control::InfoReply {
                protocol_version: pf_driver_proto::PROTOCOL_VERSION,
                watchdog_timeout_s: WATCHDOG_TIMEOUT_S,
            };
            write_output_prefix_complete(request, &reply, size_of::<control::InfoReply>());
        }
        control::IOCTL_PING => request.complete(STATUS_SUCCESS),
        control::IOCTL_ADD => add(request),
        control::IOCTL_REMOVE => remove(request),
        control::IOCTL_CLEAR_ALL => {
            crate::monitor::clear_all();
            request.complete(STATUS_SUCCESS);
        }
        control::IOCTL_SET_RENDER_ADAPTER => set_render_adapter(request),
        control::IOCTL_SET_FRAME_CHANNEL => set_frame_channel(request),
        control::IOCTL_UPDATE_MODES => update_modes(request),
        control::IOCTL_SET_CURSOR_CHANNEL => set_cursor_channel(request),
        control::IOCTL_SET_CURSOR_FORWARD => set_cursor_forward(request),
        _ => request.complete(STATUS_NOT_FOUND),
    }
}

/// Sanity bounds for a requested mode — generous (covers any real client) but rejects zero/absurd
/// values that would otherwise feed the EDID/mode math unchecked.
fn valid_mode(width: u32, height: u32, refresh_hz: u32) -> bool {
    (1..=16384).contains(&width)
        && (1..=16384).contains(&height)
        && (1..=1000).contains(&refresh_hz)
}

/// `IOCTL_SET_RENDER_ADAPTER`: pin the IddCx render adapter (hybrid-GPU IDD-push).
fn set_render_adapter(request: Request) {
    let Some(req) = read_input::<control::SetRenderAdapterRequest>(&request) else {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    };
    let st = crate::adapter::set_render_adapter(req.luid_low, req.luid_high);
    request.complete(st);
}

/// `IOCTL_ADD`: create a virtual monitor at the requested mode → reply with the OS target id + LUID.
fn add(request: Request) {
    let Some(req) = read_add_request(&request) else {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    };
    if !valid_mode(req.width, req.height, req.refresh_hz) {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    }
    let Some((monitor_id, target_id, luid_low, luid_high)) = crate::monitor::create_monitor(
        req.session_id,
        req.width,
        req.height,
        req.refresh_hz,
        req.preferred_monitor_id,
        crate::edid::ClientLuminance {
            max_nits: req.max_luminance_nits,
            max_frame_avg_nits: req.max_frame_avg_nits,
            min_millinits: req.min_luminance_millinits,
        },
        req.hw_cursor != 0,
    ) else {
        request.complete(STATUS_NOT_FOUND);
        return;
    };
    let reply = control::AddReply {
        adapter_luid_low: luid_low,
        adapter_luid_high: luid_high,
        target_id,
        resolved_monitor_id: monitor_id,
        // This WUDFHost's pid — where the host duplicates the sealed frame channel's handles INTO
        // (`ProcessSharingDisabled`: this process is exclusively ours and dies with the device).
        wudf_pid: std::process::id(),
        // An irrevocable hardware-cursor declare from an EARLIER session excludes the pointer
        // ADAPTER-wide, not just on the declaring target, so a channel-less session on this
        // adapter must self-composite the pointer (§8.6 gap).
        cursor_excluded: crate::monitor::any_declared() as u32,
    };
    // Dual-size reply (the `cursor_excluded` tail ext): an un-upgraded host retrieves only the
    // legacy 20-byte buffer — write the prefix it asked for instead of failing its ADD.
    write_output_prefix_complete(request, &reply, control::ADD_REPLY_LEGACY_SIZE);
}

/// `IOCTL_SET_CURSOR_CHANNEL` (v5): adopt a monitor's hardware-cursor section, declare the
/// hardware cursor to the OS, start the query→publish worker.
fn set_cursor_channel(request: Request) {
    let Some(req) = read_input::<control::SetCursorChannelRequest>(&request) else {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    };
    let Some(ch) = crate::cursor_worker::CursorChannel::from_request(&req) else {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    };
    match crate::monitor::set_cursor_channel(req.target_id, ch) {
        Ok(()) => request.complete(STATUS_SUCCESS),
        Err(ch) => {
            dbglog!(
                "[pf-vd] SET_CURSOR_CHANNEL: no hw-cursor monitor with target_id {} — rejecting",
                req.target_id
            );
            // NOT adopted: the host's error path reaps the duplicated handle remotely.
            ch.into_unowned();
            request.complete(STATUS_NOT_FOUND);
        }
    }
}

/// `IOCTL_SET_CURSOR_FORWARD` (v6): the mid-stream cursor-render flip — (un)declare a LIVE
/// monitor's hardware cursor as the client's mouse model demands.
fn set_cursor_forward(request: Request) {
    let Some(req) = read_input::<control::SetCursorForwardRequest>(&request) else {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    };
    if crate::monitor::set_cursor_forward(req.target_id, req.enable != 0) {
        request.complete(STATUS_SUCCESS);
    } else {
        dbglog!(
            "[pf-vd] SET_CURSOR_FORWARD: no cursor-channel monitor with target_id {} — rejecting",
            req.target_id
        );
        request.complete(STATUS_NOT_FOUND);
    }
}

/// `IOCTL_SET_FRAME_CHANNEL`: adopt the handle values the host duplicated into this process and
/// stash them on the target monitor for the swap-chain worker to attach with. The ownership
/// contract with the host is **adopt-on-success only**: this driver owns (and eventually closes)
/// the handles iff the IOCTL completes successfully; on ANY error completion it leaves them
/// untouched, because the host reaps its remote duplicates whenever the IOCTL fails — a close on
/// both sides would double-close values the OS may already have reused for unrelated handles.
fn set_frame_channel(request: Request) {
    // The v2 request (two shared-fence handles behind the v1 prefix) is told apart by input
    // LENGTH; a v1-sized buffer fails the v2 read and takes the v1 path. Either way a malformed
    // request adopts nothing (no FrameChannel is built, so no Drop can close anything).
    let (target_id, ch) =
        if let Some(req) = read_input::<control::SetFrameChannelRequestV2>(&request) {
            (
                req.v1.target_id,
                crate::frame_transport::FrameChannel::from_request_v2(&req),
            )
        } else if let Some(req) = read_input::<control::SetFrameChannelRequest>(&request) {
            (
                req.target_id,
                crate::frame_transport::FrameChannel::from_request(&req),
            )
        } else {
            request.complete(STATUS_INVALID_PARAMETER);
            return;
        };
    let Some(ch) = ch else {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    };
    match crate::monitor::set_frame_channel(target_id, ch) {
        Ok(()) => request.complete(STATUS_SUCCESS),
        Err(ch) => {
            dbglog!(
                "[pf-vd] SET_FRAME_CHANNEL: no monitor with target_id {} — rejecting (host reaps the handles)",
                target_id
            );
            // NOT adopted: disarm the channel so its Drop does NOT close the handles (see the contract
            // above — the host's error path reaps them remotely).
            ch.into_unowned();
            request.complete(STATUS_NOT_FOUND);
        }
    }
}

/// `IOCTL_UPDATE_MODES` (v4): refresh a LIVE monitor's target-mode list to a new preferred mode —
/// the in-place mid-stream resize (`design/first-frame-and-resize-latency.md` P2). The monitor is
/// NOT departed: its OS identity, swap-chain machinery and retained frame stash all survive; the
/// host force-sets the freshly-advertised mode afterwards.
fn update_modes(request: Request) {
    let Some(req) = read_input::<control::UpdateModesRequest>(&request) else {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    };
    if !valid_mode(req.width, req.height, req.refresh_hz) {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    }
    let st =
        crate::monitor::update_monitor_modes(req.session_id, req.width, req.height, req.refresh_hz);
    request.complete(st);
}

/// `IOCTL_REMOVE`: depart + drop the monitor for the given session id.
fn remove(request: Request) {
    let Some(req) = read_input::<control::RemoveRequest>(&request) else {
        request.complete(STATUS_INVALID_PARAMETER);
        return;
    };
    crate::monitor::remove_monitor(req.session_id);
    request.complete(STATUS_SUCCESS);
}

/// Read an [`control::AddRequest`], accepting BOTH wire sizes: the full struct, or an un-upgraded
/// host's [`ADD_REQUEST_LEGACY_SIZE`](control::ADD_REQUEST_LEGACY_SIZE)-byte prefix (no client-HDR
/// luminance tail), whose missing tail zero-fills to "unknown" — so a new driver keeps serving an
/// old host (see the `AddRequest` size-compatibility docs).
fn read_add_request(request: &Request) -> Option<control::AddRequest> {
    const FULL: usize = size_of::<control::AddRequest>();
    let (bytes, _) = request.input_bytes(FULL).ok()?;
    if bytes.len() < control::ADD_REQUEST_LEGACY_SIZE {
        return None;
    }
    // Zero fill = the Zeroable contract's "unknown" for every field past what the host sent.
    let mut buf = [0u8; FULL];
    buf[..bytes.len()].copy_from_slice(&bytes);
    Some(bytemuck::pod_read_unaligned(&buf))
}

/// Read a Pod input struct from the request's input buffer. `None` if the host sent fewer bytes
/// than the struct (or no input buffer at all) — every caller answers that with
/// `STATUS_INVALID_PARAMETER`.
fn read_input<T: Pod>(request: &Request) -> Option<T> {
    // `input_bytes` caps its copy at `size_of::<T>()`, so a full-length result IS the "host sent at
    // least the whole struct" check — and it keeps `pod_read_unaligned` off its panic path.
    let (bytes, _) = request.input_bytes(size_of::<T>()).ok()?;
    (bytes.len() == size_of::<T>()).then(|| bytemuck::pod_read_unaligned(&bytes))
}

/// Copy a Pod reply into the output buffer and complete with the byte count.
///
/// `min_size` is the SHORTEST reply the caller will serve; anything from there up to
/// `size_of::<T>()` is written as a prefix of the struct. That is the dual-size discipline behind
/// [`control::AddReply`]'s appended `cursor_excluded`: a host that retrieved only the legacy prefix
/// gets exactly that prefix instead of a failed IOCTL. A buffer shorter than `min_size` cannot
/// carry a usable reply and completes `STATUS_BUFFER_TOO_SMALL`.
fn write_output_prefix_complete<T: Pod>(request: Request, value: &T, min_size: usize) {
    let out_len = request.output_buffer_len();
    if out_len < min_size {
        request.complete(STATUS_BUFFER_TOO_SMALL);
        return;
    }
    let take = out_len.min(size_of::<T>());
    let st = request.copy_to_output(&bytemuck::bytes_of(value)[..take]);
    request.complete(st);
}
