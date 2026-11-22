//! Setup readiness: what the application needs before a meeting can be processed, checked
//! item by item, with a stable code, a short message and a suggested fix for each failure.
//!
//! The checks that touch the system (files, `PATH`, a child process) are small functions that
//! take their inputs as arguments, and the report itself ([`build_status`]) is a pure function
//! of the settings and a [`Probe`] of facts, so all of it is tested without Tauri. Nothing here
//! returns a token or key value; a stored token is reported as a yes or no.
//!
//! Paths a user supplies (the `uv` executable, `llama-server`, the language model, the worker
//! folder) are validated before they are saved: an absolute local path, an existing file of the
//! expected kind. A path is never handed to a shell and nothing is run from it except the
//! `--version` probe of a file that passed the check for `llama-server`.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use store::LlmKind;

use crate::processing::ProcessingSettings;

// ----- item keys and codes ---------------------------------------------------------------------

pub const KEY_WORKER_ENV: &str = "workerEnv";
pub const KEY_MODEL_WHISPER: &str = "model:whisper-turbo";
pub const KEY_MODEL_DIARIZATION: &str = "model:diarization";
pub const KEY_HF_TOKEN: &str = "hfToken";
pub const KEY_LLAMA_SERVER: &str = "llamaServer";
pub const KEY_LLM_MODEL: &str = "llmModel";
pub const KEY_VISION: &str = "vision";

/// Codes of the items; `process_meeting` returns the code of the first item that blocks it.
pub const CODE_WORKER_ENV: &str = "setup_worker_env";
pub const CODE_MODEL_MISSING: &str = "setup_model_missing";
pub const CODE_HF_TOKEN: &str = "setup_hf_token";
pub const CODE_LLAMA_SERVER: &str = "setup_llama_server";
pub const CODE_LLM_MODEL: &str = "setup_llm_model";
pub const CODE_VISION: &str = "setup_vision";

/// How long `llama-server --version` may take.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// An environment that takes longer than this to build is stopped.
const ENV_TIMEOUT: Duration = Duration::from_secs(2 * 3600);

// ----- path validation -------------------------------------------------------------------------

/// What a user-supplied path is meant to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathKind {
    /// The `uv` executable.
    Uv,
    /// The `llama-server` executable.
    LlamaServer,
    /// A GGUF language model.
    LlmModel,
    /// The folder with the worker (`pyproject.toml`, `uv.lock`).
    WorkerDir,
    /// The folder of the downloaded models.
    ModelsDir,
}

impl PathKind {
    fn label(self) -> &'static str {
        match self {
            Self::Uv => "uv",
            Self::LlamaServer => "llama-server",
            Self::LlmModel => "language model",
            Self::WorkerDir => "worker folder",
            Self::ModelsDir => "models folder",
        }
    }
}

/// Why a path was refused. `code` is stable and machine readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathProblem {
    pub code: &'static str,
    pub message: String,
}

impl PathProblem {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// The answer of `check_setup_path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathCheck {
    pub ok: bool,
    pub code: Option<&'static str>,
    pub message: Option<String>,
}

impl From<Result<PathBuf, PathProblem>> for PathCheck {
    fn from(r: Result<PathBuf, PathProblem>) -> Self {
        match r {
            Ok(_) => Self {
                ok: true,
                code: None,
                message: None,
            },
            Err(p) => Self {
                ok: false,
                code: Some(p.code),
                message: Some(p.message),
            },
        }
    }
}

/// A file name without a folder (`uv`, `llama-server.exe`): it is looked up on `PATH`.
fn is_bare_name(raw: &str) -> bool {
    !raw.contains(['/', '\\'])
}

fn file_stem_lower(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// A network or device path: executing or reading it would leave the machine.
fn is_network_or_device(path: &Path) -> bool {
    path.components().next().is_some_and(|c| match c {
        Component::Prefix(p) => !matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)),
        _ => false,
    })
}

/// The first bytes of a file, fewer when it is shorter.
fn head(path: &Path, n: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(n);
    std::fs::File::open(path)?
        .take(n as u64)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

/// Whether the file starts like a GGUF model.
pub fn is_gguf(path: &Path) -> bool {
    head(path, 4).is_ok_and(|h| h == b"GGUF")
}

/// Looks for `file` in the folders of `path_var` (absolute entries only: a relative entry,
/// and so the current folder, is never searched).
pub fn find_on_path(file: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let var = path_var?;
    std::env::split_paths(var)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(file))
        .find(|p| p.is_file())
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// Validates `raw` as a path of `kind` (see the module notes). `path_var` is used to look up
/// a bare name such as `uv`.
pub fn validate_path_in(
    kind: PathKind,
    raw: &str,
    path_var: Option<&OsStr>,
) -> Result<PathBuf, PathProblem> {
    let label = kind.label();
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(PathProblem::new(
            "path_empty",
            format!("{label}: no path was given"),
        ));
    }
    let executable = matches!(kind, PathKind::Uv | PathKind::LlamaServer);
    if executable && is_bare_name(raw) {
        let name = Path::new(raw);
        let ok_name = match kind {
            PathKind::Uv => file_stem_lower(name) == "uv",
            _ => file_stem_lower(name).starts_with("llama-server"),
        };
        if !ok_name {
            return Err(PathProblem::new(
                "path_wrong_kind",
                format!("{label}: '{raw}' is not the name of this program; give the full path"),
            ));
        }
        let file = if Path::new(raw).extension().is_some() {
            raw.to_string()
        } else {
            exe_name(raw)
        };
        return find_on_path(&file, path_var).ok_or_else(|| {
            PathProblem::new(
                "path_not_found",
                format!("{label}: '{raw}' was not found on the PATH; give the full path"),
            )
        });
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(PathProblem::new(
            "path_not_absolute",
            format!("{label}: the path must be absolute (start with a drive letter): {raw}"),
        ));
    }
    if is_network_or_device(&path) {
        return Err(PathProblem::new(
            "path_network",
            format!("{label}: network and device paths are not accepted: {raw}"),
        ));
    }
    let must_exist = kind != PathKind::ModelsDir;
    if !path.exists() {
        if must_exist {
            return Err(PathProblem::new(
                "path_not_found",
                format!("{label}: nothing exists at {raw}"),
            ));
        }
        return Ok(path);
    }
    match kind {
        PathKind::Uv | PathKind::LlamaServer | PathKind::LlmModel => {
            if !path.is_file() {
                return Err(PathProblem::new(
                    "path_not_file",
                    format!("{label}: this is not a file: {raw}"),
                ));
            }
        }
        PathKind::WorkerDir | PathKind::ModelsDir => {
            if !path.is_dir() {
                return Err(PathProblem::new(
                    "path_not_dir",
                    format!("{label}: this is not a folder: {raw}"),
                ));
            }
        }
    }
    check_kind(kind, &path, raw)?;
    Ok(path)
}

