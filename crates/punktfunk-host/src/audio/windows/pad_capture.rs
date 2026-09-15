//! WASAPI loopback of one pad's own render endpoint, plus the tone and probe that exercise it.
//!
//! The pad endpoints are minted by [`super::pad_endpoint`]; capturing from one is a different
//! job, and the same one [`super::wasapi_cap`] does for the desktop's default render device.
//! Same COM discipline as that file: the WASAPI objects live on a dedicated thread and the
//! struct holds channel + stop + join, so a device error ends the thread and the caller reopens.

use super::pad_endpoint::{open_wasapi_device, probe_activation};
use super::{AudioCapturer, SAMPLE_RATE};
use anyhow::{anyhow, Context, Result};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use wasapi::{Direction, SampleType, StreamMode, WaveFormat};

pub const PAD_CHANNELS: u32 = 4;
/// 4-ch pad layout (FL FR BL BR). Not `punktfunk_core::audio::wasapi_channel_mask`,
/// which only speaks GameStream stereo/5.1/7.1.
pub(super) const PAD_CHANNEL_MASK: u32 = 0x33;
const PAD_BLOCK_ALIGN: usize = PAD_CHANNELS as usize * 4;

/// WASAPI loopback of one pad endpoint: interleaved 4-ch f32 at 48 kHz.
/// Same COM discipline as [`super::wasapi_cap`]: WASAPI objects live on a
/// dedicated thread; the struct holds channel + stop + join. A device error
/// ends the thread — [`AudioCapturer::next_chunk`] returns `Err` and the caller reopens.
pub struct PadLoopbackCapturer {
    chunks: Receiver<Vec<f32>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl PadLoopbackCapturer {
    pub fn open(endpoint_id: &str) -> Result<PadLoopbackCapturer> {
        let (tx, rx) = sync_channel::<Vec<f32>>(64);
        let stop = Arc::new(AtomicBool::new(false));
        // Surface an open failure as Err (caller retries), never a silent dead thread.
        let (ready_tx, ready_rx) = sync_channel::<Result<()>>(1);
        let (stop_t, id) = (stop.clone(), endpoint_id.to_string());
        let join = thread::Builder::new()
            .name("punktfunk-pad-cap".into())
            .spawn(move || {
                if let Err(e) = pad_capture_thread(&id, tx, stop_t, ready_tx) {
                    tracing::error!(error = %format!("{e:#}"), "pad loopback thread failed");
                }
            })
            .context("spawn pad loopback thread")?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(PadLoopbackCapturer {
                chunks: rx,
                stop,
                join: Some(join),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                // Signal and reap. Dropping `join` leaked one WASAPI thread per
                // ~2 s reopen. If it is stuck in a blocking call, fail rather
                // than leak.
                stop.store(true, Ordering::SeqCst);
                match reap_with_timeout(join, Duration::from_secs(2)) {
                    true => Err(anyhow!("pad loopback init timed out")),
                    false => Err(anyhow!(
                        "pad loopback init timed out and its thread did not exit — the audio \
                         stack is wedged; not retrying into a thread leak"
                    )),
                }
            }
        }
    }
}

fn reap_with_timeout(join: JoinHandle<()>, budget: Duration) -> bool {
    let deadline = std::time::Instant::now() + budget;
    while !join.is_finished() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = join.join();
    true
}

/// Channel pair for [`render_test_tone`]. Front = pad speaker (FL FR);
/// Back = voice coils (BL BR). Driving one pair and silencing the other
/// is how a result names which kind the framer routed.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TonePair {
    Front,
    Back,
    Both,
}

impl TonePair {
    /// `--pair` argument; anything unrecognised keeps the haptics (Back) default.
    pub(crate) fn parse(s: &str) -> TonePair {
        match s {
            "front" | "speaker" => TonePair::Front,
            "both" => TonePair::Both,
            _ => TonePair::Back,
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            TonePair::Front => "FRONT pair (the pad's speaker)",
            TonePair::Back => "BACK pair (the voice coils)",
            TonePair::Both => "BOTH pairs (speaker + voice coils)",
        }
    }
    fn carries(self, c: usize) -> bool {
        match self {
            TonePair::Front => c < 2,
            TonePair::Back => c >= 2,
            TonePair::Both => true,
        }
    }
}

