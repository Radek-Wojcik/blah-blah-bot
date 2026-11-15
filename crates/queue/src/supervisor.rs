//! Runs stages one at a time and records every transition.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use store::{StageStatus, Store};

use crate::stage::{next_step, Next, Stage, STAGES};

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

        let Ok(_guard) = self.running.try_lock() else {
            return Ok(StepOutcome::Busy);
        };

        if stage.needs_gpu() {
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

        match self.runner.run(meeting_id, stage) {
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

    #[test]
    fn drive_runs_every_stage_in_order() {
        let (sup, runner, _) = setup(true);
        assert_eq!(sup.drive("m1").unwrap(), StepOutcome::Finished);
        assert_eq!(*runner.calls.lock().unwrap(), STAGES.to_vec());
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
