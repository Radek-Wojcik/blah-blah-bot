//! IPC commands for project preferences, the guided project wizard, manual audio/video
//! meeting import, local Markdown export, and automatic local model detection.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cycle_store::schema::is_identifier;
use cycle_store::{
    Cycle, DiscordTarget, GithubTarget, LanguageSetting, PublishMode, Repo, ReportMode,
};
use queue::worker::{
    IngestMode, IngestRequest, IngestResponse, IngestScreenshot, IngestSegment, IngestTrack,
    JobOptions,
};
use serde::{Deserialize, Serialize};
use store::{Meeting, MeetingState, ProjectPrefs, ReportSettings, Store};
use tauri::{AppHandle, Emitter, State};

use crate::commands;
use crate::cycle_commands::EVENT_CYCLES;
use crate::cycles::{self, CycleDto, ParticipantSync};
use crate::ipc::{self, IpcError, IpcResult, MeetingDetails};
use crate::llm;
use crate::local_export;
use crate::processing;
use crate::processing_commands::meeting_dir;
use crate::recording;
use crate::secrets;
use crate::settings::{CycleRepoSetting, LastReport, RepoKind, Settings};
use crate::state::AppState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPrefsDto {
    pub cycle_id: String,
    pub local_save_enabled: bool,
    pub local_save_dir: String,
    #[serde(default)]
    pub llm_kind_override: Option<String>,
    #[serde(default)]
    pub llm_model_override: Option<String>,
    /// `keep` or `delete`; absent follows the global "keep audio after processing" setting.
    #[serde(default)]
    pub audio_retention: Option<String>,
}

impl From<ProjectPrefs> for ProjectPrefsDto {
    fn from(p: ProjectPrefs) -> Self {
        Self {
            cycle_id: p.cycle_id,
            local_save_enabled: p.local_save_enabled,
            local_save_dir: p.local_save_dir,
            llm_kind_override: p.llm_kind_override,
            llm_model_override: p.llm_model_override,
            audio_retention: p.audio_retention,
        }
    }
}

