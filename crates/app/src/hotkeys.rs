//! Global hotkeys: Record / Stop and region screenshot.

use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::commands::{emit_meetings_changed, emit_state};
use crate::ipc::{IpcError, IpcResult};
use crate::overlay;
use crate::recording_dto::RecordingState;
use crate::settings::Settings;
use crate::state::AppState;

/// Registration state shared by the startup path, `save_settings` and the capture suspend.
struct Registry {
    /// Global hotkeys are unregistered while the UI records a new shortcut.
    suspended: bool,
    record_error: Option<String>,
    screenshot_error: Option<String>,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    suspended: false,
    record_error: None,
    screenshot_error: None,
});

/// Serialises every hotkey change (suspend, resume, save) so they never interleave.
static CHANGE_LOCK: Mutex<()> = Mutex::new(());

pub fn change_lock() -> MutexGuard<'static, ()> {
    CHANGE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Why a configured hotkey is not active (for the Record tab); `None` when it works.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyStatus {
    pub record_error: Option<String>,
    pub screenshot_error: Option<String>,
}

pub fn status() -> HotkeyStatus {
    let r = registry();
    HotkeyStatus {
        record_error: r.record_error.clone(),
        screenshot_error: r.screenshot_error.clone(),
    }
}

/// True for `F1`..`F24`.
fn is_function_key(shortcut: &Shortcut) -> bool {
    let name = format!("{:?}", shortcut.key);
    name.strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

/// Parse a hotkey string into a `Shortcut`. Empty strings return `Ok(None)` (unassigned).
/// Non-empty strings must include Ctrl, Alt or Super; Shift alone is accepted only with an
/// F-key (Shift+S or Shift+Tab would hijack typing and focus movement system-wide).
pub fn parse_shortcut(text: &str) -> IpcResult<Option<Shortcut>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let shortcut = trimmed
        .parse::<Shortcut>()
        .map_err(|e| hotkey_error(trimmed, &e))?;
    let strong = Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER;
    let ok = shortcut.mods.intersects(strong)
        || (shortcut.mods.contains(Modifiers::SHIFT) && is_function_key(&shortcut));
    if !ok {
        return Err(IpcError::new(
            "invalid",
            format!(
                "`{trimmed}` must include Ctrl, Alt or Super (Shift alone works only with F1-F24)"
            ),
        ));
    }
    Ok(Some(shortcut))
}

/// Validate both hotkeys in `settings` and ensure two non-empty hotkeys are not identical.
pub fn validate_pair(settings: &Settings) -> IpcResult<(Option<Shortcut>, Option<Shortcut>)> {
    let record = parse_shortcut(&settings.record_hotkey)?;
    let screenshot = parse_shortcut(&settings.screenshot_hotkey)?;
    if let (Some(r), Some(s)) = (record, screenshot) {
        if r == s {
            return Err(IpcError::new(
                "invalid",
                "Record and region screenshot hotkeys must be different",
            ));
        }
    }
    Ok((record, screenshot))
}

fn register_record(app: &AppHandle, shortcut: Shortcut, raw: &str) -> IpcResult<()> {
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _, event| {
            if event.state() == ShortcutState::Pressed {
                on_record_hotkey(app);
            }
        })
        .map_err(|e| hotkey_error(raw, &e))
}

fn register_screenshot(app: &AppHandle, shortcut: Shortcut, raw: &str) -> IpcResult<()> {
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _, event| {
            if event.state() == ShortcutState::Pressed {
                // Window creation must not run on the thread that delivers the hotkey.
                let app = app.clone();
                std::thread::spawn(move || {
                    if let Err(e) = overlay::begin(&app) {
                        tracing::info!(error = %e, "screenshot hotkey ignored");
                    }
                });
            }
        })
        .map_err(|e| hotkey_error(raw, &e))
}

/// Register hotkeys on a per-key best-effort basis so one taken or invalid hotkey does not
/// prevent the other from working or block startup. Failures are kept for `status()`.
pub fn register_initial(app: &AppHandle, settings: &Settings) {
    let (record_error, screenshot_error) = register_best_effort(app, settings);
    let mut r = registry();
    r.record_error = record_error;
    r.screenshot_error = screenshot_error;
}

