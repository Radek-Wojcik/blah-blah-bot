//! Stages of a processing job and the rules for moving between them.
//!
//! This module is pure: it looks at the persisted rows and says what happens next.

use serde::{Deserialize, Serialize};
use store::{JobRow, LlmKind, StageStatus};

/// A step of the pipeline, in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Transcription per track with word timestamps.
    Asr,
    /// Speaker segments for every track that mixes several people.
    Diarize,
    /// One transcript from all tracks.
    Merge,
    /// Matches the voice of every detected speaker with the known voice profiles.
    SpeakerId,
    /// Cuts short sample clips of every detected voice for naming.
    Clips,
    /// Describes each screenshot with the transcript around it as context.
    Vision,
    /// Finds tasks, decisions and other items in windows of the transcript (language model).
    Extract,
    /// Deduplicates the items, links them to topics and issues, writes the summary and the
    /// draft report (language model).
    Reduce,
    /// Removes the source audio once every other stage succeeded and the draft exists, unless
    /// the retention policy keeps it. It runs last so that nothing that reads the audio can
    /// be missing it and a failed job never loses its audio.
    AudioCleanup,
}

/// Every stage in execution order.
pub const STAGES: [Stage; 9] = [
    Stage::Asr,
    Stage::Diarize,
    Stage::Merge,
    Stage::SpeakerId,
    Stage::Clips,
    Stage::Vision,
    Stage::Extract,
    Stage::Reduce,
    Stage::AudioCleanup,
];

/// Stages that read the full audio. They are all done long before the audio is deleted: the
/// cleanup waits for every stage of the job (see [`audio_is_expendable`]), this list only names
/// the ones that need it. Voice embeddings come from the diarization output, so only the
/// clips are added.
pub const AUDIO_DEPENDENT: [Stage; 3] = [Stage::Asr, Stage::Diarize, Stage::Clips];

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asr => "asr",
            Self::Diarize => "diarize",
            Self::Merge => "merge",
            Self::SpeakerId => "speaker_id",
            Self::Clips => "clips",
            Self::AudioCleanup => "audio_cleanup",
            Self::Vision => "vision",
            Self::Extract => "extract",
            Self::Reduce => "reduce",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        STAGES.iter().copied().find(|st| st.as_str() == s)
    }

    /// Whether the stage is one of the two that use a language model.
    pub fn uses_llm(self) -> bool {
        matches!(self, Self::Extract | Self::Reduce)
    }

    /// Whether the stage loads a model onto the GPU and so must pass the GPU gate.
    ///
    /// `llm` is the backend chosen for the job. The language-model stages load a model
    /// only with the managed `llama-server`; an external server or a hosted API uses no
    /// GPU of this machine, and without a backend the stages do not run at all.
    pub fn needs_gpu(self, llm: Option<LlmKind>) -> bool {
        match self {
            Self::Asr | Self::Diarize | Self::Vision => true,
            Self::Extract | Self::Reduce => llm == Some(LlmKind::Managed),
            Self::Merge | Self::SpeakerId | Self::Clips | Self::AudioCleanup => false,
        }
    }

    /// Relative weight of this stage in overall job progress (sums to 100 across all 9 stages).
    pub const fn weight(self) -> f32 {
        match self {
            Self::Asr => 30.0,
            Self::Diarize => 28.0,
            Self::Merge => 2.0,
            Self::SpeakerId => 2.0,
            Self::Clips => 2.0,
            Self::AudioCleanup => 1.0,
            Self::Vision => 12.0,
            Self::Extract => 15.0,
            Self::Reduce => 8.0,
        }
    }
}

/// What the queue should do next for one job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Next {
    /// Run this stage.
    Run { stage: Stage },
    /// A stage failed and needs an explicit retry.
    Failed { stage: Stage },
    /// Every stage is done.
    Finished,
}

/// Decide the next step from the persisted stage rows.
///
/// Stages run strictly in order. A failed stage blocks the job until it is retried; a
/// stage left `running` by a crash is treated as pending (see [`recover`]).
pub fn next_step(job: &JobRow) -> Next {
    for stage in STAGES {
        let status = job
            .stages
            .iter()
            .find(|s| s.stage == stage.as_str())
            .map(|s| s.status)
            .unwrap_or(StageStatus::Pending);
        match status {
            StageStatus::Done => continue,
            StageStatus::Failed => return Next::Failed { stage },
            StageStatus::Pending | StageStatus::Running => return Next::Run { stage },
        }
    }
    Next::Finished
}

