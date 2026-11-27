//! Running the Python worker: environment bootstrap with `uv`, one process per stage,
//! progress events from its output, timeout and process-tree kill.

use std::ffi::OsString;
use std::fmt;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use store::{LlmChoice, LlmKind, MeetingDetails, MeetingState, Screenshot, Segment, Store, Track};

use crate::stage::Stage;
use crate::supervisor::RunError;

/// A secret that never shows up in logs or debug output. It can be read only through
/// [`Secret::expose`], which is called where the secret is handed to a child process.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Name of the environment variable that carries the API key of an external or cloud
/// language model to the worker. The key is never written to the manifest.
pub const LLM_API_KEY_ENV: &str = "BBB_LLM_API_KEY";

/// How to find and run the worker.
#[derive(Clone)]
pub struct WorkerConfig {
    /// The `uv` executable.
    pub uv: PathBuf,
    /// Folder with the worker's `pyproject.toml` and `uv.lock`.
    pub worker_dir: PathBuf,
    /// Where the Python environment is created (outside the installation folder).
    pub env_dir: PathBuf,
    /// Dedicated `uv` cache folder under Local AppData (never `%TEMP%`).
    pub uv_cache_dir: Option<PathBuf>,
    /// Cache folder for downloaded models; `None` uses the Hugging Face default cache.
    pub models_dir: Option<PathBuf>,
    /// `llama-server` executable for the managed vision model.
    pub llama_server: Option<PathBuf>,
    /// Hugging Face access token for gated models; passed only through the environment.
    pub hf_token: Option<String>,
    /// A stage that runs longer than this is killed and marked failed.
    pub step_timeout: Duration,
    /// API key of the external or cloud language model, read by the application from the
    /// Windows Credential Manager. Passed only through the environment of the `extract` and
    /// `reduce` processes of those backends: not in the manifest, not in any log.
    pub llm_api_key: Option<Secret>,
}

impl fmt::Debug for WorkerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redact = |present: bool| if present { "Some(<redacted>)" } else { "None" };
        f.debug_struct("WorkerConfig")
            .field("uv", &self.uv)
            .field("worker_dir", &self.worker_dir)
            .field("env_dir", &self.env_dir)
            .field("uv_cache_dir", &self.uv_cache_dir)
            .field("models_dir", &self.models_dir)
            .field("llama_server", &self.llama_server)
            .field(
                "hf_token",
                &format_args!("{}", redact(self.hf_token.is_some())),
            )
            .field("step_timeout", &self.step_timeout)
            .field(
                "llm_api_key",
                &format_args!("{}", redact(self.llm_api_key.is_some())),
            )
            .finish()
    }
}

/// Runs a different program in place of `uv run ... python -m bbb_worker`. The step and its
/// arguments follow `args`. For tests that fake the worker with a small script.
#[derive(Debug, Clone)]
pub struct Launcher {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// One line of the worker's output.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WorkerEvent {
    Progress {
        step: String,
        fraction: f64,
        #[serde(default)]
        message: String,
    },
    Done {
        step: String,
    },
    Error {
        step: String,
        message: String,
    },
    Models {
        cached: serde_json::Map<String, Value>,
    },
}

/// Parse one output line; anything that is not a protocol event is ignored.
pub fn parse_event(line: &str) -> Option<WorkerEvent> {
    serde_json::from_str(line.trim()).ok()
}

/// Backend of the `extract` and `reduce` stages: `options.llm` of the manifest
/// (`schemas/job-manifest.schema.json`). It holds no key.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LlmOptions {
    pub kind: LlmKind,
    /// Managed: path to the GGUF file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_path: Option<String>,
    /// Context size in tokens (at least 2048).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx: Option<u32>,
    /// External and cloud: OpenAI-compatible base URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// External and cloud: model name sent in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Cloud runs only when this is true. It is decided from the store (see
    /// [`confirmed_for_run`]), never taken from the job options as stored.
    #[serde(default)]
    pub cloud_confirmed: bool,
}

/// Options a job was created with (stored as JSON in the job row).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct JobOptions {
    /// `auto`, `pl` or `en`.
    pub language: String,
    /// `turbo` or `large-v3`.
    pub asr_model: String,
    pub glossary: Vec<String>,
    pub max_speakers: Option<u32>,
    /// Base URL of an external OpenAI-compatible server for the vision step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vision_base_url: Option<String>,
    /// Backend of `extract` and `reduce`. Without it (and without a cycle on the meeting)
    /// those stages are skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmOptions>,
    /// Leave the screenshots undescribed: the `vision` stage is skipped (the user turned
    /// screenshot analysis off). Written to the manifest only when set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_vision: bool,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            asr_model: "turbo".into(),
            glossary: Vec::new(),
            max_speakers: None,
            vision_base_url: None,
            llm: None,
            skip_vision: false,
        }
    }
}

impl JobOptions {
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }
}

/// Host part of a URL (`https://api.example.com:443/v1` gives `api.example.com`).
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Whether a cloud backend may run: the store holds a confirmation for this meeting that
/// was given for the endpoint in `llm`. Other kinds need no confirmation and give `false`.
pub fn confirmed_for_run(llm: &LlmOptions, choice: Option<&LlmChoice>) -> bool {
    if llm.kind != LlmKind::Cloud {
        return false;
    }
    let Some(c) = choice else {
        return false;
    };
    if c.kind != LlmKind::Cloud || !c.cloud_confirmed {
        return false;
    }
    // A confirmation is for one host: without a recorded host, or without a host in the
    // job's address, nothing is confirmed.
    match (
        c.endpoint_host.as_deref(),
        llm.base_url.as_deref().and_then(url_host),
    ) {
        (Some(confirmed), Some(actual)) => {
            !confirmed.trim().is_empty() && confirmed.eq_ignore_ascii_case(&actual)
        }
        _ => false,
    }
}

