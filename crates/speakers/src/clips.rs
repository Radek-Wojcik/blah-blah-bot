//! Choosing clear, non-overlapping sample clips for every detected voice.

use serde::{Deserialize, Serialize};

/// A stretch of one speaker's speech as diarization reports it (milliseconds on the session
/// clock). Turns come from the exclusive output, so turns of different speakers do not overlap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: String,
}

/// A chosen clip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipSpan {
    pub speaker: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipParams {
    /// Clips per voice.
    pub count: usize,
    /// Shortest useful clip.
    pub min_ms: u64,
    /// Longest clip; longer stretches are cut to this length around their middle.
    pub max_ms: u64,
    /// Two turns of one speaker closer than this are joined into one stretch.
    pub join_gap_ms: u64,
    /// A clip must have no other speaker's speech within this distance before or after it.
    pub isolation_ms: u64,
    /// Preferred distance between the starts of two clips of one voice.
    pub spacing_ms: u64,
}

impl Default for ClipParams {
    fn default() -> Self {
        Self {
            count: 3,
            min_ms: 2_500,
            max_ms: 8_000,
            join_gap_ms: 400,
            isolation_ms: 500,
            spacing_ms: 30_000,
        }
    }
}

/// Pick up to `params.count` clips for each speaker in `speakers`.
///
/// A candidate is a stretch of one speaker's speech (adjacent turns joined) at least
/// `min_ms` long and isolated from everyone else's speech. Longer candidates rank first and
/// clips are spread over the meeting: a candidate closer than `spacing_ms` to one already
/// chosen is used only when there are not enough distant ones. The result is ordered by
/// speaker and then by time. A speaker with no suitable stretch gets no clips.
pub fn select_clips(turns: &[Turn], speakers: &[String], params: &ClipParams) -> Vec<ClipSpan> {
    select_clips_with(turns, speakers, params, |_| true)
}

/// Like [`select_clips`], but a candidate is used only when `accept` agrees (for example,
/// when the audio of the stretch turns out to be silent).
pub fn select_clips_with(
    turns: &[Turn],
    speakers: &[String],
    params: &ClipParams,
    mut accept: impl FnMut(&ClipSpan) -> bool,
) -> Vec<ClipSpan> {
    let mut sorted: Vec<&Turn> = turns.iter().filter(|t| t.end_ms > t.start_ms).collect();
    sorted.sort_by_key(|t| (t.start_ms, t.end_ms));

    let mut out = Vec::new();
    for speaker in speakers {
        let stretches = join_turns(&sorted, speaker, params.join_gap_ms);
        let mut candidates: Vec<(u64, u64)> = stretches
            .into_iter()
            .filter(|(s, e)| e - s >= params.min_ms)
            .filter(|(s, e)| is_isolated(&sorted, speaker, *s, *e, params.isolation_ms))
            .map(|(s, e)| cut_to(s, e, params.max_ms))
            .collect();
        // Longest first; earlier first on ties so the choice is deterministic.
        candidates.sort_by(|a, b| (b.1 - b.0).cmp(&(a.1 - a.0)).then(a.0.cmp(&b.0)));

        let mut chosen: Vec<(u64, u64)> = Vec::new();
        let mut rejected: Vec<(u64, u64)> = Vec::new();
        for pass_spaced in [true, false] {
            for &c in &candidates {
                if chosen.len() >= params.count {
                    break;
                }
                if chosen.contains(&c) || rejected.contains(&c) {
                    continue;
                }
                let far = chosen
                    .iter()
                    .all(|k| c.0.abs_diff(k.0) >= params.spacing_ms);
                if !pass_spaced || far {
                    let span = ClipSpan {
                        speaker: speaker.clone(),
                        start_ms: c.0,
                        end_ms: c.1,
                    };
                    if accept(&span) {
                        chosen.push(c);
                    } else {
                        rejected.push(c);
                    }
                }
            }
        }
        chosen.sort_unstable();
        out.extend(chosen.into_iter().map(|(s, e)| ClipSpan {
            speaker: speaker.clone(),
            start_ms: s,
            end_ms: e,
        }));
    }
    out
}

fn join_turns(sorted: &[&Turn], speaker: &str, gap: u64) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for t in sorted.iter().filter(|t| t.speaker == speaker) {
        match out.last_mut() {
            Some(last) if t.start_ms.saturating_sub(last.1) <= gap => last.1 = last.1.max(t.end_ms),
            _ => out.push((t.start_ms, t.end_ms)),
        }
    }
    out
}

