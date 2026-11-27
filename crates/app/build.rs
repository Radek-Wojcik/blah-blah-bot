mod bundle_guard;

use std::path::{Path, PathBuf};

fn load_config_json(manifest_dir: &Path, tauri_config: Option<&str>) -> Result<String, String> {
    if let Some(raw) = tauri_config.map(str::trim).filter(|v| !v.is_empty()) {
        let candidate = if raw.starts_with('{') {
            raw.to_string()
        } else {
            let p = Path::new(raw);
            let path = if p.is_absolute() {
                p.to_path_buf()
            } else {
                manifest_dir.join(p)
            };
            std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read TAURI_CONFIG at {}: {e}", path.display()))?
        };
        let has_resources = serde_json::from_str::<serde_json::Value>(&candidate)
            .ok()
            .and_then(|v| v.get("bundle")?.get("resources").cloned())
            .is_some();
        if has_resources {
            return Ok(candidate);
        }
    }
    let default_cfg = manifest_dir.join("tauri.conf.json");
    std::fs::read_to_string(&default_cfg)
        .map_err(|e| format!("failed to read {}: {e}", default_cfg.display()))
}

fn main() {
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    println!("cargo:rerun-if-env-changed=BBB_INSTALLER_BUILD");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=tauri.bundle.conf.json");
    println!("cargo:rerun-if-changed=../../worker/pyproject.toml");
    println!("cargo:rerun-if-changed=../../worker/uv.lock");

    let tauri_config = std::env::var("TAURI_CONFIG").ok();
    let installer_build = std::env::var("BBB_INSTALLER_BUILD").ok();
    if bundle_guard::should_enforce(tauri_config.as_deref(), installer_build.as_deref()) {
        let manifest_dir = PathBuf::from(
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"),
        );
        let config_json = load_config_json(&manifest_dir, tauri_config.as_deref())
            .unwrap_or_else(|e| panic!("bundle resource guard failed: {e}"));
        match bundle_guard::check_bundle_resources(&config_json, &manifest_dir) {
            Ok(watched) => {
                for path in watched {
                    println!("cargo:rerun-if-changed={}", path.display());
                }
            }
            Err(err) => {
                panic!(
                    "bundle resource guard failed: {err}\nBuild installers with `powershell -File scripts\\build-installer.ps1` (never plain `tauri build`)."
                );
            }
        }
    }

    tauri_build::build()
}
