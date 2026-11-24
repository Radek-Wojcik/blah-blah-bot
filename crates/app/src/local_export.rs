//! Local Markdown export of meeting reports, transcripts and key screenshots.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use queue::draft::DraftFiles;
use queue::transcript::TranscriptFiles;
use store::Store;

use crate::draft_commands;
use crate::ipc::{IpcError, IpcResult};
use crate::processing_commands::meeting_dir;
use crate::settings::{CycleRepoSetting, Settings};
use crate::state::AppState;

/// Default local save directory under the user's documents or data directory.
pub fn default_local_save_dir(data_dir: &Path) -> PathBuf {
    if let Some(parent) = data_dir.parent() {
        parent.join("notes")
    } else {
        data_dir.join("notes")
    }
}

/// Format a Unix timestamp in milliseconds as `YYYY-MM-DD` (UTC).
pub fn format_iso_date(unix_ms: i64) -> String {
    let secs = (unix_ms.max(0) / 1000) as u64;
    let id = crate::recording::meeting_id(secs * 1000);
    format!("{}-{}-{}", &id[0..4], &id[4..6], &id[6..8])
}

fn format_timestamp(ms: u64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins:02}:{secs:02}")
}

/// Render a meeting's transcript as standalone Markdown.
pub fn render_transcript_markdown(
    store: &Store,
    meeting_id: &str,
    meeting_dir: &Path,
    title: &str,
    date: &str,
) -> String {
    let mut out = format!("# Transcript — {title} ({date})\n\n");
    let files = TranscriptFiles::new(meeting_dir);
    let Ok(mut view) = files.view() else {
        out.push_str("_Transcript is not available for this meeting yet._\n");
        return out;
    };
    if let (Ok(voices), Ok(participants)) = (
        store.list_meeting_voices(meeting_id),
        queue::voices::participants_for_meeting(store, meeting_id),
    ) {
        let names = queue::voices::speaker_names(&voices, &participants);
        queue::voices::apply_names(&mut view, &voices, &names);
    }
    let speaker_labels: std::collections::HashMap<String, String> = view
        .speakers
        .iter()
        .map(|s| (s.id.clone(), s.label.clone()))
        .collect();

    let active_segments: Vec<_> = view.segments.iter().filter(|s| !s.excluded).collect();
    if active_segments.is_empty() {
        out.push_str("_No transcript segments._\n");
        return out;
    }
    for seg in active_segments {
        let speaker = speaker_labels
            .get(&seg.speaker)
            .map(String::as_str)
            .unwrap_or(&seg.speaker);
        let ts = format_timestamp(seg.start_ms);
        out.push_str(&format!("- **[{ts}] {speaker}:** {}\n", seg.text.trim()));
    }
    out
}

/// Read `key_screenshots` from `<meeting_dir>/work/draft.json` if present.
fn read_key_screenshots(meeting_dir: &Path) -> Vec<String> {
    let Ok(bytes) = fs::read(meeting_dir.join("work").join("draft.json")) else {
        return Vec::new();
    };
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    val.get("key_screenshots")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().map(str::trim))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn is_allowed_image_ext(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp"
        )
    })
}

fn safe_rel_image_path(rel: &str) -> Option<&str> {
    let trimmed = rel.trim();
    if trimmed.is_empty()
        || trimmed.contains('\0')
        || trimmed.contains(':')
        || trimmed.starts_with('/')
        || trimmed.starts_with('\\')
        || trimmed.split(['/', '\\']).any(|seg| seg == "..")
    {
        return None;
    }
    let path = Path::new(trimmed);
    if path.is_absolute() || !is_allowed_image_ext(path) {
        return None;
    }
    let mut has_normal = false;
    for comp in path.components() {
        match comp {
            Component::Normal(_) => has_normal = true,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
            Component::CurDir => {}
        }
    }
    has_normal.then_some(trimmed)
}

fn validate_safe_path_segment(label: &str, value: &str) -> IpcResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('.')
        || trimmed.ends_with('.')
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains(':')
        || trimmed.contains("..")
        || trimmed
            .chars()
            .any(|c| c.is_control() || matches!(c, '\0' | '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(IpcError::new(
            "invalid",
            format!("unsafe {label} path segment"),
        ));
    }
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    let is_reserved = matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if is_reserved {
        return Err(IpcError::new(
            "invalid",
            format!("reserved {label} path segment"),
        ));
    }
    let mut comps = Path::new(trimmed).components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(_)), None) => Ok(trimmed.to_string()),
        _ => Err(IpcError::new(
            "invalid",
            format!("unsafe {label} path segment"),
        )),
    }
}

