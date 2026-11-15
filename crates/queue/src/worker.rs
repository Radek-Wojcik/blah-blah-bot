//! Running the Python worker: environment bootstrap with `uv`, one process per stage,
//! progress events from its output, timeout and process-tree kill.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use store::MeetingDetails;

use crate::stage::Stage;
use crate::supervisor::RunError;

/// How to find and run the worker.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// The `uv` executable.
    pub uv: PathBuf,
    /// Folder with the worker's `pyproject.toml` and `uv.lock`.
    pub worker_dir: PathBuf,
    /// Where the Python environment is created (outside the installation folder).
    pub env_dir: PathBuf,
    /// Cache folder for downloaded models; `None` uses the Hugging Face default cache.
    pub models_dir: Option<PathBuf>,
    /// `llama-server` executable for the managed vision model.
    pub llama_server: Option<PathBuf>,
    /// Hugging Face access token for gated models; passed only through the environment.
    pub hf_token: Option<String>,
    /// A stage that runs longer than this is killed and marked failed.
    pub step_timeout: Duration,
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
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            asr_model: "turbo".into(),
            glossary: Vec::new(),
            max_speakers: None,
            vision_base_url: None,
        }
    }
}

impl JobOptions {
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }
}

/// The manifest the worker reads: tracks with their segments, screenshots and options.
pub fn build_manifest(details: &MeetingDetails, options: &JobOptions) -> Value {
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
    json!({
        "version": 1,
        "meeting_id": details.meeting.id,
        "tracks": tracks,
        "screenshots": screenshots,
        "options": options,
    })
}

/// Kill a process and everything it started (the worker may have a `llama-server` child).
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn command(cfg: &WorkerConfig, args: &[&str]) -> Command {
    let mut cmd = Command::new(&cfg.uv);
    cmd.args(args)
        .arg("--project")
        .arg(&cfg.worker_dir)
        .env("UV_PROJECT_ENVIRONMENT", &cfg.env_dir)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = &cfg.models_dir {
        cmd.env("BBB_MODELS_DIR", dir);
    }
    if let Some(p) = &cfg.llama_server {
        cmd.env("BBB_LLAMA_SERVER", p);
    }
    if let Some(t) = &cfg.hf_token {
        cmd.env("HF_TOKEN", t);
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
    mut cmd: Command,
    timeout: Duration,
    mut on_event: impl FnMut(&WorkerEvent),
) -> Result<Option<String>, RunError> {
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
    let tail = err_tail.join().unwrap_or_default();
    if status.success() {
        Ok(None)
    } else {
        Err(RunError::new(match reported_error {
            Some(m) => m,
            None if tail.is_empty() => format!("the worker exited with {status}"),
            None => tail,
        }))
    }
}

/// Create or update the Python environment from the lock file. Needs the network on the
/// first run (several GB). `on_event` receives progress lines of the installer as messages.
pub fn ensure_environment(cfg: &WorkerConfig) -> Result<(), RunError> {
    let mut cmd = Command::new(&cfg.uv);
    cmd.args(["sync", "--frozen", "--project"])
        .arg(&cfg.worker_dir)
        .env("UV_PROJECT_ENVIRONMENT", &cfg.env_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
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

/// Download models (needs the network). `keys` are model keys known to the worker.
pub fn ensure_models(
    cfg: &WorkerConfig,
    keys: &[&str],
    on_event: impl FnMut(&WorkerEvent),
) -> Result<(), RunError> {
    let list = keys.join(",");
    let cmd = command(cfg, &["run", "--frozen"]).tap(|c| {
        c.args(["python", "-m", "bbb_worker", "models", "--ensure", &list]);
    });
    run_process(cmd, Duration::from_secs(6 * 3600), on_event).map(|_| ())
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
    let meeting_dir = meeting_root.join(&details.meeting.dir);
    let work = meeting_dir.join("work");
    std::fs::create_dir_all(&work).map_err(|e| RunError::new(e.to_string()))?;
    let manifest_path = work.join("manifest.json");
    let manifest = build_manifest(details, options);
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| RunError::new(e.to_string()))?;

    let cmd = command(cfg, &["run", "--frozen"]).tap(|c| {
        c.args(["python", "-m", "bbb_worker", step, "--manifest"])
            .arg(&manifest_path)
            .arg("--meeting-dir")
            .arg(&meeting_dir);
    });
    run_process(cmd, cfg.step_timeout, on_event).map(|_| ())
}

/// The worker step that implements a stage, if the stage is not done in Rust.
pub fn step_name(stage: Stage) -> Option<&'static str> {
    match stage {
        Stage::Asr => Some("asr"),
        Stage::Diarize => Some("diarize"),
        Stage::Merge => Some("merge"),
        Stage::Vision => Some("vision"),
        Stage::AudioCleanup | Stage::SpeakerId | Stage::Clips => None,
    }
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
        let m = build_manifest(&details, &JobOptions::default());
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
}
