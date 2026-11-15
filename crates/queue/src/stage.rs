//! Stages of a processing job and the rules for moving between them.
//!
//! This module is pure: it looks at the persisted rows and says what happens next.

use serde::{Deserialize, Serialize};
use store::{JobRow, StageStatus};

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
    /// Removes the full audio once every audio-dependent stage succeeded.
    AudioCleanup,
    /// Describes each screenshot with the transcript around it as context.
    Vision,
}

/// Every stage in execution order.
pub const STAGES: [Stage; 5] = [
    Stage::Asr,
    Stage::Diarize,
    Stage::Merge,
    Stage::AudioCleanup,
    Stage::Vision,
];

/// Stages that read the full audio. Audio may be deleted only after all of them are done.
/// Later stages that read audio (voice embeddings, sample clips) are added here.
pub const AUDIO_DEPENDENT: [Stage; 2] = [Stage::Asr, Stage::Diarize];

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asr => "asr",
            Self::Diarize => "diarize",
            Self::Merge => "merge",
            Self::AudioCleanup => "audio_cleanup",
            Self::Vision => "vision",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        STAGES.iter().copied().find(|st| st.as_str() == s)
    }

    /// Whether the stage loads a model onto the GPU and so must pass the GPU gate.
    pub fn needs_gpu(self) -> bool {
        matches!(self, Self::Asr | Self::Diarize | Self::Vision)
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

    fn job(statuses: [StageStatus; 5]) -> JobRow {
        JobRow {
            meeting_id: "m".into(),
            created_unix_ms: 0,
            updated_unix_ms: 0,
            options: "{}".into(),
            stages: STAGES
                .iter()
                .zip(statuses)
                .map(|(st, status)| StageRow {
                    stage: st.as_str().into(),
                    status,
                    attempts: 0,
                    started_unix_ms: None,
                    finished_unix_ms: None,
                    error: None,
                })
                .collect(),
        }
    }

    use StageStatus::{Done, Failed, Pending, Running};

    #[test]
    fn fresh_job_starts_with_asr() {
        let j = job([Pending; 5]);
        assert_eq!(next_step(&j), Next::Run { stage: Stage::Asr });
    }

    #[test]
    fn stages_run_in_order() {
        let j = job([Done, Done, Pending, Pending, Pending]);
        assert_eq!(
            next_step(&j),
            Next::Run {
                stage: Stage::Merge
            }
        );
    }

    #[test]
    fn running_stage_is_resumed() {
        let j = job([Done, Running, Pending, Pending, Pending]);
        assert_eq!(
            next_step(&j),
            Next::Run {
                stage: Stage::Diarize
            }
        );
    }

    #[test]
    fn failed_stage_blocks_later_ones() {
        let j = job([Done, Failed, Pending, Pending, Pending]);
        assert_eq!(
            next_step(&j),
            Next::Failed {
                stage: Stage::Diarize
            }
        );
    }

    #[test]
    fn all_done_is_finished() {
        assert_eq!(next_step(&job([Done; 5])), Next::Finished);
    }

    #[test]
    fn audio_is_expendable_only_after_every_audio_stage() {
        assert!(!audio_is_expendable(&job([
            Done, Pending, Pending, Pending, Pending
        ])));
        assert!(!audio_is_expendable(&job([
            Done, Failed, Pending, Pending, Pending
        ])));
        assert!(audio_is_expendable(&job([
            Done, Done, Pending, Pending, Pending
        ])));
    }

    #[test]
    fn progress_counts_done_stages() {
        let p = progress(&job([Done, Done, Pending, Pending, Pending]));
        assert!((p - 0.4).abs() < 1e-6);
    }

    #[test]
    fn stage_names_round_trip() {
        for st in STAGES {
            assert_eq!(Stage::parse(st.as_str()), Some(st));
        }
        assert_eq!(Stage::parse("extract"), None);
    }
}
