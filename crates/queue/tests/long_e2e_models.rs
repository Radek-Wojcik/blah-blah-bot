//! A long meeting through the whole pipeline with the real worker, real models, a managed
//! `llama-server` and a GPU. The audio is synthetic: the reference meetings are repeated until
//! the requested length is reached and cut into 60 second segment files like the recorder
//! writes them.
//!
//! Ignored by default. Do not start it by hand: `scripts/ten-hour-run.ps1` checks the disk and
//! the GPU first, samples the machine while it runs and removes everything afterwards.
//!
//! ```text
//! BBB_AUDIO_DIR=<folder with the synthetic meetings, one folder each> \
//! BBB_LLM_MODEL=<GGUF file> BBB_LLAMA_SERVER=<llama-server.exe> \
//! BBB_LONG_HOURS=10 BBB_RESULTS_OUT=<file.json> \
//! cargo test -p queue --test long_e2e_models -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Optional: `BBB_LONG_MEETINGS=pl-03,en-03` (default: every meeting folder of the audio
//! folder, in name order), `BBB_E2E_WORK=<folder>` (default `target/e2e-tmp`, the work folder
//! is removed at the end, also after a failure), `BBB_WORKER_ENV_DIR=<folder>` (worker
//! environment, default `worker/.venv`).
//!
//! The result file holds the seconds and the peak VRAM of every stage, the draft checks and
//! the verdicts against the targets of `docs/TESTING.md` (4 hours for 10 hours of audio,
//! 11.5 GB of VRAM). It is written before the assertions, so a miss leaves evidence.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cycle_store::{Cycle, Participant as CycleParticipant, Schema, Template};
use queue::cleanup::AudioRetention;
use queue::context::{CycleData, CycleSource};
use queue::draft::DraftFiles;
use queue::gpu::{GuardPolicy, NvmlGate, NvmlSampler};
use queue::runner::{PipelineRunner, RunnerSettings};
use queue::voices::VoiceSettings;
use queue::worker::{JobOptions, LlmOptions, WorkerConfig};
use queue::{GateDecision, GpuGate, RunError, Stage, StepOutcome, Supervisor, STAGES};
use store::{LlmKind, Meeting, MeetingState, ReportSettings, Segment, StageStatus, Store, Track};

const SAMPLE_RATE: usize = 16_000;
const SEGMENT_SAMPLES: usize = SAMPLE_RATE * 60;
const CYCLE_ID: &str = "NOMAD";
const MEETING_ID: &str = "long-run";
const CTX: u32 = 16_384;
/// 2026-11-13 10:00 UTC.
const STARTED_UNIX_MS: i64 = 1_794_564_000_000;
/// docs/TESTING.md section 5: 10 hours of audio, full pipeline.
const LIMIT_HOURS: f64 = 4.0;
/// docs/TESTING.md section 5: peak VRAM in any stage ("11.5 GB" read as 11,500 MiB).
const LIMIT_VRAM_MIB: u64 = 11_500;
/// How long a stage may wait for the GPU guard before the run gives up.
const GATE_PATIENCE: Duration = Duration::from_secs(45 * 60);

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set")))
}

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
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, out).unwrap();
}

/// One reference meeting: the mixed track and the speakers of its script.
struct Source {
    pcm: Vec<u8>,
    speakers: Vec<String>,
}

