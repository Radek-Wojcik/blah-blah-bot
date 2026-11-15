//! Runs a job through the supervisor and the production runner with a fake worker: a tiny
//! Python script stands in for `python -m bbb_worker`, so no model, GPU or `uv` is needed.
//! The script records what it was started with; the tests check the stages, the manifest,
//! the context files and the rules for the cloud.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cycle_store::{Cycle, Draft, Participant as CycleParticipant, Template};
use queue::cleanup::AudioRetention;
use queue::context::{CycleData, CycleSource};
use queue::draft::DraftFiles;
use queue::runner::{PipelineRunner, RunnerSettings};
use queue::voices::VoiceSettings;
use queue::worker::{JobOptions, Launcher, LlmOptions, Secret, WorkerConfig};
use queue::{GateDecision, GpuGate, RunError, Stage, StageRunner, StepOutcome, Supervisor, STAGES};
use serde_json::Value;
use store::{LlmKind, Meeting, MeetingState, ReportSettings, StageStatus, Store};

const TRANSCRIPT: &str = include_str!("../../../worker/tests/fixtures/transcript.json");
const DRAFT: &str = include_str!("../../../worker/tests/fixtures/meeting.valid.json");
const MANIFEST_SCHEMA: &str = include_str!("../../../schemas/job-manifest.schema.json");

const API_KEY: &str = "sk-fake-test-key-123";

/// The fake worker. Started as `python fake_worker.py <draft fixture> <step> --manifest P
/// --meeting-dir D`. It records the call, writes what the real step would write and speaks
/// the worker's line protocol.
const FAKE_WORKER: &str = r#"
import json, os, pathlib, shutil, sys

fixture, step = sys.argv[1], sys.argv[2]
args = dict(zip(sys.argv[3::2], sys.argv[4::2]))
manifest = json.loads(pathlib.Path(args["--manifest"]).read_text(encoding="utf-8"))
meeting = pathlib.Path(args["--meeting-dir"])
work = meeting / "work"

call = {
    "step": step,
    "llm": manifest["options"].get("llm"),
    "key": os.environ.get("BBB_LLM_API_KEY"),
    "context": (work / "context.json").is_file(),
    "named": (work / "transcript.named.json").is_file(),
    "windows": (work / "extract" / "windows.json").is_file(),
}
with open(meeting / "calls.jsonl", "a", encoding="utf-8") as f:
    f.write(json.dumps(call) + "\n")

print(json.dumps({"event": "progress", "step": step, "fraction": 0.5, "message": "working"}), flush=True)
if step == "extract":
    (work / "extract").mkdir(parents=True, exist_ok=True)
    (work / "extract" / "windows.json").write_text(json.dumps({"version": 1, "windows": []}), encoding="utf-8")
elif step == "reduce":
    shutil.copy(fixture, work / "draft.json")
print(json.dumps({"event": "done", "step": step}), flush=True)
"#;

/// A Python 3 interpreter, or a clear failure: the worker is Python, so CI has one.
fn python() -> String {
    for candidate in ["python3", "python"] {
        let ok = Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return candidate.to_string();
        }
    }
    panic!("no Python interpreter found (tried python3 and python)");
}

struct Allow;
impl GpuGate for Allow {
    fn check(&self, _: Stage) -> GateDecision {
        GateDecision::Go
    }
}

/// A gate that never lets anything through and counts the questions.
struct Closed(AtomicUsize);
impl GpuGate for Closed {
    fn check(&self, _: Stage) -> GateDecision {
        self.0.fetch_add(1, Ordering::SeqCst);
        GateDecision::Wait("GPU busy".into())
    }
}

struct Lab {
    root: tempfile::TempDir,
    store: Arc<Mutex<Store>>,
    runner: Arc<PipelineRunner>,
    sup: Supervisor,
}

impl Lab {
    fn dir(&self) -> PathBuf {
        self.root.path().join("m1")
    }

    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.dir().join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn manifest(&self) -> Value {
        serde_json::from_slice(&fs::read(self.dir().join("work/manifest.json")).unwrap()).unwrap()
    }

    fn stage(&self, stage: Stage) -> store::StageRow {
        let job = self.store.lock().unwrap().get_job("m1").unwrap().unwrap();
        job.stages
            .into_iter()
            .find(|s| s.stage == stage.as_str())
            .unwrap()
    }