fn register_best_effort(app: &AppHandle, settings: &Settings) -> (Option<String>, Option<String>) {
    let mut record_error = None;
    let mut screenshot_error = None;

    let rec = match parse_shortcut(&settings.record_hotkey) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(hotkey = %settings.record_hotkey, error = %e.message, "record hotkey invalid");
            record_error = Some(e.message);
            None
        }
    };
    if let Some(shortcut) = rec {
        if let Err(e) = register_record(app, shortcut, &settings.record_hotkey) {
            tracing::warn!(hotkey = %settings.record_hotkey, error = %e.message, "record hotkey not registered");
            record_error = Some(e.message);
        }
    }

    let scr = match parse_shortcut(&settings.screenshot_hotkey) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(hotkey = %settings.screenshot_hotkey, error = %e.message, "screenshot hotkey invalid");
            screenshot_error = Some(e.message);
            None
        }
    };
    if let Some(shortcut) = scr {
        if rec == Some(shortcut) {
            tracing::warn!(
                hotkey = %settings.screenshot_hotkey,
                "screenshot hotkey duplicates record hotkey; skipping"
            );
            screenshot_error =
                Some("Region screenshot hotkey duplicates the Record / Stop hotkey".into());
        } else if let Err(e) = register_screenshot(app, shortcut, &settings.screenshot_hotkey) {
            tracing::warn!(hotkey = %settings.screenshot_hotkey, error = %e.message, "screenshot hotkey not registered");
            screenshot_error = Some(e.message);
        }
    }
    (record_error, screenshot_error)
}

/// Unregister every global hotkey while the UI records a new shortcut, so pressing the current
/// combination is captured instead of triggering its action. Idempotent.
pub fn suspend(app: &AppHandle) {
    let _guard = change_lock();
    let mut r = registry();
    if r.suspended {
        return;
    }
    let _ = app.global_shortcut().unregister_all();
    r.suspended = true;
}

/// Register the hotkeys from `settings` again after a capture. A no-op unless suspended (a save
/// in between already registered the new ones).
pub fn resume(app: &AppHandle, settings: &Settings) {
    let _guard = change_lock();
    {
        let mut r = registry();
        if !r.suspended {
            return;
        }
        r.suspended = false;
    }
    let _ = app.global_shortcut().unregister_all();
    register_initial(app, settings);
}

