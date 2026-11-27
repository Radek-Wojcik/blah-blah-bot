//! IPC commands of the setup: the readiness report, detection of `llama-server`, GGUF files,
//! path checks, the processing environment and the first-run flag. The logic is in
//! [`crate::setup`]; this module gathers the facts from the machine and the application.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use queue::worker::{self, JobOptions};
use serde::Serialize;
use store::LlmKind;
use tauri::State;

use crate::ipc::{IpcError, IpcResult};
use crate::processing::Processing;
use crate::secrets;
use crate::setup::{
    self, DetectEnv, GgufFile, JobNeeds, LlamaResolution, PathCheck, PathKind, PathProblem, Probe,
    SetupItem, SetupStatus,
};
use crate::state::AppState;

/// Collects the facts about this machine. `probe_llama` also runs `llama-server --version`.
/// Asking the worker which models are cached starts a Python process, so call this off the
/// main thread.
pub(crate) fn gather(
    p: &Processing,
    probe_llama: bool,
) -> (crate::processing::ProcessingSettings, Probe) {
    gather_with(p, probe_llama, true)
}

fn gather_with(
    p: &Processing,
    probe_llama: bool,
    probe_models: bool,
) -> (crate::processing::ProcessingSettings, Probe) {
    let s = p.settings_snapshot();
    let env = DetectEnv::from_process();
    let cfg = p.worker_config();

    let uv = setup::resolve_uv(&s.uv_path, p.resource_dir(), env.path_var.as_deref());
    let worker_dir = if cfg.worker_dir.is_absolute() {
        setup::validate_path(PathKind::WorkerDir, &cfg.worker_dir.to_string_lossy())
    } else {
        Err(PathProblem::new(
            "path_not_found",
            "no worker folder was found next to the application",
        ))
    };
    let env_ready = setup::env_ready(p.env_dir());
    let env_working = p.env_busy().load(Ordering::SeqCst);
    let models_working = p.models_busy();

    let models = (probe_models
        && env_ready
        && uv.is_ok()
        && worker_dir.is_ok()
        && !env_working
        && !models_working)
        .then(|| worker::model_status(&cfg).ok())
        .flatten()
        .map(|v| v.into_iter().collect::<HashMap<_, _>>());

    let llama = setup::resolve_llama(&s.llama_server_path, &env);
    let llama_version = match (&llama, probe_llama) {
        (LlamaResolution::Configured(path) | LlamaResolution::Detected(path), true) => {
            Some(setup::probe_llama_version(path, setup::VERSION_TIMEOUT))
        }
        _ => None,
    };
    let model_path = s.llm.managed.model_path.trim();
    let llm_model =
        (!model_path.is_empty()).then(|| setup::validate_path(PathKind::LlmModel, model_path));
    let legacy_venv_notice = setup::legacy_venv_migration_notice(
        p.roaming_data_dir(),
        p.local_data_dir(),
        p.env_dir(),
        s.legacy_venv_notice_dismissed,
    );

    let probe = Probe {
        uv,
        worker_dir,
        env_ready,
        env_working,
        models,
        models_working,
        hf_token_set: secrets::get(secrets::HF_TOKEN).is_some(),
        llama,
        llama_version,
        llama_candidates: setup::detect_llama_candidates(&s.llama_server_path, &env),
        llm_model,
        legacy_venv_notice,
    };
    (s, probe)
}

/// The readiness report of the whole setup.
pub(crate) fn status_of(p: &Processing) -> SetupStatus {
    let (settings, probe) = gather(p, true);
    setup::build_status(&settings, &probe)
}

/// The IPC error for an item that blocks processing: the code of the item, its message and its
/// suggested fix in one text.
pub(crate) fn blocker_error(item: &SetupItem) -> IpcError {
    let text = match &item.fix_hint {
        Some(hint) => format!("{} {hint}", item.message),
        None => item.message.clone(),
    };
    IpcError::new(item.code.unwrap_or("setup_incomplete"), text)
}

/// Converts the first blocking item of `status` for a processing job into the IPC error shown
/// to the user.
pub(crate) fn check_readiness_for_queue(status: &SetupStatus, needs: &JobNeeds) -> IpcResult<()> {
    match setup::first_blocker(status, needs) {
        Some(item) => Err(blocker_error(item)),
        None => Ok(()),
    }
}

/// Converts a worker-environment blocker in `status` into the same IPC error as
/// [`check_readiness_for_queue`], for commands that invoke the worker directly (recording import).
pub(crate) fn check_readiness_for_import(status: &SetupStatus) -> IpcResult<()> {
    match setup::worker_env_blocker(status) {
        Some(item) => Err(blocker_error(item)),
        None => Ok(()),
    }
}

