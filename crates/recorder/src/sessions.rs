//! Listing of audio sessions on all active playback devices, used to offer the applications
//! that can be captured.

use serde::Serialize;
use wasapi::{initialize_mta, DeviceEnumerator, Direction};

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
    // Already initialized on this thread is fine; the result is intentionally ignored.
    let _ = initialize_mta();
    let enumerator =
        DeviceEnumerator::new().map_err(|e| RecorderError::audio("device enumerator", e))?;
    let devices = enumerator
        .get_device_collection(Direction::Render)
        .map_err(|e| RecorderError::audio("list playback devices", e))?;

    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut out = Vec::new();
    for device in &devices {
        let device = device.map_err(|e| RecorderError::audio("open playback device", e))?;
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
            });
        }
    }
    out.sort_by_key(|s| (s.pid, s.device.clone()));
    Ok(out)
}
