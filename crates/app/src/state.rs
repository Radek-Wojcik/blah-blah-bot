//! Shared application state managed by Tauri.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use recorder::screenshot::Frame;
use recorder::session::{CaptureBackend, RoutingBackend, ToneBackend, WasapiBackend};
use tauri::{AppHandle, Manager};

use crate::briefing::{BriefingSource, PublishingSource};
use crate::browser::{AppHost, Browser};
use crate::cycles::ParticipantSync;
use crate::detection::Detection;
use crate::ipc::{IpcError, IpcResult};
use crate::overlay::Preview;
use crate::processing::Processing;
use crate::publishing::{AppEnv, Publishing};
use crate::recording::Recorder;
use crate::settings::{CycleRepoSetting, Settings};

/// A desktop capture waiting for the user to choose a region, with the moment it was taken.
pub struct PendingShot {
    pub frame: Arc<Frame>,
    /// The PNG shown in the overlay, encoded while the overlay opens.
    pub preview: Arc<Preview>,
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
    publishing: Arc<Publishing>,
    pub pending_shot: Mutex<Option<PendingShot>>,
    browser: Arc<Browser>,
    browser_host: Arc<AppHost>,
    detection: Arc<Detection>,
    briefing: Arc<dyn BriefingSource>,
    demo: bool,
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
        let devices: Arc<dyn CaptureBackend> = if demo {
            Arc::new(ToneBackend)
        } else {
            Arc::new(WasapiBackend)
        };
        let loaded = Settings::load(&settings_path);
        // Browser tabs come from the bridge, in demo mode as well.
        let browser_host = Arc::new(AppHost::default());
        let browser = Browser::new(&loaded.bridge, browser_host.clone());
        let backend: Arc<dyn CaptureBackend> =
            Arc::new(RoutingBackend::new(devices, browser.tab_source()));
        let detection = Detection::new(&loaded.detection);
        let settings = Arc::new(Mutex::new(loaded));
        let root_app = app.clone();
        let repos_settings = settings.clone();
        let briefing_settings = settings.clone();
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
        let env_settings = settings.clone();
        let env_app = app.clone();
        let publishing = Publishing::new(
            store.clone(),
            Arc::new(AppEnv::new(
                move || {
                    env_settings
                        .lock()
                        .map(|s| s.cycle_repos.clone())
                        .unwrap_or_default()
                },
                move || {
                    env_app
                        .state::<AppState>()
                        .recordings_root()
                        .unwrap_or_default()
                },
                Arc::new(crate::secrets::Credentials),
            )),
        );
        Ok(Self {
            recorder: Recorder::new(store.clone(), backend, demo),
            processing,
            publishing,
            store,
            settings,
            settings_path,
            data_dir,
            default_recordings,
            pending_shot: Mutex::new(None),
            browser,
            browser_host,
            detection,
            // The tasks come from the issues Stage 5 reads back from GitHub; a repository that
            // is not linked or whose issues were never read falls back to the cycle files.
            briefing: Arc::new(PublishingSource::new(
                move || {
                    briefing_settings
                        .lock()
                        .map(|s| s.cycle_repos.clone())
                        .unwrap_or_default()
                },
                Arc::new(crate::secrets::Credentials),
                crate::issue_sync::quick_client,
            )),
            demo,
        })
    }

    /// Where the briefing gets its tasks and topics from.
    pub fn briefing(&self) -> &Arc<dyn BriefingSource> {
        &self.briefing
    }

    /// The bridge to the Chrome extension.
    pub fn browser(&self) -> &Arc<Browser> {
        &self.browser
    }

    /// Start the bridge once the application is set up.
    pub fn start_browser(&self, app: AppHandle) {
        self.browser_host.bind(app);
        match self.settings() {
            Ok(s) => self.browser.apply(&s.bridge),
            Err(e) => tracing::error!(error = %e, "bridge not started"),
        }
    }

    /// Meeting detection.
    pub fn detection(&self) -> &Arc<Detection> {
        &self.detection
    }

    /// Start watching for calls once the application is set up (not in demo mode, which has
    /// no real audio sessions).
    pub fn start_detection(&self, app: AppHandle) {
        if !self.demo {
            self.detection.start(app);
        }
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

    pub fn publishing(&self) -> &Arc<Publishing> {
        &self.publishing
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
        new.bridge
            .validate()
            .map_err(|m| IpcError::new("invalid", m))?;
        let bridge = self.update_settings(|s| {
            new.check_new_locations(s)
                .map_err(|p| IpcError::new("invalid", p.message))?;
            *s = new.keeping_managed_from(s);
            Ok(s.bridge.clone())
        })?;
        // Outside the settings lock: the bridge may report back into the settings.
        self.browser.apply(&bridge);
        Ok(())
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
