//! The production [`StageRunner`]: worker steps through `uv`, audio cleanup in Rust.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use store::Store;

use crate::cleanup::{self, AudioRetention};
use crate::context::{self, ContextEnv, ContextOutcome, CycleSource};
use crate::stage::Stage;
use crate::supervisor::{now_ms, RunError, StageRunner};
use crate::voices::{self, VoiceSettings};
use crate::worker::{self, JobOptions, Launcher, WorkerConfig, WorkerEvent};

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
    /// Where the cycles of the meetings come from, for the context before `extract`.
    cycles: Option<Arc<dyn CycleSource>>,
    /// Replaces `uv run ... python -m bbb_worker`; for tests with a fake worker.
    launcher: Option<Launcher>,
    utc_offset_minutes: i32,
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
            cycles: None,
            launcher: None,
            utc_offset_minutes: 0,
        }
    }

    /// Supplies the cycles of meetings. Without it a meeting that has a cycle cannot get
    /// its context and `extract` fails with a clear message.
    pub fn with_cycle_source(mut self, source: Arc<dyn CycleSource>) -> Self {
        self.cycles = Some(source);
        self
    }

    /// Runs `launcher` in place of the Python worker and skips the environment bootstrap.
    /// For tests that fake the worker with a small script.
    pub fn with_launcher(mut self, launcher: Launcher) -> Self {
        self.launcher = Some(launcher);
        self
    }

    /// Offset of the user's time zone from UTC in minutes, to date meetings by the local
    /// calendar day (default 0).
    pub fn with_utc_offset_minutes(mut self, minutes: i32) -> Self {
        self.utc_offset_minutes = minutes;
        self
    }

    /// The step before `extract`: context and named transcript. Returns `false` when the
    /// meeting has no cycle and so nothing is to be extracted.
    fn prepare_context(
        &self,
        store: &Store,
        meeting_id: &str,
        meeting_root: &std::path::Path,
    ) -> Result<bool, RunError> {
        let source = self.cycles.as_deref().ok_or_else(|| {
            RunError::new("no cycle source is configured, so the context cannot be built")
        })?;
        let env = ContextEnv {
            meeting_root,
            utc_offset_minutes: self.utc_offset_minutes,
        };
        match context::prepare_context(store, source, &env, meeting_id)? {
            ContextOutcome::NoCycle => Ok(false),
            ContextOutcome::Written {
                warnings,
                local_drafts,
                ..
            } => {
                for w in &warnings {
                    tracing::warn!(meeting = meeting_id, "context: {w}");
                }
                (self.progress)(
                    meeting_id,
                    Stage::Extract,
                    0.0,
                    &format!("Context ready ({local_drafts} earlier drafts)"),
                );
                Ok(true)
            }
        }
    }
}

impl StageRunner for PipelineRunner {
    fn run(&self, meeting_id: &str, stage: Stage) -> Result<(), RunError> {
        let settings = (self.settings)();
        // Read what the stage needs, then release the store: a stage can run for an hour
        // and the UI must stay responsive meanwhile.
        let (job, details, choice) = {
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
            let choice = store
                .get_llm_choice(meeting_id)
                .map_err(|e| RunError::new(e.to_string()))?;
            // The context is built by the application, not the worker, right before the
            // first language-model stage.
            if stage == Stage::Extract
                && !self.prepare_context(&store, meeting_id, &settings.meeting_root)?
            {
                return Ok(());
            }
            (job, details, choice)
        };

        match worker::step_name(stage) {
            Some(step) => {
                let mut options = JobOptions::from_json(&job.options);
                if stage.uses_llm() {
                    if options.llm.is_none() {
                        return Ok(());
                    }
                    // Whether the cloud may be used is the store's word, not the job's.
                    options = worker::with_store_confirmation(&options, choice.as_ref());
                }
                if self.launcher.is_none() {
                    // Environment bootstrap is idempotent and quick once the environment
                    // exists.
                    worker::ensure_environment(&settings.worker)?;
                }
                worker::run_step_with(
                    self.launcher.as_ref(),
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
