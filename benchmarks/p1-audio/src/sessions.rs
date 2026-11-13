//! Listing of audio sessions on all active playback devices.

use serde::Serialize;
use wasapi::{initialize_mta, DeviceEnumerator, Direction};

/// One audio session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub process: String,
    pub display_name: String,
    pub state: String,
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
pub fn list_sessions() -> Result<Vec<SessionInfo>, String> {
    let _ = initialize_mta();
    let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
    let devices = enumerator
        .get_device_collection(Direction::Render)
        .map_err(|e| e.to_string())?;

    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut out = Vec::new();
    for device in &devices {
        let device = device.map_err(|e| e.to_string())?;
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

/// Set volume (0.0 to 1.0) and mute state of every session owned by `pid` on all active
/// render devices. Returns the number of sessions changed.
///
/// Windows remembers the level per application, so a test player that was once muted starts
/// muted next time; process loopback captures after the session volume, which makes a muted
/// player record as silence.
pub fn set_session_levels(pid: u32, level: f32, mute: bool) -> Result<usize, String> {
    use windows::core::Interface;
    use windows::Win32::Media::Audio::{
        IAudioSessionControl2, IAudioSessionManager2, ISimpleAudioVolume,
    };
    use windows::Win32::System::Com::CLSCTX_ALL;

    let _ = initialize_mta();
    let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
    let devices = enumerator
        .get_device_collection(Direction::Render)
        .map_err(|e| e.to_string())?;
    let mut changed = 0;
    for device in &devices {
        let device = device.map_err(|e| e.to_string())?;
        // SAFETY: plain COM calls on interfaces obtained from the system; COM is initialized.
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
                let owner = control
                    .cast::<IAudioSessionControl2>()
                    .ok()
                    .and_then(|c| c.GetProcessId().ok());
                if owner != Some(pid) {
                    continue;
                }
                if let Ok(volume) = control.cast::<ISimpleAudioVolume>() {
                    let ok = volume.SetMasterVolume(level, std::ptr::null()).is_ok()
                        && volume.SetMute(mute, std::ptr::null()).is_ok();
                    if ok {
                        changed += 1;
                    }
                }
            }
        }
    }
    Ok(changed)
}
