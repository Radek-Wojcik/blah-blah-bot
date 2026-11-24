//! Runs stages one at a time and records every transition.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use store::{JobRow, LlmKind, StageStatus, Store};

use crate::stage::{next_step, Next, Stage, STAGES};
use crate::worker::{confirmed_for_run, JobOptions};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error(transparent)]
    Store(#[from] store::StoreError),
    #[error("internal lock poisoned")]
    Poisoned,
    #[error("no job for meeting {0}")]
    NoJob(String),
}

/// A stage ended in an error.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct RunError {
    pub message: String,
}

impl RunError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Executes one stage of one meeting. The implementation spawns the worker process and
/// returns when it has exited; it must leave no model process behind.
pub trait StageRunner: Send + Sync {
    fn run(&self, meeting_id: &str, stage: Stage) -> Result<(), RunError>;
}

/// Answer of the GPU gate for one stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    Go,
    /// The stage must wait; the text is shown to the user.
    Wait(String),
}

/// Decides whether a GPU stage may start now. Evaluated between stages, never during one.
pub trait GpuGate: Send + Sync {
    fn check(&self, stage: Stage) -> GateDecision;
}

/// Result of one [`Supervisor::step`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepOutcome {
    /// A stage ran to completion.
    Ran { stage: Stage },
    /// A stage ran and failed; the job is blocked until retried.
    Failed { stage: Stage, error: String },
    /// The GPU is not available for this stage yet.
    Waiting { stage: Stage, reason: String },
    /// Another stage is running; only one model process may exist.
    Busy,
    /// The job is blocked by an earlier failure.
    Blocked { stage: Stage },
    /// Every stage is done.
    Finished,
}

/// Notification for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueueEvent {
    StageStarted {
        meeting_id: String,
        stage: Stage,
    },
    StageDone {
        meeting_id: String,
        stage: Stage,
    },
    StageFailed {
        meeting_id: String,
        stage: Stage,
        error: String,
    },
    Waiting {
        meeting_id: String,
        stage: Stage,
        reason: String,
    },
    Finished {
        meeting_id: String,
    },
}

type Listener = Box<dyn Fn(&QueueEvent) + Send + Sync>;

/// What to do with a stage before it is started, decided from the store.
enum Plan {
    /// Run it; the kind is the language-model backend of the job, which decides whether the
    /// stage needs the GPU.
    Run(Option<LlmKind>),
    /// Nothing to do for this meeting: mark the stage done with this note.
    Skip(String),
    /// Must not run: fail the stage with this message.
    Refuse(String),
}

pub struct Supervisor {
    store: Arc<Mutex<Store>>,
    runner: Arc<dyn StageRunner>,
    gate: Arc<dyn GpuGate>,
    /// Held while a stage runs: at most one worker, hence one model process, at a time.
    running: Mutex<()>,
    /// Why each job is waiting, for the queue screen.
    waiting: Mutex<HashMap<String, String>>,
    listener: Mutex<Option<Listener>>,
    /// A stage that crashed this many times in a row is marked failed instead of retried.
    max_attempts: u32,
}

impl Supervisor {
    pub fn new(
        store: Arc<Mutex<Store>>,
        runner: Arc<dyn StageRunner>,
        gate: Arc<dyn GpuGate>,
        max_attempts: u32,
    ) -> Self {
        Self {
            store,
            runner,
            gate,
            running: Mutex::new(()),
            waiting: Mutex::new(HashMap::new()),
            listener: Mutex::new(None),
            max_attempts,
        }
    }

    pub fn set_listener(&self, listener: impl Fn(&QueueEvent) + Send + Sync + 'static) {
        if let Ok(mut l) = self.listener.lock() {
            *l = Some(Box::new(listener));
        }
    }

    fn emit(&self, event: QueueEvent) {
        if let Ok(l) = self.listener.lock() {
            if let Some(f) = l.as_ref() {
                f(&event);
            }
        }
    }

