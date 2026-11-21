//! Integration tests of the whole pipeline with the real worker, real models and a real GPU.
//!
//! They are ignored by default and run locally before a release:
//!
//! ```text
//! BBB_MINI_DIR=<folder with mini-en> BBB_SLIDES_DIR=<slides> BBB_VISION_DIR=<gguf folder> \
//! BBB_LLAMA_SERVER=<llama-server.exe> cargo test -p queue --test pipeline_models -- --ignored --test-threads=1
//! ```
//!
//! The recordings are synthetic (scripted and synthesised speech) and not part of the repository.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use queue::cleanup::AudioRetention;
use queue::gpu::{GpuSampler, GuardPolicy, NvmlGate, NvmlSampler};
use queue::runner::{PipelineRunner, RunnerSettings};
use queue::worker::{JobOptions, WorkerConfig};
use queue::{GateDecision, GpuGate, Stage, StepOutcome, Supervisor};
use store::{Meeting, MeetingState, Screenshot, Segment, StageStatus, Store, Track};

const SEGMENT_SAMPLES: usize = 16_000 * 60;

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set")))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// PCM data of a 16 kHz mono 16-bit WAV file.
fn read_pcm(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap();
    let pos = bytes
        .windows(4)
        .position(|w| w == b"data")
        .expect("data chunk");
    let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
    bytes[pos + 8..pos + 8 + len].to_vec()
}

fn write_wav(path: &Path, pcm: &[u8]) {
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, out).unwrap();
}

struct Fixture {
    root: PathBuf,
    store: Arc<Mutex<Store>>,
    /// Removes the work folder when the fixture is dropped; keep it the last field.
    _guard: tempfile::TempDir,
}

/// A stopped meeting recorded the way the recorder writes it: segment files per track.
fn fixture(name: &str, with_screenshot: bool) -> Fixture {
    let mini = env_path("BBB_MINI_DIR").join("mini-en");
    let guard = tempfile::Builder::new()
        .prefix(&format!("bbb-it-{name}-"))
        .tempdir()
        .unwrap();
    let root = guard.path().to_path_buf();
    let store = Store::open_in_memory().unwrap();
    store
        .create_meeting(&Meeting {
            id: "m1".into(),
            title: "Reference meeting".into(),
            started_unix_ms: 1,
            duration_ms: Some(60_000),
            state: MeetingState::Stopped,
            dir: "m1".into(),
        })
        .unwrap();
    for (track, source, wav) in [
        ("mic", "microphone", "track-magda.wav"),
        ("meet", "app:meet.exe", "mixed.wav"),
    ] {
        store
            .upsert_track(
                "m1",
                &Track {
                    name: track.into(),
                    source: source.into(),
                    pid: None,
                    offset_ms: 0.0,
                    ended: true,
                    end_offset_ms: None,
                    total_samples: None,
                },
            )
            .unwrap();
        let pcm = read_pcm(&mini.join(wav));
        for (i, chunk) in pcm.chunks(SEGMENT_SAMPLES * 2).enumerate() {
            let file = format!("{track}/{i:06}.wav");
            write_wav(&root.join("m1").join(&file), chunk);
            store
                .add_segment(
                    "m1",
                    &Segment {
                        track: track.into(),
                        segment: i as u32,
                        file,
                        offset_ms: (i * 60_000) as f64,
                        samples: (chunk.len() / 2) as u64,
                    },
                )
                .unwrap();
        }
    }
    if with_screenshot {
        let slide = env_path("BBB_SLIDES_DIR").join("bullets-1.png");
        std::fs::create_dir_all(root.join("m1/screenshots")).unwrap();
        std::fs::copy(slide, root.join("m1/screenshots/0001.png")).unwrap();
        store
            .add_screenshot(
                "m1",
                &Screenshot {
                    file: "screenshots/0001.png".into(),
                    offset_ms: 5_000.0,
                },
            )
            .unwrap();
    }
    Fixture {
        root,
        store: Arc::new(Mutex::new(store)),
        _guard: guard,
    }
}

struct Allow;
impl GpuGate for Allow {
    fn check(&self, _: Stage) -> GateDecision {
        GateDecision::Go
    }
}

fn supervisor(
    fx: &Fixture,
    gate: Arc<dyn GpuGate>,
    retention: AudioRetention,
    timeout_secs: Arc<AtomicU64>,
) -> Supervisor {
    let worker_dir = repo_root().join("worker");
    let root = fx.root.clone();
    let runner = PipelineRunner::new(
        fx.store.clone(),
        move || RunnerSettings {
            worker: WorkerConfig {
                uv: PathBuf::from("uv"),
                worker_dir: worker_dir.clone(),
                env_dir: worker_dir.join(".venv"),
                models_dir: None,
                llama_server: std::env::var_os("BBB_LLAMA_SERVER").map(PathBuf::from),
                hf_token: None,
                step_timeout: Duration::from_secs(timeout_secs.load(Ordering::SeqCst)),
                llm_api_key: None,
            },
            meeting_root: root.clone(),
            retention,
            voice: Default::default(),
        },
        |_, _, _, _| {},
    );
    let sup = Supervisor::new(fx.store.clone(), Arc::new(runner), gate, 3);
    let options = serde_json::to_string(&JobOptions {
        max_speakers: Some(3),
        ..JobOptions::default()
    })
    .unwrap();
    sup.enqueue("m1", &options).unwrap();
    sup
}

fn free_mib() -> u64 {
    NvmlSampler::new().sample().expect("NVML").free_mib
}

fn stage_status(fx: &Fixture, stage: &str) -> StageStatus {
    let job = fx.store.lock().unwrap().get_job("m1").unwrap().unwrap();
    job.stages.iter().find(|s| s.stage == stage).unwrap().status
}

