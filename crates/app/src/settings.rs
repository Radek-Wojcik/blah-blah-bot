//! User settings, stored as JSON in the application config folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Folder for recordings. Empty means the default under the user's documents folder.
    pub recordings_dir: String,
    pub record_hotkey: String,
    pub screenshot_hotkey: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            recordings_dir: String::new(),
            record_hotkey: "Ctrl+Alt+R".into(),
            screenshot_hotkey: "Ctrl+Alt+S".into(),
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
}
