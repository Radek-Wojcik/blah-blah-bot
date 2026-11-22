//! Scratch folders for tests.
//!
//! A [`TestDir`] lives under `<workspace>/target/test-tmp` (never in the temp directory of the
//! user, however the tests are started), is named `bbb-<label>-XXXXXX` so that
//! `scripts/check-temp-hygiene.ps1` can find a folder a killed run left behind, and is removed
//! when the value is dropped, also while a test panics. Hold it for the whole test.
//!
//! Git writes its object files read-only and Windows refuses to delete those, which silently
//! left whole repositories behind when only `tempfile::TempDir` removed them. The drop clears the
//! read-only attributes first, retries a few times (a process may still be letting go of a
//! file) and then fails the test, naming the folder, instead of leaving it behind quietly.

use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often the removal is tried before it counts as failed.
const ATTEMPTS: u32 = 5;

/// A scratch folder removed on drop. Dereferences to its [`Path`].
pub struct TestDir {
    dir: tempfile::TempDir,
}

/// The folder all scratch folders of the tests are created in.
pub fn root() -> PathBuf {
    // crates/test-tmp -> crates -> the workspace root.
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the workspace root")
        .to_path_buf();
    let root = workspace.join("target").join("test-tmp");
    std::fs::create_dir_all(&root).expect("create target/test-tmp");
    root
}

impl TestDir {
    /// A new empty folder; `label` says what it is for (letters, digits and dashes).
    pub fn new(label: &str) -> TestDir {
        let dir = tempfile::Builder::new()
            .prefix(&format!("bbb-{label}-"))
            .tempdir_in(root())
            .expect("create a scratch folder in target/test-tmp");
        TestDir { dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Deref for TestDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.dir.path()
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let path = self.dir.path().to_path_buf();
        if let Err(error) = remove_tree(&path) {
            let message = format!(
                "the scratch folder {} could not be removed: {error}",
                path.display()
            );
            if std::thread::panicking() {
                // A second panic would abort the whole test run; the test fails already.
                eprintln!("{message}");
            } else {
                panic!("{message}");
            }
        }
    }
}

/// Removes `path` with everything in it, also read-only files; the error names the cause.
pub fn remove_tree(path: &Path) -> io::Result<()> {
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        clear_read_only(path);
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => last = Some(e),
        }
        if attempt + 1 < ATTEMPTS {
            std::thread::sleep(Duration::from_millis(200 * u64::from(attempt + 1)));
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("unknown")))
}

/// Clears the read-only attribute of every file and folder under `path`.
fn clear_read_only(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    let mut permissions = meta.permissions();
    if permissions.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(path, permissions);
    }
    if meta.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                clear_read_only(&entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_with_read_only_files_is_removed_on_drop() {
        let dir = TestDir::new("selftest");
        let path = dir.path().to_path_buf();
        let nested = path.join("objects").join("ab");
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("0123");
        std::fs::write(&file, b"object").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();
        assert!(path.starts_with(root()));
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn the_folder_name_carries_the_hygiene_prefix() {
        let dir = TestDir::new("selftest");
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("bbb-selftest-"), "{name}");
    }
}
