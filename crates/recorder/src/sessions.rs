//! Listing of audio sessions on all active playback devices, used to offer the applications
//! that can be captured, and the poll of playback and microphone sessions that feeds the
//! meeting detector.

use serde::Serialize;
use wasapi::{initialize_mta, DeviceEnumerator, Direction};

use crate::detect::{resolve_app, AppSession, Flow, Level, SessionState};
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

/// List sessions of all active render devices. Session 0 (system sounds) is included with
/// pid 0.
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
                let proc = sys.process(sysinfo::Pid::from_u32(pid));
                let (volume, muted, peak) = session_levels(&device, i);
                out.push(SessionInfo {
                    pid,
                    parent_pid: proc.and_then(|p| p.parent()).map(sysinfo::Pid::as_u32),
                    process: proc
                        .map(|p| p.name().to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    display_name: session.get_display_name().unwrap_or_default(),
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

/// Sessions of all active devices of one direction, reading only what the detector needs: the
/// owner, the state and the coarse peak. One activation per device.
fn raw_sessions(enumerator: &DeviceEnumerator, direction: Direction) -> Result<Vec<RawSession>> {
    use windows::core::Interface;
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{
        AudioSessionStateActive, AudioSessionStateExpired, IAudioSessionControl2,
        IAudioSessionManager2,
    };
    use windows::Win32::System::Com::CLSCTX_ALL;

    let flow = flow_of(direction);
    let devices = enumerator
        .get_device_collection(direction)
        .map_err(|e| RecorderError::audio("list audio devices", e))?;
    let mut out = Vec::new();
    for device in &devices {
        let Ok(device) = device else { continue };
        // SAFETY: plain COM calls on interfaces obtained from the system; COM is initialized
        // on this thread.
        unsafe {
            let Ok(manager) = device
                .as_immdevice()
                .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
            else {
                continue;
            };
            let Ok(list) = manager.GetSessionEnumerator() else {
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
                    flow,
                    state,
                    peak: Level::from_peak(peak),
                });
            }
        }
    }
    Ok(out)
}

/// Longest parent chain whose processes are refreshed.
const CHAIN: usize = 8;

/// Polls playback and microphone sessions and ties each to its application. Meant to be kept
/// by one thread and polled about once a second: it keeps its process table between polls and
/// refreshes only the owners of the sessions and their parents, not every process.
pub struct SessionMonitor {
    enumerator: DeviceEnumerator,
    sys: sysinfo::System,
    known: Vec<String>,
    own_pid: u32,
}

impl SessionMonitor {
    /// `known` are the application stems that session owners are mapped to when one of their
    /// ancestors is such an application (see [`crate::detect::resolve_app`]).
    pub fn new(known: Vec<String>) -> Result<Self> {
        let _ = initialize_mta();
        Ok(Self {
            enumerator: DeviceEnumerator::new()
                .map_err(|e| RecorderError::audio("device enumerator", e))?,
            sys: sysinfo::System::new(),
            known,
            own_pid: std::process::id(),
        })
    }

    /// Change the applications that session owners are mapped to.
    pub fn set_known(&mut self, known: Vec<String>) {
        self.known = known;
    }

    /// One poll. Sessions of this process and the system sounds session are left out.
    pub fn poll(&mut self) -> Result<Vec<AppSession>> {
        let mut raw = raw_sessions(&self.enumerator, Direction::Render)?;
        raw.extend(raw_sessions(&self.enumerator, Direction::Capture)?);
        raw.retain(|s| s.pid != 0 && s.pid != self.own_pid);
        self.refresh(&raw);

        let sys = &self.sys;
        let lookup = |pid: u32| {
            sys.process(sysinfo::Pid::from_u32(pid)).map(|p| {
                (
                    p.name().to_string_lossy().into_owned(),
                    p.parent().map(sysinfo::Pid::as_u32),
                )
            })
        };
        let mut out: Vec<AppSession> = raw
            .iter()
            .filter_map(|s| {
                let app = resolve_app(s.pid, &self.known, lookup)?;
                Some(AppSession {
                    app: app.app,
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

    /// Refresh the session owners and, one level at a time, their parents.
    fn refresh(&mut self, raw: &[RawSession]) {
        let mut frontier: Vec<sysinfo::Pid> =
            raw.iter().map(|s| sysinfo::Pid::from_u32(s.pid)).collect();
        frontier.sort();
        frontier.dedup();
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
    }
}
