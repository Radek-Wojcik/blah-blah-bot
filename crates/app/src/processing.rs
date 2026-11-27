//! Processing service: settings, the queue supervisor with its background driver, and the
//! views the UI shows (queue, transcript).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use queue::cleanup::AudioRetention;
use queue::gpu::{GpuSampler, Guard, GuardPolicy, NvmlSampler, OnUnavailable};
use queue::runner::{PipelineRunner, RunnerSettings};
use queue::voices::VoiceSettings;
use queue::worker::{self, JobOptions, WorkerConfig, WorkerEvent};
use queue::{GateDecision, GpuGate, Next, Stage, StepOutcome, Supervisor, STAGES};
use serde::{Deserialize, Serialize};
use speakers::Thresholds;
use store::{LlmKind, StageStatus, Store};
use tauri::{AppHandle, Emitter, Manager};

use crate::cycles::{PullLedger, RepoCycleSource};
use crate::ipc::{IpcError, IpcResult};
use crate::llm::{self, LlmSettings};
use crate::secrets;
use crate::settings::CycleRepoSetting;
use crate::setup;

type RepoList = Arc<dyn Fn() -> Vec<CycleRepoSetting> + Send + Sync>;

pub const EVENT_QUEUE: &str = "queue-changed";
pub const EVENT_MODELS_PROGRESS: &str = "models-progress";
pub const EVENT_MODELS_CHANGED: &str = "models-changed";
/// Progress of the processing environment set-up (`prepare_worker_env`).
pub const EVENT_SETUP_PROGRESS: &str = "setup-progress";

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

/// What happens to the source audio once a meeting was processed successfully.
///
/// `KeepDays` is what older versions stored for "keep for N days". That option no longer
/// exists; the value is still read and counts as `Keep`, so audio that someone asked to keep
/// is never deleted by an upgrade. The next save writes `keep` or `delete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RetentionMode {
    Keep,
    Delete,
    KeepDays,
}

/// The global "keep audio after processing" setting. Stored as `audioRetention.mode`; the
/// `days` field older versions wrote next to it is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RetentionSetting {
    pub mode: RetentionMode,
}

impl Default for RetentionSetting {
    fn default() -> Self {
        // Deleting the audio once processing succeeded is the default; the owner chose this.
        Self {
            mode: RetentionMode::Delete,
        }
    }
}

