//! Integration tests: whole sessions recorded from generated tones, replayed from disk.
//!
//! The tone backend goes through the real session clock, track writer, journal and
//! recovery code; only the audio hardware is replaced.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use recorder::journal::Problem;
use recorder::recovery::recover;
use recorder::session::{Session, SessionError, SourceSpec, ToneBackend};
use recorder::timeline::SAMPLE_RATE;
use recorder::wav::read_samples;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bbb-it-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ms(n: u64) {
    sleep(Duration::from_millis(n));
}

fn start(dir: &Path) -> Session {
    // One-second segments keep the tests short.
    Session::start(dir, 1, Arc::new(ToneBackend)).unwrap()
}

/// All samples of a track, read from its segment files in journal order.
fn track_samples(dir: &Path, track: &recorder::recovery::RecoveredTrack) -> Vec<i16> {
    let mut out = Vec::new();
    for seg in &track.segments {
        let (_, samples) = read_samples(&dir.join(&seg.file)).unwrap();
        out.extend(samples);
    }
    out
}

/// Frequency of a sine wave estimated from its zero crossings.
fn estimate_hz(samples: &[i16]) -> f64 {
    let crossings = samples
        .windows(2)
        .filter(|w| (w[0] < 0) != (w[1] < 0))
        .count();
    crossings as f64 / 2.0 / (samples.len() as f64 / f64::from(SAMPLE_RATE))
}

#[test]
fn two_tones_are_recorded_as_separate_continuous_tracks() {
    let dir = TempDir::new("two");
    let mut s = start(dir.path());
    s.add_source(SourceSpec::Tone { hz: 440 }, "A").unwrap();
    s.add_source(SourceSpec::Tone { hz: 1000 }, "B").unwrap();
    ms(2_600);
    let duration = s.stop().unwrap();

    let m = recover(dir.path()).unwrap();
    assert!(m.clean);
    assert_eq!(m.tracks.len(), 2);
    for (name, hz) in [("tone-440", 440.0), ("tone-1000", 1000.0)] {
        let t = m.tracks.iter().find(|t| t.name == name).unwrap();
        assert!(t.ended, "{name} should be closed");
        assert!(t.problems.is_empty(), "{name}: {:?}", t.problems);
        assert!(t.unusable_segments.is_empty());
        // Duration: close to the session length.
        let track_ms = t.total_samples as f64 * 1000.0 / f64::from(SAMPLE_RATE);
        assert!(
            (track_ms - (duration - t.offset_ms)).abs() < 150.0,
            "{name}: {track_ms} ms vs session {duration} ms"
        );
        // Separation: each track carries only its own tone.
        let est = estimate_hz(&track_samples(dir.path(), t));
        assert!((est - hz).abs() < hz * 0.02, "{name}: estimated {est} Hz");
    }
}

#[test]
fn source_added_mid_recording_is_aligned_on_the_session_clock() {
    let dir = TempDir::new("add");
    let mut s = start(dir.path());
    s.add_source(SourceSpec::Tone { hz: 440 }, "A").unwrap();
    ms(1_200);
    let before_add = s.now_ms();
    s.add_source(SourceSpec::Tone { hz: 880 }, "B").unwrap();
    ms(1_300);
    s.stop().unwrap();

    let m = recover(dir.path()).unwrap();
    let a = m.tracks.iter().find(|t| t.name == "tone-440").unwrap();
    let b = m.tracks.iter().find(|t| t.name == "tone-880").unwrap();
    assert!(
        a.offset_ms < 100.0,
        "first track starts near zero: {}",
        a.offset_ms
    );
    // The new track starts when it was added, within the 50 ms alignment target.
    assert!(
        b.offset_ms >= before_add && b.offset_ms - before_add < 50.0,
        "offset {} vs added at {before_add}",
        b.offset_ms
    );
    // Both tracks end together at the end of the session.
    assert!((a.end_offset_ms - b.end_offset_ms).abs() < 100.0);
}

#[test]
fn removing_a_source_ends_only_that_track() {
    let dir = TempDir::new("remove");
    let mut s = start(dir.path());
    s.add_source(SourceSpec::Tone { hz: 440 }, "A").unwrap();
    s.add_source(SourceSpec::Tone { hz: 880 }, "B").unwrap();
    ms(1_300);
    s.remove_source("tone:440").unwrap();
    let snap = s.snapshot();
    let a = snap.sources.iter().find(|x| x.id == "tone:440").unwrap();
    assert_eq!(a.phase, "ended");
    assert_eq!(a.detail.as_deref(), Some("Removed"));
    ms(1_200);
    let duration = s.stop().unwrap();

    let m = recover(dir.path()).unwrap();
    let a = m.tracks.iter().find(|t| t.name == "tone-440").unwrap();
    let b = m.tracks.iter().find(|t| t.name == "tone-880").unwrap();
    assert!(a.ended && b.ended);
    assert!(
        a.end_offset_ms < 1_700.0,
        "A stopped early: {}",
        a.end_offset_ms
    );
    assert!(b.end_offset_ms > duration - 150.0, "B kept recording");
    assert!(a.problems.is_empty() && b.problems.is_empty());
}