/// The options to run a stage with: `cloud_confirmed` is taken from the store, so a value
/// in the stored job options can neither grant nor lose the confirmation.
pub fn with_store_confirmation(options: &JobOptions, choice: Option<&LlmChoice>) -> JobOptions {
    let mut out = options.clone();
    if let Some(llm) = &mut out.llm {
        llm.cloud_confirmed = confirmed_for_run(llm, choice);
    }
    out
}

/// Refuses options that would send a meeting to the cloud without confirmation.
pub fn check_llm_options(options: &JobOptions) -> Result<(), RunError> {
    match &options.llm {
        Some(l) if l.kind == LlmKind::Cloud && !l.cloud_confirmed => Err(RunError::new(
            "the cloud language model was not confirmed for this meeting",
        )),
        _ => Ok(()),
    }
}

/// The manifest the worker reads: tracks with their segments, screenshots and options.
/// Fails for a cloud backend that is not confirmed (see [`check_llm_options`]).
pub fn build_manifest(details: &MeetingDetails, options: &JobOptions) -> Result<Value, RunError> {
    check_llm_options(options)?;
    let tracks: Vec<Value> = details
        .tracks
        .iter()
        .map(|t| {
            let mut segs: Vec<_> = details
                .segments
                .iter()
                .filter(|s| s.track == t.name)
                .collect();
            segs.sort_by_key(|s| s.segment);
            json!({
                "name": t.name,
                "source": t.source,
                "offset_ms": t.offset_ms,
                "segments": segs.iter().map(|s| s.file.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let screenshots: Vec<Value> = details
        .screenshots
        .iter()
        .map(|s| json!({ "file": s.file, "offset_ms": s.offset_ms }))
        .collect();
    Ok(json!({
        "version": 1,
        "meeting_id": details.meeting.id,
        "tracks": tracks,
        "screenshots": screenshots,
        "options": options,
    }))
}

/// Kill a process and everything it started (the worker may have a `llama-server` child).
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn command(cfg: &WorkerConfig, args: &[&str]) -> Command {
    command_with(cfg, None, args)
}

fn command_with(cfg: &WorkerConfig, launcher: Option<&Launcher>, args: &[&str]) -> Command {
    let mut cmd = match launcher {
        Some(l) => {
            let mut c = Command::new(&l.program);
            c.args(&l.args);
            c
        }
        None => {
            let mut c = Command::new(&cfg.uv);
            c.args(args).arg("--project").arg(&cfg.worker_dir);
            c
        }
    };
    cmd.env("UV_PROJECT_ENVIRONMENT", &cfg.env_dir)
        // Only `run_step` hands the key to a process; nothing inherits it by accident.
        .env_remove(LLM_API_KEY_ENV)
        // Only `ensure_models` gets `HF_TOKEN`; inference steps and `llama-server` never see it.
        .env_remove("HF_TOKEN")
        .env_remove("HUGGING_FACE_HUB_TOKEN")
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cache_dir) = &cfg.uv_cache_dir {
        cmd.env("UV_CACHE_DIR", cache_dir);
    }
    if let Some(dir) = &cfg.models_dir {
        cmd.env("BBB_MODELS_DIR", dir);
    }
    if let Some(p) = &cfg.llama_server {
        cmd.env("BBB_LLAMA_SERVER", p);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Run a command, forwarding worker events, and enforce the timeout.
fn run_process(
    cmd: Command,
    timeout: Duration,
    on_event: impl FnMut(&WorkerEvent),
) -> Result<Option<String>, RunError> {
    run_process_cancellable(cmd, timeout, None, on_event).map(|_| None)
}

/// Run a command, forwarding worker events, enforcing `timeout`, and stopping early when
/// `cancel` is set. Returns `Ok(true)` on normal completion and `Ok(false)` when cancelled.
fn run_process_cancellable(
    mut cmd: Command,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    mut on_event: impl FnMut(&WorkerEvent),
) -> Result<bool, RunError> {
    if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
        return Ok(false);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| RunError::new(format!("could not start the worker: {e}")))?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // Keep the tail of stderr for the error message.
    let err_tail = std::thread::spawn(move || {
        let mut tail = std::collections::VecDeque::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tail.len() == 12 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        tail.into_iter().collect::<Vec<_>>().join("\n")
    });

    let started = Instant::now();
    let mut reported_error: Option<String> = None;
    let status = loop {
        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            kill_tree(&mut child);
            return Ok(false);
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                if let Some(ev) = parse_event(&line) {
                    if let WorkerEvent::Error { message, .. } = &ev {
                        reported_error = Some(message.clone());
                    }
                    on_event(&ev);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break child
                    .wait()
                    .map_err(|e| RunError::new(format!("worker wait failed: {e}")))?;
            }
        }
        if started.elapsed() > timeout {
            kill_tree(&mut child);
            return Err(RunError::new(format!(
                "the worker was stopped after {} minutes without finishing",
                timeout.as_secs() / 60
            )));
        }
    };
    if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
        return Ok(false);
    }
    let tail = err_tail.join().unwrap_or_default();
    if status.success() {
        Ok(true)
    } else {
        Err(RunError::new(match reported_error {
            Some(m) => m,
            None if tail.is_empty() => format!("the worker exited with {status}"),
            None => tail,
        }))
    }
}

fn ensure_environment_command(cfg: &WorkerConfig) -> Command {
    let mut cmd = Command::new(&cfg.uv);
    cmd.args(["sync", "--frozen", "--project"])
        .arg(&cfg.worker_dir)
        .env("UV_PROJECT_ENVIRONMENT", &cfg.env_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(cache_dir) = &cfg.uv_cache_dir {
        cmd.env("UV_CACHE_DIR", cache_dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Create or update the Python environment from the lock file. Needs the network on the
/// first run (several GB). `on_event` receives progress lines of the installer as messages.
pub fn ensure_environment(cfg: &WorkerConfig) -> Result<(), RunError> {
    let mut cmd = ensure_environment_command(cfg);
    let out = cmd
        .output()
        .map_err(|e| RunError::new(format!("could not run uv ({}): {e}", cfg.uv.display())))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(8).collect();
        Err(RunError::new(format!(
            "creating the Python environment failed: {}",
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        )))
    }
}

/// Cancellable variant of [`ensure_environment`]. Returns `Ok(true)` on success and
/// `Ok(false)` when stopped via `cancel`.
pub fn ensure_environment_cancellable(
    cfg: &WorkerConfig,
    cancel: &AtomicBool,
) -> Result<bool, RunError> {
    if cancel.load(Ordering::SeqCst) {
        return Ok(false);
    }
    let mut cmd = ensure_environment_command(cfg);
    let mut child = cmd
        .spawn()
        .map_err(|e| RunError::new(format!("could not run uv ({}): {e}", cfg.uv.display())))?;
    let stderr = child.stderr.take();
    let err_tail = std::thread::spawn(move || {
        let mut tail = std::collections::VecDeque::new();
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tail.len() == 8 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        }
        tail.into_iter().collect::<Vec<_>>().join(" | ")
    });
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            kill_tree(&mut child);
            return Ok(false);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                kill_tree(&mut child);
                return Err(RunError::new(format!("waiting for uv failed: {e}")));
            }
        }
    };
    if cancel.load(Ordering::SeqCst) {
        return Ok(false);
    }
    let tail = err_tail.join().unwrap_or_default();
    if status.success() {
        Ok(true)
    } else {
        Err(RunError::new(format!(
            "creating the Python environment failed: {tail}"
        )))
    }
}

fn ensure_models_command(cfg: &WorkerConfig, keys: &[&str]) -> Command {
    let list = keys.join(",");
    command(cfg, &["run", "--frozen"]).tap(|c| {
        c.args(["python", "-m", "bbb_worker", "models", "--ensure", &list]);
        if let Some(t) = &cfg.hf_token {
            c.env("HF_TOKEN", t);
        }
    })
}

/// Download models (needs the network). `keys` are model keys known to the worker.
pub fn ensure_models(
    cfg: &WorkerConfig,
    keys: &[&str],
    on_event: impl FnMut(&WorkerEvent),
) -> Result<(), RunError> {
    let cmd = ensure_models_command(cfg, keys);
    run_process(cmd, Duration::from_secs(6 * 3600), on_event).map(|_| ())
}

/// Cancellable variant of [`ensure_models`]. Returns `Ok(true)` on success and `Ok(false)`
/// when stopped via `cancel`.
pub fn ensure_models_cancellable(
    cfg: &WorkerConfig,
    keys: &[&str],
    cancel: &AtomicBool,
    on_event: impl FnMut(&WorkerEvent),
) -> Result<bool, RunError> {
    let cmd = ensure_models_command(cfg, keys);
    run_process_cancellable(cmd, Duration::from_secs(6 * 3600), Some(cancel), on_event)
}

/// Which models are cached: `(key, cached)` pairs.
pub fn model_status(cfg: &WorkerConfig) -> Result<Vec<(String, bool)>, RunError> {
    let cmd = command(cfg, &["run", "--frozen"]).tap(|c| {
        c.args(["python", "-m", "bbb_worker", "models", "--status"]);
    });
    let mut out = Vec::new();
    run_process(cmd, Duration::from_secs(120), |ev| {
        if let WorkerEvent::Models { cached } = ev {
            out = cached
                .iter()
                .map(|(k, v)| (k.clone(), v.as_bool().unwrap_or(false)))
                .collect();
        }
    })?;
    Ok(out)
}

trait Tap: Sized {
    fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }
}
impl Tap for Command {}

/// Write the manifest and run one worker step for a meeting. The caller reads `details`
/// from the store beforehand and must not hold any store lock while this runs.
pub fn run_step(
    cfg: &WorkerConfig,
    details: &MeetingDetails,
    meeting_root: &Path,
    options: &JobOptions,
    step: &str,
    on_event: impl FnMut(&WorkerEvent),
) -> Result<(), RunError> {
    run_step_with(None, cfg, details, meeting_root, options, step, on_event)
}

/// [`run_step`] with an optional replacement for the `uv` command line.
///
/// The worker is not started when the options would send the meeting to the cloud without
/// confirmation. The API key, when there is one, goes into the environment of the
/// `extract` and `reduce` steps of an external or cloud backend and nowhere else.
fn step_command(
    launcher: Option<&Launcher>,
    cfg: &WorkerConfig,
    manifest_path: &Path,
    meeting_dir: &Path,
    options: &JobOptions,
    step: &str,
) -> Command {
    let mut cmd = command_with(cfg, launcher, &["run", "--frozen"]).tap(|c| {
        if launcher.is_none() {
            c.args(["python", "-m", "bbb_worker"]);
        }
        c.arg(step)
            .arg("--manifest")
            .arg(manifest_path)
            .arg("--meeting-dir")
            .arg(meeting_dir);
    });
    let remote = options
        .llm
        .as_ref()
        .is_some_and(|l| l.kind != LlmKind::Managed);
    if remote && matches!(step, "extract" | "reduce") {
        if let Some(key) = &cfg.llm_api_key {
            cmd.env(LLM_API_KEY_ENV, key.expose());
        }
    }
    cmd
}

pub fn run_step_with(
    launcher: Option<&Launcher>,
    cfg: &WorkerConfig,
    details: &MeetingDetails,
    meeting_root: &Path,
    options: &JobOptions,
    step: &str,
    on_event: impl FnMut(&WorkerEvent),
) -> Result<(), RunError> {
    let manifest = build_manifest(details, options)?;
    let meeting_dir = meeting_root.join(&details.meeting.dir);
    let work = meeting_dir.join("work");
    std::fs::create_dir_all(&work).map_err(|e| RunError::new(e.to_string()))?;
    let manifest_path = work.join("manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| RunError::new(e.to_string()))?;

    let cmd = step_command(launcher, cfg, &manifest_path, &meeting_dir, options, step);
    run_process(cmd, cfg.step_timeout, on_event).map(|_| ())
}

/// The worker step that implements a stage, if the stage is not done in Rust.
pub fn step_name(stage: Stage) -> Option<&'static str> {
    match stage {
        Stage::Asr => Some("asr"),
        Stage::Diarize => Some("diarize"),
        Stage::Merge => Some("merge"),
        Stage::Vision => Some("vision"),
        Stage::Extract => Some("extract"),
        Stage::Reduce => Some("reduce"),
        Stage::AudioCleanup | Stage::SpeakerId | Stage::Clips => None,
    }
}

// ----- media ingest and manual voice enrollment -----------------------------------------------

/// Mode for media file ingest (`"audio_only"` or `"audio_and_slides"`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestMode {
    AudioOnly,
    #[default]
    AudioAndSlides,
}