/// Whether the source audio of this job may be deleted: every stage except the cleanup
/// itself is done, so the transcript, the clips, the screenshot descriptions and the draft
/// exist. A skipped stage counts as done. A failed, pending or running stage keeps the audio.
pub fn audio_is_expendable(job: &JobRow) -> bool {
    STAGES
        .iter()
        .filter(|st| **st != Stage::AudioCleanup)
        .all(|stage| {
            job.stages
                .iter()
                .any(|s| s.stage == stage.as_str() && s.status == StageStatus::Done)
        })
}

/// Whether a stage row represents a skipped stage (`done` with an explanatory `note`).
fn row_is_skipped(row: &store::StageRow) -> bool {
    row.status == StageStatus::Done && row.note.is_some()
}

/// Normalized weight (`0.0..=1.0`) of `stage` among the non-skipped stages of `job`.
pub fn stage_weight(job: &JobRow, stage: Stage) -> f32 {
    let mut total = 0.0f32;
    let mut target = 0.0f32;
    for st in STAGES {
        let row = job.stages.iter().find(|s| s.stage == st.as_str());
        if row.is_some_and(row_is_skipped) {
            continue;
        }
        let w = st.weight();
        total += w;
        if st == stage {
            target = w;
        }
    }
    if total <= 0.0 {
        0.0
    } else {
        target / total
    }
}

/// Overall weighted progress (`0.0..=1.0`) of a job across its non-skipped stages.
pub fn progress(job: &JobRow) -> f32 {
    progress_with_live(job, None)
}

