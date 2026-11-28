//! The production [`StageRunner`]: worker steps through `uv`; speaker matching, clips and the
//! audio cleanup run in Rust.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use store::{SampleRow, Store};

use crate::cleanup::{self, AudioRetention};
use crate::context::{self, ContextEnv, ContextOutcome, CycleSource};
use crate::stage::Stage;
use crate::supervisor::{now_ms, RunError, StageRunner};
use crate::voices::{self, VoiceSettings};
use crate::worker::{
    self, EnrollRequest, EnrollResponse, IngestRequest, IngestResponse, JobOptions, Launcher,
    LlmOptions, Secret, WorkerConfig, WorkerEvent,
};

/// Settings read before every stage, so changes apply to the next stage without a restart.
pub struct RunnerSettings {
    pub worker: WorkerConfig,
    pub meeting_root: PathBuf,
    /// The global policy; a project's own choice is applied over it by the runner.
    pub retention: AudioRetention,
    pub voice: VoiceSettings,
}

/// Picks the API key for the backend a stage runs with. It is called with the options the
/// stage actually uses, so the key always belongs to that job's endpoint.
type KeyResolver = Box<dyn Fn(&LlmOptions) -> Option<Secret> + Send + Sync>;

/// Called with `(meeting id, stage, fraction 0..1, message)` for every progress event.
type ProgressFn = Box<dyn Fn(&str, Stage, f64, &str) + Send + Sync>;

pub struct PipelineRunner {
    store: Arc<Mutex<Store>>,
    settings: Box<dyn Fn() -> RunnerSettings + Send + Sync>,
    progress: ProgressFn,
    /// Where the cycles of the meetings come from, for the context before `extract`.
    cycles: Option<Arc<dyn CycleSource>>,
    /// Chooses the API key from the options of the job; without it the key in
    /// [`RunnerSettings::worker`] is used as it is.
    key_resolver: Option<KeyResolver>,
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
            key_resolver: None,
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

    /// Chooses the API key of `extract` and `reduce` from the options of the job that runs
    /// (its backend and endpoint), instead of using a key fixed in the settings.
    pub fn with_key_resolver(
        mut self,
        resolver: impl Fn(&LlmOptions) -> Option<Secret> + Send + Sync + 'static,
    ) -> Self {
        self.key_resolver = Some(Box::new(resolver));
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

    /// The cycle of the meeting, read without holding the store while the repository is
    /// consulted. `None` when the meeting has no cycle and so nothing is to be extracted.
    fn load_cycle_for(&self, meeting_id: &str) -> Result<Option<context::CycleData>, RunError> {
        let cycle_id = {
            let store = self
                .store
                .lock()
                .map_err(|_| RunError::new("store lock poisoned"))?;
            context::cycle_id_of(&store, meeting_id)?
        };
        let Some(cycle_id) = cycle_id else {
            return Ok(None);
        };
        let source = self.cycles.as_deref().ok_or_else(|| {
            RunError::new("no project source is configured, so the context cannot be built")
        })?;
        context::load_cycle(source, &cycle_id).map(Some)
    }

    /// The step before `extract`: context and named transcript, from the cycle loaded by
    /// [`Self::load_cycle_for`]. Returns the progress message to report once the caller has
    /// released the store: the progress callback may lock the store itself (the application
    /// reads the queue state from it), and the lock is not reentrant.
    fn prepare_context(
        &self,
        store: &Store,
        data: &context::CycleData,
        meeting_id: &str,
        meeting_root: &std::path::Path,
    ) -> Result<Option<String>, RunError> {
        let env = ContextEnv {
            meeting_root,
            utc_offset_minutes: self.utc_offset_minutes,
        };
        if let ContextOutcome::Written {
            warnings,
            local_drafts,
            ..
        } = context::prepare_context_with(store, data, &env, meeting_id)?
        {
            for w in &warnings {
                tracing::warn!(meeting = meeting_id, "context: {w}");
            }
            return Ok(Some(format!(
                "Context ready ({local_drafts} earlier drafts)"
            )));
        }
        Ok(None)
    }
}

impl StageRunner for PipelineRunner {
    fn run(&self, meeting_id: &str, stage: Stage) -> Result<(), RunError> {
        let settings = (self.settings)();
        // Read what the stage needs, then release the store: a stage can run for an hour
        // and the UI must stay responsive meanwhile.
        // The cycle comes from the repository (files, Git): fetch it before the store is
        // locked for anything else.
        let cycle = if stage == Stage::Extract {
            self.load_cycle_for(meeting_id)?
        } else {
            None
        };
        let (job, details, choice, context_ready) = {
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
            let mut context_ready = None;
            if stage == Stage::Extract {
                let Some(data) = &cycle else {
                    return Ok(());
                };
                context_ready =
                    self.prepare_context(&store, data, meeting_id, &settings.meeting_root)?;
            }
            (job, details, choice, context_ready)
        };
        // Outside the store lock: reporting reads the queue state from the store.
        if let Some(message) = context_ready {
            tracing::info!(stage = stage.as_str(), "context ready");
            (self.progress)(meeting_id, Stage::Extract, 0.0, &message);
        }

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
                tracing::info!(stage = stage.as_str(), step, "starting the worker");
                let mut cfg = settings.worker.clone();
                if let Some(resolve) = &self.key_resolver {
                    cfg.llm_api_key = options.llm.as_ref().and_then(resolve);
                }
                worker::run_step_with(
                    self.launcher.as_ref(),
                    &cfg,
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
                )?;
                tracing::info!(stage = stage.as_str(), step, "the worker finished");
                if stage == Stage::Extract {
                    ensure_something_was_extracted(
                        &settings.meeting_root.join(&details.meeting.dir),
                    )?;
                }
                Ok(())
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
                    _ => {
                        // The project's own choice beats the global setting. A stored
                        // choice that cannot be read fails the stage: the audio stays.
                        let retention =
                            cleanup::effective_retention(&store, meeting_id, settings.retention)
                                .map_err(|e| RunError::new(e.to_string()))?;
                        cleanup::apply(&store, &job, &settings.meeting_root, retention)
                            .map(|_| ())
                            .map_err(|e| RunError::new(e.to_string()))
                    }
                }
            }
        }
    }
}

