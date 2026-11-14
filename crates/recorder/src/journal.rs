//! Append-only session journal (JSON lines) and its replay.
//!
//! The journal is flushed after every entry, so after a crash it still describes every
//! completed segment. A torn last line is ignored on replay.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::timeline::samples_to_ms;

/// File name of the journal inside a recording directory.
pub const JOURNAL_FILE_NAME: &str = "journal.jsonl";

/// Kind of a stream irregularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscKind {
    /// The audio engine flagged a data discontinuity: buffers were lost.
    DataDiscontinuity,
    /// The audio engine flagged an unreliable timestamp.
    TimestampError,
    /// The source delivered nothing for a while and silence was inserted to stay on the clock.
    SilenceFilled,
}

impl DiscKind {
    /// True for irregularities that mean captured audio was lost.
    pub fn is_loss(self) -> bool {
        !matches!(self, DiscKind::SilenceFilled)
    }
}

/// One journal line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Entry {
    /// The session began.
    SessionStart {
        /// Wall-clock start time, milliseconds since the Unix epoch.
        started_unix_ms: u64,
        /// Sample rate of every track.
        sample_rate: u32,
        /// Samples in a full segment.
        segment_samples: u64,
        /// Storage format label, for example `pcm`.
        format: String,
    },
    /// A track began recording.
    TrackStart {
        /// Track name (also its directory name).
        track: String,
        /// Kind of source, for example `mic` or `process-loopback`.
        source: String,
        /// Target process id for process loopback.
        pid: Option<u32>,
        /// Process-tree mode for process loopback.
        include_tree: Option<bool>,
        /// Start of the track on the session clock, in milliseconds.
        offset_ms: f64,
    },
    /// A segment file is complete. Written when the segment is closed.
    Segment {
        /// Track name.
        track: String,
        /// Zero-based segment number.
        segment: u32,
        /// Path of the segment file relative to the recording directory.
        file: String,
        /// Session-clock position of the first sample, in milliseconds.
        offset_ms: f64,
        /// Samples in the segment.
        samples: u64,
    },
    /// A stream irregularity.
    Discontinuity {
        /// Track name.
        track: String,
        /// Session-clock position where it was detected, in milliseconds.
        offset_ms: f64,
        /// What kind of irregularity it was.
        kind: DiscKind,
        /// Frames it covers (0 when unknown).
        frames: u64,
    },
    /// A track ended.
    TrackEnd {
        /// Track name.
        track: String,
        /// Samples stored in the track, including inserted silence.
        total_samples: u64,
        /// Session-clock position where the stored audio ends, in milliseconds.
        end_offset_ms: f64,
    },
    /// The session ended.
    SessionEnd {
        /// Length of the session in milliseconds.
        duration_ms: f64,
    },
}

/// Encode an entry as one line without the trailing newline.
pub fn encode_line(entry: &Entry) -> Result<String> {
    Ok(serde_json::to_string(entry)?)
}

/// Decode one line.
pub fn decode_line(line: &str) -> Result<Entry> {
    Ok(serde_json::from_str(line)?)
}

/// Append-only writer that flushes after every entry.
pub struct Journal {
    file: File,
}

impl Journal {
    /// Open the journal at `path`, creating it when missing. Existing content is kept and
    /// new entries are appended.
    pub fn create(path: &Path) -> Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file })
    }

    /// Append one entry and flush it to the operating system.
    pub fn append(&mut self, entry: &Entry) -> Result<()> {
        let mut line = encode_line(entry)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        Ok(())
    }
}

/// A segment as described by the journal.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentInfo {
    /// Zero-based segment number.
    pub segment: u32,
    /// Path of the segment file relative to the recording directory.
    pub file: String,
    /// Session-clock position of the first sample, in milliseconds.
    pub offset_ms: f64,
    /// Samples in the segment.
    pub samples: u64,
}

/// An irregularity as described by the journal.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscInfo {
    /// Session-clock position in milliseconds.
    pub offset_ms: f64,
    /// Kind of irregularity.
    pub kind: DiscKind,
    /// Frames it covers (0 when unknown).
    pub frames: u64,
}

/// What replay learned about one track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackReplay {
    /// Kind of source.
    pub source: String,
    /// Target process id, if any.
    pub pid: Option<u32>,
    /// Start of the track on the session clock, in milliseconds.
    pub offset_ms: f64,
    /// Journaled (complete) segments in journal order.
    pub segments: Vec<SegmentInfo>,
    /// Journaled irregularities.
    pub discontinuities: Vec<DiscInfo>,
    /// True when a `track_end` entry was found.
    pub ended: bool,
    /// Sample count declared by `track_end`.
    pub declared_total_samples: Option<u64>,
}

