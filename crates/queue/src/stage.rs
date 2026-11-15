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
    /// Removes the full audio once every audio-dependent stage succeeded.
    AudioCleanup,
    /// Describes each screenshot with the transcript around it as context.
    Vision,
    /// Finds tasks, decisions and other items in windows of the transcript (language model).
    Extract,
    /// Deduplicates the items, links them to topics and issues, writes the summary and the
    /// draft report (language model).
    Reduce,
}

/// Every stage in execution order.
pub const STAGES: [Stage; 9] = [
    Stage::Asr,
    Stage::Diarize,
    Stage::Merge,
    Stage::SpeakerId,
    Stage::Clips,
    Stage::AudioCleanup,
    Stage::Vision,
    Stage::Extract,
    Stage::Reduce,
];

/// Stages that read the full audio. Audio may be deleted only after all of them are done.
/// Voice embeddings come from the diarization output, so only the clips are added.
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

/// Whether the full audio of this job may be deleted: every audio-dependent stage is done.
pub fn audio_is_expendable(job: &JobRow) -> bool {
    AUDIO_DEPENDENT.iter().all(|stage| {
        job.stages
            .iter()
            .any(|s| s.stage == stage.as_str() && s.status == StageStatus::Done)
    })
}

/// Share of stages that are done, for progress bars.
pub fn progress(job: &JobRow) -> f32 {
    let done = STAGES
        .iter()
        .filter(|st| {
            job.stages
                .iter()
                .any(|s| s.stage == st.as_str() && s.status == StageStatus::Done)
        })
        .count();
    done as f32 / STAGES.len() as f32
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
            STAGES[STAGES.len() - 3..],
            [Stage::Vision, Stage::Extract, Stage::Reduce]
        );
        assert_eq!(
            next_step(&job(&[Done; 7])),
            Next::Run {
                stage: Stage::Extract
            }
        );
        assert_eq!(
            next_step(&job(&[Done; 8])),
            Next::Run {
                stage: Stage::Reduce
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
        let mut old = job(&[Done; 7]);
        old.stages.truncate(7);
        assert_eq!(
            next_step(&old),
            Next::Run {
                stage: Stage::Extract
            }
        );
    }

    #[test]
    fn audio_is_expendable_only_after_every_audio_stage() {
        assert!(!audio_is_expendable(&job(&[Done])));
        assert!(!audio_is_expendable(&job(&[Done, Failed])));
        // The clips are cut from the audio, so they must be done too.
        assert!(!audio_is_expendable(&job(&[Done, Done, Done, Done])));
        assert!(audio_is_expendable(&job(&[Done, Done, Done, Done, Done])));
    }

    #[test]
    fn progress_counts_done_stages() {
        let p = progress(&job(&[Done, Done]));
        assert!((p - 2.0 / 9.0).abs() < 1e-6);
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
