//! Packet bookkeeping for one track: gap filling, irregularity counting and clock drift.
//!
//! Pure logic fed with what WASAPI reports per packet, so it can be tested without hardware.

use serde::Serialize;

use crate::journal::DiscKind;
use crate::timeline::{gap_to_fill, sample_position_ms, samples_to_ms};

/// Flags of one captured packet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PacketFlags {
    pub discontinuity: bool,
    pub silent: bool,
    pub timestamp_error: bool,
}

/// What to do before storing a packet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Samples of silence to store first.
    pub fill: u64,
    /// Irregularities to journal, with the frame count they cover (0 when unknown).
    pub events: Vec<(DiscKind, u64)>,
}

/// Counters of one track. Serialized into the summary.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    pub packets: u64,
    pub captured_samples: u64,
    pub silent_packets: u64,
    pub data_discontinuities: u64,
    pub timestamp_errors: u64,
    /// True when the engine flagged a discontinuity on the very first packet. That flag is
    /// normal after a stream starts and is not counted as a lost buffer.
    pub start_discontinuity_ignored: bool,
    pub silence_fill_events: u64,
    pub silence_filled_samples: u64,
    pub max_packet_interval_ms: f64,
    pub max_abs_deviation_ms: f64,
    /// Sum of (timestamp step minus audio duration of the previous packet) over
    /// contiguous packets, in ms. Positive: the sample clock runs slower than the
    /// performance counter. Only meaningful for sources with real timestamps.
    pub residual_sum_ms: f64,
    /// Time covered by those contiguous steps, in ms.
    pub contiguous_ms: f64,
    #[serde(skip)]
    pub skew: Regression,
}

/// Streaming least-squares fit of `skew = a + b * t`, where `t` is the arrival time of a
/// packet and `skew` is arrival time minus the track position at the end of the packet.
#[derive(Debug, Clone, Default)]
pub struct Regression {
    n: f64,
    t0: f64,
    s0: f64,
    sum_t: f64,
    sum_s: f64,
    sum_tt: f64,
    sum_ts: f64,
}

impl Regression {
    pub fn add(&mut self, t: f64, skew: f64) {
        if self.n == 0.0 {
            self.t0 = t;
            self.s0 = skew;
        }
        let (t, s) = (t - self.t0, skew - self.s0);
        self.n += 1.0;
        self.sum_t += t;
        self.sum_s += s;
        self.sum_tt += t * t;
        self.sum_ts += t * s;
    }

    /// Number of points and span of the arrival times in ms.
    pub fn span_ms(&self) -> f64 {
        if self.n < 2.0 {
            0.0
        } else {
            // Mean-free estimate of the span; exact bounds are not kept.
            (12.0 * (self.sum_tt / self.n - (self.sum_t / self.n).powi(2))).sqrt()
        }
    }

    /// Slope of the skew against arrival time (ms per ms).
    pub fn slope(&self) -> Option<f64> {
        if self.n < 10.0 {
            return None;
        }
        let den = self.n * self.sum_tt - self.sum_t * self.sum_t;
        if den <= 0.0 {
            return None;
        }
        Some((self.n * self.sum_ts - self.sum_t * self.sum_s) / den)
    }
}

impl Stats {
    /// Sample clock rate relative to the performance counter, in parts per million, from
    /// the arrival times of packets. Positive means the source produces more audio than
    /// wall time elapsed. Needs at least a minute of data to be meaningful.
    pub fn clock_ratio_ppm(&self) -> Option<f64> {
        if self.skew.span_ms() < 60_000.0 {
            return None;
        }
        self.skew.slope().map(|b| -b * 1e6)
    }

    /// The same ratio from the timestamps the engine attaches to packets. Process loopback
    /// derives them from the sample count, so there it is zero by construction.
    pub fn timestamp_ratio_ppm(&self) -> Option<f64> {
        if self.contiguous_ms < 1000.0 {
            return None;
        }
        Some(-self.residual_sum_ms / self.contiguous_ms * 1e6)
    }

    /// Irregularities that mean audio was lost.
    pub fn dropped_buffers(&self) -> u64 {
        self.data_discontinuities
    }
}

/// Tracks the position of one track against the session clock.
#[derive(Debug, Clone)]
pub struct Tracker {
    rate: u32,
    offset_ms: f64,
    tolerance_ms: f64,
    written: u64,
    prev: Option<Prev>,
    pub stats: Stats,
}

#[derive(Debug, Clone, Copy)]
struct Prev {
    due_ms: Option<f64>,
    frames: u64,
}