/// A problem found while checking continuity.
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    /// Segment numbers are not consecutive from zero.
    MissingSegment {
        /// Number the sequence required at this position.
        expected: u32,
        /// Number that was found.
        found: u32,
    },
    /// A segment other than the last one is not full.
    ShortSegment {
        /// Segment number.
        segment: u32,
        /// Samples it holds.
        samples: u64,
    },
    /// The offset of a segment does not follow from the previous one.
    OffsetMismatch {
        /// Segment number.
        segment: u32,
        /// Offset implied by the previous segment, in milliseconds.
        expected_ms: f64,
        /// Offset recorded in the journal, in milliseconds.
        found_ms: f64,
    },
}

impl TrackReplay {
    /// Samples in all journaled segments.
    pub fn total_samples(&self) -> u64 {
        self.segments.iter().map(|s| s.samples).sum()
    }

    /// Session-clock position where the stored audio ends.
    pub fn end_offset_ms(&self, rate: u32) -> f64 {
        self.offset_ms + samples_to_ms(self.total_samples(), rate)
    }

    /// Check segment numbering, fullness and offsets.
    pub fn problems(&self, seg_samples: u64, rate: u32) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut expected_offset = self.offset_ms;
        let last = self.segments.len().saturating_sub(1);
        for (i, s) in self.segments.iter().enumerate() {
            if s.segment as usize != i {
                out.push(Problem::MissingSegment {
                    expected: u32::try_from(i).unwrap_or(u32::MAX),
                    found: s.segment,
                });
            }
            if i != last && s.samples != seg_samples {
                out.push(Problem::ShortSegment {
                    segment: s.segment,
                    samples: s.samples,
                });
            }
            if (s.offset_ms - expected_offset).abs() > 0.01 {
                out.push(Problem::OffsetMismatch {
                    segment: s.segment,
                    expected_ms: expected_offset,
                    found_ms: s.offset_ms,
                });
            }
            expected_offset = s.offset_ms + samples_to_ms(s.samples, rate);
        }
        out
    }

    /// Number of irregularities that mean captured audio was lost.
    pub fn loss_events(&self) -> usize {
        self.discontinuities
            .iter()
            .filter(|d| d.kind.is_loss())
            .count()
    }
}

/// Result of replaying a journal.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Replay {
    /// Sample rate from `session_start`.
    pub sample_rate: u32,
    /// Samples in a full segment, from `session_start`.
    pub segment_samples: u64,
    /// Wall-clock start of the session.
    pub started_unix_ms: u64,
    /// Storage format label.
    pub format: String,
    /// Tracks by name.
    pub tracks: BTreeMap<String, TrackReplay>,
    /// True when a `session_end` entry was found.
    pub session_ended: bool,
    /// Session length declared by `session_end`.
    pub duration_ms: Option<f64>,
    /// Lines that could not be decoded (a torn final line counts here).
    pub bad_lines: usize,
}

/// Replay journal text from a reader.
///
/// Lines that cannot be decoded (a torn last line after a crash, for example) are counted in
/// [`Replay::bad_lines`] and otherwise ignored.
pub fn replay<R: Read>(reader: R) -> Result<Replay> {
    let mut r = Replay::default();
    for line in BufReader::new(reader).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = decode_line(&line) else {
            r.bad_lines += 1;
            continue;
        };
        match entry {
            Entry::SessionStart {
                started_unix_ms,
                sample_rate,
                segment_samples,
                format,
            } => {
                r.started_unix_ms = started_unix_ms;
                r.sample_rate = sample_rate;
                r.segment_samples = segment_samples;
                r.format = format;
            }
            Entry::TrackStart {
                track,
                source,
                pid,
                offset_ms,
                ..
            } => {
                let t = r.tracks.entry(track).or_default();
                t.source = source;
                t.pid = pid;
                t.offset_ms = offset_ms;
            }
            Entry::Segment {
                track,
                segment,
                file,
                offset_ms,
                samples,
            } => {
                r.tracks
                    .entry(track)
                    .or_default()
                    .segments
                    .push(SegmentInfo {
                        segment,
                        file,
                        offset_ms,
                        samples,
                    });
            }
            Entry::Discontinuity {
                track,
                offset_ms,
                kind,
                frames,
            } => {
                r.tracks
                    .entry(track)
                    .or_default()
                    .discontinuities
                    .push(DiscInfo {
                        offset_ms,
                        kind,
                        frames,
                    });
            }
            Entry::TrackEnd {
                track,
                total_samples,
                ..
            } => {
                let t = r.tracks.entry(track).or_default();
                t.ended = true;
                t.declared_total_samples = Some(total_samples);
            }
            Entry::SessionEnd { duration_ms } => {
                r.session_ended = true;
                r.duration_ms = Some(duration_ms);
            }
        }
    }
    Ok(r)
}

