//! Shared application state managed by Tauri.

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Manager};

use crate::ipc::{IpcError, IpcResult};
use crate::settings::Settings;

pub struct AppState {
    store: Mutex<store::Store>,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    default_recordings: PathBuf,
}

impl AppState {
    pub fn open(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = app.path().app_data_dir()?;
        std::fs::create_dir_all(&data_dir)?;
        let config_dir = app.path().app_config_dir()?;
        let settings_path = config_dir.join("settings.json");
        let default_recordings = app
            .path()
            .document_dir()?
            .join("BlahBlahBot")
            .join("recordings");
        let store = store::Store::open(&data_dir.join("blah-blah-bot.db"))?;
        Ok(Self {
            store: Mutex::new(store),
            settings: Mutex::new(Settings::load(&settings_path)),
            settings_path,
            default_recordings,
        })
    }

    /// Run `f` with the store locked.
    pub fn with_store<T>(&self, f: impl FnOnce(&store::Store) -> store::Result<T>) -> IpcResult<T> {
        let guard = self
            .store
            .lock()
            .map_err(|_| IpcError::new("internal", "store lock poisoned"))?;
        Ok(f(&guard)?)
    }

    pub fn mark_interrupted(&self) -> IpcResult<Vec<String>> {
        self.with_store(|s| s.mark_interrupted_recoverable())
    }

    pub fn settings(&self) -> IpcResult<Settings> {
        self.settings
            .lock()
            .map(|s| s.clone())
            .map_err(|_| IpcError::new("internal", "settings lock poisoned"))
    }

    pub fn save_settings(&self, new: Settings) -> IpcResult<()> {
        new.save(&self.settings_path)?;
        *self
            .settings
            .lock()
            .map_err(|_| IpcError::new("internal", "settings lock poisoned"))? = new;
        Ok(())
    }

    /// Folder recordings are written to.
    #[allow(dead_code)]
    pub fn recordings_root(&self) -> IpcResult<PathBuf> {
        Ok(self
            .settings()?
            .recordings_root(self.default_recordings.clone()))
    }
}
