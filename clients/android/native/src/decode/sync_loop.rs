//! The synchronous MediaCodec decode loop (the original poll path) + its feed/drain helpers.

use ndk::data_space::DataSpace;
use ndk::media::media_codec::{
    DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
    OutputBuffer,
};
use ndk::native_window::NativeWindow;
use punktfunk_core::client::NativeClient;
use punktfunk_core::error::PunktfunkError;
use punktfunk_core::reanchor::{GateVerdict, ReanchorGate};
use punktfunk_core::session::Frame;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::display::{
    hdr_dataspace, install_render_callback, release_render_callback, DisplayTracker,
};
use super::latency::{note_decoded_pts, note_received_frame, now_realtime_ns, take_flags};
use super::setup::{
    boost_hot_threads, boost_thread_priority, codec_mime, create_codec, hdr_static,
    low_latency_format, try_set_frame_rate,
};
use super::{Backstops, DecodeOptions, IN_FLIGHT_CAP};

/// The synchronous poll loop — the original decode path: the only one when low-latency mode is off,
/// and the [`USE_ASYNC_DECODE`] A/B fallback when it's on. Feeds and drains on this one thread; the
/// only blocking wait is a short output dequeue while input is backed up.
pub(super) fn run_sync(
    client: Arc<NativeClient>,
    window: NativeWindow,
    shutdown: Arc<AtomicBool>,
    stats: Arc<crate::stats::VideoStats>,
    opts: DecodeOptions,
) {
    let DecodeOptions {
        decoder_name,
        ll_feature,
        low_latency_mode,
        is_tv,
        // The timeline presenter lives in the async loop only; this loop IS the escape hatch.
        present_priority: _,
        smooth_buffer: _,
        panel_hz: _,
        // The ASurfaceControl backend is async-loop only; the sync loop renders straight to the
        // SurfaceView, so it never needs the view's on-screen size.
        surface_size: _,
    } = opts;
    boost_thread_priority();
    let mode = client.mode();
    // The MediaCodec MIME for the codec the host resolved (`Welcome.codec`). AMediaCodec needs no
    // out-of-band extradata — the in-band VPS/SPS/PPS on every IDR configure it either way.
    let mime = codec_mime(client.codec);
    let codec = match create_codec(mime, decoder_name.as_deref()) {
        Some(c) => c,
        None => {
            log::error!("decode: no {mime} decoder on this device");
            return;
        }
    };
    // The decoder's *actual* resolved name (Kotlin's pick, or the platform default when it fell
    // back) drives both the HUD label and which vendor low-latency keys apply below.
    let codec_name = codec.name().unwrap_or_default();
    stats.set_decoder(&codec_name, ll_feature);
    log::info!(
        "decode: codec mime = {mime}, decoder = {codec_name} (low-latency feature: {ll_feature})"
    );

    // Standard + per-SoC vendor low-latency keys, gated on the resolved decoder name and the
    // master toggle (see `configure_low_latency`).
    let format = low_latency_format(
        mime,
        &mode,
        &codec_name,
        low_latency_mode,
        hdr_static(&client).as_ref(),
    );
    if let Err(e) = codec.configure(&format, Some(&window), MediaCodecDirection::Decoder) {
        log::error!("decode: configure failed: {e}");
        return;
    }
    if let Err(e) = codec.start() {
        // No bring-up ladder here, unlike the async loop: this path only runs with low-latency
        // mode OFF, which is already the conservative key set, and it renders straight into the
        // SurfaceView rather than an `AImageReader` — the two things that ladder sheds are both
        // already shed. Name the symptom instead, because the session stays up around this
        // failure (audio, input and the library all keep working) and the host sees only a
        // keyframe request every 2 s that reads as a slow link.
        log::error!(
            "decode: start failed: {e} — this session has no video. Audio and input keep working, \
             so the stream will look alive while the screen stays black"
        );
        return;
    }
    log::info!(
        "decode: {mime} decoder started at {}x{}",
        mode.width,
        mode.height
    );
    // Tell the display the stream's refresh so Android can pick a matching display mode and align
    // vsync (no 60-in-120 judder on high-refresh panels). `ANativeWindow_setFrameRate` is NDK API 30,
    // above our API-28 floor, so we resolve it at runtime (see `try_set_frame_rate`) rather than link
    // it — a hard import would stop `libpunktfunk_android.so` loading at all on API 28/29. Absent
    // there ⇒ we simply skip the hint (non-fatal; the stream renders fine without it).
    // The forced TV mode switch (`is_tv` ⇒ ALWAYS strategy) is part of the experimental stack;
    // off, every form factor gets the original soft seamless hint.
    if mode.refresh_hz > 0
        && !try_set_frame_rate(&window, mode.refresh_hz as f32, is_tv && low_latency_mode)
    {
        log::debug!(
            "decode: set_frame_rate({} Hz) unavailable/declined (non-fatal)",
            mode.refresh_hz
        );
    }

    // ADPF: hint the platform that the whole video pipeline — this pf-decode feed/drain/present
    // loop, the core's data-plane pump (UDP receive + FEC reassembly), and the audio thread — runs a
    // per-frame real-time workload, so the CPU governor keeps those threads on fast cores at high
    // clocks instead of down-clocking between frames or parking them on a little core. Snapdragon's
    // ADPF backend responds well to this. We register this thread now but create the session lazily
    // on the first presented frame: by then the pump + audio threads have registered their ids too,
    // and ADPF `createSession` rejects a set with any not-yet-live/dead tid. No-op below API 33.
    let frame_period_ns = if mode.refresh_hz > 0 {
        1_000_000_000i64 / mode.refresh_hz as i64
    } else {
        0
    };
    client.register_hot_thread(); // this decode thread → the pipeline's hot-thread set
    let mut hint: Option<crate::adpf::HintSession> = None;
    let mut hint_tried = false;
    // Accumulates the loop's productive (feed+drain) time between displayed frames; reported to ADPF
    // once per rendered frame against the frame-period target.
    let mut work_accum_ns: i64 = 0;

    let mut fed: u64 = 0;
    let mut rendered: u64 = 0;
    let mut discarded: u64 = 0;
    let mut backstops = Backstops::new();
    // AUs larger than the codec input buffer, dropped whole (see `feed`/`feed_ready`).
    let mut oversized_dropped: u64 = 0;
    // The AU waiting for a free codec input buffer. `feed` is non-blocking; on transient input
    // pressure the AU stays parked here instead of being dropped (a drop forces a keyframe
    // round-trip) and we only pop the next one once it's queued.
    let mut pending: Option<Frame> = None;
    // Freeze-until-reanchor ([`ReanchorGate`]): armed on a frame-index gap or a dropped-count
    // climb, it withholds concealed output (released WITHOUT rendering, so the SurfaceView keeps
    // the last frame on glass) until a clean re-anchor. `recovery_flags` carries each AU's
    // user_flags from feed to present, keyed by the codec-echoed pts.
    let mut gate = ReanchorGate::new(client.frames_dropped());
    let mut recovery_flags: VecDeque<(u64, u32)> = VecDeque::new();
    // Skew-corrected latency stats (spec: design/stats-unification.md) use the negotiated
    // host-minus-client clock offset (0 if the host didn't answer the skew handshake — then the
    // HUD flags it "(same-host clock)").
    let clock_offset = client.clock_offset_shared();
    // Display stage (spec `display` + the capture→displayed headline): frames released with
    // render = true are parked in the tracker; the OnFrameRendered callback pairs them with
    // SurfaceFlinger's render timestamp. `render_cb` is the callback's leaked Arc refcount,
    // reclaimed after the codec is dropped below.
    // The `video_e2e` cell is the audio plane's alignment reference (see `DisplayTracker`): this
    // legacy loop feeds it too, so A/V sync works with "Low-latency mode" off as well.
    let tracker = DisplayTracker::new(
        stats.clone(),
        clock_offset.clone(),
        client.video_e2e_shared(),
        std::sync::Arc::new(super::presenter::PresentMeter::new()),
    );
    let render_cb = install_render_callback(&codec, &tracker);
    // Receipt timestamps keyed by the pts we queue into the codec, so the decoded point (output-
    // buffer dequeue — MediaCodec round-trips presentationTimeUs) can be paired back to its receipt
    // for the `decode` stage. Fed while the HUD is visible OR the adaptive-bitrate controller wants
    // the decode signal (`measure_decode`) — the decoder-backlog bottleneck the network can't see.
    let measure_decode = client.wants_decode_latency();
    let mut in_flight: VecDeque<(u64, i128)> = VecDeque::new();
    // Phase-2 host/network split: received AUs awaiting their 0xCF host timing, as
    // (pts_ns, capture→received µs). Only fed while the HUD is visible.
    let mut pending_split: VecDeque<(u64, u64)> = VecDeque::new();
    let mut last_phase_ack: Option<i32> = None;
    // The dataspace we've signalled on the Surface so far (None = default/SDR). Set reactively once
    // the decoder reports an HDR stream (see `drain`); avoids re-applying every format event.
    let mut applied_ds: Option<DataSpace> = None;
    // One thread feeds AND drains: the NDK AMediaCodec wrapper isn't documented thread-safe for
    // cross-thread feed/drain, so instead of splitting threads the loop decouples the two — input
    // dequeue is non-blocking (never stalls presentation of already-decoded frames) and the only
    // blocking wait is a short output dequeue while input is backed up (decoder progress is exactly
    // what frees the next input buffer).
    while !shutdown.load(Ordering::Relaxed) {
        if pending.is_none() {
            match client.next_frame(Duration::from_millis(5)) {
                Ok(frame) => {
                    // Loss recovery (RFI): feed the frame index so a forward gap fires a throttled
                    // reference-frame-invalidation request — an RFI-capable host (AMD LTR / NVENC)
                    // recovers with a cheap clean P-frame instead of a full IDR. The same forward gap
                    // arms the freeze gate so the decoder's concealment is held off the screen until the
                    // recovery re-anchors. The frames_dropped keyframe path below stays the backstop.
                    // Credited arm: the gap width pre-covers the reassembler's ~120 ms-later
                    // `frames_dropped` climb for the same loss, so a fast RFI anchor that heals in
                    // between isn't re-frozen by it (the double-arm race — see
                    // `ReanchorGate::arm_expecting_drops`).
                    let gap = client.note_frame_index(frame.frame_index);
                    if gap > 0 {
                        gate.arm_expecting_drops(Instant::now(), u64::from(gap));
                    }
                    // Park this AU's re-anchor flags for the present side (keyed by the pts the codec
                    // echoes on the output buffer) — unconditional, unlike the HUD's `in_flight` map.
                    recovery_flags.push_back((frame.pts_ns / 1000, frame.flags));
                    if recovery_flags.len() > IN_FLIGHT_CAP {
                        recovery_flags.pop_front();
                    }
                    if fed == 0 {
                        let p = &frame.data;
                        log::info!(
                            "decode: first AU {} bytes, head {:02x?}",
                            p.len(),
                            &p[..p.len().min(6)]
                        );
                    }
                    // Receipt stamp for the `decode` stage pairing, whenever it's needed: the HUD
                    // being visible, or the ABR decode signal (`measure_decode`).
                    if stats.enabled() || measure_decode {
                        let received_ns = note_received_frame(
                            &client,
                            &stats,
                            &frame,
                            clock_offset.load(Ordering::Relaxed),
                            &mut pending_split,
                            &mut last_phase_ack,
                        );
                        in_flight.push_back((frame.pts_ns / 1000, received_ns));
                        if in_flight.len() > IN_FLIGHT_CAP {
                            in_flight.pop_front(); // stale — codec never echoed it back
                        }
                    }
                    pending = Some(frame);
                }
                Err(PunktfunkError::NoFrame) => {} // timeout — still drain output below
                Err(_) => break,                   // session closed
            }
        }
        // Time the productive work (feed + drain) only — the `next_frame` poll wait above is idle
        // and excluded, so ADPF sees this thread's real per-frame CPU cost, not the poll timeout.
        let work_t0 = Instant::now();
        if let Some(frame) = pending.take() {
            if feed(
                &codec,
                &client,
                &frame.data,
                frame.pts_ns / 1000,
                &mut oversized_dropped,
            ) {
                fed += 1;
                if fed % 300 == 0 {
                    log::info!("decode: fed={fed} rendered={rendered} discarded={discarded}");
                }
            } else {
                // No input buffer free — transient back-pressure. Keep the AU and let `drain` block
                // briefly below; a released output buffer is what recycles an input slot.
                pending = Some(frame);
            }
        }
        // Drain every iteration. When input is blocked, wait ~2 ms on output so the loop rides
        // decoder progress instead of busy-spinning against a full input queue.
        let wait = if pending.is_some() {
            Duration::from_millis(2)
        } else {
            Duration::ZERO
        };
        let (r, d) = drain(
            &codec,
            &client,
            measure_decode,
            &window,
            &mut applied_ds,
            wait,
            &stats,
            &mut in_flight,
            clock_offset.load(Ordering::Relaxed),
            &tracker,
            &mut gate,
            &mut recovery_flags,
        );
        rendered += r;
        discarded += d;
        // The one line that separates "the stream never reached glass" from "it reached glass and
        // looked wrong"; the tally above counts AUs FED, which a black session racks up happily.
        if r > 0 && rendered == r {
            log::info!("decode: first frame presented (fed={fed} discarded={discarded})");
        }

        // ADPF: attribute this iteration's feed+drain time to the frame being produced, and report
        // the accumulated per-frame work once one is actually presented (r > 0). Under back-pressure
        // the short output-dequeue wait is included in the tally — for a latency-first client,
        // biasing the governor toward "boost" is the desired behaviour. Cheap when `hint` is None
        // (one `Instant` diff, no report).
        work_accum_ns += work_t0.elapsed().as_nanos() as i64;
        if r > 0 {
            if !hint_tried {
                // First presented frame: the pump + audio threads have registered their ids by now.
                // Build one ADPF session over the whole pipeline's thread set (empty below API 33,
                // or where the platform declines → `None`, and the loop runs unhinted).
                hint_tried = true;
                let tids = client.hot_thread_ids();
                // The pump/audio priority boost is part of the experimental low-latency stack; the
                // ADPF session itself predates it and always runs (max-performance bias gated inside).
                if low_latency_mode {
                    boost_hot_threads(&tids);
                }
                hint = crate::adpf::HintSession::create(frame_period_ns, &tids, low_latency_mode);
                log::info!(
                    "decode: ADPF hint session {} — {} hot thread(s), target {frame_period_ns} ns",
                    if hint.is_some() {
                        "active"
                    } else {
                        "unavailable"
                    },
                    tids.len(),
                );
            }
            if let Some(h) = &hint {
                h.report_actual(work_accum_ns);
            }
            work_accum_ns = 0;
        }

        // Loss recovery + the overdue backstops (see [`Backstops::poll`]). Under infinite GOP the
        // only recovery keyframe is one we request; the reassembler drops unrecoverable AUs and the
        // decoder conceals the reference-missing deltas without error, so the gate arms on the
        // drop-count climb instead. `pending` holds an AU waiting for a free input buffer.
        backstops.poll(&client, &mut gate, fed, r + d > 0, pending.is_some(), 0);
    }

    let _ = codec.stop();
    drop(codec); // AMediaCodec_delete — after this no render callback can fire
    if let Some(ud) = render_cb {
        // SAFETY: the codec was dropped above; this registration's single reclaim.
        unsafe { release_render_callback(ud) };
    }
    log::info!("decode: stopped (fed={fed} rendered={rendered} discarded={discarded})");
}

