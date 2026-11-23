//! Turns the audio sessions of the system into the applications the user can record.
//!
//! Pure logic over [`SessionInfo`] lists: sessions of one program become one source, tied to
//! the topmost process of that program so that helper processes are included. Google Chrome
//! is told apart as the whole-browser source, the fallback for tabs the extension cannot
//! capture. Other browsers are listed like any application; they are out of scope for
//! whole-browser capture.

use crate::detect::{app_stem, resolve_app};
use crate::sessions::SessionInfo;

/// Label of the whole-browser source.
pub const CHROME_LABEL: &str = "Chrome (whole browser)";

/// What an application source is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppKind {
    Application,
    /// Google Chrome: every tab and extension of the browser, through its top process.
    Chrome,
}

/// One application with an audio session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSource {
    /// Topmost process of the application; its whole process tree is captured.
    pub pid: u32,
    pub label: String,
    /// A session of the application is playing.
    pub active: bool,
    pub kind: AppKind,
}

/// True for the process of Google Chrome (not Edge, Brave and the other Chromium browsers).
pub fn is_chrome(process: &str) -> bool {
    app_stem(process) == "chrome"
}

/// True when `process` is a Windows system audio host (`audiodg`, `svchost`, `system`).
pub fn is_system_audio_host(process: &str) -> bool {
    matches!(
        app_stem(process).as_str(),
        "audiodg" | "svchost" | "system" | "system idle process"
    )
}

/// True when a Windows audio session display name points to `AudioSrv.dll` (the Windows
/// system-sounds resource string, e.g. `@%SystemRoot%\System32\AudioSrv.Dll,-202`).
pub fn is_audiosrv_display(display_name: &str) -> bool {
    display_name
        .trim()
        .to_ascii_lowercase()
        .contains("audiosrv.dll")
}

/// True when an audio session belongs to Windows system sounds or a system audio host and
/// must not be offered as an application source.
pub fn is_system_audio_session(pid: u32, process: &str, display_name: &str) -> bool {
    pid == 0
        || process.trim().is_empty()
        || is_system_audio_host(process)
        || is_audiosrv_display(display_name)
}

/// Capitalized executable stem (`Discord.exe` -> `Discord`, `spotify.exe` -> `Spotify`),
/// used when Windows reports an indirect MUI resource string (`@...`) as the session name.
pub fn clean_stem_label(process: &str) -> String {
    let stem = app_stem(process);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Choose the user-visible label for an application session.
fn session_label(process: &str, display_name: &str) -> String {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        process
            .strip_suffix(".exe")
            .or_else(|| process.strip_suffix(".EXE"))
            .unwrap_or(process)
            .to_string()
    } else if trimmed.starts_with('@') {
        clean_stem_label(process)
    } else {
        trimmed.to_string()
    }
}

/// Group `sessions` by program. Sessions of `own_pid`, of the system sounds (`pid == 0`,
/// `AudioSrv.dll`, or system audio hosts) and of programs already listed are skipped.
///
/// `lookup` gives the executable name and the parent of a process; it finds the real top of
/// Chrome, whose audio sessions belong to a helper process (the audio service) while the
/// browser process itself plays nothing.
pub fn classify_sessions(
    sessions: &[SessionInfo],
    own_pid: u32,
    lookup: impl Fn(u32) -> Option<(String, Option<u32>)>,
) -> Vec<AppSource> {
    let keep = |s: &SessionInfo| {
        s.pid != own_pid && !is_system_audio_session(s.pid, &s.process, &s.display_name)
    };
    let mut out = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for s in sessions {
        if !keep(s) || seen.contains(&s.process.as_str()) {
            continue;
        }
        seen.push(&s.process);
        let group: Vec<&SessionInfo> = sessions
            .iter()
            .filter(|x| x.process == s.process && keep(x))
            .collect();
        let top = group
            .iter()
            .find(|x| !group.iter().any(|y| Some(y.pid) == x.parent_pid))
            .unwrap_or(&group[0]);
        let active = group.iter().any(|x| x.state.eq_ignore_ascii_case("active"));
        if is_chrome(&s.process) {
            let pid = resolve_app(top.pid, &[], &lookup).map_or(top.pid, |a| a.top_pid);
            out.push(AppSource {
                pid,
                label: CHROME_LABEL.into(),
                active,
                kind: AppKind::Chrome,
            });
            continue;
        }
        let chosen_display = group
            .iter()
            .map(|x| x.display_name.trim())
            .find(|d| !d.is_empty() && !d.starts_with('@'))
            .unwrap_or_else(|| s.display_name.trim());
        let label = session_label(&s.process, chosen_display);
        out.push(AppSource {
            pid: top.pid,
            label,
            active,
            kind: AppKind::Application,
        });
    }
    out
}