/// Replace the registered hotkeys with `next`. Validates first, then swaps atomically; if
/// OS registration fails, rolls back to `previous` and returns `hotkey_in_use`. The caller holds
/// `change_lock()`.
pub fn replace(app: &AppHandle, next: &Settings, previous: &Settings) -> IpcResult<()> {
    let (record, screenshot) = validate_pair(next)?;
    // Unchanged hotkeys stay as they are: one that is taken by another application must not
    // block saving unrelated options.
    if hotkeys_unchanged(next, previous) {
        return Ok(());
    }
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    registry().suspended = false;

    let result = (|| {
        if let Some(shortcut) = record {
            register_record(app, shortcut, next.record_hotkey.trim())?;
        }
        if let Some(shortcut) = screenshot {
            register_screenshot(app, shortcut, next.screenshot_hotkey.trim())?;
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = gs.unregister_all();
        register_initial(app, previous);
    } else {
        let mut r = registry();
        r.record_error = None;
        r.screenshot_error = None;
    }
    result
}

fn hotkeys_unchanged(next: &Settings, previous: &Settings) -> bool {
    let same = |a: &str, b: &str| match (parse_shortcut(a), parse_shortcut(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    };
    same(&next.record_hotkey, &previous.record_hotkey)
        && same(&next.screenshot_hotkey, &previous.screenshot_hotkey)
}

fn hotkey_error(text: &str, e: &dyn std::fmt::Display) -> IpcError {
    tracing::warn!(hotkey = text, error = %e, "hotkey registration failed");
    IpcError::new(
        "hotkey_in_use",
        format!("`{text}` cannot be used as a hotkey (already taken or not a valid shortcut)"),
    )
}

/// Stops a running recording; otherwise brings up the recorder widget.
fn on_record_hotkey(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.recorder.is_recording() {
        match state.recorder.stop() {
            Ok(_) => {
                state.browser().sync_recording(None);
                overlay::cancel(app);
                emit_state(app, &RecordingState::idle());
                emit_meetings_changed(app);
            }
            Err(e) => tracing::error!(error = %e, "stop from hotkey failed"),
        }
    } else if let Some(w) = app.get_webview_window("widget") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shortcut_allows_empty_and_requires_modifier() {
        assert_eq!(parse_shortcut("").unwrap(), None);
        assert_eq!(parse_shortcut("   ").unwrap(), None);
        assert!(parse_shortcut("Ctrl+Shift+R").unwrap().is_some());
        assert!(parse_shortcut("F9").is_err());
        assert!(parse_shortcut("KeyR").is_err());
        assert!(parse_shortcut("Ctrl+Alt+Shift+Tab").unwrap().is_some());
        assert!(parse_shortcut("Super+R").unwrap().is_some());
        assert!(parse_shortcut("NotAKey+Ctrl").is_err());
    }

    #[test]
    fn shift_alone_is_only_accepted_with_function_keys() {
        assert!(parse_shortcut("Shift+S").is_err());
        assert!(parse_shortcut("Shift+Tab").is_err());
        assert!(parse_shortcut("Shift+Space").is_err());
        assert!(parse_shortcut("Shift+1").is_err());
        assert!(parse_shortcut("Shift+F9").unwrap().is_some());
        assert!(parse_shortcut("Shift+F24").unwrap().is_some());
        assert!(parse_shortcut("Ctrl+S").unwrap().is_some());
    }

    #[test]
    fn every_key_the_ui_can_record_parses() {
        for key in [
            "R",
            "5",
            "F1",
            "F24",
            "Space",
            "Tab",
            "Enter",
            "Up",
            "Down",
            "Left",
            "Right",
            "Insert",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "-",
            "=",
            "[",
            "]",
            "\\",
            ";",
            "'",
            "`",
            ",",
            ".",
            "/",
            "Numpad0",
            "Numpad9",
            "NumpadAdd",
            "NumpadSubtract",
            "NumpadMultiply",
            "NumpadDivide",
            "NumpadDecimal",
            "NumpadEnter",
        ] {
            let text = format!("Ctrl+Alt+{key}");
            assert!(
                parse_shortcut(&text).unwrap().is_some(),
                "`{text}` should parse"
            );
        }
        assert!(parse_shortcut("Super+Shift+R").unwrap().is_some());
    }

    #[test]
    fn unchanged_hotkeys_are_detected_ignoring_case_and_order() {
        let a = Settings::default();
        let mut b = a.clone();
        assert!(hotkeys_unchanged(&b, &a));
        b.record_hotkey = "shift+alt+ctrl+f9".into();
        assert!(hotkeys_unchanged(&b, &a));
        b.screenshot_hotkey = "Ctrl+Alt+F10".into();
        assert!(!hotkeys_unchanged(&b, &a));
    }

    #[test]
    fn default_hotkeys_are_valid_and_distinct() {
        assert!(validate_pair(&Settings::default()).is_ok());
    }

    #[test]
    fn validate_pair_rejects_duplicates_and_allows_empty() {
        let mut s = Settings::default();
        assert!(validate_pair(&s).is_ok());

        s.record_hotkey = String::new();
        assert!(validate_pair(&s).is_ok());

        s.screenshot_hotkey = String::new();
        assert!(validate_pair(&s).is_ok());

        s.record_hotkey = "Ctrl+Alt+Shift+F9".into();
        s.screenshot_hotkey = "shift+alt+ctrl+f9".into();
        let err = validate_pair(&s).unwrap_err();
        assert_eq!(err.code, "invalid");
    }
}
