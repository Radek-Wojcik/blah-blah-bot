//! Rebuilding a meeting from its session journal.
//!
//! The journal is the source of truth for what was recorded. It is replayed when a
//! recording finishes normally (to fill the database) and after a crash (to recover the
//! meeting). At most the segment that was being written when the process died is lost.

use std::path::Path;

use crate::error::Result;
use crate::journal::{replay_file, Problem, SegmentInfo, JOURNAL_FILE_NAME};
use crate::timeline::samples_to_ms;
use crate::wav::{parse_header, HEADER_LEN};

/// A track as it can be used after recovery.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredTrack {
    pub name: String,
    /// Kind of source as journaled.
    pub source: String,
    pub pid: Option<u32>,
    /// Start on the session clock.
    pub offset_ms: f64,
    /// True when the track was closed cleanly.
    pub ended: bool,
    /// Where the usable audio ends on the session clock.
    pub end_offset_ms: f64,
    /// Samples in the usable segments.
    pub total_samples: u64,
    /// Segments that exist on disk with the length the journal promises.
    pub segments: Vec<SegmentInfo>,
    /// Continuity problems found in the usable segments.
    pub problems: Vec<Problem>,
    /// Segments the journal lists but whose file is missing or too short.
    pub unusable_segments: Vec<u32>,
}

/// A whole meeting rebuilt from the journal.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredMeeting {
    pub started_unix_ms: u64,
    pub sample_rate: u32,
    pub segment_samples: u64,
    /// Length of the meeting: declared by the journal when it ended cleanly, otherwise the
    /// latest end of any track or screenshot.
    pub duration_ms: f64,
    /// True when the journal has a `session_end` entry (the recording was stopped normally).
    pub clean: bool,
    pub tracks: Vec<RecoveredTrack>,
    /// Screenshots as `(file, offset_ms)`.
    pub screenshots: Vec<(String, f64)>,
    /// Journal lines that could not be read (a torn last line after a crash).
    pub bad_lines: usize,
}

/// Does `path` hold a WAVE file with at least `samples` samples of audio?
fn segment_is_complete(path: &Path, samples: u64) -> bool {
    use std::io::Read;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() < HEADER_LEN as u64 + samples * 2 {
        return false;
    }
    let mut header = [0u8; HEADER_LEN];
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    if f.read_exact(&mut header).is_err() {
        return false;
    }
    parse_header(&header).is_ok()
}

