//! Tracks whose audio is pushed to the recorder in numbered frames (browser tabs).
//!
//! A pushed stream has no device timestamps. Each frame carries a sequence number and the
//! sender's wall clock at capture. [`PushTrack`] places frames by sample count, so a delivery
//! delay or a burst after a reconnect loses nothing and moves nothing:
//!
//! - The first frame starts at the track offset. Later frames follow by sample count.
//! - Missing sequence numbers mean frames were lost on the way: silence of their length is
//!   inserted and journaled as [`DiscKind::StreamGap`](crate::journal::DiscKind::StreamGap).
//! - Frames that arrive with an old sequence number (duplicates) are dropped.
//! - The sender clock, measured from the first frame, feeds the drift corrector and reveals
//!   capture stalls: when it runs more than [`STALL_TOLERANCE_MS`] ahead of the track,
//!   silence is inserted (`silence_filled`), as for any other source.
//!
//! [`TabSource`] is how a capture thread gets the frames of one stream; the application
//! implements it over the bridge, [`ToneTabSource`] generates a tone for tests and demos.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::drift::Corrector;
use crate::error::{RecorderError, Result};
use crate::journal::Entry;
use crate::timeline::{sample_position_ms, SAMPLE_RATE};
use crate::track::{SharedJournal, TrackWriter};
use crate::tracker::{PacketFlags, Plan, Stats, Tracker};

/// Source kind written to the journal for tab tracks.
pub const TAB_SOURCE_KIND: &str = "browser-tab";
/// How far the sender clock may run ahead of the track before silence is inserted, in ms.
/// Larger than the delivery jitter of a browser (P7 saw stalls up to about 700 ms whose
/// audio arrived later), so only a capture that really produced nothing is filled.
pub const STALL_TOLERANCE_MS: f64 = 1_000.0;
/// Longest gap filled for missing frames (ten minutes); a larger jump is a sender bug.
const MAX_GAP_SAMPLES: u64 = 10 * 60 * SAMPLE_RATE as u64;

/// One frame of a pushed stream.
#[derive(Debug, Clone, PartialEq)]
pub struct PushFrame {
    /// Frame number within the stream, from 0.
    pub seq: u32,
    /// Sender wall clock at capture, ms since the Unix epoch.
    pub sent_unix_ms: f64,
    /// 16 kHz mono samples.
    pub samples: Vec<i16>,
}

/// What a tab feed delivers.
#[derive(Debug, Clone, PartialEq)]
pub enum TabItem {
    Frame(PushFrame),
    /// Nothing arrived within the timeout.
    Idle,
    /// The stream ended, with the reason.
    End(String),
}

/// The frames of one stream, read by its capture thread.
pub trait TabFeed: Send {
    /// The next frame, or `Idle` after `timeout`, or `End`.
    fn next(&mut self, timeout: Duration) -> TabItem;
}

/// Where tab streams come from.
pub trait TabSource: Send + Sync + 'static {
    /// Take the frames of a stream. Called once by the capture thread of its track.
    fn attach(&self, stream: &str) -> std::result::Result<Box<dyn TabFeed>, String>;
    /// The track of the stream ended (removed, recording stopped, or the stream ended).
    fn detach(&self, stream: &str);
}

/// Counters of a pushed track besides the tracker statistics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushStats {
    /// Frames stored.
    pub frames: u64,
    /// Frames missing (sequence numbers skipped).
    pub missing_frames: u64,
    /// Frames dropped because their sequence number was already seen.
    pub stale_frames: u64,
}

/// Writer of one pushed track: journal entries, segments, gap and stall handling.
pub struct PushTrack {
    name: String,
    journal: SharedJournal,
    writer: TrackWriter,
    tracker: Tracker,
    corrector: Corrector,
    offset_ms: f64,
    /// Sender time of the first frame and its place on the session clock.
    anchor: Option<(f64, f64)>,
    next_seq: Option<u32>,
    frame_len: u64,
    corrected: Vec<i16>,
    pub stats: PushStats,
}