#[test]
fn a_removed_source_can_be_added_again_as_a_new_track() {
    let dir = TempDir::new("readd");
    let mut s = start(dir.path());
    let first = s.add_source(SourceSpec::Tone { hz: 440 }, "A").unwrap();
    ms(400);
    s.remove_source("tone:440").unwrap();
    let second = s.add_source(SourceSpec::Tone { hz: 440 }, "A").unwrap();
    ms(400);
    s.stop().unwrap();
    assert_ne!(
        first, second,
        "the second track must not overwrite the first"
    );
    let m = recover(dir.path()).unwrap();
    assert_eq!(m.tracks.len(), 2);
}

#[test]
fn duplicate_and_unknown_sources_are_rejected() {
    let dir = TempDir::new("dup");
    let mut s = start(dir.path());
    s.add_source(SourceSpec::Tone { hz: 440 }, "A").unwrap();
    assert!(matches!(
        s.add_source(SourceSpec::Tone { hz: 440 }, "A"),
        Err(SessionError::SourceAlreadyAdded(_))
    ));
    assert!(matches!(
        s.remove_source("tone:999"),
        Err(SessionError::SourceNotFound(_))
    ));
    s.stop().unwrap();
}

#[test]
fn screenshots_carry_their_offsets() {
    let dir = TempDir::new("shots");
    let mut s = start(dir.path());
    s.add_source(SourceSpec::Tone { hz: 440 }, "A").unwrap();
    ms(300);
    let (f1, o1) = s.next_screenshot().unwrap();
    ms(400);
    let (f2, o2) = s.next_screenshot().unwrap();
    s.stop().unwrap();
    assert!(f1.starts_with("img/0001-") && f2.starts_with("img/0002-"));
    assert!(o2 - o1 >= 400.0 && o2 - o1 < 460.0);
    let m = recover(dir.path()).unwrap();
    assert_eq!(m.screenshots, vec![(f1, o1), (f2, o2)]);
}

#[test]
fn snapshot_reports_levels_while_recording() {
    let dir = TempDir::new("level");
    let mut s = start(dir.path());
    s.add_source(SourceSpec::Tone { hz: 440 }, "A").unwrap();
    ms(300);
    let snap = s.snapshot();
    assert_eq!(snap.sources[0].phase, "recording");
    assert!(
        snap.sources[0].level > 0.1,
        "level {}",
        snap.sources[0].level
    );
    s.stop().unwrap();
}

/// Helper run in a child process by `killed_process_loses_at_most_one_segment`.
#[test]
fn kill_child_helper() {
    let Ok(dir) = std::env::var("BBB_KILL_CHILD_DIR") else {
        return; // Not running as the child.
    };
    let dir = PathBuf::from(dir);
    let mut s = start(&dir);
    s.add_source(SourceSpec::Tone { hz: 440 }, "A").unwrap();
    s.add_source(SourceSpec::Tone { hz: 1000 }, "B").unwrap();
    std::fs::write(dir.join("READY"), b"1").unwrap();
    // Record until the parent kills this process.
    sleep(Duration::from_secs(120));
    drop(s);
}

#[test]
fn killed_process_loses_at_most_one_segment() {
    let dir = TempDir::new("kill");
    let exe = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(exe)
        .args([
            "--exact",
            "kill_child_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("BBB_KILL_CHILD_DIR", dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let ready = dir.path().join("READY");
    for _ in 0..200 {
        if ready.exists() {
            break;
        }
        ms(50);
    }
    assert!(ready.exists(), "child did not start recording");
    ms(3_400);
    let recorded_before_kill_ms = 3_400.0;
    // TerminateProcess: no destructors, no flushing, no `session_end`.
    child.kill().unwrap();
    child.wait().unwrap();

    let m = recover(dir.path()).unwrap();
    assert!(!m.clean, "a killed recording has no session end");
    assert_eq!(m.tracks.len(), 2);
    let seg_ms = 1_000.0;
    for t in &m.tracks {
        let kept_ms = t.total_samples as f64 * 1000.0 / f64::from(SAMPLE_RATE);
        // The audio of the segment being written is lost, nothing more.
        assert!(
            kept_ms >= recorded_before_kill_ms - seg_ms - 300.0,
            "{}: kept only {kept_ms} ms",
            t.name
        );
        assert!(kept_ms >= 2_000.0, "{}: kept {kept_ms} ms", t.name);
        assert!(t.problems.is_empty(), "{}: {:?}", t.name, t.problems);
        assert!(!t.ended);
        let est = estimate_hz(&track_samples(dir.path(), t));
        let want = if t.name == "tone-440" { 440.0 } else { 1000.0 };
        assert!((est - want).abs() < want * 0.02);
    }
    assert!(m.duration_ms > 2_000.0);
    // Every recovered offset check passes on the journal itself.
    let _ = Problem::MissingSegment {
        expected: 0,
        found: 0,
    };
}
