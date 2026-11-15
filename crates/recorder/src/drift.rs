//! Clock drift correction for sources with real packet timestamps (microphone, system loopback).
//!
//! A capture device has its own sample clock. Against the session clock (the performance
//! counter) it runs a few tens of ppm fast or slow, so a long track slowly slides against the
//! others. The corrector measures the ratio between the performance counter and the device
//! sample count from the packet timestamps and resamples the stream by that ratio with linear
//! interpolation, so that one stored sample is always 1/16 ms of session time.

use crate::timeline::SAMPLE_RATE;

/// Device time that must be observed before the ratio is trusted.
const WARMUP_MS: f64 = 30_000.0;
/// A timestamp this far from the straight-line model means a stall or a loss: measure again.
const RESET_ERROR_MS: f64 = 30.0;
/// Largest correction applied, in parts per million.
const MAX_PPM: f64 = 5_000.0;
/// Time constant of the position trim: an error of 1 ms changes the step by 1/TRIM_MS.
const TRIM_MS: f64 = 60_000.0;
/// Smoothing of the position error per packet (about one second at 10 ms packets).
const ERROR_ALPHA: f64 = 0.01;

/// Streaming resampler with a slowly updated step.
#[derive(Debug, Clone)]
pub struct Corrector {
    enabled: bool,
    /// Input samples consumed per output sample from the long-term ratio (1.0 = none).
    step: f64,
    /// Smoothed difference between session time and output position, in ms. It trims
    /// the step so that slow changes of the device rate do not accumulate.
    error_ms: f64,
    /// Session time of output sample 0 and output samples produced so far.
    origin: Option<f64>,
    n_out: u64,
    /// Session time and input count at the start of the current measurement.
    base: Option<(f64, u64)>,
    /// Input samples received so far.
    n_in: u64,
    /// Fractional read position in input samples.
    pos: f64,
    /// Input samples from index `tail_base` on, kept for interpolation.
    tail: Vec<i16>,
    tail_base: u64,
}