impl Tracker {
    pub fn new(rate: u32, offset_ms: f64, tolerance_ms: f64) -> Self {
        Self {
            rate,
            offset_ms,
            tolerance_ms,
            written: 0,
            prev: None,
            stats: Stats::default(),
        }
    }

    /// Samples stored so far, including silence.
    #[cfg(test)]
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Session-clock position of the next sample to be stored.
    pub fn position_ms(&self) -> f64 {
        sample_position_ms(self.offset_ms, self.written, self.rate)
    }

    /// Register a packet of `frames` frames whose first frame is due at `due_ms` on the
    /// session clock (None when the timestamp is unavailable). `arrival_ms` is the session
    /// clock when the packet was read.
    pub fn on_packet(
        &mut self,
        due_ms: Option<f64>,
        frames: u64,
        arrival_ms: f64,
        flags: PacketFlags,
    ) -> Plan {
        let mut plan = Plan::default();
        self.stats.packets += 1;
        self.stats.captured_samples += frames;
        if flags.silent {
            self.stats.silent_packets += 1;
        }
        if flags.discontinuity {
            if self.stats.packets == 1 {
                self.stats.start_discontinuity_ignored = true;
            } else {
                self.stats.data_discontinuities += 1;
                plan.events.push((DiscKind::DataDiscontinuity, 0));
            }
        }
        if flags.timestamp_error {
            self.stats.timestamp_errors += 1;
            plan.events.push((DiscKind::TimestampError, 0));
        }
        let usable_ts = if flags.timestamp_error { None } else { due_ms };

        if let Some(due) = usable_ts {
            let fill = gap_to_fill(
                self.offset_ms,
                self.written,
                due,
                self.rate,
                self.tolerance_ms,
            );
            if fill > 0 {
                plan.fill = fill;
                self.stats.silence_fill_events += 1;
                self.stats.silence_filled_samples += fill;
                plan.events.push((DiscKind::SilenceFilled, fill));
                self.written += fill;
            }
            let deviation = due - self.position_ms();
            self.stats.max_abs_deviation_ms = self.stats.max_abs_deviation_ms.max(deviation.abs());
            if let Some((prev, prev_due)) = self.prev.and_then(|p| p.due_ms.map(|d| (p, d))) {
                let step = due - prev_due;
                let delta = step - samples_to_ms(prev.frames, self.rate);
                self.stats.max_packet_interval_ms = self.stats.max_packet_interval_ms.max(step);
                if delta.abs() < self.tolerance_ms && !flags.discontinuity {
                    self.stats.residual_sum_ms += delta;
                    self.stats.contiguous_ms += step;
                }
            }
        }

        self.written += frames;
        self.stats
            .skew
            .add(arrival_ms, arrival_ms - self.position_ms());
        self.prev = Some(Prev {
            due_ms: usable_ts,
            frames,
        });
        plan
    }