/// Try to copy one access unit into a codec input buffer and queue it, without blocking. Returns
/// `false` only on `TryAgainLater` (no input buffer free) — the caller keeps the AU pending and
/// retries; a hard dequeue/queue error counts as consumed (retrying can't salvage the AU, and
/// parking it forever would wedge the loop on a broken codec). An AU larger than the input
/// buffer is DROPPED (+ a recovery keyframe requested), never truncated — a truncated AU is
/// corrupt input the decoder chews on silently, poisoning the reference chain.
fn feed(
    codec: &MediaCodec,
    client: &NativeClient,
    au: &[u8],
    pts_us: u64,
    oversized_dropped: &mut u64,
) -> bool {
    match codec.dequeue_input_buffer(Duration::ZERO) {
        Ok(DequeuedInputBufferResult::Buffer(mut buf)) => {
            let n = {
                let dst = buf.buffer_mut();
                if au.len() > dst.len() {
                    *oversized_dropped += 1;
                    log::warn!(
                        "decode: AU {} > input buffer {} — dropped ({} so far), requesting keyframe",
                        au.len(),
                        dst.len(),
                        *oversized_dropped
                    );
                    let _ = client.request_keyframe();
                    0 // return the slot with zero valid bytes — a no-op input, not corrupt data
                } else {
                    let n = au.len();
                    // SAFETY: `au` and `dst` are distinct allocations (wire AU vs. codec buffer),
                    // both valid for `n` bytes; `MaybeUninit<u8>` is layout-identical to `u8`, so
                    // the cast write initializes exactly `dst[..n]`.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            au.as_ptr(),
                            dst.as_mut_ptr().cast::<u8>(),
                            n,
                        );
                    }
                    n
                }
            };
            if let Err(e) = codec.queue_input_buffer(buf, 0, n, pts_us, 0) {
                log::warn!("decode: queue_input_buffer: {e}");
            }
            true
        }
        Ok(DequeuedInputBufferResult::TryAgainLater) => false, // caller keeps the AU pending
        Err(e) => {
            log::warn!("decode: dequeue_input_buffer: {e}");
            true
        }
    }
}

