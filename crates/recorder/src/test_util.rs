//! Helpers shared by the unit tests.

use std::path::Path;

/// A temporary directory under `target/test-tmp`, removed when dropped.
pub struct TempDir(test_tmp::TestDir);

impl TempDir {
    /// Create a new empty directory under `target/test-tmp`.
    pub fn new(label: &str) -> Self {
        Self(test_tmp::TestDir::new(&format!("recorder-{label}")))
    }

    pub fn path(&self) -> &Path {
        self.0.path()
    }
}
