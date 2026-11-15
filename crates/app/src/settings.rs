//! User settings, stored as JSON in the application config folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How a cycle repository came to be registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoKind {
    /// A folder on this machine (created by the application or chosen by the user).
    Local,
    /// A clone of a remote repository made by the application.
    Clone,
}

/// A registered cycle repository. Holds no credentials: a remote that needs them relies on
/// the Git credential helper of the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleRepoSetting {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub remote: Option<String>,
    pub kind: RepoKind,
}

/// The report choice of the last recording that started, offered again next time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastReport {
    pub cycle_id: String,
    pub mode: String,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub meeting_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Folder for recordings. Empty means the default under the user's documents folder.
    pub recordings_dir: String,
    pub record_hotkey: String,
    pub screenshot_hotkey: String,
    /// Registered cycle repositories. Changed only through the cycle commands: a settings
    /// save from the UI keeps the stored list.
    pub cycle_repos: Vec<CycleRepoSetting>,
    /// What the last recording used; filled by the application when a recording starts.
    pub last_report: Option<LastReport>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            recordings_dir: String::new(),
            record_hotkey: "Ctrl+Alt+R".into(),
            screenshot_hotkey: "Ctrl+Alt+S".into(),
            cycle_repos: Vec::new(),
            last_report: None,
        }
    }
}

impl Settings {
    /// Load settings; a missing or unreadable file yields the defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Write to a temporary file first so a crash cannot leave a half-written settings file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    /// These settings with the values only the application changes (the repository list and
    /// the last report) taken from `current`, so a settings screen that does not know them
    /// cannot wipe them.
    pub fn keeping_managed_from(mut self, current: &Settings) -> Self {
        self.cycle_repos = current.cycle_repos.clone();
        self.last_report = current.last_report.clone();
        self
    }

    /// Effective recordings folder.
    pub fn recordings_root(&self, default: PathBuf) -> PathBuf {
        if self.recordings_dir.trim().is_empty() {
            default
        } else {
            PathBuf::from(&self.recordings_dir)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bbb-settings-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn missing_file_gives_defaults() {
        assert_eq!(Settings::load(&temp("missing")), Settings::default());
    }

    #[test]
    fn round_trip() {
        let path = temp("round");
        let s = Settings {
            recordings_dir: "D:\\Rec".into(),
            record_hotkey: "Ctrl+F9".into(),
            screenshot_hotkey: "Ctrl+F10".into(),
            cycle_repos: vec![CycleRepoSetting {
                id: "local".into(),
                path: "D:/Cycles".into(),
                remote: None,
                kind: RepoKind::Local,
            }],
            last_report: Some(LastReport {
                cycle_id: "NOMAD".into(),
                mode: "by-type".into(),
                template: None,
                meeting_type: Some("budget".into()),
            }),
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let path = temp("partial");
        std::fs::write(&path, r#"{"recordHotkey":"Ctrl+F1"}"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.record_hotkey, "Ctrl+F1");
        assert_eq!(s.screenshot_hotkey, Settings::default().screenshot_hotkey);
    }

    #[test]
    fn corrupt_file_gives_defaults() {
        let path = temp("corrupt");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn empty_dir_uses_default_root() {
        let s = Settings::default();
        assert_eq!(s.recordings_root(PathBuf::from("X")), PathBuf::from("X"));
    }

    #[test]
    fn a_file_from_before_cycles_still_loads() {
        let path = temp("old");
        std::fs::write(
            &path,
            r#"{"recordingsDir":"D:/Rec","recordHotkey":"Ctrl+F1","screenshotHotkey":"Ctrl+F2"}"#,
        )
        .unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.recordings_dir, "D:/Rec");
        assert_eq!(s.record_hotkey, "Ctrl+F1");
        assert!(s.cycle_repos.is_empty());
        assert_eq!(s.last_report, None);
    }

    #[test]
    fn repo_and_last_report_use_the_camel_case_contract() {
        let s: Settings = serde_json::from_str(
            r#"{"cycleRepos":[{"id":"r1","path":"C:/r","remote":"https://example.com/r.git","kind":"clone"}],
                "lastReport":{"cycleId":"A","mode":"template","template":"weekly"}}"#,
        )
        .unwrap();
        assert_eq!(s.cycle_repos[0].kind, RepoKind::Clone);
        assert_eq!(
            s.cycle_repos[0].remote.as_deref(),
            Some("https://example.com/r.git")
        );
        let last = s.last_report.unwrap();
        assert_eq!((last.cycle_id.as_str(), last.meeting_type), ("A", None));
    }

    #[test]
    fn a_settings_save_keeps_the_repositories_and_the_last_report() {
        let mut current = Settings::default();
        current.cycle_repos.push(CycleRepoSetting {
            id: "local".into(),
            path: "X".into(),
            remote: None,
            kind: RepoKind::Local,
        });
        current.last_report = Some(LastReport {
            cycle_id: "A".into(),
            mode: "fixed".into(),
            template: None,
            meeting_type: None,
        });
        let from_ui = Settings {
            record_hotkey: "Ctrl+F5".into(),
            ..Settings::default()
        };
        let merged = from_ui.keeping_managed_from(&current);
        assert_eq!(merged.record_hotkey, "Ctrl+F5");
        assert_eq!(merged.cycle_repos, current.cycle_repos);
        assert_eq!(merged.last_report, current.last_report);
    }
}