/// Dequeue every ready output buffer and present only the NEWEST (render = true), discarding the
/// rest (render = false) — when decode falls behind, a back-to-back burst of stale frames on glass
/// is worse than skipping straight to the freshest one (the Apple client's 1-slot newest-ready
/// ring, ported). `first_wait` is the timeout for the first dequeue only: zero normally, ~2 ms when
/// the caller's input is blocked so the loop waits on decoder progress instead of busy-spinning.
/// Returns `(rendered, discarded)`. Also reacts to `OutputFormatChanged` (which can interleave
/// between buffers — handled without losing the held buffer) to signal HDR on the Surface.
///
/// Each dequeued buffer is also the HUD's `decoded` measurement point (rendered or not — the frame
/// finished decoding either way): end-to-end = decoded + clock_offset − capture pts, and the
/// `decode` stage pairs the buffer's echoed presentationTimeUs back to the receipt stamp in
/// `in_flight` (single-clock local difference, no skew involved). The presented frame's
/// `(pts, decoded stamp)` is additionally parked in `tracker` for the OnFrameRendered callback —
/// the `display` stage's other endpoint.
#[allow(clippy::too_many_arguments)] // one call site; mirrors the async loop's present_ready
fn drain(
    codec: &MediaCodec,
    client: &NativeClient,
    measure_decode: bool,
    window: &NativeWindow,
    applied_ds: &mut Option<DataSpace>,
    first_wait: Duration,
    stats: &crate::stats::VideoStats,
    in_flight: &mut VecDeque<(u64, i128)>,
    clock_offset: i64,
    tracker: &DisplayTracker,
    gate: &mut ReanchorGate,
    recovery_flags: &mut VecDeque<(u64, u32)>,
) -> (u64, u64) {
    // Newest ready buffer so far (presented after the loop) with its HUD metadata —
    // `Some((pts_us, decoded_ns))` only while the HUD is visible. `held_present` is the freeze gate's
    // verdict for that newest buffer (`false` = a post-loss concealment to withhold).
    let mut held: Option<(OutputBuffer<'_>, Option<(u64, i128)>)> = None;
    let mut held_present = true;
    let mut discarded: u64 = 0;
    let mut wait = first_wait;
    loop {
        match codec.dequeue_output_buffer(wait) {
            Ok(DequeuedOutputBufferInfoResult::Buffer(buf)) => {
                // Only the first dequeue may block; later ones poll (wait == ZERO).
                wait = Duration::ZERO;
                // Fold every dequeued frame through the gate in pts (== decode) order — even the ones
                // the newest-wins policy discards — so the two-mark re-anchor count stays correct; the
                // verdict of the newest (last folded) buffer decides whether it reaches glass.
                let pts_us = buf.info().presentation_time_us().max(0) as u64;
                let flags = take_flags(recovery_flags, pts_us);
                held_present =
                    gate.on_decoded(flags, false, Instant::now()) == GateVerdict::Present;
                let meta = if stats.enabled() || measure_decode {
                    // The dequeue IS the sync loop's decoded-availability instant.
                    let decoded_ns = now_realtime_ns();
                    note_decoded_pts(
                        client,
                        measure_decode,
                        stats,
                        in_flight,
                        clock_offset,
                        pts_us,
                        decoded_ns,
                    );
                    // The tracker's `display` stage is a HUD concern — park only when visible.
                    stats.enabled().then_some((pts_us, decoded_ns))
                } else {
                    None
                };
                if let Some((stale, _)) = held.replace((buf, meta)) {
                    // A newer frame is ready — drop the held one without rendering.
                    if let Err(e) = codec.release_output_buffer(stale, false) {
                        log::warn!("decode: release_output_buffer(discard): {e}");
                    }
                    discarded += 1;
                    stats.note_skipped(1); // HUD `skipped` counter; no-op while hidden
                }
            }
            Ok(DequeuedOutputBufferInfoResult::OutputFormatChanged) => {
                // The decoder has parsed the SPS and now reports the stream's real colour signalling
                // (the AMediaCodec analogue of VideoToolbox's format description on the Apple client).
                // If it's HDR (BT.2020 PQ/HLG), tell the Surface so the compositor/display switch to
                // HDR; SDR streams leave the default dataspace alone. The decoder itself picks a
                // Main10 path from the SPS — no profile override needed. Keep looping (buffers
                // follow, and any held buffer stays held across this event).
                wait = Duration::ZERO;
                if let Some(ds) = hdr_dataspace(codec) {
                    if *applied_ds != Some(ds) {
                        match window.set_buffers_data_space(ds) {
                            Ok(()) => {
                                *applied_ds = Some(ds);
                                log::info!("decode: HDR stream → Surface dataspace {ds}");
                            }
                            Err(e) => log::warn!(
                                "decode: set_buffers_data_space({ds}) failed (non-fatal): {e}"
                            ),
                        }
                    }
                }
            }
            // TryAgainLater / OutputBuffersChanged — nothing more to dequeue now.
            Ok(_) => break,
            Err(e) => {
                log::warn!("decode: dequeue_output_buffer: {e}");
                break;
            }
        }
    }
    // Present the newest ready frame — UNLESS the gate is withholding it as a post-loss concealment,
    // in which case release it without rendering (the SurfaceView keeps the last rendered frame frozen
    // on glass) and count it as a discard rather than a display.
    let mut rendered = 0;
    if let Some((buf, meta)) = held {
        match codec.release_output_buffer(buf, held_present) {
            Ok(()) if held_present => {
                rendered = 1;
                if let Some((pts_us, decoded_ns)) = meta {
                    tracker.note_rendered(pts_us, decoded_ns, super::latency::now_realtime_ns());
                }
            }
            Ok(()) => discarded += 1, // held off the screen — awaiting a clean re-anchor
            Err(e) => log::warn!("decode: release_output_buffer: {e}"),
        }
    }
    (rendered, discarded)
}