impl PushTrack {
    /// Start a track named `name` at `offset_ms` on the session clock: journals its start
    /// and creates its folder under `root`.
    pub fn start(
        name: &str,
        root: &Path,
        journal: SharedJournal,
        offset_ms: f64,
        seg_samples: u64,
        drift_correction: bool,
    ) -> Result<Self> {
        journal
            .lock()
            .map_err(|_| RecorderError::LockPoisoned("journal"))?
            .append(&Entry::TrackStart {
                track: name.to_string(),
                source: TAB_SOURCE_KIND.to_string(),
                pid: None,
                include_tree: None,
                offset_ms,
            })?;
        let writer = TrackWriter::new(
            name,
            root,
            journal.clone(),
            offset_ms,
            SAMPLE_RATE,
            seg_samples,
        )?;
        Ok(Self {
            name: name.to_string(),
            journal,
            writer,
            tracker: Tracker::new(SAMPLE_RATE, offset_ms, STALL_TOLERANCE_MS),
            corrector: Corrector::new(drift_correction),
            offset_ms,
            anchor: None,
            next_seq: None,
            frame_len: 0,
            corrected: Vec::new(),
            stats: PushStats::default(),
        })
    }

    /// Session-clock position of the next stored sample.
    pub fn position_ms(&self) -> f64 {
        self.tracker.position_ms()
    }

    /// Samples stored so far, silence included.
    pub fn written(&self) -> u64 {
        self.tracker.written()
    }

    /// Packet statistics of the track.
    pub fn tracker_stats(&self) -> &Stats {
        &self.tracker.stats
    }

    /// Correction applied for the sender's clock rate, in ppm.
    pub fn correction_ppm(&self) -> f64 {
        self.corrector.correction_ppm()
    }

    fn apply(&mut self, plan: &Plan, at_ms: f64) -> Result<()> {
        for (kind, frames) in &plan.events {
            tracing::debug!(track = %self.name, ?kind, frames, at_ms, "tab stream irregularity");
            self.journal
                .lock()
                .map_err(|_| RecorderError::LockPoisoned("journal"))?
                .append(&Entry::Discontinuity {
                    track: self.name.clone(),
                    offset_ms: at_ms,
                    kind: *kind,
                    frames: *frames,
                })?;
        }
        if plan.fill > 0 {
            self.writer.push_silence(plan.fill)?;
        }
        Ok(())
    }

    /// Store one frame that arrived at `arrival_ms` on the session clock.
    pub fn push(&mut self, frame: &PushFrame, arrival_ms: f64) -> Result<()> {
        if frame.samples.is_empty() || !frame.sent_unix_ms.is_finite() {
            return Ok(());
        }
        if let Some(next) = self.next_seq {
            let ahead = frame.seq.wrapping_sub(next);
            if ahead >= 1 << 31 {
                self.stats.stale_frames += 1;
                return Ok(());
            }
            if ahead > 0 {
                self.stats.missing_frames += u64::from(ahead);
                let fill = (u64::from(ahead) * self.frame_len).min(MAX_GAP_SAMPLES);
                let at = self.tracker.position_ms();
                let plan = self.tracker.on_missing(fill);
                self.apply(&plan, at)?;
            }
        }
        self.next_seq = Some(frame.seq.wrapping_add(1));
        self.frame_len = frame.samples.len() as u64;

        // Sender time relative to the first frame, placed on the session clock.
        let (sent0, placed0) = *self
            .anchor
            .get_or_insert((frame.sent_unix_ms, self.tracker.position_ms()));
        let due = placed0 + (frame.sent_unix_ms - sent0);
        self.corrected.clear();
        self.corrector
            .process(Some(due), &frame.samples, &mut self.corrected);
        let at = self.tracker.position_ms();
        let plan = self.tracker.on_packet(
            Some(due),
            self.corrected.len() as u64,
            arrival_ms,
            PacketFlags::default(),
        );
        self.apply(&plan, at)?;
        let samples = std::mem::take(&mut self.corrected);
        self.writer.push(&samples)?;
        self.corrected = samples;
        self.stats.frames += 1;
        Ok(())
    }

