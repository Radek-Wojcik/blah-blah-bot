//! Listing of audio sessions on all active playback devices, used to offer the applications
//! that can be captured, and the poll of playback and microphone sessions that feeds the
//! meeting detector.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Serialize;
use wasapi::{initialize_mta, DeviceEnumerator, Direction};

use crate::detect::{resolve_app, AppSession, Flow, Level, ResolvedApp, SessionState};
use crate::error::{RecorderError, Result};

/// One audio session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    /// Process that owns the session (0 for the system sounds session).
    pub pid: u32,
    /// Parent of that process, when known. Record the topmost process of an application
    /// (the one whose parent is not the same program) to include its helper processes.
    pub parent_pid: Option<u32>,
    /// Executable name of the process.
    pub process: String,
    /// Display name set by the application.
    pub display_name: String,
    /// Session state (active, inactive, expired).
    pub state: String,
    /// Playback device the session lives on.
    pub device: String,
    /// Session volume 0.0 to 1.0 (process loopback is captured after this volume).
    pub volume: Option<f32>,
    pub muted: Option<bool>,
    /// Current peak level of the session, 0.0 to 1.0.
    pub peak: Option<f32>,
    /// Playback or microphone session.
    pub flow: Flow,
}

/// Volume, mute flag and peak of one session of a device.
fn session_levels(device: &wasapi::Device, index: i32) -> (Option<f32>, Option<bool>, Option<f32>) {
    use windows::core::Interface;
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{IAudioSessionManager2, ISimpleAudioVolume};
    use windows::Win32::System::Com::CLSCTX_ALL;

    // SAFETY: plain COM calls on interfaces obtained from the system; COM is initialized.
    unsafe {
        let Ok(manager) = device
            .as_immdevice()
            .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
        else {
            return (None, None, None);
        };
        let Ok(list) = manager.GetSessionEnumerator() else {
            return (None, None, None);
        };
        let Ok(control) = list.GetSession(index) else {
            return (None, None, None);
        };
        let vol = control.cast::<ISimpleAudioVolume>().ok();
        (
            vol.as_ref().and_then(|v| v.GetMasterVolume().ok()),
            vol.as_ref()
                .and_then(|v| v.GetMute().ok())
                .map(|m| m.as_bool()),
            control
                .cast::<IAudioMeterInformation>()
                .ok()
                .and_then(|m| m.GetPeakValue().ok()),
        )
    }
}

/// List sessions of all active render devices. System sound and system audio host sessions
/// (`pid == 0`, `AudioSrv.dll`, `audiodg`, `svchost`, `system`) are excluded.
pub fn list_sessions() -> Result<Vec<SessionInfo>> {
    list_sessions_of(&[Direction::Render])
}

/// List the sessions of all active playback devices and of all active microphones, with the
/// direction in `flow`.
pub fn list_sessions_with_capture() -> Result<Vec<SessionInfo>> {
    list_sessions_of(&[Direction::Render, Direction::Capture])
}

fn flow_of(direction: Direction) -> Flow {
    match direction {
        Direction::Render => Flow::Render,
        Direction::Capture => Flow::Capture,
    }
}