/// Overall weighted progress (`0.0..=1.0`) of a job across its non-skipped stages, smoothly
/// incorporating the intra-stage progress (`0.0..=1.0`) of the currently running stage.
pub fn progress_with_live(job: &JobRow, live: Option<(Stage, f32)>) -> f32 {
    let mut total = 0.0f32;
    let mut completed = 0.0f32;
    for st in STAGES {
        let row = job.stages.iter().find(|s| s.stage == st.as_str());
        if row.is_some_and(row_is_skipped) {
            continue;
        }
        let w = st.weight();
        total += w;
        if row.is_some_and(|r| r.status == StageStatus::Done) {
            completed += w;
        } else if let Some((live_stage, fraction)) = live {
            if live_stage == st {
                completed += w * fraction.clamp(0.0, 1.0);
            }
        }
    }
    if total <= 0.0 {
        1.0
    } else {
        (completed / total).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::StageRow;

    /// A job whose first stages have the given statuses; the rest are pending.
    fn job(statuses: &[StageStatus]) -> JobRow {
        JobRow {
            meeting_id: "m".into(),
            created_unix_ms: 0,
            updated_unix_ms: 0,
            options: "{}".into(),
            scheduled_unix_ms: None,
            stages: STAGES
                .iter()
                .enumerate()
                .map(|(i, st)| StageRow {
                    stage: st.as_str().into(),
                    status: statuses.get(i).copied().unwrap_or(StageStatus::Pending),
                    attempts: 0,
                    started_unix_ms: None,
                    finished_unix_ms: None,
                    error: None,
                    note: None,
                })
                .collect(),
        }
    }

    use StageStatus::{Done, Failed, Running};

    #[test]
    fn fresh_job_starts_with_asr() {
        let j = job(&[]);
        assert_eq!(next_step(&j), Next::Run { stage: Stage::Asr });
    }

    #[test]
    fn stages_run_in_order() {
        let j = job(&[Done, Done]);
        assert_eq!(
            next_step(&j),
            Next::Run {
                stage: Stage::Merge
            }
        );
    }

    #[test]
    fn the_language_model_stages_come_last_in_order() {
        assert_eq!(
            STAGES[STAGES.len() - 4..],
            [
                Stage::Vision,
                Stage::Extract,
                Stage::Reduce,
                Stage::AudioCleanup
            ]
        );
        assert_eq!(
            next_step(&job(&[Done; 6])),
            Next::Run {
                stage: Stage::Extract
            }
        );
        assert_eq!(
            next_step(&job(&[Done; 7])),
            Next::Run {
                stage: Stage::Reduce
            }
        );
    }

    #[test]
    fn the_audio_cleanup_is_the_last_stage() {
        assert_eq!(STAGES[STAGES.len() - 1], Stage::AudioCleanup);
        assert_eq!(
            next_step(&job(&[Done; 8])),
            Next::Run {
                stage: Stage::AudioCleanup
            }
        );
    }

    #[test]
    fn running_stage_is_resumed() {
        let j = job(&[Done, Running]);
        assert_eq!(
            next_step(&j),
            Next::Run {
                stage: Stage::Diarize
            }
        );
    }

    #[test]
    fn failed_stage_blocks_later_ones() {
        let j = job(&[Done, Failed]);
        assert_eq!(
            next_step(&j),
            Next::Failed {
                stage: Stage::Diarize
            }
        );
    }

    #[test]
    fn all_done_is_finished() {
        assert_eq!(next_step(&job(&[Done; 9])), Next::Finished);
        // A job made before the language-model stages existed has no rows for them: they
        // count as pending, so the job runs (or skips) them.
        let mut old = job(&[Done; 6]);
        old.stages.truncate(6);
        assert_eq!(
            next_step(&old),
            Next::Run {
                stage: Stage::Extract
            }
        );
    }

    #[test]
    fn audio_is_expendable_only_after_every_other_stage() {
        assert!(!audio_is_expendable(&job(&[Done])));
        assert!(!audio_is_expendable(&job(&[Done, Failed])));
        // The clips are cut from the audio and the draft is the last thing written, so
        // neither may be missing.
        assert!(!audio_is_expendable(&job(&[Done; 5])));
        assert!(!audio_is_expendable(&job(&[Done; 7])));
        assert!(audio_is_expendable(&job(&[Done; 8])));
        // A failed language-model stage keeps the audio even when everything else is done.
        let mut statuses = [Done; 8];
        statuses[7] = Failed;
        assert!(!audio_is_expendable(&job(&statuses)));
        // The cleanup row itself does not matter.
        assert!(audio_is_expendable(&job(&[Done; 9])));
    }

    #[test]
    fn progress_counts_done_stages() {
        let p = progress(&job(&[Done, Done]));
        // Asr (30) + Diarize (28) = 58 / 100.
        assert!((p - 0.58).abs() < 1e-6, "got {p}");
    }

    #[test]
    fn progress_weights_intra_stage_and_normalizes_skipped_stages() {
        let mut j = job(&[Done, Running]);
        // Smooth intra-stage progress while Diarize is halfway (30 + 28 * 0.5 = 44 / 100).
        let p_half = progress_with_live(&j, Some((Stage::Diarize, 0.5)));
        assert!((p_half - 0.44).abs() < 1e-6, "got {p_half}");

        // Mark Extract (15) and Reduce (8) as skipped (`done` with a note).
        // Remaining active weight is 100 - 15 - 8 = 77.
        for name in ["extract", "reduce"] {
            let row = j.stages.iter_mut().find(|s| s.stage == name).unwrap();
            row.status = Done;
            row.note = Some("the meeting has no cycle".into());
        }
        let p_norm = progress_with_live(&j, Some((Stage::Diarize, 0.5)));
        assert!((p_norm - 44.0 / 77.0).abs() < 1e-6, "got {p_norm}");
        assert!((stage_weight(&j, Stage::Asr) - 30.0 / 77.0).abs() < 1e-6);
        assert_eq!(stage_weight(&j, Stage::Extract), 0.0);
    }

    #[test]
    fn stage_names_round_trip() {
        for st in STAGES {
            assert_eq!(Stage::parse(st.as_str()), Some(st));
        }
        assert_eq!(Stage::Extract.as_str(), "extract");
        assert_eq!(Stage::Reduce.as_str(), "reduce");
        assert_eq!(Stage::parse("publish"), None);
    }

    #[test]
    fn only_the_managed_backend_puts_the_language_model_on_the_gpu() {
        use LlmKind::{Cloud, External, Managed};
        for stage in [Stage::Extract, Stage::Reduce] {
            assert!(stage.uses_llm());
            assert!(stage.needs_gpu(Some(Managed)));
            assert!(!stage.needs_gpu(Some(External)));
            assert!(!stage.needs_gpu(Some(Cloud)));
            assert!(!stage.needs_gpu(None));
        }
        // The other stages do not depend on the backend.
        for kind in [None, Some(Managed), Some(External), Some(Cloud)] {
            assert!(Stage::Asr.needs_gpu(kind));
            assert!(Stage::Diarize.needs_gpu(kind));
            assert!(Stage::Vision.needs_gpu(kind));
            assert!(!Stage::Merge.needs_gpu(kind));
            assert!(!Stage::SpeakerId.needs_gpu(kind));
            assert!(!Stage::Clips.needs_gpu(kind));
            assert!(!Stage::AudioCleanup.needs_gpu(kind));
            assert!(!Stage::Asr.uses_llm());
        }
    }
}