/// Replay the journal in `dir` and check the segment files it lists.
pub fn recover(dir: &Path) -> Result<RecoveredMeeting> {
    let replay = replay_file(&dir.join(JOURNAL_FILE_NAME))?;
    let rate = replay.sample_rate;
    let mut tracks = Vec::new();
    for (name, t) in &replay.tracks {
        let mut usable = Vec::new();
        let mut unusable = Vec::new();
        for s in &t.segments {
            if segment_is_complete(&dir.join(&s.file), s.samples) {
                usable.push(s.clone());
            } else {
                unusable.push(s.segment);
            }
        }
        let total: u64 = usable.iter().map(|s| s.samples).sum();
        let usable_track = crate::journal::TrackReplay {
            segments: usable.clone(),
            ..t.clone()
        };
        tracks.push(RecoveredTrack {
            name: name.clone(),
            source: t.source.clone(),
            pid: t.pid,
            offset_ms: t.offset_ms,
            ended: t.ended,
            end_offset_ms: t.offset_ms + samples_to_ms(total, rate),
            total_samples: total,
            problems: usable_track.problems(replay.segment_samples, rate),
            segments: usable,
            unusable_segments: unusable,
        });
    }
    let latest = tracks
        .iter()
        .map(|t| t.end_offset_ms)
        .chain(replay.screenshots.iter().map(|(_, o)| *o))
        .fold(0.0_f64, f64::max);
    Ok(RecoveredMeeting {
        started_unix_ms: replay.started_unix_ms,
        sample_rate: rate,
        segment_samples: replay.segment_samples,
        duration_ms: replay.duration_ms.unwrap_or(latest),
        clean: replay.session_ended,
        tracks,
        screenshots: replay.screenshots,
        bad_lines: replay.bad_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Entry, Journal};
    use crate::test_util::TempDir;
    use crate::track::TrackWriter;
    use std::sync::{Arc, Mutex};

    const RATE: u32 = 16_000;
    const SEG: u64 = 1_600;

    fn start(dir: &Path) -> Arc<Mutex<Journal>> {
        let j = Arc::new(Mutex::new(
            Journal::create(&dir.join(JOURNAL_FILE_NAME)).unwrap(),
        ));
        j.lock()
            .unwrap()
            .append(&Entry::SessionStart {
                started_unix_ms: 42,
                sample_rate: RATE,
                segment_samples: SEG,
                format: "pcm".into(),
            })
            .unwrap();
        j
    }

    fn write_track(dir: &Path, j: &Arc<Mutex<Journal>>, name: &str, offset: f64, samples: usize) {
        j.lock()
            .unwrap()
            .append(&Entry::TrackStart {
                track: name.into(),
                source: "mic".into(),
                pid: None,
                include_tree: None,
                offset_ms: offset,
            })
            .unwrap();
        let mut w = TrackWriter::new(name, dir, j.clone(), offset, RATE, SEG).unwrap();
        w.push(&vec![100i16; samples]).unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn clean_session_is_recovered_as_is() {
        let d = TempDir::new("rec-clean");
        let j = start(d.path());
        write_track(d.path(), &j, "mic", 0.0, 4_000);
        j.lock()
            .unwrap()
            .append(&Entry::Screenshot {
                file: "img/0001.png".into(),
                offset_ms: 150.0,
            })
            .unwrap();
        j.lock()
            .unwrap()
            .append(&Entry::SessionEnd { duration_ms: 250.0 })
            .unwrap();
        let m = recover(d.path()).unwrap();
        assert!(m.clean);
        assert_eq!(m.duration_ms, 250.0);
        assert_eq!(m.tracks.len(), 1);
        assert_eq!(m.tracks[0].total_samples, 4_000);
        assert_eq!(m.tracks[0].segments.len(), 3);
        assert!(m.tracks[0].problems.is_empty());
        assert_eq!(m.screenshots, vec![("img/0001.png".to_string(), 150.0)]);
    }

    #[test]
    fn unfinished_session_uses_latest_track_end() {
        let d = TempDir::new("rec-crash");
        let j = start(d.path());
        write_track(d.path(), &j, "mic", 0.0, 3_200);
        write_track(d.path(), &j, "app", 500.0, 1_600);
        let m = recover(d.path()).unwrap();
        assert!(!m.clean);
        // mic ends at 200 ms, app at 500 + 100 ms.
        assert_eq!(m.duration_ms, 600.0);
    }

    #[test]
    fn torn_last_line_is_tolerated() {
        let d = TempDir::new("rec-torn");
        let j = start(d.path());
        write_track(d.path(), &j, "mic", 0.0, 1_600);
        drop(j);
        let path = d.path().join(JOURNAL_FILE_NAME);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"event\":\"segment\",\"track\":\"mic\",\"segm");
        std::fs::write(&path, text).unwrap();
        let m = recover(d.path()).unwrap();
        assert_eq!(m.bad_lines, 1);
        assert_eq!(m.tracks[0].total_samples, 1_600);
    }

    #[test]
    fn missing_segment_file_is_unusable() {
        let d = TempDir::new("rec-missing");
        let j = start(d.path());
        write_track(d.path(), &j, "mic", 0.0, 3_200);
        std::fs::remove_file(d.path().join("mic").join("seg-00001.wav")).unwrap();
        let m = recover(d.path()).unwrap();
        assert_eq!(m.tracks[0].unusable_segments, vec![1]);
        assert_eq!(m.tracks[0].total_samples, 1_600);
    }

    #[test]
    fn truncated_segment_file_is_unusable() {
        let d = TempDir::new("rec-trunc");
        let j = start(d.path());
        write_track(d.path(), &j, "mic", 0.0, 1_600);
        let f = d.path().join("mic").join("seg-00000.wav");
        let bytes = std::fs::read(&f).unwrap();
        std::fs::write(&f, &bytes[..bytes.len() / 2]).unwrap();
        let m = recover(d.path()).unwrap();
        assert_eq!(m.tracks[0].unusable_segments, vec![0]);
        assert_eq!(m.tracks[0].total_samples, 0);
    }

    #[test]
    fn empty_journal_directory_is_an_error() {
        let d = TempDir::new("rec-none");
        assert!(recover(d.path()).is_err());
    }
}