impl IngestMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AudioOnly => "audio_only",
            Self::AudioAndSlides => "audio_and_slides",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "audio_only" => Some(Self::AudioOnly),
            "audio_and_slides" => Some(Self::AudioAndSlides),
            _ => None,
        }
    }
}

/// Request to ingest an external audio or video file into a meeting directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct IngestRequest {
    pub meeting_id: String,
    pub input_path: PathBuf,
    pub meeting_dir: PathBuf,
    pub mode: IngestMode,
}

/// One track entry produced by `ingest` in `work/ingest.json`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct IngestTrack {
    pub name: String,
    pub source: String,
    pub offset_ms: f64,
    #[serde(default)]
    pub segments: Vec<String>,
}

fn default_ingest_track() -> String {
    "imported".into()
}

/// One 60 s WAV segment entry produced by `ingest` in `work/ingest.json`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct IngestSegment {
    #[serde(default = "default_ingest_track")]
    pub track: String,
    #[serde(default)]
    pub segment: u32,
    pub file: String,
    pub offset_ms: f64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub samples: u64,
}

impl IngestSegment {
    /// Sample count at 16 kHz (`samples` if reported, else `duration_ms * 16`).
    pub fn sample_count(&self) -> u64 {
        if self.samples > 0 {
            self.samples
        } else {
            self.duration_ms.saturating_mul(16)
        }
    }
}