    /// Silence needed to bring the track up to `now_ms` when recording stops.
    pub fn on_stop(&mut self, now_ms: f64) -> Plan {
        let mut plan = Plan::default();
        let fill = gap_to_fill(
            self.offset_ms,
            self.written,
            now_ms,
            self.rate,
            self.tolerance_ms,
        );
        if fill > 0 {
            plan.fill = fill;
            plan.events.push((DiscKind::SilenceFilled, fill));
            self.stats.silence_fill_events += 1;
            self.stats.silence_filled_samples += fill;
            self.written += fill;
        }
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::SAMPLE_RATE;

    const FRAMES: u64 = 160; // 10 ms at 16 kHz

    fn flags() -> PacketFlags {
        PacketFlags::default()
    }

    /// Feed `n` packets whose timestamps and arrival advance by `step_ms` each.
    fn feed(t: &mut Tracker, n: u64, step_ms: f64) {
        for i in 0..n {
            let at = i as f64 * step_ms;
            t.on_packet(Some(at), FRAMES, at + 10.0, flags());
        }
    }

    #[test]
    fn contiguous_packets_need_no_fill() {
        let mut t = Tracker::new(SAMPLE_RATE, 0.0, 30.0);
        for i in 0..100u64 {
            let at = i as f64 * 10.0;
            let plan = t.on_packet(Some(at + 0.4), FRAMES, at + 10.5, flags());
            assert_eq!(plan, Plan::default());
        }
        assert_eq!(t.written(), 100 * FRAMES);
        assert_eq!(t.stats.dropped_buffers(), 0);
        assert_eq!(t.stats.silence_fill_events, 0);
    }

    #[test]
    fn gap_is_filled_and_journaled() {
        let mut t = Tracker::new(SAMPLE_RATE, 0.0, 30.0);
        t.on_packet(Some(0.0), FRAMES, 10.0, flags());
        // Next packet arrives 500 ms later than expected.
        let plan = t.on_packet(Some(510.0), FRAMES, 520.0, flags());
        assert_eq!(plan.fill, 8_000);
        assert_eq!(plan.events, vec![(DiscKind::SilenceFilled, 8_000)]);
        // After the fill the track sits at the packet's due time (510 ms) plus the packet.
        assert_eq!(t.written(), 8_160 + FRAMES);
        assert_eq!(t.stats.silence_fill_events, 1);
        assert_eq!(t.stats.dropped_buffers(), 0);
    }

    #[test]
    fn discontinuity_flag_counts_as_dropped_buffer() {
        let mut t = Tracker::new(SAMPLE_RATE, 0.0, 30.0);
        t.on_packet(Some(0.0), FRAMES, 10.0, flags());
        let f = PacketFlags {
            discontinuity: true,
            ..flags()
        };
        let plan = t.on_packet(Some(10.0), FRAMES, 20.0, f);
        assert_eq!(plan.events, vec![(DiscKind::DataDiscontinuity, 0)]);
        assert_eq!(t.stats.dropped_buffers(), 1);
    }

    #[test]
    fn discontinuity_on_first_packet_is_not_a_loss() {
        let mut t = Tracker::new(SAMPLE_RATE, 0.0, 30.0);
        let f = PacketFlags {
            discontinuity: true,
            ..flags()
        };
        let plan = t.on_packet(Some(0.0), FRAMES, 10.0, f);
        assert!(plan.events.is_empty());
        assert_eq!(t.stats.dropped_buffers(), 0);
        assert!(t.stats.start_discontinuity_ignored);
    }

    #[test]
    fn timestamp_error_skips_filling() {
        let mut t = Tracker::new(SAMPLE_RATE, 0.0, 30.0);
        let f = PacketFlags {
            timestamp_error: true,
            ..flags()
        };
        let plan = t.on_packet(Some(5_000.0), FRAMES, 10.0, f);
        assert_eq!(plan.fill, 0);
        assert_eq!(t.stats.timestamp_errors, 1);
        assert_eq!(t.written(), FRAMES);
    }

    #[test]
    fn silent_packets_are_counted_not_lost() {
        let mut t = Tracker::new(SAMPLE_RATE, 0.0, 30.0);
        let f = PacketFlags {
            silent: true,
            ..flags()
        };
        t.on_packet(Some(0.0), FRAMES, 10.0, f);
        assert_eq!(t.stats.silent_packets, 1);
        assert_eq!(t.stats.dropped_buffers(), 0);
    }

    #[test]
    fn timestamp_drift_of_a_slow_sample_clock_is_measured() {
        // 160 frames every 10.001 ms of reference time: audio runs 100 ppm slow.
        let mut t = Tracker::new(SAMPLE_RATE, 0.0, 30.0);
        feed(&mut t, 2_000, 10.001);
        let ppm = t.stats.timestamp_ratio_ppm().unwrap();
        assert!((ppm + 100.0).abs() < 1.0, "ppm = {ppm}");
    }

    #[test]
    fn arrival_drift_is_measured_over_a_minute() {
        // A fast sample clock: 160 frames arrive every 9.999 ms, 100 ppm fast.
        let mut t = Tracker::new(SAMPLE_RATE, 0.0, 30.0);
        for i in 0..7_000u64 {
            let at = i as f64 * 9.999;
            // Timestamps unavailable, only arrival times.
            t.on_packet(None, FRAMES, at + 10.0, flags());
        }
        let ppm = t.stats.clock_ratio_ppm().unwrap();
        assert!((ppm - 100.0).abs() < 2.0, "ppm = {ppm}");
    }

    #[test]
    fn drift_needs_enough_data() {
        let mut t = Tracker::new(SAMPLE_RATE, 0.0, 30.0);
        feed(&mut t, 100, 10.0);
        assert!(t.stats.clock_ratio_ppm().is_none());
        assert!(t.stats.timestamp_ratio_ppm().is_none());
    }

    #[test]
    fn stop_fills_to_the_clock() {
        let mut t = Tracker::new(SAMPLE_RATE, 100.0, 30.0);
        t.on_packet(Some(100.0), FRAMES, 110.0, flags());
        let plan = t.on_stop(1_110.0);
        // The track should cover 1010 ms = 16160 samples, 160 are stored.
        assert_eq!(plan.fill, 16_000);
        assert_eq!(t.written(), 16_160);
        // Already up to date: nothing more.
        assert_eq!(t.on_stop(1_110.0), Plan::default());
    }
}
