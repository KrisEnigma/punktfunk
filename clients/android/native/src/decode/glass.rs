//! Follow the glass: a stream refresh the panel never shows is decode work for nothing. Once the
//! panel has settled below the stream's rate for long enough, the session asks the host for the
//! rate the glass actually runs — half the decode, half the bits, for the same picture.
//!
//! Why the panel sits low at all: with a "Dynamic" refresh setting the platform's idle timer
//! drops the panel to its floor after ~3 s without touch, and that idle signal outranks every
//! frame-rate vote the app makes (observed on the A024 with SurfaceFlinger's own dump). Only the
//! user's "High" setting pins it, and that is theirs to set — this follows whichever they chose.

use std::time::{Duration, Instant};

use punktfunk_core::client::NativeClient;
use punktfunk_core::config::Mode;

/// Seconds the panel must sit below the stream's rate before the switch. Past the idle timer,
/// and long enough that touch bursts (which lift an idle panel for a moment) reset it.
const SETTLE_SECS: u32 = 8;
/// A panel is slower than the stream at ≤ ⅔ of its rate: 60 under 90 or 120 qualifies, 90
/// under 120 does not (no whole pulldown to gain).
const SLOW_NUM: i64 = 3;
const SLOW_DEN: i64 = 2;
/// The learned grid is only the panel's when frames were offered at ≥ ¾ of the stream rate:
/// a source at half rate latches every other vsync and reads as a panel at half rate.
const OFFERED_NUM: u64 = 3;
const OFFERED_DEN: u64 = 4;

pub(super) struct GlassFollower {
    slow_secs: u32,
    /// One switch per session: the next session re-reads the glass from scratch.
    decided: bool,
    last_tick: Instant,
    /// Stream refresh the presenter was told; `tick` reports a change so it can re-tune.
    stream_hz: u32,
}

impl GlassFollower {
    pub(super) fn new(stream_hz: u32) -> Self {
        GlassFollower {
            slow_secs: 0,
            decided: false,
            last_tick: Instant::now(),
            stream_hz,
        }
    }

    /// One sample of the measured panel period (0 = unknown) and the frames offered to glass in
    /// that second; self-limits to 1 Hz. Returns the stream refresh whenever it differs from
    /// the last call — a mode switch landed.
    pub(super) fn tick(
        &mut self,
        client: &NativeClient,
        panel_ns: i64,
        offered: u64,
    ) -> Option<u32> {
        if self.last_tick.elapsed() < Duration::from_millis(900) {
            return None;
        }
        self.last_tick = Instant::now();
        let mode = client.mode();
        let changed = (mode.refresh_hz != self.stream_hz).then_some(mode.refresh_hz);
        self.stream_hz = mode.refresh_hz;
        if self.decided || panel_ns <= 0 || mode.refresh_hz == 0 {
            return changed;
        }
        let stream_ns = 1_000_000_000 / i64::from(mode.refresh_hz);
        let offered_full = offered * OFFERED_DEN >= u64::from(mode.refresh_hz) * OFFERED_NUM;
        if !offered_full || panel_ns * SLOW_DEN < stream_ns * SLOW_NUM {
            self.slow_secs = 0;
            return changed;
        }
        self.slow_secs += 1;
        if self.slow_secs < SETTLE_SECS {
            return changed;
        }
        self.decided = true;
        let hz = snap_refresh_hz(1_000_000_000.0 / panel_ns as f64);
        if hz == 0 || hz >= mode.refresh_hz {
            return changed;
        }
        log::info!(
            "decode: the panel settled at {hz} Hz under a {} Hz stream for {SETTLE_SECS} s — \
             following the glass",
            mode.refresh_hz
        );
        if let Err(e) = client.request_mode(Mode {
            width: mode.width,
            height: mode.height,
            refresh_hz: hz,
        }) {
            log::warn!("decode: mode request for the panel's {hz} Hz did not go out: {e}");
        }
        changed
    }
}

/// Panels advertise round rates; a learned period does not (16.4 ms reads as 61 Hz, and the
/// host would build a 61 Hz display). Snap to the nearest common rate within 3 %.
fn snap_refresh_hz(measured: f64) -> u32 {
    const COMMON: [u32; 12] = [24, 30, 48, 50, 60, 72, 75, 90, 100, 120, 144, 240];
    COMMON
        .into_iter()
        .find(|&hz| (measured - f64::from(hz)).abs() <= f64::from(hz) * 0.03)
        .unwrap_or(measured.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_learned_period_snaps_to_the_panel_rate() {
        assert_eq!(snap_refresh_hz(1e9 / 16_400_000.0), 60); // 60.98
        assert_eq!(snap_refresh_hz(59.2), 60);
        assert_eq!(snap_refresh_hz(118.9), 120);
        assert_eq!(snap_refresh_hz(83.0), 83); // nothing common nearby: keep it
    }

    #[test]
    fn slow_ratio_needs_a_whole_pulldown() {
        let slow = |panel_hz: i64, stream_hz: i64| {
            let panel_ns = 1_000_000_000 / panel_hz;
            let stream_ns = 1_000_000_000 / stream_hz;
            panel_ns * SLOW_DEN >= stream_ns * SLOW_NUM
        };
        assert!(slow(60, 120));
        assert!(slow(60, 90));
        assert!(!slow(90, 120));
        assert!(!slow(120, 120));
    }
}
