//! Build-time guard that ensures release/bundle builds include the staged worker payload
//! (`worker/pyproject.toml`, `worker/uv.lock`) and bundled `uv` executable in `bundle.resources`.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Returns `true` when the bundle resource guard must run: any build with `TAURI_CONFIG` set
/// or with `BBB_INSTALLER_BUILD=1`. Plain `cargo run --release` does not enforce staging.
pub fn should_enforce(tauri_config: Option<&str>, installer_build: Option<&str>) -> bool {
    tauri_config.is_some_and(|v| !v.trim().is_empty())
        || installer_build.is_some_and(|v| v.trim() == "1")
}

fn normalize_rel(raw: &str) -> String {
    let mut s = raw.trim().replace('\\', "/");
    while let Some(stripped) = s.strip_suffix("/**/*") {
        s = stripped.to_string();
    }
    while let Some(stripped) = s.strip_suffix("/*") {
        s = stripped.to_string();
    }
    s.trim_end_matches('/').to_string()
}

fn resolve_src(manifest_dir: &Path, src: &str) -> PathBuf {
    let normalized = normalize_rel(src);
    let p = Path::new(&normalized);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        manifest_dir.join(p)
    }
}

/// Checks that `config_json` defines `bundle.resources` containing both:
/// - the worker payload (`worker/pyproject.toml` and `worker/uv.lock` present on disk), and
/// - the bundled `uv` binary (`bin/uv.exe` or `bin/uv` present on disk).
///
/// Accepts both Tauri v2 map form (`{ "<src>": "<dst>" }`) and array form (`["<src>", ...]`).
pub fn check_bundle_resources(config_json: &str, manifest_dir: &Path) -> Result<(), String> {
    let root: Value =
        serde_json::from_str(config_json).map_err(|e| format!("invalid Tauri config JSON: {e}"))?;
    let resources = root
        .get("bundle")
        .and_then(|b| b.get("resources"))
        .ok_or_else(|| {
            "bundle.resources is missing; worker/ and bin/uv are not bundled".to_string()
        })?;

    let mut has_pyproject = false;
    let mut has_uv_lock = false;
    let mut has_uv_bin = false;
    let mut problems: Vec<String> = Vec::new();

    let mut check_entry = |src: &str, dst: Option<&str>| {
        let src_path = resolve_src(manifest_dir, src);
        let dst_norm = dst.map(normalize_rel);
        let src_norm = normalize_rel(src);

        let is_worker_dir = dst_norm.as_deref() == Some("worker")
            || (dst_norm.is_none() && (src_norm == "worker" || src_norm.ends_with("/worker")));
        if is_worker_dir {
            let pyproject = src_path.join("pyproject.toml");
            let uv_lock = src_path.join("uv.lock");
            if pyproject.is_file() {
                has_pyproject = true;
            } else {
                problems.push(format!(
                    "missing worker/pyproject.toml at {}",
                    pyproject.display()
                ));
            }
            if uv_lock.is_file() {
                has_uv_lock = true;
            } else {
                problems.push(format!("missing worker/uv.lock at {}", uv_lock.display()));
            }
        }

        let is_pyproject_file = dst_norm.as_deref() == Some("worker/pyproject.toml")
            || (dst_norm.is_none() && src_norm.ends_with("worker/pyproject.toml"));
        if is_pyproject_file {
            if src_path.is_file() {
                has_pyproject = true;
            } else {
                problems.push(format!(
                    "missing worker/pyproject.toml at {}",
                    src_path.display()
                ));
            }
        }

        let is_uv_lock_file = dst_norm.as_deref() == Some("worker/uv.lock")
            || (dst_norm.is_none() && src_norm.ends_with("worker/uv.lock"));
        if is_uv_lock_file {
            if src_path.is_file() {
                has_uv_lock = true;
            } else {
                problems.push(format!("missing worker/uv.lock at {}", src_path.display()));
            }
        }

        let is_bin_dir = dst_norm.as_deref() == Some("bin")
            || (dst_norm.is_none() && (src_norm == "bin" || src_norm.ends_with("/bin")));
        if is_bin_dir {
            let uv_exe = src_path.join("uv.exe");
            let uv_unix = src_path.join("uv");
            if uv_exe.is_file() || uv_unix.is_file() {
                has_uv_bin = true;
            } else {
                problems.push(format!(
                    "missing bundled uv executable in {}",
                    src_path.display()
                ));
            }
        }

        let is_uv_file = matches!(dst_norm.as_deref(), Some("bin/uv.exe" | "bin/uv"))
            || (dst_norm.is_none()
                && (src_norm.ends_with("bin/uv.exe") || src_norm.ends_with("bin/uv")));
        if is_uv_file {
            if src_path.is_file() {
                has_uv_bin = true;
            } else {
                problems.push(format!("missing bundled uv at {}", src_path.display()));
            }
        }
    };

    match resources {
        Value::Object(map) => {
            for (src, dst_val) in map {
                let dst = dst_val.as_str();
                check_entry(src, dst);
            }
        }
        Value::Array(items) => {
            for item in items {
                if let Some(src) = item.as_str() {
                    check_entry(src, None);
                }
            }
        }
        _ => {
            return Err("bundle.resources must be a JSON object or array".to_string());
        }
    }

    if !has_pyproject {
        problems.push("bundle.resources does not include worker/pyproject.toml".to_string());
    }
    if !has_uv_lock {
        problems.push("bundle.resources does not include worker/uv.lock".to_string());
    }
    if !has_uv_bin {
        problems.push("bundle.resources does not include bin/uv(.exe)".to_string());
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("bbb-bundle-guard-{name}-"))
            .tempdir()
            .unwrap()
    }

    fn stage_complete(root: &Path) {
        let worker = root.join("stage/worker");
        let bin = root.join("stage/bin");
        fs::create_dir_all(&worker).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(worker.join("pyproject.toml"), "[project]\nname = \"w\"\n").unwrap();
        fs::write(worker.join("uv.lock"), "version = 1\n").unwrap();
        fs::write(bin.join("uv.exe"), b"MZ").unwrap();
    }

    #[test]
    fn bundle_guard_accepts_valid_map_resources() {
        let d = scratch("map-ok");
        stage_complete(d.path());
        let cfg = r#"{
            "bundle": {
                "resources": {
                    "stage/worker": "worker",
                    "stage/bin": "bin"
                }
            }
        }"#;
        assert!(check_bundle_resources(cfg, d.path()).is_ok());
    }

    #[test]
    fn bundle_guard_accepts_valid_array_resources() {
        let d = scratch("arr-ok");
        stage_complete(d.path());
        let cfg = r#"{
            "bundle": {
                "resources": [
                    "stage/worker",
                    "stage/bin"
                ]
            }
        }"#;
        assert!(check_bundle_resources(cfg, d.path()).is_ok());
    }

    #[test]
    fn bundle_guard_rejects_missing_resources_section() {
        let d = scratch("no-res");
        let err = check_bundle_resources(r#"{"bundle":{"active":true}}"#, d.path()).unwrap_err();
        assert!(err.contains("bundle.resources"), "{err}");
    }

    #[test]
    fn bundle_guard_rejects_missing_worker_files() {
        let d = scratch("no-lock");
        stage_complete(d.path());
        fs::remove_file(d.path().join("stage/worker/uv.lock")).unwrap();
        let cfg = r#"{
            "bundle": {
                "resources": {
                    "stage/worker": "worker",
                    "stage/bin": "bin"
                }
            }
        }"#;
        let err = check_bundle_resources(cfg, d.path()).unwrap_err();
        assert!(err.contains("uv.lock"), "{err}");
    }

    #[test]
    fn bundle_guard_rejects_missing_uv_binary() {
        let d = scratch("no-uv");
        stage_complete(d.path());
        fs::remove_file(d.path().join("stage/bin/uv.exe")).unwrap();
        let cfg = r#"{
            "bundle": {
                "resources": {
                    "stage/worker": "worker",
                    "stage/bin": "bin"
                }
            }
        }"#;
        let err = check_bundle_resources(cfg, d.path()).unwrap_err();
        assert!(err.contains("uv"), "{err}");
    }

    #[test]
    fn bundle_guard_enforces_on_tauri_config_or_installer_build() {
        assert!(!should_enforce(None, None));
        assert!(!should_enforce(Some("   "), None));
        assert!(!should_enforce(None, Some("0")));
        assert!(!should_enforce(None, Some("")));
        assert!(should_enforce(Some("{}"), None));
        assert!(should_enforce(None, Some("1")));
        assert!(should_enforce(Some("{}"), Some("1")));
    }
}