/// Copy `key_screenshots` (or all meeting screenshots when `key_screenshots` is empty)
/// into `<target_dir>/img/`.
fn copy_screenshots_to_export(
    store: &Store,
    meeting_id: &str,
    meeting_dir: &Path,
    target_dir: &Path,
) -> Vec<String> {
    let Ok(canon_meeting_dir) = fs::canonicalize(meeting_dir) else {
        return Vec::new();
    };
    let mut candidates = read_key_screenshots(meeting_dir);
    if candidates.is_empty() {
        if let Ok(Some(details)) = store.get_meeting(meeting_id) {
            candidates = details.screenshots.into_iter().map(|s| s.file).collect();
        }
    }
    if candidates.is_empty() {
        if let Ok(entries) = fs::read_dir(meeting_dir.join("img")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if is_allowed_image_ext(&path) {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        candidates.push(format!("img/{name}"));
                    }
                }
            }
        }
    }

    let mut copied = Vec::new();
    let mut seen = HashSet::new();
    let img_out_dir = target_dir.join("img");
    for rel in candidates {
        let Some(safe_rel) = safe_rel_image_path(&rel) else {
            continue;
        };
        let rel_path = Path::new(safe_rel);
        let candidate = if meeting_dir.join(rel_path).is_file() {
            meeting_dir.join(rel_path)
        } else if meeting_dir.join("img").join(rel_path).is_file() {
            meeting_dir.join("img").join(rel_path)
        } else {
            continue;
        };
        let Ok(canon_src) = fs::canonicalize(&candidate) else {
            continue;
        };
        if !canon_src.is_file()
            || !canon_src.starts_with(&canon_meeting_dir)
            || !is_allowed_image_ext(&canon_src)
        {
            continue;
        }
        let Some(file_name) = canon_src.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !seen.insert(file_name.to_string()) {
            continue;
        }
        if fs::create_dir_all(&img_out_dir).is_ok()
            && fs::copy(&canon_src, img_out_dir.join(file_name)).is_ok()
        {
            copied.push(file_name.to_string());
        }
    }
    copied
}

/// Render the report Markdown for a meeting (from `work/draft.json` when available, or a
/// fallback summary header when `reduce` has not run yet).
pub fn render_report_markdown(
    repos: &[CycleRepoSetting],
    store: &Store,
    meeting_id: &str,
    meeting_dir: &Path,
) -> IpcResult<(String, String, String)> {
    let details = store
        .get_meeting(meeting_id)?
        .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?;
    let report_settings = store.get_report_settings(meeting_id)?.unwrap_or_default();
    let fallback_cycle = report_settings
        .cycle_id
        .unwrap_or_else(|| "general".to_string());
    let fallback_date = format_iso_date(details.meeting.started_unix_ms);

    let files = DraftFiles::new(meeting_dir);
    if files.exists() {
        if let Ok(view) = draft_commands::load_view(repos, meeting_dir) {
            let cycle = if view.draft.cycle.trim().is_empty() {
                fallback_cycle
            } else {
                view.draft.cycle
            };
            let date = if view.draft.date.trim().is_empty() {
                fallback_date
            } else {
                view.draft.date
            };
            return Ok((cycle, date, view.report_markdown));
        }
        if let Ok(bytes) = fs::read(meeting_dir.join("work").join("draft.json")) {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                let cycle = val
                    .get("cycle")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(&fallback_cycle)
                    .to_string();
                let date = val
                    .get("date")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(&fallback_date)
                    .to_string();
                let title = val
                    .get("short_title")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(&details.meeting.title);
                let summary = val
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let mut md = format!("# {title}\n\n- **Date:** {date}\n- **Project:** {cycle}\n\n");
                if !summary.trim().is_empty() {
                    md.push_str(&format!("## Summary\n\n{}\n", summary.trim()));
                }
                return Ok((cycle, date, md));
            }
        }
    }

    let md = format!(
        "# {}\n\n- **Meeting ID:** {}\n- **Date:** {}\n- **Project:** {}\n",
        details.meeting.title, details.meeting.id, fallback_date, fallback_cycle
    );
    Ok((fallback_cycle, fallback_date, md))
}

