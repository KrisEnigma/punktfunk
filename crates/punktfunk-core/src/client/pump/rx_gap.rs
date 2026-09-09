//! Splits a receive hole into its two causes: no datagram reached the socket, or this
//! thread stopped asking. The receive loop polls every ~300 µs, so the longest poll-to-poll
//! interval inside a silence is how long the client itself was away.

use std::time::{Duration, Instant};

/// Shortest silence worth a line. 100 ms is 12 frames at 120 Hz and past any pacing jitter.
const SILENCE_FLOOR: Duration = Duration::from_millis(100);
/// A silence must also be this many mean intervals of the last live second, so a host that
/// sends little while idle does not trip the floor.
const SILENCE_INTERVALS: u32 = 10;
/// A second below this rate teaches no cadence; the last one at or above it stays in force.
const MIN_PPS: u64 = 30;
const REPORT_MIN_GAP: Duration = Duration::from_secs(1);

/// One silence that ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GapReport {
    /// No datagram reached the socket for this long.
    pub silence_ms: u32,
    /// Longest stretch inside that silence with no poll from this thread.
    pub unpolled_ms: u32,
    /// Datagrams the first poll after the silence returned.
    pub burst: u32,
}

pub(super) struct RxGap {
    last_rx: Instant,
    last_poll: Instant,
    unpolled_max: Duration,
    seen: u64,
    window_start: Instant,
    window_count: u64,
    /// Datagrams in the last full second that carried a live stream.
    cadence: Option<u64>,
    last_report: Option<Instant>,
}

impl RxGap {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            last_rx: now,
            last_poll: now,
            unpolled_max: Duration::ZERO,
            seen: 0,
            window_start: now,
            window_count: 0,
            cadence: None,
            last_report: None,
        }
    }

    /// Once per loop pass with the running datagram count. `Some` when a silence just ended
    /// and deserves a line; the poll that ended it counts toward `unpolled_ms`.
    pub(super) fn observe(&mut self, now: Instant, packets_received: u64) -> Option<GapReport> {
        let poll_gap = now.saturating_duration_since(self.last_poll);
        self.last_poll = now;
        self.unpolled_max = self.unpolled_max.max(poll_gap);
        if now.saturating_duration_since(self.window_start) >= Duration::from_secs(1) {
            if self.window_count >= MIN_PPS {
                self.cadence = Some(self.window_count);
            }
            self.window_count = 0;
            self.window_start = now;
        }
        let new = packets_received.saturating_sub(self.seen);
        if new == 0 {
            return None;
        }
        self.seen = packets_received;
        self.window_count += new;
        let silence = now.saturating_duration_since(self.last_rx);
        self.last_rx = now;
        let unpolled = std::mem::take(&mut self.unpolled_max);
        let pps = self.cadence?;
        let mean_interval = Duration::from_secs(1) / pps.min(u32::MAX as u64) as u32;
        let threshold = SILENCE_FLOOR.max(mean_interval * SILENCE_INTERVALS);
        if silence < threshold {
            return None;
        }
        if self
            .last_report
            .is_some_and(|t| now.saturating_duration_since(t) < REPORT_MIN_GAP)
        {
            return None;
        }
        self.last_report = Some(now);
        Some(GapReport {
            silence_ms: silence.as_millis().min(u32::MAX as u128) as u32,
            unpolled_ms: unpolled.as_millis().min(u32::MAX as u128) as u32,
            burst: new.min(u32::MAX as u64) as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// One steady second at 120 datagrams/s so the cadence is known; returns the clock.
    fn warmed(g: &mut RxGap, t0: Instant) -> (Instant, u64) {
        let mut count = 0;
        for i in 1..=125u64 {
            count += 1;
            assert!(g.observe(t0 + ms(i * 8), count).is_none());
        }
        (t0 + ms(1000), count)
    }

    #[test]
    fn a_silence_the_thread_polled_through_reads_as_nothing_arrived() {
        let t0 = Instant::now();
        let mut g = RxGap::new(t0);
        let (t, mut count) = warmed(&mut g, t0);
        for i in 1..=200u64 {
            assert!(g.observe(t + ms(i), count).is_none());
        }
        count += 3;
        let r = g
            .observe(t + ms(201), count)
            .expect("a 200 ms silence reports");
        assert_eq!(r.silence_ms, 201);
        assert_eq!(r.unpolled_ms, 1);
        assert_eq!(r.burst, 3);
    }

    #[test]
    fn a_silence_with_no_polls_reads_as_this_client_stalled() {
        let t0 = Instant::now();
        let mut g = RxGap::new(t0);
        let (t, mut count) = warmed(&mut g, t0);
        count += 40;
        let r = g
            .observe(t + ms(300), count)
            .expect("a 300 ms silence reports");
        assert_eq!(r.silence_ms, 300);
        assert_eq!(r.unpolled_ms, 300);
        assert_eq!(r.burst, 40);
    }

    #[test]
    fn a_quiet_host_never_trips_the_floor() {
        let t0 = Instant::now();
        let mut g = RxGap::new(t0);
        // 4 datagrams/s for two seconds: below MIN_PPS, so 250 ms gaps are cadence.
        let mut count = 0;
        for i in 1..=8u64 {
            count += 1;
            assert!(g.observe(t0 + ms(i * 250), count).is_none());
        }
    }

    #[test]
    fn reports_are_at_most_one_per_second() {
        let t0 = Instant::now();
        let mut g = RxGap::new(t0);
        let (t, mut count) = warmed(&mut g, t0);
        count += 1;
        assert!(g.observe(t + ms(150), count).is_some());
        count += 1;
        assert!(
            g.observe(t + ms(400), count).is_none(),
            "second hole inside 1 s is folded"
        );
        count += 1;
        assert!(g.observe(t + ms(1600), count).is_some());
    }
}
