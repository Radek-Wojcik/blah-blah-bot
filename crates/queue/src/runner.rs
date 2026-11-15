//! The production [`StageRunner`]: worker steps through `uv`, audio cleanup in Rust.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use store::Store;

use crate::cleanup::{self, AudioRetention};
use crate::stage::Stage;
use crate::supervisor::{now_ms, RunError, StageRunner};
use crate::voices::{self, VoiceSettings};
use crate::worker::{self, JobOptions, WorkerConfig, WorkerEvent};

/// Settings read before every stage, so changes apply to the next stage without a restart.
pub struct RunnerSettings {
    pub worker: WorkerConfig,
    pub meeting_root: PathBuf,
    pub retention: AudioRetention,
    pub voice: VoiceSettings,
}

/// Called with `(meeting id, stage, fraction 0..1, message)` for every progress event.
type ProgressFn = Box<dyn Fn(&str, Stage, f64, &str) + Send + Sync>;

pub struct PipelineRunner {
    store: Arc<Mutex<Store>>,
    settings: Box<dyn Fn() -> RunnerSettings + Send + Sync>,
    progress: ProgressFn,
}

impl PipelineRunner {
    pub fn new(
        store: Arc<Mutex<Store>>,
        settings: impl Fn() -> RunnerSettings + Send + Sync + 'static,
        progress: impl Fn(&str, Stage, f64, &str) + Send + Sync + 'static,
    ) -> Self {
        Self {
            store,
            settings: Box::new(settings),
            progress: Box::new(progress),
        }
    }
}

impl StageRunner for PipelineRunner {
    fn run(&self, meeting_id: &str, stage: Stage) -> Result<(), RunError> {
        let settings = (self.settings)();
        // Read what the stage needs, then release the store: a stage can run for an hour
        // and the UI must stay responsive meanwhile.
        let (job, details) = {
            let store = self
                .store
                .lock()
                .map_err(|_| RunError::new("store lock poisoned"))?;
            let job = store
                .get_job(meeting_id)
                .map_err(|e| RunError::new(e.to_string()))?
                .ok_or_else(|| RunError::new("the job no longer exists"))?;
            let details = store
                .get_meeting(meeting_id)
                .map_err(|e| RunError::new(e.to_string()))?
                .ok_or_else(|| RunError::new("the meeting no longer exists"))?;
            (job, details)
        };

        match worker::step_name(stage) {
            Some(step) => {
                let options = JobOptions::from_json(&job.options);
                // Environment bootstrap is idempotent and quick once the environment exists.
                worker::ensure_environment(&settings.worker)?;
                worker::run_step(
                    &settings.worker,
                    &details,
                    &settings.meeting_root,
                    &options,
                    step,
                    |ev| {
                        if let WorkerEvent::Progress {
                            fraction, message, ..
                        } = ev
                        {
                            (self.progress)(meeting_id, stage, *fraction, message);
                        }
                    },
                )
            }
            None => {
                let store = self
                    .store
                    .lock()
                    .map_err(|_| RunError::new("store lock poisoned"))?;
                match stage {
                    Stage::SpeakerId => voices::run_speaker_id(
                        &store,
                        meeting_id,
                        &settings.meeting_root.join(&details.meeting.dir),
                        &settings.voice,
                    )
                    .map(|_| ()),
                    Stage::Clips => voices::run_clips(
                        &store,
                        meeting_id,
                        &settings.meeting_root,
                        &settings.voice,
                        now_ms(),
                    )
                    .map(|_| ()),
                    _ => cleanup::apply(
                        &store,
                        &job,
                        &settings.meeting_root,
                        settings.retention,
                        now_ms(),
                    )
                    .map(|_| ())
                    .map_err(|e| RunError::new(e.to_string())),
                }
            }
        }
    }
}