impl From<ProjectPrefsDto> for ProjectPrefs {
    fn from(d: ProjectPrefsDto) -> Self {
        Self {
            cycle_id: d.cycle_id.trim().to_string(),
            local_save_enabled: d.local_save_enabled,
            local_save_dir: d.local_save_dir.trim().to_string(),
            llm_kind_override: d
                .llm_kind_override
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            llm_model_override: d
                .llm_model_override
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            audio_retention: d
                .audio_retention
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WizardCreateProjectDto {
    pub mode: String,
    #[serde(default)]
    pub repo_path: Option<String>,
    #[serde(default)]
    pub remote_url: Option<String>,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub output_language: Option<String>,
    #[serde(default)]
    pub default_report_mode: Option<String>,
    #[serde(default)]
    pub github_repo: Option<String>,
    #[serde(default)]
    pub discord_webhook_ref: Option<String>,
    #[serde(default)]
    pub local_save_enabled: Option<bool>,
    #[serde(default)]
    pub local_save_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportMeetingDto {
    pub cycle_id: String,
    pub file_path: String,
    pub media_mode: String,
    pub report_mode: String,
    #[serde(default)]
    pub meeting_type: Option<String>,
    #[serde(default)]
    pub output_language: Option<String>,
    #[serde(default)]
    pub process_now: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedVisionModelDto {
    pub model_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedModelsDto {
    pub llama_server_path: Option<String>,
    pub llm_models: Vec<String>,
    pub vision_models: Vec<String>,
    #[serde(default)]
    pub vision_pairs: Vec<DetectedVisionModelDto>,
    pub default_local_save_dir: String,
}

fn parse_language_setting(s: &str) -> IpcResult<LanguageSetting> {
    match s.trim() {
        "auto" | "" => Ok(LanguageSetting::Auto),
        "pl" => Ok(LanguageSetting::Pl),
        "en" => Ok(LanguageSetting::En),
        other => Err(IpcError::new(
            "invalid",
            format!("unsupported output language {other:?}"),
        )),
    }
}

fn parse_report_mode(s: &str) -> IpcResult<ReportMode> {
    match s.trim() {
        "fixed" | "" => Ok(ReportMode::Fixed),
        "template" => Ok(ReportMode::Template),
        "auto" => Ok(ReportMode::Auto),
        "by-type" => Ok(ReportMode::ByType),
        other => Err(IpcError::new(
            "invalid",
            format!("unsupported report mode {other:?}"),
        )),
    }
}

fn normalize_webhook_ref(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("cred:") {
        trimmed.to_string()
    } else {
        format!("cred:{trimmed}")
    }
}

fn apply_wizard_cycle_overrides(cycle: &mut Cycle, req: &WizardCreateProjectDto) -> IpcResult<()> {
    if let Some(lang) = req
        .output_language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cycle.languages.output = parse_language_setting(lang)?;
    }
    if let Some(mode) = req
        .default_report_mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cycle.report.default_mode = parse_report_mode(mode)?;
    }
    if let Some(gh) = req
        .github_repo
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = gh;
        if cycle.targets.github.is_none() {
            cycle.targets.github = Some(GithubTarget {
                path: cycle_store::DEFAULT_MEETINGS_PATH.to_string(),
                publish: PublishMode::OnApproval,
                issues: true,
            });
        }
    }
    if let Some(wh) = req
        .discord_webhook_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cycle.targets.discord = Some(DiscordTarget {
            webhook: normalize_webhook_ref(wh),
            publish: PublishMode::OnApproval,
            template: None,
        });
    }
    Ok(())
}

fn register_or_update_repo(
    settings: &mut Settings,
    id_hint: &str,
    path: &Path,
    remote: Option<String>,
    kind: RepoKind,
    github_repo: Option<String>,
) -> CycleRepoSetting {
    let canonical_or_raw = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path_str = canonical_or_raw.to_string_lossy().into_owned();
    if let Some(existing) = settings.cycle_repos.iter_mut().find(|r| {
        let existing_canon = fs::canonicalize(&r.path).unwrap_or_else(|_| PathBuf::from(&r.path));
        existing_canon == canonical_or_raw || r.path == path_str
    }) {
        if let Some(gh) = github_repo.filter(|s| !s.trim().is_empty()) {
            existing.github_repo = Some(gh);
        }
        if remote.is_some() && existing.remote.is_none() {
            existing.remote = remote;
        }
        return existing.clone();
    }

    let mut repo_id = format!("project-{}", id_hint.to_ascii_lowercase());
    let mut n = 2u32;
    while settings.cycle_repos.iter().any(|r| r.id == repo_id) {
        repo_id = format!("project-{}-{n}", id_hint.to_ascii_lowercase());
        n += 1;
    }
    let setting = CycleRepoSetting {
        id: repo_id,
        path: path_str,
        remote,
        kind,
        github_repo: github_repo.filter(|s| !s.trim().is_empty()),
        github_api: None,
    };
    settings.cycle_repos.push(setting.clone());
    setting
}

/// The checks of the project id and name that every wizard path makes before anything is
/// created (here or on GitHub).
pub(crate) fn validate_wizard_basics(id: &str, name: &str) -> IpcResult<()> {
    let (id, name) = (id.trim(), name.trim());
    if !is_identifier(id) {
        return Err(IpcError::new(
            "invalid",
            "the project id may contain letters, digits, - and _ and must start with a letter or digit",
        ));
    }
    if name.is_empty() || name.chars().count() > 120 {
        return Err(IpcError::new(
            "invalid",
            "enter a project name of 1 to 120 characters",
        ));
    }
    Ok(())
}

/// Pure implementation of `wizard_create_project` for unit testing and IPC command use.
pub fn wizard_create_project_in<S>(
    settings: &mut Settings,
    store: &Store,
    sync: &S,
    data_dir: &Path,
    req: WizardCreateProjectDto,
) -> IpcResult<CycleDto>
where
    S: ParticipantSync + ?Sized,
{
    validate_wizard_basics(&req.id, &req.name)?;
    let id = req.id.trim();
    let name = req.name.trim();

    let mode = req.mode.trim();
    let github_repo = req
        .github_repo
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let target_setting = match mode {
        "auto" => {
            let repo_dir = req
                .repo_path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("projects").join(id));
            fs::create_dir_all(&repo_dir)?;
            Repo::init_local(&repo_dir)?;
            register_or_update_repo(settings, id, &repo_dir, None, RepoKind::Local, github_repo)
        }
        "existing" => {
            let remote = req
                .remote_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let local_path = req
                .repo_path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(url) = remote {
                let checked_url = cycles::validate_remote(url)?;
                let dest = match local_path {
                    Some(p) => PathBuf::from(p),
                    None => cycles::plan_clone(settings, data_dir, &checked_url).1,
                };
                if !dest.join(".git").exists() {
                    Repo::clone(&checked_url, &dest)?;
                }
                register_or_update_repo(
                    settings,
                    id,
                    &dest,
                    Some(checked_url),
                    RepoKind::Clone,
                    github_repo,
                )
            } else if let Some(p) = local_path {
                let dir = PathBuf::from(p);
                if !dir.exists() {
                    fs::create_dir_all(&dir)?;
                }
                let repo = match Repo::open(&dir) {
                    Ok(r) => r,
                    Err(_) => Repo::init_local(&dir)?,
                };
                let _ = repo;
                register_or_update_repo(settings, id, &dir, None, RepoKind::Local, github_repo)
            } else {
                return Err(IpcError::new(
                    "invalid",
                    "provide an existing folder path or a remote repository URL",
                ));
            }
        }
        other => {
            return Err(IpcError::new(
                "invalid",
                format!("unknown wizard mode {other:?}; expected \"auto\" or \"existing\""),
            ))
        }
    };

    let repo = Repo::open(&target_setting.path)?;
    let cycle_file = repo.cycle_dir(id)?.join("cycle.yaml");
    let mut cycle = if cycle_file.is_file() {
        let mut loaded = repo.load_cycle(id)?;
        if !name.is_empty() {
            loaded.name = name.to_string();
        }
        loaded
    } else {
        let fresh = Cycle::new(id, name);
        repo.create_cycle(&fresh)?;
        repo.load_cycle(id)?
    };

    apply_wizard_cycle_overrides(&mut cycle, &req)?;
    repo.save_cycle(&cycle)?;

    let _ = cycles::sync_cycle(&settings.cycle_repos, sync, id);

    let default_save_dir = local_export::default_local_save_dir(data_dir)
        .to_string_lossy()
        .into_owned();
    let prefs = ProjectPrefs {
        cycle_id: id.to_string(),
        local_save_enabled: req.local_save_enabled.unwrap_or(false),
        local_save_dir: req
            .local_save_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if req.local_save_enabled == Some(true) {
                    default_save_dir
                } else {
                    String::new()
                }
            }),
        llm_kind_override: None,
        llm_model_override: None,
        audio_retention: None,
    };
    store.set_project_prefs(&prefs)?;

    let mode_str = match cycle.report.default_mode {
        ReportMode::Fixed => "fixed",
        ReportMode::Template => "template",
        ReportMode::Auto => "auto",
        ReportMode::ByType => "by-type",
    };
    settings.last_report = Some(LastReport {
        cycle_id: id.to_string(),
        mode: mode_str.to_string(),
        template: (cycle.report.default_mode == ReportMode::Template)
            .then(|| cycle.report.default_template.clone()),
        meeting_type: (cycle.report.default_mode == ReportMode::ByType)
            .then(|| cycle.report.by_type.keys().next().cloned())
            .flatten(),
    });

    Ok(CycleDto::from(&cycle))
}

pub fn get_project_prefs_in(store: &Store, cycle_id: &str) -> IpcResult<ProjectPrefsDto> {
    let cid = cycle_id.trim();
    if cid.is_empty() {
        return Err(IpcError::new("invalid", "cycle_id cannot be empty"));
    }
    Ok(store.get_project_prefs(cid)?.into())
}

pub fn save_project_prefs_in(store: &Store, prefs: ProjectPrefsDto) -> IpcResult<ProjectPrefsDto> {
    let cid = prefs.cycle_id.trim().to_string();
    if cid.is_empty() {
        return Err(IpcError::new("invalid", "cycle_id cannot be empty"));
    }
    let domain: ProjectPrefs = prefs.into();
    store.set_project_prefs(&domain).map_err(|e| match e {
        store::StoreError::Invalid(msg) => IpcError::new("invalid", msg),
        other => IpcError::from(other),
    })?;
    Ok(store.get_project_prefs(&cid)?.into())
}

