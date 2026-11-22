//! Recorder core: capture of the microphone and of individual applications, segmented
//! writing of every track, and an append-only session journal that survives crashes.
//!
//! Every track is stored as 16 kHz mono 16-bit PCM in fixed-length WAVE segments
//! (`seg-NNNNN.wav`). All tracks share one session clock, so a track that starts later only
//! needs a start offset on that clock.
//!
//! The modules that contain only logic (`timeline`, `journal`, `track`, `tracker`, `drift`,
//! `push`, `wav`, `convert`) build and are tested on every platform. The modules that talk to the
//! Windows audio stack (`capture`, `sessions`, `sysutil`) exist on Windows only.

pub mod convert;
pub mod drift;
pub mod error;
pub mod journal;
pub mod push;
pub mod recovery;
pub mod screenshot;
pub mod source_state;
pub mod timeline;
pub mod track;
pub mod tracker;
pub mod wav;

#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod session;
#[cfg(windows)]
pub mod sessions;
#[cfg(windows)]
pub mod sysutil;

#[cfg(test)]
mod test_util;

pub use error::{RecorderError, Result};
