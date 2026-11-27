//! The progress callback of the production runner may read the queue state, which needs the
//! store. The application does exactly that, so the runner must never report progress while
//! it holds the store: the lock is not reentrant and the stage would hang as `running`.

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cycle_store::{Cycle, Template};
use queue::cleanup::AudioRetention;
use queue::context::{CycleData, CycleSource};
use queue::runner::{PipelineRunner, RunnerSettings};
use queue::voices::VoiceSettings;
use queue::worker::{JobOptions, Launcher, LlmOptions, WorkerConfig};
use queue::{RunError, Stage, StageRunner, STAGES};
use store::{LlmKind, Meeting, MeetingState, ReportSettings, Store};

const TRANSCRIPT: &str = include_str!("../../../worker/tests/fixtures/transcript.json");

#[test]
fn the_context_is_reported_without_holding_the_store() {
    let root = tempfile::tempdir().unwrap();
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
                cycle_id: Some("TEAM".into()),
                ..ReportSettings::default()
            },
        )
        .unwrap();
    let options = JobOptions {
        llm: Some(LlmOptions {
            kind: LlmKind::Managed,
            model_path: Some("models/gemma.gguf".into()),
            ctx: Some(16_384),
            base_url: None,
            model: None,
            cloud_confirmed: false,
        }),
        ..JobOptions::default()
    };
    let names: Vec<&str> = STAGES.iter().map(|s| s.as_str()).collect();
    store
        .create_job("m1", &names, &serde_json::to_string(&options).unwrap(), 1)
        .unwrap();
    let store = Arc::new(Mutex::new(store));

    let reported = Arc::new(Mutex::new(Vec::<String>::new()));
    let meeting_root = root.path().to_path_buf();
    let progress_store = store.clone();
    let progress_log = reported.clone();
    let source: Arc<dyn CycleSource> =
        Arc::new(|id: &str| -> Result<Option<CycleData>, RunError> {
            Ok((id == "TEAM").then(|| CycleData {
                cycle: Cycle::new("TEAM", "Team"),
                templates: vec![Template::builtin_default()],
                topics: vec![],
                issues: vec![],
                closed_issue_ids: vec![],
                issue_links: vec![],
                previous_dirs: vec![],
            }))
        });
    let runner = PipelineRunner::new(
        store,
        move || RunnerSettings {
            worker: WorkerConfig {
                uv_cache_dir: None,
                uv: PathBuf::from("uv-is-not-used"),
                worker_dir: PathBuf::from("worker"),
                env_dir: PathBuf::from("env"),
                models_dir: None,
                llama_server: None,
                hf_token: None,
                step_timeout: Duration::from_secs(60),
                llm_api_key: None,
            },
            meeting_root: meeting_root.clone(),
            retention: AudioRetention::Keep,
            voice: VoiceSettings::default(),
        },
        // Like the application: reading the queue state takes the store.
        move |_, _, _, message| {
            let store = progress_store.lock().unwrap();
            let _ = store.get_job("m1");
            progress_log.lock().unwrap().push(message.to_string());
        },
    )
    .with_launcher(Launcher {
        program: "no-such-program-for-this-test".into(),
        args: vec![],
    })
    .with_cycle_source(source);

    // On a regression the run never returns; the thread is then left behind and the test
    // fails after the wait instead of hanging the suite.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(runner.run("m1", Stage::Extract));
    });
    let result = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("`extract` hung: the progress callback was called with the store locked");

    // The fake launcher does not exist, so the step ends in the worker start, after the
    // context was written and reported.
    let error = result.unwrap_err().message;
    assert!(error.contains("could not start the worker"), "{error}");
    assert!(root.path().join("m1/work/context.json").is_file());
    let reported = reported.lock().unwrap();
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert!(reported[0].starts_with("Context ready"));
}
