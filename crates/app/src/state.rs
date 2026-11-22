//! Shared application state managed by Tauri.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use recorder::screenshot::Frame;
use recorder::session::{CaptureBackend, ToneBackend, WasapiBackend};
use tauri::{AppHandle, Manager};

use crate::cycles::ParticipantSync;
use crate::ipc::{IpcError, IpcResult};
use crate::processing::Processing;
use crate::recording::Recorder;
use crate::settings::{CycleRepoSetting, Settings};

/// A desktop capture waiting for the user to choose a region, with the moment it was taken.
pub struct PendingShot {
    pub frame: Frame,
    pub offset_ms: f64,
}

pub struct AppState {
    store: Arc<Mutex<store::Store>>,
    settings: Arc<Mutex<Settings>>,
    settings_path: PathBuf,
    data_dir: PathBuf,
    default_recordings: PathBuf,
    pub recorder: Recorder,
    processing: Arc<Processing>,
    pub pending_shot: Mutex<Option<PendingShot>>,
}

/// Keeps the stored participants of a cycle in step with `cycle.yaml`, locking the database
/// for that call alone.
impl ParticipantSync for AppState {
    fn sync_participants(
        &self,
        cycle_id: &str,
        participants: &[(String, String)],
    ) -> IpcResult<store::SyncOutcome> {
        self.with_store(|s| s.sync_cycle_participants(cycle_id, participants))
    }
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
        let settings = Arc::new(Mutex::new(Settings::load(&settings_path)));
        let root_app = app.clone();
        let repos_settings = settings.clone();
        let processing = Processing::new(
            app.clone(),
            store.clone(),
            &data_dir,
            &config_dir,
            move || {
                root_app
                    .state::<AppState>()
                    .recordings_root()
                    .unwrap_or_default()
            },
            move || {
                repos_settings
                    .lock()
                    .map(|s| s.cycle_repos.clone())
                    .unwrap_or_default()
            },
        );
        Ok(Self {
            recorder: Recorder::new(store.clone(), backend, demo),
            processing,
            store,
            settings,
            settings_path,
            data_dir,
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

    /// Like [`AppState::with_store`] for code that has its own error type already.
    pub fn with_store_ipc<T>(&self, f: impl FnOnce(&store::Store) -> IpcResult<T>) -> IpcResult<T> {
        let guard = self
            .store
            .lock()
            .map_err(|_| IpcError::new("internal", "store lock poisoned"))?;
        f(&guard)
    }

    /// Folder of the database and of the repositories the application creates.
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// The registered cycle repositories.
    pub fn cycle_repos(&self) -> Vec<CycleRepoSetting> {
        self.settings
            .lock()
            .map(|s| s.cycle_repos.clone())
            .unwrap_or_default()
    }

    pub fn processing(&self) -> &Arc<Processing> {
        &self.processing
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

    /// Save settings from the settings screen. The repository list and the last report are
    /// kept as they are: only the cycle commands change them.
    pub fn save_settings(&self, new: Settings) -> IpcResult<()> {
        self.update_settings(|s| {
            new.check_new_locations(s)
                .map_err(|p| IpcError::new("invalid", p.message))?;
            *s = new.keeping_managed_from(s);
            Ok(())
        })
    }

    /// Change the settings under the lock and save them. Nothing is stored when `f` fails.
    pub fn update_settings<T>(
        &self,
        f: impl FnOnce(&mut Settings) -> IpcResult<T>,
    ) -> IpcResult<T> {
        let mut guard = self
            .settings
            .lock()
            .map_err(|_| IpcError::new("internal", "settings lock poisoned"))?;
        let mut next = guard.clone();
        let out = f(&mut next)?;
        next.save(&self.settings_path)?;
        *guard = next;
        Ok(out)
    }

    /// Folder recordings are written to.
    pub fn recordings_root(&self) -> IpcResult<PathBuf> {
        Ok(self
            .settings()?
            .recordings_root(self.default_recordings.clone()))
    }
}
