//! Persistent processing queue.
//!
//! One job per meeting, split into resumable stages (see [`stage`]). The [`Supervisor`]
//! runs at most one stage at a time, asks a [`GpuGate`] before every stage that needs the
//! GPU, and records every transition in the store, so a crash resumes from the stage that
//! was interrupted.

pub mod cleanup;
pub mod context;
pub mod draft;
pub mod gpu;
pub mod runner;
pub mod stage;
mod supervisor;
pub mod transcript;
pub mod voices;
pub mod worker;

pub use stage::{audio_is_expendable, next_step, progress, Next, Stage, AUDIO_DEPENDENT, STAGES};
pub use supervisor::{
    now_ms, GateDecision, GpuGate, QueueError, QueueEvent, RunError, StageRunner, StepOutcome,
    Supervisor,
};