fn list_sessions_of(directions: &[Direction]) -> Result<Vec<SessionInfo>> {
    // Already initialized on this thread is fine; the result is intentionally ignored.
    let _ = initialize_mta();
    let enumerator =
        DeviceEnumerator::new().map_err(|e| RecorderError::audio("device enumerator", e))?;

    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut out = Vec::new();
    for direction in directions {
        let flow = flow_of(*direction);
        let devices = enumerator
            .get_device_collection(*direction)
            .map_err(|e| RecorderError::audio("list audio devices", e))?;
        for device in &devices {
            let device = device.map_err(|e| RecorderError::audio("open audio device", e))?;
            let device_name = device.get_friendlyname().unwrap_or_default();
            let Ok(manager) = device.get_iaudiosessionmanager() else {
                continue;
            };
            let Ok(sessions) = manager.get_audiosessionenumerator() else {
                continue;
            };
            let count = sessions.get_count().unwrap_or(0);
            for i in 0..count {
                let Ok(session) = sessions.get_session(i) else {
                    continue;
                };
                let pid = session.get_process_id().unwrap_or(0);
                if pid == 0 {
                    continue;
                }
                let proc = sys.process(sysinfo::Pid::from_u32(pid));
                let process = proc
                    .map(|p| p.name().to_string_lossy().into_owned())
                    .unwrap_or_default();
                let display_name = session.get_display_name().unwrap_or_default();
                if crate::source_list::is_system_audio_session(pid, &process, &display_name) {
                    continue;
                }
                let (volume, muted, peak) = session_levels(&device, i);
                out.push(SessionInfo {
                    pid,
                    parent_pid: proc.and_then(|p| p.parent()).map(sysinfo::Pid::as_u32),
                    process,
                    display_name,
                    state: session
                        .get_state()
                        .map_or_else(|_| "unknown".into(), |s| s.to_string()),
                    device: device_name.clone(),
                    volume,
                    muted,
                    peak,
                    flow,
                });
            }
        }
    }
    out.sort_by_key(|s| (s.pid, s.flow, s.device.clone()));
    Ok(out)
}

/// One session as the Windows audio stack reports it, before it is tied to an application.
struct RawSession {
    pid: u32,
    flow: Flow,
    state: SessionState,
    peak: Level,
}

type Managers = Vec<(Flow, windows::Win32::Media::Audio::IAudioSessionManager2)>;

/// The session manager of every active playback and microphone device.
fn open_managers(enumerator: &DeviceEnumerator) -> Result<Managers> {
    use windows::Win32::Media::Audio::IAudioSessionManager2;
    use windows::Win32::System::Com::CLSCTX_ALL;

    let mut out = Vec::new();
    for direction in [Direction::Render, Direction::Capture] {
        let devices = enumerator
            .get_device_collection(direction)
            .map_err(|e| RecorderError::audio("list audio devices", e))?;
        for device in &devices {
            let Ok(device) = device else { continue };
            // SAFETY: plain COM call on an interface obtained from the system; COM is
            // initialized on this thread.
            let manager = unsafe {
                device
                    .as_immdevice()
                    .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
            };
            if let Ok(manager) = manager {
                out.push((flow_of(direction), manager));
            }
        }
    }
    Ok(out)
}

/// Sessions of all given managers, reading only what the detector needs: the owner, the
/// state and the coarse peak. The second value is `false` when a manager failed (a device
/// went away), so the caller opens them again.
fn read_sessions(managers: &Managers) -> (Vec<RawSession>, bool) {
    use windows::core::Interface;
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{
        AudioSessionStateActive, AudioSessionStateExpired, IAudioSessionControl2,
    };

    let mut out = Vec::new();
    let mut ok = true;
    for (flow, manager) in managers {
        // SAFETY: plain COM calls on interfaces obtained from the system; COM is initialized
        // on this thread.
        unsafe {
            let Ok(list) = manager.GetSessionEnumerator() else {
                ok = false;
                continue;
            };
            for i in 0..list.GetCount().unwrap_or(0) {
                let Ok(control) = list.GetSession(i) else {
                    continue;
                };
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let state = match control.GetState() {
                    Ok(s) if s == AudioSessionStateActive => SessionState::Active,
                    Ok(s) if s == AudioSessionStateExpired => SessionState::Expired,
                    _ => SessionState::Inactive,
                };
                let peak = control
                    .cast::<IAudioMeterInformation>()
                    .ok()
                    .and_then(|m| m.GetPeakValue().ok());
                out.push(RawSession {
                    pid: control2.GetProcessId().unwrap_or(0),
                    flow: *flow,
                    state,
                    peak: Level::from_peak(peak),
                });
            }
        }
    }
    (out, ok)
}

/// Longest parent chain whose processes are refreshed.
const CHAIN: usize = 8;
/// The list of devices is read again this often (a device may be plugged in or removed).
const DEVICES_EVERY: Duration = Duration::from_secs(30);
/// The application of a process is looked up again this often.
const APPS_EVERY: Duration = Duration::from_secs(300);

