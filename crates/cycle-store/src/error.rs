use std::fmt;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One problem found while validating a file or a document, with the location of the
/// offending value (for example `participants[1].id`; empty for the document itself).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

impl ValidationIssue {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// Renders issues as one line for error messages.
pub fn join_issues(issues: &[ValidationIssue]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug, thiserror::Error)]
pub enum CycleError {
    #[error("I/O error at {path}: {source}")]
    Io { path: String, source: io::Error },
    #[error("invalid {what}: {}", join_issues(.issues))]
    Invalid {
        what: String,
        issues: Vec<ValidationIssue>,
    },
    #[error("git is not available: {0}")]
    GitUnavailable(io::Error),
    #[error("git {command} failed: {message}")]
    Git { command: String, message: String },
    #[error("cycle {0} already exists")]
    CycleExists(String),
    #[error("cycle {0} was not found")]
    CycleNotFound(String),
    #[error("template {0} was not found")]
    TemplateNotFound(String),
    #[error(
        "invalid name {0:?}: use letters, digits, '_' and '-', starting with a letter or digit"
    )]
    InvalidName(String),
    #[error("{0}")]
    Other(String),
}

impl CycleError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }

    pub(crate) fn invalid(what: impl Into<String>, issues: Vec<ValidationIssue>) -> Self {
        Self::Invalid {
            what: what.into(),
            issues,
        }
    }

    /// The validation issues when this error is [`CycleError::Invalid`].
    pub fn issues(&self) -> &[ValidationIssue] {
        match self {
            Self::Invalid { issues, .. } => issues,
            _ => &[],
        }
    }
}

pub type Result<T> = std::result::Result<T, CycleError>;