/// Render a test tone into a pad endpoint. Default BACK (voice coils) so a
/// pass is felt in the grips and cannot be the speaker; `--pair front` is
/// the speaker kind without a game.
pub(crate) fn render_test_tone(
    endpoint_id: &str,
    seconds: u32,
    hz: f32,
    pair: TonePair,
) -> Result<()> {
    wasapi::initialize_mta()
        .ok()
        .context("initialize COM (MTA) for the tone render")?;

    probe_activation(endpoint_id);
    // By id, never a default-device resolve: the question is whether THIS endpoint is heard.
    let device = open_wasapi_device(endpoint_id)
        .with_context(|| format!("pad endpoint {endpoint_id} not found"))?;
    let mut audio_client = device.get_iaudioclient().context("IAudioClient")?;
    // Same mask as the endpoint and loopback. `None` lets wasapi derive
    // `(1 << 4) - 1` = 0x0F (FL FR FC LFE) instead of 0x33 (FL FR BL BR).
    let desired = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        SAMPLE_RATE as usize,
        PAD_CHANNELS as usize,
        Some(PAD_CHANNEL_MASK),
    );
    let (default_period, _min) = audio_client.get_device_period().context("device period")?;
    audio_client
        .initialize_client(
            &desired,
            &Direction::Render,
            &StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: default_period,
            },
        )
        .context("initialize render client")?;
    let h_event = audio_client.set_get_eventhandle().context("event handle")?;
    let render = audio_client
        .get_audiorenderclient()
        .context("IAudioRenderClient")?;
    let buf_frames = audio_client.get_buffer_size().context("buffer size")? as usize;
    let block = PAD_CHANNELS as usize * std::mem::size_of::<f32>();
    // Start on silence so the stream opens without a glitch, as the mic pump does.
    let _ = render.write_to_device(buf_frames, &vec![0u8; buf_frames * block], None);
    audio_client.start_stream().context("start render stream")?;

    let total = u64::from(SAMPLE_RATE) * u64::from(seconds.clamp(1, 60));
    let step = std::f32::consts::TAU * hz / SAMPLE_RATE as f32;
    let mut phase = 0.0f32;
    let mut written = 0u64;
    let mut bytes = vec![0u8; buf_frames * block];

    while written < total {
        if h_event.wait_for_event(1000).is_err() {
            anyhow::bail!("render event timed out after {written} frames");
        }
        let free = audio_client
            .get_available_space_in_frames()
            .context("available space")? as usize;
        let n = free.min((total - written) as usize);
        if n == 0 {
            continue;
        }
        for f in 0..n {
            let s = phase.sin() * 0.5;
            phase += step;
            if phase >= std::f32::consts::TAU {
                phase -= std::f32::consts::TAU;
            }
            for c in 0..PAD_CHANNELS as usize {
                let v: f32 = if pair.carries(c) { s } else { 0.0 };
                let at = (f * PAD_CHANNELS as usize + c) * 4;
                bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        render
            .write_to_device(n, &bytes[..n * block], None)
            .context("write tone")?;
        written += n as u64;
    }
    // Let the tail drain before tearing the stream down.
    std::thread::sleep(Duration::from_millis(200));
    let _ = audio_client.stop_stream();
    Ok(())
}

/// Open the real loopback on a pad endpoint and report peaks by pair.
/// Run with [`render_test_tone`]: BACK-only is the 0xD1 coil signal;
/// front energy means pair routing is wrong; both silent means the
/// endpoint carries no audio.
pub(crate) fn capture_probe(endpoint_id: &str, seconds: u32) -> Result<()> {
    let mut cap = PadLoopbackCapturer::open(endpoint_id)
        .with_context(|| format!("open pad loopback on {endpoint_id}"))?;
    let deadline = Instant::now() + Duration::from_secs(u64::from(seconds.clamp(1, 60)));
    let (mut frames, mut peak_front, mut peak_back) = (0u64, 0f32, 0f32);
    while Instant::now() < deadline {
        let chunk = cap.next_chunk().context("read pad loopback")?;
        for f in chunk.chunks_exact(PAD_CHANNELS as usize) {
            frames += 1;
            peak_front = peak_front.max(f[0].abs()).max(f[1].abs());
            peak_back = peak_back.max(f[2].abs()).max(f[3].abs());
        }
    }
    println!(
        "pad-endpoint capture: {frames} frames over {seconds}s, peak_front={peak_front:.4} \
         peak_back={peak_back:.4}"
    );
    // Report what arrived, not a haptics-shaped verdict: `--pair front` is a pass too.
    const FLOOR: f32 = 0.0001;
    match (frames, peak_front > FLOOR, peak_back > FLOOR) {
        (0, _, _) => println!("  VERDICT: FAIL — the capture opened but delivered nothing."),
        (_, false, false) => println!(
            "  VERDICT: silent — capture works, but nothing was rendered. Run `pad-endpoint \
             tone` against this endpoint at the same time."
        ),
        (_, false, true) => println!(
            "  VERDICT: BACK pair only (the voice coils), front silent — channel-exact for \
             haptics."
        ),
        (_, true, false) => println!(
            "  VERDICT: FRONT pair only (the pad's speaker), back silent — channel-exact for \
             the speaker."
        ),
        (_, true, true) => println!(
            "  VERDICT: BOTH pairs carry signal — correct for `--pair both`, otherwise the pairs \
             are leaking into each other."
        ),
    }
    Ok(())
}

impl Drop for PadLoopbackCapturer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl AudioCapturer for PadLoopbackCapturer {
    fn next_chunk(&mut self) -> Result<Vec<f32>> {
        match self.chunks.recv_timeout(Duration::from_secs(5)) {
            Ok(c) => Ok(c),
            // Quiet pad is not a failure — empty chunk, keep the capturer. Err
            // is a dead capture thread (device invalidated); the caller reopens.
            Err(RecvTimeoutError::Timeout) => Ok(Vec::new()),
            Err(RecvTimeoutError::Disconnected) => Err(anyhow!("pad loopback thread ended")),
        }
    }
    fn channels(&self) -> u32 {
        PAD_CHANNELS
    }
    fn drain(&mut self) {
        while self.chunks.try_recv().is_ok() {}
    }
}

fn pad_capture_thread(
    endpoint_id: &str,
    tx: SyncSender<Vec<f32>>,
    stop: Arc<AtomicBool>,
    ready: SyncSender<Result<()>>,
) -> Result<()> {
    if let Err(e) = wasapi::initialize_mta()
        .ok()
        .context("CoInitializeEx (MTA)")
    {
        let _ = ready.send(Err(e));
        return Ok(());
    }
    // By id, never a default-device resolve. Shared-mode autoconvert so the
    // engine hands us 48 kHz 4-ch f32 regardless of mix format. Capture on a
    // render device in shared mode is WASAPI loopback.
    let setup = (|| -> Result<(wasapi::AudioClient, wasapi::AudioCaptureClient, wasapi::Handle)> {
        let device = open_wasapi_device(endpoint_id)
            .map_err(|e| anyhow!("open pad endpoint {endpoint_id}: {e:#}"))?;
        let mut audio_client = device.get_iaudioclient().context("IAudioClient")?;
        let desired = WaveFormat::new(
            32,
            32,
            &SampleType::Float,
            SAMPLE_RATE as usize,
            PAD_CHANNELS as usize,
            Some(PAD_CHANNEL_MASK),
        );
        let (default_period, _min) = audio_client.get_device_period().context("device period")?;
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: default_period,
        };
        audio_client
            .initialize_client(&desired, &Direction::Capture, &mode)
            .map_err(|e| {
                // The engine configures the driver with the stamped 4-ch device format; a
                // driver that refuses it fails every open here with UNSUPPORTED_FORMAT.
                let mix = audio_client
                    .get_mixformat()
                    .map(|f| {
                        format!(
                            "{}ch/{}Hz/{}bit",
                            f.get_nchannels(),
                            f.get_samplespersec(),
                            f.get_bitspersample()
                        )
                    })
                    .unwrap_or_else(|_| "unknown".into());
                anyhow!(
                    "initialize pad loopback client (endpoint mix format {mix}, asked \
                     {PAD_CHANNELS}ch/{SAMPLE_RATE}Hz): {e}"
                )
            })?;
        let h_event = audio_client.set_get_eventhandle().context("event handle")?;
        let capture_client = audio_client
            .get_audiocaptureclient()
            .context("IAudioCaptureClient")?;
        audio_client.start_stream().context("start pad loopback")?;
        Ok((audio_client, capture_client, h_event))
    })();
    let (audio_client, capture_client, h_event) = match setup {
        Ok(t) => t,
        Err(e) => {
            let _ = ready.send(Err(anyhow!("{e:#}")));
            return Ok(());
        }
    };
    let _ = ready.send(Ok(()));
    tracing::info!(endpoint = %endpoint_id, "pad loopback capturing (4 ch / 48 kHz f32)");

    // Endpoint invalidated or engine restart ends the thread; next_chunk Err, caller reopens.
    let mut bytes: VecDeque<u8> = VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        // Loopback fires events only while a game renders; the timeout keeps `stop` responsive.
        let _ = h_event.wait_for_event(100);
        loop {
            match capture_client.get_next_packet_size() {
                Ok(Some(0)) | Ok(None) => break,
                Ok(Some(_n)) => {
                    capture_client
                        .read_from_device_to_deque(&mut bytes)
                        .context("read pad loopback")?;
                }
                Err(e) => return Err(anyhow!("get_next_packet_size: {e}")),
            }
        }
        let whole = (bytes.len() / PAD_BLOCK_ALIGN) * PAD_BLOCK_ALIGN;
        if whole > 0 {
            let raw: Vec<u8> = bytes.drain(..whole).collect();
            let mut samples = Vec::with_capacity(whole / 4);
            for c in raw.chunks_exact(4) {
                samples.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
            let _ = tx.try_send(samples); // non-blocking, lossy — crate capture discipline
        }
    }
    audio_client.stop_stream().ok();
    Ok(())
}