    fn with_store<T>(&self, f: impl FnOnce(&Store) -> T) -> T {
        f(&self.store.lock().unwrap())
    }
}

fn llm(kind: LlmKind) -> JobOptions {
    JobOptions {
        llm: Some(LlmOptions {
            kind,
            model_path: (kind == LlmKind::Managed).then(|| "models/gemma.gguf".to_string()),
            ctx: Some(16_384),
            base_url: (kind != LlmKind::Managed).then(|| "https://api.example.com/v1".to_string()),
            model: (kind != LlmKind::Managed).then(|| "some-model".to_string()),
            // The stored claim is ignored for the cloud: only the store counts.
            cloud_confirmed: true,
        }),
        ..JobOptions::default()
    }
}

fn cycle_source() -> Arc<dyn CycleSource> {
    Arc::new(|id: &str| -> Result<Option<CycleData>, RunError> {
        if id != "TEAM" {
            return Ok(None);
        }
        let mut cycle = Cycle::new("TEAM", "Team");
        cycle.participants.push(CycleParticipant {
            id: "alex".into(),
            name: "Alex Example".into(),
            github: None,
            discord: None,
        });
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

/// A meeting with a finished transcript whose first seven stages are already done, so a
/// run goes straight to `extract` and `reduce`.
fn lab(
    options: &JobOptions,
    cycle: Option<&str>,
    gate: Arc<dyn GpuGate>,
    key: Option<&str>,
) -> Lab {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("fake_worker.py"), FAKE_WORKER).unwrap();
    fs::write(root.path().join("draft.fixture.json"), DRAFT).unwrap();
    fs::create_dir_all(root.path().join("m1/work")).unwrap();
    fs::write(root.path().join("m1/work/transcript.json"), TRANSCRIPT).unwrap();

    let store = Store::open_in_memory().unwrap();
    store
        .create_meeting(&Meeting {
            id: "m1".into(),
            title: "Weekly".into(),
            started_unix_ms: 1_794_564_000_000,
            duration_ms: Some(60_000),
            state: MeetingState::Stopped,
            dir: "m1".into(),
        })
        .unwrap();
    store
        .set_report_settings(
            "m1",
            &ReportSettings {
                cycle_id: cycle.map(str::to_string),
                ..ReportSettings::default()
            },
        )
        .unwrap();
    let store = Arc::new(Mutex::new(store));

    let meeting_root = root.path().to_path_buf();
    let key = key.map(Secret::new);
    let runner = Arc::new(
        PipelineRunner::new(
            store.clone(),
            move || RunnerSettings {
                worker: WorkerConfig {
                    uv: PathBuf::from("uv-is-not-used"),
                    worker_dir: PathBuf::from("worker"),
                    env_dir: PathBuf::from("env"),
                    models_dir: None,
                    llama_server: None,
                    hf_token: None,
                    step_timeout: Duration::from_secs(60),
                    llm_api_key: key.clone(),
                },
                meeting_root: meeting_root.clone(),
                retention: AudioRetention::Keep,
                voice: VoiceSettings::default(),
            },
            |_, _, _, _| {},
        )
        .with_launcher(Launcher {
            program: python().into(),
            args: vec![
                root.path().join("fake_worker.py").into(),
                root.path().join("draft.fixture.json").into(),
            ],
        })
        .with_cycle_source(cycle_source()),
    );

    struct Delegate(Arc<PipelineRunner>);
    impl StageRunner for Delegate {
        fn run(&self, meeting_id: &str, stage: Stage) -> Result<(), RunError> {
            self.0.run(meeting_id, stage)
        }
    }
    let sup = Supervisor::new(store.clone(), Arc::new(Delegate(runner.clone())), gate, 3);
    sup.enqueue("m1", &serde_json::to_string(options).unwrap())
        .unwrap();
    {
        let s = store.lock().unwrap();
        for stage in &STAGES[..7] {
            s.set_stage_status("m1", stage.as_str(), StageStatus::Done, None, 1)
                .unwrap();
        }
    }
    Lab {
        root,
        store,
        runner,
        sup,
    }
}

fn steps(calls: &[Value]) -> Vec<&str> {
    calls.iter().map(|c| c["step"].as_str().unwrap()).collect()
}

#[test]
fn extract_and_reduce_run_with_the_context_and_the_llm_options() {
    let lab = lab(
        &llm(LlmKind::Managed),
        Some("TEAM"),
        Arc::new(Allow),
        Some(API_KEY),
    );
    assert_eq!(lab.sup.drive("m1").unwrap(), StepOutcome::Finished);

    let calls = lab.calls();
    assert_eq!(steps(&calls), ["extract", "reduce"]);
    // The application prepared the context and the named transcript before `extract`.
    assert_eq!(calls[0]["context"], true);
    assert_eq!(calls[0]["named"], true);
    // `reduce` saw the windows `extract` wrote.
    assert_eq!(calls[1]["windows"], true);
    // A managed backend gets no key, even though one is configured.
    assert_eq!(calls[0]["key"], Value::Null);

    // The manifest carries the backend as the schema wants it.
    let manifest = lab.manifest();
    let schema: Value = serde_json::from_str(MANIFEST_SCHEMA).unwrap();
    assert!(jsonschema::validator_for(&schema)
        .unwrap()
        .is_valid(&manifest));
    assert_eq!(manifest["options"]["llm"]["kind"], "managed");
    assert_eq!(
        manifest["options"]["llm"]["model_path"],
        "models/gemma.gguf"
    );
    assert_eq!(manifest["options"]["llm"]["ctx"], 16_384);

    // The context file is what the worker needs.
    let context: Value =
        serde_json::from_slice(&fs::read(lab.dir().join("work/context.json")).unwrap()).unwrap();
    assert_eq!(context["cycle"], "TEAM");
    assert_eq!(context["meeting_date"], "2026-11-13");
    assert_eq!(context["participants"][0]["id"], "alex");

    // The draft the fake `reduce` wrote loads through the application's editor code.
    let draft: Draft = DraftFiles::new(&lab.dir()).load().unwrap();
    assert_eq!(draft.cycle, "TEAM");

    for stage in [Stage::Extract, Stage::Reduce] {
        let row = lab.stage(stage);
        assert_eq!(row.status, StageStatus::Done);
        assert_eq!(row.note, None);
        assert_eq!(row.attempts, 1);
    }
}

#[test]
fn an_external_backend_gets_the_key_through_the_environment_only() {
    // The GPU is never free, which an external backend does not care about.
    let gate = Arc::new(Closed(AtomicUsize::new(0)));
    let lab = lab(
        &llm(LlmKind::External),
        Some("TEAM"),
        gate.clone(),
        Some(API_KEY),
    );
    assert_eq!(lab.sup.drive("m1").unwrap(), StepOutcome::Finished);
    assert_eq!(gate.0.load(Ordering::SeqCst), 0, "the gate was not asked");

    let calls = lab.calls();
    assert_eq!(steps(&calls), ["extract", "reduce"]);
    assert!(calls.iter().all(|c| c["key"] == API_KEY));
    assert_eq!(calls[0]["llm"]["kind"], "external");
    assert_eq!(calls[0]["llm"]["base_url"], "https://api.example.com/v1");

    // The key is in no file the application wrote.
    for file in ["work/manifest.json", "work/context.json"] {
        let text = fs::read_to_string(lab.dir().join(file)).unwrap();
        assert!(!text.contains(API_KEY), "{file}");
    }
}

#[test]
fn a_managed_backend_waits_for_the_gpu() {
    let gate = Arc::new(Closed(AtomicUsize::new(0)));
    let lab = lab(&llm(LlmKind::Managed), Some("TEAM"), gate.clone(), None);
    assert_eq!(
        lab.sup.drive("m1").unwrap(),
        StepOutcome::Waiting {
            stage: Stage::Extract,
            reason: "GPU busy".into()
        }
    );
    assert!(lab.calls().is_empty(), "no worker was started");
    assert_eq!(lab.stage(Stage::Extract).status, StageStatus::Pending);
}

#[test]
fn the_cloud_never_starts_the_worker_without_a_confirmation() {
    let lab = lab(
        &llm(LlmKind::Cloud),
        Some("TEAM"),
        Arc::new(Allow),
        Some(API_KEY),
    );
    // The job options claim `cloud_confirmed: true`; the store has no record.
    match lab.sup.drive("m1").unwrap() {
        StepOutcome::Failed { stage, error } => {
            assert_eq!(stage, Stage::Extract);
            assert!(error.contains("not confirmed"), "{error}");
        }
        other => panic!("{other:?}"),
    }
    assert!(lab.calls().is_empty(), "the worker was started");
    assert!(!lab.dir().join("work/manifest.json").exists());
    assert!(!lab.dir().join("work/context.json").exists());

    // The runner refuses on its own as well (defense in depth, in case it is used without
    // the supervisor's check): nothing is started and no manifest is written.
    let error = lab.runner.run("m1", Stage::Extract).unwrap_err();
    assert!(error.message.contains("not confirmed"), "{error}");
    assert!(lab.calls().is_empty());
    assert!(!lab.dir().join("work/manifest.json").exists());

    // A choice the user has not confirmed is no confirmation.
    lab.with_store(|s| {
        s.set_llm_choice("m1", LlmKind::Cloud, Some("api.example.com"))
            .unwrap()
    });
    lab.sup.retry("m1").unwrap();
    assert!(matches!(
        lab.sup.drive("m1").unwrap(),
        StepOutcome::Failed { .. }
    ));
    assert!(lab.calls().is_empty());

    // Once the user confirmed this endpoint, the stages run and the key reaches them.
    lab.with_store(|s| s.confirm_cloud("m1", 99).unwrap());
    lab.sup.retry("m1").unwrap();
    assert_eq!(lab.sup.drive("m1").unwrap(), StepOutcome::Finished);
    let calls = lab.calls();
    assert_eq!(steps(&calls), ["extract", "reduce"]);
    assert!(calls.iter().all(|c| c["key"] == API_KEY));
    assert_eq!(calls[0]["llm"]["cloud_confirmed"], true);
    assert_eq!(lab.manifest()["options"]["llm"]["cloud_confirmed"], true);
}

#[test]
fn a_confirmation_for_another_endpoint_does_not_cover_the_job() {
    let lab = lab(
        &llm(LlmKind::Cloud),
        Some("TEAM"),
        Arc::new(Allow),
        Some(API_KEY),
    );
    lab.with_store(|s| {
        s.set_llm_choice("m1", LlmKind::Cloud, Some("other.example.net"))
            .unwrap();
        s.confirm_cloud("m1", 1).unwrap();
    });
    assert!(matches!(
        lab.sup.drive("m1").unwrap(),
        StepOutcome::Failed { .. }
    ));
    assert!(lab.calls().is_empty());
}

#[test]
fn a_meeting_without_a_cycle_skips_both_stages_as_done() {
    let lab = lab(&llm(LlmKind::Managed), None, Arc::new(Allow), None);
    assert_eq!(lab.sup.drive("m1").unwrap(), StepOutcome::Finished);
    assert!(lab.calls().is_empty());
    assert!(!lab.dir().join("work/context.json").exists());
    for stage in [Stage::Extract, Stage::Reduce] {
        let row = lab.stage(stage);
        assert_eq!(row.status, StageStatus::Done);
        assert_eq!(row.error, None);
        assert_eq!(row.note.as_deref(), Some("the meeting has no cycle"));
    }
}

#[test]
fn a_job_without_a_backend_finishes_as_before() {
    // The options of a job made before the backend existed.
    let lab = lab(&JobOptions::default(), Some("TEAM"), Arc::new(Allow), None);
    assert_eq!(lab.sup.drive("m1").unwrap(), StepOutcome::Finished);
    assert!(lab.calls().is_empty());
    assert!(lab.stage(Stage::Reduce).note.is_some());
}

#[test]
fn a_cycle_that_is_not_registered_fails_extract_and_the_worker_is_not_started() {
    let lab = lab(&llm(LlmKind::Managed), Some("GONE"), Arc::new(Allow), None);
    match lab.sup.drive("m1").unwrap() {
        StepOutcome::Failed { stage, error } => {
            assert_eq!(stage, Stage::Extract);
            assert!(error.contains("GONE"), "{error}");
        }
        other => panic!("{other:?}"),
    }
    assert!(lab.calls().is_empty());
}
