//! User settings, stored as JSON in the application config folder.

use std::path::{Path, PathBuf};

use recorder::detect::Sensitivity;
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

/// The local bridge to the Chrome extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BridgeSettings {
    /// Listen for the extension at all.
    pub enabled: bool,
    /// Port tried first on `127.0.0.1`.
    pub port: u16,
    /// Ports tried in order when `port` is taken.
    pub port_fallbacks: Vec<u16>,
    /// Extension ids accepted as `Origin`; by default the id of the extension of this
    /// release (it follows from the key in its manifest). Empty accepts any extension
    /// (pairing still decides who may stream).
    pub allowed_extension_ids: Vec<String>,
    /// Paired extensions. Changed only through pairing and unpairing: a settings save from
    /// the UI keeps the stored list.
    pub clients: Vec<bridge::pairing::PairedClient>,
    /// Record a tab the extension starts while a recording runs, without asking.
    pub auto_add_tabs: bool,
}

impl Default for BridgeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: bridge::server::DEFAULT_PORT,
            port_fallbacks: bridge::server::FALLBACK_PORTS.collect(),
            allowed_extension_ids: vec![bridge::server::EXTENSION_ID.to_string()],
            clients: Vec::new(),
            auto_add_tabs: true,
        }
    }
}

impl BridgeSettings {
    /// Ports in the order they are tried.
    pub fn ports(&self) -> Vec<u16> {
        let mut out = vec![self.port];
        out.extend(
            self.port_fallbacks
                .iter()
                .copied()
                .filter(|p| *p != self.port),
        );
        out
    }

    /// The settings as the UI may see them: no secret hashes.
    pub fn redacted(mut self) -> Self {
        for c in &mut self.clients {
            c.secret_sha256.clear();
        }
        self
    }

    /// Check values entered in the settings screen.
    pub fn validate(&self) -> Result<(), String> {
        if self.port < 1024 || self.port_fallbacks.iter().any(|p| *p < 1024) {
            return Err("Bridge ports must be between 1024 and 65535".into());
        }
        if self.port_fallbacks.len() > 32 {
            return Err("At most 32 fallback ports".into());
        }
        let id_ok = |id: &String| id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b));
        if let Some(bad) = self.allowed_extension_ids.iter().find(|id| !id_ok(id)) {
            return Err(format!(
                "`{bad}` is not a Chrome extension id (32 letters a to p)"
            ));
        }
        Ok(())
    }
}

/// Meeting detection: which applications raise a "start recording?" prompt and how readily.
/// The numbers behind each sensitivity live in `recorder::detect::Presets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DetectionSettings {
    /// Watch for calls at all.
    pub enabled: bool,
    /// `low`, `balanced` or `high`.
    pub sensitivity: Sensitivity,
    /// Conferencing applications whose use of the microphone counts as a call (executable
    /// names without `.exe`, lowercase). Browsers are never detected this way.
    pub known_apps: Vec<String>,
    /// Applications the user chose "Never" for.
    pub opted_out: Vec<String>,
    /// Use the Google Meet reports of the browser extension.
    pub meet: bool,
}

impl Default for DetectionSettings {
    fn default() -> Self {
        let d = recorder::detect::DetectConfig::default();
        Self {
            enabled: d.enabled,
            sensitivity: d.sensitivity,
            known_apps: d.known_apps,
            opted_out: d.opted_out,
            meet: d.meet,
        }
    }
}

impl DetectionSettings {
    /// The detector's configuration.
    pub fn to_config(&self) -> recorder::detect::DetectConfig {
        recorder::detect::DetectConfig {
            enabled: self.enabled,
            sensitivity: self.sensitivity,
            known_apps: self.known_apps.clone(),
            opted_out: self.opted_out.clone(),
            meet: self.meet,
            ..recorder::detect::DetectConfig::default()
        }
    }

    /// Normalize the lists (stems, lowercase, no duplicates) and check them.
    pub fn normalized(mut self) -> Result<Self, String> {
        let clean = |list: &mut Vec<String>| {
            let mut out: Vec<String> = Vec::new();
            for a in list.iter() {
                let stem = recorder::detect::app_stem(a);
                if !stem.is_empty() && !out.contains(&stem) {
                    out.push(stem);
                }
            }
            *list = out;
        };
        clean(&mut self.known_apps);
        clean(&mut self.opted_out);
        if self.known_apps.len() > 64 || self.opted_out.len() > 64 {
            return Err("At most 64 applications in a list".into());
        }
        if let Some(bad) = self
            .known_apps
            .iter()
            .chain(&self.opted_out)
            .find(|a| a.len() > 64 || a.contains(['\\', '/', ':']))
        {
            return Err(format!("`{bad}` is not an executable name"));
        }
        Ok(self)
    }
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
    /// The bridge to the Chrome extension.
    pub bridge: BridgeSettings,
    /// Meeting detection. Changed through the detection commands: a settings save from the
    /// settings screen keeps the stored value.
    pub detection: DetectionSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            recordings_dir: String::new(),
            record_hotkey: "Ctrl+Alt+R".into(),
            screenshot_hotkey: "Ctrl+Alt+S".into(),
            cycle_repos: Vec::new(),
            last_report: None,
            bridge: BridgeSettings::default(),
            detection: DetectionSettings::default(),
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

    /// These settings with the values only the application changes (the repository list,
    /// the last report and the paired extensions) taken from `current`, so a settings screen
    /// that does not know them cannot wipe them.
    pub fn keeping_managed_from(mut self, current: &Settings) -> Self {
        self.cycle_repos = current.cycle_repos.clone();
        self.last_report = current.last_report.clone();
        self.bridge.clients = current.bridge.clients.clone();
        self.detection = current.detection.clone();
        self
    }