/// One extracted slide screenshot entry produced by `ingest` in `work/ingest.json`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct IngestScreenshot {
    pub file: String,
    pub offset_ms: f64,
}

/// Parsed contents of `<meeting_dir>/work/ingest.json` after the worker finishes `ingest`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct IngestResponse {
    #[serde(default)]
    pub meeting_id: String,
    #[serde(default)]
    pub duration_ms: i64,
    #[serde(default)]
    pub tracks: Vec<IngestTrack>,
    #[serde(default)]
    pub segments: Vec<IngestSegment>,
    #[serde(default)]
    pub screenshots: Vec<IngestScreenshot>,
}

/// Run the worker's `"ingest"` command for an imported audio or video file.
pub fn run_ingest(
    cfg: &WorkerConfig,
    req: &IngestRequest,
    on_event: impl FnMut(&WorkerEvent),
) -> Result<IngestResponse, RunError> {
    run_ingest_with(None, cfg, req, on_event)
}

/// [`run_ingest`] with an optional replacement launcher for tests.
pub fn run_ingest_with(
    launcher: Option<&Launcher>,
    cfg: &WorkerConfig,
    req: &IngestRequest,
    on_event: impl FnMut(&WorkerEvent),
) -> Result<IngestResponse, RunError> {
    let work = req.meeting_dir.join("work");
    std::fs::create_dir_all(&work).map_err(|e| RunError::new(e.to_string()))?;
    let manifest_path = work.join("ingest-manifest.json");
    let manifest = json!({
        "version": 1,
        "meeting_id": req.meeting_id,
        "tracks": [],
        "screenshots": [],
        "options": {
            "ingest": {
                "input_path": req.input_path,
                "mode": req.mode.as_str(),
            }
        }
    });
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| RunError::new(e.to_string()))?;

    let cmd = command_with(cfg, launcher, &["run", "--frozen"]).tap(|c| {
        if launcher.is_none() {
            c.args(["python", "-m", "bbb_worker"]);
        }
        c.arg("ingest")
            .arg("--manifest")
            .arg(&manifest_path)
            .arg("--meeting-dir")
            .arg(&req.meeting_dir)
            .arg("--input-path")
            .arg(&req.input_path)
            .arg("--mode")
            .arg(req.mode.as_str());
    });
    run_process(cmd, cfg.step_timeout, on_event)?;

    let out_path = work.join("ingest.json");
    let bytes = std::fs::read(&out_path)
        .map_err(|e| RunError::new(format!("could not read {}: {e}", out_path.display())))?;
    let mut parsed: IngestResponse = serde_json::from_slice(&bytes)
        .map_err(|e| RunError::new(format!("invalid ingest.json: {e}")))?;
    if parsed.meeting_id.is_empty() {
        parsed.meeting_id = req.meeting_id.clone();
    }
    for (idx, seg) in parsed.segments.iter_mut().enumerate() {
        if seg.segment == 0 && idx > 0 {
            seg.segment = idx as u32;
        }
    }
    Ok(parsed)
}