fn is_isolated(sorted: &[&Turn], speaker: &str, start: u64, end: u64, margin: u64) -> bool {
    let from = start.saturating_sub(margin);
    let to = end + margin;
    sorted
        .iter()
        .filter(|t| t.speaker != speaker)
        .all(|t| t.end_ms <= from || t.start_ms >= to)
}

/// Cut a stretch to `max` milliseconds around its middle.
fn cut_to(start: u64, end: u64, max: u64) -> (u64, u64) {
    let len = end - start;
    if len <= max {
        return (start, end);
    }
    let s = start + (len - max) / 2;
    (s, s + max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(start: u64, end: u64, who: &str) -> Turn {
        Turn {
            start_ms: start,
            end_ms: end,
            speaker: who.into(),
        }
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn short_turns_give_no_clip() {
        let turns = [t(0, 2_000, "a"), t(5_000, 7_400, "a")];
        assert!(select_clips(&turns, &names(&["a"]), &ClipParams::default()).is_empty());
    }

    #[test]
    fn adjacent_turns_of_one_speaker_are_joined() {
        let turns = [t(0, 1_800, "a"), t(2_000, 4_000, "a")];
        let clips = select_clips(&turns, &names(&["a"]), &ClipParams::default());
        assert_eq!(clips.len(), 1);
        assert_eq!((clips[0].start_ms, clips[0].end_ms), (0, 4_000));
    }

    #[test]
    fn a_stretch_close_to_another_speaker_is_skipped() {
        let turns = [
            t(0, 4_000, "a"),
            t(4_200, 9_000, "b"),
            t(60_000, 64_000, "a"),
        ];
        let clips = select_clips(&turns, &names(&["a"]), &ClipParams::default());
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].start_ms, 60_000);
    }

    #[test]
    fn long_stretches_are_cut_around_the_middle() {
        let turns = [t(10_000, 30_000, "a")];
        let clips = select_clips(&turns, &names(&["a"]), &ClipParams::default());
        assert_eq!((clips[0].start_ms, clips[0].end_ms), (16_000, 24_000));
    }

    #[test]
    fn clips_are_spread_over_the_meeting_when_possible() {
        let turns = [
            t(0, 6_000, "a"),
            t(10_000, 16_000, "a"),
            t(100_000, 105_000, "a"),
            t(200_000, 204_000, "a"),
            t(300_000, 303_000, "a"),
        ];
        let clips = select_clips(&turns, &names(&["a"]), &ClipParams::default());
        let starts: Vec<u64> = clips.iter().map(|c| c.start_ms).collect();
        assert_eq!(starts.len(), 3);
        // The two close stretches (0 and 10 s) are not both used while distant ones exist.
        assert!(
            !(starts.contains(&0) && starts.contains(&10_000)),
            "{starts:?}"
        );
    }

    #[test]
    fn close_stretches_fill_up_when_nothing_else_exists() {
        let turns = [t(0, 6_000, "a"), t(10_000, 16_000, "a")];
        let clips = select_clips(&turns, &names(&["a"]), &ClipParams::default());
        assert_eq!(clips.len(), 2);
    }

    #[test]
    fn every_speaker_gets_its_own_clips_and_clips_never_overlap_other_speech() {
        let turns = [
            t(0, 5_000, "a"),
            t(6_000, 12_000, "b"),
            t(20_000, 26_000, "a"),
            t(40_000, 46_000, "b"),
        ];
        let params = ClipParams::default();
        let clips = select_clips(&turns, &names(&["a", "b"]), &params);
        for c in &clips {
            assert!(c.end_ms - c.start_ms >= params.min_ms);
            for other in turns.iter().filter(|x| x.speaker != c.speaker) {
                assert!(
                    other.end_ms <= c.start_ms || other.start_ms >= c.end_ms,
                    "{c:?} overlaps {other:?}"
                );
            }
        }
        assert!(clips.iter().any(|c| c.speaker == "a"));
        assert!(clips.iter().any(|c| c.speaker == "b"));
    }

    #[test]
    fn a_speaker_without_turns_gets_nothing() {
        assert!(select_clips(&[], &names(&["a"]), &ClipParams::default()).is_empty());
    }
}