fn next_imported_meeting_id(store: &Store) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let base_ms = queue::now_ms().max(0) as u64;
    for offset in 0..1000 {
        let candidate = recording::meeting_id(
            base_ms + offset * 1000 + SEQ.fetch_add(1, Ordering::Relaxed) * 1000,
        );
        if store.get_meeting(&candidate).ok().flatten().is_none() {
            return candidate;
        }
    }
    format!(
        "{}-{}",
        recording::meeting_id(base_ms),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Write a valid 2-second 16 kHz mono WAV sine tone file to `path`.
pub fn write_demo_wav(path: &Path, hz: f64, duration_secs: usize) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let sample_rate = 16_000usize;
    let total_samples = sample_rate * duration_secs;
    let data_len = (total_samples * 2) as u32;
    let mut out = Vec::with_capacity(44 + total_samples * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(sample_rate as u32).to_le_bytes());
    out.extend_from_slice(&((sample_rate * 2) as u32).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    let step = 2.0 * std::f64::consts::PI * hz / (sample_rate as f64);
    for i in 0..total_samples {
        let sample = ((step * i as f64).sin() * 8_000.0) as i16;
        out.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, out)
}

fn synthetic_ingest(
    store: &Store,
    meeting_id: &str,
    meeting_dir: &Path,
    mode: IngestMode,
) -> IpcResult<()> {
    let seg_rel = "imported/seg-00000.wav";
    write_demo_wav(&meeting_dir.join(seg_rel), 440.0, 2)?;
    let mut screenshots = Vec::new();
    if mode == IngestMode::AudioAndSlides {
        let frame = recorder::screenshot::Frame {
            rect: recorder::screenshot::Rect::new(0, 0, 16, 9),
            rgba: vec![120u8; 16 * 9 * 4],
        };
        let img_rel = "img/slide-0001.png";
        fs::create_dir_all(meeting_dir.join("img"))?;
        let _ = recorder::screenshot::write_png(&frame, &meeting_dir.join(img_rel));
        screenshots.push(IngestScreenshot {
            file: img_rel.to_string(),
            offset_ms: 500.0,
        });
    }
    let resp = IngestResponse {
        meeting_id: meeting_id.to_string(),
        duration_ms: 2_000,
        tracks: vec![IngestTrack {
            name: "imported".into(),
            source: "file:imported".into(),
            offset_ms: 0.0,
            segments: vec![seg_rel.to_string()],
        }],
        segments: vec![IngestSegment {
            track: "imported".into(),
            segment: 0,
            file: seg_rel.to_string(),
            offset_ms: 0.0,
            duration_ms: 2_000,
            samples: 32_000,
        }],
        screenshots,
    };
    queue::worker::record_ingest(store, meeting_id, &resp)?;
    Ok(())
}

fn is_gguf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gguf"))
}

/// True when `vl` appears as a token, as in `Qwen3-VL-8B`, `Qwen3VL-8B`, `qwen2.5vl-7b`.
fn has_vl_token(name_lower: &str) -> bool {
    let bytes = name_lower.as_bytes();
    let mut from = 0;
    while let Some(pos) = name_lower[from..].find("vl") {
        let start = from + pos;
        let end = start + 2;
        let before_ok = start > 0 && {
            let b = bytes[start - 1];
            b.is_ascii_digit() || b == b'-' || b == b'_' || b == b'.'
        };
        let after_ok = end >= bytes.len() || matches!(bytes[end], b'-' | b'_' | b'.');
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

fn is_vision_model_name(name_lower: &str) -> bool {
    has_vl_token(name_lower)
        || name_lower.contains("vision")
        || name_lower.contains("llava")
        || name_lower.contains("pixtral")
        || name_lower.contains("minicpm-v")
}

fn scan_models_in_dir(
    dir: &Path,
    depth: u32,
    llm_out: &mut BTreeSet<String>,
    vision_out: &mut BTreeSet<String>,
) {
    if depth > 3 || !dir.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_models_in_dir(&path, depth + 1, llm_out, vision_out);
            continue;
        }
        if !is_gguf_path(&path) {
            continue;
        }
        let name_lower = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name_lower.contains("mmproj") {
            continue;
        }
        let full = path.to_string_lossy().into_owned();
        if is_vision_model_name(&name_lower) {
            vision_out.insert(full);
        } else {
            llm_out.insert(full);
        }
    }
}

fn hf_cache_root_from(
    hf_hub_cache: Option<std::ffi::OsString>,
    hf_home: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let non_empty = |v: std::ffi::OsString| {
        let trimmed = v.to_string_lossy().trim().to_string();
        (!trimmed.is_empty()).then(|| std::ffi::OsString::from(trimmed))
    };
    if let Some(hub) = hf_hub_cache.and_then(non_empty) {
        return Some(PathBuf::from(hub));
    }
    if let Some(home) = hf_home.and_then(non_empty) {
        return Some(PathBuf::from(home).join("hub"));
    }
    user_profile.and_then(non_empty).map(|u| {
        PathBuf::from(u)
            .join(".cache")
            .join("huggingface")
            .join("hub")
    })
}

fn hf_cache_root() -> Option<PathBuf> {
    hf_cache_root_from(
        std::env::var_os("HF_HUB_CACHE"),
        std::env::var_os("HF_HOME"),
        std::env::var_os("USERPROFILE"),
    )
}

