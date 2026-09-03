//! Host-side IDD-push capture: the driver encodes, so this side owns no pixels.
//!
//! The driver's swap-chain worker passes each composed frame through its encode
//! pool into a sealed AU section ([`driver_encode`]); this capturer owns the
//! session's display identity — mode, HDR, cursor channel, health — and reports
//! the driver's frame cadence as pixel-less [`CapturedFrame`]s so the stream loop
//! keeps its geometry and pacing. Driver:
//! `packaging/windows/drivers/pf-vdisplay/src/encode/`. Layout and status codes
//! live in [`pf_driver_proto`] — both sides `use` it, so drift is a compile error.

use super::dxgi::WinCaptureTarget;
use super::{CapturedFrame, Capturer, FramePayload, PixelFormat};
use anyhow::{bail, Context, Result};
use pf_driver_proto::encode;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    DuplicateHandle, LocalFree, DUPLICATE_CLOSE_SOURCE, DUPLICATE_HANDLE_OPTIONS,
    DUPLICATE_SAME_ACCESS, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, POINT, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
    PROCESS_DUP_HANDLE, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

/// Map-only on the driver's section duplicate. No OWNER / `WRITE_DAC` / DELETE.
const SECTION_MAP_RW: u32 = 0x0004 | 0x0002;
/// Driver only `SetEvent`s; host keeps `SYNCHRONIZE` on its own handle.
const EVENT_MODIFY_STATE: u32 = 0x0002;

/// Stamped into every AU section so a publish an old encoder left behind is rejected.
static IDD_GENERATION: AtomicU32 = AtomicU32::new(1);