/// Export `<base_dir>/<cycle>/<date>/report.md`, `transcript.md`, and `img/*`.
/// Returns the path to `report.md`.
pub fn export_to_local_folder(
    repos: &[CycleRepoSetting],
    store: &Store,
    meeting_id: &str,
    meeting_dir: &Path,
    base_dir: &Path,
) -> IpcResult<PathBuf> {
    let details = store
        .get_meeting(meeting_id)?
        .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?;
    let (cycle, date, mut report_md) =
        render_report_markdown(repos, store, meeting_id, meeting_dir)?;
    let cycle = validate_safe_path_segment("cycle", &cycle)?;
    let date = validate_safe_path_segment("date", &date)?;
    let target_dir = base_dir.join(&cycle).join(&date);
    fs::create_dir_all(&target_dir)?;

    let copied_imgs = copy_screenshots_to_export(store, meeting_id, meeting_dir, &target_dir);
    if !copied_imgs.is_empty() && !report_md.contains("](img/") {
        report_md.push_str("\n## Key Screenshots\n\n");
        for img in &copied_imgs {
            report_md.push_str(&format!("![{img}](img/{img})\n"));
        }
    }

    let report_path = target_dir.join("report.md");
    fs::write(&report_path, &report_md)?;

    let transcript_md = render_transcript_markdown(
        store,
        meeting_id,
        meeting_dir,
        &details.meeting.title,
        &date,
    );
    let transcript_path = target_dir.join("transcript.md");
    fs::write(&transcript_path, transcript_md)?;

    Ok(report_path)
}

/// Export a meeting's Markdown to `dest_path` (either a specific `.md` file path or a base
/// directory; when `None`, uses the effective local save directory).
pub fn export_meeting_markdown_with(
    repos: &[CycleRepoSetting],
    store: &Store,
    settings: &Settings,
    data_dir: &Path,
    meeting_id: &str,
    meeting_dir: &Path,
    dest_path: Option<&str>,
) -> IpcResult<String> {
    let cycle_id = store
        .get_report_settings(meeting_id)?
        .and_then(|r| r.cycle_id);
    let prefs = cycle_id
        .as_deref()
        .and_then(|cid| store.get_project_prefs(cid).ok());

    let effective_base_dir = || -> PathBuf {
        if let Some(p) = &prefs {
            if !p.local_save_dir.trim().is_empty() {
                return PathBuf::from(p.local_save_dir.trim());
            }
        }
        if !settings.local_save_dir.trim().is_empty() {
            return PathBuf::from(settings.local_save_dir.trim());
        }
        default_local_save_dir(data_dir)
    };

    if let Some(dest) = dest_path.map(str::trim).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(dest);
        let is_md_file = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if is_md_file {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let (_, _, report_md) = render_report_markdown(repos, store, meeting_id, meeting_dir)?;
            fs::write(&path, report_md)?;
            // Also trigger automatic folder export if local save is enabled.
            let auto_enabled =
                prefs.as_ref().is_some_and(|p| p.local_save_enabled) || settings.local_save_enabled;
            if auto_enabled {
                let _ = export_to_local_folder(
                    repos,
                    store,
                    meeting_id,
                    meeting_dir,
                    &effective_base_dir(),
                );
            }
            return Ok(path.to_string_lossy().into_owned());
        }
        let report_path = export_to_local_folder(repos, store, meeting_id, meeting_dir, &path)?;
        return Ok(report_path.to_string_lossy().into_owned());
    }

    let base_dir = effective_base_dir();
    let report_path = export_to_local_folder(repos, store, meeting_id, meeting_dir, &base_dir)?;
    Ok(report_path.to_string_lossy().into_owned())
}

/// Automatically export `<dir>/<cycle>/<date>/report.md`, `transcript.md` and `img/*` when
/// project `local_save_enabled` OR global `settings.local_save_enabled` is true.
pub fn auto_export_if_enabled_with(
    repos: &[CycleRepoSetting],
    store: &Store,
    settings: &Settings,
    data_dir: &Path,
    meeting_id: &str,
    meeting_dir: &Path,
) -> IpcResult<Option<PathBuf>> {
    let cycle_id = store
        .get_report_settings(meeting_id)?
        .and_then(|r| r.cycle_id);
    let prefs = cycle_id
        .as_deref()
        .and_then(|cid| store.get_project_prefs(cid).ok());

    let project_enabled = prefs.as_ref().is_some_and(|p| p.local_save_enabled);
    let global_enabled = settings.local_save_enabled;
    if !project_enabled && !global_enabled {
        return Ok(None);
    }

    let base_dir = if project_enabled
        && prefs
            .as_ref()
            .is_some_and(|p| !p.local_save_dir.trim().is_empty())
    {
        PathBuf::from(prefs.unwrap().local_save_dir.trim())
    } else if !settings.local_save_dir.trim().is_empty() {
        PathBuf::from(settings.local_save_dir.trim())
    } else if let Some(p) = prefs.filter(|p| !p.local_save_dir.trim().is_empty()) {
        PathBuf::from(p.local_save_dir.trim())
    } else {
        default_local_save_dir(data_dir)
    };

    let report_path = export_to_local_folder(repos, store, meeting_id, meeting_dir, &base_dir)?;
    Ok(Some(report_path))
}

