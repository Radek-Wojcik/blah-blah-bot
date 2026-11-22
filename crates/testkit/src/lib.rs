//! Helpers shared by the tests of every crate. Used as a dev-dependency only.
//!
//! Test data never goes to the system temporary folder: every temporary folder lives under
//! the build output folder (`target/tmp`), so a failed or killed run leaves nothing behind in
//! the user profile and `cargo clean` removes whatever is left.

use std::path::PathBuf;

/// Folder that holds the temporary folders of tests.
///
/// Taken from `CARGO_TARGET_TMPDIR` or `CARGO_TARGET_DIR` when set at run time, otherwise
/// `target/tmp` of the workspace this crate belongs to.
pub fn target_tmp_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_TMPDIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir).join("tmp");
    }
    // crates/testkit -> workspace root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("tmp")
}

/// A new empty folder under [`target_tmp_root`] whose name starts with `prefix`. It is
/// removed when the returned value is dropped, also while a failed test unwinds.
///
/// # Panics
///
/// When the folder cannot be created: a test cannot run without it.
pub fn target_tempdir(prefix: &str) -> tempfile::TempDir {
    let root = target_tmp_root();
    std::fs::create_dir_all(&root)
        .unwrap_or_else(|e| panic!("cannot create {}: {e}", root.display()));
    tempfile::Builder::new()
        .prefix(&format!("{prefix}-"))
        .tempdir_in(&root)
        .unwrap_or_else(|e| {
            panic!(
                "cannot create a temporary folder in {}: {e}",
                root.display()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_live_under_target_and_are_removed() {
        let dir = target_tempdir("testkit");
        let path = dir.path().to_path_buf();
        assert!(path.is_dir());
        let canonical = path.canonicalize().unwrap();
        let root = target_tmp_root().canonicalize().unwrap();
        assert!(
            canonical.starts_with(&root),
            "{canonical:?} not under {root:?}"
        );
        assert!(!canonical.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("testkit-"));
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn a_panicking_test_still_removes_its_folder() {
        let mut seen = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let dir = target_tempdir("testkit-panic");
            seen = Some(dir.path().to_path_buf());
            panic!("test failure");
        }));
        assert!(result.is_err());
        assert!(!seen.unwrap().exists());
    }
}
