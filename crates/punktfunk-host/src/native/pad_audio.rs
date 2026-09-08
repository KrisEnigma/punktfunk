//! Per-pad DualSense audio (wire `0xD1`).
//!
//! Capture of the pad's own device — WASAPI loopback ([`crate::audio::pad_endpoint`]) on
//! Windows, the per-pad PipeWire sink (`crate::audio::pad_sink`) on Linux — is de-interleaved
//! into speaker (front) and voice-coil (back) pairs, silence-gated, Opus-encoded at 48 kHz
//! CBR, and sent as [`PAD_AUDIO_MAGIC`](punktfunk_core::quic::PAD_AUDIO_MAGIC) datagrams.
//!
//! One thread per pad, spawned and reaped by [`super::input`]. Capture death reopens with
//! backoff; seq stays monotonic across reopens so the client sees a gap, not a restart.

use super::*;

/// The shared pipeline; a host with no pad source has no use for it.
#[cfg(any(target_os = "windows", target_os = "linux", test))]
mod engine;
#[cfg(target_os = "windows")]
#[path = "pad_audio/windows.rs"]
mod plat;
#[cfg(target_os = "linux")]
#[path = "pad_audio/linux.rs"]
mod plat;
/// Other hosts have no virtual DualSense audio source; [`host_cap`] never advertises the cap.
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod plat {
    use super::*;
    pub(super) fn host_cap(_asked: bool) -> bool {
        false
    }
    pub(in crate::native) fn spawn(
        _conn: super::super::link::SessionLink,
        _pad: u8,
        _slot: u8,
        _kinds: u8,
        _edge: bool,
        _stop: Arc<AtomicBool>,
    ) -> Option<PadAudioHandle> {
        None
    }
}
pub(super) use plat::spawn;

/// [`stop`](PadAudioHandle::stop) flags and joins; [`signal`](PadAudioHandle::signal) only flags
/// so the input thread can overlap joins instead of serializing the ~5 s quiet-endpoint timeout.
pub(super) struct PadAudioHandle {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl PadAudioHandle {
    pub(super) fn signal(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Bounded by the capturer's ~5 s quiet-endpoint recv. Mid-session reaps go through a detached
    /// reaper (`input.rs::PadAudioSlots::stop`); session teardown joins inline (10 s grace).
    pub(super) fn stop(mut self) {
        self.reap();
    }

    fn reap(&mut self) {
        self.signal();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Fallback if `stop()` never ran (reaper-spawn failure).
impl Drop for PadAudioHandle {
    fn drop(&mut self) {
        self.reap();
    }
}

/// Advertise [`HOST_CAP_PAD_AUDIO`](punktfunk_core::quic::HOST_CAP_PAD_AUDIO) when the client asked
/// ([`CLIENT_CAP_PAD_AUDIO`](punktfunk_core::quic::CLIENT_CAP_PAD_AUDIO)), `PUNKTFUNK_PAD_AUDIO` ≠ "0",
/// and a source exists: Windows has a provisioned endpoint; Linux has a reachable PipeWire daemon
/// (sinks mint lazily at spawn).
pub(super) fn host_cap(client_caps: u8) -> bool {
    plat::host_cap(client_caps & punktfunk_core::quic::CLIENT_CAP_PAD_AUDIO != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_cap_requires_the_client_bit() {
        // Without CLIENT_CAP_PAD_AUDIO the answer is no on every platform (Windows env +
        // provisioning legs are environment-dependent — not unit-tested here).
        assert!(!host_cap(0));
        assert!(!host_cap(punktfunk_core::quic::CLIENT_CAP_CURSOR));
    }
}