    fn store(&self) -> Result<MutexGuard<'_, Store>, QueueError> {
        self.store.lock().map_err(|_| QueueError::Poisoned)
    }

    /// Create a job for a meeting. Returns whether a new job was created (an existing job
    /// is left alone).
    pub fn enqueue(&self, meeting_id: &str, options: &str) -> Result<bool, QueueError> {
        let names: Vec<&str> = STAGES.iter().map(|s| s.as_str()).collect();
        Ok(self
            .store()?
            .create_job(meeting_id, &names, options, now_ms())?)
    }

    /// Run after a start: stages left `running` by a crash go back to pending, unless they
    /// already crashed `max_attempts` times, in which case they are marked failed so a
    /// crash loop cannot run forever. Returns the affected `(meeting, stage)` pairs.
    pub fn recover(&self) -> Result<Vec<(String, Stage)>, QueueError> {
        let store = self.store()?;
        let mut out = Vec::new();
        for (meeting_id, name) in store.reset_running_stages()? {
            let Some(stage) = Stage::parse(&name) else {
                continue;
            };
            let attempts = store
                .get_job(&meeting_id)?
                .and_then(|j| j.stages.into_iter().find(|s| s.stage == name))
                .map(|s| s.attempts)
                .unwrap_or(0);
            if attempts >= self.max_attempts {
                store.set_stage_status(
                    &meeting_id,
                    &name,
                    StageStatus::Failed,
                    Some("the worker stopped unexpectedly too many times"),
                    now_ms(),
                )?;
            }
            out.push((meeting_id, stage));
        }
        Ok(out)
    }

    /// Put a failed stage back to pending so the job can continue from it.
    pub fn retry(&self, meeting_id: &str) -> Result<(), QueueError> {
        let store = self.store()?;
        let job = store
            .get_job(meeting_id)?
            .ok_or_else(|| QueueError::NoJob(meeting_id.to_string()))?;
        for s in job
            .stages
            .iter()
            .filter(|s| s.status == StageStatus::Failed)
        {
            store.set_stage_status(meeting_id, &s.stage, StageStatus::Pending, None, now_ms())?;
        }
        Ok(())
    }

    /// Reason the job is waiting, when it is.
    pub fn waiting_reason(&self, meeting_id: &str) -> Option<String> {
        self.waiting.lock().ok()?.get(meeting_id).cloned()
    }

    fn set_waiting(&self, meeting_id: &str, reason: Option<String>) {
        if let Ok(mut w) = self.waiting.lock() {
            match reason {
                Some(r) => w.insert(meeting_id.to_string(), r),
                None => w.remove(meeting_id),
            };
        }
    }

    /// Decide what to do with `stage`. The language-model stages (`extract`, `reduce`) need a
    /// backend chosen in the job options and a cycle on the meeting; without either they are
    /// skipped, so old jobs and meetings without a cycle still finish. A cloud backend runs
    /// only when the store holds the user's confirmation for it (the worker refuses too).
    fn plan(&self, meeting_id: &str, job: &JobRow, stage: Stage) -> Result<Plan, QueueError> {
        let options = JobOptions::from_json(&job.options);
        let kind = options.llm.as_ref().map(|l| l.kind);
        if stage == Stage::Vision && options.skip_vision {
            return Ok(Plan::Skip("screenshot analysis is turned off".into()));
        }
        if !stage.uses_llm() {
            return Ok(Plan::Run(kind));
        }
        let Some(llm) = options.llm.as_ref() else {
            return Ok(Plan::Skip(
                "no language model backend was chosen for this meeting".into(),
            ));
        };
        let store = self.store()?;
        let settings = store.get_report_settings(meeting_id)?.unwrap_or_default();
        if settings.cycle_id.is_none() {
            return Ok(Plan::Skip("the meeting has no cycle".into()));
        }
        if llm.kind == LlmKind::Cloud
            && !confirmed_for_run(llm, store.get_llm_choice(meeting_id)?.as_ref())
        {
            return Ok(Plan::Refuse(
                "the cloud language model was not confirmed for this meeting".into(),
            ));
        }
        Ok(Plan::Run(kind))
    }

    /// Run at most one stage of the job.
    pub fn step(&self, meeting_id: &str) -> Result<StepOutcome, QueueError> {
        let job = self
            .store()?
            .get_job(meeting_id)?
            .ok_or_else(|| QueueError::NoJob(meeting_id.to_string()))?;
        let stage = match next_step(&job) {
            Next::Finished => {
                self.set_waiting(meeting_id, None);
                return Ok(StepOutcome::Finished);
            }
            Next::Failed { stage } => return Ok(StepOutcome::Blocked { stage }),
            Next::Run { stage } => stage,
        };

        let kind = match self.plan(meeting_id, &job, stage)? {
            Plan::Run(kind) => kind,
            Plan::Skip(note) => {
                self.set_waiting(meeting_id, None);
                self.store()?
                    .mark_stage_skipped(meeting_id, stage.as_str(), &note, now_ms())?;
                self.emit(QueueEvent::StageDone {
                    meeting_id: meeting_id.to_string(),
                    stage,
                });
                return Ok(StepOutcome::Ran { stage });
            }
            Plan::Refuse(error) => {
                self.set_waiting(meeting_id, None);
                self.store()?.set_stage_status(
                    meeting_id,
                    stage.as_str(),
                    StageStatus::Failed,
                    Some(&error),
                    now_ms(),
                )?;
                self.emit(QueueEvent::StageFailed {
                    meeting_id: meeting_id.to_string(),
                    stage,
                    error: error.clone(),
                });
                return Ok(StepOutcome::Failed { stage, error });
            }
        };

        let Ok(_guard) = self.running.try_lock() else {
            return Ok(StepOutcome::Busy);
        };

        if stage.needs_gpu(kind) {
            if let GateDecision::Wait(reason) = self.gate.check(stage) {
                self.set_waiting(meeting_id, Some(reason.clone()));
                self.emit(QueueEvent::Waiting {
                    meeting_id: meeting_id.to_string(),
                    stage,
                    reason: reason.clone(),
                });
                return Ok(StepOutcome::Waiting { stage, reason });
            }
        }
        self.set_waiting(meeting_id, None);

        self.store()?.set_stage_status(
            meeting_id,
            stage.as_str(),
            StageStatus::Running,
            None,
            now_ms(),
        )?;
        self.emit(QueueEvent::StageStarted {
            meeting_id: meeting_id.to_string(),
            stage,
        });

        tracing::info!(
            meeting = meeting_id,
            stage = stage.as_str(),
            "stage started"
        );
        // A panic in the runner must not leave the stage `running` for good: it ends the
        // stage as failed. Its text can hold anything the stage was handling, so only the
        // fact is recorded (the panic hook logs where it happened).
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.runner.run(meeting_id, stage)
        }))
        .unwrap_or_else(|_| {
            Err(RunError::new(
                "an internal error stopped this stage; retry it, and report it if it keeps happening",
            ))
        });
        match result {
            Ok(()) => {
                self.store()?.set_stage_status(
                    meeting_id,
                    stage.as_str(),
                    StageStatus::Done,
                    None,
                    now_ms(),
                )?;
                self.emit(QueueEvent::StageDone {
                    meeting_id: meeting_id.to_string(),
                    stage,
                });
                Ok(StepOutcome::Ran { stage })
            }
            Err(e) => {
                self.store()?.set_stage_status(
                    meeting_id,
                    stage.as_str(),
                    StageStatus::Failed,
                    Some(&e.message),
                    now_ms(),
                )?;
                self.emit(QueueEvent::StageFailed {
                    meeting_id: meeting_id.to_string(),
                    stage,
                    error: e.message.clone(),
                });
                Ok(StepOutcome::Failed {
                    stage,
                    error: e.message,
                })
            }
        }
    }

    /// Step until the job finishes, fails or has to wait. The caller decides when to try
    /// again after a wait.
    pub fn drive(&self, meeting_id: &str) -> Result<StepOutcome, QueueError> {
        loop {
            match self.step(meeting_id)? {
                StepOutcome::Ran { .. } => continue,
                StepOutcome::Finished => {
                    self.emit(QueueEvent::Finished {
                        meeting_id: meeting_id.to_string(),
                    });
                    return Ok(StepOutcome::Finished);
                }
                other => return Ok(other),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::LlmOptions;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use store::{Meeting, MeetingState};

    struct Recording {
        calls: Mutex<Vec<Stage>>,
        fail_on: Mutex<Option<Stage>>,
    }

    impl Recording {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                fail_on: Mutex::new(None),
            })
        }
    }

    impl StageRunner for Recording {
        fn run(&self, _meeting_id: &str, stage: Stage) -> Result<(), RunError> {
            self.calls.lock().unwrap().push(stage);
            if *self.fail_on.lock().unwrap() == Some(stage) {
                return Err(RunError::new("boom"));
            }
            Ok(())
        }
    }

    struct Gate(AtomicBool);

    impl GpuGate for Gate {
        fn check(&self, _stage: Stage) -> GateDecision {
            if self.0.load(Ordering::SeqCst) {
                GateDecision::Go
            } else {
                GateDecision::Wait("GPU busy".into())
            }
        }
    }

    /// Options that choose a backend for the language-model stages.
    fn llm_options(kind: LlmKind) -> String {
        serde_json::to_string(&JobOptions {
            llm: Some(LlmOptions {
                kind,
                model_path: None,
                ctx: None,
                base_url: Some("https://api.example.com/v1".into()),
                model: None,
                // A claim in the stored options counts for nothing.
                cloud_confirmed: true,
            }),
            ..JobOptions::default()
        })
        .unwrap()
    }

    /// A gate that records which stages it was asked about.
    struct Spy(Mutex<Vec<Stage>>);

    impl GpuGate for Spy {
        fn check(&self, stage: Stage) -> GateDecision {
            self.0.lock().unwrap().push(stage);
            GateDecision::Go
        }
    }

    fn setup_with(options: &str, cycle: Option<&str>) -> (Supervisor, Arc<Recording>, Arc<Spy>) {
        let store = Store::open_in_memory().unwrap();
        store
            .create_meeting(&Meeting {
                id: "m1".into(),
                title: "t".into(),
                started_unix_ms: 1,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: "m1".into(),
            })
            .unwrap();
        store
            .set_report_settings(
                "m1",
                &store::ReportSettings {
                    cycle_id: cycle.map(str::to_string),
                    ..Default::default()
                },
            )
            .unwrap();
        let runner = Recording::new();
        let gate = Arc::new(Spy(Mutex::new(Vec::new())));
        let sup = Supervisor::new(Arc::new(Mutex::new(store)), runner.clone(), gate.clone(), 3);
        sup.enqueue("m1", options).unwrap();
        (sup, runner, gate)
    }

    fn stage_row(sup: &Supervisor, stage: Stage) -> store::StageRow {
        let job = sup.store().unwrap().get_job("m1").unwrap().unwrap();
        job.stages
            .into_iter()
            .find(|s| s.stage == stage.as_str())
            .unwrap()
    }

    fn setup(gpu_free: bool) -> (Supervisor, Arc<Recording>, Arc<Gate>) {
        let store = Store::open_in_memory().unwrap();
        store
            .create_meeting(&Meeting {
                id: "m1".into(),
                title: "t".into(),
                started_unix_ms: 1,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: "m1".into(),
            })
            .unwrap();
        let runner = Recording::new();
        let gate = Arc::new(Gate(AtomicBool::new(gpu_free)));
        let sup = Supervisor::new(Arc::new(Mutex::new(store)), runner.clone(), gate.clone(), 3);
        sup.enqueue("m1", "{}").unwrap();
        (sup, runner, gate)
    }

    struct Panicking;

    impl StageRunner for Panicking {
        fn run(&self, _meeting_id: &str, _stage: Stage) -> Result<(), RunError> {
            panic!("secret-token-123 in the panic text");
        }
    }

    #[test]
    fn a_panic_in_the_runner_fails_the_stage_instead_of_leaving_it_running() {
        let (sup, _, gate) = setup(true);
        let sup = Supervisor::new(sup.store.clone(), Arc::new(Panicking), gate, 3);
        let outcome = sup.step("m1").unwrap();
        let StepOutcome::Failed { stage, error } = outcome else {
            panic!("expected a failed stage, got {outcome:?}");
        };
        assert_eq!(stage, Stage::Asr);
        assert!(!error.contains("secret-token"), "the panic text leaked");
        let row = stage_row(&sup, Stage::Asr);
        assert_eq!(row.status, StageStatus::Failed);
        assert_eq!(row.error.as_deref(), Some(error.as_str()));
        // The stage can be retried like any failed one, and the runner lock was released.
        sup.retry("m1").unwrap();
        assert_eq!(stage_row(&sup, Stage::Asr).status, StageStatus::Pending);
        assert!(sup.running.try_lock().is_ok());
    }

    #[test]
    fn drive_runs_every_stage_in_order() {
        let (sup, runner, _) = setup_with(&llm_options(LlmKind::Managed), Some("TEAM"));
        assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
        assert_eq!(*runner.calls.lock().unwrap(), STAGES.to_vec());
        assert_eq!(stage_row(&sup, Stage::Extract).note, None);
    }

    #[test]
    fn a_job_without_a_backend_skips_the_language_model_stages_and_finishes() {
        // The shape of every job made before cycles existed.
        let (sup, runner, _) = setup(true);
        assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
        assert_eq!(*runner.calls.lock().unwrap(), STAGES[..7].to_vec());
        for stage in [Stage::Extract, Stage::Reduce] {
            let row = stage_row(&sup, stage);
            assert_eq!(row.status, StageStatus::Done);
            assert!(row.note.as_deref().unwrap().contains("backend"));
            assert_eq!(row.error, None);
            assert_eq!(row.attempts, 0);
        }
    }

    #[test]
    fn turning_screenshot_analysis_off_skips_only_the_vision_stage_and_the_job_finishes() {
        let options = serde_json::to_string(&JobOptions {
            skip_vision: true,
            ..JobOptions::default()
        })
        .unwrap();
        let (sup, runner, gate) = setup_with(&options, None);
        assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
        let ran = runner.calls.lock().unwrap().clone();
        assert!(!ran.contains(&Stage::Vision), "{ran:?}");
        // Every other stage before the language-model ones still ran.
        assert_eq!(
            ran,
            STAGES[..7]
                .iter()
                .copied()
                .filter(|s| *s != Stage::Vision)
                .collect::<Vec<_>>()
        );
        let row = stage_row(&sup, Stage::Vision);
        assert_eq!(row.status, StageStatus::Done);
        assert_eq!(
            row.note.as_deref(),
            Some("screenshot analysis is turned off")
        );
        assert_eq!((row.error, row.attempts), (None, 0));
        // A skipped stage never waits for the GPU.
        assert!(!gate.0.lock().unwrap().contains(&Stage::Vision));
    }

    #[test]
    fn the_vision_stage_runs_unless_it_was_turned_off_and_old_options_still_load() {
        let (sup, runner, _) = setup_with("{}", None);
        assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
        assert!(runner.calls.lock().unwrap().contains(&Stage::Vision));
        assert!(!JobOptions::from_json(r#"{"language":"pl"}"#).skip_vision);
        // The flag is left out of the manifest options while it is off.
        let json = serde_json::to_value(JobOptions::default()).unwrap();
        assert!(json.get("skip_vision").is_none());
        let json = serde_json::to_value(JobOptions {
            skip_vision: true,
            ..JobOptions::default()
        })
        .unwrap();
        assert_eq!(json["skip_vision"], true);
    }

    #[test]
    fn a_meeting_without_a_cycle_skips_the_language_model_stages() {
        let (sup, runner, gate) = setup_with(&llm_options(LlmKind::Managed), None);
        assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
        assert_eq!(*runner.calls.lock().unwrap(), STAGES[..7].to_vec());
        assert_eq!(
            stage_row(&sup, Stage::Reduce).note.as_deref(),
            Some("the meeting has no cycle")
        );
        // A skipped stage never waits for the GPU.
        assert!(!gate.0.lock().unwrap().contains(&Stage::Extract));
    }

    #[test]
    fn only_the_managed_backend_goes_through_the_gpu_gate() {
        for (kind, gated) in [(LlmKind::Managed, true), (LlmKind::External, false)] {
            let (sup, runner, gate) = setup_with(&llm_options(kind), Some("TEAM"));
            assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
            assert_eq!(*runner.calls.lock().unwrap(), STAGES.to_vec());
            let asked = gate.0.lock().unwrap().clone();
            assert_eq!(asked.contains(&Stage::Extract), gated, "{kind:?}");
            assert_eq!(asked.contains(&Stage::Reduce), gated, "{kind:?}");
            // The speech stages are gated whatever the language-model backend.
            assert!(asked.contains(&Stage::Asr));
        }
    }

    #[test]
    fn an_external_backend_runs_while_the_gpu_is_busy() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_meeting(&Meeting {
                id: "m1".into(),
                title: "t".into(),
                started_unix_ms: 1,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: "m1".into(),
            })
            .unwrap();
        store
            .set_report_settings(
                "m1",
                &store::ReportSettings {
                    cycle_id: Some("TEAM".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let runner = Recording::new();
        let gate = Arc::new(Gate(AtomicBool::new(false)));
        let sup = Supervisor::new(Arc::new(Mutex::new(store)), runner.clone(), gate, 3);
        sup.enqueue("m1", &llm_options(LlmKind::External)).unwrap();
        for stage in &STAGES[..7] {
            sup.store()
                .unwrap()
                .set_stage_status("m1", stage.as_str(), StageStatus::Done, None, 1)
                .unwrap();
        }
        assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
        assert_eq!(
            *runner.calls.lock().unwrap(),
            vec![Stage::Extract, Stage::Reduce]
        );
    }

    #[test]
    fn the_cloud_runs_only_after_the_store_holds_a_confirmation() {
        let (sup, runner, _) = setup_with(&llm_options(LlmKind::Cloud), Some("TEAM"));
        for stage in &STAGES[..7] {
            sup.store()
                .unwrap()
                .set_stage_status("m1", stage.as_str(), StageStatus::Done, None, 1)
                .unwrap();
        }
        // The stored options claim `cloud_confirmed`, the store says nothing: refused.
        let out = sup.drive("m1").unwrap();
        assert!(
            matches!(&out, StepOutcome::Failed { stage: Stage::Extract, error } if error.contains("not confirmed")),
            "{out:?}"
        );
        assert!(runner.calls.lock().unwrap().is_empty());

        // A choice that is not confirmed is refused as well.
        sup.store()
            .unwrap()
            .set_llm_choice("m1", LlmKind::Cloud, Some("api.example.com"))
            .unwrap();
        sup.retry("m1").unwrap();
        assert!(matches!(
            sup.drive("m1").unwrap(),
            StepOutcome::Failed { .. }
        ));
        assert!(runner.calls.lock().unwrap().is_empty());

        // Confirmed for another host than the options name: refused.
        sup.store()
            .unwrap()
            .set_llm_choice("m1", LlmKind::Cloud, Some("evil.example.net"))
            .unwrap();
        sup.store().unwrap().confirm_cloud("m1", 5).unwrap();
        sup.retry("m1").unwrap();
        assert!(matches!(
            sup.drive("m1").unwrap(),
            StepOutcome::Failed { .. }
        ));
        assert!(runner.calls.lock().unwrap().is_empty());

        // Confirmed for the endpoint of the job: runs.
        sup.store()
            .unwrap()
            .set_llm_choice("m1", LlmKind::Cloud, Some("api.example.com"))
            .unwrap();
        sup.store().unwrap().confirm_cloud("m1", 6).unwrap();
        sup.retry("m1").unwrap();
        assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
        assert_eq!(
            *runner.calls.lock().unwrap(),
            vec![Stage::Extract, Stage::Reduce]
        );
    }

    #[test]
    fn busy_gpu_keeps_the_job_waiting_until_it_is_free() {
        let (sup, runner, gate) = setup(false);
        let out = sup.drive("m1").unwrap();
        assert_eq!(
            out,
            StepOutcome::Waiting {
                stage: Stage::Asr,
                reason: "GPU busy".into()
            }
        );
        assert!(runner.calls.lock().unwrap().is_empty());
        assert_eq!(sup.waiting_reason("m1").as_deref(), Some("GPU busy"));
        gate.0.store(true, Ordering::SeqCst);
        assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
        assert_eq!(sup.waiting_reason("m1"), None);
    }

    #[test]
    fn failure_blocks_the_job_and_retry_resumes_from_that_stage() {
        let (sup, runner, _) = setup(true);
        *runner.fail_on.lock().unwrap() = Some(Stage::Diarize);
        assert!(matches!(
            sup.drive("m1").unwrap(),
            StepOutcome::Failed {
                stage: Stage::Diarize,
                ..
            }
        ));
        assert_eq!(
            sup.step("m1").unwrap(),
            StepOutcome::Blocked {
                stage: Stage::Diarize
            }
        );
        *runner.fail_on.lock().unwrap() = None;
        sup.retry("m1").unwrap();
        runner.calls.lock().unwrap().clear();
        assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
        // ASR was already done and is not run again.
        assert_eq!(runner.calls.lock().unwrap()[0], Stage::Diarize);
    }

    #[test]
    fn recover_resumes_an_interrupted_stage() {
        let (sup, _, _) = setup(true);
        sup.store()
            .unwrap()
            .set_stage_status("m1", "asr", StageStatus::Running, None, 5)
            .unwrap();
        assert_eq!(sup.recover().unwrap(), vec![("m1".to_string(), Stage::Asr)]);
        let job = sup.store().unwrap().get_job("m1").unwrap().unwrap();
        assert_eq!(job.stages[0].status, StageStatus::Pending);
    }

    #[test]
    fn crash_loop_ends_in_a_failed_stage() {
        let (sup, _, _) = setup(true);
        for _ in 0..3 {
            sup.store()
                .unwrap()
                .set_stage_status("m1", "asr", StageStatus::Running, None, 5)
                .unwrap();
            sup.recover().unwrap();
        }
        let job = sup.store().unwrap().get_job("m1").unwrap().unwrap();
        assert_eq!(job.stages[0].status, StageStatus::Failed);
    }

    #[test]
    fn only_one_stage_runs_at_a_time() {
        struct Slow {
            peak: AtomicUsize,
            now: AtomicUsize,
            sup: Mutex<Option<Arc<Supervisor>>>,
            nested: Mutex<Vec<StepOutcome>>,
        }
        impl StageRunner for Slow {
            fn run(&self, _m: &str, _s: Stage) -> Result<(), RunError> {
                let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(n, Ordering::SeqCst);
                // While this stage runs, a second request must be refused.
                if let Some(sup) = self.sup.lock().unwrap().clone() {
                    self.nested.lock().unwrap().push(sup.step("m2").unwrap());
                }
                self.now.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            }
        }
        let store = Store::open_in_memory().unwrap();
        for id in ["m1", "m2"] {
            store
                .create_meeting(&Meeting {
                    id: id.into(),
                    title: "t".into(),
                    started_unix_ms: 1,
                    duration_ms: None,
                    state: MeetingState::Stopped,
                    dir: id.into(),
                })
                .unwrap();
        }
        let runner = Arc::new(Slow {
            peak: AtomicUsize::new(0),
            now: AtomicUsize::new(0),
            sup: Mutex::new(None),
            nested: Mutex::new(Vec::new()),
        });
        let sup = Arc::new(Supervisor::new(
            Arc::new(Mutex::new(store)),
            runner.clone(),
            Arc::new(Gate(AtomicBool::new(true))),
            3,
        ));
        sup.enqueue("m1", "{}").unwrap();
        sup.enqueue("m2", "{}").unwrap();
        *runner.sup.lock().unwrap() = Some(sup.clone());
        sup.step("m1").unwrap();
        assert_eq!(runner.peak.load(Ordering::SeqCst), 1);
        assert_eq!(runner.nested.lock().unwrap()[0], StepOutcome::Busy);
    }
}