/// Refuses to process a meeting while something it needs is missing, with the message of the
/// setup instead of an error from deep inside the worker. `job` is the job as it will be
/// created.
pub(crate) fn ensure_ready(state: &AppState, meeting_id: &str, job: &JobOptions) -> IpcResult<()> {
    let p = state.processing();
    let settings = p.settings_snapshot();
    let (has_screenshots, has_cycle) = {
        let details = state.with_store(|s| s.get_meeting(meeting_id))?;
        let cycle = state.with_store(|s| s.get_report_settings(meeting_id))?;
        (
            details.is_some_and(|d| !d.screenshots.is_empty()),
            cycle.and_then(|r| r.cycle_id).is_some(),
        )
    };
    let needs = JobNeeds {
        vision: settings.describe_screenshots
            && !settings.skip_screenshot_analysis
            && has_screenshots,
        llm_local: job.llm.as_ref().is_some_and(|l| l.kind == LlmKind::Managed),
        llm_missing: has_cycle && job.llm.is_none() && !settings.skip_reports,
    };
    let (settings, probe) = gather(p, needs.vision || needs.llm_local);
    let status = setup::build_status(&settings, &probe);
    check_readiness_for_queue(&status, &needs)
}

/// Refuses to import a recording while the worker environment is missing, invalid, or still
/// being prepared, with the exact same readiness message as [`ensure_ready`].
pub(crate) fn ensure_import_ready(p: &Processing) -> IpcResult<()> {
    let (settings, probe) = gather_with(p, false, false);
    let status = setup::build_status(&settings, &probe);
    check_readiness_for_import(&status)
}

/// Async: the report asks the worker which models are cached and may run `llama-server
/// --version`, both of which start a process.
#[tauri::command]
pub async fn setup_status(state: State<'_, AppState>) -> IpcResult<SetupStatus> {
    Ok(status_of(state.processing()))
}

/// The `llama-server` programs found on this machine (the configured one first). Only
/// detection: nothing is downloaded or run.
#[tauri::command]
pub async fn detect_llama_server(state: State<'_, AppState>) -> IpcResult<Vec<String>> {
    let configured = state.processing().settings_snapshot().llama_server_path;
    Ok(
        setup::detect_llama_candidates(&configured, &DetectEnv::from_process())
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    )
}

/// GGUF files below `dir`, or, without a folder, in the usual model folders. Language models
/// come first; a `mmproj-*` file is marked as a vision projector.
#[tauri::command]
pub async fn find_gguf_models(
    state: State<'_, AppState>,
    dir: Option<String>,
) -> IpcResult<Vec<GgufFile>> {
    let dirs: Vec<PathBuf> = match dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => {
            let path = setup::validate_path(PathKind::ModelsDir, d)
                .map_err(|p| IpcError::new(p.code, p.message))?;
            if !path.is_dir() {
                return Err(IpcError::new(
                    "path_not_found",
                    format!("models folder: nothing exists at {d}"),
                ));
            }
            vec![path]
        }
        None => {
            let models_dir = state.processing().settings_snapshot().models_dir;
            setup::common_model_dirs(&models_dir, &DetectEnv::from_process())
        }
    };
    Ok(setup::find_gguf(&dirs))
}

