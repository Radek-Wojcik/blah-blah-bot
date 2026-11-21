//! Append-only session journal (JSON lines) and its replay.
//!
//! The journal is flushed after every segment, so after a crash it still describes every
//! completed segment. A torn last line is ignored on replay.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::timeline::samples_to_ms;

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
    SessionStart {
        started_unix_ms: u64,
        sample_rate: u32,
        segment_samples: u64,
        format: String,
    },
    TrackStart {
        track: String,
        source: String,
        pid: Option<u32>,
        include_tree: Option<bool>,
        offset_ms: f64,
    },
    Segment {
        track: String,
        segment: u32,
        file: String,
        offset_ms: f64,
        samples: u64,
    },
    Discontinuity {
        track: String,
        offset_ms: f64,
        kind: DiscKind,
        frames: u64,
    },
    TrackEnd {
        track: String,
        total_samples: u64,
        end_offset_ms: f64,
    },
    SessionEnd {
        duration_ms: f64,
    },
}

/// Encode an entry as one line without the trailing newline.
pub fn encode_line(entry: &Entry) -> String {
    serde_json::to_string(entry).expect("journal entries always serialize")
}

/// Decode one line.
pub fn decode_line(line: &str) -> Result<Entry, serde_json::Error> {
    serde_json::from_str(line)
}

/// Append-only writer that flushes after every entry.
pub struct Journal {
    file: File,
}

impl Journal {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file })
    }

    pub fn append(&mut self, entry: &Entry) -> std::io::Result<()> {
        let mut line = encode_line(entry);
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()
    }
}

/// A segment as described by the journal.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentInfo {
    pub segment: u32,
    pub file: String,
    pub offset_ms: f64,
    pub samples: u64,
}

/// An irregularity as described by the journal.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscInfo {
    pub offset_ms: f64,
    pub kind: DiscKind,
    pub frames: u64,
}

/// What replay learned about one track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackReplay {
    pub source: String,
    pub pid: Option<u32>,
    pub offset_ms: f64,
    pub segments: Vec<SegmentInfo>,
    pub discontinuities: Vec<DiscInfo>,
    pub ended: bool,
    pub declared_total_samples: Option<u64>,
}

/// A problem found while checking continuity.
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    /// Segment numbers are not consecutive from zero.
    MissingSegment { expected: u32, found: u32 },
    /// A segment other than the last one is not full.
    ShortSegment { segment: u32, samples: u64 },
    /// The offset of a segment does not follow from the previous one.
    OffsetMismatch {
        segment: u32,
        expected_ms: f64,
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
            if s.segment != i as u32 {
                out.push(Problem::MissingSegment {
                    expected: i as u32,
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
    pub sample_rate: u32,
    pub segment_samples: u64,
    pub started_unix_ms: u64,
    pub format: String,
    pub tracks: BTreeMap<String, TrackReplay>,
    pub session_ended: bool,
    pub duration_ms: Option<f64>,
    /// Lines that could not be decoded (a torn final line counts here).
    pub bad_lines: usize,
}

/// Replay journal text from a reader.
pub fn replay<R: Read>(reader: R) -> std::io::Result<Replay> {
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
pub fn replay_file(path: &Path) -> std::io::Result<Replay> {
    replay(File::open(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .map(|e| encode_line(e) + "\n")
            .collect::<String>()
    }

    #[test]
    fn line_round_trip() {
        for e in sample_entries() {
            let line = encode_line(&e);
            assert!(!line.contains('\n'));
            assert_eq!(decode_line(&line).unwrap(), e);
        }
    }

    #[test]
    fn line_has_event_tag() {
        let line = encode_line(&sample_entries()[2]);
        assert!(line.contains("\"event\":\"segment\""));
        assert!(line.contains("\"offset_ms\":12.5"));
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
    fn silence_fill_is_not_a_loss() {
        assert!(DiscKind::DataDiscontinuity.is_loss());
        assert!(DiscKind::TimestampError.is_loss());
        assert!(!DiscKind::SilenceFilled.is_loss());
    }

    #[test]
    fn journal_file_appends_and_replays() {
        let dir = crate::test_util::TempDir::new("journal");
        let path = dir.path().join("journal.jsonl");
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