/// [`validate_path_in`] with the `PATH` of this process.
pub fn validate_path(kind: PathKind, raw: &str) -> Result<PathBuf, PathProblem> {
    validate_path_in(kind, raw, std::env::var_os("PATH").as_deref())
}

/// The content checks of an existing path.
fn check_kind(kind: PathKind, path: &Path, raw: &str) -> Result<(), PathProblem> {
    let label = kind.label();
    let wrong = |what: String| {
        Err(PathProblem::new(
            "path_wrong_kind",
            format!("{label}: {what}"),
        ))
    };
    match kind {
        PathKind::Uv | PathKind::LlamaServer => {
            let stem = file_stem_lower(path);
            let name_ok = if kind == PathKind::Uv {
                stem == "uv"
            } else {
                stem.starts_with("llama-server")
            };
            if !name_ok {
                let expected = if kind == PathKind::Uv {
                    "uv.exe"
                } else {
                    "llama-server.exe"
                };
                return wrong(format!("the file is not named {expected}: {raw}"));
            }
            if cfg!(windows) {
                let is_exe = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
                if !is_exe {
                    return wrong(format!("an .exe file is expected: {raw}"));
                }
                if head(path, 2).map_or(true, |h| h != b"MZ") {
                    return wrong(format!("this is not a Windows program: {raw}"));
                }
            }
        }
        PathKind::LlmModel => {
            let is_gguf_name = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("gguf"));
            if !is_gguf_name {
                return wrong(format!("a .gguf file is expected: {raw}"));
            }
            if !is_gguf(path) {
                return wrong(format!("the file is not a valid GGUF model: {raw}"));
            }
            if is_projector_name(path) {
                return wrong(format!(
                    "this is a vision projector (mmproj), not a language model: {raw}"
                ));
            }
        }
        PathKind::WorkerDir => {
            for file in ["pyproject.toml", "uv.lock"] {
                if !path.join(file).is_file() {
                    return wrong(format!("the folder has no {file}: {raw}"));
                }
            }
        }
        PathKind::ModelsDir => {}
    }
    Ok(())
}

fn is_projector_name(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().contains("mmproj"))
}

/// Checks the paths of `new` that differ from `current`: a stored path that went stale (a
/// moved file) must not stop the user from saving something unrelated. Empty means default.
pub fn validate_settings_paths(
    new: &ProcessingSettings,
    current: &ProcessingSettings,
) -> Result<(), PathProblem> {
    let fields: [(PathKind, &str, &str); 5] = [
        (PathKind::Uv, &new.uv_path, &current.uv_path),
        (
            PathKind::LlamaServer,
            &new.llama_server_path,
            &current.llama_server_path,
        ),
        (PathKind::WorkerDir, &new.worker_dir, &current.worker_dir),
        (PathKind::ModelsDir, &new.models_dir, &current.models_dir),
        (
            PathKind::LlmModel,
            &new.llm.managed.model_path,
            &current.llm.managed.model_path,
        ),
    ];
    for (kind, new_value, old_value) in fields {
        if new_value.trim().is_empty() || new_value.trim() == old_value.trim() {
            continue;
        }
        validate_path(kind, new_value)?;
    }
    Ok(())
}

// ----- finding tools ---------------------------------------------------------------------------

/// Where tools are looked for. Built from the process environment, or by hand in tests.
#[derive(Debug, Clone, Default)]
pub struct DetectEnv {
    pub path_var: Option<OsString>,
    pub local_app_data: Option<PathBuf>,
    pub user_profile: Option<PathBuf>,
    /// `BBB_LLAMA_SERVER`.
    pub llama_override: Option<OsString>,
}

impl DetectEnv {
    pub fn from_process() -> Self {
        Self {
            path_var: std::env::var_os("PATH"),
            local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            user_profile: std::env::var_os("USERPROFILE").map(PathBuf::from),
            llama_override: std::env::var_os("BBB_LLAMA_SERVER"),
        }
    }
}

fn dirs_starting_with(parent: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = read
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with(prefix)
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// The `llama-server` programs found on this machine, best first: the configured one, the
/// `BBB_LLAMA_SERVER` override, the `PATH`, WinGet and the usual install folders. Only
/// detection: nothing is downloaded and nothing is run.
pub fn detect_llama_candidates(configured: &str, env: &DetectEnv) -> Vec<PathBuf> {
    let name = exe_name("llama-server");
    let mut found: Vec<PathBuf> = Vec::new();
    let path_var = env.path_var.as_deref();
    let mut push = |p: PathBuf| {
        if validate_path_in(PathKind::LlamaServer, &p.to_string_lossy(), path_var).is_ok()
            && !found.contains(&p)
        {
            found.push(p);
        }
    };
    if !configured.trim().is_empty() {
        if let Ok(p) = validate_path_in(PathKind::LlamaServer, configured, path_var) {
            push(p);
        }
    }
    if let Some(o) = &env.llama_override {
        push(PathBuf::from(o));
    }
    if let Some(p) = find_on_path(&name, path_var) {
        push(p);
    }
    if let Some(local) = &env.local_app_data {
        push(local.join("Microsoft/WinGet/Links").join(&name));
        for dir in dirs_starting_with(&local.join("Microsoft/WinGet/Packages"), "ggml.llamacpp") {
            push(dir.join(&name));
        }
        for dir in dirs_starting_with(&local.join("Programs"), "llama.cpp") {
            push(dir.join(&name));
            push(dir.join("bin").join(&name));
        }
    }
    found
}

/// Which `llama-server` the application uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlamaResolution {
    /// The path of the settings.
    Configured(PathBuf),
    /// Nothing in the settings: the first program found on this machine.
    Detected(PathBuf),
    NotSet,
    /// The path of the settings does not pass the check.
    Invalid {
        path: String,
        problem: PathProblem,
    },
}

pub fn resolve_llama(configured: &str, env: &DetectEnv) -> LlamaResolution {
    let configured = configured.trim();
    if configured.is_empty() {
        return detect_llama_candidates("", env)
            .into_iter()
            .next()
            .map_or(LlamaResolution::NotSet, LlamaResolution::Detected);
    }
    match validate_path_in(PathKind::LlamaServer, configured, env.path_var.as_deref()) {
        Ok(p) => LlamaResolution::Configured(p),
        Err(problem) => LlamaResolution::Invalid {
            path: configured.to_string(),
            problem,
        },
    }
}

impl LlamaResolution {
    /// The program to start, if there is one.
    pub fn path(&self) -> Option<PathBuf> {
        match self {
            Self::Configured(p) | Self::Detected(p) => Some(p.clone()),
            Self::NotSet => None,
            // Handed on as it is: starting it fails with a clear message.
            Self::Invalid { path, .. } => Some(PathBuf::from(path)),
        }
    }
}

