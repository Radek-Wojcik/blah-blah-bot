//! Global hotkeys: Record / Stop and region screenshot.

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::commands::{emit_meetings_changed, emit_state};
use crate::ipc::{IpcError, IpcResult};
use crate::overlay;
use crate::recording_dto::RecordingState;
use crate::settings::Settings;
use crate::state::AppState;

/// Replace the registered hotkeys with the ones in `settings`. When one cannot be
/// registered (another program owns it, or the text is not a shortcut) nothing changes.
pub fn register(app: &AppHandle, settings: &Settings) -> IpcResult<()> {
    let gs = app.global_shortcut();
    let previous = gs.unregister_all();
    let result = (|| {
        gs.on_shortcut(settings.record_hotkey.as_str(), |app, _, event| {
            if event.state() == ShortcutState::Pressed {
                on_record_hotkey(app);
            }
        })
        .map_err(|e| hotkey_error(&settings.record_hotkey, &e))?;
        gs.on_shortcut(settings.screenshot_hotkey.as_str(), |app, _, event| {
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
        .map_err(|e| hotkey_error(&settings.screenshot_hotkey, &e))?;
        Ok(())
    })();
    if result.is_err() {
        // Leave the application without half-registered keys.
        let _ = gs.unregister_all();
    }
    let _ = previous;
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
