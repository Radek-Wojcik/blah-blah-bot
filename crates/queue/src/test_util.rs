//! Test helper: a scratch folder that is removed when the value is dropped.

use std::ops::Deref;
use std::path::Path;

/// A folder under the system temp directory, removed on drop (also while a test panics).
/// Hold it for the whole test.
pub struct TestDir(tempfile::TempDir);

impl TestDir {
    pub fn new(label: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix(&format!("bbb-{label}-"))
            .tempdir()
            .unwrap();
        TestDir(dir)
    }
}

impl Deref for TestDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.0.path()
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}
