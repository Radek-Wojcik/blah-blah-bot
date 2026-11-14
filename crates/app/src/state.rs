//! Shared application state managed by Tauri.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use recorder::screenshot::Frame;
use recorder::session::{CaptureBackend, ToneBackend, WasapiBackend};
use tauri::{AppHandle, Manager};

use crate::ipc::{IpcError, IpcResult};
use crate::recording::Recorder;
use crate::settings::Settings;

/// A desktop capture waiting for the user to choose a region, with the moment it was taken.
pub struct PendingShot {
    pub frame: Frame,
    pub offset_ms: f64,
}

pub struct AppState {
    store: Arc<Mutex<store::Store>>,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    default_recordings: PathBuf,
    pub recorder: Recorder,
    pub pending_shot: Mutex<Option<PendingShot>>,
}

impl AppState {
    pub fn open(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        // `BBB_DATA_DIR` puts all application data in one folder: used by the end-to-end
        // tests so they never touch the real library.
        let (data_dir, config_dir, default_recordings) = match std::env::var_os("BBB_DATA_DIR") {
            Some(dir) => {
                let dir = PathBuf::from(dir);
                (dir.join("data"), dir.join("config"), dir.join("recordings"))
            }
            None => (
                app.path().app_data_dir()?,
                app.path().app_config_dir()?,
                app.path()
                    .document_dir()?
                    .join("BlahBlahBot")
                    .join("recordings"),
            ),
        };
        std::fs::create_dir_all(&data_dir)?;
        let settings_path = config_dir.join("settings.json");
        let store = Arc::new(Mutex::new(store::Store::open(
            &data_dir.join("blah-blah-bot.db"),
        )?));

        // `BBB_DEMO=1` swaps real audio capture for generated tones: used by the demo mode,
        // the UI screenshots script and tests of the application itself.
        let demo = std::env::var_os("BBB_DEMO").is_some();
        let backend: Arc<dyn CaptureBackend> = if demo {
            Arc::new(ToneBackend)
        } else {
            Arc::new(WasapiBackend)
        };
        Ok(Self {
            recorder: Recorder::new(store.clone(), backend, demo),
            store,
            settings: Mutex::new(Settings::load(&settings_path)),
            settings_path,
            default_recordings,
            pending_shot: Mutex::new(None),
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
    pub fn recordings_root(&self) -> IpcResult<PathBuf> {
        Ok(self
            .settings()?
            .recordings_root(self.default_recordings.clone()))
    }
}