/// Replay a journal file.
pub fn replay_file(path: &Path) -> Result<Replay> {
    replay(File::open(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    const SEG: u64 = 100;
    const RATE: u32 = 16_000;

    fn sample_entries() -> Vec<Entry> {
        vec![
            Entry::SessionStart {
                started_unix_ms: 1_700_000_000_000,
                sample_rate: RATE,
                segment_samples: SEG,
                format: "pcm".into(),
            },
            Entry::TrackStart {
                track: "mic".into(),
                source: "mic".into(),
                pid: None,
                include_tree: None,
                offset_ms: 12.5,
            },
            Entry::Segment {
                track: "mic".into(),
                segment: 0,
                file: "mic/seg-00000.wav".into(),
                offset_ms: 12.5,
                samples: SEG,
            },
            Entry::Discontinuity {
                track: "mic".into(),
                offset_ms: 20.0,
                kind: DiscKind::DataDiscontinuity,
                frames: 0,
            },
            Entry::Segment {
                track: "mic".into(),
                segment: 1,
                file: "mic/seg-00001.wav".into(),
                offset_ms: 12.5 + samples_to_ms(SEG, RATE),
                samples: 40,
            },
            Entry::TrackEnd {
                track: "mic".into(),
                total_samples: SEG + 40,
                end_offset_ms: 0.0,
            },
            Entry::SessionEnd {
                duration_ms: 1000.0,
            },
        ]
    }

    fn encode_all(entries: &[Entry]) -> String {
        entries
            .iter()
            .map(|e| encode_line(e).expect("encode") + "\n")
            .collect::<String>()
    }

    #[test]
    fn line_round_trip() {
        for e in sample_entries() {
            let line = encode_line(&e).unwrap();
            assert!(!line.contains('\n'));
            assert_eq!(decode_line(&line).unwrap(), e);
        }
    }

    #[test]
    fn line_round_trip_keeps_process_fields() {
        let e = Entry::TrackStart {
            track: "pid-4242".into(),
            source: "process-loopback".into(),
            pid: Some(4242),
            include_tree: Some(true),
            offset_ms: 1234.5625,
        };
        let line = encode_line(&e).unwrap();
        assert!(line.contains("\"pid\":4242"));
        assert_eq!(decode_line(&line).unwrap(), e);
    }

    #[test]
    fn line_has_event_tag() {
        let line = encode_line(&sample_entries()[2]).unwrap();
        assert!(line.contains("\"event\":\"segment\""));
        assert!(line.contains("\"offset_ms\":12.5"));
    }

    #[test]
    fn every_entry_kind_uses_snake_case_event_names() {
        let names: Vec<String> = sample_entries()
            .iter()
            .map(|e| {
                let line = encode_line(e).unwrap();
                let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                v["event"].as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(
            names,
            [
                "session_start",
                "track_start",
                "segment",
                "discontinuity",
                "segment",
                "track_end",
                "session_end"
            ]
        );
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode_line("").is_err());
        assert!(decode_line("not json").is_err());
        assert!(decode_line("{\"event\":\"unknown_event\"}").is_err());
    }

    #[test]
    fn replay_rebuilds_session() {
        let text = encode_all(&sample_entries());
        let r = replay(text.as_bytes()).unwrap();
        assert_eq!(r.sample_rate, RATE);
        assert_eq!(r.segment_samples, SEG);
        assert!(r.session_ended);
        assert_eq!(r.bad_lines, 0);
        let t = &r.tracks["mic"];
        assert_eq!(t.segments.len(), 2);
        assert_eq!(t.total_samples(), 140);
        assert!(t.ended);
        assert_eq!(t.loss_events(), 1);
        assert!(t.problems(SEG, RATE).is_empty());
        assert_eq!(t.end_offset_ms(RATE), 12.5 + samples_to_ms(140, RATE));
    }

    #[test]
    fn replay_of_empty_input_is_empty() {
        let r = replay("".as_bytes()).unwrap();
        assert!(r.tracks.is_empty());
        assert!(!r.session_ended);
        assert_eq!(r.bad_lines, 0);
    }

    #[test]
    fn blank_lines_are_skipped_not_counted() {
        let text = format!("\n{}\n   \n", encode_line(&sample_entries()[1]).unwrap());
        let r = replay(text.as_bytes()).unwrap();
        assert_eq!(r.bad_lines, 0);
        assert_eq!(r.tracks.len(), 1);
    }

    #[test]
    fn replay_tolerates_torn_last_line() {
        let mut text = encode_all(&sample_entries()[..3]);
        text.push_str("{\"event\":\"segment\",\"track\":\"mic\",\"segm");
        let r = replay(text.as_bytes()).unwrap();
        assert_eq!(r.bad_lines, 1);
        assert_eq!(r.tracks["mic"].segments.len(), 1);
        assert!(!r.session_ended);
        assert!(!r.tracks["mic"].ended);
    }

    #[test]
    fn torn_last_line_in_a_file_keeps_the_complete_entries() {
        let dir = TempDir::new("journal-torn");
        let path = dir.path().join(JOURNAL_FILE_NAME);
        let mut text = encode_all(&sample_entries()[..5]);
        // The process died in the middle of writing the next line.
        text.push_str(&encode_line(&sample_entries()[5]).unwrap()[..20]);
        std::fs::write(&path, text).unwrap();
        let r = replay_file(&path).unwrap();
        assert_eq!(r.bad_lines, 1);
        assert!(!r.session_ended);
        let t = &r.tracks["mic"];
        assert_eq!(t.segments.len(), 2);
        assert!(!t.ended);
        assert!(t.problems(SEG, RATE).is_empty());
    }

    #[test]
    fn replay_detects_missing_and_short_segments() {
        let mut entries = sample_entries();
        // Drop segment 0 so numbering starts at 1 and the offset no longer follows.
        entries.remove(2);
        let r = replay(encode_all(&entries).as_bytes()).unwrap();
        let problems = r.tracks["mic"].problems(SEG, RATE);
        assert!(problems.contains(&Problem::MissingSegment {
            expected: 0,
            found: 1
        }));
        assert!(problems
            .iter()
            .any(|p| matches!(p, Problem::OffsetMismatch { .. })));

        let mut entries = sample_entries();
        if let Entry::Segment { samples, .. } = &mut entries[2] {
            *samples = SEG - 1;
        }
        let r = replay(encode_all(&entries).as_bytes()).unwrap();
        let problems = r.tracks["mic"].problems(SEG, RATE);
        assert!(problems.contains(&Problem::ShortSegment {
            segment: 0,
            samples: SEG - 1
        }));
    }

    #[test]
    fn continuity_check_flags_offset_gaps_between_segments() {
        let mut entries = sample_entries();
        if let Entry::Segment { offset_ms, .. } = &mut entries[4] {
            *offset_ms += 5.0;
        }
        let r = replay(encode_all(&entries).as_bytes()).unwrap();
        let problems = r.tracks["mic"].problems(SEG, RATE);
        assert_eq!(problems.len(), 1);
        assert!(matches!(
            problems[0],
            Problem::OffsetMismatch { segment: 1, .. }
        ));
    }

    #[test]
    fn full_sixty_second_segments_with_partial_last_are_continuous() {
        let seg = crate::timeline::segment_samples(60, RATE);
        let start = 250.0;
        let mut entries = vec![Entry::TrackStart {
            track: "t".into(),
            source: "mic".into(),
            pid: None,
            include_tree: None,
            offset_ms: start,
        }];
        for n in 0..3u32 {
            entries.push(Entry::Segment {
                track: "t".into(),
                segment: n,
                file: format!("t/seg-{n:05}.wav"),
                offset_ms: crate::timeline::segment_offset_ms(start, n, seg, RATE),
                samples: if n == 2 { seg / 2 } else { seg },
            });
        }
        let r = replay(encode_all(&entries).as_bytes()).unwrap();
        let t = &r.tracks["t"];
        assert!(t.problems(seg, RATE).is_empty());
        assert_eq!(t.total_samples(), 2 * seg + seg / 2);
        assert_eq!(t.end_offset_ms(RATE), start + 150_000.0);
    }

    #[test]
    fn silence_fill_is_not_a_loss() {
        assert!(DiscKind::DataDiscontinuity.is_loss());
        assert!(DiscKind::TimestampError.is_loss());
        assert!(!DiscKind::SilenceFilled.is_loss());
    }

    #[test]
    fn journal_file_appends_and_replays() {
        let dir = TempDir::new("journal");
        let path = dir.path().join(JOURNAL_FILE_NAME);
        {
            let mut j = Journal::create(&path).unwrap();
            for e in sample_entries() {
                j.append(&e).unwrap();
            }
        }
        // Reopening appends rather than truncates.
        {
            let mut j = Journal::create(&path).unwrap();
            j.append(&Entry::SessionEnd { duration_ms: 2.0 }).unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), sample_entries().len() + 1);
        let r = replay_file(&path).unwrap();
        assert_eq!(r.tracks["mic"].segments.len(), 2);
    }
}
