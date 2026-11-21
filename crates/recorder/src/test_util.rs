//! Helpers shared by the unit tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A temporary directory that is removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Create a new empty directory under the system temp directory.
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        // The `bbb-` prefix lets scripts/check-temp-hygiene.ps1 find a folder left by a killed run.
        let dir = std::env::temp_dir().join(format!(
            "bbb-recorder-test-{label}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