    /// The settings as the UI may see them: secret hashes removed.
    pub fn redacted(mut self) -> Self {
        self.bridge = self.bridge.redacted();
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

    /// A settings path in a fresh folder under `target/tmp`; the folder goes with the guard.
    fn temp(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = testkit::target_tempdir(&format!("app-settings-{name}"));
        let path = dir.path().join("settings.json");
        (dir, path)
    }

    #[test]
    fn missing_file_gives_defaults() {
        let (_dir, path) = temp("missing");
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn round_trip() {
        let (_dir, path) = temp("round");
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
            bridge: BridgeSettings {
                port: 18000,
                clients: vec![client("c-1")],
                ..BridgeSettings::default()
            },
            detection: DetectionSettings {
                sensitivity: Sensitivity::High,
                opted_out: vec!["zoom".into()],
                ..DetectionSettings::default()
            },
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let (_dir, path) = temp("partial");
        std::fs::write(&path, r#"{"recordHotkey":"Ctrl+F1"}"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.record_hotkey, "Ctrl+F1");
        assert_eq!(s.screenshot_hotkey, Settings::default().screenshot_hotkey);
    }

    #[test]
    fn corrupt_file_gives_defaults() {
        let (_dir, path) = temp("corrupt");
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
        let (_dir, path) = temp("old");
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

    fn client(id: &str) -> bridge::pairing::PairedClient {
        bridge::pairing::PairedClient {
            id: id.into(),
            name: "Chrome 136".into(),
            ext_version: "0.6.0".into(),
            secret_sha256: "ab".repeat(32),
            paired_unix_ms: 1,
        }
    }

    #[test]
    fn a_settings_save_keeps_the_paired_extensions_but_takes_the_bridge_options() {
        let mut current = Settings::default();
        current.bridge.clients.push(client("c-1"));
        let mut from_ui = Settings::default().redacted();
        from_ui.bridge.enabled = false;
        from_ui.bridge.auto_add_tabs = false;
        let merged = from_ui.keeping_managed_from(&current);
        assert_eq!(merged.bridge.clients, current.bridge.clients);
        assert!(!merged.bridge.enabled && !merged.bridge.auto_add_tabs);
    }

    #[test]
    fn the_ui_never_sees_secret_hashes() {
        let mut s = Settings::default();
        s.bridge.clients.push(client("c-1"));
        let shown = s.redacted();
        assert_eq!(shown.bridge.clients[0].id, "c-1");
        assert!(shown.bridge.clients[0].secret_sha256.is_empty());
    }

    #[test]
    fn bridge_defaults_and_validation() {
        let b = BridgeSettings::default();
        assert!(b.enabled && b.auto_add_tabs);
        assert_eq!(b.ports().first(), Some(&17_555));
        assert_eq!(b.ports().len(), 10);
        assert_eq!(b.allowed_extension_ids, [bridge::server::EXTENSION_ID]);
        assert!(b.validate().is_ok());
        let low = BridgeSettings {
            port: 80,
            ..BridgeSettings::default()
        };
        assert!(low.validate().is_err());
        let bad_id = BridgeSettings {
            allowed_extension_ids: vec!["not-an-id".into()],
            ..BridgeSettings::default()
        };
        assert!(bad_id.validate().is_err());
        let good_id = BridgeSettings {
            allowed_extension_ids: vec!["mddhpjgdobfbknicbomoojpbpkgfpima".into()],
            ..BridgeSettings::default()
        };
        assert!(good_id.validate().is_ok());
        // A file from before the bridge existed gets the defaults.
        let s: Settings = serde_json::from_str(r#"{"recordHotkey":"Ctrl+F1"}"#).unwrap();
        assert_eq!(s.bridge, BridgeSettings::default());
    }

    #[test]
    fn detection_defaults_to_balanced_with_the_known_apps() {
        let d = DetectionSettings::default();
        assert!(d.enabled && d.meet);
        assert_eq!(d.sensitivity, Sensitivity::Balanced);
        assert_eq!(d.known_apps, recorder::detect::KNOWN_APPS);
        assert!(d.opted_out.is_empty());
        // A file from before detection existed gets the defaults; the camelCase names hold.
        let s: Settings = serde_json::from_str(r#"{"recordHotkey":"Ctrl+F1"}"#).unwrap();
        assert_eq!(s.detection, d);
        let s: Settings = serde_json::from_str(
            r#"{"detection":{"sensitivity":"low","knownApps":["zoom"],"optedOut":["slack"]}}"#,
        )
        .unwrap();
        assert_eq!(s.detection.sensitivity, Sensitivity::Low);
        assert_eq!(s.detection.known_apps, ["zoom"]);
        assert_eq!(s.detection.opted_out, ["slack"]);
        assert!(s.detection.enabled);
    }

    #[test]
    fn detection_lists_are_normalized_and_checked() {
        let d = DetectionSettings {
            known_apps: vec![
                " Zoom.EXE ".into(),
                "zoom".into(),
                "".into(),
                "Slack".into(),
            ],
            ..DetectionSettings::default()
        };
        assert_eq!(d.normalized().unwrap().known_apps, ["zoom", "slack"]);
        let bad = DetectionSettings {
            known_apps: vec!["C:\\x\\app".into()],
            ..DetectionSettings::default()
        };
        assert!(bad.normalized().is_err());
    }

    #[test]
    fn a_settings_save_keeps_the_detection_settings() {
        let mut current = Settings::default();
        current.detection.opted_out.push("discord".into());
        let merged = Settings::default().keeping_managed_from(&current);
        assert_eq!(merged.detection, current.detection);
    }
}