/// Record the tracks, segments, screenshots and duration from [`IngestResponse`] in the store.
pub fn record_ingest(
    store: &Store,
    meeting_id: &str,
    resp: &IngestResponse,
) -> Result<(), store::StoreError> {
    let total_samples: u64 = resp.segments.iter().map(IngestSegment::sample_count).sum();
    if resp.tracks.is_empty() {
        store.upsert_track(
            meeting_id,
            &Track {
                name: "imported".into(),
                source: "file:imported".into(),
                pid: None,
                offset_ms: 0.0,
                ended: true,
                end_offset_ms: Some(resp.duration_ms as f64),
                total_samples: Some(total_samples),
            },
        )?;
    } else {
        for t in &resp.tracks {
            let track_samples: u64 = resp
                .segments
                .iter()
                .filter(|s| s.track == t.name)
                .map(IngestSegment::sample_count)
                .sum();
            store.upsert_track(
                meeting_id,
                &Track {
                    name: t.name.clone(),
                    source: t.source.clone(),
                    pid: None,
                    offset_ms: t.offset_ms,
                    ended: true,
                    end_offset_ms: Some(resp.duration_ms as f64),
                    total_samples: Some(track_samples),
                },
            )?;
        }
    }
    for (i, s) in resp.segments.iter().enumerate() {
        store.add_segment(
            meeting_id,
            &Segment {
                track: s.track.clone(),
                segment: if s.segment == 0 && i > 0 {
                    i as u32
                } else {
                    s.segment
                },
                file: s.file.clone(),
                offset_ms: s.offset_ms,
                samples: s.sample_count(),
            },
        )?;
    }
    for sc in &resp.screenshots {
        store.add_screenshot(
            meeting_id,
            &Screenshot {
                file: sc.file.clone(),
                offset_ms: sc.offset_ms,
            },
        )?;
    }
    store.set_meeting_state(meeting_id, MeetingState::Stopped, Some(resp.duration_ms))?;
    Ok(())
}

/// Request to enroll a speaker profile from a 15-30 s user-supplied audio/video sample.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct EnrollRequest {
    pub input_path: PathBuf,
    pub clip_out_path: PathBuf,
    pub output_path: PathBuf,
}

/// Result written by the worker's `enroll` command to `output_path`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct EnrollResponse {
    pub embedding: Vec<f32>,
    pub duration_ms: u64,
    pub clip_file: String,
}

/// Run the worker's `"enroll"` command to extract a preview clip and 256-dim speaker embedding.
pub fn run_enroll(
    cfg: &WorkerConfig,
    req: &EnrollRequest,
    on_event: impl FnMut(&WorkerEvent),
) -> Result<EnrollResponse, RunError> {
    run_enroll_with(None, cfg, req, on_event)
}