/// Masked to [`encode::FrameToken::GENERATION_MASK`] and never `0` — `0` is the
/// cleared-`latest` sentinel a freshly created section carries.
fn next_generation() -> u32 {
    loop {
        let g =
            IDD_GENERATION.fetch_add(1, Ordering::Relaxed) & encode::FrameToken::GENERATION_MASK;
        if g != 0 {
            return g;
        }
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// File mapping + mapped view. Drop unmaps, then [`OwnedHandle`] closes.
/// Borrowers hold the pointer, so declare this before whatever borrows it.
struct MappedSection {
    handle: OwnedHandle,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl MappedSection {
    /// View base; valid only while this section lives.
    fn ptr<T>(&self) -> *mut T {
        self.view.Value as *mut T
    }
}

impl Drop for MappedSection {
    fn drop(&mut self) {
        // SAFETY: `view` is the live `MapViewOfFile` mapping; unmap before `handle` closes.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
        }
    }
}

/// Image path is `%SystemRoot%\System32\WUDFHost.exe` before duplicating
/// handles into `process`. `what` names the channel in the error.
///
/// Path only — not our UMDF host, and not authorization. Callers judge
/// sufficiency (`design/idd-push-security.md`). A token/session check
/// false-negatives: genuine host and spawned copy are both session 0
/// LocalService.
///
/// # Safety
/// `process` must carry `PROCESS_QUERY_LIMITED_INFORMATION`.
pub unsafe fn verify_is_wudfhost(process: HANDLE, wudf_pid: u32, what: &str) -> Result<()> {
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    // SAFETY: `process` carries QUERY_LIMITED; `buf`/`len` are a valid out-buffer.
    // On success `len` is the UTF-16 unit count written (no NUL).
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .with_context(|| format!("QueryFullProcessImageNameW on the {what} pid"))?;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    let got = path.to_ascii_lowercase().replace('/', "\\");
    let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let expected = format!("{}\\system32\\wudfhost.exe", sysroot.to_ascii_lowercase());
    if got != expected {
        bail!(
            "{what} pid {wudf_pid} is not the system WUDFHost (image={path:?}, expected \
             {expected:?}) — refusing to duplicate the channel's handles into it (spoofed driver / \
             wrong devnode?)"
        );
    }
    Ok(())
}

#[path = "idd_push/channel.rs"]
mod channel;
// Construction: adapter, HDR, cursor opt-in.
#[path = "idd_push/open.rs"]
mod open;
// Synthetic DWM compose kick — the first-frame lever on an idle desktop.
#[path = "idd_push/compose_kick.rs"]
mod compose_kick;
use compose_kick::kick_dwm_compose;
#[path = "idd_push/cursor.rs"]
mod cursor;
#[path = "idd_push/cursor_poll.rs"]
mod cursor_poll;
#[path = "idd_push/descriptor.rs"]
mod descriptor;
// Stall reporting: the driver-clock verdict, plus DxgKrnl ETW and micro-probes as evidence.
#[path = "idd_push/dxgkrnl_etw.rs"]
mod dxgkrnl_etw;
#[path = "idd_push/probes.rs"]
mod probes;
#[path = "idd_push/stall.rs"]
mod stall;
// Live health classification + the staged-recovery ladder (immunity plan WP12/WP13).
#[path = "idd_push/recovery.rs"]
mod recovery;
// In-driver encode: the AU section, `SET_ENCODE`, and the `Encoder` proxy over `ENCODE_CTL`.
#[path = "idd_push/driver_encode.rs"]
pub(crate) mod driver_encode;
use channel::ChannelBroker;
use descriptor::{DescriptorPoller, DisplayDescriptor};
use stall::{StallEvidence, StallWatch};

/// The session's virtual display: its mode, its cursor channel, and its health.
///
/// Frames carry no pixels — the driver encodes them — so a delivery is only the
/// news that the driver's pool took a new composed frame, which is what the
/// stream loop needs to pace and what the classifier needs as source progress.
pub struct IddPushCapturer {
    /// Driver-protocol target id (encoder open, cursor channel, logs). CCD path selection goes
    /// through `ccd` — a bare id is only unique per adapter.
    target_id: u32,
    /// Complete CCD identity (adapter LUID + target id) for every display-global helper.
    ccd: pf_win_display::win_display::CcdTargetKey,
    /// Monotonic count of NEW source images the driver reported (`FrameOrigin::Source` only).
    source_seq: u64,
    /// The driver's own source counter at the last delivery — the freshness test.
    driver_source_seq: u64,
    /// Geometry and format of the last delivery. A mode or depth change must reach the stream
    /// loop even over a desktop composing nothing: it rebuilds the encoder off the frame, and
    /// the in-place resize waits for one at the new size with the encoder untouched.
    delivered: Option<(u32, u32, PixelFormat)>,
    /// Health classifier + staged-recovery ladder over this capturer's clocks (WP12/WP13).
    recovery: recovery::Supervisor,
    /// A closed episode's measured outage, until the stream loop takes it (WP14).
    recovered_outage: Option<Duration>,
    /// A rung the stream loop's actuator runs, until it takes it (`take_pending_stage`).
    pending_stage: Option<pf_frame::recovery::Stage>,
    /// A typed end the ladder decided off the capture path; `try_consume` returns it next.
    pending_fault: Option<anyhow::Error>,
    /// The session encoder's clocks from the loop's last `observe_encoder` — the only view
    /// this side has of the driver's frame and access-unit progress.
    encoder: Option<pf_frame::health::EncoderTelemetry>,
    /// Handle-duplication into WUDFHost, and the driver-death probe.
    broker: ChannelBroker,
    /// Hardware-cursor shm (`Some` = delivered). The driver publishes into it and blends
    /// from it; this side reads it for the client's own pointer.
    cursor_shared: Option<cursor::CursorShared>,
    /// GDI overlay source while alive — full-fidelity shapes IddCx never delivers.
    cursor_poll: Option<cursor_poll::CursorPoller>,
    /// `IOCTL_SET_CURSOR_FORWARD`. A declared IddCx hardware cursor blocks the
    /// OS software-cursor path; [`Self::poll_secure_desktop`] stands it down at UAC/Winlogon.
    cursor_forward: Option<crate::CursorForwardSender>,
    /// Poller reports a secure input desktop and the declare is stood down.
    secure_active: bool,
    /// The client draws no pointer, so the driver blends the excluded one into what it encodes.
    composite_cursor: bool,
    /// No cursor channel, but the target still has an earlier session's hardware-cursor
    /// declare (`WinCaptureTarget::cursor_excluded`). Pins `composite_cursor` on.
    composite_forced: bool,
    /// [`Self::live_cursor`] fell back to shm. Independent serial namespaces — never unlatch.
    cursor_shm_latched: bool,
    /// HDR cursor match to desktop SDR white (vs 80 nits). 2.5 ≈ Windows default; stamped into
    /// the cursor section because session 0 cannot query it.
    sdr_white_scale: f32,
    width: u32,
    height: u32,
    /// Handshake advertised `VIDEO_CAP_HDR` (not merely 10-bit). Pins composition
    /// so an SDR client never gets in-band PQ.
    want_hdr: bool,
    /// 10-bit SDR: the driver expands BGRA 8→10 into [`PixelFormat::Rgb10a2Sdr`]. Display
    /// colour is never touched — `want_hdr` stays false.
    ten_bit_sdr: bool,
    /// Live `advanced_color_enabled`. A change re-opens the driver's encoder.
    display_hdr: bool,
    /// One-shot: the display refused the negotiated depth (poller is ~4 Hz).
    hdr_pin_warned: bool,
    /// Failed pin attempts. Past [`Self::HDR_PIN_EAGER`] retry every
    /// [`Self::HDR_PIN_RETRY_EVERY`]th sample — CCD write+query takes the session-global lock.
    hdr_pin_failures: u32,
    /// Full-chroma 4:4:4.
    want_444: bool,
    /// Wavelet session (`design/pyrowave-windows-host-zerocopy.md`).
    pyrowave: bool,
    /// Off-thread CCD snapshot; the capture loop never runs those queries inline.
    desc_poller: DescriptorPoller,
    /// Last consumed poller sequence (0 = none yet).
    desc_seq: u64,
    /// Two-strikes debounce: act only when a second consecutive sample agrees,
    /// so a topology re-probe blip never re-opens the encoder.
    pending_desc: Option<DisplayDescriptor>,
    /// The topology generation `pending_desc` was sampled under ([`poll_display_hdr`]).
    pending_desc_gen: u64,
    /// A presentation restart is in flight; if no frame resumes past the window,
    /// `try_consume` drops the session (recover-or-drop, no DDA).
    recovering_since: Option<Instant>,
    /// Last fresh driver frame. A dead WUDFHost and an idle desktop both stop
    /// advancing the driver's source counter.
    last_fresh: Instant,
    /// One 0 ms wait per second, and only while stale.
    last_liveness: Instant,
    /// Mid-session [`kick_dwm_compose`] (recovery window only).
    last_kick: Instant,
    /// Multi-hundred-ms DWM holes during active flow; warns when they turn metronomic.
    stall_watch: StallWatch,
    /// The stalest drain heartbeat (µs) seen since the last fresh frame.
    max_hb_age_us: u64,
    /// Damage witness for [`stall::StallEvidence::cursor_moved_px`]. Pending sample
    /// is held one call so the stall-ending move is not counted into the gap it ended.
    /// Sampled at most every [`Self::CURSOR_WITNESS_INTERVAL`]; user32, never the display-config lock.
    cursor_last: Option<(i32, i32)>,
    cursor_gap_px: u32,
    cursor_pending_px: u32,
    cursor_sampled_at: Instant,
    /// Micro-probe singleton. `None` when `PUNKTFUNK_STALL_PROBES=0`; the matrix
    /// treats a missing window as never-stalled, so reports never invent legs.
    probes: Option<Arc<probes::ProbeEngine>>,
    /// DxgKrnl ETW; `None` when the session cannot start it (reports `etw=unavailable`).
    etw: Option<Arc<dxgkrnl_etw::EtwWatch>>,
    /// `PowerRequestDisplayRequired` for this capturer's life: DWM composes nothing
    /// once the console goes dark. Waking an already-off display is the HID kick.
    _display_wake: Option<pf_frame::session_tuning::DisplayWakeRequest>,
    _keepalive: Box<dyn Send>,
}
// SAFETY: `!Send` only through the cursor section's mapped-view pointer. Created, used and
// dropped on the capture thread, and the driver's writes into that section arrive through its
// own seqlock. `Send` moves ownership with no concurrent access; we do not claim `Sync`.
unsafe impl Send for IddPushCapturer {}

impl IddPushCapturer {
    /// Failed pins before [`Self::poll_display_hdr`] backs off (≈2 s at 4 Hz).
    const HDR_PIN_EAGER: u32 = 8;
    /// While backed off, re-pin every this-many-th sample (~4 s at 4 Hz).
    const HDR_PIN_RETRY_EVERY: u64 = 16;

    /// Age of a driver QPC stamp in µs (QPC is system-wide). 0 if the stamp is ahead.
    fn qpc_age_us(stamp: u64) -> u64 {
        static FREQ: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let freq = *FREQ.get_or_init(|| {
            let mut f = 0i64;
            // SAFETY: plain FFI; `f` is a valid local out-param. Frequency is fixed at boot;
            // 0 means the call failed — guarded below.
            let _ = unsafe { QueryPerformanceFrequency(&mut f) };
            f.max(0) as u64
        });
        if freq == 0 {
            return 0;
        }
        let mut now = 0i64;
        // SAFETY: plain FFI; `now` is a valid local out-param.
        if unsafe { QueryPerformanceCounter(&mut now) }.is_err() {
            return 0;
        }
        (now as u64).saturating_sub(stamp).saturating_mul(1_000_000) / freq
    }

    /// The frame format the driver's encoder takes, from display HDR + session 4:4:4. The
    /// stream loop rebuilds the encoder — a fresh `SET_ENCODE` — whenever this changes.
    fn out_format(&self) -> PixelFormat {
        // PyroWave carries planar studio codes; the label follows the display's depth.
        if self.pyrowave {
            return if self.display_hdr {
                PixelFormat::P010
            } else {
                PixelFormat::Nv12
            };
        }
        if self.display_hdr {
            if self.want_444 {
                // Packed RGB; the encoder CSCs to YUV 4:4:4. No subsampling here.
                return PixelFormat::Rgb10a2;
            }
            PixelFormat::P010
        } else if self.ten_bit_sdr {
            PixelFormat::Rgb10a2Sdr
        } else if self.want_444 {
            PixelFormat::Bgra
        } else {
            PixelFormat::Nv12
        }
    }

    /// Re-open the encoder when two consecutive poller samples agree on a new descriptor
    /// (~½ s), so a topology re-probe blip never costs a session rebuild.
    fn poll_display_hdr(&mut self) {
        let (mut now, seq) = self.desc_poller.snapshot();
        if seq == self.desc_seq {
            return;
        }
        self.desc_seq = seq;
        // Exclusive-watchdog reassert in flight: a sample here is the transient eviction.
        if pf_win_display::topology_churn::held() {
            self.pending_desc = None;
            return;
        }
        // Re-assert negotiated depth instead of following a mid-session flip:
        // PyroWave plane formats are fixed; an SDR session must not promote to
        // P010 PQ. HDR H.26x is not pinned — its encoder re-opens on a flip.
        if (self.pyrowave || !self.want_hdr) && now.hdr != self.want_hdr {
            let want = self.want_hdr;
            if self.hdr_pin_failures < Self::HDR_PIN_EAGER
                || self.desc_seq % Self::HDR_PIN_RETRY_EVERY == 0
            {
                // OBSERVE the flip; never assert it. Substituting the DESIRED state for the
                // observed one breaks in both directions on a display that cannot be flipped:
                // the encoder opens for a depth the driver does not compose, and every frame
                // is wrong until the poller corrects it.
                let requested = pf_win_display::win_display::set_advanced_color(self.ccd, want);
                let observed = pf_win_display::win_display::advanced_color_enabled(self.ccd);
                // A failed READ is not evidence of a failed flip — keep the poller's sample then.
                now.hdr = observed.unwrap_or(now.hdr);
                if now.hdr != want {
                    self.hdr_pin_failures = self.hdr_pin_failures.saturating_add(1);
                    if !self.hdr_pin_warned {
                        self.hdr_pin_warned = true;
                        tracing::error!(
                            target_id = self.target_id,
                            want_hdr = want,
                            observed_hdr = ?observed,
                            set_advanced_color_returned = requested,
                            pyrowave = self.pyrowave,
                            "IDD push: could not pin the display to the NEGOTIATED depth — following what \
                             it actually composes instead (a physical display forcing HDR, or a driver that \
                             refuses the flip). The stream's depth will not match the negotiation; the \
                             encoder's caps cross-check reports the truth to the client"
                        );
                    }
                } else {
                    self.hdr_pin_failures = 0;
                }
            }
        } else {
            // No mismatch (or the session follows flips): a later refusal starts eager again.
            self.hdr_pin_failures = 0;
        }
        let current = DisplayDescriptor {
            hdr: self.display_hdr,
            width: self.width,
            height: self.height,
        };
        if now == current {
            self.pending_desc = None;
            return;
        }
        // Samples name the topology generation they were observed under (immunity plan WP10 item
        // 5): two strikes straddling a finished topology transaction are two different desktops,
        // never one confirmed change.
        let topo_gen = pf_win_display::topology_churn::generation();
        if self.pending_desc != Some(now) || self.pending_desc_gen != topo_gen {
            // First strike — act only when a second consecutive sample agrees.
            self.pending_desc = Some(now);
            self.pending_desc_gen = topo_gen;
            return;
        }
        self.pending_desc = None;
        tracing::info!(
            target_id = self.target_id,
            from = format!("{}x{} hdr={}", self.width, self.height, self.display_hdr),
            to = format!("{}x{} hdr={}", now.width, now.height, now.hdr),
            "IDD push: display descriptor changed — the next frame re-opens the driver's encoder"
        );
        self.display_hdr = now.hdr;
        self.width = now.width;
        self.height = now.height;
        self.refresh_sdr_white_scale();
    }

    /// Overlay source for [`Capturer::cursor`] and the shape the client draws.
    ///
    /// A live poller wins even while it still reports `None`. Shm is only for a
    /// dead/missing poller, then latched: the two serial namespaces must not interleave.
    fn live_cursor(&mut self) -> Option<pf_frame::CursorOverlay> {
        if !self.cursor_shm_latched {
            if let Some(p) = &self.cursor_poll {
                if p.alive() {
                    return p.read();
                }
            }
            // About to read shm — latch so a revived poller cannot recross serials.
            if self.cursor_shared.is_some() {
                self.cursor_shm_latched = true;
                tracing::warn!(
                    target_id = self.target_id,
                    "cursor: the GDI shape poller is not running — degrading to the driver's \
                     hardware-cursor shm section for the rest of the session (alpha-only shapes: \
                     monochrome/masked cursors will look wrong)"
                );
            }
        }
        self.cursor_shared.as_mut().and_then(|c| c.read())
    }

    /// Where DWM places SDR white on this HDR desktop. 2.5× at the Windows default.
    /// Stamped into the cursor section: the driver's blend needs it and session 0
    /// cannot query it. The CCD read takes the display-config lock, so this runs at
    /// open and on a descriptor change only.
    fn refresh_sdr_white_scale(&mut self) {
        if !self.display_hdr {
            if let Some(cs) = self.cursor_shared.as_ref() {
                cs.set_sdr_white_scale(0.0);
            }
            return;
        }
        let queried = pf_win_display::win_display::sdr_white_level_scale(self.ccd);
        self.sdr_white_scale = queried.unwrap_or(self.sdr_white_scale);
        if let Some(cs) = self.cursor_shared.as_ref() {
            cs.set_sdr_white_scale(self.sdr_white_scale);
        }
        tracing::info!(
            target_id = self.target_id,
            queried = ?queried,
            applied = self.sdr_white_scale,
            "cursor composite: HDR SDR-white scale (1.0 = 80 nits; None = query failed — keeping \
             the prior value)"
        );
    }

    /// UAC/Winlogon use the software-cursor path; a declared IddCx hardware cursor
    /// blocks it. Stand the declare down on the secure edge; restore on dismissal.
    /// Must run every tick, including while frames are stalled.
    fn poll_secure_desktop(&mut self) {
        let Some(fwd) = self.cursor_forward.as_ref() else {
            return;
        };
        // Channel session, or forced-composite on a reused monitor that may still
        // run an earlier worker. A clean target has no poller — no guard.
        if self.cursor_shared.is_none() && !self.composite_forced {
            return;
        }
        let secure = pf_win_display::secure_desktop();
        if secure == self.secure_active {
            return;
        }
        self.secure_active = secure;
        if secure {
            tracing::info!(
                target_id = self.target_id,
                "secure desktop (UAC/Winlogon) active — standing the IddCx hardware-cursor \
                 declare down so the OS software-cursor path can render it"
            );
            if let Err(e) = fwd(false) {
                tracing::warn!(
                    "secure-desktop cursor-forward stand-down failed (secure content may stay \
                     invisible this session): {e:#}"
                );
            }
        } else {
            tracing::info!(
                target_id = self.target_id,
                "secure desktop dismissed — restoring the cursor render model"
            );
            // Only the session that runs the cursor channel. Forced-composite never
            // wanted the declare; leaving desired-state off stops per-assign re-declares.
            if self.cursor_shared.is_some() {
                if let Err(e) = fwd(true) {
                    tracing::warn!(
                        "secure-desktop cursor-forward re-enable failed (client-drawn cursor \
                         may double with a composited one): {e:#}"
                    );
                }
            }
        }
    }

    /// Two user32 reads; 8 ms so a ≥150 ms hole still gets many samples.
    const CURSOR_WITNESS_INTERVAL: Duration = Duration::from_millis(8);

    /// The damage witness (see the `cursor_*` field docs): fold the PREVIOUS call's pending
    /// delta into the gap accumulator — if that call had consumed a fresh frame, the fresh-frame
    /// bookkeeping would have zeroed the pending, so whatever survives belongs to the gap — then
    /// take a fresh rate-limited `GetCursorPos` sample into the pending slot. The one-call lag is
    /// what keeps the stall-ending frame's own cursor move out of the gap it ended.
    ///
    /// `GetCursorPos` is global, not per-display: a delta of 0 therefore proves the cursor sat
    /// still EVERYWHERE (the demotion direction is strict), while a delta > 0 on a
    /// parallel-displays host may be a sibling display's motion — that direction only ever
    /// upholds today's CONTENT-SILENCE labeling, never worsens it.
    fn sample_cursor_witness(&mut self) {
        self.cursor_gap_px = self.cursor_gap_px.saturating_add(self.cursor_pending_px);
        self.cursor_pending_px = 0;
        if self.cursor_sampled_at.elapsed() < Self::CURSOR_WITNESS_INTERVAL {
            return;
        }
        self.cursor_sampled_at = Instant::now();
        let mut pos = POINT::default();
        // SAFETY: plain FFI; `pos` is a valid out-param for this synchronous call.
        if unsafe { GetCursorPos(&mut pos) }.is_ok() {
            if let Some((x, y)) = self.cursor_last {
                self.cursor_pending_px = pos.x.abs_diff(x).saturating_add(pos.y.abs_diff(y));
            }
            self.cursor_last = Some((pos.x, pos.y));
        }
    }

    /// Staged recovery (immunity plan WP12/WP13). A wedged-but-ALIVE display answers
    /// `Ok(None)` forever, so a known-active desktop could stream nothing indefinitely. The
    /// classifier names the gap from the driver's own clocks — drain heartbeat, pool source
    /// sequence, access units — plus cursor travel and an unanswered canary; the coordinator
    /// walks the ladder from that class, one rung at a time under a deadline, and proves each
    /// rung with NEW source frames (access units for an encoder stall). No evidence = plain
    /// idle = no recovery: a static desktop composes nothing, and that is healthy. An
    /// exhausted ladder ends the plane with the typed `CaptureFault::SourceStalled`.
    fn recovery_tick(&mut self) -> Result<()> {
        let now = Instant::now();
        let inputs = recovery::Inputs {
            now,
            last_source: self.last_fresh,
            source_seq: self.source_seq,
            heartbeat_age: self.heartbeat_age(),
            // With the pointer composited in the driver, cursor travel dirties nothing and the
            // input canary can never be answered: neither is evidence of a changed desktop.
            cursor_gap_px: if self.composite_cursor {
                0
            } else {
                self.cursor_gap_px
            },
            recreating: self.recovering_since.is_some(),
            secure_desktop: self.secure_active,
            topology_held: pf_win_display::topology_churn::held(),
            encoder: self.encoder,
        };
        let step = self.recovery.tick(inputs);
        self.drive(step)
    }

    /// How long ago the driver's drain worker last finished a pass; `None` before its first
    /// heartbeat (no encoder open yet).
    fn heartbeat_age(&self) -> Option<Duration> {
        let t = self.encoder?;
        Some(
            Instant::now()
                .checked_duration_since(t.drain_heartbeat?)
                .unwrap_or_default(),
        )
    }

    /// Walk the supervisor's steps until it rests or the plane ends. A rung the stream loop
    /// owns is parked in `pending_stage`; the loop's `stage_done` re-enters here with its
    /// outcome.
    fn drive(&mut self, mut step: recovery::Step) -> Result<()> {
        loop {
            match step {
                recovery::Step::Nothing => return Ok(()),
                recovery::Step::Canary => {
                    tracing::info!(
                        target = %self.ccd,
                        "IDD push: source suspect on weak evidence — presenting the compose canary"
                    );
                    kick_dwm_compose(self.ccd);
                    return Ok(());
                }
                recovery::Step::Run(stage) => {
                    let Some(outcome) = self.run_stage(stage) else {
                        self.pending_stage = Some(stage);
                        return Ok(());
                    };
                    step = self.finish_stage(stage, outcome);
                }
                recovery::Step::Recovered { summary, outage } => {
                    tracing::info!(
                        target = %self.ccd,
                        ?summary,
                        outage_ms = outage.as_millis() as u64,
                        "IDD push: recovery episode closed"
                    );
                    self.recovered_outage = Some(outage);
                    return Ok(());
                }
                recovery::Step::Failed { gap, summary } => {
                    let fault = crate::CaptureFault::SourceStalled {
                        secs: gap.as_secs() as u32,
                    };
                    tracing::error!(
                        target = %self.ccd,
                        %fault,
                        ?summary,
                        "IDD push: recovery ladder exhausted"
                    );
                    return Err(anyhow::Error::new(fault).context(
                        "IDD-push: a known-active display delivered no source frame through the \
                         recovery ladder — ending the video plane with a typed error",
                    ));
                }
            }
        }
    }

    /// Record a rung's outcome with the supervisor and take its next step.
    fn finish_stage(
        &mut self,
        stage: pf_frame::recovery::Stage,
        outcome: pf_frame::recovery::StageOutcome,
    ) -> recovery::Step {
        tracing::warn!(
            target = %self.ccd,
            ?stage,
            ?outcome,
            "IDD push: recovery stage"
        );
        self.recovery.stage_done(Instant::now(), stage, outcome)
    }

    /// Run one ladder rung here, or `None` for a rung the stream loop owns: the encoder reset
    /// and the driver cycle, where the loop holds the encoder and the display manager. Rungs
    /// with no actuator on this host report `Unsupported` and the ladder moves on unpenalised.
    fn run_stage(
        &mut self,
        stage: pf_frame::recovery::Stage,
    ) -> Option<pf_frame::recovery::StageOutcome> {
        use pf_frame::recovery::{Stage, StageOutcome};
        Some(match stage {
            Stage::PresentationReset => {
                if self.restart_presentation_in_place() {
                    StageOutcome::Applied
                } else {
                    StageOutcome::Failed
                }
            }
            Stage::EncoderReset | Stage::DriverCycle => return None,
            Stage::SwapChainReset => StageOutcome::Unsupported,
        })
    }

    /// One tick: pollers, recovery, then the driver's frame cadence. A delivery carries no
    /// pixels (the driver encodes them) — its geometry, format and provenance are what the
    /// stream loop consumes, and `Ok(None)` means the desktop composed nothing since the last.
    fn try_consume(&mut self) -> Result<Option<CapturedFrame>> {
        if let Some(e) = self.pending_fault.take() {
            return Err(e);
        }
        // Secure-desktop first: UAC/Winlogon may produce no frames until this edge.
        self.poll_secure_desktop();
        // Witness before any early return so every gap shape accumulates cursor motion.
        self.sample_cursor_witness();
        // A "Use HDR" flip or a resize re-opens the encoder at the matching format.
        self.poll_display_hdr();
        // Recover-or-drop: a presentation restart that never resumes ends the session.
        if let Some(since) = self.recovering_since {
            // Under a recovery episode the ladder's stage deadlines govern instead.
            if since.elapsed() > Duration::from_secs(3) && !self.recovery.owns_episode() {
                bail!(
                    "IDD-push: the display was restarted in place and no frame followed within 3s \
                     — dropping the session so the client reconnects"
                );
            }
            // Idle desktop after the restart: no compose, and recover-or-drop would kill a
            // healthy session. The driver's retained pool slot covers most of it; this kick is
            // the fallback. Rate-limited; may block ~35 ms on the sibling-display branch.
            if since.elapsed() > Duration::from_millis(600)
                && self.last_kick.elapsed() > Duration::from_millis(800)
            {
                self.last_kick = Instant::now();
                tracing::debug!(
                    target_id = self.target_id,
                    "IDD push: no frame after the presentation restart — falling back to a \
                     synthetic compose kick"
                );
                kick_dwm_compose(self.ccd);
            }
        }
        // A dead WUDFHost and an idle desktop both stop advancing the source counter. Probe
        // while stale so the driver cycle fires instead of the session streaming nothing.
        if self.last_fresh.elapsed() > Duration::from_secs(2)
            && self.last_liveness.elapsed() > Duration::from_secs(1)
        {
            self.last_liveness = Instant::now();
            if !self.broker.driver_alive() {
                tracing::warn!(
                    wudf_pid = self.broker.wudf_pid,
                    "IDD push: the pf-vdisplay WUDFHost is gone — firing the driver cycle"
                );
                self.pending_stage = Some(pf_frame::recovery::Stage::DriverCycle);
                return Ok(None);
            }
        }
        // First frame: DWM presents a display only when something dirties it, and the driver's
        // retained pool slot is empty on a monitor's first session. Kick until the pool takes
        // one. Rate-limited, and only once the encoder is open — before that nobody would see it.
        if self.driver_source_seq == 0
            && self.encoder.is_some()
            && self.last_kick.elapsed() > Duration::from_millis(800)
        {
            self.last_kick = Instant::now();
            kick_dwm_compose(self.ccd);
        }
        // Staged recovery — after the driver-death watch, so an episode means the WUDFHost is
        // ALIVE and the presentation path is what stopped.
        self.recovery_tick()?;
        // Stall-attribution evidence: the STALEST the driver's drain heartbeat ever reads
        // between fresh frames. A heartbeat that goes quiet for the hole convicts the worker
        // (starved/dead WUDFHost); one that stays fresh through it indicts the compose path.
        if let Some(age) = self.heartbeat_age() {
            self.max_hb_age_us = self.max_hb_age_us.max(age.as_micros() as u64);
        }
        // The driver's pool counter is the only source clock this side has. Before the
        // encoder opens there is no counter at all, and the loop needs one frame to open it.
        let opened = self.encoder.is_some();
        let driver_seq = self.encoder.map_or(0, |t| t.source_seq);
        let geometry = (self.width, self.height, self.out_format());
        if opened && driver_seq == self.driver_source_seq && self.delivered == Some(geometry) {
            return Ok(None);
        }
        self.driver_source_seq = driver_seq;
        self.delivered = Some(geometry);
        let now = Instant::now();
        // The newest access unit's OS present stamp against the moment the host took it: the
        // ground-truth clock that tells "DWM stopped presenting" from "we were late".
        let arrival_ms = self
            .encoder
            .and_then(|t| t.present_to_arrival)
            .map(|d| d.as_millis() as u64);
        if self.recovering_since.take().is_some() {
            // Self-inflicted gap (the presentation restart). Reset so it is not a DWM stall.
            self.stall_watch.reset();
        } else if let Some(stall) = self.stall_watch.note_fresh(now, arrival_ms) {
            // ETW prose uses gap + 300 ms lead-in (the cause lands just before);
            // discriminator counts use the gap only — presents from healthy flow
            // would falsely acquit.
            let (etw, etw_counts) = self
                .etw
                .as_ref()
                .and_then(|w| {
                    now.checked_sub(stall.gap)
                        .map(|from| w.window_report(from, now, Duration::from_millis(300)))
                })
                .unzip();
            let evidence = StallEvidence {
                max_heartbeat_age_ms: (self.encoder.is_some())
                    .then_some(self.max_hb_age_us / 1_000),
                // Same window as the report's OS-event correlation (gap + cause lead-in).
                probes: now
                    .checked_sub(stall.gap + Duration::from_millis(300))
                    .zip(self.probes.as_deref())
                    .map(|(from, p)| p.window(from, now)),
                etw,
                etw_counts,
                // Gap accumulator only; this call's pending (ending-frame move) is still unfolded.
                cursor_moved_px: self.cursor_last.map(|_| self.cursor_gap_px),
            };
            self.stall_watch.report(&stall, now, &evidence);
        }
        // Sustained ~2 fps stretch: per-hole lines gate on prior ACTIVE flow.
        if let Some(r) = self.stall_watch.take_recovery() {
            let arrival = r.arrival_ms();
            tracing::info!(
                degraded_ms = r.degraded.as_millis() as u64,
                holes = r.holes,
                hole_time_ms = r.hole_time.as_millis() as u64,
                worst_hole_ms = r.worst.as_millis() as u64,
                // last/mean/max between the OS present and the access unit reaching us.
                present_to_arrival_ms = arrival.as_deref().unwrap_or("absent"),
                present_to_arrival_n = r.arrival_n,
                "IDD-push capture recovered from a degraded stretch — fresh frames arrived \
                 only between stall-sized holes for its whole span; the per-stall lines \
                 above cover at most its first hole"
            );
        }
        // A recovery episode closes only on the budgeted count of NEW source frames.
        if let Some((summary, outage)) = self.recovery.source_frame(now) {
            tracing::info!(
                target = %self.ccd,
                ?summary,
                outage_ms = outage.as_millis() as u64,
                "IDD push: recovery episode closed"
            );
            self.recovered_outage = Some(outage);
        }
        self.last_fresh = now;
        self.max_hb_age_us = 0;
        // Pending sample is the ending frame's move — discarded, never folded.
        self.cursor_gap_px = 0;
        self.cursor_pending_px = 0;
        self.source_seq += 1;
        Ok(Some(CapturedFrame {
            // The driver stamps each access unit with the frame's own present QPC; this side
            // reports the sequence only.
            provenance: pf_frame::Provenance::source(self.source_seq, 0),
            width: self.width,
            height: self.height,
            pts_ns: now_ns(),
            format: geometry.2,
            // No pixels cross the boundary: the loop sees `Encoder::ready_aus` answer `Some`
            // and owes wire indexes instead of submitting this frame.
            payload: FramePayload::Cpu(Vec::new()),
            cursor: None,
        }))
    }
}

/// Duplicate `cs` into WUDFHost and `IOCTL_SET_CURSOR_CHANNEL`.
/// `true` = adopted. Idempotent driver-side (replaced worker is stopped).
fn deliver_cursor_channel(
    broker: &ChannelBroker,
    target_id: u32,
    cs: &cursor::CursorShared,
    send_cursor: &crate::CursorChannelSender,
) -> bool {
    // SAFETY: `cs.section_handle()` borrows the mapping `cs` owns for this call;
    // the broker's WUDFHost process handle is live for the broker's lifetime.
    let value = match unsafe { broker.dup_into_public(cs.section_handle()) } {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("cursor section duplication failed (composited cursor stays): {e:#}");
            return false;
        }
    };
    let req = pf_driver_proto::control::SetCursorChannelRequest {
        target_id,
        _pad: 0,
        header_handle: value,
    };
    match send_cursor(&req) {
        Ok(()) => {
            tracing::info!(
                target_id,
                "IDD push(host): cursor channel delivered — driver declares the hardware cursor"
            );
            true
        }
        Err(e) => {
            broker.close_remote_public(value);
            tracing::warn!("cursor channel delivery failed (composited cursor stays): {e:#}");
            false
        }
    }
}

impl Capturer for IddPushCapturer {
    fn cursor(&mut self) -> Option<pf_frame::CursorOverlay> {
        self.live_cursor()
    }

    fn set_cursor_forward(&mut self, on: bool) {
        // Capture model: the declared hardware cursor stays excluded (no working un-declare);
        // the driver blends it into the frames it encodes. `composite_forced` cannot turn off
        // — no client draws.
        let composite = (!on && self.cursor_shared.is_some()) || self.composite_forced;
        if self.composite_cursor != composite {
            self.composite_cursor = composite;
            tracing::info!(
                composite,
                "cursor render model: the driver composites {}",
                if composite {
                    "ON (capture model — blending the pointer into what it encodes)"
                } else {
                    "OFF (client draws locally)"
                }
            );
            if let (Some(_), Some(fwd)) =
                (self.cursor_shared.as_ref(), self.cursor_forward.as_ref())
                && let Err(e) = fwd(!composite)
            {
                tracing::warn!(
                    composite,
                    error = %format!("{e:#}"),
                    "cursor render model: the driver did not take the flip"
                );
            }
        }
    }

    fn next_frame(&mut self) -> Result<CapturedFrame> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(f) = self.try_consume()? {
                return Ok(f);
            }
            if Instant::now() > deadline {
                bail!(
                    "no IDD-push frame within 20s (target {}) — the driver's encode pool took no \
                     composed frame: the swap-chain was never assigned, the display is powered \
                     off, or DWM composes nothing for it",
                    self.target_id
                );
            }
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    fn try_latest(&mut self) -> Result<Option<CapturedFrame>> {
        self.try_consume()
    }

    fn hdr_meta(&self) -> Option<pf_frame::HdrMeta> {
        // BT.2020 PQ while HDR. The driver does not forward IDDCX_HDR10_METADATA;
        // send the same generic HDR10 baseline as the native 0xCE path.
        self.display_hdr.then(pf_frame::hdr::generic_hdr10)
    }

    fn capture_target_id(&self) -> Option<u32> {
        Some(self.target_id)
    }

    fn resize_output(&mut self, width: u32, height: u32) -> bool {
        // The session already committed the new mode. Adopt it now — no two-strike debounce
        // (that stays for external HDR/game mode-sets); the loop's next frame carries the new
        // geometry, which re-opens the driver's encoder.
        if (width, height) == (self.width, self.height) {
            return true;
        }
        tracing::info!(
            target_id = self.target_id,
            from = format!("{}x{}", self.width, self.height),
            to = format!("{width}x{height}"),
            "IDD push: host-initiated resize — re-opening the driver's encoder at the new mode"
        );
        self.width = width;
        self.height = height;
        true
    }

    fn take_recovered_outage(&mut self) -> Option<Duration> {
        self.recovered_outage.take()
    }

    fn health(&self) -> Option<crate::CaptureHealth> {
        Some(self.recovery.report(Instant::now(), self.encoder.as_ref()))
    }

    fn observe_encoder(&mut self, t: Option<pf_frame::health::EncoderTelemetry>) {
        self.encoder = t;
    }

    fn take_pending_stage(&mut self) -> Option<pf_frame::recovery::Stage> {
        self.pending_stage.take()
    }

    fn stage_done(
        &mut self,
        stage: pf_frame::recovery::Stage,
        outcome: pf_frame::recovery::StageOutcome,
    ) {
        use pf_frame::recovery::{Stage, StageOutcome};
        let step = self.finish_stage(stage, outcome);
        // A driver cycle reaped the WUDFHost this capturer's encoder points at: end it here so
        // the host rebuilds the whole pipeline (SET_ENCODE included) against the fresh host.
        if matches!(stage, Stage::DriverCycle) && matches!(outcome, StageOutcome::Applied) {
            self.pending_fault = Some(anyhow::anyhow!(
                "IDD-push: the pf-vdisplay driver was cycled (adapter reload) — ending the \
                 capturer so the session rebuilds its virtual output on the fresh WUDFHost"
            ));
            return;
        }
        if let Err(e) = self.drive(step) {
            self.pending_fault = Some(e);
        }
    }

    fn driver_endpoint(&self) -> Option<crate::DriverEndpoint> {
        Some(crate::DriverEndpoint {
            target_id: self.target_id,
            wudf_pid: self.broker.wudf_pid,
        })
    }

    fn restart_presentation_in_place(&mut self) -> bool {
        // A target with no ACTIVE path cannot be recovered in place (immunity plan WP10 item 7):
        // a same-mode reset would attach to a known-inactive display. Fail fast — on a FRESH
        // snapshot only; a last-known-good one is not evidence either way.
        let snap = pf_win_display::display_events::snapshot_or_query();
        if snap.is_fresh() && !snap.target(self.ccd).is_some_and(|t| t.active) {
            tracing::warn!(
                target = %self.ccd,
                "IDD push: same-mode recovery refused — the target has no active display path \
                 (topology removed it); a topology recovery must precede a presentation restart"
            );
            return false;
        }
        // The eviction's topology commit leaves DWM not presenting to this display, so the
        // driver's drain worker acquires nothing. CDS_RESET forces a real mode-set at the
        // CURRENT mode — the same lever bring-up's ADD path relies on.
        match pf_win_display::win_display::resolve_gdi_name(self.ccd) {
            Some(gdi) => {
                if !pf_win_display::win_display::force_mode_reset(&gdi) {
                    tracing::warn!(
                        target_id = self.target_id,
                        "IDD push: presentation-restart mode reset failed"
                    );
                    return false;
                }
            }
            None => {
                tracing::warn!(
                    target_id = self.target_id,
                    "IDD push: no GDI name for the presentation-restart mode reset"
                );
                return false;
            }
        }
        tracing::info!(
            target_id = self.target_id,
            mode = format!("{}x{}", self.width, self.height),
            "IDD push: same-mode presentation restart"
        );
        self.recovering_since.get_or_insert_with(Instant::now);
        true
    }
}

impl Drop for IddPushCapturer {
    fn drop(&mut self) {
        // Must not leave per-target desired-state off: the next session would
        // adopt undeclared and silently run the composite model. Open-time reset
        // covers host crash; this is orderly teardown.
        if self.secure_active && self.cursor_shared.is_some() {
            if let Some(fwd) = self.cursor_forward.as_ref() {
                let _ = fwd(true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stall::Stall;
    use super::*;

    /// The `CcdTargetKey` packing must equal `pf_frame::dxgi::pack_luid` — the capture target's
    /// `adapter_luid` (packed by pf-frame) is what pf-capture builds its CCD keys from, so a
    /// divergence would make every display-global helper miss its own target's paths. This crate
    /// is the lowest one that depends on both, which is why the assertion lives here.
    #[test]
    fn ccd_key_packing_matches_pf_frame_pack_luid() {
        for (low, high) in [
            (0u32, 0i32),
            (0xdead_beef, -2),
            (7, 0x7fff_ffff),
            (u32::MAX, -1),
        ] {
            let luid = windows::Win32::Foundation::LUID {
                LowPart: low,
                HighPart: high,
            };
            assert_eq!(
                pf_win_display::win_display::CcdTargetKey::from_luid_parts(low, high, 1)
                    .adapter_luid,
                pf_frame::dxgi::pack_luid(luid),
                "packing diverged for LUID {high:#x}:{low:#x}"
            );
        }
    }

    /// The mint must stay inside the publish token's 24-bit generation field, and must skip 0.
    ///
    /// `IDD_GENERATION` is a full `u32` while `FrameToken` carries 24 bits and `unpack` MASKS
    /// what it reads, so an unmasked generation stops matching any token past 2²⁴ opens and
    /// every publish is rejected forever. The counter is parked just below the boundary here so
    /// the wrap is what gets exercised.
    #[test]
    fn the_section_generation_survives_the_publish_token() {
        IDD_GENERATION.store(encode::FrameToken::GENERATION_MASK - 2, Ordering::Relaxed);
        let mut seen = Vec::new();
        for _ in 0..8 {
            let g = next_generation();
            assert_ne!(g, 0, "0 also means the cleared-`latest` sentinel");
            assert_eq!(
                g & encode::FrameToken::GENERATION_MASK,
                g,
                "generation {g} does not fit the token's field"
            );
            let tok = encode::FrameToken {
                generation: g,
                seq: 12345,
                slot: 2,
            };
            let back = encode::FrameToken::unpack(tok.pack());
            assert_eq!(back.generation, g, "generation lost in the token");
            assert_eq!(back.seq, 12345, "seq lost in the token");
            assert_eq!(back.slot, 2, "slot lost in the token");
            seen.push(g);
        }
        // Started 2 below the mask; wrap produced no duplicate 0.
        assert!(
            seen.contains(&encode::FrameToken::GENERATION_MASK),
            "{seen:?}"
        );
        assert!(
            seen.iter().any(|&g| g < 8),
            "the counter should have wrapped: {seen:?}"
        );
    }

    /// Feed [`StallWatch`] at `offsets_ms`; metronome is non-damage-idle, as `report` feeds it.
    fn watch_run(offsets_ms: &[u64]) -> Vec<Option<(Stall, Option<Duration>)>> {
        let base = Instant::now();
        let mut w = StallWatch::new();
        offsets_ms
            .iter()
            .map(|ms| {
                let at = base + Duration::from_millis(*ms);
                w.note_fresh(at, None).map(|s| {
                    let period = w.cycle(at, false);
                    (s, period)
                })
            })
            .collect()
    }

    fn flow(out: &mut Vec<u64>, start_ms: u64, frames: u64) {
        out.extend((0..frames).map(|i| start_ms + i * 16));
    }

    #[test]
    fn stall_detected_after_active_flow() {
        // 20 frames of 60 fps, then a 300 ms hole — the resuming frame is a stall.
        let mut t = Vec::new();
        flow(&mut t, 0, 20); // last frame at 304 ms
        t.push(604);
        let out = watch_run(&t);
        assert!(out[..20].iter().all(Option::is_none));
        let (stall, period) = out[20].as_ref().expect("hole after active flow is a stall");
        assert_eq!(stall.gap.as_millis(), 300);
        assert!(period.is_none(), "one stall is not a cycle");
    }

    #[test]
    fn idle_desktop_gaps_are_not_stalls() {
        // ~530 ms caret blink: activity gate never opens.
        let t: Vec<u64> = (0..12).map(|i| i * 530).chain([20_000]).collect();
        assert!(watch_run(&t).iter().all(Option::is_none));
    }

    #[test]
    fn thirty_fps_content_still_qualifies_as_active() {
        // 33 ms cadence: 8 pre-gap frames span 231 ms ≤ ACTIVE_SPAN.
        let mut t: Vec<u64> = (0..10).map(|i| i * 33).collect(); // last at 297 ms
        t.push(497);
        let out = watch_run(&t);
        assert!(out[10].is_some(), "30 fps flow must pass the activity gate");
    }

    /// First degraded-stretch summary, checked after every frame like the capture loop.
    /// Every frame reports the same 40 ms present→arrival, so the folded tally is
    /// assertable without modelling which frames land inside the stretch.
    fn watch_recovery(offsets_ms: &[u64]) -> (StallWatch, Option<super::stall::Recovery>) {
        let base = Instant::now();
        let mut w = StallWatch::new();
        let mut recovery = None;
        for ms in offsets_ms {
            w.note_fresh(base + Duration::from_millis(*ms), Some(40));
            if let Some(r) = w.take_recovery() {
                recovery.get_or_insert(r);
            }
        }
        (w, recovery)
    }

    #[test]
    fn a_degraded_stretch_summarizes_on_recovery() {
        // ~2 fps phase (10×500 ms holes) after active flow: one summary for the stretch.
        let mut t = Vec::new();
        flow(&mut t, 0, 20); // last frame at 304 ms
        t.extend((1..=10).map(|i| 304 + i * 500)); // 804..5304: ten 500 ms holes
        t.extend((1..=12).map(|i| 5304 + i * 16)); // sustained flow is back
        let (_, r) = watch_recovery(&t);
        let r = r.expect("a multi-hole degraded stretch summarizes at recovery");
        assert_eq!(r.holes, 10);
        assert_eq!(r.hole_time.as_millis(), 5000);
        assert_eq!(r.worst.as_millis(), 500);
        assert_eq!(r.degraded.as_millis(), 5000);
        // Every stamped frame reported 40 ms, at least one per hole.
        assert_eq!(r.arrival_ms().as_deref(), Some("40/40/40"));
        assert!(r.arrival_n >= r.holes, "n={}", r.arrival_n);
    }

    #[test]
    fn a_single_stall_never_summarizes() {
        // One hole in healthy flow: its stall line covers it; a one-hole stretch must not summarize.
        let mut t = Vec::new();
        flow(&mut t, 0, 20);
        t.push(604); // the lone 300 ms hole
        t.extend((1..=12).map(|i| 604 + i * 16));
        let (_, r) = watch_recovery(&t);
        assert!(
            r.is_none(),
            "single stall must not produce a stretch summary"
        );
    }

    #[test]
    fn a_reset_cut_stretch_still_summarizes() {
        // A reset clears flow history mid-stretch; holes before it must still surface.
        let mut t = Vec::new();
        flow(&mut t, 0, 20);
        t.extend((1..=3).map(|i| 304 + i * 500));
        let (mut w, r) = watch_recovery(&t);
        assert!(r.is_none(), "stretch still open — no summary yet");
        w.reset();
        let r = w
            .take_recovery()
            .expect("reset closes and summarizes the open stretch");
        assert_eq!(r.holes, 3);
        assert_eq!(r.hole_time.as_millis(), 1500);
    }

    #[test]
    fn a_content_stop_closes_the_stretch_without_folding_the_pause_in() {
        // Two degraded holes, then a 20 s pause. Summary covers the stretch only.
        let mut t = Vec::new();
        flow(&mut t, 0, 20);
        t.extend([804, 1304, 21_304]);
        let (_, r) = watch_recovery(&t);
        let r = r.expect("the content stop closes the stretch");
        assert_eq!(r.holes, 2);
        assert_eq!(r.hole_time.as_millis(), 1000);
        assert_eq!(r.degraded.as_millis(), 1000);
    }

    #[test]
    fn metronomic_stalls_self_diagnose() {
        // ~300 ms DWM holes every 4 s in 60 fps flow. 5 cycles → 4 stalls; the 4th is the period.
        let mut t = Vec::new();
        for cycle in 0..5u64 {
            // ~3.7 s of flow, then the hole to the next cycle.
            flow(&mut t, cycle * 4_000, 232); // last frame at cycle*4000 + 3696
        }
        let out = watch_run(&t);
        let stalls: Vec<&(Stall, Option<Duration>)> = out.iter().flatten().collect();
        assert_eq!(stalls.len(), 4, "each cycle boundary is one stall");
        assert!(stalls[..3].iter().all(|(_, period)| period.is_none()));
        let period = stalls[3]
            .1
            .expect("the 4th evenly-spaced event completes the metronome streak");
        assert!(
            (period.as_secs_f64() - 4.0).abs() < 0.3,
            "period={period:?}"
        );
    }

    /// Same four evenly-spaced stalls as [`metronomic_stalls_self_diagnose`], one
    /// damage-idle: a hand/input pause is not display-disturbance evidence.
    #[test]
    fn damage_idle_stalls_do_not_feed_the_metronome() {
        let base = Instant::now();
        let mut w = StallWatch::new();
        let mut periods = Vec::new();
        for cycle in 0..5u64 {
            let mut t = Vec::new();
            flow(&mut t, cycle * 4_000, 232);
            for ms in t {
                let at = base + Duration::from_millis(ms);
                if let Some(_stall) = w.note_fresh(at, None) {
                    // 2nd stall is damage-idle (cursor still on a dwm-only desktop).
                    let damage_idle = periods.len() == 1;
                    periods.push(w.cycle(at, damage_idle));
                }
            }
        }
        assert_eq!(periods.len(), 4);
        assert!(
            periods.iter().all(Option::is_none),
            "a skipped beat must break the streak: {periods:?}"
        );
    }

    #[test]
    fn reset_swallows_the_restart_gap() {
        // Restart, then resume 800 ms later: not a stall; detection re-arms after.
        let base = Instant::now();
        let at = |ms: u64| base + Duration::from_millis(ms);
        let mut w = StallWatch::new();
        for i in 0..20u64 {
            assert!(w.note_fresh(at(i * 16), None).is_none());
        }
        w.reset();
        assert!(
            w.note_fresh(at(1_104), None).is_none(),
            "restart gap swallowed"
        );
        for i in 1..20u64 {
            assert!(w.note_fresh(at(1_104 + i * 16), None).is_none());
        }
        assert!(
            w.note_fresh(at(1_104 + 19 * 16 + 300), None).is_some(),
            "detection re-armed after the reset"
        );
    }

    /// Third stall in 60 s warns; quiet through 300 s re-warn spacing; re-arms after age-out.
    #[test]
    fn stall_rate_warn_window_and_rewarn() {
        let base = Instant::now();
        let at = |s: u64| base + Duration::from_secs(s);
        let mut w = StallWatch::new();
        assert_eq!(w.note_for_rate_warn(at(0)), None);
        assert_eq!(w.note_for_rate_warn(at(10)), None);
        assert_eq!(
            w.note_for_rate_warn(at(20)),
            Some(3),
            "third stall in 60 s warns"
        );
        assert_eq!(
            w.note_for_rate_warn(at(30)),
            None,
            "inside the re-warn spacing the arm stays quiet"
        );
        // Past the spacing: old entries aged out, so RATE_MIN_STALLS again then re-warns.
        assert_eq!(w.note_for_rate_warn(at(400)), None);
        assert_eq!(w.note_for_rate_warn(at(401)), None);
        assert_eq!(
            w.note_for_rate_warn(at(402)),
            Some(3),
            "re-warns after the spacing"
        );
    }

    /// [`stall::attribute`] verdict table: the drain heartbeat, then the cursor witness.
    #[test]
    fn stall_attribution_verdicts() {
        use super::stall::{attribute, StallVerdict};
        let verdict = |gap_ms: u64, hb_age_ms: Option<u64>, moved: Option<u32>| {
            attribute(
                Duration::from_millis(gap_ms),
                &StallEvidence {
                    max_heartbeat_age_ms: hb_age_ms,
                    probes: None,
                    etw: None,
                    etw_counts: None,
                    cursor_moved_px: moved,
                },
            )
        };
        // No encoder open yet: no heartbeat, no verdict.
        assert_eq!(verdict(300, None, None), StallVerdict::NoTelemetry);
        // Heartbeat silent for most of the hole → worker starved.
        assert_eq!(verdict(600, Some(400), None), StallVerdict::WorkerStalled);
        // ≤16 ms heartbeat; 200 ms silence on a 300 ms gap is under max(gap/2, 250 ms).
        assert_eq!(verdict(300, Some(200), None), StallVerdict::ComposeSilence);
        assert_eq!(
            verdict(300, Some(20), Some(312)),
            StallVerdict::ComposeSilence
        );
        // Long holes scale the bar: 900 ms silence on a 3 s gap is not half.
        assert_eq!(
            verdict(3_000, Some(900), None),
            StallVerdict::ComposeSilence
        );
        assert_eq!(
            verdict(3_000, Some(1_600), None),
            StallVerdict::WorkerStalled
        );
        // The cursor never moved through the hole: nothing was dirty.
        assert_eq!(verdict(600, Some(16), Some(0)), StallVerdict::DamageIdle);
        // A starved worker is never demoted by a still cursor.
        assert_eq!(
            verdict(600, Some(400), Some(0)),
            StallVerdict::WorkerStalled
        );
    }

    /// With the ETW leg on (`PUNKTFUNK_IDD_DIAG`), a game presenting through the hole keeps
    /// compose-silence even under a still cursor; dwm-only flow still demotes.
    #[test]
    fn a_present_witness_blocks_the_damage_idle_demotion() {
        use super::dxgkrnl_etw::EtwWindowCounts;
        use super::stall::{attribute, StallVerdict};
        let verdict = |dwm_only: bool| {
            attribute(
                Duration::from_millis(600),
                &StallEvidence {
                    max_heartbeat_age_ms: Some(16),
                    probes: None,
                    etw: None,
                    etw_counts: Some(EtwWindowCounts {
                        presents: 40,
                        queue_adds: 0,
                        present_history: true,
                        queue_history: true,
                        flow_dwm_only: dwm_only,
                    }),
                    cursor_moved_px: Some(0),
                },
            )
        };
        assert_eq!(verdict(false), StallVerdict::ComposeSilence);
        assert_eq!(verdict(true), StallVerdict::DamageIdle);
    }
}