impl From<RetentionSetting> for AudioRetention {
    fn from(r: RetentionSetting) -> Self {
        match r.mode {
            RetentionMode::Delete => Self::Delete,
            RetentionMode::Keep | RetentionMode::KeepDays => Self::Keep,
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
    /// Optional path to a local vision GGUF file or folder.
    pub vision_model_path: String,
    /// Skip screenshot vision analysis during processing.
    pub skip_screenshot_analysis: bool,
    /// Voice matching: a match at or above this similarity names a voice automatically.
    pub voice_accept: f32,
    /// A match at or above this similarity is offered as a suggestion.
    pub voice_suggest: f32,
    /// An automatic match must beat the runner-up by this much.
    pub voice_margin: f32,
    /// Sample clips of voices nobody confirmed are deleted after this many days.
    pub clip_max_days: u32,
    /// Language-model backend of the extraction stages.
    pub llm: LlmSettings,
    /// Describe the screenshots of a meeting with the vision model. Off: the stage is skipped
    /// and the report is made without screenshot descriptions.
    pub describe_screenshots: bool,
    /// The user chose to go without reports: no language model is needed.
    pub skip_reports: bool,
    /// The first-run setup was completed. Changed only by `set_first_run_done`: a settings
    /// save keeps the stored value.
    pub first_run_done: bool,
    /// The one-time notice about a legacy Roaming `worker-venv` folder was dismissed.
    /// Changed only by `dismiss_legacy_venv_notice`: a settings save keeps the stored value.
    pub legacy_venv_notice_dismissed: bool,
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
            vision_model_path: String::new(),
            skip_screenshot_analysis: false,
            voice_accept: voice.thresholds.accept,
            voice_suggest: voice.thresholds.suggest,
            voice_margin: voice.thresholds.margin,
            clip_max_days: voice.clip_max_days,
            llm: LlmSettings::default(),
            describe_screenshots: true,
            skip_reports: false,
            first_run_done: false,
            legacy_venv_notice_dismissed: false,
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

/// The worker folder: the setting, then (development builds only) `BBB_WORKER_DIR`, then
/// `worker/` in the resource folder of the installation or next to the executable, and
/// (development builds only) in a parent folder of the executable. A release build looks
/// nowhere else, so a folder planted in a parent directory or in the environment is never run.
fn find_worker_dir(configured: &str, resource_dir: Option<&Path>) -> PathBuf {
    let env_override = std::env::var_os("BBB_WORKER_DIR");
    let exe_path = std::env::current_exe().ok();
    find_worker_dir_in(
        configured,
        resource_dir,
        env_override.as_deref(),
        exe_path.as_deref(),
        cfg!(debug_assertions),
    )
}

fn find_worker_dir_in(
    configured: &str,
    resource_dir: Option<&Path>,
    env_override: Option<&std::ffi::OsStr>,
    exe_path: Option<&Path>,
    is_debug: bool,
) -> PathBuf {
    if let Some(p) = non_empty(configured) {
        return p;
    }
    if is_debug {
        if let Some(p) = env_override.and_then(|v| non_empty(&v.to_string_lossy())) {
            return p;
        }
    }
    let is_worker = |d: &Path| d.join("pyproject.toml").is_file();
    if let Some(dir) = resource_dir {
        let candidate = dir.join("worker");
        if is_worker(&candidate) {
            return candidate;
        }
    }
    if let Some(exe) = exe_path {
        let reach = if is_debug { usize::MAX } else { 1 };
        for dir in exe.ancestors().skip(1).take(reach) {
            let candidate = dir.join("worker");
            if is_worker(&candidate) {
                return candidate;
            }
        }
    }
    PathBuf::from("worker")
}

/// The Python environment of the worker: `BBB_WORKER_ENV_DIR` when it is set to a folder
/// (a shared environment, used by the end-to-end tests so that none of them installs its own
/// copy), otherwise `worker-venv` in Local AppData.
fn worker_env_dir(local_data_dir: &Path) -> PathBuf {
    env_dir_from(local_data_dir, std::env::var_os("BBB_WORKER_ENV_DIR"))
}

fn env_dir_from(local_data_dir: &Path, shared: Option<std::ffi::OsString>) -> PathBuf {
    shared
        .and_then(|p| non_empty(&p.to_string_lossy()))
        .unwrap_or_else(|| local_data_dir.join("worker-venv"))
}

/// Dedicated `uv` cache folder under Local AppData (never `%TEMP%`).
fn uv_cache_dir_for(local_data_dir: &Path) -> PathBuf {
    local_data_dir.join("uv-cache")
}

// ----- GPU gate with live settings -----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueGpuOverrideDto {
    pub gpu_max_util_percent: u32,
    pub gpu_idle_seconds: u32,
}

/// The GPU gate, rebuilt when the thresholds in the settings or queue override change.
struct SettingsGate {
    settings: Arc<Mutex<ProcessingSettings>>,
    gpu_override: Arc<Mutex<Option<QueueGpuOverrideDto>>>,
    inner: Mutex<(NvmlSampler, Guard)>,
}

impl GpuGate for SettingsGate {
    fn check(&self, stage: Stage) -> GateDecision {
        let mut policy = match self.settings.lock() {
            Ok(s) => s.guard_policy(),
            Err(_) => return GateDecision::Wait("settings are unavailable".into()),
        };
        if let Ok(ovr) = self.gpu_override.lock() {
            if let Some(cfg) = *ovr {
                policy.max_util_percent = cfg.gpu_max_util_percent;
                policy.idle_ms = u64::from(cfg.gpu_idle_seconds) * 1000;
            }
        }
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
    /// Why a stage that is `done` did nothing (for example "the meeting has no cycle").
    pub note: Option<String>,
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
    pub scheduled_unix_ms: Option<i64>,
    pub eta_seconds: Option<u64>,
    pub stages: Vec<StageView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueState {
    pub jobs: Vec<JobView>,
    pub gpu_override: Option<QueueGpuOverrideDto>,
}

pub type QueueView = QueueState;

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

/// Side-effect hooks invoked by [`Processing`] (UI event emission and post-reduce actions).
/// Abstracted behind a trait so unit tests can construct [`Processing`] without a Tauri runtime.
pub trait ProcessingHooks: Send + Sync {
    fn emit_queue(&self, state: &QueueState);
    fn emit_models_progress(&self, payload: serde_json::Value);
    fn emit_models_changed(&self);
    fn emit_setup_progress(&self, payload: serde_json::Value);
    fn on_reduce_completed(&self, meeting_id: &str);
}

struct TauriProcessingHooks(AppHandle);

impl ProcessingHooks for TauriProcessingHooks {
    fn emit_queue(&self, state: &QueueState) {
        if let Err(e) = self.0.emit(EVENT_QUEUE, state) {
            tracing::warn!(error = %e, "could not emit the queue state");
        }
    }

    fn emit_models_progress(&self, payload: serde_json::Value) {
        let _ = self.0.emit(EVENT_MODELS_PROGRESS, payload);
    }

    fn emit_models_changed(&self) {
        let _ = self.0.emit(EVENT_MODELS_CHANGED, ());
    }

    fn emit_setup_progress(&self, payload: serde_json::Value) {
        let _ = self.0.emit(EVENT_SETUP_PROGRESS, payload);
    }

    fn on_reduce_completed(&self, meeting_id: &str) {
        if let Some(state) = self.0.try_state::<crate::state::AppState>() {
            state.publishing().draft_completed(meeting_id);
            if let Err(e) = crate::local_export::auto_export_if_enabled(&state, meeting_id) {
                tracing::warn!(
                    meeting = %meeting_id,
                    error = %e,
                    "automatic local markdown export failed"
                );
            }
        }
    }
}

#[cfg(test)]
struct NoopProcessingHooks;

#[cfg(test)]
impl ProcessingHooks for NoopProcessingHooks {
    fn emit_queue(&self, _state: &QueueState) {}
    fn emit_models_progress(&self, _payload: serde_json::Value) {}
    fn emit_models_changed(&self) {}
    fn emit_setup_progress(&self, _payload: serde_json::Value) {}
    fn on_reduce_completed(&self, _meeting_id: &str) {}
}

pub struct Processing {
    hooks: Arc<dyn ProcessingHooks>,
    store: Arc<Mutex<Store>>,
    supervisor: Arc<Supervisor>,
    settings: Arc<Mutex<ProcessingSettings>>,
    gpu_override: Arc<Mutex<Option<QueueGpuOverrideDto>>>,
    had_active_jobs: AtomicBool,
    driver_wake: Arc<(Mutex<bool>, Condvar)>,
    settings_path: PathBuf,
    roaming_data_dir: PathBuf,
    local_data_dir: PathBuf,
    env_dir: PathBuf,
    uv_cache_dir: PathBuf,
    /// The folder the installer puts its resources in (the bundled `uv` and `worker/`).
    resource_dir: Option<PathBuf>,
    live: Arc<Mutex<HashMap<String, Live>>>,
    /// Which attempt of which job already pulled its cycle repository.
    pulls: PullLedger,
    cycle_repos: RepoList,
    last_event: Mutex<Instant>,
    driver_running: Arc<AtomicBool>,
    models_busy: Arc<AtomicBool>,
    /// The processing environment is being created (`prepare_worker_env`).
    env_busy: Arc<AtomicBool>,
    env_cancel: Arc<AtomicBool>,
    llama_busy: Arc<AtomicBool>,
    llama_cancel: Arc<AtomicBool>,
    llm_busy: Arc<AtomicBool>,
    llm_cancel: Arc<AtomicBool>,
}

impl Processing {
    /// `meeting_root` is asked before every stage, so a changed recordings folder applies.
    pub fn new(
        app: AppHandle,
        store: Arc<Mutex<Store>>,
        roaming_data_dir: &Path,
        local_data_dir: &Path,
        config_dir: &Path,
        meeting_root: impl Fn() -> PathBuf + Send + Sync + 'static,
        cycle_repos: impl Fn() -> Vec<CycleRepoSetting> + Send + Sync + 'static,
    ) -> Arc<Self> {
        let resource_dir = app.path().resource_dir().ok();
        Self::new_with_hooks(
            Arc::new(TauriProcessingHooks(app)),
            store,
            roaming_data_dir,
            local_data_dir,
            config_dir,
            resource_dir,
            meeting_root,
            cycle_repos,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_hooks(
        hooks: Arc<dyn ProcessingHooks>,
        store: Arc<Mutex<Store>>,
        roaming_data_dir: &Path,
        local_data_dir: &Path,
        config_dir: &Path,
        resource_dir: Option<PathBuf>,
        meeting_root: impl Fn() -> PathBuf + Send + Sync + 'static,
        cycle_repos: impl Fn() -> Vec<CycleRepoSetting> + Send + Sync + 'static,
    ) -> Arc<Self> {
        let settings_path = config_dir.join("processing.json");
        let settings = Arc::new(Mutex::new(ProcessingSettings::load(&settings_path)));
        let gpu_override: Arc<Mutex<Option<QueueGpuOverrideDto>>> = Arc::default();
        let env_dir = worker_env_dir(local_data_dir);
        let uv_cache_dir = uv_cache_dir_for(local_data_dir);
        let live: Arc<Mutex<HashMap<String, Live>>> = Arc::default();
        let cycle_repos: RepoList = Arc::new(cycle_repos);

        let runner_settings = {
            let settings = settings.clone();
            let env_dir = env_dir.clone();
            let uv_cache_dir = uv_cache_dir.clone();
            let resource_dir = resource_dir.clone();
            move || {
                let s = settings.lock().map(|s| s.clone()).unwrap_or_default();
                RunnerSettings {
                    // The key is chosen by the runner from the options the stage runs with.
                    worker: worker_config(
                        &s,
                        &env_dir,
                        &uv_cache_dir,
                        resource_dir.as_deref(),
                        None,
                    ),
                    meeting_root: meeting_root(),
                    retention: s.audio_retention.into(),
                    voice: s.voice_settings(),
                }
            }
        };

        // The runner reports progress through the service, which exists only afterwards.
        let progress_target: Arc<Mutex<Option<std::sync::Weak<Processing>>>> = Arc::default();
        let source_repos = cycle_repos.clone();
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
        })
        .with_cycle_source(Arc::new(RepoCycleSource::new(move || source_repos())))
        .with_key_resolver(|options| llm::key_for(options, secrets::get));

        let gate = SettingsGate {
            settings: settings.clone(),
            gpu_override: gpu_override.clone(),
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
            Arc::new(AppStageRunner {
                inner: runner,
                settings: settings.clone(),
            }),
            Arc::new(gate),
            MAX_CRASHES,
        ));

        let this = Arc::new(Self {
            hooks,
            store,
            supervisor,
            settings,
            gpu_override,
            had_active_jobs: AtomicBool::new(false),
            driver_wake: Arc::new((Mutex::new(false), Condvar::new())),
            settings_path,
            roaming_data_dir: roaming_data_dir.to_path_buf(),
            local_data_dir: local_data_dir.to_path_buf(),
            env_dir,
            uv_cache_dir,
            resource_dir,
            live,
            pulls: PullLedger::default(),
            cycle_repos,
            last_event: Mutex::new(Instant::now() - EVENT_THROTTLE),
            driver_running: Arc::default(),
            models_busy: Arc::default(),
            env_busy: Arc::default(),
            env_cancel: Arc::default(),
            llama_busy: Arc::default(),
            llama_cancel: Arc::default(),
            llm_busy: Arc::default(),
            llm_cancel: Arc::default(),
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

    #[cfg(test)]
    pub fn new_for_test(
        store: Arc<Mutex<Store>>,
        dir: &Path,
        runner: Arc<dyn queue::StageRunner>,
        gate: Arc<dyn GpuGate>,
    ) -> Arc<Self> {
        let settings_path = dir.join("processing.json");
        let settings = Arc::new(Mutex::new(ProcessingSettings::default()));
        let gpu_override: Arc<Mutex<Option<QueueGpuOverrideDto>>> = Arc::default();
        let supervisor = Arc::new(Supervisor::new(store.clone(), runner, gate, MAX_CRASHES));
        let this = Arc::new(Self {
            hooks: Arc::new(NoopProcessingHooks),
            store,
            supervisor,
            settings,
            gpu_override,
            had_active_jobs: AtomicBool::new(false),
            driver_wake: Arc::new((Mutex::new(false), Condvar::new())),
            settings_path,
            roaming_data_dir: dir.join("roaming"),
            local_data_dir: dir.join("local"),
            env_dir: dir.join("local").join("worker-venv"),
            uv_cache_dir: dir.join("local").join("uv-cache"),
            resource_dir: None,
            live: Arc::default(),
            pulls: PullLedger::default(),
            cycle_repos: Arc::new(Vec::new),
            last_event: Mutex::new(Instant::now() - EVENT_THROTTLE),
            driver_running: Arc::default(),
            models_busy: Arc::default(),
            env_busy: Arc::default(),
            env_cancel: Arc::default(),
            llama_busy: Arc::default(),
            llama_cancel: Arc::default(),
            llm_busy: Arc::default(),
            llm_cancel: Arc::default(),
        });
        let weak = Arc::downgrade(&this);
        this.supervisor.set_listener(move |_| {
            if let Some(p) = weak.upgrade() {
                p.emit_queue();
            }
        });
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
        // A key is stored for one host: the flags tell whether there is one for the
        // address in the settings now.
        let key_set = |kind| {
            s.llm
                .key_entry(kind)
                .is_some_and(|entry| secrets::get(&entry).is_some())
        };
        let (cloud_set, external_set) = (key_set(LlmKind::Cloud), key_set(LlmKind::External));
        s.llm.cloud_key_set = cloud_set;
        s.llm.external_key_set = external_set;
        s.llm.external_host_is_remote = s.llm.external_default_is_remote();
        Ok(s)
    }

    pub fn save_settings(&self, mut new: ProcessingSettings) -> IpcResult<()> {
        if new.gpu_max_util_percent == 0 || new.gpu_max_util_percent > 100 {
            return Err(IpcError::new(
                "invalid",
                "the GPU utilization limit must be between 1 and 100",
            ));
        }
        new.llm.validate()?;
        let external_key_stored = new
            .llm
            .key_entry(LlmKind::External)
            .is_some_and(|entry| secrets::get(&entry).is_some());
        new.llm.check_external_key_transport(external_key_stored)?;
        let thresholds = new.voice_settings().thresholds;
        if !thresholds.is_valid() {
            return Err(IpcError::new(
                "invalid",
                "voice thresholds must satisfy 0 <= suggest <= accept <= 1 and margin >= 0",
            ));
        }
        let mut current = self
            .settings
            .lock()
            .map_err(|_| IpcError::new("internal", "settings lock poisoned"))?;
        // A path that is run or read later is checked when it changes (an executable is never
        // taken on trust), and the setup flag is not the settings screen's to change.
        setup::validate_settings_paths(&new, &current)
            .map_err(|p| IpcError::new(p.code, p.message))?;
        new.first_run_done = current.first_run_done;
        new.legacy_venv_notice_dismissed = current.legacy_venv_notice_dismissed;
        new.save(&self.settings_path)?;
        *current = new;
        Ok(())
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

    pub(crate) fn worker_config(&self) -> WorkerConfig {
        let s = self.settings.lock().map(|s| s.clone()).unwrap_or_default();
        worker_config(
            &s,
            &self.env_dir,
            &self.uv_cache_dir,
            self.resource_dir.as_deref(),
            None,
        )
    }

    /// The settings as stored (without the flags filled in by [`Processing::settings`]).
    pub(crate) fn settings_snapshot(&self) -> ProcessingSettings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub(crate) fn roaming_data_dir(&self) -> &Path {
        &self.roaming_data_dir
    }

    pub(crate) fn local_data_dir(&self) -> &Path {
        &self.local_data_dir
    }

    pub(crate) fn env_dir(&self) -> &Path {
        &self.env_dir
    }

    pub(crate) fn uv_cache_dir(&self) -> &Path {
        &self.uv_cache_dir
    }

    pub(crate) fn resource_dir(&self) -> Option<&Path> {
        self.resource_dir.as_deref()
    }

    pub(crate) fn models_busy(&self) -> bool {
        self.models_busy.load(Ordering::SeqCst)
    }

    pub(crate) fn env_busy(&self) -> &Arc<AtomicBool> {
        &self.env_busy
    }

    pub(crate) fn env_cancel(&self) -> &Arc<AtomicBool> {
        &self.env_cancel
    }

    pub(crate) fn llama_busy(&self) -> &Arc<AtomicBool> {
        &self.llama_busy
    }

    pub(crate) fn llama_cancel(&self) -> &Arc<AtomicBool> {
        &self.llama_cancel
    }

    pub(crate) fn llm_busy(&self) -> &Arc<AtomicBool> {
        &self.llm_busy
    }

    pub(crate) fn llm_cancel(&self) -> &Arc<AtomicBool> {
        &self.llm_cancel
    }

    pub(crate) fn cancel_setup_tasks(&self) {
        self.env_cancel.store(true, Ordering::SeqCst);
        self.llama_cancel.store(true, Ordering::SeqCst);
        self.llm_cancel.store(true, Ordering::SeqCst);
    }

    pub(crate) fn downloads_dir(&self) -> PathBuf {
        self.local_data_dir.join("downloads")
    }

    pub(crate) fn emit_setup_event(&self, event: setup::SetupProgressEvent) {
        if let Ok(payload) = serde_json::to_value(event) {
            self.hooks.emit_setup_progress(payload);
        }
    }

    /// Records that the first-run setup was completed (or is to be run again).
    pub fn set_first_run_done(&self, done: bool) -> IpcResult<()> {
        let mut guard = self.settings.lock().map_err(poisoned)?;
        let mut next = guard.clone();
        next.first_run_done = done;
        next.save(&self.settings_path)?;
        *guard = next;
        Ok(())
    }

    /// Marks the one-time notice about a legacy Roaming `worker-venv` folder as dismissed.
    pub fn dismiss_legacy_venv_notice(&self) -> IpcResult<()> {
        let mut guard = self.settings.lock().map_err(poisoned)?;
        let mut next = guard.clone();
        next.legacy_venv_notice_dismissed = true;
        next.save(&self.settings_path)?;
        *guard = next;
        Ok(())
    }

    /// The language-model settings, for choosing a backend and the cloud dialog.
    pub fn llm_settings(&self) -> IpcResult<LlmSettings> {
        Ok(self.settings.lock().map_err(poisoned)?.llm.clone())
    }

    // ----- queue

    fn wake_driver(self: &Arc<Self>) {
        if let Ok(mut notified) = self.driver_wake.0.lock() {
            *notified = true;
            self.driver_wake.1.notify_all();
        }
        self.kick();
    }

    fn wait_driver_poll(&self, timeout: Duration) {
        let Ok(mut guard) = self.driver_wake.0.lock() else {
            std::thread::sleep(timeout);
            return;
        };
        if *guard {
            *guard = false;
            return;
        }
        if let Ok((mut g, _)) = self.driver_wake.1.wait_timeout(guard, timeout) {
            *g = false;
        }
    }

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

    pub fn process(self: &Arc<Self>, meeting_id: &str, mut options: JobOptions) -> IpcResult<()> {
        let llm_settings = self.llm_settings()?;
        let state = {
            let store = self.store.lock().map_err(poisoned)?;
            let details = store
                .get_meeting(meeting_id)?
                .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?;
            apply_project_prefs_to_job_options(&store, meeting_id, &llm_settings, &mut options)?;
            details.meeting.state
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
        let mut options = options;
        options.skip_vision = should_skip_vision(&self.settings_snapshot());
        let json = serde_json::to_string(&options).unwrap_or_else(|_| "{}".into());
        let created = self
            .supervisor
            .enqueue(meeting_id, &json)
            .map_err(|e| IpcError::new("internal", e.to_string()))?;
        if !created {
            self.apply_llm_to_existing_job(meeting_id, &options)?;
            self.refresh_skip_vision(meeting_id)?;
        }
        self.had_active_jobs.store(true, Ordering::SeqCst);
        self.emit_queue();
        self.wake_driver();
        Ok(())
    }

    /// Enqueue a processing job without immediately kicking the background driver.
    pub fn enqueue_only(&self, meeting_id: &str, mut options: JobOptions) -> IpcResult<()> {
        let llm_settings = self.llm_settings()?;
        let state = {
            let store = self.store.lock().map_err(poisoned)?;
            let details = store
                .get_meeting(meeting_id)?
                .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?;
            apply_project_prefs_to_job_options(&store, meeting_id, &llm_settings, &mut options)?;
            details.meeting.state
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
        options.skip_vision = should_skip_vision(&self.settings_snapshot());
        let json = serde_json::to_string(&options).unwrap_or_else(|_| "{}".into());
        let created = self
            .supervisor
            .enqueue(meeting_id, &json)
            .map_err(|e| IpcError::new("internal", e.to_string()))?;
        if !created {
            self.apply_llm_to_existing_job(meeting_id, &options)?;
        }
        self.had_active_jobs.store(true, Ordering::SeqCst);
        self.emit_queue();
        Ok(())
    }

    pub fn set_gpu_override(
        self: &Arc<Self>,
        override_cfg: Option<QueueGpuOverrideDto>,
    ) -> IpcResult<QueueState> {
        if let Some(cfg) = &override_cfg {
            if cfg.gpu_max_util_percent == 0 || cfg.gpu_max_util_percent > 100 {
                return Err(IpcError::new(
                    "invalid",
                    "the GPU utilization limit must be between 1 and 100",
                ));
            }
        }
        *self.gpu_override.lock().map_err(poisoned)? = override_cfg;
        if self.has_runnable() {
            self.had_active_jobs.store(true, Ordering::SeqCst);
        }
        self.emit_queue();
        if override_cfg.is_some() && self.has_runnable() {
            self.wake_driver();
        }
        self.queue_state()
    }

    pub fn force_process(self: &Arc<Self>, meeting_id: &str) -> IpcResult<JobView> {
        self.ensure_job_exists(meeting_id)?;
        self.supervisor
            .set_schedule(meeting_id, None)
            .map_err(|e| IpcError::new("internal", e.to_string()))?;
        self.supervisor.force_job(meeting_id);
        self.had_active_jobs.store(true, Ordering::SeqCst);
        self.emit_queue();
        self.wake_driver();
        self.job_view(meeting_id)
    }

    pub fn schedule_job(
        self: &Arc<Self>,
        meeting_id: &str,
        scheduled_unix_ms: Option<i64>,
    ) -> IpcResult<JobView> {
        self.ensure_job_exists(meeting_id)?;
        self.supervisor
            .set_schedule(meeting_id, scheduled_unix_ms)
            .map_err(|e| IpcError::new("internal", e.to_string()))?;
        self.had_active_jobs.store(true, Ordering::SeqCst);
        self.emit_queue();
        self.wake_driver();
        self.job_view(meeting_id)
    }

    fn ensure_job_exists(&self, meeting_id: &str) -> IpcResult<()> {
        let has_job = {
            let store = self.store.lock().map_err(poisoned)?;
            store.get_job(meeting_id)?.is_some()
        };
        if has_job {
            return Ok(());
        }
        let llm_settings = self.llm_settings()?;
        let mut options = JobOptions::default();
        {
            let store = self.store.lock().map_err(poisoned)?;
            let details = store
                .get_meeting(meeting_id)?
                .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?;
            if !matches!(
                details.meeting.state,
                store::MeetingState::Stopped | store::MeetingState::Recovered
            ) {
                return Err(IpcError::new(
                    "invalid_state",
                    "only a finished recording can be processed",
                ));
            }
            apply_project_prefs_to_job_options(&store, meeting_id, &llm_settings, &mut options)?;
        }
        let json = serde_json::to_string(&options).unwrap_or_else(|_| "{}".into());
        self.supervisor
            .enqueue(meeting_id, &json)
            .map_err(|e| IpcError::new("internal", e.to_string()))?;
        Ok(())
    }

    pub fn job_view(&self, meeting_id: &str) -> IpcResult<JobView> {
        let q = self.queue_state()?;
        q.jobs
            .into_iter()
            .find(|j| j.meeting_id == meeting_id)
            .ok_or_else(|| IpcError::new("not_found", "job not found"))
    }

    /// A job that exists keeps its options, except for a backend chosen now.
    fn apply_llm_to_existing_job(&self, meeting_id: &str, options: &JobOptions) -> IpcResult<()> {
        let store = self.store.lock().map_err(poisoned)?;
        apply_llm_to_job(&store, meeting_id, options)
    }

    /// Before the briefing (`extract`) of a meeting, bring a cloned cycle repository up to
    /// date: once per job and attempt of the stage, however often the queue looks at the job
    /// while it waits for the GPU. Runs outside the store lock; a failed pull, a conflict or
    /// uncommitted cycle edits only mean the local state is used.
    fn prepare_step(&self, meeting_id: &str) {
        let (job, cycle) = {
            let Ok(store) = self.store.lock() else {
                return;
            };
            (
                store.get_job(meeting_id).ok().flatten(),
                store
                    .get_report_settings(meeting_id)
                    .ok()
                    .flatten()
                    .and_then(|s| s.cycle_id),
            )
        };
        let Some(job) = job else {
            return;
        };
        if matches!(
            queue::next_step(&job),
            Next::Run {
                stage: Stage::Vision
            }
        ) && self
            .settings
            .lock()
            .map(|s| should_skip_vision(&s))
            .unwrap_or(false)
        {
            if let Ok(store) = self.store.lock() {
                let _ = store.mark_stage_skipped(
                    meeting_id,
                    Stage::Vision.as_str(),
                    "screenshot analysis disabled in settings",
                    queue::now_ms(),
                );
            }
            return;
        }
        let briefing = matches!(
            queue::next_step(&job),
            Next::Run {
                stage: Stage::Extract
            }
        );
        if let (true, Some(cycle)) = (briefing, cycle) {
            let attempt = job
                .stages
                .iter()
                .find(|s| s.stage == Stage::Extract.as_str())
                .map_or(0, |s| s.attempts);
            crate::cycles::pull_for_briefing(
                &self.pulls,
                &(self.cycle_repos)(),
                meeting_id,
                attempt,
                &cycle,
            );
        }
    }

    /// A job keeps its options, but the screenshot setting of now applies when it runs again.
    fn refresh_skip_vision(&self, meeting_id: &str) -> IpcResult<()> {
        let skip = should_skip_vision(&self.settings_snapshot());
        let store = self.store.lock().map_err(poisoned)?;
        set_skip_vision(&store, meeting_id, skip)
    }

    pub fn retry(self: &Arc<Self>, meeting_id: &str) -> IpcResult<()> {
        self.refresh_skip_vision(meeting_id)?;
        self.supervisor
            .retry(meeting_id)
            .map_err(|e| IpcError::new("not_found", e.to_string()))?;
        self.had_active_jobs.store(true, Ordering::SeqCst);
        self.emit_queue();
        self.wake_driver();
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
        self.supervisor.clear_force(meeting_id);
        if !self.has_runnable() {
            self.reset_gpu_override_on_idle();
        }
        self.emit_queue();
        Ok(())
    }

    fn reset_gpu_override_on_idle(&self) {
        self.had_active_jobs.store(false, Ordering::SeqCst);
        if let Ok(mut ovr) = self.gpu_override.lock() {
            *ovr = None;
        }
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
                // A panic must not leave the flag set: no driver would ever start again.
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| this.drive()));
                this.driver_running.store(false, Ordering::SeqCst);
                if outcome.is_err() {
                    tracing::error!("the queue driver stopped after a panic");
                    this.emit_queue();
                    return;
                }
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

    fn has_scheduled_or_running(&self) -> bool {
        let Ok(store) = self.store.lock() else {
            return false;
        };
        let now = queue::now_ms();
        store
            .list_job_ids()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| store.get_job(&id).ok().flatten())
            .any(|j| {
                matches!(queue::next_step(&j), Next::Run { .. })
                    && (j.stages.iter().any(|s| s.status == StageStatus::Running)
                        || j.scheduled_unix_ms.is_some_and(|ts| ts > now))
            })
    }

    fn drive(&self) {
        let mut wake_lock: Option<recorder::SystemWakeLock> = None;
        loop {
            let ids = self.runnable_ids();
            if ids.is_empty() {
                self.reset_gpu_override_on_idle();
                break;
            }
            self.had_active_jobs.store(true, Ordering::SeqCst);
            if wake_lock.is_none() {
                wake_lock = Some(recorder::SystemWakeLock::acquire());
            }
            let mut progressed = false;
            let mut earliest_schedule_wait: Option<Duration> = None;
            for id in ids {
                self.prepare_step(&id);
                match self.supervisor.step(&id) {
                    Ok(StepOutcome::Ran { stage }) => {
                        progressed = true;
                        // The draft is complete: what the cycle publishes by itself goes now.
                        if matches!(stage, Stage::Reduce) {
                            self.hooks.on_reduce_completed(&id);
                        }
                    }
                    Ok(StepOutcome::Failed { stage, error }) => {
                        // The text can quote meeting content (an invalid answer of the model, a
                        // traceback): the log keeps its size only; the job row has the text.
                        tracing::error!(
                            meeting = %id,
                            stage = stage.as_str(),
                            error_chars = error.chars().count(),
                            "stage failed"
                        );
                        progressed = true;
                    }
                    Ok(StepOutcome::Scheduled {
                        scheduled_unix_ms, ..
                    }) => {
                        let remain_ms = (scheduled_unix_ms - queue::now_ms()).max(0) as u64;
                        let wait = Duration::from_millis(remain_ms).min(WAIT_POLL);
                        earliest_schedule_wait = Some(match earliest_schedule_wait {
                            Some(cur) => cur.min(wait),
                            None => wait,
                        });
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(error = %e, "queue step failed"),
                }
                if let Ok(mut l) = self.live.lock() {
                    l.remove(&id);
                }
            }
            if !self.has_runnable() {
                self.reset_gpu_override_on_idle();
            }
            self.emit_queue();
            if !progressed {
                if !self.has_scheduled_or_running() {
                    wake_lock = None;
                }
                let wait = earliest_schedule_wait.unwrap_or(WAIT_POLL);
                self.wait_driver_poll(wait);
            }
        }
        drop(wake_lock);
        self.emit_queue();
    }

    // ----- views

    pub fn queue_state(&self) -> IpcResult<QueueState> {
        let store = self.store.lock().map_err(poisoned)?;
        let live = self.live.lock().map_err(poisoned)?.clone();
        let now = queue::now_ms();
        let mut jobs = Vec::new();
        for id in store.list_job_ids()? {
            let Some(job) = store.get_job(&id)? else {
                continue;
            };
            let meeting_details = store.get_meeting(&id)?;
            let duration_ms = meeting_details.as_ref().and_then(|d| d.meeting.duration_ms);
            let title = meeting_details
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
                        note: row.and_then(|r| r.note.clone()),
                        fraction: l.map(|l| l.fraction),
                        message: l.map(|l| l.message.clone()),
                    }
                })
                .collect();
            let live_pair = live_now.map(|l| (l.stage, l.fraction as f32));
            let progress = queue::progress_with_live(&job, live_pair);
            let eta_seconds = estimate_eta_seconds(&job, state, progress, duration_ms, now);
            jobs.push(JobView {
                meeting_id: id,
                title,
                state,
                progress,
                waiting_reason: if state == JobState::Waiting {
                    waiting_reason
                } else {
                    None
                },
                scheduled_unix_ms: job.scheduled_unix_ms,
                eta_seconds,
                stages,
            });
        }
        drop(store);
        let has_active = jobs.iter().any(|j| {
            matches!(
                j.state,
                JobState::Queued | JobState::Waiting | JobState::Running
            )
        });
        if has_active {
            self.had_active_jobs.store(true, Ordering::SeqCst);
        } else if self.had_active_jobs.swap(false, Ordering::SeqCst) {
            if let Ok(mut ovr) = self.gpu_override.lock() {
                *ovr = None;
            }
        }
        let gpu_override = *self.gpu_override.lock().map_err(poisoned)?;
        Ok(QueueState { jobs, gpu_override })
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
            self.hooks.emit_queue(&state);
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
                let started = Instant::now();
                this.emit_setup_event(setup::SetupProgressEvent {
                    task: setup::SetupTask::SpeechModels,
                    phase: setup::SetupPhase::Started,
                    downloaded_bytes: None,
                    total_bytes: None,
                    elapsed_secs: 0,
                    message: "Downloading speech models".to_string(),
                });
                let mut cfg = this.worker_config();
                cfg.hf_token = secrets::get(secrets::HF_TOKEN);
                let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
                let result = worker::ensure_environment(&cfg).and_then(|()| {
                    worker::ensure_models(&cfg, &refs, |ev| {
                        if let WorkerEvent::Progress {
                            fraction, message, ..
                        } = ev
                        {
                            this.hooks.emit_models_progress(serde_json::json!({
                                "key": refs.first().copied().unwrap_or(""),
                                "fraction": fraction,
                                "message": message,
                            }));
                            this.emit_setup_event(setup::SetupProgressEvent {
                                task: setup::SetupTask::SpeechModels,
                                phase: setup::SetupPhase::Progress,
                                downloaded_bytes: None,
                                total_bytes: None,
                                elapsed_secs: started.elapsed().as_secs(),
                                message: message.to_string(),
                            });
                        }
                    })
                });
                match result {
                    Ok(()) => {
                        this.emit_setup_event(setup::SetupProgressEvent {
                            task: setup::SetupTask::SpeechModels,
                            phase: setup::SetupPhase::Done,
                            downloaded_bytes: None,
                            total_bytes: None,
                            elapsed_secs: started.elapsed().as_secs(),
                            message: "Speech models are ready".to_string(),
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e.message, "model download failed");
                        this.hooks.emit_models_progress(serde_json::json!({
                            "key": refs.first().copied().unwrap_or(""),
                            "fraction": 0.0,
                            "message": format!("Failed: {}", e.message),
                        }));
                        this.emit_setup_event(setup::SetupProgressEvent {
                            task: setup::SetupTask::SpeechModels,
                            phase: setup::SetupPhase::Failed,
                            downloaded_bytes: None,
                            total_bytes: None,
                            elapsed_secs: started.elapsed().as_secs(),
                            message: e.message,
                        });
                    }
                }
                this.models_busy.store(false, Ordering::SeqCst);
                this.hooks.emit_models_changed();
            });
        if spawned.is_err() {
            self.models_busy.store(false, Ordering::SeqCst);
            return Err(IpcError::new("internal", "could not start the download"));
        }
        Ok(())
    }
}

/// Estimate remaining seconds for an active/queued/waiting job based on `progress_with_live`
/// and elapsed stage execution time, falling back to a reasonable initial estimate.
pub fn estimate_eta_seconds(
    job: &store::JobRow,
    state: JobState,
    progress: f32,
    meeting_duration_ms: Option<i64>,
    now_ms: i64,
) -> Option<u64> {
    if matches!(state, JobState::Done | JobState::Failed) || progress >= 1.0 {
        return None;
    }
    let remaining = (1.0 - progress.clamp(0.0, 1.0)).max(0.0);
    if remaining <= 0.0 {
        return None;
    }
    let baseline_total_secs = match meeting_duration_ms {
        Some(ms) if ms > 0 => ((ms as f32 / 1000.0) * 0.25).clamp(30.0, 3600.0),
        _ => 120.0,
    };
    let baseline_remaining = baseline_total_secs * remaining;

    let mut elapsed_ms: i64 = 0;
    for row in &job.stages {
        if row.status == StageStatus::Done && row.note.is_some() {
            continue;
        }
        match (row.status, row.started_unix_ms, row.finished_unix_ms) {
            (StageStatus::Done, Some(start), Some(end)) if end > start => {
                elapsed_ms += end - start;
            }
            (StageStatus::Running, Some(start), _) if now_ms > start => {
                elapsed_ms += now_ms - start;
            }
            _ => {}
        }
    }

    let est_secs = if progress > 0.03 && elapsed_ms >= 500 {
        let elapsed_secs = elapsed_ms as f32 / 1000.0;
        let rate_remaining = elapsed_secs * (remaining / progress);
        let blend = ((progress - 0.03) / 0.22).clamp(0.0, 1.0);
        baseline_remaining * (1.0 - blend) + rate_remaining * blend
    } else {
        baseline_remaining
    };
    Some(est_secs.ceil().max(1.0) as u64)
}

/// Apply `ProjectPrefs.llm_kind_override` and `llm_model_override` to `JobOptions` when building
/// options for a meeting of `cycle_id`.
pub fn apply_project_prefs_to_job_options(
    store: &Store,
    meeting_id: &str,
    llm_settings: &LlmSettings,
    options: &mut JobOptions,
) -> IpcResult<()> {
    let Some(cycle_id) = store
        .get_report_settings(meeting_id)?
        .and_then(|r| r.cycle_id)
    else {
        return Ok(());
    };
    let prefs = store.get_project_prefs(&cycle_id)?;
    let kind_ovr = prefs
        .llm_kind_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let model_ovr = prefs
        .llm_model_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if kind_ovr.is_none() && model_ovr.is_none() {
        return Ok(());
    }

    if let Some(k) = kind_ovr {
        let kind = llm::parse_kind(k)?;
        let need_replace = options.llm.as_ref().is_none_or(|l| l.kind != kind);
        if need_replace {
            let built =
                llm_settings
                    .options_for(kind)
                    .unwrap_or_else(|_| queue::worker::LlmOptions {
                        kind,
                        model_path: (kind == LlmKind::Managed)
                            .then(|| llm_settings.managed.model_path.trim().to_string())
                            .filter(|s| !s.is_empty()),
                        ctx: (kind == LlmKind::Managed).then_some(llm_settings.managed.ctx),
                        base_url: match kind {
                            LlmKind::Managed => None,
                            LlmKind::External => {
                                Some(llm_settings.external.base_url.trim().to_string())
                                    .filter(|s| !s.is_empty())
                            }
                            LlmKind::Cloud => Some(llm_settings.cloud.base_url.trim().to_string())
                                .filter(|s| !s.is_empty()),
                        },
                        model: match kind {
                            LlmKind::Managed => None,
                            LlmKind::External => {
                                Some(llm_settings.external.model.trim().to_string())
                                    .filter(|s| !s.is_empty())
                            }
                            LlmKind::Cloud => Some(llm_settings.cloud.model.trim().to_string())
                                .filter(|s| !s.is_empty()),
                        },
                        cloud_confirmed: false,
                    });
            options.llm = Some(built);
        }
    }

    if let Some(m) = model_ovr {
        if options.llm.is_none() {
            let kind = llm_settings.default_kind;
            options.llm = Some(llm_settings.options_for(kind).unwrap_or_else(|_| {
                queue::worker::LlmOptions {
                    kind,
                    model_path: None,
                    ctx: (kind == LlmKind::Managed).then_some(llm_settings.managed.ctx),
                    base_url: (kind == LlmKind::External)
                        .then(|| llm_settings.external.base_url.trim().to_string())
                        .filter(|s| !s.is_empty()),
                    model: None,
                    cloud_confirmed: false,
                }
            }));
        }
        if let Some(llm_opts) = options.llm.as_mut() {
            if llm_opts.kind == LlmKind::Managed {
                llm_opts.model_path = Some(m.to_string());
                llm_opts.model = Some(m.to_string());
            } else {
                llm_opts.model = Some(m.to_string());
            }
        }
    }
    Ok(())
}

/// Gives an existing job the backend chosen now: the job's options keep everything else, and
/// the stages that were skipped for lack of a backend (or failed) run again. Does nothing
/// without a chosen backend. `busy` while one of the two stages is running.
fn apply_llm_to_job(store: &Store, meeting_id: &str, options: &JobOptions) -> IpcResult<()> {
    let Some(chosen) = options.llm.clone() else {
        return Ok(());
    };
    let Some(job) = store.get_job(meeting_id)? else {
        return Ok(());
    };
    let llm_stages = [Stage::Extract, Stage::Reduce];
    let row = |st: Stage| job.stages.iter().find(|r| r.stage == st.as_str());
    if llm_stages
        .iter()
        .any(|st| row(*st).is_some_and(|r| r.status == StageStatus::Running))
    {
        return Err(IpcError::new(
            "busy",
            "the report is being extracted; wait for it to finish",
        ));
    }
    let mut current = JobOptions::from_json(&job.options);
    current.llm = Some(chosen);
    store.set_job_options(
        meeting_id,
        &serde_json::to_string(&current).unwrap_or_else(|_| "{}".into()),
        queue::now_ms(),
    )?;
    for st in llm_stages {
        let again = row(st).is_some_and(|r| {
            r.status == StageStatus::Failed || (r.status == StageStatus::Done && r.note.is_some())
        });
        if again {
            store.set_stage_status(
                meeting_id,
                st.as_str(),
                StageStatus::Pending,
                None,
                queue::now_ms(),
            )?;
        }
    }
    Ok(())
}

fn should_skip_vision(s: &ProcessingSettings) -> bool {
    !s.describe_screenshots || s.skip_screenshot_analysis
}

/// Stores `skip_vision` in the options of the job of a meeting, when it differs. Does nothing
/// for a meeting without a job.
fn set_skip_vision(store: &Store, meeting_id: &str, skip: bool) -> IpcResult<()> {
    let Some(job) = store.get_job(meeting_id)? else {
        return Ok(());
    };
    let mut options = JobOptions::from_json(&job.options);
    if options.skip_vision != skip {
        options.skip_vision = skip;
        store.set_job_options(
            meeting_id,
            &serde_json::to_string(&options).unwrap_or_else(|_| "{}".into()),
            queue::now_ms(),
        )?;
    }
    Ok(())
}

struct AppStageRunner {
    inner: PipelineRunner,
    settings: Arc<Mutex<ProcessingSettings>>,
}

fn vision_dir_override(path_str: &str) -> Option<PathBuf> {
    let trimmed = path_str.trim();
    if trimmed.is_empty() {
        return None;
    }
    let p = Path::new(trimmed);
    if p.is_dir() {
        Some(p.to_path_buf())
    } else if p.is_file() || p.extension().is_some() {
        p.parent().map(Path::to_path_buf)
    } else {
        Some(p.to_path_buf())
    }
}

impl queue::StageRunner for AppStageRunner {
    fn run(&self, meeting_id: &str, stage: Stage) -> Result<(), queue::RunError> {
        if stage == Stage::Vision {
            let (skip, vision_path) = self
                .settings
                .lock()
                .map(|s| (should_skip_vision(&s), s.vision_model_path.clone()))
                .unwrap_or((false, String::new()));
            if skip {
                return Ok(());
            }
            if let Some(dir) = vision_dir_override(&vision_path) {
                struct EnvGuard(Option<std::ffi::OsString>);
                impl Drop for EnvGuard {
                    fn drop(&mut self) {
                        match &self.0 {
                            Some(prev) => std::env::set_var("BBB_VISION_DIR", prev),
                            None => std::env::remove_var("BBB_VISION_DIR"),
                        }
                    }
                }
                let prev = std::env::var_os("BBB_VISION_DIR");
                std::env::set_var("BBB_VISION_DIR", &dir);
                let _guard = EnvGuard(prev);
                return self.inner.run(meeting_id, stage);
            }
        }
        self.inner.run(meeting_id, stage)
    }
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> IpcError {
    IpcError::new("internal", "lock poisoned")
}

fn worker_config(
    s: &ProcessingSettings,
    env_dir: &Path,
    uv_cache_dir: &Path,
    resource_dir: Option<&Path>,
    llm_api_key: Option<worker::Secret>,
) -> WorkerConfig {
    let path_var = std::env::var_os("PATH");
    WorkerConfig {
        // The setting, the bundled uv, then the PATH (absolute folders only). When none of them
        // gives a usable program the path stays empty, so starting it fails; a program that
        // did not pass the check is never run. The readiness check refuses the job before that.
        uv: setup::resolve_uv(&s.uv_path, resource_dir, path_var.as_deref()).unwrap_or_default(),
        worker_dir: find_worker_dir(&s.worker_dir, resource_dir),
        env_dir: env_dir.to_path_buf(),
        uv_cache_dir: Some(uv_cache_dir.to_path_buf()),
        models_dir: non_empty(&s.models_dir),
        llama_server: setup::resolve_llama(&s.llama_server_path, &setup::DetectEnv::from_process())
            .path(),
        hf_token: secrets::get(secrets::HF_TOKEN),
        // The reference estimate for a 10-hour meeting is below 4 hours; stop after 6.
        step_timeout: Duration::from_secs(6 * 3600),
        // Only for the extraction stages of a job with an external or cloud model; the worker
        // hands it to those processes through their environment and to no other.
        llm_api_key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_environment_is_per_data_folder_unless_a_shared_one_is_given() {
        let data = Path::new("D:/data");
        assert_eq!(env_dir_from(data, None), data.join("worker-venv"));
        assert_eq!(
            env_dir_from(data, Some("".into())),
            data.join("worker-venv")
        );
        assert_eq!(
            env_dir_from(data, Some("  ".into())),
            data.join("worker-venv")
        );
        assert_eq!(
            env_dir_from(data, Some("D:/shared/venv".into())),
            PathBuf::from("D:/shared/venv")
        );
    }

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
        assert_eq!(json["visionModelPath"], "");
        assert_eq!(json["skipScreenshotAnalysis"], false);
        assert!(json.get("gpuIdleSeconds").is_some());
        let parsed: ProcessingSettings = serde_json::from_str(
            r#"{"audioRetention":{"mode":"keepDays","days":3},"visionModelPath":"C:/models/qwen-vl.gguf","skipScreenshotAnalysis":true,"hfTokenSet":true}"#,
        )
        .unwrap();
        // The retired "keep for N days" choice is read as "keep".
        assert_eq!(
            AudioRetention::from(parsed.audio_retention),
            AudioRetention::Keep
        );
        assert_eq!(parsed.vision_model_path, "C:/models/qwen-vl.gguf");
        assert!(parsed.skip_screenshot_analysis);
        // The stored flag is never read back from a saved file.
        assert!(!parsed.hf_token_set);
    }

    #[test]
    fn stored_audio_retention_settings_stay_readable() {
        let parse = |json: &str| -> AudioRetention {
            serde_json::from_str::<ProcessingSettings>(json)
                .unwrap()
                .audio_retention
                .into()
        };
        // Files written before the setting existed, or with the old shape, still load.
        assert_eq!(parse("{}"), AudioRetention::Delete);
        assert_eq!(
            parse(r#"{"audioRetention":{"mode":"delete","days":30}}"#),
            AudioRetention::Delete
        );
        assert_eq!(
            parse(r#"{"audioRetention":{"mode":"keep","days":7}}"#),
            AudioRetention::Keep
        );
        assert_eq!(parse(r#"{"audioRetention":{}}"#), AudioRetention::Delete);
        // The new shape has no `days`.
        let json = serde_json::to_value(ProcessingSettings::default()).unwrap();
        assert!(json["audioRetention"].get("days").is_none());
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
        assert_eq!(find_worker_dir("D:/w", None), PathBuf::from("D:/w"));
    }

    fn job_store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&store::Meeting {
            id: "m1".into(),
            title: "t".into(),
            started_unix_ms: 1,
            duration_ms: None,
            state: store::MeetingState::Stopped,
            dir: "m1".into(),
        })
        .unwrap();
        let names: Vec<&str> = STAGES.iter().map(|s| s.as_str()).collect();
        s.create_job(
            "m1",
            &names,
            &serde_json::to_string(&JobOptions::default()).unwrap(),
            1,
        )
        .unwrap();
        s
    }

    fn external() -> JobOptions {
        JobOptions {
            llm: Some(queue::worker::LlmOptions {
                kind: LlmKind::External,
                model_path: None,
                ctx: None,
                base_url: Some("http://127.0.0.1:11434/v1".into()),
                model: Some("gemma".into()),
                cloud_confirmed: false,
            }),
            ..JobOptions::default()
        }
    }

    fn stage(s: &Store, st: Stage) -> store::StageRow {
        s.get_job("m1")
            .unwrap()
            .unwrap()
            .stages
            .into_iter()
            .find(|r| r.stage == st.as_str())
            .unwrap()
    }

    #[test]
    fn a_backend_chosen_later_reruns_the_skipped_stages_and_keeps_the_other_options() {
        let s = job_store();
        s.mark_stage_skipped("m1", "extract", "no language model backend was chosen", 5)
            .unwrap();
        s.mark_stage_skipped("m1", "reduce", "no language model backend was chosen", 5)
            .unwrap();
        s.set_stage_status("m1", "asr", StageStatus::Done, None, 5)
            .unwrap();
        // A call without a backend changes nothing.
        apply_llm_to_job(&s, "m1", &JobOptions::default()).unwrap();
        assert!(stage(&s, Stage::Extract).note.is_some());

        let mut chosen = external();
        chosen.language = "pl".into();
        apply_llm_to_job(&s, "m1", &chosen).unwrap();
        let job = s.get_job("m1").unwrap().unwrap();
        let stored = JobOptions::from_json(&job.options);
        assert_eq!(stored.llm.as_ref().unwrap().kind, LlmKind::External);
        // Only the backend is taken over; the language of the first call stays.
        assert_eq!(stored.language, "auto");
        for st in [Stage::Extract, Stage::Reduce] {
            let r = stage(&s, st);
            assert_eq!((r.status, r.note), (StageStatus::Pending, None));
        }
        assert_eq!(stage(&s, Stage::Asr).status, StageStatus::Done);
    }

    #[test]
    fn a_failed_extraction_runs_again_but_a_real_one_is_left_alone_and_a_running_one_blocks() {
        let s = job_store();
        s.set_stage_status("m1", "extract", StageStatus::Failed, Some("boom"), 5)
            .unwrap();
        s.set_stage_status("m1", "reduce", StageStatus::Done, None, 5)
            .unwrap();
        apply_llm_to_job(&s, "m1", &external()).unwrap();
        assert_eq!(stage(&s, Stage::Extract).status, StageStatus::Pending);
        // `reduce` really ran (no skip note): not repeated.
        assert_eq!(stage(&s, Stage::Reduce).status, StageStatus::Done);

        s.set_stage_status("m1", "extract", StageStatus::Running, None, 6)
            .unwrap();
        let e = apply_llm_to_job(&s, "m1", &external()).unwrap_err();
        assert_eq!(e.code, "busy");
        // A meeting without a job is not an error here.
        apply_llm_to_job(&s, "nope", &external()).unwrap();
    }

    #[test]
    fn the_screenshot_setting_reaches_a_job_that_exists_and_keeps_its_other_options() {
        let s = job_store();
        let options = JobOptions {
            language: "pl".into(),
            ..JobOptions::default()
        };
        s.set_job_options("m1", &serde_json::to_string(&options).unwrap(), 2)
            .unwrap();
        let stored = |s: &Store| JobOptions::from_json(&s.get_job("m1").unwrap().unwrap().options);
        assert!(!stored(&s).skip_vision);
        set_skip_vision(&s, "m1", true).unwrap();
        let after = stored(&s);
        assert!(after.skip_vision);
        assert_eq!(after.language, "pl");
        set_skip_vision(&s, "m1", false).unwrap();
        assert!(!stored(&s).skip_vision);
        // A meeting without a job is not an error.
        set_skip_vision(&s, "nope", true).unwrap();
    }

    #[test]
    fn screenshot_analysis_is_on_by_default_and_the_setup_flag_is_not_part_of_a_save() {
        let s = ProcessingSettings::default();
        assert!(s.describe_screenshots);
        assert!(!s.skip_reports && !s.first_run_done);
        let json = serde_json::to_string(&s).unwrap().to_lowercase();
        assert!(!json.contains("token") || json.contains("hftokenset"));
    }

    #[test]
    fn the_worker_folder_comes_from_the_installation_resources_when_it_has_one() {
        let dir = tempfile::Builder::new()
            .prefix("bbb-processing-worker-")
            .tempdir()
            .unwrap();
        let worker = dir.path().join("worker");
        std::fs::create_dir_all(&worker).unwrap();
        std::fs::write(worker.join("pyproject.toml"), "").unwrap();
        // The setting wins; without it the installation's folder is found (the development
        // override and the folders around the executable come after it).
        assert_eq!(
            find_worker_dir("D:/w", Some(dir.path())),
            PathBuf::from("D:/w")
        );
        if std::env::var_os("BBB_WORKER_DIR").is_none() {
            assert_eq!(find_worker_dir("", Some(dir.path())), worker);
        }
    }

    #[test]
    fn the_stage_list_has_the_extraction_stages_and_their_skip_note() {
        let json = serde_json::to_value(StageView {
            stage: Stage::Extract,
            status: StageStatus::Done,
            attempts: 0,
            error: None,
            note: Some("the meeting has no cycle".into()),
            fraction: None,
            message: None,
        })
        .unwrap();
        assert_eq!(json["stage"], "extract");
        assert_eq!(json["note"], "the meeting has no cycle");
        assert!(STAGES.contains(&Stage::Reduce));
    }

    #[test]
    fn llm_settings_travel_with_the_processing_settings_and_old_files_load() {
        let old: ProcessingSettings =
            serde_json::from_str(r#"{"processAfterRecording":"now","gpuIdleSeconds":9}"#).unwrap();
        assert_eq!(old.llm, LlmSettings::default());
        assert_eq!(old.process_after_recording, ProcessAfter::Now);
        let new: ProcessingSettings = serde_json::from_str(
            r#"{"llm":{"defaultKind":"external","external":{"baseUrl":"http://h:1/v1","model":"m"}}}"#,
        )
        .unwrap();
        assert_eq!(new.llm.default_kind, LlmKind::External);
        assert_eq!(new.llm.external.model, "m");
        assert_eq!(new.llm.managed.ctx, 16384);
        let json = serde_json::to_string(&new).unwrap();
        // No key material is ever part of the settings.
        assert!(!json.to_lowercase().contains("apikey"));
    }

    struct FastRunner;
    impl queue::StageRunner for FastRunner {
        fn run(&self, _meeting_id: &str, _stage: Stage) -> Result<(), queue::RunError> {
            Ok(())
        }
    }

    struct AlwaysBusyGate;
    impl GpuGate for AlwaysBusyGate {
        fn check(&self, _stage: Stage) -> GateDecision {
            GateDecision::Wait("GPU busy".into())
        }
    }

    #[test]
    fn gpu_override_auto_resets_when_queue_becomes_idle_and_schedule_force_work() {
        let dir = test_tmp::TestDir::new("proc-queue-ctrl");
        let store = Arc::new(Mutex::new(job_store()));
        let proc = Processing::new_for_test(
            store.clone(),
            dir.path(),
            Arc::new(FastRunner),
            Arc::new(AlwaysBusyGate),
        );

        let ovr = QueueGpuOverrideDto {
            gpu_max_util_percent: 95,
            gpu_idle_seconds: 1,
        };
        let q = proc.set_gpu_override(Some(ovr)).unwrap();
        assert_eq!(q.gpu_override, Some(ovr));
        assert_eq!(q.jobs.len(), 1);
        assert!(q.jobs[0].eta_seconds.is_some());

        // Schedule the job into the future and verify scheduled_unix_ms is surfaced.
        let future = queue::now_ms() + 600_000;
        let scheduled = proc.schedule_job("m1", Some(future)).unwrap();
        assert_eq!(scheduled.scheduled_unix_ms, Some(future));

        // Force process clears the schedule, bypasses the AlwaysBusyGate, and drives to Done.
        let forced = proc.force_process("m1").unwrap();
        assert_eq!(forced.scheduled_unix_ms, None);

        // Wait for the fast background driver to finish the job and drop back to idle.
        for _ in 0..100 {
            let q_now = proc.queue_state().unwrap();
            if q_now.jobs[0].state == JobState::Done && !proc.driver_running.load(Ordering::SeqCst)
            {
                assert_eq!(q_now.jobs[0].progress, 1.0);
                assert_eq!(q_now.jobs[0].eta_seconds, None);
                assert_eq!(
                    q_now.gpu_override, None,
                    "gpu_override must auto-reset when zero active/waiting/scheduled jobs remain"
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("queue driver did not finish forced job in time");
    }

    #[test]
    fn project_prefs_override_llm_kind_and_model_on_job_options() {
        let s = job_store();
        s.set_report_settings(
            "m1",
            &store::ReportSettings {
                cycle_id: Some("NOMAD".into()),
                ..Default::default()
            },
        )
        .unwrap();
        s.set_project_prefs(&store::ProjectPrefs {
            cycle_id: "NOMAD".into(),
            local_save_enabled: false,
            local_save_dir: String::new(),
            llm_kind_override: Some("external".into()),
            llm_model_override: Some("qwen3:14b".into()),
            audio_retention: None,
        })
        .unwrap();

        let mut llm_settings = LlmSettings::default();
        llm_settings.external.base_url = "http://127.0.0.1:11434/v1".into();
        llm_settings.external.model = "default-model".into();

        let mut opts = JobOptions::default();
        apply_project_prefs_to_job_options(&s, "m1", &llm_settings, &mut opts).unwrap();
        let chosen = opts.llm.unwrap();
        assert_eq!(chosen.kind, LlmKind::External);
        assert_eq!(chosen.model.as_deref(), Some("qwen3:14b"));
    }

    #[test]
    fn eta_decreases_as_live_progress_advances() {
        let s = job_store();
        s.set_stage_status("m1", "asr", StageStatus::Running, None, 1_000)
            .unwrap();
        let job = s.get_job("m1").unwrap().unwrap();
        let p0 = queue::progress_with_live(&job, Some((Stage::Asr, 0.0)));
        let p1 = queue::progress_with_live(&job, Some((Stage::Asr, 0.9)));
        assert!(p1 > p0);
        let eta0 =
            estimate_eta_seconds(&job, JobState::Running, p0, Some(600_000), 11_000).unwrap();
        let eta1 =
            estimate_eta_seconds(&job, JobState::Running, p1, Some(600_000), 11_000).unwrap();
        assert!(eta1 < eta0);
        assert_eq!(
            estimate_eta_seconds(&job, JobState::Done, 1.0, Some(600_000), 11_000),
            None
        );
    }

    #[test]
    fn vision_settings_persist_and_skip_screenshot_analysis_skips_vision_stage() {
        let dir = test_tmp::TestDir::new("proc-vision");
        let store = Arc::new(Mutex::new(job_store()));
        {
            let s = store.lock().unwrap();
            for st in [
                Stage::Asr,
                Stage::Diarize,
                Stage::Merge,
                Stage::SpeakerId,
                Stage::Clips,
            ] {
                s.set_stage_status("m1", st.as_str(), StageStatus::Done, None, 1)
                    .unwrap();
            }
        }
        let proc = Processing::new_for_test(
            store.clone(),
            dir.path(),
            Arc::new(FastRunner),
            Arc::new(AlwaysBusyGate),
        );
        let mut s = proc.settings().unwrap();
        s.vision_model_path = dir
            .path()
            .join("qwen-vl.gguf")
            .to_string_lossy()
            .into_owned();
        s.skip_screenshot_analysis = true;
        proc.save_settings(s).unwrap();

        let reloaded = ProcessingSettings::load(&dir.path().join("processing.json"));
        assert!(reloaded.skip_screenshot_analysis);
        assert!(reloaded.vision_model_path.ends_with("qwen-vl.gguf"));
        assert_eq!(
            vision_dir_override(&reloaded.vision_model_path).as_deref(),
            Some(dir.path())
        );

        proc.prepare_step("m1");
        let job = store.lock().unwrap().get_job("m1").unwrap().unwrap();
        let vision_row = job.stages.iter().find(|r| r.stage == "vision").unwrap();
        assert_eq!(vision_row.status, StageStatus::Done);
        assert!(vision_row.note.is_some());
    }

    #[test]
    fn find_worker_dir_release_prefers_resource_dir_and_ignores_env_and_parents() {
        let dir = test_tmp::TestDir::new("proc-worker-release");
        let res = dir.path().join("res");
        let res_worker = res.join("worker");
        std::fs::create_dir_all(&res_worker).unwrap();
        std::fs::write(res_worker.join("pyproject.toml"), "").unwrap();

        let parent_worker = dir.path().join("worker");
        std::fs::create_dir_all(&parent_worker).unwrap();
        std::fs::write(parent_worker.join("pyproject.toml"), "").unwrap();
        let nested_exe = dir.path().join("sub").join("deep").join("app.exe");
        std::fs::create_dir_all(nested_exe.parent().unwrap()).unwrap();
        let env_dir = std::ffi::OsStr::new("D:/env-worker");

        // Release ignores BBB_WORKER_DIR and picks resource_dir/worker.
        assert_eq!(
            find_worker_dir_in("", Some(&res), Some(env_dir), Some(&nested_exe), false),
            res_worker
        );
        // Release without resource_dir ignores parent-folder search and BBB_WORKER_DIR.
        assert_eq!(
            find_worker_dir_in("", None, Some(env_dir), Some(&nested_exe), false),
            PathBuf::from("worker")
        );
        // Release still checks worker/ directly next to the executable.
        let sibling_worker = nested_exe.parent().unwrap().join("worker");
        std::fs::create_dir_all(&sibling_worker).unwrap();
        std::fs::write(sibling_worker.join("pyproject.toml"), "").unwrap();
        assert_eq!(
            find_worker_dir_in("", None, Some(env_dir), Some(&nested_exe), false),
            sibling_worker
        );
    }

    #[test]
    fn find_worker_dir_debug_honours_env_override_and_parent_search() {
        let dir = test_tmp::TestDir::new("proc-worker-debug");
        let parent_worker = dir.path().join("worker");
        std::fs::create_dir_all(&parent_worker).unwrap();
        std::fs::write(parent_worker.join("pyproject.toml"), "").unwrap();
        let nested_exe = dir.path().join("sub").join("deep").join("app.exe");
        std::fs::create_dir_all(nested_exe.parent().unwrap()).unwrap();

        let env_dir = std::ffi::OsStr::new("D:/env-worker");
        assert_eq!(
            find_worker_dir_in("", None, Some(env_dir), Some(&nested_exe), true),
            PathBuf::from("D:/env-worker")
        );
        assert_eq!(
            find_worker_dir_in("", None, None, Some(&nested_exe), true),
            parent_worker
        );
    }

    #[test]
    fn worker_config_uses_bundled_uv_and_local_uv_cache() {
        let dir = test_tmp::TestDir::new("proc-bundled-uv");
        let res = dir.path().join("resources");
        let bin = res.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let uv_name = if cfg!(windows) { "uv.exe" } else { "uv" };
        let bundled_uv = bin.join(uv_name);
        std::fs::write(&bundled_uv, b"MZ fake").unwrap();

        let local = dir.path().join("local");
        let env_dir = env_dir_from(&local, None);
        let cache_dir = uv_cache_dir_for(&local);

        let cfg = worker_config(
            &ProcessingSettings::default(),
            &env_dir,
            &cache_dir,
            Some(&res),
            None,
        );
        assert_eq!(cfg.uv, bundled_uv);
        assert_eq!(cfg.env_dir, local.join("worker-venv"));
        assert_eq!(cfg.uv_cache_dir.as_deref(), Some(cache_dir.as_path()));
    }

    #[test]
    fn uv_cache_dir_lives_under_local_app_data() {
        let local = Path::new("C:/Users/test/AppData/Local/BlahBlahBot");
        assert_eq!(uv_cache_dir_for(local), local.join("uv-cache"));
    }
}