impl Corrector {
    /// Create a corrector. When `enabled` is false samples pass through unchanged.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            step: 1.0,
            error_ms: 0.0,
            origin: None,
            n_out: 0,
            base: None,
            n_in: 0,
            pos: 0.0,
            tail: Vec::new(),
            tail_base: 0,
        }
    }

    /// Current correction in ppm: positive when the device runs fast and samples are dropped.
    pub fn correction_ppm(&self) -> f64 {
        (self.effective_step() - 1.0) * 1e6
    }

    fn effective_step(&self) -> f64 {
        (self.step - self.error_ms / TRIM_MS).clamp(1.0 - MAX_PPM * 1e-6, 1.0 + MAX_PPM * 1e-6)
    }

    /// Track the difference between session time and the output position.
    fn update_error(&mut self, due_ms: f64) {
        let origin = *self.origin.get_or_insert(due_ms);
        let position = origin + self.n_out as f64 * 1000.0 / f64::from(SAMPLE_RATE);
        let error = due_ms - position;
        if error.abs() > RESET_ERROR_MS {
            // A stall or loss: the track is realigned elsewhere, start from here.
            self.origin = Some(due_ms - self.n_out as f64 * 1000.0 / f64::from(SAMPLE_RATE));
            self.error_ms = 0.0;
        } else {
            self.error_ms += ERROR_ALPHA * (error - self.error_ms);
        }
    }

    fn update_step(&mut self, due_ms: f64) {
        let Some((due0, n0)) = self.base else {
            self.base = Some((due_ms, self.n_in));
            return;
        };
        let device_ms = (self.n_in - n0) as f64 * 1000.0 / f64::from(SAMPLE_RATE);
        let wall_ms = due_ms - due0;
        // Straight-line model with the current step: wall time per input sample is 1/step.
        let expected = device_ms / self.step;
        if (wall_ms - expected).abs() > RESET_ERROR_MS {
            self.base = Some((due_ms, self.n_in));
            return;
        }
        if device_ms >= WARMUP_MS && wall_ms > 0.0 {
            let step = device_ms / wall_ms;
            self.step = step.clamp(1.0 - MAX_PPM * 1e-6, 1.0 + MAX_PPM * 1e-6);
        }
    }

    /// Feed one packet whose first frame is at `due_ms` on the session clock and append the
    /// corrected samples to `out`. Without timestamps or when disabled, samples pass through.
    pub fn process(&mut self, due_ms: Option<f64>, input: &[i16], out: &mut Vec<i16>) {
        if !self.enabled {
            out.extend_from_slice(input);
            return;
        }
        if let Some(due) = due_ms {
            self.update_step(due);
            self.update_error(due);
        }
        self.n_in += input.len() as u64;
        self.tail.extend_from_slice(input);
        loop {
            let i = self.pos.floor() as u64;
            if i < self.tail_base || i + 1 >= self.tail_base + self.tail.len() as u64 {
                break;
            }
            let k = (i - self.tail_base) as usize;
            let frac = self.pos - i as f64;
            let a = f64::from(self.tail[k]);
            let b = f64::from(self.tail[k + 1]);
            out.push((a + (b - a) * frac).round() as i16);
            self.n_out += 1;
            self.pos += self.effective_step();
        }
        // Drop what can no longer be read.
        let keep_from = self.pos.floor() as u64;
        if keep_from > self.tail_base {
            let drop = ((keep_from - self.tail_base) as usize).min(self.tail.len());
            self.tail.drain(..drop);
            self.tail_base += drop as u64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAMES: usize = 160;

    /// Run a source whose clock is `ppm` fast for `seconds` of wall time. Returns the output
    /// length, the final session time of the output and the largest sample-to-sample step.
    fn run(ppm: f64, seconds: f64, enabled: bool) -> (usize, f64, i32) {
        let mut c = Corrector::new(enabled);
        let rate = f64::from(SAMPLE_RATE) * (1.0 + ppm * 1e-6);
        let total_in = (seconds * rate) as usize;
        let mut out = Vec::new();
        let mut n = 0usize;
        let mut worst = 0i32;
        while n + FRAMES <= total_in {
            let due = n as f64 / rate * 1000.0;
            // 200 Hz sine at amplitude 8000, sampled by the fast device clock.
            let block: Vec<i16> = (n..n + FRAMES)
                .map(|i| (8000.0 * (std::f64::consts::TAU * 200.0 * i as f64 / rate).sin()) as i16)
                .collect();
            let before = out.len();
            c.process(Some(due), &block, &mut out);
            if before > 0 && out.len() > before {
                worst = worst.max((i32::from(out[before]) - i32::from(out[before - 1])).abs());
            }
            for w in out[before.saturating_sub(1)..].windows(2) {
                worst = worst.max((i32::from(w[1]) - i32::from(w[0])).abs());
            }
            n += FRAMES;
        }
        let wall_ms = total_in as f64 / rate * 1000.0;
        let out_ms = out.len() as f64 * 1000.0 / f64::from(SAMPLE_RATE);
        (out.len(), out_ms - wall_ms, worst)
    }

    #[test]
    fn disabled_passes_samples_through() {
        let mut c = Corrector::new(false);
        let mut out = Vec::new();
        c.process(Some(0.0), &[1, 2, 3], &mut out);
        assert_eq!(out, vec![1, 2, 3]);
    }

    #[test]
    fn without_drift_length_is_preserved() {
        let (len, err_ms, _) = run(0.0, 120.0, true);
        assert!(err_ms.abs() < 1.0, "error {err_ms} ms");
        assert!(len > 1_900_000);
    }

    #[test]
    fn fast_clock_is_corrected() {
        let (_, err_uncorrected, _) = run(54.0, 600.0, false);
        let (_, err_ms, worst) = run(54.0, 600.0, true);
        // Without correction the stream is 32 ms too long after 10 minutes.
        assert!(err_uncorrected > 30.0, "uncorrected {err_uncorrected}");
        // The first 30 s run uncorrected (about 1.6 ms); afterwards the error stays small.
        assert!(err_ms.abs() < 3.0, "corrected error {err_ms} ms");
        // Interpolation keeps the waveform smooth: a 200 Hz sine steps at most ~800.
        assert!(worst < 900, "worst step {worst}");
    }

    #[test]
    fn slow_clock_is_corrected() {
        // The 30 s warm-up runs uncorrected (2.4 ms at 80 ppm); the rest stays on the clock.
        let (_, err_ms, _) = run(-80.0, 600.0, true);
        assert!(err_ms.abs() < 6.0, "corrected error {err_ms} ms");
    }

    #[test]
    fn stall_restarts_the_measurement() {
        let mut c = Corrector::new(true);
        let mut out = Vec::new();
        let block = vec![0i16; FRAMES];
        // 40 s on a 100 ppm fast clock so the step is established.
        let rate = 16_000.0 * (1.0 + 100e-6);
        let mut n = 0usize;
        while n < (40.0 * rate) as usize {
            c.process(Some(n as f64 / rate * 1000.0), &block, &mut out);
            n += FRAMES;
        }
        let ppm = (c.step - 1.0) * 1e6;
        assert!((ppm - 100.0).abs() < 10.0, "ppm {ppm}");
        // A 2 s stall: the next packet arrives 2000 ms late. The step must not jump.
        c.process(Some(n as f64 / rate * 1000.0 + 2000.0), &block, &mut out);
        assert!(((c.step - 1.0) * 1e6 - ppm).abs() < 1.0);
    }

    #[test]
    fn changing_device_rate_is_followed() {
        // The device runs 20 ppm fast for 5 minutes, then 70 ppm fast for 5 minutes.
        let mut c = Corrector::new(true);
        let mut out = Vec::new();
        let (mut n, mut wall_ms) = (0usize, 0.0f64);
        for ppm in [20.0, 70.0] {
            let rate = f64::from(SAMPLE_RATE) * (1.0 + ppm * 1e-6);
            let end = n + (300.0 * rate) as usize;
            while n + FRAMES <= end {
                let block = vec![0i16; FRAMES];
                c.process(Some(wall_ms), &block, &mut out);
                n += FRAMES;
                wall_ms += FRAMES as f64 / rate * 1000.0;
            }
        }
        let out_ms = out.len() as f64 * 1000.0 / f64::from(SAMPLE_RATE);
        assert!(
            (out_ms - wall_ms).abs() < 5.0,
            "error {} ms",
            out_ms - wall_ms
        );
    }
}
