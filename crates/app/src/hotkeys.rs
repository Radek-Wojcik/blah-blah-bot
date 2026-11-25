//! Global hotkeys: Record / Stop and region screenshot.

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::commands::{emit_meetings_changed, emit_state};
use crate::ipc::{IpcError, IpcResult};
use crate::overlay;
use crate::recording_dto::RecordingState;
use crate::settings::Settings;
use crate::state::AppState;

/// Parse a hotkey string into a `Shortcut`. Empty strings return `Ok(None)` (unassigned).
/// Non-empty strings must parse as a valid shortcut with at least one modifier key.
pub fn parse_shortcut(text: &str) -> IpcResult<Option<Shortcut>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let shortcut = trimmed
        .parse::<Shortcut>()
        .map_err(|e| hotkey_error(trimmed, &e))?;
    if shortcut.mods.is_empty() {
        return Err(IpcError::new(
            "invalid",
            format!(
                "`{trimmed}` must include at least one modifier key (Ctrl, Alt, Shift, or Super)"
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

/// Register hotkeys at startup on a per-key best-effort basis so one taken or invalid hotkey
/// does not prevent the other from working or block startup.
pub fn register_initial(app: &AppHandle, settings: &Settings) {
    let rec = match parse_shortcut(&settings.record_hotkey) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(hotkey = %settings.record_hotkey, error = %e.message, "record hotkey invalid");
            None
        }
    };
    if let Some(shortcut) = rec {
        if let Err(e) = register_record(app, shortcut, &settings.record_hotkey) {
            tracing::warn!(hotkey = %settings.record_hotkey, error = %e.message, "record hotkey not registered");
        }
    }

    let scr = match parse_shortcut(&settings.screenshot_hotkey) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(hotkey = %settings.screenshot_hotkey, error = %e.message, "screenshot hotkey invalid");
            None
        }
    };
    if let Some(shortcut) = scr {
        if rec == Some(shortcut) {
            tracing::warn!(
                hotkey = %settings.screenshot_hotkey,
                "screenshot hotkey duplicates record hotkey; skipping"
            );
            return;
        }
        if let Err(e) = register_screenshot(app, shortcut, &settings.screenshot_hotkey) {
            tracing::warn!(hotkey = %settings.screenshot_hotkey, error = %e.message, "screenshot hotkey not registered");
        }
    }
}

/// Replace the registered hotkeys with `next`. Validates first, then swaps atomically; if
/// OS registration fails, rolls back to `previous` and returns `hotkey_in_use`.
pub fn replace(app: &AppHandle, next: &Settings, previous: &Settings) -> IpcResult<()> {
    let (record, screenshot) = validate_pair(next)?;
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

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
    }
    result
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
        assert!(parse_shortcut("NotAKey+Ctrl").is_err());
    }

    #[test]
    fn validate_pair_rejects_duplicates_and_allows_empty() {
        let mut s = Settings::default();
        assert!(validate_pair(&s).is_ok());

        s.record_hotkey = String::new();
        assert!(validate_pair(&s).is_ok());

        s.screenshot_hotkey = String::new();
        assert!(validate_pair(&s).is_ok());

        s.record_hotkey = "Ctrl+Shift+R".into();
        s.screenshot_hotkey = "shift+ctrl+r".into();
        let err = validate_pair(&s).unwrap_err();
        assert_eq!(err.code, "invalid");
    }
}
