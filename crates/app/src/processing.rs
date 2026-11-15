//! Processing service: settings, the queue supervisor with its background driver, and the
//! views the UI shows (queue, transcript).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use queue::cleanup::AudioRetention;
use queue::gpu::{GpuSampler, Guard, GuardPolicy, NvmlSampler, OnUnavailable};
use queue::runner::{PipelineRunner, RunnerSettings};
use queue::voices::VoiceSettings;
use queue::worker::{self, JobOptions, WorkerConfig, WorkerEvent};
use queue::{GateDecision, GpuGate, Next, Stage, StepOutcome, Supervisor, STAGES};
use serde::{Deserialize, Serialize};
use speakers::Thresholds;
use store::{StageStatus, Store};
use tauri::{AppHandle, Emitter};

use crate::ipc::{IpcError, IpcResult};
use crate::secrets;

pub const EVENT_QUEUE: &str = "queue-changed";
pub const EVENT_MODELS_PROGRESS: &str = "models-progress";
pub const EVENT_MODELS_CHANGED: &str = "models-changed";

/// A stage that crashed this many times in a row is marked failed instead of retried.
const MAX_CRASHES: u32 = 3;
/// Pause between attempts while a job waits for the GPU.
const WAIT_POLL: Duration = Duration::from_secs(5);
/// Minimum time between two queue events caused by progress lines.
const EVENT_THROTTLE: Duration = Duration::from_millis(250);