#[test]
#[ignore = "needs real models and a GPU"]
fn meeting_is_processed_end_to_end_and_audio_is_removed_afterwards() {
    let fx = fixture("e2e", true);
    let before = free_mib();
    let sup = supervisor(
        &fx,
        Arc::new(Allow),
        AudioRetention::Delete,
        Arc::new(AtomicU64::new(3600)),
    );
    assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);

    let work = fx.root.join("m1/work");
    assert!(work.join("transcript.json").exists());
    assert!(work.join("descriptions.json").exists());
    // Audio is gone only because every audio stage succeeded; screenshots and results stay.
    assert!(!fx.root.join("m1/mic").exists());
    assert!(!fx.root.join("m1/meet").exists());
    assert!(fx.root.join("m1/screenshots/0001.png").exists());

    // Only one model process at a time, and nothing keeps the GPU afterwards.
    std::thread::sleep(Duration::from_secs(3));
    let after = free_mib();
    assert!(
        before.saturating_sub(after) < 600,
        "VRAM not released: {before} MiB free before, {after} MiB after"
    );
}

#[test]
#[ignore = "needs real models and a GPU"]
fn audio_is_kept_when_the_retention_is_keep() {
    let fx = fixture("keep", false);
    let sup = supervisor(
        &fx,
        Arc::new(Allow),
        AudioRetention::Keep,
        Arc::new(AtomicU64::new(3600)),
    );
    assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
    assert!(fx.root.join("m1/mic/000000.wav").exists());
}

#[test]
#[ignore = "needs real models and a GPU"]
fn a_worker_killed_in_a_stage_resumes_from_that_stage() {
    let fx = fixture("kill", false);
    let timeout = Arc::new(AtomicU64::new(3600));
    let sup = supervisor(
        &fx,
        Arc::new(Allow),
        AudioRetention::Delete,
        timeout.clone(),
    );

    // Run ASR normally, then kill the worker of each later stage in turn by a very short
    // timeout, which terminates the whole process tree exactly as a crash would.
    assert_eq!(
        sup.step("m1").unwrap(),
        StepOutcome::Ran { stage: Stage::Asr }
    );
    let asr_output = fx.root.join("m1/work/asr/mic.json");
    let asr_written = std::fs::metadata(&asr_output).unwrap().modified().unwrap();

    for stage in [Stage::Diarize, Stage::Merge] {
        timeout.store(if stage == Stage::Merge { 0 } else { 5 }, Ordering::SeqCst);
        let out = sup.step("m1").unwrap();
        assert!(
            matches!(out, StepOutcome::Failed { stage: s, .. } if s == stage),
            "{out:?}"
        );
        assert_eq!(stage_status(&fx, stage.as_str()), StageStatus::Failed);
        // The audio must survive a failed audio stage.
        assert!(fx.root.join("m1/mic/000000.wav").exists());
        timeout.store(3600, Ordering::SeqCst);
        sup.retry("m1").unwrap();
        assert_eq!(sup.step("m1").unwrap(), StepOutcome::Ran { stage });
    }
    assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
    // ASR was not repeated.
    assert_eq!(
        std::fs::metadata(&asr_output).unwrap().modified().unwrap(),
        asr_written
    );
}

#[test]
#[ignore = "needs real models and a GPU"]
fn processing_waits_while_the_gpu_is_busy_and_starts_when_it_is_free() {
    let fx = fixture("busy", false);
    let policy = GuardPolicy {
        idle_ms: 3_000,
        max_util_percent: 40,
        ..GuardPolicy::default()
    };
    let gate: Arc<dyn GpuGate> = Arc::new(NvmlGate::new(NvmlSampler::new(), policy));
    let sup = supervisor(
        &fx,
        gate,
        AudioRetention::default(),
        Arc::new(AtomicU64::new(3600)),
    );

    // Another process keeps the GPU at full load for about 40 seconds.
    let load = "import torch,time\n\
                a=torch.randn(8192,8192,device='cuda',dtype=torch.float16)\n\
                t=time.time()\n\
                while time.time()-t<40: (a@a).sum().item()\n";
    let mut child = std::process::Command::new("uv")
        .args(["run", "--frozen", "--project"])
        .arg(repo_root().join("worker"))
        .args(["python", "-c", load])
        .env("UV_PROJECT_ENVIRONMENT", repo_root().join("worker/.venv"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    // Wait until the load is visible, then check that the job is held back.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut waited_with_reason = None;
    while Instant::now() < deadline {
        match sup.step("m1").unwrap() {
            StepOutcome::Waiting { reason, .. } if reason.contains("busy") => {
                waited_with_reason = Some(reason);
                break;
            }
            StepOutcome::Waiting { .. } => {}
            other => panic!("started while the GPU was busy or unexpectedly: {other:?}"),
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let reason = waited_with_reason.expect("the job never reported the busy GPU");
    println!("waiting reason: {reason}");
    assert_eq!(stage_status(&fx, "asr"), StageStatus::Pending);
    assert_eq!(sup.waiting_reason("m1").as_deref(), Some(reason.as_str()));

    // Once the load ends the job starts by itself.
    let _ = child.wait();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut started = false;
    while Instant::now() < deadline {
        match sup.step("m1").unwrap() {
            StepOutcome::Ran { stage: Stage::Asr } => {
                started = true;
                break;
            }
            StepOutcome::Waiting { .. } => std::thread::sleep(Duration::from_secs(1)),
            other => panic!("{other:?}"),
        }
    }
    assert!(started, "the job did not start after the GPU became free");
    assert_eq!(sup.waiting_reason("m1"), None);
}
