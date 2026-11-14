//! Error type of the recorder crate.

/// Everything that can go wrong in the recorder.
#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    /// A file or directory operation failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// A journal entry could not be encoded or decoded.
    #[error("journal entry error: {0}")]
    Json(#[from] serde_json::Error),
    /// A WAVE file does not have the expected layout.
    #[error("invalid WAVE file: {0}")]
    InvalidWav(String),
    /// A caller-supplied value is not usable.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// A lock shared between threads was poisoned by a panic.
    #[error("{0} lock poisoned")]
    LockPoisoned(&'static str),
    /// A call into the operating system audio stack failed.
    #[error("{context}: {message}")]
    Audio {
        /// What the recorder was doing.
        context: &'static str,
        /// Message reported by the system.
        message: String,
    },
}

impl RecorderError {
    /// Build an [`RecorderError::Audio`] from a context and any displayable error.
    pub fn audio(context: &'static str, error: impl std::fmt::Display) -> Self {
        RecorderError::Audio {
            context,
            message: error.to_string(),
        }
    }
}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, RecorderError>;
