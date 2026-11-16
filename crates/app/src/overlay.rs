//! Region screenshot overlay.
//!
//! The desktop is captured first and shown frozen in a borderless window that covers every
//! monitor. The user drags a rectangle; the UI reports it as fractions of the shown image,
//! which keeps the mapping independent of monitor scaling, then the chosen region is
//! cropped from the frozen capture and saved with the offset of the moment the hotkey was
//! pressed.

use recorder::screenshot::{capture_virtual_desktop, encode_png, Rect};
use serde::Deserialize;
use tauri::ipc::Response;
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl, WebviewWindowBuilder,
};

use crate::commands::emit_state;
use crate::ipc::{IpcError, IpcResult};
use crate::state::{AppState, PendingShot};

const LABEL: &str = "overlay";
/// Selections smaller than this (in pixels) in either direction are treated as a click.
const MIN_SIDE: u32 = 4;

/// Region chosen by the user, as fractions (0 to 1) of the image shown in the overlay.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct FractionRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Convert a fractional selection to pixels of the captured frame, clamped to it.
/// Returns `None` for an empty or tiny selection.
pub fn fraction_to_rect(frame: Rect, f: FractionRect) -> Option<Rect> {
    let fw = f64::from(frame.width);
    let fh = f64::from(frame.height);
    let x0 = (f.x.clamp(0.0, 1.0) * fw).round();
    let y0 = (f.y.clamp(0.0, 1.0) * fh).round();
    let x1 = ((f.x + f.w).clamp(0.0, 1.0) * fw).round();
    let y1 = ((f.y + f.h).clamp(0.0, 1.0) * fh).round();
    let (w, h) = (x1 - x0, y1 - y0);
    if w < f64::from(MIN_SIDE) || h < f64::from(MIN_SIDE) {
        return None;
    }
    Some(Rect::new(
        frame.x + x0 as i32,
        frame.y + y0 as i32,
        w as u32,
        h as u32,
    ))
}

/// Capture the desktop and open the overlay. Does nothing while no recording is running
/// or while an overlay is already open.
pub fn begin(app: &AppHandle) -> IpcResult<()> {
    let state = app.state::<AppState>();
    let offset_ms = state
        .recorder
        .now_ms()
        .ok_or_else(|| IpcError::new("not_recording", "Screenshots need a running recording"))?;
    {
        let pending = state
            .pending_shot
            .lock()
            .map_err(|_| IpcError::new("internal", "overlay lock poisoned"))?;
        if pending.is_some() {
            return Ok(());
        }
    }
    // Capture before any of our own windows appear.
    let frame = capture_virtual_desktop().map_err(|e| IpcError::new("io", e.to_string()))?;
    let rect = frame.rect;
    *state
        .pending_shot
        .lock()
        .map_err(|_| IpcError::new("internal", "overlay lock poisoned"))? =
        Some(PendingShot { frame, offset_ms });

    let window = WebviewWindowBuilder::new(
        app,
        LABEL,
        WebviewUrl::App("index.html?view=overlay".into()),
    )
    .decorations(false)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .visible(false)
    .build()
    .map_err(|e| IpcError::new("internal", e.to_string()))?;
    let placed = window
        .set_position(PhysicalPosition::new(rect.x, rect.y))
        .and_then(|()| window.set_size(PhysicalSize::new(rect.width, rect.height)))
        .and_then(|()| window.show())
        .and_then(|()| window.set_focus());
    if let Err(e) = placed {
        cancel(app);
        return Err(IpcError::new("internal", e.to_string()));
    }
    Ok(())
}

/// Close the overlay and drop the frozen capture.
pub fn cancel(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut p) = state.pending_shot.lock() {
            *p = None;
        }
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.destroy();
    }
}