/// Tauri state wrapper for [`auto_export_if_enabled_with`].
pub fn auto_export_if_enabled(state: &AppState, meeting_id: &str) -> IpcResult<Option<PathBuf>> {
    let dir = meeting_dir(state, meeting_id)?;
    let repos = state.cycle_repos();
    let settings = state.settings()?;
    let data_dir = state.data_dir().to_path_buf();
    state.with_store_ipc(|s| {
        auto_export_if_enabled_with(&repos, s, &settings, &data_dir, meeting_id, &dir)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::{Meeting, MeetingState, ReportSettings};

    fn seed_meeting(store: &Store, id: &str, cycle_id: Option<&str>) {
        store
            .create_meeting(&Meeting {
                id: id.into(),
                title: "Test Meeting".into(),
                started_unix_ms: 1_795_500_000_000,
                duration_ms: Some(10_000),
                state: MeetingState::Stopped,
                dir: id.into(),
            })
            .unwrap();
        if let Some(cid) = cycle_id {
            store
                .set_report_settings(
                    id,
                    &ReportSettings {
                        cycle_id: Some(cid.into()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }

    #[test]
    fn copy_screenshots_rejects_traversal_absolute_and_non_image_paths() {
        let dir = test_tmp::TestDir::new("export-traversal");
        let store = Store::open_in_memory().unwrap();
        seed_meeting(&store, "m1", Some("ALPHA"));

        fs::write(dir.path().join("secret.txt"), b"top-secret").unwrap();
        fs::write(dir.path().join("outside.png"), b"outside-png").unwrap();

        let meeting_dir = dir.path().join("rec").join("m1");
        fs::create_dir_all(meeting_dir.join("work")).unwrap();
        fs::create_dir_all(meeting_dir.join("img")).unwrap();
        fs::write(meeting_dir.join("img").join("valid.png"), b"ok-png").unwrap();
        fs::write(meeting_dir.join("img").join("notes.txt"), b"not-an-image").unwrap();
        fs::write(
            meeting_dir.join("work").join("draft.json"),
            serde_json::json!({
                "cycle": "ALPHA",
                "date": "2026-11-24",
                "summary": "Summary",
                "key_screenshots": [
                    "../secret.txt",
                    "../../secret.txt",
                    "../../outside.png",
                    "C:\\Windows\\win.ini",
                    "/etc/passwd",
                    "",
                    "img/notes.txt",
                    "img/valid.png"
                ]
            })
            .to_string(),
        )
        .unwrap();

        let base_dir = dir.path().join("exports");
        let report_path =
            export_to_local_folder(&[], &store, "m1", &meeting_dir, &base_dir).unwrap();
        assert!(report_path.is_file());

        let img_dir = base_dir.join("ALPHA").join("2026-11-24").join("img");
        assert!(img_dir.join("valid.png").is_file());
        assert!(!img_dir.join("secret.txt").exists());
        assert!(!img_dir.join("outside.png").exists());
        assert!(!img_dir.join("win.ini").exists());
        assert!(!img_dir.join("passwd").exists());
        assert!(!img_dir.join("notes.txt").exists());
    }

    #[test]
    fn export_to_local_folder_rejects_unsafe_cycle_and_date_segments() {
        let dir = test_tmp::TestDir::new("export-segments");
        let store = Store::open_in_memory().unwrap();
        seed_meeting(&store, "m1", Some("ALPHA"));
        let meeting_dir = dir.path().join("rec").join("m1");
        fs::create_dir_all(meeting_dir.join("work")).unwrap();
        let base_dir = dir.path().join("exports");

        let bad_cases = [
            ("../evil", "2026-11-24"),
            ("a/b", "2026-11-24"),
            ("a\\b", "2026-11-24"),
            ("C:foo", "2026-11-24"),
            (".hidden", "2026-11-24"),
            ("CON", "2026-11-24"),
            ("nul.txt", "2026-11-24"),
            ("ALPHA", "../escape"),
            ("ALPHA", "2026/11/24"),
            ("ALPHA", "2026\\11\\24"),
            ("ALPHA", ".."),
            ("ALPHA", ".dot"),
            ("ALPHA", "LPT1"),
        ];

        for (cycle, date) in bad_cases {
            fs::write(
                meeting_dir.join("work").join("draft.json"),
                serde_json::json!({
                    "cycle": cycle,
                    "date": date,
                    "summary": "Summary"
                })
                .to_string(),
            )
            .unwrap();
            let err =
                export_to_local_folder(&[], &store, "m1", &meeting_dir, &base_dir).unwrap_err();
            assert_eq!(err.code, "invalid", "expected invalid for {cycle}/{date}");
        }
    }
}
