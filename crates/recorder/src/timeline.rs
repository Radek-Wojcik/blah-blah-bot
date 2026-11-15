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

/// Number of segment files needed to store `total_samples` samples (the last may be partial).
pub fn segment_count(total_samples: u64, seg_samples: u64) -> u32 {
    if seg_samples == 0 {
        return 0;
    }
    total_samples.div_ceil(seg_samples) as u32
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
    /// Create a segmenter. A segment length of 0 is treated as 1 sample so that the
    /// segmenter can never loop forever; callers validate their input before getting here.
    pub fn new(seg_samples: u64) -> Self {
        Self {
            seg_samples: seg_samples.max(1),
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
    fn full_sixty_second_segments_and_a_partial_last_one() {
        let seg = segment_samples(60, SAMPLE_RATE);
        let mut s = Segmenter::new(seg);
        // 150 s of audio pushed in 10 ms packets: two full segments and 30 s in a third.
        let mut completed = Vec::new();
        let mut last = None;
        for _ in 0..15_000 {
            for c in s.push(160) {
                if c.completes {
                    completed.push(c.segment);
                }
                last = Some(c);
            }
        }
        assert_eq!(completed, vec![0, 1]);
        assert_eq!(s.total(), 2_400_000);
        let last = last.expect("some chunk");
        assert_eq!(last.segment, 2);
        assert!(!last.completes);
        assert_eq!(segment_count(s.total(), seg), 3);
        assert_eq!(samples_to_ms(s.total() - 2 * seg, SAMPLE_RATE), 30_000.0);
    }

    #[test]
    fn segment_count_handles_exact_and_partial_totals() {
        let seg = segment_samples(60, SAMPLE_RATE);
        assert_eq!(segment_count(0, seg), 0);
        assert_eq!(segment_count(1, seg), 1);
        assert_eq!(segment_count(seg, seg), 1);
        assert_eq!(segment_count(seg + 1, seg), 2);
        assert_eq!(segment_count(5 * seg, seg), 5);
        assert_eq!(segment_count(10, 0), 0);
    }

    #[test]
    fn segment_offsets_follow_the_start_offset() {
        let seg = segment_samples(60, SAMPLE_RATE);
        // A track that joined 1.5 s after the session start: every segment is 60 s later.
        let offsets: Vec<f64> = (0..4)
            .map(|n| segment_offset_ms(1_500.0, n, seg, SAMPLE_RATE))
            .collect();
        assert_eq!(offsets, vec![1_500.0, 61_500.0, 121_500.0, 181_500.0]);
        // Sample positions agree with segment offsets at segment starts.
        for n in 0..4u32 {
            assert_eq!(
                sample_position_ms(1_500.0, u64::from(n) * seg, SAMPLE_RATE),
                offsets[n as usize]
            );
        }
        // And in the middle of a segment: sample 8000 of segment 1 is 500 ms into it.
        let (segment, inside) = locate_sample(seg + 8_000, seg);
        assert_eq!((segment, inside), (1, 8_000));
        assert_eq!(
            sample_position_ms(1_500.0, seg + 8_000, SAMPLE_RATE),
            segment_offset_ms(1_500.0, segment, seg, SAMPLE_RATE) + 500.0
        );
    }

    #[test]
    fn zero_length_segments_cannot_hang() {
        let mut s = Segmenter::new(0);
        assert_eq!(s.push(3).len(), 3);
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