/// The `uv` to run: the settings (a full path, or a bare `uv` looked up on the `PATH`), then
/// the one bundled with the installation (`<resources>/bin/uv.exe`), then the `PATH`. Looked up
/// by absolute folders only, so a `uv.exe` in the current folder is never picked up.
pub fn resolve_uv(
    configured: &str,
    resource_dir: Option<&Path>,
    path_var: Option<&OsStr>,
) -> Result<PathBuf, PathProblem> {
    if !configured.trim().is_empty() {
        return validate_path_in(PathKind::Uv, configured, path_var);
    }
    if let Some(dir) = resource_dir {
        let bundled = dir.join("bin").join(exe_name("uv"));
        if bundled.is_file() {
            return Ok(bundled);
        }
    }
    find_on_path(&exe_name("uv"), path_var).ok_or_else(|| {
        PathProblem::new(
            "tool_not_found",
            "uv was not found on this computer".to_string(),
        )
    })
}

/// `llama-server --version` with a time limit. The first line of the output on success.
pub fn probe_llama_version(path: &Path, timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new(path);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        // llama.cpp prints its version on stderr.
        let mut text = String::new();
        if let Some(mut s) = stderr {
            let _ = s.by_ref().take(4096).read_to_string(&mut text);
        }
        if let Some(mut s) = stdout {
            let _ = s.by_ref().take(4096).read_to_string(&mut text);
        }
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer within {} seconds", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let text = reader.join().unwrap_or_default();
    let line = text
        .lines()
        .find(|l| l.to_ascii_lowercase().contains("version"))
        .or_else(|| text.lines().next())
        .unwrap_or("")
        .trim()
        .to_string();
    if status.success() {
        Ok(line)
    } else {
        Err(if line.is_empty() {
            format!("it exited with {status}")
        } else {
            line
        })
    }
}

// ----- GGUF files ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GgufKind {
    /// A language model.
    Llm,
    /// The projector of a vision model (`mmproj-*`).
    Mmproj,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GgufFile {
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    pub kind: GgufKind,
}

const GGUF_MAX_DEPTH: usize = 5;
const GGUF_MAX_VISITED: usize = 5000;
const GGUF_MAX_RESULTS: usize = 200;

fn scan_one(dir: &Path, depth: usize, visited: &mut usize, out: &mut Vec<GgufFile>) {
    if depth > GGUF_MAX_DEPTH || *visited >= GGUF_MAX_VISITED {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        *visited += 1;
        if *visited >= GGUF_MAX_VISITED || out.len() >= GGUF_MAX_RESULTS {
            return;
        }
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            scan_one(&path, depth + 1, visited, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gguf"))
        {
            // A file symlink (Hugging Face snapshots) is followed by `metadata`.
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_file() && is_gguf(&path) {
                out.push(GgufFile {
                    path: path.to_string_lossy().into_owned(),
                    size: meta.len(),
                    kind: if is_projector_name(&path) {
                        GgufKind::Mmproj
                    } else {
                        GgufKind::Llm
                    },
                });
            }
        }
    }
}

/// The GGUF files below `dirs` (a few levels deep, directory links are not followed),
/// language models first. A file that does not start with the GGUF marker is left out.
pub fn find_gguf(dirs: &[PathBuf]) -> Vec<GgufFile> {
    let mut out = Vec::new();
    let mut visited = 0;
    for dir in dirs {
        scan_one(dir, 0, &mut visited, &mut out);
    }
    let mut seen = HashSet::new();
    out.retain(|f| seen.insert(f.path.to_ascii_lowercase()));
    out.sort_by(|a, b| {
        (a.kind == GgufKind::Mmproj, &a.path).cmp(&(b.kind == GgufKind::Mmproj, &b.path))
    });
    out
}

/// The folders where language models usually are: the models folder of the settings, the
/// LM Studio, llama.cpp and Hugging Face caches, `models` and `Downloads` of the user.
pub fn common_model_dirs(models_dir: &str, env: &DetectEnv) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if !models_dir.trim().is_empty() {
        dirs.push(PathBuf::from(models_dir.trim()));
    }
    if let Some(home) = &env.user_profile {
        dirs.push(home.join(".lmstudio/models"));
        dirs.push(home.join(".cache/lm-studio/models"));
        dirs.push(home.join(".cache/huggingface/hub"));
        dirs.push(home.join("models"));
        dirs.push(home.join("Downloads"));
    }
    if let Some(local) = &env.local_app_data {
        dirs.push(local.join("llama.cpp"));
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

// ----- the report ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemState {
    Ok,
    Missing,
    Invalid,
    Skipped,
    Working,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupItem {
    pub key: String,
    pub state: ItemState,
    /// Stable code of a problem; absent for `ok` and `skipped`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    /// Every item is `ok` or `skipped`.
    pub ready: bool,
    pub first_run_done: bool,
    pub items: Vec<SetupItem>,
    /// `llama-server` programs found on this machine.
    pub llama_candidates: Vec<String>,
}

/// Facts about the machine, gathered by the caller.
pub struct Probe {
    pub uv: Result<PathBuf, PathProblem>,
    pub worker_dir: Result<PathBuf, PathProblem>,
    pub env_ready: bool,
    pub env_working: bool,
    /// Which models the worker reports as cached; `None` when it could not be asked.
    pub models: Option<HashMap<String, bool>>,
    pub models_working: bool,
    pub hf_token_set: bool,
    pub llama: LlamaResolution,
    /// Result of `--version`, when it was tried.
    pub llama_version: Option<Result<String, String>>,
    pub llama_candidates: Vec<PathBuf>,
    /// The check of the configured language-model file; `None` while no path is set.
    pub llm_model: Option<Result<PathBuf, PathProblem>>,
}

impl Probe {
    /// A machine with nothing set up.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            uv: Err(PathProblem::new("tool_not_found", "uv was not found")),
            worker_dir: Err(PathProblem::new("path_not_found", "no worker folder")),
            env_ready: false,
            env_working: false,
            models: None,
            models_working: false,
            hf_token_set: false,
            llama: LlamaResolution::NotSet,
            llama_version: None,
            llama_candidates: Vec::new(),
            llm_model: None,
        }
    }
}

fn item(
    key: &str,
    state: ItemState,
    code: &'static str,
    message: impl Into<String>,
    hint: impl Into<String>,
) -> SetupItem {
    let problem = matches!(
        state,
        ItemState::Missing | ItemState::Invalid | ItemState::Working
    );
    SetupItem {
        key: key.into(),
        state,
        code: problem.then_some(code),
        message: message.into(),
        fix_hint: Some(hint.into()).filter(|h: &String| !h.is_empty()),
    }
}

fn ok(key: &str, message: impl Into<String>) -> SetupItem {
    item(key, ItemState::Ok, "", message, "")
}

fn skipped(key: &str, message: impl Into<String>, hint: impl Into<String>) -> SetupItem {
    item(key, ItemState::Skipped, "", message, hint)
}