/// A finished `extract` in which every window was skipped (no valid answer from the model)
/// left no items: the draft would be empty but look like a meeting without items. The stage
/// fails instead, so the user can retry it; `windows.json` stays for inspection.
fn ensure_something_was_extracted(meeting_dir: &std::path::Path) -> Result<(), RunError> {
    let path = meeting_dir
        .join("work")
        .join("extract")
        .join("windows.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(());
    };
    let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(());
    };
    let Some(windows) = doc.get("windows").and_then(|w| w.as_array()) else {
        return Ok(());
    };
    let skipped = windows
        .iter()
        .filter(|w| w.get("skipped").and_then(|s| s.as_bool()) == Some(true))
        .count();
    if !windows.is_empty() && skipped == windows.len() {
        return Err(RunError::new(format!(
            "the model gave no usable answer for any of the {} windows (rate limit, outage or              invalid output); retry the stage",
            windows.len()
        )));
    }
    Ok(())
}

/// Run the worker's `"ingest"` command for an imported media file and persist the resulting
/// tracks, segments, screenshots and duration in `store`.
pub fn ingest_meeting(
    store: &Store,
    cfg: &WorkerConfig,
    launcher: Option<&Launcher>,
    req: &IngestRequest,
    on_event: impl FnMut(&WorkerEvent),
) -> Result<IngestResponse, RunError> {
    if launcher.is_none() {
        worker::ensure_environment(cfg)?;
    }
    let resp = worker::run_ingest_with(launcher, cfg, req, on_event)?;
    worker::record_ingest(store, &req.meeting_id, &resp)
        .map_err(|e| RunError::new(e.to_string()))?;
    Ok(resp)
}

/// Run the worker's `"enroll"` command on a 15-30 s user-supplied sample file, save the preview
/// clip under `<meeting_root>/.voice-samples/<participant_id>.wav`, and store the 256-dim
/// voice sample and `sample_clip_file` on `participant_id` in `store`.
pub fn enroll_participant_voice(
    store: &Store,
    cfg: &WorkerConfig,
    launcher: Option<&Launcher>,
    participant_id: &str,
    input_path: &Path,
    meeting_root: &Path,
    on_event: impl FnMut(&WorkerEvent),
) -> Result<EnrollResponse, RunError> {
    if launcher.is_none() {
        worker::ensure_environment(cfg)?;
    }
    let rel_clip = voices::participant_sample_clip_rel(participant_id);
    let clip_out_path = meeting_root.join(&rel_clip);
    let output_path = meeting_root
        .join(voices::VOICE_SAMPLES_DIR)
        .join(format!("{}.enroll.json", participant_id.replace(':', "_")));
    let req = EnrollRequest {
        input_path: input_path.to_path_buf(),
        clip_out_path,
        output_path: output_path.clone(),
    };
    let resp = worker::run_enroll_with(launcher, cfg, &req, on_event)?;
    let _ = std::fs::remove_file(&output_path);
    let _ = std::fs::remove_file(
        meeting_root
            .join(voices::VOICE_SAMPLES_DIR)
            .join("enroll-manifest.json"),
    );

    store
        .put_sample(
            participant_id,
            &SampleRow {
                source_key: "manual_enroll".into(),
                embedding: resp.embedding.clone(),
                speech_ms: resp.duration_ms.max(10_000),
            },
            now_ms(),
        )
        .map_err(|e| RunError::new(e.to_string()))?;
    store
        .set_participant_sample_clip(participant_id, Some(&rel_clip))
        .map_err(|e| RunError::new(e.to_string()))?;
    Ok(resp)
}