/// The frozen capture as PNG bytes, for the overlay page.
#[tauri::command]
pub fn overlay_image(state: State<'_, AppState>) -> IpcResult<Response> {
    let pending = state
        .pending_shot
        .lock()
        .map_err(|_| IpcError::new("internal", "overlay lock poisoned"))?;
    let shot = pending
        .as_ref()
        .ok_or_else(|| IpcError::new("internal", "no screenshot in progress"))?;
    let png = encode_png(&shot.frame).map_err(|e| IpcError::new("io", e.to_string()))?;
    Ok(Response::new(png))
}

/// Finish the overlay: save the region, or only close it when `region` is `None` (cancel)
/// or too small.
#[tauri::command]
pub fn overlay_finish(
    app: AppHandle,
    state: State<'_, AppState>,
    region: Option<FractionRect>,
) -> IpcResult<()> {
    let shot = state
        .pending_shot
        .lock()
        .map_err(|_| IpcError::new("internal", "overlay lock poisoned"))?
        .take();
    // Close the overlay first and do the heavy work on another thread: this command runs on the
    // thread that draws the windows, so cropping, encoding and saving here would keep the
    // frozen desktop (or a black window) on screen until they finish.
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
    cancel(&app);
    let (Some(shot), Some(region)) = (shot, region) else {
        return Ok(());
    };
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        if let Err(e) = save_region(&app, &state, &shot, region) {
            tracing::warn!(error = %e.message, "screenshot not saved");
        }
    });
    Ok(())
}

fn save_region(
    app: &AppHandle,
    state: &AppState,
    shot: &PendingShot,
    region: FractionRect,
) -> IpcResult<()> {
    let Some(px) = fraction_to_rect(shot.frame.rect, region) else {
        return Ok(());
    };
    let cropped = shot
        .frame
        .crop(px)
        .map_err(|e| IpcError::new("io", e.to_string()))?;
    let file = state.recorder.save_screenshot(&cropped, shot.offset_ms)?;
    tracing::info!(file = %file, "screenshot saved");
    if let Some(s) = state.recorder.tick() {
        emit_state(app, &s);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Rect {
        // Two monitors side by side, the second to the left of the primary.
        Rect::new(-1920, 0, 3840, 1080)
    }

    #[test]
    fn full_selection_covers_the_frame() {
        let r = fraction_to_rect(
            frame(),
            FractionRect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        )
        .unwrap();
        assert_eq!(r, frame());
    }

    #[test]
    fn selection_on_the_left_monitor_has_negative_origin() {
        let r = fraction_to_rect(
            frame(),
            FractionRect {
                x: 0.25,
                y: 0.5,
                w: 0.125,
                h: 0.25,
            },
        )
        .unwrap();
        assert_eq!(r, Rect::new(-960, 540, 480, 270));
    }

    #[test]
    fn selection_is_clamped_to_the_frame() {
        let r = fraction_to_rect(
            frame(),
            FractionRect {
                x: 0.9,
                y: 0.9,
                w: 0.5,
                h: 0.5,
            },
        )
        .unwrap();
        assert_eq!(r.x + r.width as i32, 1920);
        assert_eq!(r.y + r.height as i32, 1080);
    }

    /// Without `connect-src` the CSP falls back to `default-src 'self'` and blocks the IPC
    /// custom protocol. Tauri then answers over `postMessage`, which turns the raw PNG bytes of
    /// `overlay_image` into a plain array of numbers; the overlay then showed a black screen.
    #[test]
    fn csp_lets_the_web_view_reach_the_ipc_protocol() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = conf["app"]["security"]["csp"].as_str().unwrap();
        let connect = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("connect-src"))
            .expect("connect-src is set");
        assert!(connect.split(' ').any(|s| s == "ipc:"), "{connect}");
        assert!(connect.contains("http://ipc.localhost"), "{connect}");
    }

    #[test]
    fn tiny_selection_counts_as_a_click() {
        let f = FractionRect {
            x: 0.5,
            y: 0.5,
            w: 0.0005,
            h: 0.5,
        };
        assert!(fraction_to_rect(frame(), f).is_none());
    }
}
