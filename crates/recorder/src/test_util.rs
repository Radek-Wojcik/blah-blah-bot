//! Helpers shared by the unit tests.

use std::path::Path;

/// A temporary directory under the build output folder, removed when dropped.
pub struct TempDir(tempfile::TempDir);

impl TempDir {
    /// Create a new empty directory under `target/tmp`.
    pub fn new(label: &str) -> Self {
        Self(testkit::target_tempdir(&format!("recorder-{label}")))
    }

    pub fn path(&self) -> &Path {
        self.0.path()
    }
}