fn worker_env_item(p: &Probe) -> SetupItem {
    let key = KEY_WORKER_ENV;
    if p.env_working {
        return item(
            key,
            ItemState::Working,
            CODE_WORKER_ENV,
            "The processing environment is being prepared.",
            "Wait until it finishes; the first run downloads several GB.",
        );
    }
    if let Err(problem) = &p.uv {
        let state = if problem.code == "tool_not_found" {
            ItemState::Missing
        } else {
            ItemState::Invalid
        };
        return item(
            key,
            state,
            CODE_WORKER_ENV,
            format!(
                "uv, the installer of the processing environment, is not available: {}.",
                problem.message
            ),
            "Install uv, or set the path to uv.exe in Settings > Processing.",
        );
    }
    if let Err(problem) = &p.worker_dir {
        return item(
            key,
            ItemState::Invalid,
            CODE_WORKER_ENV,
            format!("The processing worker was not found: {}.", problem.message),
            "Reinstall the application, or set the worker folder in Settings > Processing.",
        );
    }
    if !p.env_ready {
        return item(
            key,
            ItemState::Missing,
            CODE_WORKER_ENV,
            "The processing environment has not been created yet.",
            "Prepare it in the setup. The first run needs a network connection and downloads several GB.",
        );
    }
    ok(key, "The processing environment is ready.")
}

fn model_item(p: &Probe, key: &str, name: &str, hint_missing: &str) -> SetupItem {
    let model_key = key.trim_start_matches("model:");
    if p.models_working {
        return item(
            key,
            ItemState::Working,
            CODE_MODEL_MISSING,
            format!("{name} is being downloaded."),
            "Wait until the download finishes.",
        );
    }
    match &p.models {
        None => item(
            key,
            ItemState::Missing,
            CODE_MODEL_MISSING,
            format!("{name} cannot be checked until the processing environment is ready."),
            "Prepare the processing environment first, then download the model.",
        ),
        Some(m) if m.get(model_key).copied().unwrap_or(false) => {
            ok(key, format!("{name} is downloaded."))
        }
        Some(_) => item(
            key,
            ItemState::Missing,
            CODE_MODEL_MISSING,
            format!("{name} is not downloaded."),
            hint_missing,
        ),
    }
}

fn hf_token_item(p: &Probe) -> SetupItem {
    let diarization_cached = p
        .models
        .as_ref()
        .is_some_and(|m| m.get("diarization").copied().unwrap_or(false));
    if p.hf_token_set {
        ok(KEY_HF_TOKEN, "A Hugging Face token is stored.")
    } else if diarization_cached {
        skipped(
            KEY_HF_TOKEN,
            "No Hugging Face token is stored, and none is needed: the speaker model is already downloaded.",
            "",
        )
    } else {
        item(
            KEY_HF_TOKEN,
            ItemState::Missing,
            CODE_HF_TOKEN,
            "No Hugging Face token is stored. The speaker model needs one.",
            "Accept the terms of the speaker model on Hugging Face, create a read token and enter it in the setup.",
        )
    }
}

fn llama_needed(s: &ProcessingSettings) -> bool {
    s.describe_screenshots || (s.llm.default_kind == LlmKind::Managed && !s.skip_reports)
}

fn llama_item(s: &ProcessingSettings, p: &Probe) -> SetupItem {
    let key = KEY_LLAMA_SERVER;
    let found = |path: &Path, detected: bool| match &p.llama_version {
        Some(Err(e)) => item(
            key,
            ItemState::Invalid,
            CODE_LLAMA_SERVER,
            format!("llama-server at {} did not run: {e}.", path.display()),
            "Check the file, or choose another llama-server in the setup.",
        ),
        version => {
            let how = if detected {
                "found automatically"
            } else {
                "set in the settings"
            };
            let extra = match version {
                Some(Ok(v)) if !v.is_empty() => format!(" ({v})"),
                _ => String::new(),
            };
            ok(
                key,
                format!("llama-server {how}: {}{extra}.", path.display()),
            )
        }
    };
    match &p.llama {
        LlamaResolution::Configured(path) => found(path, false),
        LlamaResolution::Detected(path) => found(path, true),
        LlamaResolution::Invalid { problem, .. } => item(
            key,
            ItemState::Invalid,
            CODE_LLAMA_SERVER,
            format!("The llama-server path in the settings is not usable: {}.", problem.message),
            "Choose a detected llama-server or enter the full path to llama-server.exe in the setup.",
        ),
        LlamaResolution::NotSet if llama_needed(s) => item(
            key,
            ItemState::Missing,
            CODE_LLAMA_SERVER,
            "llama-server was not found on this computer.",
            "Install llama.cpp (for example with winget install ggml.LlamaCpp), then detect it in the setup or enter the path to llama-server.exe.",
        ),
        LlamaResolution::NotSet => skipped(
            key,
            "llama-server is not needed with the current choices.",
            "",
        ),
    }
}

fn no_model_chosen(s: &ProcessingSettings) -> SetupItem {
    if s.skip_reports {
        skipped(
            KEY_LLM_MODEL,
            "Reports are turned off: meetings get a transcript only.",
            "Choose a language model in the setup to get reports.",
        )
    } else {
        item(
            KEY_LLM_MODEL,
            ItemState::Missing,
            CODE_LLM_MODEL,
            "No language model was chosen, so no report can be written.",
            "Choose a .gguf model file, use an external server, or skip reports for now (transcripts only).",
        )
    }
}