fn scan_hf_snapshot_dir(
    dir: &Path,
    cache_canon: &Path,
    depth: u32,
    llm_out: &mut BTreeSet<String>,
    vision_out: &mut BTreeSet<String>,
    vision_pairs_out: &mut BTreeMap<String, Option<String>>,
) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut subdirs = Vec::new();
    let mut mmproj_files = BTreeSet::new();
    let mut gguf_files = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let file_type = meta.file_type();
        let name_lower = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if file_type.is_symlink() {
            let Ok(target) = fs::canonicalize(&path) else {
                continue;
            };
            if !target.starts_with(cache_canon) || !target.is_file() {
                continue;
            }
            if is_gguf_path(&path) {
                let full = path.to_string_lossy().into_owned();
                if name_lower.contains("mmproj") {
                    mmproj_files.insert(full);
                } else {
                    gguf_files.push((full, name_lower));
                }
            }
            continue;
        }

        if file_type.is_dir() {
            if name_lower == "blobs" || name_lower == "refs" {
                continue;
            }
            subdirs.push(path);
            continue;
        }

        if file_type.is_file() && is_gguf_path(&path) {
            let full = path.to_string_lossy().into_owned();
            if name_lower.contains("mmproj") {
                mmproj_files.insert(full);
            } else {
                gguf_files.push((full, name_lower));
            }
        }
    }

    let paired_mmproj = mmproj_files.iter().next().cloned();
    for (full, name_lower) in gguf_files {
        if let Some(proj) = &paired_mmproj {
            vision_out.insert(full.clone());
            vision_pairs_out.insert(full, Some(proj.clone()));
        } else if is_vision_model_name(&name_lower) {
            vision_out.insert(full.clone());
            vision_pairs_out.entry(full).or_insert(None);
        } else {
            llm_out.insert(full);
        }
    }

    for sub in subdirs {
        scan_hf_snapshot_dir(
            &sub,
            cache_canon,
            depth + 1,
            llm_out,
            vision_out,
            vision_pairs_out,
        );
    }
}

fn scan_hf_cache(
    cache_root: &Path,
    llm_out: &mut BTreeSet<String>,
    vision_out: &mut BTreeSet<String>,
    vision_pairs_out: &mut BTreeMap<String, Option<String>>,
) {
    let Ok(cache_canon) = fs::canonicalize(cache_root) else {
        return;
    };
    if !cache_canon.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(cache_root) else {
        return;
    };
    for entry in entries.flatten() {
        let repo_dir = entry.path();
        let Ok(meta) = fs::symlink_metadata(&repo_dir) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
            continue;
        }
        let name = repo_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !name.starts_with("models--") {
            continue;
        }
        let snapshots_dir = repo_dir.join("snapshots");
        let Ok(snap_meta) = fs::symlink_metadata(&snapshots_dir) else {
            continue;
        };
        if snap_meta.file_type().is_symlink() || !snap_meta.file_type().is_dir() {
            continue;
        }
        let Ok(snaps) = fs::read_dir(&snapshots_dir) else {
            continue;
        };
        for snap in snaps.flatten() {
            let snap_path = snap.path();
            let Ok(rev_meta) = fs::symlink_metadata(&snap_path) else {
                continue;
            };
            if rev_meta.file_type().is_symlink() || !rev_meta.file_type().is_dir() {
                continue;
            }
            let rev_name = snap_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if rev_name == "blobs" || rev_name == "refs" {
                continue;
            }
            scan_hf_snapshot_dir(
                &snap_path,
                &cache_canon,
                3,
                llm_out,
                vision_out,
                vision_pairs_out,
            );
        }
    }
}