fn meeting_names(audio: &Path) -> Vec<String> {
    if let Ok(list) = std::env::var("BBB_LONG_MEETINGS") {
        return list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    let mut names: Vec<String> = std::fs::read_dir(audio)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("mixed.wav").is_file())
        .filter(|e| e.path().join("annotation.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn load_sources(audio: &Path, names: &[String]) -> Vec<Source> {
    names
        .iter()
        .map(|name| {
            let dir = audio.join(name);
            let annotation: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join("annotation.json")).unwrap())
                    .unwrap();
            let speakers = annotation["speakers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect();
            Source {
                pcm: read_pcm(&dir.join("mixed.wav")),
                speakers,
            }
        })
        .collect()
}

/// Writes `total_samples` of the repeated sources as 60 second segment files of the track
/// `meet` and registers them. Returns the number of segments.
fn write_segments(store: &Store, dir: &Path, sources: &[Source], total_samples: usize) -> u32 {
    let mut pending: Vec<u8> = Vec::with_capacity(SEGMENT_SAMPLES * 4);
    let mut written_samples = 0usize;
    let mut index = 0u32;
    let flush = |chunk: &[u8], index: u32| {
        let file = format!("meet/{index:06}.wav");
        write_wav(&dir.join(&file), chunk);
        store
            .add_segment(
                MEETING_ID,
                &Segment {
                    track: "meet".into(),
                    segment: index,
                    file,
                    offset_ms: f64::from(index) * 60_000.0,
                    samples: (chunk.len() / 2) as u64,
                },
            )
            .unwrap();
    };
    'fill: loop {
        for source in sources {
            let remaining = (total_samples - written_samples - pending.len() / 2) * 2;
            let take = source.pcm.len().min(remaining);
            pending.extend_from_slice(&source.pcm[..take]);
            while pending.len() >= SEGMENT_SAMPLES * 2 {
                let rest = pending.split_off(SEGMENT_SAMPLES * 2);
                flush(&pending, index);
                index += 1;
                written_samples += SEGMENT_SAMPLES;
                pending = rest;
            }
            if written_samples + pending.len() / 2 >= total_samples {
                break 'fill;
            }
        }
    }
    if !pending.is_empty() {
        flush(&pending, index);
        index += 1;
    }
    index
}

/// Peak used VRAM in MiB since the last `take`, sampled with `nvidia-smi` every two seconds.
struct VramSampler {
    peak: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

fn used_vram_mib() -> u64 {
    Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used",
            "--format=csv,noheader,nounits",
            "--id=0",
        ])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