/// Polls playback and microphone sessions and ties each to its application. Meant to be kept
/// by one thread (create it there) and polled about once a second. The session managers of
/// the devices are kept between polls, and the application of a process is looked up once
/// when its session first appears, from a refresh of that process and its parents only (not
/// of every process); the answer is kept until the session is gone.
pub struct SessionMonitor {
    enumerator: DeviceEnumerator,
    managers: Managers,
    managers_at: Option<Instant>,
    sys: sysinfo::System,
    known: Vec<String>,
    own_pid: u32,
    apps: HashMap<u32, Option<ResolvedApp>>,
    apps_at: Instant,
}

impl SessionMonitor {
    /// `known` are the application stems that session owners are mapped to when one of their
    /// ancestors is such an application (see [`crate::detect::resolve_app`]).
    pub fn new(known: Vec<String>) -> Result<Self> {
        let _ = initialize_mta();
        Ok(Self {
            enumerator: DeviceEnumerator::new()
                .map_err(|e| RecorderError::audio("device enumerator", e))?,
            managers: Vec::new(),
            managers_at: None,
            sys: sysinfo::System::new(),
            known,
            own_pid: std::process::id(),
            apps: HashMap::new(),
            apps_at: Instant::now(),
        })
    }

    /// Change the applications that session owners are mapped to.
    pub fn set_known(&mut self, known: Vec<String>) {
        if known != self.known {
            self.known = known;
            self.apps.clear();
        }
    }

    /// One poll. Sessions of this process and the system sounds session are left out.
    pub fn poll(&mut self) -> Result<Vec<AppSession>> {
        let now = Instant::now();
        if self
            .managers_at
            .is_none_or(|t| now.duration_since(t) >= DEVICES_EVERY)
        {
            self.managers = open_managers(&self.enumerator)?;
            self.managers_at = Some(now);
        }
        let (mut raw, ok) = read_sessions(&self.managers);
        if !ok {
            // A device went away: read the device list again at the next poll.
            self.managers_at = None;
        }
        raw.retain(|s| s.pid != 0 && s.pid != self.own_pid);

        if now.duration_since(self.apps_at) >= APPS_EVERY {
            self.apps.clear();
            self.apps_at = now;
        }
        let mut missing: Vec<u32> = raw
            .iter()
            .map(|s| s.pid)
            .filter(|p| !self.apps.contains_key(p))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        if !missing.is_empty() {
            self.resolve(&missing);
        }
        // A pid that has no session now is looked up again when it comes back.
        self.apps.retain(|pid, _| raw.iter().any(|s| s.pid == *pid));

        let mut out: Vec<AppSession> = raw
            .iter()
            .filter_map(|s| {
                let app = self.apps.get(&s.pid)?.as_ref()?;
                Some(AppSession {
                    app: app.app.clone(),
                    pid: app.top_pid,
                    flow: s.flow,
                    state: s.state,
                    peak: s.peak,
                })
            })
            .collect();
        out.sort_by(|a, b| (&a.app, a.pid, a.flow).cmp(&(&b.app, b.pid, b.flow)));
        Ok(out)
    }

    /// Refresh `pids` and, one level at a time, their parents; then map each pid.
    fn resolve(&mut self, pids: &[u32]) {
        let mut frontier: Vec<sysinfo::Pid> =
            pids.iter().map(|p| sysinfo::Pid::from_u32(*p)).collect();
        for _ in 0..CHAIN {
            if frontier.is_empty() {
                break;
            }
            self.sys.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&frontier),
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );
            let mut parents: Vec<sysinfo::Pid> = frontier
                .iter()
                .filter_map(|p| self.sys.process(*p).and_then(sysinfo::Process::parent))
                .collect();
            parents.sort();
            parents.dedup();
            frontier = parents;
        }
        let sys = &self.sys;
        let lookup = |pid: u32| {
            sys.process(sysinfo::Pid::from_u32(pid)).map(|p| {
                (
                    p.name().to_string_lossy().into_owned(),
                    p.parent().map(sysinfo::Pid::as_u32),
                )
            })
        };
        for pid in pids {
            let resolved = resolve_app(*pid, &self.known, lookup)
                .filter(|r| !crate::source_list::is_system_audio_host(&r.app));
            self.apps.insert(*pid, resolved);
        }
    }
}
