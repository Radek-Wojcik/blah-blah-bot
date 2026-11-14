//! Arithmetic on the session timeline: sample counts, millisecond offsets and segment bounds.
//!
//! Everything here is pure so it can be tested without audio hardware.

/// Sample rate of every stored track.
pub const SAMPLE_RATE: u32 = 16_000;

/// Convert a sample count to milliseconds.
pub fn samples_to_ms(samples: u64, rate: u32) -> f64 {
    samples as f64 * 1000.0 / f64::from(rate)
}

/// Convert milliseconds to the nearest whole number of samples (negative values give 0).
pub fn ms_to_samples(ms: f64, rate: u32) -> u64 {
    if ms <= 0.0 {
        return 0;
    }
    (ms * f64::from(rate) / 1000.0).round() as u64
}

/// Convert a time in 100 ns units (the unit of WASAPI packet timestamps) to milliseconds.
pub fn hns_to_ms(hns: u64) -> f64 {
    hns as f64 / 10_000.0
}

/// Number of samples in one segment.
pub fn segment_samples(segment_s: u32, rate: u32) -> u64 {
    u64::from(segment_s) * u64::from(rate)
}

/// Session-clock offset of the first sample of `segment` for a track that started at
/// `track_offset_ms`, given that all earlier segments are full.
pub fn segment_offset_ms(track_offset_ms: f64, segment: u32, seg_samples: u64, rate: u32) -> f64 {
    track_offset_ms + samples_to_ms(u64::from(segment) * seg_samples, rate)
}

/// Session-clock position of sample `index` of a track.
pub fn sample_position_ms(track_offset_ms: f64, index: u64, rate: u32) -> f64 {
    track_offset_ms + samples_to_ms(index, rate)
}

/// Segment that holds sample `index` and the position inside it.
pub fn locate_sample(index: u64, seg_samples: u64) -> (u32, u64) {
    ((index / seg_samples) as u32, index % seg_samples)
}

/// Number of silent samples to insert so that a packet whose first sample is due at
/// `due_ms` on the session clock lands at the right place, when `written` samples of a track
/// that started at `track_offset_ms` are already stored.
///
/// Returns 0 when the shortfall is smaller than `tolerance_ms`; small timestamp jitter must
/// not be turned into silence.
pub fn gap_to_fill(
    track_offset_ms: f64,
    written: u64,
    due_ms: f64,
    rate: u32,
    tolerance_ms: f64,
) -> u64 {
    let due = ms_to_samples(due_ms - track_offset_ms, rate);
    if due <= written {
        return 0;
    }
    let shortfall = due - written;
    if samples_to_ms(shortfall, rate) < tolerance_ms {
        0
    } else {
        shortfall
    }
}

/// One piece of a sample run that belongs to a single segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    /// Segment number.
    pub segment: u32,
    /// Offset of the piece in the pushed run.
    pub start: usize,
    /// Length of the piece.
    pub len: usize,
    /// True when this piece completes its segment.
    pub completes: bool,
}

/// Splits a stream of samples into fixed-size segments.
#[derive(Debug, Clone)]
pub struct Segmenter {
    seg_samples: u64,
    total: u64,
}

impl Segmenter {
    pub fn new(seg_samples: u64) -> Self {
        assert!(seg_samples > 0, "segment length must be positive");
        Self {
            seg_samples,
            total: 0,
        }
    }

    /// Samples accepted so far.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Account for `len` more samples and return the pieces per segment.
    pub fn push(&mut self, len: usize) -> Vec<Chunk> {
        let mut out = Vec::new();
        let mut start = 0usize;
        while start < len {
            let (segment, inside) = locate_sample(self.total, self.seg_samples);
            let room = (self.seg_samples - inside) as usize;
            let take = room.min(len - start);
            self.total += take as u64;
            out.push(Chunk {
                segment,
                start,
                len: take,
                completes: take == room,
            });
            start += take;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(segment: u32, start: usize, len: usize, completes: bool) -> Chunk {
        Chunk {
            segment,
            start,
            len,
            completes,
        }
    }

    #[test]
    fn conversions_round_trip() {
        assert_eq!(samples_to_ms(16_000, SAMPLE_RATE), 1000.0);
        assert_eq!(samples_to_ms(1, SAMPLE_RATE), 0.0625);
        assert_eq!(ms_to_samples(1000.0, SAMPLE_RATE), 16_000);
        assert_eq!(ms_to_samples(0.0625, SAMPLE_RATE), 1);
        assert_eq!(ms_to_samples(-5.0, SAMPLE_RATE), 0);
        assert_eq!(hns_to_ms(10_000), 1.0);
    }

    #[test]
    fn segment_sizes_and_offsets() {
        let seg = segment_samples(60, SAMPLE_RATE);
        assert_eq!(seg, 960_000);
        assert_eq!(segment_offset_ms(250.0, 0, seg, SAMPLE_RATE), 250.0);
        assert_eq!(segment_offset_ms(250.0, 3, seg, SAMPLE_RATE), 180_250.0);
        assert_eq!(sample_position_ms(10.0, 8_000, SAMPLE_RATE), 510.0);
    }

    #[test]
    fn locate_sample_at_boundaries() {
        assert_eq!(locate_sample(0, 100), (0, 0));
        assert_eq!(locate_sample(99, 100), (0, 99));
        assert_eq!(locate_sample(100, 100), (1, 0));
        assert_eq!(locate_sample(250, 100), (2, 50));
    }

    #[test]
    fn segmenter_splits_across_boundary() {
        let mut s = Segmenter::new(100);
        assert_eq!(s.push(60), vec![chunk(0, 0, 60, false)]);
        assert_eq!(
            s.push(250),
            vec![
                chunk(0, 0, 40, true),
                chunk(1, 40, 100, true),
                chunk(2, 140, 100, true),
                chunk(3, 240, 10, false),
            ]
        );
        assert_eq!(s.total(), 310);
    }

    #[test]
    fn segmenter_exact_fit_completes() {
        let mut s = Segmenter::new(100);
        let a = s.push(100);
        assert_eq!(a, vec![chunk(0, 0, 100, true)]);
        assert!(s.push(0).is_empty());
        assert_eq!(s.push(1), vec![chunk(1, 0, 1, false)]);
    }

    #[test]
    fn gap_fill_respects_tolerance() {
        // Track started at 0 ms, 1 s written, next packet due at 1.5 s.
        assert_eq!(gap_to_fill(0.0, 16_000, 1500.0, SAMPLE_RATE, 30.0), 8_000);
        // 10 ms late is jitter, not a gap.
        assert_eq!(gap_to_fill(0.0, 16_000, 1010.0, SAMPLE_RATE, 30.0), 0);
        // Packet early: nothing to insert.
        assert_eq!(gap_to_fill(0.0, 16_000, 900.0, SAMPLE_RATE, 30.0), 0);
        // Offset of the track is taken into account.
        assert_eq!(gap_to_fill(100.0, 0, 600.0, SAMPLE_RATE, 30.0), 8_000);
    }
}