// ----- settings ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessAfter {
    Ask,
    Now,
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RetentionMode {
    Keep,
    Delete,
    KeepDays,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RetentionSetting {
    pub mode: RetentionMode,
    pub days: u32,
}

impl Default for RetentionSetting {
    fn default() -> Self {
        // Deleting the audio once processing succeeded is the default; the user chose this.
        Self {
            mode: RetentionMode::Delete,
            days: 7,
        }
    }
}

impl From<RetentionSetting> for AudioRetention {
    fn from(r: RetentionSetting) -> Self {
        match r.mode {
            RetentionMode::Keep => Self::Keep,
            RetentionMode::Delete => Self::Delete,
            RetentionMode::KeepDays => Self::KeepDays { days: r.days },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProcessingSettings {
    pub process_after_recording: ProcessAfter,
    pub audio_retention: RetentionSetting,
    pub gpu_idle_seconds: u64,
    pub gpu_max_util_percent: u32,
    pub uv_path: String,
    pub worker_dir: String,
    pub llama_server_path: String,
    pub models_dir: String,
    /// Voice matching: a match at or above this similarity names a voice automatically.
    pub voice_accept: f32,
    /// A match at or above this similarity is offered as a suggestion.
    pub voice_suggest: f32,
    /// An automatic match must beat the runner-up by this much.
    pub voice_margin: f32,
    /// Sample clips of voices nobody confirmed are deleted after this many days.
    pub clip_max_days: u32,
    /// Filled when reading; ignored when saving.
    #[serde(skip_deserializing)]
    pub hf_token_set: bool,
}

impl Default for ProcessingSettings {
    fn default() -> Self {
        let policy = GuardPolicy::default();
        let voice = VoiceSettings::default();
        Self {
            process_after_recording: ProcessAfter::Ask,
            audio_retention: RetentionSetting::default(),
            gpu_idle_seconds: policy.idle_ms / 1000,
            gpu_max_util_percent: policy.max_util_percent,
            uv_path: String::new(),
            worker_dir: String::new(),
            llama_server_path: String::new(),
            models_dir: String::new(),
            voice_accept: voice.thresholds.accept,
            voice_suggest: voice.thresholds.suggest,
            voice_margin: voice.thresholds.margin,
            clip_max_days: voice.clip_max_days,
            hf_token_set: false,
        }
    }
}

impl ProcessingSettings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    pub fn voice_settings(&self) -> VoiceSettings {
        VoiceSettings {
            thresholds: Thresholds {
                accept: self.voice_accept,
                suggest: self.voice_suggest,
                margin: self.voice_margin,
            },
            clip_max_days: self.clip_max_days,
            ..VoiceSettings::default()
        }
    }

    fn guard_policy(&self) -> GuardPolicy {
        GuardPolicy {
            idle_ms: self.gpu_idle_seconds * 1000,
            max_util_percent: self.gpu_max_util_percent,
            on_unavailable: OnUnavailable::Block,
            ..GuardPolicy::default()
        }
    }
}

fn non_empty(s: &str) -> Option<PathBuf> {
    let t = s.trim();
    (!t.is_empty()).then(|| PathBuf::from(t))
}

/// The worker folder: the setting, `BBB_WORKER_DIR`, `worker/` next to the executable, or
/// `worker/` in a parent folder of the executable (a development build).
fn find_worker_dir(configured: &str) -> PathBuf {
    if let Some(p) = non_empty(configured) {
        return p;
    }
    if let Some(p) = std::env::var_os("BBB_WORKER_DIR") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors().skip(1) {
            let candidate = dir.join("worker");
            if candidate.join("pyproject.toml").exists() {
                return candidate;
            }
        }
    }
    PathBuf::from("worker")
}

// ----- GPU gate with live settings -----------------------------------------------------------

/// The GPU gate, rebuilt when the thresholds in the settings change.
struct SettingsGate {
    settings: Arc<Mutex<ProcessingSettings>>,
    inner: Mutex<(NvmlSampler, Guard)>,
}

impl GpuGate for SettingsGate {
    fn check(&self, stage: Stage) -> GateDecision {
        let policy = match self.settings.lock() {
            Ok(s) => s.guard_policy(),
            Err(_) => return GateDecision::Wait("settings are unavailable".into()),
        };
        let Ok(mut inner) = self.inner.lock() else {
            return GateDecision::Wait("internal error in the GPU guard".into());
        };
        if *inner.1.policy() != policy {
            inner.1 = Guard::new(policy);
        }
        let sample = inner.0.sample();
        inner.1.evaluate(sample.as_ref(), stage)
    }
}

// ----- views ----------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageView {
    pub stage: Stage,
    pub status: StageStatus,
    pub attempts: u32,
    pub error: Option<String>,
    pub fraction: Option<f64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Waiting,
    Running,
    Failed,
    Done,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub meeting_id: String,
    pub title: String,
    pub state: JobState,
    pub progress: f32,
    pub waiting_reason: Option<String>,
    pub stages: Vec<StageView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueState {
    pub jobs: Vec<JobView>,
}

#[derive(Debug, Clone)]
struct Live {
    stage: Stage,
    fraction: f64,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub key: String,
    pub label: String,
    pub cached: bool,
    pub gated: bool,
    pub size_mb: Option<u64>,
}

const MODEL_INFO: [(&str, &str, bool); 4] = [
    (
        "whisper-turbo",
        "Whisper large-v3-turbo (speech recognition)",
        false,
    ),
    (
        "whisper-large-v3",
        "Whisper large-v3 (speech recognition, optional)",
        false,
    ),
    ("diarization", "pyannote community-1 (speakers)", true),
    ("vision", "Qwen3-VL 8B (screenshot description)", false),
];

// ----- service --------------------------------------------------------------------------------

pub struct Processing {
    app: AppHandle,
    store: Arc<Mutex<Store>>,
    supervisor: Arc<Supervisor>,
    settings: Arc<Mutex<ProcessingSettings>>,
    settings_path: PathBuf,
    env_dir: PathBuf,
    live: Arc<Mutex<HashMap<String, Live>>>,
    last_event: Mutex<Instant>,
    driver_running: Arc<AtomicBool>,
    models_busy: Arc<AtomicBool>,
}

impl Processing {
    /// `meeting_root` is asked before every stage, so a changed recordings folder applies.
    pub fn new(
        app: AppHandle,
        store: Arc<Mutex<Store>>,
        data_dir: &Path,
        config_dir: &Path,
        meeting_root: impl Fn() -> PathBuf + Send + Sync + 'static,
    ) -> Arc<Self> {
        let settings_path = config_dir.join("processing.json");
        let settings = Arc::new(Mutex::new(ProcessingSettings::load(&settings_path)));
        let env_dir = data_dir.join("worker-venv");
        let live: Arc<Mutex<HashMap<String, Live>>> = Arc::default();

        let runner_settings = {
            let settings = settings.clone();
            let env_dir = env_dir.clone();
            move || {
                let s = settings.lock().map(|s| s.clone()).unwrap_or_default();
                RunnerSettings {
                    worker: worker_config(&s, &env_dir),
                    meeting_root: meeting_root(),
                    retention: s.audio_retention.into(),
                    voice: s.voice_settings(),
                }
            }
        };

        // The runner reports progress through the service, which exists only afterwards.
        let progress_target: Arc<Mutex<Option<std::sync::Weak<Processing>>>> = Arc::default();
        let runner = PipelineRunner::new(store.clone(), runner_settings, {
            let target = progress_target.clone();
            let live = live.clone();
            move |meeting_id, stage, fraction, message| {
                if let Ok(mut l) = live.lock() {
                    l.insert(
                        meeting_id.to_string(),
                        Live {
                            stage,
                            fraction,
                            message: message.to_string(),
                        },
                    );
                }
                if let Some(p) = target.lock().ok().and_then(|t| t.as_ref()?.upgrade()) {
                    p.emit_queue_throttled();
                }
            }
        });

        let gate = SettingsGate {
            settings: settings.clone(),
            inner: Mutex::new((
                NvmlSampler::new(),
                Guard::new(
                    settings
                        .lock()
                        .map(|s| s.guard_policy())
                        .unwrap_or_default(),
                ),
            )),
        };
        let supervisor = Arc::new(Supervisor::new(
            store.clone(),
            Arc::new(runner),
            Arc::new(gate),
            MAX_CRASHES,
        ));

        let this = Arc::new(Self {
            app,
            store,
            supervisor,
            settings,
            settings_path,
            env_dir,
            live,
            last_event: Mutex::new(Instant::now() - EVENT_THROTTLE),
            driver_running: Arc::default(),
            models_busy: Arc::default(),
        });
        if let Ok(mut t) = progress_target.lock() {
            *t = Some(Arc::downgrade(&this));
        }
        {
            let weak = Arc::downgrade(&this);
            this.supervisor.set_listener(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.emit_queue();
                }
            });
        }
        this
    }

    // ----- settings

    pub fn settings(&self) -> IpcResult<ProcessingSettings> {
        let mut s = self
            .settings
            .lock()
            .map_err(|_| IpcError::new("internal", "settings lock poisoned"))?
            .clone();
        s.hf_token_set = secrets::get(secrets::HF_TOKEN).is_some();
        Ok(s)
    }

    pub fn save_settings(&self, new: ProcessingSettings) -> IpcResult<()> {
        if new.gpu_max_util_percent == 0 || new.gpu_max_util_percent > 100 {
            return Err(IpcError::new(
                "invalid",
                "the GPU utilization limit must be between 1 and 100",
            ));
        }
        let thresholds = new.voice_settings().thresholds;
        if !thresholds.is_valid() {
            return Err(IpcError::new(
                "invalid",
                "voice thresholds must satisfy 0 <= suggest <= accept <= 1 and margin >= 0",
            ));
        }
        new.save(&self.settings_path)?;
        *self
            .settings
            .lock()
            .map_err(|_| IpcError::new("internal", "settings lock poisoned"))? = new;
        Ok(())
    }

    /// Delete the audio whose "keep for N days" period has passed. Run at start.
    pub fn sweep_expired_audio(&self, meeting_root: &Path) {
        let retention: AudioRetention = self
            .settings
            .lock()
            .map(|s| s.audio_retention.into())
            .unwrap_or_default();
        let Ok(store) = self.store.lock() else {
            return;
        };
        match queue::cleanup::sweep_expired(&store, meeting_root, retention, queue::now_ms()) {
            Ok(0) => {}
            Ok(files) => tracing::info!(files, "deleted audio past its retention period"),
            Err(e) => tracing::warn!(error = %e, "audio sweep failed"),
        }
    }

    /// Delete sample clips of confirmed meetings and of meetings past the clip limit.
    pub fn sweep_clips(&self, meeting_root: &Path) {
        let days = self.settings.lock().map(|s| s.clip_max_days).unwrap_or(30);
        let Ok(store) = self.store.lock() else {
            return;
        };
        match queue::voices::sweep_clips(&store, meeting_root, days, queue::now_ms()) {
            Ok(0) => {}
            Ok(n) => tracing::info!(meetings = n, "deleted voice sample clips"),
            Err(e) => tracing::warn!(error = %e, "clip sweep failed"),
        }
    }

    fn worker_config(&self) -> WorkerConfig {
        let s = self.settings.lock().map(|s| s.clone()).unwrap_or_default();
        worker_config(&s, &self.env_dir)
    }

    // ----- queue

    /// Resume jobs interrupted by a crash or exit. Call once at start.
    pub fn resume_interrupted(self: &Arc<Self>) {
        match self.supervisor.recover() {
            Ok(v) if !v.is_empty() => {
                tracing::warn!(count = v.len(), "resuming interrupted processing");
                self.kick();
            }
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "could not recover the queue"),
        }
    }

    pub fn process(self: &Arc<Self>, meeting_id: &str, options: JobOptions) -> IpcResult<()> {
        let state = {
            let store = self.store.lock().map_err(poisoned)?;
            store
                .get_meeting(meeting_id)?
                .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?
                .meeting
                .state
        };
        if !matches!(
            state,
            store::MeetingState::Stopped | store::MeetingState::Recovered
        ) {
            return Err(IpcError::new(
                "invalid_state",
                "only a finished recording can be processed",
            ));
        }
        let options = serde_json::to_string(&options).unwrap_or_else(|_| "{}".into());
        self.supervisor
            .enqueue(meeting_id, &options)
            .map_err(|e| IpcError::new("internal", e.to_string()))?;
        self.emit_queue();
        self.kick();
        Ok(())
    }

    pub fn retry(self: &Arc<Self>, meeting_id: &str) -> IpcResult<()> {
        self.supervisor
            .retry(meeting_id)
            .map_err(|e| IpcError::new("not_found", e.to_string()))?;
        self.emit_queue();
        self.kick();
        Ok(())
    }

    pub fn cancel(&self, meeting_id: &str) -> IpcResult<()> {
        let store = self.store.lock().map_err(poisoned)?;
        let Some(job) = store.get_job(meeting_id)? else {
            return Ok(());
        };
        if job.stages.iter().any(|s| s.status == StageStatus::Running) {
            return Err(IpcError::new(
                "busy",
                "a stage is running; wait for it to finish",
            ));
        }
        store.delete_job(meeting_id)?;
        drop(store);
        self.emit_queue();
        Ok(())
    }

    /// Start the background driver unless it is already running. It works through every
    /// runnable job, waits while the GPU is busy and stops when nothing is left to do.
    fn kick(self: &Arc<Self>) {
        if self.driver_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        let spawned = std::thread::Builder::new()
            .name("queue-driver".into())
            .spawn(move || {
                this.drive();
                this.driver_running.store(false, Ordering::SeqCst);
                // A job added while the driver was finishing must not be left behind.
                if this.has_runnable() {
                    this.kick();
                }
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not start the queue driver");
            self.driver_running.store(false, Ordering::SeqCst);
        }
    }

    fn runnable_ids(&self) -> Vec<String> {
        let Ok(store) = self.store.lock() else {
            return Vec::new();
        };
        store
            .list_job_ids()
            .unwrap_or_default()
            .into_iter()
            .filter(|id| {
                store
                    .get_job(id)
                    .ok()
                    .flatten()
                    .is_some_and(|j| matches!(queue::next_step(&j), Next::Run { .. }))
            })
            .collect()
    }

    fn has_runnable(&self) -> bool {
        !self.runnable_ids().is_empty()
    }

    fn drive(&self) {
        loop {
            let ids = self.runnable_ids();
            if ids.is_empty() {
                break;
            }
            let mut progressed = false;
            for id in ids {
                match self.supervisor.step(&id) {
                    Ok(StepOutcome::Ran { .. }) => progressed = true,
                    Ok(StepOutcome::Failed { stage, error }) => {
                        tracing::error!(meeting = %id, stage = stage.as_str(), %error, "stage failed");
                        progressed = true;
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(error = %e, "queue step failed"),
                }
                if let Ok(mut l) = self.live.lock() {
                    l.remove(&id);
                }
            }
            self.emit_queue();
            if !progressed {
                std::thread::sleep(WAIT_POLL);
            }
        }
        self.emit_queue();
    }

    // ----- views

    pub fn queue_state(&self) -> IpcResult<QueueState> {
        let store = self.store.lock().map_err(poisoned)?;
        let live = self.live.lock().map_err(poisoned)?.clone();
        let mut jobs = Vec::new();
        for id in store.list_job_ids()? {
            let Some(job) = store.get_job(&id)? else {
                continue;
            };
            let title = store
                .get_meeting(&id)?
                .map(|d| d.meeting.title)
                .unwrap_or_else(|| id.clone());
            let live_now = live.get(&id);
            let waiting_reason = self.supervisor.waiting_reason(&id);
            let state = match queue::next_step(&job) {
                Next::Finished => JobState::Done,
                Next::Failed { .. } => JobState::Failed,
                Next::Run { stage } => {
                    let running = job
                        .stages
                        .iter()
                        .any(|s| s.stage == stage.as_str() && s.status == StageStatus::Running);
                    if running {
                        JobState::Running
                    } else if waiting_reason.is_some() {
                        JobState::Waiting
                    } else {
                        JobState::Queued
                    }
                }
            };
            let stages: Vec<StageView> = STAGES
                .iter()
                .map(|st| {
                    let row = job.stages.iter().find(|s| s.stage == st.as_str());
                    let l = live_now.filter(|l| l.stage == *st);
                    StageView {
                        stage: *st,
                        status: row.map_or(StageStatus::Pending, |r| r.status),
                        attempts: row.map_or(0, |r| r.attempts),
                        error: row.and_then(|r| r.error.clone()),
                        fraction: l.map(|l| l.fraction),
                        message: l.map(|l| l.message.clone()),
                    }
                })
                .collect();
            let mut progress = queue::progress(&job);
            if let Some(l) = live_now {
                progress += (l.fraction as f32) / STAGES.len() as f32;
            }
            jobs.push(JobView {
                meeting_id: id,
                title,
                state,
                progress: progress.min(1.0),
                waiting_reason: if state == JobState::Waiting {
                    waiting_reason
                } else {
                    None
                },
                stages,
            });
        }
        Ok(QueueState { jobs })
    }

    /// Job state of one meeting, for the meeting list.
    pub fn job_states(&self) -> HashMap<String, JobState> {
        self.queue_state()
            .map(|q| {
                q.jobs
                    .into_iter()
                    .map(|j| (j.meeting_id, j.state))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn emit_queue(&self) {
        if let Ok(state) = self.queue_state() {
            if let Err(e) = self.app.emit(EVENT_QUEUE, &state) {
                tracing::warn!(error = %e, "could not emit the queue state");
            }
        }
    }

    fn emit_queue_throttled(&self) {
        let Ok(mut last) = self.last_event.lock() else {
            return;
        };
        if last.elapsed() >= EVENT_THROTTLE {
            *last = Instant::now();
            drop(last);
            self.emit_queue();
        }
    }

    // ----- models

    pub fn model_status(&self) -> IpcResult<Vec<ModelStatus>> {
        let cached: HashMap<String, bool> = match worker::model_status(&self.worker_config()) {
            Ok(v) => v.into_iter().collect(),
            // Without an environment nothing can be checked yet: report not cached.
            Err(_) => HashMap::new(),
        };
        Ok(MODEL_INFO
            .iter()
            .map(|(key, label, gated)| ModelStatus {
                key: (*key).into(),
                label: (*label).into(),
                cached: cached.get(*key).copied().unwrap_or(false),
                gated: *gated,
                size_mb: None,
            })
            .collect())
    }

    pub fn download_models(self: &Arc<Self>, keys: Vec<String>) -> IpcResult<()> {
        if keys
            .iter()
            .any(|k| !MODEL_INFO.iter().any(|(m, _, _)| m == k))
        {
            return Err(IpcError::new("invalid", "unknown model"));
        }
        if self.models_busy.swap(true, Ordering::SeqCst) {
            return Err(IpcError::new("busy", "a download is already running"));
        }
        let this = self.clone();
        let spawned = std::thread::Builder::new()
            .name("model-download".into())
            .spawn(move || {
                let mut cfg = this.worker_config();
                cfg.hf_token = secrets::get(secrets::HF_TOKEN);
                let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
                let result = worker::ensure_environment(&cfg).and_then(|()| {
                    worker::ensure_models(&cfg, &refs, |ev| {
                        if let WorkerEvent::Progress {
                            fraction, message, ..
                        } = ev
                        {
                            let _ = this.app.emit(
                                EVENT_MODELS_PROGRESS,
                                serde_json::json!({
                                    "key": refs.first().copied().unwrap_or(""),
                                    "fraction": fraction,
                                    "message": message,
                                }),
                            );
                        }
                    })
                });
                if let Err(e) = result {
                    tracing::error!(error = %e.message, "model download failed");
                    let _ = this.app.emit(
                        EVENT_MODELS_PROGRESS,
                        serde_json::json!({
                            "key": refs.first().copied().unwrap_or(""),
                            "fraction": 0.0,
                            "message": format!("Failed: {}", e.message),
                        }),
                    );
                }
                this.models_busy.store(false, Ordering::SeqCst);
                let _ = this.app.emit(EVENT_MODELS_CHANGED, ());
            });
        if spawned.is_err() {
            self.models_busy.store(false, Ordering::SeqCst);
            return Err(IpcError::new("internal", "could not start the download"));
        }
        Ok(())
    }
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> IpcError {
    IpcError::new("internal", "lock poisoned")
}

fn worker_config(s: &ProcessingSettings, env_dir: &Path) -> WorkerConfig {
    WorkerConfig {
        uv: non_empty(&s.uv_path).unwrap_or_else(|| PathBuf::from("uv")),
        worker_dir: find_worker_dir(&s.worker_dir),
        env_dir: env_dir.to_path_buf(),
        models_dir: non_empty(&s.models_dir),
        llama_server: non_empty(&s.llama_server_path),
        hf_token: secrets::get(secrets::HF_TOKEN),
        // The reference estimate for a 10-hour meeting is below 4 hours; stop after 6.
        step_timeout: Duration::from_secs(6 * 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_delete_the_audio_after_processing_and_ask_after_recording() {
        let s = ProcessingSettings::default();
        assert_eq!(s.process_after_recording, ProcessAfter::Ask);
        assert_eq!(s.audio_retention.mode, RetentionMode::Delete);
        assert_eq!(
            AudioRetention::from(s.audio_retention),
            AudioRetention::Delete
        );
    }

    #[test]
    fn settings_use_the_camel_case_contract() {
        let json = serde_json::to_value(ProcessingSettings::default()).unwrap();
        assert_eq!(json["processAfterRecording"], "ask");
        assert_eq!(json["audioRetention"]["mode"], "delete");
        assert!(json.get("gpuIdleSeconds").is_some());
        let parsed: ProcessingSettings = serde_json::from_str(
            r#"{"audioRetention":{"mode":"keepDays","days":3},"hfTokenSet":true}"#,
        )
        .unwrap();
        assert_eq!(
            AudioRetention::from(parsed.audio_retention),
            AudioRetention::KeepDays { days: 3 }
        );
        // The stored flag is never read back from a saved file.
        assert!(!parsed.hf_token_set);
    }

    #[test]
    fn guard_policy_follows_the_settings() {
        let s = ProcessingSettings {
            gpu_idle_seconds: 3,
            gpu_max_util_percent: 60,
            ..ProcessingSettings::default()
        };
        let p = s.guard_policy();
        assert_eq!(p.idle_ms, 3000);
        assert_eq!(p.max_util_percent, 60);
    }

    #[test]
    fn explicit_worker_dir_wins() {
        assert_eq!(find_worker_dir("D:/w"), PathBuf::from("D:/w"));
    }
}