fn llm_item(s: &ProcessingSettings, p: &Probe) -> SetupItem {
    match s.llm.default_kind {
        LlmKind::External => {
            let e = &s.llm.external;
            if e.base_url.trim().is_empty() || e.model.trim().is_empty() {
                return no_model_chosen(s);
            }
            let host = queue::worker::url_host(&e.base_url).unwrap_or_default();
            ok(
                KEY_LLM_MODEL,
                format!("The language model runs on an external server ({host})."),
            )
        }
        _ => match &p.llm_model {
            None => no_model_chosen(s),
            Some(Ok(path)) => ok(
                KEY_LLM_MODEL,
                format!(
                    "Language model: {}.",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
            ),
            Some(Err(problem)) => item(
                KEY_LLM_MODEL,
                if problem.code == "path_not_found" {
                    ItemState::Missing
                } else {
                    ItemState::Invalid
                },
                CODE_LLM_MODEL,
                format!(
                    "The chosen language model cannot be used: {}.",
                    problem.message
                ),
                "Choose the model file again in the setup.",
            ),
        },
    }
}

fn vision_item(s: &ProcessingSettings, p: &Probe) -> SetupItem {
    if !s.describe_screenshots {
        return skipped(
            KEY_VISION,
            "Screenshot analysis is turned off.",
            "Turn it on in Settings > Processing to have screenshots described.",
        );
    }
    if p.models_working {
        return item(
            KEY_VISION,
            ItemState::Working,
            CODE_VISION,
            "The screenshot model is being downloaded.",
            "Wait until the download finishes.",
        );
    }
    match &p.models {
        Some(m) if m.get("vision").copied().unwrap_or(false) => {
            ok(KEY_VISION, "The screenshot model is downloaded.")
        }
        Some(_) => item(
            KEY_VISION,
            ItemState::Missing,
            CODE_VISION,
            "The screenshot model is not downloaded.",
            "Download it in the setup (about 5 GB), or skip screenshot analysis.",
        ),
        None => item(
            KEY_VISION,
            ItemState::Missing,
            CODE_VISION,
            "The screenshot model cannot be checked until the processing environment is ready.",
            "Prepare the processing environment first, then download the model or skip screenshot analysis.",
        ),
    }
}

/// The report: one item per requirement, in the order a user fixes them.
pub fn build_status(s: &ProcessingSettings, p: &Probe) -> SetupStatus {
    let items = vec![
        worker_env_item(p),
        model_item(
            p,
            KEY_MODEL_WHISPER,
            "The speech recognition model (Whisper turbo)",
            "Download it in the setup (about 1.6 GB).",
        ),
        model_item(
            p,
            KEY_MODEL_DIARIZATION,
            "The speaker model (pyannote)",
            if p.hf_token_set {
                "Accept its terms on Hugging Face if you have not, then download it in the setup."
            } else {
                "Enter a Hugging Face token (after accepting the model terms), then download it in the setup."
            },
        ),
        hf_token_item(p),
        llama_item(s, p),
        llm_item(s, p),
        vision_item(s, p),
    ];
    SetupStatus {
        ready: items
            .iter()
            .all(|i| matches!(i.state, ItemState::Ok | ItemState::Skipped)),
        first_run_done: s.first_run_done,
        items,
        llama_candidates: p
            .llama_candidates
            .iter()
            .map(|c| c.to_string_lossy().into_owned())
            .collect(),
    }
}

/// What a job will need beyond the always-needed pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobNeeds {
    /// The meeting has screenshots and their analysis is on.
    pub vision: bool,
    /// The job extracts with the managed (local) language model.
    pub llm_local: bool,
    /// The meeting has a cycle, but no language model is chosen and reports are not skipped.
    pub llm_missing: bool,
}

/// The first item that stops a job from running, if any.
pub fn first_blocker<'a>(status: &'a SetupStatus, needs: &JobNeeds) -> Option<&'a SetupItem> {
    status.items.iter().find(|i| {
        let relevant = match i.key.as_str() {
            KEY_WORKER_ENV | KEY_MODEL_WHISPER | KEY_MODEL_DIARIZATION => true,
            KEY_LLAMA_SERVER => needs.vision || needs.llm_local,
            KEY_LLM_MODEL => needs.llm_local || needs.llm_missing,
            KEY_VISION => needs.vision,
            _ => false,
        };
        relevant
            && matches!(
                i.state,
                ItemState::Missing | ItemState::Invalid | ItemState::Working
            )
    })
}

// ----- the environment -------------------------------------------------------------------------

/// Whether a Python environment exists in `env_dir` (the interpreter is there).
pub fn env_ready(env_dir: &Path) -> bool {
    env_dir.join("pyvenv.cfg").is_file()
        && (env_dir.join("Scripts").join("python.exe").is_file()
            || env_dir.join("bin").join("python").is_file())
}