impl VramSampler {
    fn start() -> VramSampler {
        let peak = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, s) = (peak.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                p.fetch_max(used_vram_mib(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        VramSampler {
            peak,
            stop,
            handle: Some(handle),
        }
    }

    fn take(&self) -> u64 {
        self.peak.swap(0, Ordering::Relaxed)
    }
}

impl Drop for VramSampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// A gate that asks the real GPU guard and counts the waits.
struct Watched {
    inner: Arc<dyn GpuGate>,
    waits: AtomicU64,
}

impl GpuGate for Watched {
    fn check(&self, stage: Stage) -> GateDecision {
        let decision = self.inner.check(stage);
        if decision != GateDecision::Go {
            self.waits.fetch_add(1, Ordering::Relaxed);
        }
        decision
    }
}

fn cycle_source(speakers: &BTreeSet<String>) -> Arc<dyn CycleSource> {
    let participants: Vec<CycleParticipant> = speakers
        .iter()
        .map(|id| CycleParticipant {
            id: id.clone(),
            name: id.to_uppercase(),
            github: None,
            discord: None,
        })
        .collect();
    Arc::new(move |id: &str| -> Result<Option<CycleData>, RunError> {
        if id != CYCLE_ID {
            return Ok(None);
        }
        let mut cycle = Cycle::new(CYCLE_ID, "Nomad");
        cycle.participants = participants.clone();
        Ok(Some(CycleData {
            cycle,
            templates: vec![Template::builtin_default()],
            topics: vec![],
            issues: vec![],
            closed_issue_ids: vec![],
            previous_dirs: vec![],
        }))
    })
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[test]
#[ignore = "needs real models, a language model and a GPU; takes hours"]
fn a_long_synthetic_meeting_goes_through_the_whole_pipeline() {
    let hours: f64 = std::env::var("BBB_LONG_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10.0);
    assert!(hours > 0.0, "BBB_LONG_HOURS must be positive");
    let audio = env_path("BBB_AUDIO_DIR");
    let model = env_path("BBB_LLM_MODEL");
    let llama = env_path("BBB_LLAMA_SERVER");
    assert!(model.is_file(), "BBB_LLM_MODEL is not a file: {model:?}");
    assert!(llama.is_file(), "BBB_LLAMA_SERVER is not a file: {llama:?}");

    let names = meeting_names(&audio);
    assert!(!names.is_empty(), "no meeting folder in {audio:?}");
    let sources = load_sources(&audio, &names);
    let speakers: BTreeSet<String> = sources
        .iter()
        .flat_map(|s| s.speakers.iter().cloned())
        .collect();

    // The work folder lives under target/, never in the system temp directory.
    let work_parent = std::env::var_os("BBB_E2E_WORK")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("target").join("e2e-tmp"));
    std::fs::create_dir_all(&work_parent).unwrap();
    let guard = tempfile::Builder::new()
        .prefix("bbb-long-")
        .tempdir_in(&work_parent)
        .unwrap();
    let root = guard.path().to_path_buf();
    let meeting_dir = root.join(MEETING_ID);

    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let total_samples = (hours * 3600.0 * SAMPLE_RATE as f64) as usize;
    let duration_ms = (total_samples * 1000 / SAMPLE_RATE) as i64;
    let started = Instant::now();
    let segments = {
        let store = store.lock().unwrap();
        store
            .create_meeting(&Meeting {
                id: MEETING_ID.into(),
                title: MEETING_ID.into(),
                started_unix_ms: STARTED_UNIX_MS,
                duration_ms: Some(duration_ms),
                state: MeetingState::Stopped,
                dir: MEETING_ID.into(),
            })
            .unwrap();
        store
            .upsert_track(
                MEETING_ID,
                &Track {
                    name: "meet".into(),
                    source: "app:meet.exe".into(),
                    pid: None,
                    offset_ms: 0.0,
                    ended: true,
                    end_offset_ms: None,
                    total_samples: None,
                },
            )
            .unwrap();
        let n = write_segments(&store, &meeting_dir, &sources, total_samples);
        for id in &speakers {
            store
                .create_participant_in(Some(CYCLE_ID), id, &id.to_uppercase(), 1)
                .unwrap();
        }
        store
            .set_report_settings(
                MEETING_ID,
                &ReportSettings {
                    cycle_id: Some(CYCLE_ID.into()),
                    report_mode: Some("fixed".into()),
                    ..ReportSettings::default()
                },
            )
            .unwrap();
        n
    };
    println!(
        "{segments} segments, {hours} h of audio from {names:?}, written in {:.0} s into {root:?}",
        started.elapsed().as_secs_f64()
    );

    let worker_dir = repo_root().join("worker");
    let env_dir = std::env::var_os("BBB_WORKER_ENV_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| worker_dir.join(".venv"));
    let runner_root = root.clone();
    let llama_for_runner = llama.clone();
    let runner = PipelineRunner::new(
        store.clone(),
        move || RunnerSettings {
            worker: WorkerConfig {
                uv: PathBuf::from("uv"),
                worker_dir: worker_dir.clone(),
                env_dir: env_dir.clone(),
                models_dir: None,
                llama_server: Some(llama_for_runner.clone()),
                hf_token: None,
                // One stage over ten hours of audio may run long.
                step_timeout: Duration::from_secs(4 * 3600),
                llm_api_key: None,
            },
            meeting_root: runner_root.clone(),
            retention: AudioRetention::Delete,
            voice: VoiceSettings::default(),
        },
        |_, _, _, _| {},
    )
    .with_cycle_source(cycle_source(&speakers));
    let gate = Arc::new(Watched {
        inner: Arc::new(NvmlGate::new(NvmlSampler::new(), GuardPolicy::default())),
        waits: AtomicU64::new(0),
    });
    let sup = Supervisor::new(store.clone(), Arc::new(runner), gate.clone(), 3);
    let options = serde_json::to_string(&JobOptions {
        max_speakers: Some(speakers.len().max(2) as u32),
        llm: Some(LlmOptions {
            kind: LlmKind::Managed,
            model_path: Some(model.to_string_lossy().into_owned()),
            ctx: Some(CTX),
            base_url: None,
            model: None,
            cloud_confirmed: false,
        }),
        ..JobOptions::default()
    })
    .unwrap();
    sup.enqueue(MEETING_ID, &options).unwrap();

    let baseline_mib = used_vram_mib();
    let sampler = VramSampler::start();
    let mut stages: Vec<serde_json::Value> = Vec::new();
    let mut peak_absolute = baseline_mib;
    let run_started = Instant::now();
    let mut waiting_since: Option<Instant> = None;
    let mut failure: Option<String> = None;
    loop {
        let stage_started = Instant::now();
        match sup.step(MEETING_ID).unwrap() {
            StepOutcome::Ran { stage } => {
                waiting_since = None;
                let seconds = stage_started.elapsed().as_secs_f64();
                let peak = sampler.take();
                peak_absolute = peak_absolute.max(peak);
                println!(
                    "  stage {:<14} {seconds:>8.1} s, peak {peak} MiB",
                    stage.as_str()
                );
                stages.push(serde_json::json!({
                    "stage": stage.as_str(),
                    "seconds": (seconds * 10.0).round() / 10.0,
                    "peak_vram_mib": peak,
                    "peak_vram_above_baseline_mib": peak.saturating_sub(baseline_mib),
                }));
            }
            StepOutcome::Finished => break,
            StepOutcome::Waiting { stage, reason } => {
                let since = *waiting_since.get_or_insert_with(|| {
                    println!("  stage {:<14} waits: {reason}", stage.as_str());
                    Instant::now()
                });
                if since.elapsed() >= GATE_PATIENCE {
                    failure = Some(format!(
                        "the GPU guard held {} back for too long: {reason}",
                        stage.as_str()
                    ));
                    break;
                }
                std::thread::sleep(Duration::from_secs(5));
            }
            StepOutcome::Failed { stage, error } => {
                failure = Some(format!("stage {} failed: {error}", stage.as_str()));
                break;
            }
            other => {
                failure = Some(format!("unexpected outcome {other:?}"));
                break;
            }
        }
    }
    drop(sampler);
    // Seconds spent waiting for the GPU are not processing time.
    let total_seconds = run_started.elapsed().as_secs_f64();

    // What the job left behind.
    let rows = store
        .lock()
        .unwrap()
        .get_job(MEETING_ID)
        .unwrap()
        .map(|j| j.stages)
        .unwrap_or_default();
    let stages_done = STAGES
        .iter()
        .filter(|s| {
            rows.iter()
                .any(|r| r.stage == s.as_str() && r.status == StageStatus::Done)
        })
        .count();
    let mut draft_valid = false;
    let mut items = 0usize;
    let mut warnings = 0usize;
    let mut verified_tasks = 0usize;
    let draft_path = meeting_dir.join("work/draft.json");
    if let Ok(bytes) = std::fs::read(&draft_path) {
        let raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        draft_valid = cycle_store::schema::validate(Schema::Meeting, &raw).is_empty();
        if let Ok(draft) = DraftFiles::new(&meeting_dir).load() {
            items = draft.items.len();
            warnings = draft.warnings.len();
            verified_tasks = draft
                .items
                .iter()
                .filter(|i| i.source.as_ref().is_some_and(|s| s.verified))
                .count();
        }
    }
    let total_hours = total_seconds / 3600.0;
    let passed = failure.is_none()
        && stages_done == STAGES.len()
        && draft_valid
        && items > 0
        && total_hours * (10.0 / hours) <= LIMIT_HOURS
        && peak_absolute <= LIMIT_VRAM_MIB;
    let result = serde_json::json!({
        "prototype": "stage8-e2e-10h",
        "finished_unix_s": unix_seconds(),
        "hours_of_audio": hours,
        "meetings_cycled": names,
        "speakers": speakers.len(),
        "segments": segments,
        "stages": stages,
        "stages_done": stages_done,
        "stages_total": STAGES.len(),
        "total_seconds": (total_seconds * 10.0).round() / 10.0,
        "total_hours": (total_hours * 1000.0).round() / 1000.0,
        "gpu_guard_waits": gate.waits.load(Ordering::Relaxed),
        "baseline_vram_mib": baseline_mib,
        "peak_vram_absolute_mib": peak_absolute,
        "threshold_hours_for_10h": LIMIT_HOURS,
        "threshold_vram_mib": LIMIT_VRAM_MIB,
        "draft_valid": draft_valid,
        "items": items,
        "items_with_verified_quote": verified_tasks,
        "warnings": warnings,
        "error": failure.clone().unwrap_or_default(),
        "full_length": hours >= 10.0,
        "passed": passed,
    });
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    if let Some(out) = std::env::var_os("BBB_RESULTS_OUT") {
        let out = PathBuf::from(out);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&out, serde_json::to_string_pretty(&result).unwrap() + "\n").unwrap();
    }

    if let Some(message) = failure {
        panic!("{message}");
    }
    assert_eq!(stages_done, STAGES.len(), "not every stage is done");
    assert!(draft_valid, "draft.json does not validate");
    assert!(items > 0, "the draft has no items");
    assert!(
        peak_absolute <= LIMIT_VRAM_MIB,
        "peak VRAM {peak_absolute} MiB is over {LIMIT_VRAM_MIB} MiB"
    );
    assert!(
        total_hours * (10.0 / hours) <= LIMIT_HOURS,
        "{total_hours:.2} h for {hours} h of audio is over the target of {LIMIT_HOURS} h per 10 h"
    );
}

/// The synthetic audio is cut exactly like the recorder cuts it: whole 60 second segments and
/// a shorter last one, the sources repeated until the length is reached.
#[test]
fn the_audio_is_cut_into_whole_minute_segments_of_the_repeated_sources() {
    let store = Store::open_in_memory().unwrap();
    store
        .create_meeting(&Meeting {
            id: MEETING_ID.into(),
            title: MEETING_ID.into(),
            started_unix_ms: STARTED_UNIX_MS,
            duration_ms: Some(150_000),
            state: MeetingState::Stopped,
            dir: MEETING_ID.into(),
        })
        .unwrap();
    store
        .upsert_track(
            MEETING_ID,
            &Track {
                name: "meet".into(),
                source: "app:meet.exe".into(),
                pid: None,
                offset_ms: 0.0,
                ended: true,
                end_offset_ms: None,
                total_samples: None,
            },
        )
        .unwrap();
    // Two sources of 40 s and 25 s: 150 s need 2.3 repeats.
    let sources = vec![
        Source {
            pcm: vec![1u8; SAMPLE_RATE * 2 * 40],
            speakers: vec!["a".into()],
        },
        Source {
            pcm: vec![2u8; SAMPLE_RATE * 2 * 25],
            speakers: vec!["b".into()],
        },
    ];
    let dir = tempfile::tempdir().unwrap();
    let total = SAMPLE_RATE * 150;
    let count = write_segments(&store, dir.path(), &sources, total);
    assert_eq!(count, 3);
    let lengths: Vec<usize> = (0..3)
        .map(|i| read_pcm(&dir.path().join(format!("meet/{i:06}.wav"))).len() / 2)
        .collect();
    assert_eq!(
        lengths,
        [SEGMENT_SAMPLES, SEGMENT_SAMPLES, SAMPLE_RATE * 30]
    );
    // Segment 0: 40 s of the first source, then 20 of the 25 s of the second. Segment 1 starts
    // with the last 5 s of the second source, then the first source starts over.
    let first = read_pcm(&dir.path().join("meet/000000.wav"));
    assert_eq!(first[0], 1);
    assert_eq!(first[SAMPLE_RATE * 2 * 40], 2);
    assert_eq!(first[SAMPLE_RATE * 2 * 60 - 1], 2);
    let second = read_pcm(&dir.path().join("meet/000001.wav"));
    assert_eq!(second[0], 2);
    assert_eq!(second[SAMPLE_RATE * 2 * 5], 1, "the sources start over");
}