/// The applications with an audio session on this computer, as sources to record.
pub fn list_apps() -> crate::Result<Vec<AppSource>> {
    let sessions = crate::sessions::list_sessions()?;
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let lookup = |pid: u32| {
        sys.process(sysinfo::Pid::from_u32(pid)).map(|p| {
            (
                p.name().to_string_lossy().into_owned(),
                p.parent().map(sysinfo::Pid::as_u32),
            )
        })
    };
    Ok(classify_sessions(&sessions, std::process::id(), lookup))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::detect::Flow;

    fn session(pid: u32, parent: Option<u32>, process: &str, state: &str) -> SessionInfo {
        SessionInfo {
            pid,
            parent_pid: parent,
            process: process.into(),
            display_name: String::new(),
            state: state.into(),
            device: "Speakers".into(),
            volume: Some(1.0),
            muted: Some(false),
            peak: Some(0.0),
            flow: Flow::Render,
        }
    }

    /// A process table: pid -> (name, parent).
    fn table(rows: &[(u32, &str, Option<u32>)]) -> impl Fn(u32) -> Option<(String, Option<u32>)> {
        let map: HashMap<u32, (String, Option<u32>)> = rows
            .iter()
            .map(|(pid, name, parent)| (*pid, ((*name).to_string(), *parent)))
            .collect();
        move |pid| map.get(&pid).cloned()
    }

    #[test]
    fn chrome_is_one_whole_browser_source_on_its_top_process() {
        // The audio service plays; the browser process (100) and its parent explorer do not.
        let sessions = [
            session(300, Some(100), "chrome.exe", "active"),
            session(310, Some(100), "chrome.exe", "inactive"),
        ];
        let lookup = table(&[
            (300, "chrome.exe", Some(100)),
            (310, "chrome.exe", Some(100)),
            (100, "chrome.exe", Some(50)),
            (50, "explorer.exe", None),
        ]);
        let out = classify_sessions(&sessions, 1, lookup);
        assert_eq!(
            out,
            vec![AppSource {
                pid: 100,
                label: CHROME_LABEL.into(),
                active: true,
                kind: AppKind::Chrome,
            }]
        );
    }

    #[test]
    fn chrome_is_recognized_by_name_in_any_case_and_only_chrome() {
        assert!(is_chrome("chrome.exe"));
        assert!(is_chrome("Chrome.EXE"));
        assert!(is_chrome("chrome"));
        for other in [
            "msedge.exe",
            "brave.exe",
            "firefox.exe",
            "chromedriver.exe",
            "",
        ] {
            assert!(!is_chrome(other), "{other}");
        }
    }

    #[test]
    fn chrome_falls_back_to_the_session_top_when_the_process_is_unknown() {
        let sessions = [
            session(300, Some(100), "chrome.exe", "inactive"),
            session(100, Some(50), "chrome.exe", "inactive"),
        ];
        // No process table: the topmost of the sessions is used.
        let out = classify_sessions(&sessions, 1, |_| None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pid, 100);
        assert_eq!(out[0].kind, AppKind::Chrome);
        assert!(!out[0].active);
    }

    #[test]
    fn other_browsers_and_applications_stay_applications() {
        let mut discord = session(500, Some(1), "Discord.exe", "active");
        discord.display_name = "Discord".into();
        let sessions = [
            session(200, Some(10), "msedge.exe", "inactive"),
            discord,
            session(510, Some(500), "Discord.exe", "inactive"),
            session(600, Some(1), "vlc.exe", "inactive"),
        ];
        let out = classify_sessions(&sessions, 1, |_| None);
        let summary: Vec<(u32, &str, bool, AppKind)> = out
            .iter()
            .map(|s| (s.pid, s.label.as_str(), s.active, s.kind))
            .collect();
        assert_eq!(
            summary,
            vec![
                (200, "msedge", false, AppKind::Application),
                // The helper is included through the topmost process, which is the parent.
                (500, "Discord", true, AppKind::Application),
                (600, "vlc", false, AppKind::Application),
            ]
        );
    }

    #[test]
    fn this_application_and_the_system_sounds_are_not_listed() {
        let sessions = [
            session(0, None, "", "active"),
            session(0, None, "System", "active"),
            session(42, Some(1), "blah-blah-bot.exe", "active"),
            session(7, Some(1), "vlc.exe", "inactive"),
        ];
        let out = classify_sessions(&sessions, 42, |_| None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pid, 7);
    }

    #[test]
    fn audiosrv_and_system_audio_hosts_are_filtered_out() {
        let mut audiosrv_on_svchost = session(1040, Some(1), "svchost.exe", "inactive");
        audiosrv_on_svchost.display_name = r"@%SystemRoot%\System32\AudioSrv.Dll,-202".into();

        let mut audiosrv_on_custom = session(1050, Some(1), "SomeHost.exe", "active");
        audiosrv_on_custom.display_name = r"@%systemroot%\system32\audiosrv.dll,-202".into();

        let audiodg = session(1060, Some(1), "audiodg.exe", "active");
        let system_proc = session(4, None, "System", "inactive");
        let pid_zero = session(0, None, "explorer.exe", "active");
        let spotify = session(2000, Some(1), "Spotify.exe", "active");

        let sessions = [
            audiosrv_on_svchost,
            audiosrv_on_custom,
            audiodg,
            system_proc,
            pid_zero,
            spotify,
        ];
        let out = classify_sessions(&sessions, 1, |_| None);
        assert_eq!(
            out,
            vec![AppSource {
                pid: 2000,
                label: "Spotify".into(),
                active: true,
                kind: AppKind::Application,
            }]
        );
    }

    #[test]
    fn indirect_mui_resource_names_fall_back_to_capitalized_process_stem() {
        let mut discord = session(300, Some(1), "discord.exe", "active");
        discord.display_name = r"@%ProgramFiles%\Discord\Discord.exe,-101".into();

        let mut teams = session(400, Some(1), "Teams.exe", "inactive");
        teams.display_name = r"@%ProgramFiles%\Microsoft\Teams\current\Teams.exe,-204".into();

        let mut spotify = session(500, Some(1), "spotify.exe", "active");
        spotify.display_name = r"@C:\Program Files\WindowsApps\SpotifyAB\Spotify.exe,-1".into();

        let sessions = [discord, teams, spotify];
        let out = classify_sessions(&sessions, 1, |_| None);
        let summary: Vec<(u32, &str, bool)> = out
            .iter()
            .map(|s| (s.pid, s.label.as_str(), s.active))
            .collect();
        assert_eq!(
            summary,
            vec![
                (300, "Discord", true),
                (400, "Teams", false),
                (500, "Spotify", true),
            ]
        );
    }
}