/// [`run_enroll`] with an optional replacement launcher for tests.
pub fn run_enroll_with(
    launcher: Option<&Launcher>,
    cfg: &WorkerConfig,
    req: &EnrollRequest,
    on_event: impl FnMut(&WorkerEvent),
) -> Result<EnrollResponse, RunError> {
    if let Some(parent) = req.clip_out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RunError::new(e.to_string()))?;
    }
    if let Some(parent) = req.output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RunError::new(e.to_string()))?;
    }
    let work_dir = req
        .output_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let manifest_path = work_dir.join("enroll-manifest.json");
    let manifest = json!({
        "version": 1,
        "meeting_id": "enroll",
        "tracks": [],
        "screenshots": [],
        "options": {
            "enroll": {
                "input_path": req.input_path,
                "clip_out_path": req.clip_out_path,
                "output_path": req.output_path,
            }
        }
    });
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| RunError::new(e.to_string()))?;

    let cmd = command_with(cfg, launcher, &["run", "--frozen"]).tap(|c| {
        if launcher.is_none() {
            c.args(["python", "-m", "bbb_worker"]);
        }
        c.arg("enroll")
            .arg("--manifest")
            .arg(&manifest_path)
            .arg("--meeting-dir")
            .arg(&work_dir)
            .arg("--input-path")
            .arg(&req.input_path)
            .arg("--clip-out-path")
            .arg(&req.clip_out_path)
            .arg("--output-path")
            .arg(&req.output_path);
    });
    run_process(cmd, cfg.step_timeout, on_event)?;

    let bytes = std::fs::read(&req.output_path)
        .map_err(|e| RunError::new(format!("could not read {}: {e}", req.output_path.display())))?;
    let parsed: EnrollResponse = serde_json::from_slice(&bytes)
        .map_err(|e| RunError::new(format!("invalid enroll output JSON: {e}")))?;
    if parsed.embedding.is_empty() || !parsed.embedding.iter().all(|x| x.is_finite()) {
        return Err(RunError::new(
            "enrollment produced an empty or non-finite speaker embedding",
        ));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::{Meeting, MeetingState, Screenshot, Segment, Track};

    #[test]
    fn parses_protocol_lines_and_ignores_noise() {
        assert_eq!(
            parse_event(r#"{"event":"progress","step":"asr","fraction":0.5,"message":"x"}"#),
            Some(WorkerEvent::Progress {
                step: "asr".into(),
                fraction: 0.5,
                message: "x".into()
            })
        );
        assert_eq!(
            parse_event(r#"{"event":"done","step":"asr"}"#),
            Some(WorkerEvent::Done { step: "asr".into() })
        );
        assert!(parse_event("Loading model...").is_none());
        assert!(parse_event(r#"{"event":"unknown"}"#).is_none());
    }

    #[test]
    fn options_default_when_missing_or_invalid() {
        assert_eq!(JobOptions::from_json("{}"), JobOptions::default());
        assert_eq!(JobOptions::from_json("not json"), JobOptions::default());
        let o = JobOptions::from_json(r#"{"language":"pl","max_speakers":4}"#);
        assert_eq!(o.language, "pl");
        assert_eq!(o.max_speakers, Some(4));
        assert_eq!(o.asr_model, "turbo");
    }

    #[test]
    fn manifest_lists_segments_in_order_per_track() {
        let details = MeetingDetails {
            meeting: Meeting {
                id: "m".into(),
                title: "t".into(),
                started_unix_ms: 0,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: "m".into(),
            },
            tracks: vec![Track {
                name: "mic".into(),
                source: "microphone".into(),
                pid: None,
                offset_ms: 0.0,
                ended: true,
                end_offset_ms: None,
                total_samples: None,
            }],
            segments: vec![
                Segment {
                    track: "mic".into(),
                    segment: 1,
                    file: "mic/000001.wav".into(),
                    offset_ms: 60000.0,
                    samples: 1,
                },
                Segment {
                    track: "mic".into(),
                    segment: 0,
                    file: "mic/000000.wav".into(),
                    offset_ms: 0.0,
                    samples: 1,
                },
            ],
            screenshots: vec![Screenshot {
                file: "screenshots/1.png".into(),
                offset_ms: 12.0,
            }],
        };
        let m = build_manifest(&details, &JobOptions::default()).unwrap();
        assert_eq!(
            m["tracks"][0]["segments"],
            json!(["mic/000000.wav", "mic/000001.wav"])
        );
        assert_eq!(m["screenshots"][0]["file"], "screenshots/1.png");
        assert_eq!(m["options"]["asr_model"], "turbo");
    }

    #[test]
    fn stages_map_to_worker_steps() {
        assert_eq!(step_name(Stage::Asr), Some("asr"));
        assert_eq!(step_name(Stage::AudioCleanup), None);
    }

    fn details() -> MeetingDetails {
        MeetingDetails {
            meeting: Meeting {
                id: "m".into(),
                title: "t".into(),
                started_unix_ms: 0,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: "m".into(),
            },
            tracks: vec![],
            segments: vec![],
            screenshots: vec![],
        }
    }

    fn llm(kind: LlmKind, confirmed: bool) -> JobOptions {
        JobOptions {
            llm: Some(LlmOptions {
                kind,
                model_path: Some("C:/models/gemma.gguf".into()),
                ctx: Some(16384),
                base_url: Some("https://api.example.com/v1".into()),
                model: Some("some-model".into()),
                cloud_confirmed: confirmed,
            }),
            ..JobOptions::default()
        }
    }

    fn manifest_validator() -> jsonschema::Validator {
        let schema: Value =
            serde_json::from_str(include_str!("../../../schemas/job-manifest.schema.json"))
                .unwrap();
        jsonschema::validator_for(&schema).unwrap()
    }

    #[test]
    fn the_manifest_carries_the_llm_options_as_the_schema_wants_them() {
        let v = manifest_validator();
        for kind in [LlmKind::Managed, LlmKind::External] {
            let m = build_manifest(&details(), &llm(kind, false)).unwrap();
            assert!(v.is_valid(&m), "{kind:?}: {m}");
            assert_eq!(m["options"]["llm"]["kind"], kind.as_str());
            assert_eq!(m["options"]["llm"]["ctx"], 16384);
            assert_eq!(m["options"]["llm"]["model_path"], "C:/models/gemma.gguf");
        }
        let cloud = build_manifest(&details(), &llm(LlmKind::Cloud, true)).unwrap();
        assert!(v.is_valid(&cloud), "{cloud}");
        assert_eq!(cloud["options"]["llm"]["cloud_confirmed"], true);
        // Without a backend the key is absent, so an old job's manifest is unchanged.
        let plain = build_manifest(&details(), &JobOptions::default()).unwrap();
        assert!(v.is_valid(&plain));
        assert!(plain["options"].get("llm").is_none());
        // Optional fields are left out rather than written as null.
        let bare = JobOptions {
            llm: Some(LlmOptions {
                kind: LlmKind::Managed,
                model_path: None,
                ctx: None,
                base_url: None,
                model: None,
                cloud_confirmed: false,
            }),
            ..JobOptions::default()
        };
        let m = build_manifest(&details(), &bare).unwrap();
        assert!(v.is_valid(&m), "{m}");
        assert_eq!(
            m["options"]["llm"],
            json!({"kind": "managed", "cloud_confirmed": false})
        );
    }

    #[test]
    fn no_manifest_is_built_for_an_unconfirmed_cloud() {
        let e = build_manifest(&details(), &llm(LlmKind::Cloud, false)).unwrap_err();
        assert!(e.message.contains("not confirmed"), "{e}");
        assert!(check_llm_options(&llm(LlmKind::Cloud, true)).is_ok());
        assert!(check_llm_options(&llm(LlmKind::External, false)).is_ok());
    }

    #[test]
    fn the_confirmation_comes_from_the_store_for_the_endpoint_that_was_confirmed() {
        let choice = |kind, confirmed, host: Option<&str>| LlmChoice {
            kind,
            cloud_confirmed: confirmed,
            confirmed_unix_ms: confirmed.then_some(1),
            endpoint_host: host.map(str::to_string),
        };
        let granted = |options: &JobOptions, choice: Option<&LlmChoice>| {
            with_store_confirmation(options, choice)
                .llm
                .unwrap()
                .cloud_confirmed
        };
        let stored_claim = llm(LlmKind::Cloud, true);
        // The claim in the job options means nothing without the store's record.
        assert!(!granted(&stored_claim, None));
        let unconfirmed = choice(LlmKind::Cloud, false, Some("api.example.com"));
        assert!(!granted(&stored_claim, Some(&unconfirmed)));
        // Confirmed for the endpoint of the job: granted, even if the options said false.
        let ok = choice(LlmKind::Cloud, true, Some("API.example.com"));
        assert!(granted(&llm(LlmKind::Cloud, false), Some(&ok)));
        // Confirmed for another host, or the choice is another kind: refused.
        let other = choice(LlmKind::Cloud, true, Some("evil.example.net"));
        assert!(!granted(&stored_claim, Some(&other)));
        let ext = choice(LlmKind::External, true, Some("api.example.com"));
        assert!(!granted(&stored_claim, Some(&ext)));
        // A confirmation that records no host (or an empty one) confirms nothing.
        for host in [None, Some(""), Some("  ")] {
            let hostless = choice(LlmKind::Cloud, true, host);
            assert!(!granted(&stored_claim, Some(&hostless)), "{host:?}");
        }
        // Neither does a job whose address has no host.
        let mut no_host = llm(LlmKind::Cloud, true);
        no_host.llm.as_mut().unwrap().base_url = Some("https:///v1".into());
        assert!(!granted(&no_host, Some(&ok)));
        // Only the cloud kind has a confirmation.
        assert!(!granted(&llm(LlmKind::External, true), Some(&ok)));
        // No options, nothing to decide.
        assert_eq!(
            with_store_confirmation(&JobOptions::default(), Some(&ok)),
            JobOptions::default()
        );
    }

    #[test]
    fn hosts_are_read_from_urls() {
        let host = |u: &str| url_host(u);
        assert_eq!(
            host("https://API.example.com:8443/v1").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(
            host("http://127.0.0.1:11434/v1").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(host("http://[::1]:8080/v1").as_deref(), Some("::1"));
        assert_eq!(
            host("https://user:pw@host.example/x").as_deref(),
            Some("host.example")
        );
        assert_eq!(host("localhost:1234").as_deref(), Some("localhost"));
        assert_eq!(host("https:///nohost"), None);
    }

    #[test]
    fn the_api_key_never_appears_in_debug_output_or_the_manifest() {
        let key = "placeholder-not-a-key";
        let cfg = WorkerConfig {
            uv: "uv".into(),
            worker_dir: "worker".into(),
            env_dir: "env".into(),
            uv_cache_dir: None,
            models_dir: None,
            llama_server: None,
            hf_token: Some("hf_SECRETTOKEN".into()),
            step_timeout: Duration::from_secs(1),
            llm_api_key: Some(Secret::new(key)),
        };
        for text in [
            format!("{cfg:?}"),
            format!("{cfg:#?}"),
            format!("{:?}", cfg.llm_api_key),
            format!("{:#?}", Some(&cfg)),
        ] {
            assert!(!text.contains(key), "{text}");
            assert!(!text.contains("hf_SECRETTOKEN"), "{text}");
            assert!(text.contains("redacted"), "{text}");
        }
        assert_eq!(cfg.llm_api_key.as_ref().unwrap().expose(), key);
        let m = build_manifest(&details(), &llm(LlmKind::External, false)).unwrap();
        assert!(!m.to_string().contains(key));
        let options = serde_json::to_string(&llm(LlmKind::Cloud, true)).unwrap();
        assert!(!options.contains(key));
        assert!(!format!("{:?}", llm(LlmKind::Cloud, true)).contains(key));
    }

    #[test]
    fn options_with_a_backend_survive_the_job_row() {
        let o = llm(LlmKind::External, false);
        let back = JobOptions::from_json(&serde_json::to_string(&o).unwrap());
        assert_eq!(back, o);
        // A job row from before the backend existed reads as no backend.
        assert_eq!(JobOptions::from_json(r#"{"language":"pl"}"#).llm, None);
    }

    #[test]
    fn language_model_stages_map_to_worker_steps() {
        assert_eq!(step_name(Stage::Extract), Some("extract"));
        assert_eq!(step_name(Stage::Reduce), Some("reduce"));
    }

    #[test]
    fn hf_token_is_passed_only_to_model_downloads() {
        use std::ffi::OsStr;
        let cfg = WorkerConfig {
            uv: "uv".into(),
            uv_cache_dir: None,
            worker_dir: "worker".into(),
            env_dir: "env".into(),
            models_dir: None,
            llama_server: None,
            hf_token: Some("hf_SECRETTOKEN".into()),
            step_timeout: Duration::from_secs(1),
            llm_api_key: None,
        };
        let env_of = |cmd: &Command, key: &str| -> Option<Option<String>> {
            cmd.get_envs()
                .find(|(k, _)| *k == OsStr::new(key))
                .map(|(_, v)| v.map(|s| s.to_string_lossy().into_owned()))
        };

        let models_cmd = ensure_models_command(&cfg, &["diarization"]);
        assert_eq!(
            env_of(&models_cmd, "HF_TOKEN"),
            Some(Some("hf_SECRETTOKEN".to_string()))
        );

        let opts = JobOptions::default();
        for step in ["asr", "diarize", "vision", "extract", "reduce"] {
            let cmd = step_command(
                None,
                &cfg,
                Path::new("m.json"),
                Path::new("dir"),
                &opts,
                step,
            );
            assert_eq!(
                env_of(&cmd, "HF_TOKEN"),
                Some(None),
                "step {step} must strip HF_TOKEN"
            );
            assert_eq!(
                env_of(&cmd, "HUGGING_FACE_HUB_TOKEN"),
                Some(None),
                "step {step} must strip HUGGING_FACE_HUB_TOKEN"
            );
        }
    }

    #[test]
    fn ingest_mode_and_response_round_trip_and_record_in_store() {
        assert_eq!(IngestMode::AudioOnly.as_str(), "audio_only");
        assert_eq!(IngestMode::AudioAndSlides.as_str(), "audio_and_slides");
        assert_eq!(IngestMode::parse("audio_only"), Some(IngestMode::AudioOnly));
        assert_eq!(
            IngestMode::parse("audio_and_slides"),
            Some(IngestMode::AudioAndSlides)
        );
        assert_eq!(IngestMode::parse("unknown"), None);

        let resp: IngestResponse = serde_json::from_value(json!({
            "meeting_id": "m-imp",
            "duration_ms": 75_000,
            "tracks": [{
                "name": "imported",
                "source": "file:demo.mp4",
                "offset_ms": 0.0,
                "segments": ["tracks/imported/000000.wav", "tracks/imported/000001.wav"]
            }],
            "segments": [
                {
                    "track": "imported",
                    "segment": 0,
                    "file": "tracks/imported/000000.wav",
                    "offset_ms": 0.0,
                    "duration_ms": 60_000,
                    "samples": 960_000
                },
                {
                    "track": "imported",
                    "segment": 1,
                    "file": "tracks/imported/000001.wav",
                    "offset_ms": 60_000.0,
                    "duration_ms": 15_000
                }
            ],
            "screenshots": [
                {"file": "img/slide-0001-0.png", "offset_ms": 0.0},
                {"file": "img/slide-0002-12000.png", "offset_ms": 12_000.0}
            ]
        }))
        .unwrap();
        assert_eq!(resp.segments[1].sample_count(), 240_000);

        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&Meeting {
            id: "m-imp".into(),
            title: "Imported".into(),
            started_unix_ms: 100,
            duration_ms: None,
            state: MeetingState::Recording,
            dir: "m-imp".into(),
        })
        .unwrap();
        record_ingest(&s, "m-imp", &resp).unwrap();
        let d = s.get_meeting("m-imp").unwrap().unwrap();
        assert_eq!(d.meeting.state, MeetingState::Stopped);
        assert_eq!(d.meeting.duration_ms, Some(75_000));
        assert_eq!(d.tracks.len(), 1);
        assert_eq!(d.segments.len(), 2);
        assert_eq!(d.screenshots.len(), 2);
    }

    #[test]
    fn uv_cache_dir_is_passed_to_uv_commands() {
        use std::ffi::OsStr;
        let cache = PathBuf::from("C:/Users/test/AppData/Local/BlahBlahBot/uv-cache");
        let cfg = WorkerConfig {
            uv: "C:/app/bin/uv.exe".into(),
            worker_dir: "C:/app/worker".into(),
            env_dir: "C:/Users/test/AppData/Local/BlahBlahBot/worker-venv".into(),
            uv_cache_dir: Some(cache.clone()),
            models_dir: None,
            llama_server: None,
            hf_token: None,
            step_timeout: Duration::from_secs(1),
            llm_api_key: None,
        };
        let env_of = |cmd: &Command, key: &str| -> Option<Option<String>> {
            cmd.get_envs()
                .find(|(k, _)| *k == OsStr::new(key))
                .map(|(_, v)| v.map(|s| s.to_string_lossy().into_owned()))
        };
        let expected = Some(Some(cache.to_string_lossy().into_owned()));

        let sync_cmd = ensure_environment_command(&cfg);
        assert_eq!(env_of(&sync_cmd, "UV_CACHE_DIR"), expected);

        let models_cmd = ensure_models_command(&cfg, &["whisper-turbo"]);
        assert_eq!(env_of(&models_cmd, "UV_CACHE_DIR"), expected);

        let step_cmd = step_command(
            None,
            &cfg,
            Path::new("m.json"),
            Path::new("dir"),
            &JobOptions::default(),
            "asr",
        );
        assert_eq!(env_of(&step_cmd, "UV_CACHE_DIR"), expected);
    }
}