fn find_llama_server(configured: &str, data_dir: &Path) -> Option<String> {
    let trimmed = configured.trim();
    if !trimmed.is_empty() && Path::new(trimmed).is_file() {
        return Some(trimmed.to_string());
    }
    if let Some(env_path) = std::env::var_os("BBB_LLAMA_SERVER") {
        let p = PathBuf::from(env_path);
        if p.is_file() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    let exe_name = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    let mut candidates = vec![
        data_dir.join("bin").join(exe_name),
        data_dir.join("llama").join(exe_name),
        PathBuf::from("tools").join("llama").join(exe_name),
    ];
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors().skip(1).take(4) {
            candidates.push(anc.join(exe_name));
            candidates.push(anc.join("bin").join(exe_name));
            candidates.push(anc.join("tools").join("llama").join(exe_name));
        }
    }
    if let Some(local_app) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(local_app);
        candidates.push(base.join("Programs").join("llama.cpp").join(exe_name));
        candidates.push(base.join("llama.cpp").join(exe_name));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let base = PathBuf::from(home);
        candidates.push(base.join("llama.cpp").join(exe_name));
        candidates.push(base.join("scoop").join("shims").join(exe_name));
    }
    for cand in candidates {
        if cand.is_file() {
            return Some(cand.to_string_lossy().into_owned());
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join(exe_name);
            if cand.is_file() {
                return Some(cand.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Collapse entries that point at the same file (for example a models folder that is also in the
/// HF cache) and make sure a vision model never shows up in the LLM list.
fn dedupe_detected(
    llm: &mut BTreeSet<String>,
    vision: &mut BTreeSet<String>,
    pairs: &mut BTreeMap<String, Option<String>>,
) {
    let canon = |p: &str| fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));

    // Vision: keep one entry per real file, preferring one that has an mmproj pair.
    let mut vision_by_canon: BTreeMap<PathBuf, String> = BTreeMap::new();
    for v in std::mem::take(vision) {
        let key = canon(&v);
        match vision_by_canon.get(&key) {
            Some(kept) => {
                let kept_has = pairs.get(kept).is_some_and(|m| m.is_some());
                let this_has = pairs.get(&v).is_some_and(|m| m.is_some());
                if this_has && !kept_has {
                    pairs.remove(kept);
                    vision_by_canon.insert(key, v);
                } else {
                    pairs.remove(&v);
                }
            }
            None => {
                vision_by_canon.insert(key, v);
            }
        }
    }
    let vision_canon: BTreeSet<PathBuf> = vision_by_canon.keys().cloned().collect();
    *vision = vision_by_canon.into_values().collect();

    // LLM: drop anything that is a vision model, and collapse duplicates.
    let mut seen = BTreeSet::new();
    *llm = std::mem::take(llm)
        .into_iter()
        .filter(|m| {
            let key = canon(m);
            !vision_canon.contains(&key) && seen.insert(key)
        })
        .collect();
}

pub fn auto_detect_models_in(
    proc_settings: &processing::ProcessingSettings,
    data_dir: &Path,
    search_dirs: &[PathBuf],
    hf_cache_dir: Option<&Path>,
) -> DetectedModelsDto {
    let llama_server_path = find_llama_server(&proc_settings.llama_server_path, data_dir);
    let mut llm_models = BTreeSet::new();
    let mut vision_models = BTreeSet::new();
    let mut vision_pairs_map = BTreeMap::new();

    if !proc_settings.llm.managed.model_path.trim().is_empty() {
        let p = Path::new(proc_settings.llm.managed.model_path.trim());
        if p.is_file() {
            llm_models.insert(p.to_string_lossy().into_owned());
        }
    }
    if !proc_settings.vision_model_path.trim().is_empty() {
        let p = Path::new(proc_settings.vision_model_path.trim());
        if p.is_file() {
            let full = p.to_string_lossy().into_owned();
            vision_models.insert(full.clone());
            vision_pairs_map.entry(full).or_insert(None);
        }
    }

    for dir in search_dirs {
        scan_models_in_dir(dir, 0, &mut llm_models, &mut vision_models);
    }
    for v in &vision_models {
        vision_pairs_map.entry(v.clone()).or_insert(None);
    }
    if let Some(cache_root) = hf_cache_dir {
        scan_hf_cache(
            cache_root,
            &mut llm_models,
            &mut vision_models,
            &mut vision_pairs_map,
        );
    }

    dedupe_detected(&mut llm_models, &mut vision_models, &mut vision_pairs_map);

    let vision_pairs = vision_pairs_map
        .into_iter()
        .map(|(model_path, mmproj_path)| DetectedVisionModelDto {
            model_path,
            mmproj_path,
        })
        .collect();

    DetectedModelsDto {
        llama_server_path,
        llm_models: llm_models.into_iter().collect(),
        vision_models: vision_models.into_iter().collect(),
        vision_pairs,
        default_local_save_dir: local_export::default_local_save_dir(data_dir)
            .to_string_lossy()
            .into_owned(),
    }
}

pub fn auto_detect_models_with(
    proc_settings: &processing::ProcessingSettings,
    data_dir: &Path,
) -> DetectedModelsDto {
    let mut search_dirs = Vec::new();
    if !proc_settings.models_dir.trim().is_empty() {
        search_dirs.push(PathBuf::from(proc_settings.models_dir.trim()));
    }
    if let Some(env_dir) = std::env::var_os("BBB_MODELS_DIR") {
        search_dirs.push(PathBuf::from(env_dir));
    }
    search_dirs.push(data_dir.join("models"));
    if let Some(parent) = data_dir.parent() {
        search_dirs.push(parent.join("models"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let base = PathBuf::from(home);
        search_dirs.push(base.join("models"));
        search_dirs.push(base.join(".cache").join("lm-studio").join("models"));
    }
    let hf_root = hf_cache_root();
    auto_detect_models_in(proc_settings, data_dir, &search_dirs, hf_root.as_deref())
}

// ----- Tauri commands ----------------------------------------------------------------------

#[tauri::command]
pub fn get_project_prefs(
    state: State<'_, AppState>,
    cycle_id: String,
) -> IpcResult<ProjectPrefsDto> {
    state.with_store_ipc(|s| get_project_prefs_in(s, &cycle_id))
}

#[tauri::command]
pub fn save_project_prefs(
    state: State<'_, AppState>,
    prefs: ProjectPrefsDto,
) -> IpcResult<ProjectPrefsDto> {
    state.with_store_ipc(|s| save_project_prefs_in(s, prefs))
}

#[tauri::command]
pub fn wizard_create_project(
    app: AppHandle,
    state: State<'_, AppState>,
    req: WizardCreateProjectDto,
) -> IpcResult<CycleDto> {
    let data_dir = state.data_dir().to_path_buf();
    let dto = state.update_settings(|settings| {
        state.with_store_ipc(|store| {
            wizard_create_project_in(settings, store, store, &data_dir, req)
        })
    })?;
    let _ = app.emit(EVENT_CYCLES, ());
    Ok(dto)
}

#[tauri::command]
pub fn import_meeting(
    app: AppHandle,
    state: State<'_, AppState>,
    req: ImportMeetingDto,
) -> IpcResult<MeetingDetails> {
    let cycle_id = req.cycle_id.trim().to_string();
    if cycle_id.is_empty() {
        return Err(IpcError::new("invalid", "cycleId cannot be empty"));
    }
    let file_path = req.file_path.trim().to_string();
    if file_path.is_empty() {
        return Err(IpcError::new("invalid", "filePath cannot be empty"));
    }
    let mode = IngestMode::parse(req.media_mode.trim()).ok_or_else(|| {
        IpcError::new(
            "invalid",
            "mediaMode must be \"audio_only\" or \"audio_and_slides\"",
        )
    })?;
    let report_mode = if req.report_mode.trim().is_empty() {
        "fixed".to_string()
    } else {
        req.report_mode.trim().to_string()
    };

    let root = state.recordings_root()?;
    let id = state.with_store(|s| Ok(next_imported_meeting_id(s)))?;
    let meeting_dir_path = root.join(&id);
    fs::create_dir_all(&meeting_dir_path)?;

    let stem_title = Path::new(&file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| recording::default_title(queue::now_ms().max(0) as u64));

    let started = queue::now_ms();
    let report_settings = ReportSettings {
        cycle_id: Some(cycle_id.clone()),
        report_mode: Some(report_mode),
        report_template: None,
        meeting_type: req
            .meeting_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        output_language: req
            .output_language
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    };

    state.with_store(|s| {
        s.create_meeting(&Meeting {
            id: id.clone(),
            title: stem_title,
            started_unix_ms: started,
            duration_ms: None,
            state: MeetingState::Stopped,
            dir: id.clone(),
        })?;
        s.set_report_settings(&id, &report_settings)?;
        Ok(())
    })?;

    let demo = state.is_demo() || std::env::var_os("BBB_DEMO").is_some();
    if demo {
        state.with_store_ipc(|s| synthetic_ingest(s, &id, &meeting_dir_path, mode))?;
    } else {
        let cfg = state.processing().worker_config();
        let ingest_req = IngestRequest {
            meeting_id: id.clone(),
            input_path: PathBuf::from(&file_path),
            meeting_dir: meeting_dir_path.clone(),
            mode,
        };
        let ingest_res = state.with_store_ipc(|s| {
            queue::runner::ingest_meeting(s, &cfg, None, &ingest_req, |_| {})
                .map_err(|e| IpcError::new("io", e.to_string()))
        });
        if let Err(e) = ingest_res {
            let _ = state.with_store(|s| s.delete_meeting(&id));
            return Err(e);
        }
    }

    let mut job_opts = JobOptions::default();
    if let Some(lang) = report_settings.output_language.as_deref() {
        if ["auto", "pl", "en"].contains(&lang) {
            job_opts.language = lang.to_string();
        }
    }
    let llm_settings = state.processing().llm_settings()?;
    job_opts.llm = state
        .with_store_ipc(|s| {
            llm::choose_llm(s, &id, &llm_settings, None, &|name| {
                secrets::get(name).is_some()
            })
        })
        .ok()
        .flatten();
    state.with_store_ipc(|s| {
        processing::apply_project_prefs_to_job_options(s, &id, &llm_settings, &mut job_opts)
    })?;

    if req.process_now {
        state.processing().process(&id, job_opts)?;
    } else {
        state.processing().enqueue_only(&id, job_opts)?;
    }

    commands::emit_meetings_changed(&app);
    let jobs = state.processing().job_states();
    state.with_store_ipc(|s| {
        let details = s
            .get_meeting(&id)?
            .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?;
        let dir = details.meeting.dir.clone();
        let mut dto = MeetingDetails::from(details);
        dto.summary.job = jobs.get(&dto.summary.id).copied();
        ipc::enrich_meeting_summary(s, &root, &dir, &mut dto.summary);
        Ok(dto)
    })
}

#[tauri::command]
pub fn export_meeting_markdown(
    state: State<'_, AppState>,
    meeting_id: String,
    dest_path: Option<String>,
) -> IpcResult<String> {
    let dir = meeting_dir(&state, &meeting_id)?;
    let repos = state.cycle_repos();
    let settings = state.settings()?;
    let data_dir = state.data_dir().to_path_buf();
    state.with_store_ipc(|s| {
        local_export::export_meeting_markdown_with(
            &repos,
            s,
            &settings,
            &data_dir,
            &meeting_id,
            &dir,
            dest_path.as_deref(),
        )
    })
}

/// Write the report note of a meeting to a `.md` file chosen in the "Save as" dialog.
#[tauri::command]
pub fn save_meeting_note_as(
    state: State<'_, AppState>,
    meeting_id: String,
    dest_path: String,
) -> IpcResult<String> {
    let dir = meeting_dir(&state, &meeting_id)?;
    let repos = state.cycle_repos();
    state.with_store_ipc(|s| {
        local_export::save_meeting_note_as(&repos, s, &meeting_id, &dir, &dest_path)
    })
}

#[tauri::command]
pub fn auto_detect_models(state: State<'_, AppState>) -> IpcResult<DetectedModelsDto> {
    let proc_settings = state.processing().settings()?;
    Ok(auto_detect_models_with(&proc_settings, state.data_dir()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_create_project_and_project_prefs_round_trip() {
        let dir = test_tmp::TestDir::new("wizard-project");
        let store = Store::open_in_memory().unwrap();
        let mut settings = Settings::default();

        let dto = wizard_create_project_in(
            &mut settings,
            &store,
            &store,
            dir.path(),
            WizardCreateProjectDto {
                mode: "auto".into(),
                repo_path: None,
                remote_url: None,
                id: "ALPHA".into(),
                name: "Alpha Project".into(),
                output_language: Some("en".into()),
                default_report_mode: Some("auto".into()),
                github_repo: Some("org/alpha".into()),
                discord_webhook_ref: Some("alpha-hook".into()),
                local_save_enabled: Some(true),
                local_save_dir: Some(dir.path().join("exports").to_string_lossy().into_owned()),
            },
        )
        .unwrap();

        assert_eq!(dto.id, "ALPHA");
        assert_eq!(dto.name, "Alpha Project");
        assert_eq!(dto.languages.output, LanguageSetting::En);
        assert_eq!(dto.report.default_mode, ReportMode::Auto);
        assert_eq!(
            dto.targets.discord.as_ref().map(|d| d.webhook.as_str()),
            Some("cred:alpha-hook")
        );
        assert_eq!(settings.cycle_repos.len(), 1);
        assert_eq!(
            settings.cycle_repos[0].github_repo.as_deref(),
            Some("org/alpha")
        );
        assert!(dir
            .path()
            .join("projects")
            .join("ALPHA")
            .join(".git")
            .exists());

        let prefs = get_project_prefs_in(&store, "ALPHA").unwrap();
        assert!(prefs.local_save_enabled);
        assert!(prefs.local_save_dir.ends_with("exports"));

        let saved = save_project_prefs_in(
            &store,
            ProjectPrefsDto {
                cycle_id: "ALPHA".into(),
                local_save_enabled: false,
                local_save_dir: "D:/Notes".into(),
                llm_kind_override: Some("external".into()),
                llm_model_override: Some("gemma3".into()),
                audio_retention: Some("keep".into()),
            },
        )
        .unwrap();
        assert!(!saved.local_save_enabled);
        assert_eq!(saved.local_save_dir, "D:/Notes");
        assert_eq!(saved.llm_kind_override.as_deref(), Some("external"));
        assert_eq!(saved.llm_model_override.as_deref(), Some("gemma3"));
        assert_eq!(saved.audio_retention.as_deref(), Some("keep"));
        // A new project follows the global setting; a bad value is refused.
        assert_eq!(prefs.audio_retention, None);
        let bad = save_project_prefs_in(
            &store,
            ProjectPrefsDto {
                cycle_id: "ALPHA".into(),
                local_save_enabled: false,
                local_save_dir: String::new(),
                llm_kind_override: None,
                llm_model_override: None,
                audio_retention: Some("forever".into()),
            },
        );
        assert!(bad.is_err());
        // The frontend contract is camelCase and old payloads without the field still parse.
        let old: ProjectPrefsDto = serde_json::from_str(
            r#"{"cycleId":"ALPHA","localSaveEnabled":false,"localSaveDir":""}"#,
        )
        .unwrap();
        assert_eq!(old.audio_retention, None);
        assert_eq!(
            serde_json::to_value(&saved).unwrap()["audioRetention"],
            "keep"
        );
    }

    #[test]
    fn export_meeting_markdown_writes_standalone_and_auto_folder_exports() {
        let dir = test_tmp::TestDir::new("export-md");
        let store = Store::open_in_memory().unwrap();
        let settings = Settings {
            local_save_enabled: true,
            local_save_dir: dir.path().join("notes").to_string_lossy().into_owned(),
            ..Settings::default()
        };
        let meeting_dir = dir.path().join("rec").join("m1");
        fs::create_dir_all(meeting_dir.join("work")).unwrap();
        fs::create_dir_all(meeting_dir.join("img")).unwrap();
        fs::write(meeting_dir.join("img").join("slide1.png"), b"png-bytes").unwrap();
        fs::write(
            meeting_dir.join("work").join("draft.json"),
            serde_json::json!({
                "version": 1,
                "meeting_id": "m1",
                "cycle": "ALPHA",
                "date": "2026-11-24",
                "language": "en",
                "mode": "fixed",
                "template": null,
                "meeting_type": null,
                "sections": [],
                "short_title": "Alpha Sprint Review",
                "key_screenshots": ["img/slide1.png"],
                "summary": "Finalized the launch checklist.",
                "items": [],
                "warnings": []
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            meeting_dir.join("work").join("transcript.json"),
            serde_json::json!({
                "version": 1,
                "meeting_id": "m1",
                "language": "en",
                "speakers": [{"id": "s1", "label": "Alice", "kind": "user"}],
                "segments": [{"id": 1, "track": "mic", "speaker": "s1", "start_ms": 1000, "end_ms": 4000, "text": "Hello team.", "words": []}]
            })
            .to_string(),
        )
        .unwrap();

        store
            .create_meeting(&Meeting {
                id: "m1".into(),
                title: "Weekly Sync".into(),
                started_unix_ms: 1_795_500_000_000,
                duration_ms: Some(120_000),
                state: MeetingState::Stopped,
                dir: "m1".into(),
            })
            .unwrap();
        store
            .set_report_settings(
                "m1",
                &ReportSettings {
                    cycle_id: Some("ALPHA".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        let standalone = dir.path().join("custom").join("note.md");
        let out_path = local_export::export_meeting_markdown_with(
            &[],
            &store,
            &settings,
            dir.path(),
            "m1",
            &meeting_dir,
            Some(&standalone.to_string_lossy()),
        )
        .unwrap();
        assert_eq!(PathBuf::from(&out_path), standalone);
        assert!(standalone.is_file());
        let note_text = fs::read_to_string(&standalone).unwrap();
        assert!(note_text.contains("Finalized the launch checklist."));

        let auto_dir = dir.path().join("notes").join("ALPHA").join("2026-11-24");
        assert!(auto_dir.join("report.md").is_file());
        assert!(auto_dir.join("transcript.md").is_file());
        assert!(auto_dir.join("img").join("slide1.png").is_file());
        let transcript_text = fs::read_to_string(auto_dir.join("transcript.md")).unwrap();
        assert!(transcript_text.contains("Hello team."));
    }

    #[test]
    fn auto_detect_models_finds_gguf_and_llama_server() {
        let dir = test_tmp::TestDir::new("detect-models");
        let models_dir = dir.path().join("models");
        fs::create_dir_all(&models_dir).unwrap();
        fs::write(models_dir.join("gemma-3-12b-q4.gguf"), b"gguf").unwrap();
        fs::write(models_dir.join("qwen3-vl-8b-q4.gguf"), b"gguf").unwrap();
        fs::write(models_dir.join("mmproj-model-f16.gguf"), b"gguf").unwrap();
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let exe = if cfg!(windows) {
            bin_dir.join("llama-server.exe")
        } else {
            bin_dir.join("llama-server")
        };
        fs::write(&exe, b"bin").unwrap();

        let proc_settings = processing::ProcessingSettings {
            models_dir: models_dir.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let detected = auto_detect_models_in(&proc_settings, dir.path(), &[models_dir], None);
        assert_eq!(
            detected.llama_server_path.as_deref(),
            Some(exe.to_string_lossy().as_ref())
        );
        assert_eq!(detected.llm_models.len(), 1);
        assert!(detected.llm_models[0].contains("gemma-3-12b-q4.gguf"));
        assert_eq!(detected.vision_models.len(), 1);
        assert!(detected.vision_models[0].contains("qwen3-vl-8b-q4.gguf"));
    }

    #[test]
    fn hf_cache_root_prefers_hub_cache_then_hf_home_then_userprofile() {
        use std::ffi::OsString;
        assert_eq!(
            hf_cache_root_from(
                Some(OsString::from("D:/hf-hub")),
                Some(OsString::from("D:/hf-home")),
                Some(OsString::from("C:/Users/demo")),
            ),
            Some(PathBuf::from("D:/hf-hub"))
        );
        assert_eq!(
            hf_cache_root_from(
                None,
                Some(OsString::from("D:/hf-home")),
                Some(OsString::from("C:/Users/demo")),
            ),
            Some(PathBuf::from("D:/hf-home").join("hub"))
        );
        assert_eq!(
            hf_cache_root_from(None, None, Some(OsString::from("C:/Users/demo"))),
            Some(
                PathBuf::from("C:/Users/demo")
                    .join(".cache")
                    .join("huggingface")
                    .join("hub")
            )
        );
        assert_eq!(hf_cache_root_from(None, None, None), None);
    }

    #[test]
    fn auto_detect_models_scans_hf_cache_snapshots_and_pairs_mmproj() {
        let dir = test_tmp::TestDir::new("detect-hf-cache");
        let hf_hub = dir.path().join("hf").join("hub");

        // Vision model snapshot: plain name + mmproj in the same snapshot -> classified as vision.
        let vision_snap = hf_hub
            .join("models--Qwen--Qwen3-VL-8B-Instruct-GGUF")
            .join("snapshots")
            .join("f982a07559d4a2f6c8744d840bf6fccab30eea96");
        fs::create_dir_all(&vision_snap).unwrap();
        let vision_gguf = vision_snap.join("custom-multimodal-q4.gguf");
        let mmproj_gguf = vision_snap.join("mmproj-Qwen3VL-8B-Instruct-F16.gguf");
        fs::write(&vision_gguf, b"GGUF").unwrap();
        fs::write(&mmproj_gguf, b"GGUF").unwrap();

        // Blobs and refs inside the same repo must be skipped.
        let blobs_dir = hf_hub
            .join("models--Qwen--Qwen3-VL-8B-Instruct-GGUF")
            .join("blobs");
        let refs_dir = hf_hub
            .join("models--Qwen--Qwen3-VL-8B-Instruct-GGUF")
            .join("refs");
        fs::create_dir_all(&blobs_dir).unwrap();
        fs::create_dir_all(&refs_dir).unwrap();
        fs::write(blobs_dir.join("ignored-blob.gguf"), b"GGUF").unwrap();
        fs::write(refs_dir.join("main.gguf"), b"GGUF").unwrap();

        // Empty snapshot must be ignored.
        let empty_snap = hf_hub
            .join("models--Org--Empty-Repo")
            .join("snapshots")
            .join("0000000000000000000000000000000000000000");
        fs::create_dir_all(&empty_snap).unwrap();

        // Regular LLM snapshot without mmproj -> classified as LLM.
        let llm_snap = hf_hub
            .join("models--Org--Gemma-GGUF")
            .join("snapshots")
            .join("1111111111111111111111111111111111111111");
        fs::create_dir_all(&llm_snap).unwrap();
        let llm_gguf = llm_snap.join("gemma-4-12b-q4.gguf");
        fs::write(&llm_gguf, b"GGUF").unwrap();

        let proc_settings = processing::ProcessingSettings::default();
        let detected = auto_detect_models_in(&proc_settings, dir.path(), &[], Some(&hf_hub));

        assert_eq!(detected.llm_models.len(), 1);
        assert_eq!(PathBuf::from(&detected.llm_models[0]), llm_gguf);
        assert_eq!(detected.vision_models.len(), 1);
        assert_eq!(PathBuf::from(&detected.vision_models[0]), vision_gguf);
        assert_eq!(detected.vision_pairs.len(), 1);
        assert_eq!(
            PathBuf::from(&detected.vision_pairs[0].model_path),
            vision_gguf
        );
        assert_eq!(
            detected.vision_pairs[0]
                .mmproj_path
                .as_deref()
                .map(PathBuf::from),
            Some(mmproj_gguf)
        );
    }

    #[test]
    fn hf_cache_root_trims_env_values() {
        use std::ffi::OsString;
        assert_eq!(
            hf_cache_root_from(Some(OsString::from("  D:/hf-hub 	")), None, None),
            Some(PathBuf::from("D:/hf-hub"))
        );
        assert_eq!(
            hf_cache_root_from(None, Some(OsString::from(" D:/hf-home ")), None),
            Some(PathBuf::from("D:/hf-home").join("hub"))
        );
    }

    #[test]
    fn vision_name_matching_covers_qwen_vl_variants() {
        for n in [
            "qwen3vl-8b-instruct-q4_k_m.gguf",
            "qwen3-vl-8b-instruct-q4_k_m.gguf",
            "qwen2.5vl-7b-q4.gguf",
            "llava-1.5-7b.gguf",
        ] {
            assert!(is_vision_model_name(n), "{n}");
        }
        for n in ["gemma-4-12b-q4.gguf", "devlog-7b.gguf", "level-up-3b.gguf"] {
            assert!(!is_vision_model_name(n), "{n}");
        }
    }

    #[test]
    fn auto_detect_dedupes_models_dir_and_hf_cache() {
        let dir = test_tmp::TestDir::new("detect-dedupe");
        let hf_hub = dir.path().join("hub");
        let snap = hf_hub
            .join("models--Qwen--Qwen3-VL-8B-Instruct-GGUF")
            .join("snapshots")
            .join("abc");
        fs::create_dir_all(&snap).unwrap();
        let vision = snap.join("Qwen3VL-8B-Instruct-Q4_K_M.gguf");
        fs::write(&vision, b"GGUF").unwrap();
        fs::write(snap.join("mmproj-F16.gguf"), b"GGUF").unwrap();

        // The same snapshot is also reachable as a plain models folder.
        let proc_settings = processing::ProcessingSettings::default();
        let detected = auto_detect_models_in(
            &proc_settings,
            dir.path(),
            std::slice::from_ref(&hf_hub),
            Some(&hf_hub),
        );
        assert!(detected.llm_models.is_empty(), "{:?}", detected.llm_models);
        assert_eq!(detected.vision_models.len(), 1);
        assert_eq!(detected.vision_pairs.len(), 1);
        assert!(detected.vision_pairs[0].mmproj_path.is_some());
    }

    fn try_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        let r = if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        };
        #[cfg(unix)]
        let r = std::os::unix::fs::symlink(target, link);
        r.is_ok()
    }

    #[test]
    fn scan_hf_cache_follows_inside_symlinks_and_rejects_escaping_ones() {
        let dir = test_tmp::TestDir::new("detect-hf-symlink");
        let hf_hub = dir.path().join("hub");
        let repo = hf_hub.join("models--Org--Gemma-GGUF");
        let blobs = repo.join("blobs");
        let snap = repo.join("snapshots").join("rev1");
        fs::create_dir_all(&blobs).unwrap();
        fs::create_dir_all(&snap).unwrap();
        let blob = blobs.join("deadbeef");
        fs::write(&blob, b"GGUF").unwrap();
        let outside = dir.path().join("outside.gguf");
        fs::write(&outside, b"GGUF").unwrap();

        let good = snap.join("gemma-good-q4.gguf");
        if !try_symlink(&blob, &good) {
            eprintln!("skipping: symlink creation is not permitted");
            return;
        }
        assert!(try_symlink(&outside, &snap.join("gemma-escape-q4.gguf")));

        // Symlinked snapshot directory pointing outside the cache.
        let outside_snap = dir.path().join("outside-snap");
        fs::create_dir_all(&outside_snap).unwrap();
        fs::write(outside_snap.join("evil-q4.gguf"), b"GGUF").unwrap();
        assert!(try_symlink(
            &outside_snap,
            &repo.join("snapshots").join("rev2")
        ));

        // Symlinked repo directory.
        let outside_repo = dir.path().join("outside-repo");
        fs::create_dir_all(outside_repo.join("snapshots").join("r")).unwrap();
        fs::write(
            outside_repo
                .join("snapshots")
                .join("r")
                .join("evil2-q4.gguf"),
            b"GGUF",
        )
        .unwrap();
        assert!(try_symlink(
            &outside_repo,
            &hf_hub.join("models--Org--Linked")
        ));

        let mut llm = BTreeSet::new();
        let mut vision = BTreeSet::new();
        let mut pairs = BTreeMap::new();
        scan_hf_cache(&hf_hub, &mut llm, &mut vision, &mut pairs);

        assert_eq!(llm.len(), 1, "{llm:?}");
        assert_eq!(PathBuf::from(llm.iter().next().unwrap()), good);
        assert!(vision.is_empty());
    }
}