/// Runs `uv sync --frozen` for the worker, handing every line of its output to `on_line`.
/// Needs the network on the first run. Stopped after two hours.
pub fn run_uv_sync(
    uv: &Path,
    worker_dir: &Path,
    env_dir: &Path,
    mut on_line: impl FnMut(&str),
) -> Result<(), String> {
    let mut cmd = Command::new(uv);
    cmd.args(["sync", "--frozen", "--project"])
        .arg(worker_dir)
        .env("UV_PROJECT_ENVIRONMENT", env_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("uv could not be started ({}): {e}", uv.display()))?;
    let stderr = child.stderr.take().expect("piped");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let started = Instant::now();
    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                on_line(&line);
                if tail.len() == 6 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if started.elapsed() > ENV_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("creating the environment took longer than two hours and was stopped; run it again to continue".into());
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "creating the environment failed: {}",
            tail.into_iter().collect::<Vec<_>>().join(" | ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("bbb-setup-{name}-"))
            .tempdir()
            .unwrap()
    }

    /// A file that passes the `.exe` checks on Windows.
    fn fake_exe(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::create_dir_all(dir).unwrap();
        fs::write(&p, b"MZ fake program").unwrap();
        p
    }

    fn fake_gguf(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::create_dir_all(dir).unwrap();
        fs::write(&p, b"GGUF\x03\0\0\0 fake model").unwrap();
        p
    }

    fn exe(stem: &str) -> String {
        exe_name(stem)
    }

    #[test]
    fn relative_missing_and_wrong_kind_paths_are_refused() {
        let d = scratch("paths");
        let uv = fake_exe(d.path(), &exe("uv"));
        assert_eq!(
            validate_path_in(PathKind::Uv, &uv.to_string_lossy(), None).unwrap(),
            uv
        );
        for (raw, code) in [
            ("", "path_empty"),
            ("tools\\uv.exe", "path_not_absolute"),
            ("..\\uv.exe", "path_not_absolute"),
            ("cmd /c calc", "path_not_absolute"),
        ] {
            assert_eq!(
                validate_path_in(PathKind::Uv, raw, None).unwrap_err().code,
                code,
                "{raw}"
            );
        }
        let missing = d.path().join(exe("nothing"));
        assert_eq!(
            validate_path_in(PathKind::Uv, &missing.to_string_lossy(), None)
                .unwrap_err()
                .code,
            "path_not_found"
        );
        // A folder is not an executable.
        assert_eq!(
            validate_path_in(PathKind::Uv, &d.path().to_string_lossy(), None)
                .unwrap_err()
                .code,
            "path_not_file"
        );
        // Another program than the one asked for.
        let other = fake_exe(d.path(), &exe("notepad"));
        assert_eq!(
            validate_path_in(PathKind::Uv, &other.to_string_lossy(), None)
                .unwrap_err()
                .code,
            "path_wrong_kind"
        );
        assert_eq!(
            validate_path_in(PathKind::LlamaServer, &uv.to_string_lossy(), None)
                .unwrap_err()
                .code,
            "path_wrong_kind"
        );
    }

    #[cfg(windows)]
    #[test]
    fn network_paths_and_files_that_are_not_programs_are_refused_on_windows() {
        assert_eq!(
            validate_path_in(PathKind::Uv, r"\\server\share\uv.exe", None)
                .unwrap_err()
                .code,
            "path_network"
        );
        let d = scratch("notexe");
        let text = d.path().join("uv.exe");
        fs::write(&text, "echo hi").unwrap();
        assert_eq!(
            validate_path_in(PathKind::Uv, &text.to_string_lossy(), None)
                .unwrap_err()
                .code,
            "path_wrong_kind"
        );
        let wrong_ext = d.path().join("uv.bat");
        fs::write(&wrong_ext, "MZ").unwrap();
        assert_eq!(
            validate_path_in(PathKind::Uv, &wrong_ext.to_string_lossy(), None)
                .unwrap_err()
                .code,
            "path_wrong_kind"
        );
    }

    #[test]
    fn a_bare_name_is_looked_up_on_absolute_path_folders_only() {
        let d = scratch("bare");
        let bin = d.path().join("bin");
        let uv = fake_exe(&bin, &exe("uv"));
        let path_var = std::env::join_paths([PathBuf::from("relative-dir"), bin.clone()]).unwrap();
        assert_eq!(
            validate_path_in(PathKind::Uv, "uv", Some(&path_var)).unwrap(),
            uv
        );
        assert_eq!(
            validate_path_in(PathKind::Uv, "uv", None).unwrap_err().code,
            "path_not_found"
        );
        // A relative folder on the path is never searched, even when it holds the file.
        let rel_var = std::env::join_paths([PathBuf::from("bin")]).unwrap();
        assert!(find_on_path(&exe("uv"), Some(&rel_var)).is_none());
        // A bare name of another program is not accepted.
        assert_eq!(
            validate_path_in(PathKind::Uv, "powershell", Some(&path_var))
                .unwrap_err()
                .code,
            "path_wrong_kind"
        );
    }

    #[test]
    fn a_language_model_must_be_a_real_gguf_and_not_a_projector() {
        let d = scratch("gguf");
        let good = fake_gguf(d.path(), "gemma-4-12b-q4_k_m.gguf");
        assert!(validate_path_in(PathKind::LlmModel, &good.to_string_lossy(), None).is_ok());
        let fake = d.path().join("fake.gguf");
        fs::write(&fake, "not a model").unwrap();
        let wrong_ext = d.path().join("model.bin");
        fs::write(&wrong_ext, b"GGUF").unwrap();
        let projector = fake_gguf(d.path(), "mmproj-Qwen3VL-F16.gguf");
        for p in [fake, wrong_ext, projector] {
            assert_eq!(
                validate_path_in(PathKind::LlmModel, &p.to_string_lossy(), None)
                    .unwrap_err()
                    .code,
                "path_wrong_kind",
                "{}",
                p.display()
            );
        }
    }

    #[test]
    fn the_worker_folder_needs_its_project_files_and_the_models_folder_may_be_new() {
        let d = scratch("dirs");
        let worker = d.path().join("worker");
        fs::create_dir_all(&worker).unwrap();
        let raw = worker.to_string_lossy().into_owned();
        assert_eq!(
            validate_path_in(PathKind::WorkerDir, &raw, None)
                .unwrap_err()
                .code,
            "path_wrong_kind"
        );
        fs::write(worker.join("pyproject.toml"), "").unwrap();
        fs::write(worker.join("uv.lock"), "").unwrap();
        assert!(validate_path_in(PathKind::WorkerDir, &raw, None).is_ok());
        let new_models = d.path().join("models-to-be-created");
        assert!(validate_path_in(PathKind::ModelsDir, &new_models.to_string_lossy(), None).is_ok());
        let a_file = d.path().join("file.txt");
        fs::write(&a_file, "x").unwrap();
        assert_eq!(
            validate_path_in(PathKind::ModelsDir, &a_file.to_string_lossy(), None)
                .unwrap_err()
                .code,
            "path_not_dir"
        );
    }

    #[test]
    fn saving_checks_only_the_paths_that_changed() {
        let d = scratch("save");
        let stale = ProcessingSettings {
            llama_server_path: d.path().join(exe("llama-server")).to_string_lossy().into(),
            ..ProcessingSettings::default()
        };
        // The stale path is kept as it is: an unrelated change can be saved.
        let mut next = stale.clone();
        next.gpu_idle_seconds += 1;
        assert!(validate_settings_paths(&next, &stale).is_ok());
        // A new bad value is refused, a new good one is accepted.
        let mut bad = stale.clone();
        bad.uv_path = "C:\\nope\\uv.exe".into();
        assert!(validate_settings_paths(&bad, &stale).is_err());
        let uv = fake_exe(d.path(), &exe("uv"));
        let mut good = stale.clone();
        good.uv_path = uv.to_string_lossy().into();
        assert!(validate_settings_paths(&good, &stale).is_ok());
        // Empty means the default and is always fine.
        let mut empty = stale.clone();
        empty.llama_server_path.clear();
        assert!(validate_settings_paths(&empty, &stale).is_ok());
    }

    #[test]
    fn llama_server_is_detected_in_the_usual_places_and_in_order() {
        let d = scratch("detect");
        let local = d.path().join("local");
        let name = exe("llama-server");
        let winget = fake_exe(
            &local.join("Microsoft/WinGet/Packages/ggml.llamacpp_Microsoft.Winget.Source_8wek"),
            &name,
        );
        let programs = fake_exe(&local.join("Programs/llama.cpp-b9999/bin"), &name);
        let on_path = fake_exe(&d.path().join("tools"), &name);
        let env = DetectEnv {
            path_var: Some(std::env::join_paths([d.path().join("tools")]).unwrap()),
            local_app_data: Some(local.clone()),
            ..DetectEnv::default()
        };
        assert_eq!(
            detect_llama_candidates("", &env),
            vec![on_path.clone(), winget.clone(), programs.clone()]
        );
        // The configured one comes first and is not listed twice.
        let configured = on_path.to_string_lossy().into_owned();
        assert_eq!(
            detect_llama_candidates(&configured, &env)[0],
            on_path,
            "configured first"
        );
        assert_eq!(detect_llama_candidates(&configured, &env).len(), 3);
        // A candidate that is not a program of that name is ignored.
        let junk_dir = local.join("Programs/llama.cpp-old");
        fs::create_dir_all(&junk_dir).unwrap();
        fs::write(junk_dir.join(&name), "text").unwrap();
        let found = detect_llama_candidates("", &env);
        assert!(!found.contains(&junk_dir.join(&name)) || !cfg!(windows));
        // Nothing found.
        assert!(detect_llama_candidates("", &DetectEnv::default()).is_empty());
        assert_eq!(
            resolve_llama("", &DetectEnv::default()),
            LlamaResolution::NotSet
        );
        assert_eq!(resolve_llama("", &env), LlamaResolution::Detected(on_path));
    }

    #[test]
    fn a_configured_llama_server_that_is_wrong_is_reported_not_replaced() {
        let d = scratch("llama-invalid");
        let name = exe("llama-server");
        let good = fake_exe(&d.path().join("tools"), &name);
        let env = DetectEnv {
            path_var: Some(std::env::join_paths([d.path().join("tools")]).unwrap()),
            ..DetectEnv::default()
        };
        match resolve_llama("C:\\gone\\llama-server.exe", &env) {
            LlamaResolution::Invalid { problem, .. } => assert_eq!(problem.code, "path_not_found"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            resolve_llama(&good.to_string_lossy(), &env),
            LlamaResolution::Configured(good)
        );
    }

    #[test]
    fn uv_comes_from_the_settings_then_the_installation_then_the_path() {
        let d = scratch("uv");
        let name = exe("uv");
        let bundled_dir = d.path().join("app");
        let bundled = fake_exe(&bundled_dir.join("bin"), &name);
        let on_path = fake_exe(&d.path().join("tools"), &name);
        let path_var = std::env::join_paths([d.path().join("tools")]).unwrap();
        // The bundled one wins over the path when nothing is configured.
        assert_eq!(
            resolve_uv("", Some(&bundled_dir), Some(&path_var)).unwrap(),
            bundled
        );
        assert_eq!(resolve_uv("", None, Some(&path_var)).unwrap(), on_path);
        let explicit = fake_exe(&d.path().join("mine"), &name);
        assert_eq!(
            resolve_uv(
                &explicit.to_string_lossy(),
                Some(&bundled_dir),
                Some(&path_var)
            )
            .unwrap(),
            explicit
        );
        assert_eq!(
            resolve_uv("", None, None).unwrap_err().code,
            "tool_not_found"
        );
        // A configured but wrong path is an error, not a silent fallback.
        assert_eq!(
            resolve_uv("C:\\gone\\uv.exe", Some(&bundled_dir), Some(&path_var))
                .unwrap_err()
                .code,
            "path_not_found"
        );
    }

    #[test]
    fn gguf_files_are_found_classified_and_checked() {
        let d = scratch("scan");
        fake_gguf(d.path(), "gemma-4-12b-q4_k_m.gguf");
        fake_gguf(&d.path().join("sub/deeper"), "other-model.GGUF");
        fake_gguf(d.path(), "mmproj-Qwen3VL-F16.gguf");
        fs::write(d.path().join("broken.gguf"), "not gguf").unwrap();
        fs::write(d.path().join("readme.txt"), "x").unwrap();
        let found = find_gguf(&[d.path().to_path_buf(), d.path().to_path_buf()]);
        let names: Vec<(String, GgufKind)> = found
            .iter()
            .map(|f| {
                (
                    Path::new(&f.path)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    f.kind,
                )
            })
            .collect();
        // Language models first, the projector last, the broken file and the text file out,
        // and a folder given twice lists its files once.
        assert_eq!(
            names,
            vec![
                ("gemma-4-12b-q4_k_m.gguf".to_string(), GgufKind::Llm),
                ("other-model.GGUF".to_string(), GgufKind::Llm),
                ("mmproj-Qwen3VL-F16.gguf".to_string(), GgufKind::Mmproj),
            ]
        );
        assert!(found.iter().all(|f| f.size > 0));
        assert!(find_gguf(&[d.path().join("missing")]).is_empty());
    }

    #[test]
    fn common_model_folders_are_the_ones_that_exist() {
        let d = scratch("common");
        let home = d.path().join("home");
        fs::create_dir_all(home.join(".lmstudio/models")).unwrap();
        fs::create_dir_all(home.join("Downloads")).unwrap();
        let env = DetectEnv {
            user_profile: Some(home.clone()),
            ..DetectEnv::default()
        };
        let dirs = common_model_dirs("", &env);
        assert_eq!(
            dirs,
            vec![home.join(".lmstudio/models"), home.join("Downloads")]
        );
        let own = d.path().join("my-models");
        fs::create_dir_all(&own).unwrap();
        assert_eq!(
            common_model_dirs(&own.to_string_lossy(), &env)[0],
            own,
            "the models folder of the settings comes first"
        );
    }

    fn item_of<'a>(s: &'a SetupStatus, key: &str) -> &'a SetupItem {
        s.items.iter().find(|i| i.key == key).unwrap()
    }

    #[test]
    fn a_fresh_machine_is_not_ready_and_each_problem_has_a_code_a_message_and_a_fix() {
        let st = build_status(&ProcessingSettings::default(), &Probe::empty());
        assert!(!st.ready && !st.first_run_done);
        let keys: Vec<&str> = st.items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "workerEnv",
                "model:whisper-turbo",
                "model:diarization",
                "hfToken",
                "llamaServer",
                "llmModel",
                "vision"
            ]
        );
        // The three problems the first run hit: no screenshot model, no llama-server, no
        // language model.
        for (key, code) in [
            ("llamaServer", "setup_llama_server"),
            ("llmModel", "setup_llm_model"),
            ("vision", "setup_vision"),
        ] {
            let i = item_of(&st, key);
            assert_eq!(i.state, ItemState::Missing, "{key}");
            assert_eq!(i.code, Some(code));
            assert!(!i.message.is_empty());
            assert!(
                i.fix_hint.as_deref().is_some_and(|h| !h.is_empty()),
                "{key}"
            );
        }
        assert!(st
            .items
            .iter()
            .all(|i| (i.state == ItemState::Ok) == i.code.is_none()));
    }

    fn ready_probe() -> Probe {
        Probe {
            uv: Ok(PathBuf::from("C:/uv.exe")),
            worker_dir: Ok(PathBuf::from("C:/worker")),
            env_ready: true,
            models: Some(
                [
                    ("whisper-turbo", true),
                    ("diarization", true),
                    ("vision", true),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            ),
            hf_token_set: true,
            llama: LlamaResolution::Configured(PathBuf::from("C:/llama/llama-server.exe")),
            llama_version: Some(Ok("version: 4000".into())),
            llm_model: Some(Ok(PathBuf::from("D:/models/gemma.gguf"))),
            ..Probe::empty()
        }
    }

    #[test]
    fn everything_in_place_is_ready() {
        let st = build_status(&ProcessingSettings::default(), &ready_probe());
        assert!(st.ready, "{st:#?}");
        assert!(st.items.iter().all(|i| i.code.is_none()));
        assert!(item_of(&st, "llmModel").message.contains("gemma.gguf"));
    }

    #[test]
    fn skipping_screenshots_and_reports_makes_the_matching_items_skipped() {
        let mut probe = ready_probe();
        probe
            .models
            .as_mut()
            .unwrap()
            .insert("vision".into(), false);
        probe.llm_model = None;
        probe.llama = LlamaResolution::NotSet;
        let s = ProcessingSettings {
            describe_screenshots: false,
            skip_reports: true,
            ..ProcessingSettings::default()
        };
        let st = build_status(&s, &probe);
        assert_eq!(item_of(&st, "vision").state, ItemState::Skipped);
        assert_eq!(item_of(&st, "llmModel").state, ItemState::Skipped);
        // Nothing local needs llama-server any more.
        assert_eq!(item_of(&st, "llamaServer").state, ItemState::Skipped);
        assert!(st.ready, "{st:#?}");
        // With only the screenshots skipped, the managed model still needs it.
        let s = ProcessingSettings {
            describe_screenshots: false,
            ..ProcessingSettings::default()
        };
        assert_eq!(
            item_of(&build_status(&s, &probe), "llamaServer").state,
            ItemState::Missing
        );
    }

    #[test]
    fn the_report_tells_a_missing_file_from_a_wrong_one_and_runs_nothing_it_cannot_explain() {
        let mut probe = ready_probe();
        probe.llm_model = Some(Err(PathProblem::new(
            "path_not_found",
            "nothing exists at X",
        )));
        probe.llama = LlamaResolution::Invalid {
            path: "C:/x".into(),
            problem: PathProblem::new("path_wrong_kind", "not a program"),
        };
        probe.hf_token_set = false;
        probe
            .models
            .as_mut()
            .unwrap()
            .insert("diarization".into(), false);
        let st = build_status(&ProcessingSettings::default(), &probe);
        assert_eq!(item_of(&st, "llmModel").state, ItemState::Missing);
        assert_eq!(item_of(&st, "llamaServer").state, ItemState::Invalid);
        assert!(item_of(&st, "llamaServer")
            .message
            .contains("not a program"));
        assert_eq!(item_of(&st, "hfToken").code, Some("setup_hf_token"));
        assert_eq!(item_of(&st, "model:diarization").state, ItemState::Missing);
        probe.llama = LlamaResolution::Detected(PathBuf::from("C:/l/llama-server.exe"));
        probe.llama_version = Some(Err("no answer within 5 seconds".into()));
        let st = build_status(&ProcessingSettings::default(), &probe);
        assert_eq!(item_of(&st, "llamaServer").state, ItemState::Invalid);
        assert!(item_of(&st, "llamaServer").message.contains("no answer"));
    }

    #[test]
    fn work_in_progress_is_reported_as_working_and_is_not_ready() {
        let mut probe = ready_probe();
        probe.env_working = true;
        probe.models_working = true;
        let st = build_status(&ProcessingSettings::default(), &probe);
        assert_eq!(item_of(&st, "workerEnv").state, ItemState::Working);
        assert_eq!(
            item_of(&st, "model:whisper-turbo").state,
            ItemState::Working
        );
        assert_eq!(item_of(&st, "vision").state, ItemState::Working);
        assert!(!st.ready);
    }

    #[test]
    fn an_external_server_counts_as_a_chosen_language_model() {
        let mut s = ProcessingSettings::default();
        s.llm.default_kind = LlmKind::External;
        let mut probe = ready_probe();
        probe.llm_model = None;
        assert_eq!(
            item_of(&build_status(&s, &probe), "llmModel").state,
            ItemState::Missing
        );
        s.llm.external.base_url = "http://127.0.0.1:11434/v1".into();
        s.llm.external.model = "gemma".into();
        let st = build_status(&s, &probe);
        assert_eq!(item_of(&st, "llmModel").state, ItemState::Ok);
        assert!(item_of(&st, "llmModel").message.contains("127.0.0.1"));
    }

    #[test]
    fn a_job_is_blocked_only_by_what_it_needs() {
        let mut probe = ready_probe();
        probe.llama = LlamaResolution::NotSet;
        probe.llm_model = None;
        probe
            .models
            .as_mut()
            .unwrap()
            .insert("vision".into(), false);
        let st = build_status(&ProcessingSettings::default(), &probe);
        let none = JobNeeds {
            vision: false,
            llm_local: false,
            llm_missing: false,
        };
        // A meeting without screenshots and without a report needs neither.
        assert!(first_blocker(&st, &none).is_none());
        let with_shots = JobNeeds {
            vision: true,
            ..none
        };
        assert_eq!(
            first_blocker(&st, &with_shots).unwrap().code,
            Some("setup_llama_server")
        );
        let report = JobNeeds {
            llm_missing: true,
            ..none
        };
        assert_eq!(
            first_blocker(&st, &report).unwrap().code,
            Some("setup_llm_model")
        );
        // The always-needed pieces come first.
        let mut broken = ready_probe();
        broken.env_ready = false;
        let st = build_status(&ProcessingSettings::default(), &broken);
        assert_eq!(first_blocker(&st, &none).unwrap().key, "workerEnv");
    }

    #[test]
    fn the_report_never_carries_a_token_and_uses_the_camel_case_contract() {
        let st = build_status(&ProcessingSettings::default(), &ready_probe());
        let json = serde_json::to_value(&st).unwrap();
        assert_eq!(json["ready"], true);
        assert_eq!(json["firstRunDone"], false);
        assert!(json["items"][0].get("fixHint").is_none());
        assert_eq!(json["items"][0]["state"], "ok");
        assert!(json["items"][0].get("code").is_none());
        assert!(json.get("llamaCandidates").is_some());
        let text = json.to_string().to_lowercase();
        assert!(!text.contains("hf_"), "no token value");
        let miss = build_status(&ProcessingSettings::default(), &Probe::empty());
        let json = serde_json::to_value(&miss).unwrap();
        assert_eq!(json["items"][0]["code"], "setup_worker_env");
        assert!(json["items"][0]["fixHint"].as_str().is_some_and(|h| !h.is_empty()));
    }

    #[test]
    fn the_environment_is_ready_when_the_interpreter_is_there() {
        let d = scratch("env");
        assert!(!env_ready(d.path()));
        fs::write(d.path().join("pyvenv.cfg"), "home = x").unwrap();
        assert!(!env_ready(d.path()));
        fs::create_dir_all(d.path().join("Scripts")).unwrap();
        fs::write(d.path().join("Scripts/python.exe"), "MZ").unwrap();
        assert!(env_ready(d.path()));
    }

    #[test]
    fn a_missing_program_fails_with_a_message_and_not_with_a_panic() {
        let d = scratch("nouv");
        let gone = d.path().join(exe("uv"));
        let err = run_uv_sync(&gone, d.path(), d.path(), |_| {}).unwrap_err();
        assert!(err.contains("could not be started"), "{err}");
        assert!(probe_llama_version(&gone, Duration::from_secs(1)).is_err());
    }

    #[test]
    fn old_settings_files_get_the_new_defaults() {
        let s: ProcessingSettings =
            serde_json::from_str(r#"{"processAfterRecording":"now","uvPath":"uv"}"#).unwrap();
        assert!(s.describe_screenshots);
        assert!(!s.first_run_done);
        assert!(!s.skip_reports);
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["describeScreenshots"], true);
        assert_eq!(json["firstRunDone"], false);
        assert_eq!(json["skipReports"], false);
    }
}
