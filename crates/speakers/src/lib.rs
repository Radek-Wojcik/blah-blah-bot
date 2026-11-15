//! Voice recognition logic that needs no model, database or audio: similarity between
//! embeddings, matching against profiles, profile updates and the choice of sample clips.
//!
//! Biometric data handled here never leaves the machine: this crate has no I/O at all.

mod clips;
mod matching;
mod profile;

pub use clips::{select_clips, select_clips_with, ClipParams, ClipSpan, Turn};
pub use matching::{
    assign_unique, classify, cosine, normalize, Candidate, Decision, ProfileRef, Thresholds,
};
pub use profile::{centroid, AddOutcome, Sample, SampleSet, MAX_SAMPLES, MIN_SAMPLE_MS};