    /// Fill up to `now_ms` when the sender went quiet for longer than the tolerance, close
    /// the track and journal its end. Returns the stored sample count.
    pub fn finish(mut self, now_ms: f64) -> Result<u64> {
        let at = self.tracker.position_ms();
        let plan = self.tracker.on_stop(now_ms);
        self.apply(&plan, at)?;
        let total = self.writer.finish()?;
        self.journal
            .lock()
            .map_err(|_| RecorderError::LockPoisoned("journal"))?
            .append(&Entry::TrackEnd {
                track: self.name.clone(),
                total_samples: total,
                end_offset_ms: sample_position_ms(self.offset_ms, total, SAMPLE_RATE),
            })?;
        Ok(total)
    }
}

/// A tab source that plays a sine tone on every stream, paced by the wall clock. For tests
/// and the demo mode.
pub struct ToneTabSource {
    pub hz: u32,
    /// End every stream after this long (a closed tab).
    pub end_after: Option<Duration>,
}

struct ToneFeed {
    hz: u32,
    started: Instant,
    started_unix_ms: f64,
    seq: u32,
    end_after: Option<Duration>,
}

const TONE_FRAME: u64 = 320;

impl TabFeed for ToneFeed {
    fn next(&mut self, timeout: Duration) -> TabItem {
        let elapsed = self.started.elapsed();
        if self.end_after.is_some_and(|d| elapsed >= d) {
            return TabItem::End("tab_closed".into());
        }
        // Frame `seq` is complete once its last sample is due.
        let ready_at = Duration::from_micros(
            (u64::from(self.seq) + 1) * TONE_FRAME * 1_000_000 / u64::from(SAMPLE_RATE),
        );
        if elapsed < ready_at {
            std::thread::sleep((ready_at - elapsed).min(timeout));
            if self.started.elapsed() < ready_at {
                return TabItem::Idle;
            }
        }
        let first = u64::from(self.seq) * TONE_FRAME;
        let step = std::f64::consts::TAU * f64::from(self.hz) / f64::from(SAMPLE_RATE);
        let samples = (first..first + TONE_FRAME)
            .map(|n| (f64::sin(step * n as f64) * 9_000.0) as i16)
            .collect();
        let sent = self.started_unix_ms + first as f64 * 1000.0 / f64::from(SAMPLE_RATE);
        let frame = PushFrame {
            seq: self.seq,
            sent_unix_ms: sent,
            samples,
        };
        self.seq += 1;
        TabItem::Frame(frame)
    }
}

impl TabSource for ToneTabSource {
    fn attach(&self, _stream: &str) -> std::result::Result<Box<dyn TabFeed>, String> {
        let started_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        Ok(Box::new(ToneFeed {
            hz: self.hz,
            started: Instant::now(),
            started_unix_ms,
            seq: 0,
            end_after: self.end_after,
        }))
    }

    fn detach(&self, _stream: &str) {}
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::journal::{replay_file, DiscKind, Journal, JOURNAL_FILE_NAME};
    use crate::test_util::TempDir;
    use crate::wav::read_samples;

    const N: usize = 320;
    const T0: f64 = 1_794_579_901_000.0;

    struct Fixture {
        dir: TempDir,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            Self {
                dir: TempDir::new(label),
            }
        }

        fn track(&self, drift: bool) -> PushTrack {
            let journal = Arc::new(Mutex::new(
                Journal::create(&self.dir.path().join(JOURNAL_FILE_NAME)).unwrap(),
            ));
            // Long segments: one file per test track.
            PushTrack::start("tab", self.dir.path(), journal, 100.0, 16_000 * 600, drift).unwrap()
        }

        fn replay(&self) -> crate::journal::TrackReplay {
            replay_file(&self.dir.path().join(JOURNAL_FILE_NAME))
                .unwrap()
                .tracks["tab"]
                .clone()
        }