/// Checks a path the user typed or chose before it is saved. Reads only.
#[tauri::command]
pub fn check_setup_path(kind: PathKind, path: String) -> PathCheck {
    setup::validate_path(kind, &path).into()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupProgress {
    /// The item the progress belongs to (`workerEnv`).
    key: &'static str,
    /// A line of output of the installer; the last line when `done`.
    line: String,
    /// Not known for the environment: the installer does not report a fraction.
    fraction: Option<f64>,
    done: bool,
    ok: Option<bool>,
    error: Option<String>,
}

/// Creates or updates the processing environment (`uv sync --frozen`) in the background and
/// reports it with `setup-progress` events; the last one has `done: true`. Needs the network on
/// the first run (several GB). Running it again continues an interrupted run.
#[tauri::command]
pub fn prepare_worker_env(state: State<'_, AppState>) -> IpcResult<()> {
    let processing: Arc<Processing> = state.processing().clone();
    let settings = processing.settings_snapshot();
    let cfg = processing.worker_config();
    let uv = setup::resolve_uv(
        &settings.uv_path,
        processing.resource_dir(),
        std::env::var_os("PATH").as_deref(),
    )
    .map_err(|p| {
        IpcError::new(
            setup::CODE_WORKER_ENV,
            format!("uv is not available: {}", p.message),
        )
    })?;
    if !cfg.worker_dir.is_absolute() {
        return Err(IpcError::new(
            setup::CODE_WORKER_ENV,
            "no worker folder was found next to the application; set it in Settings > Processing",
        ));
    }
    let worker_dir = setup::validate_path(PathKind::WorkerDir, &cfg.worker_dir.to_string_lossy())
        .map_err(|p| IpcError::new(setup::CODE_WORKER_ENV, p.message))?;
    if processing.env_busy().swap(true, Ordering::SeqCst) {
        return Err(IpcError::new(
            "busy",
            "the environment is already being prepared",
        ));
    }
    let env_dir = processing.env_dir().to_path_buf();
    let uv_cache_dir = processing.uv_cache_dir().to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("worker-env".into())
        .spawn(move || {
            let emit = |line: &str, done: bool, ok: Option<bool>, error: Option<String>| {
                if let Ok(payload) = serde_json::to_value(SetupProgress {
                    key: setup::KEY_WORKER_ENV,
                    line: line.to_string(),
                    fraction: done.then_some(1.0).filter(|_| ok == Some(true)),
                    done,
                    ok,
                    error,
                }) {
                    processing.emit_setup_progress(payload);
                }
            };
            emit("Preparing the processing environment", false, None, None);
            let result = setup::run_uv_sync(&uv, &worker_dir, &env_dir, &uv_cache_dir, |line| {
                emit(line, false, None, None);
            });
            match result {
                Ok(()) => emit(
                    "The processing environment is ready",
                    true,
                    Some(true),
                    None,
                ),
                Err(e) => {
                    tracing::error!(error = %e, "preparing the processing environment failed");
                    emit(&e, true, Some(false), Some(e.clone()));
                }
            }
            processing.env_busy().store(false, Ordering::SeqCst);
        });
    if spawned.is_err() {
        state.processing().env_busy().store(false, Ordering::SeqCst);
        return Err(IpcError::new("internal", "could not start the set-up"));
    }
    Ok(())
}

/// Marks the first-run setup as completed (`done: true`) or to be shown again (`false`).
#[tauri::command]
pub fn set_first_run_done(state: State<'_, AppState>, done: bool) -> IpcResult<()> {
    state.processing().set_first_run_done(done)
}

/// Dismisses the one-time notice about a legacy Roaming `worker-venv` folder.
#[tauri::command]
pub fn dismiss_legacy_venv_notice(state: State<'_, AppState>) -> IpcResult<()> {
    state.processing().dismiss_legacy_venv_notice()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::ProcessingSettings;

    #[test]
    fn import_and_queue_readiness_produce_the_same_error_when_worker_env_is_not_ready() {
        let settings = ProcessingSettings::default();
        let needs = JobNeeds {
            vision: false,
            llm_local: false,
            llm_missing: false,
        };

        // 1. Missing uv
        let mut p_no_uv = Probe::empty();
        p_no_uv.worker_dir = Ok(PathBuf::from("C:/app/worker"));
        let st = setup::build_status(&settings, &p_no_uv);
        let q_err = check_readiness_for_queue(&st, &needs).unwrap_err();
        let i_err = check_readiness_for_import(&st).unwrap_err();
        assert_eq!(i_err.code, q_err.code);
        assert_eq!(i_err.message, q_err.message);
        assert_eq!(i_err.code, setup::CODE_WORKER_ENV);

        // 2. Missing worker folder
        let mut p_no_worker = Probe::empty();
        p_no_worker.uv = Ok(PathBuf::from("C:/app/bin/uv.exe"));
        let st = setup::build_status(&settings, &p_no_worker);
        let q_err = check_readiness_for_queue(&st, &needs).unwrap_err();
        let i_err = check_readiness_for_import(&st).unwrap_err();
        assert_eq!(i_err.code, q_err.code);
        assert_eq!(i_err.message, q_err.message);

        // 3. Worker env not created yet
        let mut p_no_env = Probe::empty();
        p_no_env.uv = Ok(PathBuf::from("C:/app/bin/uv.exe"));
        p_no_env.worker_dir = Ok(PathBuf::from("C:/app/worker"));
        p_no_env.env_ready = false;
        let st = setup::build_status(&settings, &p_no_env);
        let q_err = check_readiness_for_queue(&st, &needs).unwrap_err();
        let i_err = check_readiness_for_import(&st).unwrap_err();
        assert_eq!(i_err.code, q_err.code);
        assert_eq!(i_err.message, q_err.message);
        assert!(i_err.message.contains("has not been created yet"));

        // 4. Worker env currently being prepared
        let mut p_working = p_no_env;
        p_working.env_working = true;
        let st = setup::build_status(&settings, &p_working);
        let q_err = check_readiness_for_queue(&st, &needs).unwrap_err();
        let i_err = check_readiness_for_import(&st).unwrap_err();
        assert_eq!(i_err.code, q_err.code);
        assert_eq!(i_err.message, q_err.message);
    }
}