        fn samples(&self) -> Vec<i16> {
            read_samples(&self.dir.path().join("tab/seg-00000.wav"))
                .unwrap()
                .1
        }
    }

    fn frame(seq: u32, sent_ms: f64, value: i16) -> PushFrame {
        PushFrame {
            seq,
            sent_unix_ms: sent_ms,
            samples: vec![value; N],
        }
    }

    #[test]
    fn contiguous_frames_are_stored_back_to_back() {
        let f = Fixture::new("push-contiguous");
        let mut t = f.track(true);
        for seq in 0..100u32 {
            let sent = T0 + f64::from(seq) * 20.0;
            t.push(&frame(seq, sent, 7), 100.0 + f64::from(seq) * 20.0 + 5.0)
                .unwrap();
        }
        assert_eq!(t.written(), 100 * N as u64);
        let total = t.finish(100.0 + 2_000.0).unwrap();
        assert_eq!(total, 100 * N as u64);
        let r = f.replay();
        assert!(r.discontinuities.is_empty(), "{:?}", r.discontinuities);
        assert!(r.ended);
        assert_eq!(r.source, TAB_SOURCE_KIND);
        assert!(f.samples().iter().all(|&s| s == 7));
    }

    #[test]
    fn missing_frames_become_journaled_silence() {
        let f = Fixture::new("push-gap");
        let mut t = f.track(true);
        for seq in (0..10u32).chain(15..20) {
            let sent = T0 + f64::from(seq) * 20.0;
            t.push(&frame(seq, sent, 5), 100.0 + f64::from(seq) * 20.0)
                .unwrap();
        }
        assert_eq!(t.stats.missing_frames, 5);
        t.finish(500.0).unwrap();
        let r = f.replay();
        assert_eq!(r.discontinuities.len(), 1);
        let d = &r.discontinuities[0];
        assert_eq!(d.kind, DiscKind::StreamGap);
        assert_eq!(d.frames, 5 * N as u64);
        assert_eq!(d.offset_ms, 100.0 + 200.0);
        assert_eq!(r.loss_events(), 1);
        // Frames after the gap sit where their sequence number puts them.
        let s = f.samples();
        assert_eq!(s.len(), 20 * N);
        assert!(s[..10 * N].iter().all(|&x| x == 5));
        assert!(s[10 * N..15 * N].iter().all(|&x| x == 0));
        assert!(s[15 * N..].iter().all(|&x| x == 5));
    }

    #[test]
    fn a_late_burst_is_placed_by_sample_count() {
        // Five seconds of frames arrive all at once after a reconnect: nothing is lost,
        // nothing is filled.
        let f = Fixture::new("push-burst");
        let mut t = f.track(true);
        for seq in 0..10u32 {
            t.push(
                &frame(seq, T0 + f64::from(seq) * 20.0, 1),
                100.0 + f64::from(seq) * 20.0,
            )
            .unwrap();
        }
        for seq in 10..260u32 {
            t.push(&frame(seq, T0 + f64::from(seq) * 20.0, 1), 5_300.0)
                .unwrap();
        }
        assert_eq!(t.written(), 260 * N as u64);
        t.finish(5_300.0).unwrap();
        assert!(f.replay().discontinuities.is_empty());
    }

    #[test]
    fn stale_and_duplicate_frames_are_dropped() {
        let f = Fixture::new("push-stale");
        let mut t = f.track(false);
        t.push(&frame(0, T0, 1), 100.0).unwrap();
        t.push(&frame(1, T0 + 20.0, 1), 120.0).unwrap();
        t.push(&frame(1, T0 + 20.0, 9), 121.0).unwrap();
        t.push(&frame(0, T0, 9), 122.0).unwrap();
        t.push(&frame(2, T0 + 40.0, 1), 140.0).unwrap();
        assert_eq!(t.stats.stale_frames, 2);
        assert_eq!(t.written(), 3 * N as u64);
        t.finish(160.0).unwrap();
        assert!(f.samples().iter().all(|&s| s == 1));
    }

    #[test]
    fn a_capture_stall_is_filled_to_stay_on_the_clock() {
        // Sequence numbers are contiguous but the sender produced nothing for two seconds.
        let f = Fixture::new("push-stall");
        let mut t = f.track(false);
        for seq in 0..10u32 {
            t.push(&frame(seq, T0 + f64::from(seq) * 20.0, 1), 0.0)
                .unwrap();
        }
        t.push(&frame(10, T0 + 200.0 + 2_000.0, 1), 0.0).unwrap();
        // 2 s of silence before the frame.
        assert_eq!(t.written(), 11 * N as u64 + 32_000);
        t.finish(0.0).unwrap();
        let r = f.replay();
        assert_eq!(r.discontinuities.len(), 1);
        assert_eq!(r.discontinuities[0].kind, DiscKind::SilenceFilled);
        assert_eq!(r.loss_events(), 0);
    }

    #[test]
    fn small_delivery_jitter_is_not_filled() {
        let f = Fixture::new("push-jitter");
        let mut t = f.track(false);
        for seq in 0..50u32 {
            // Sender stamps wobble by up to 300 ms around the true time.
            let wobble = if seq % 7 == 0 { 300.0 } else { 0.0 };
            t.push(&frame(seq, T0 + f64::from(seq) * 20.0 + wobble, 1), 0.0)
                .unwrap();
        }
        assert_eq!(t.written(), 50 * N as u64);
    }

    /// Runs `seconds` of a sender whose 320-sample frames each cover `frame_ms` of sender
    /// time; returns the track position minus the sender time of the end of the audio.
    fn drift_error(label: &str, frame_ms: f64, seconds: f64, drift: bool) -> f64 {
        let f = Fixture::new(label);
        let mut t = f.track(drift);
        let frames = (seconds * 1000.0 / frame_ms) as u32;
        for seq in 0..frames {
            let sent = T0 + f64::from(seq) * frame_ms;
            t.push(&frame(seq, sent, 0), 100.0 + f64::from(seq) * frame_ms)
                .unwrap();
        }
        let error = t.position_ms() - (100.0 + f64::from(frames) * frame_ms);
        t.finish(0.0).unwrap();
        error
    }

    #[test]
    fn a_slow_sender_clock_is_corrected() {
        // Each frame takes 20.006 ms of wall time: the sender's sample clock is 300 ppm slow.
        let uncorrected = drift_error("push-drift-off", 20.006, 240.0, false);
        let corrected = drift_error("push-drift-on", 20.006, 240.0, true);
        assert!(uncorrected < -60.0, "uncorrected {uncorrected} ms");
        assert!(corrected.abs() < 10.0, "corrected {corrected} ms");
    }

    #[test]
    fn a_closed_tab_ends_short_and_stop_fills_to_the_clock() {
        let f = Fixture::new("push-stop");
        let mut t = f.track(false);
        for seq in 0..50u32 {
            t.push(&frame(seq, T0 + f64::from(seq) * 20.0, 1), 0.0)
                .unwrap();
        }
        // Track: 100 ms offset + 1 s of audio. Stopped 3 s later: 2 s are filled.
        let total = t.finish(100.0 + 3_000.0).unwrap();
        assert_eq!(total, 3 * 16_000);
        let r = f.replay();
        assert_eq!(r.declared_total_samples, Some(3 * 16_000));
    }

    #[test]
    fn the_tone_source_paces_frames_by_the_clock() {
        let src = ToneTabSource {
            hz: 440,
            end_after: Some(Duration::from_millis(200)),
        };
        let mut feed = src.attach("s").unwrap();
        let mut seqs = Vec::new();
        loop {
            match feed.next(Duration::from_millis(50)) {
                TabItem::Frame(f) => {
                    assert_eq!(f.samples.len(), N);
                    seqs.push(f.seq);
                }
                TabItem::Idle => {}
                TabItem::End(_) => break,
            }
        }
        // About 10 frames in 200 ms, numbered from 0 without holes.
        assert!((8..=11).contains(&seqs.len()), "{seqs:?}");
        assert!(seqs.iter().enumerate().all(|(i, &s)| s == i as u32));
    }
}
