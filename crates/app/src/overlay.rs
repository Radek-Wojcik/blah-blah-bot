//! Region screenshot overlay.
//!
//! The desktop is captured first and shown frozen in a borderless window that covers every
//! monitor. The user drags a rectangle; the UI reports it as fractions of the shown image,
//! which keeps the mapping independent of monitor scaling, then the chosen region is
//! cropped from the frozen capture and saved with the offset of the moment the hotkey was
//! pressed.
//!
//! The window is created hidden and only shown once its page has fetched and decoded the
//! frozen desktop (`overlay_ready`), so the live desktop stays on screen until the frozen one
//! can replace it: a covering window shown earlier is a black screen for as long as the
//! page needs to load. The preview PNG is encoded on another thread while the window and its
//! page start up.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use recorder::screenshot::{
    capture_virtual_desktop, encode_png_preview, overlay_window_rect, Frame, Rect,
};
use serde::Deserialize;
use tauri::ipc::Response;
use tauri::window::Color;
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl, WebviewWindowBuilder,
};

use crate::commands::emit_state;
use crate::ipc::{IpcError, IpcResult};
use crate::state::{AppState, PendingShot};

const LABEL: &str = "overlay";
/// Selections smaller than this (in pixels) in either direction are treated as a click.
const MIN_SIDE: u32 = 4;
/// The overlay is shown after this long even when its page never reports that it is ready, so
/// a page that failed to load still gets the window with its error hint (and an Escape key).
const SHOW_TIMEOUT: Duration = Duration::from_secs(4);

/// Why a capture has no preview: the IPC error code and message the overlay page receives.
type PreviewError = (&'static str, String);

/// The preview PNG of a capture. Whoever asks first encodes it, the others wait for the result.
///
/// A capture without any visible content (all black or fully transparent) has no preview: it is
/// refused with the code `blank_capture` so the overlay can say so instead of presenting a black
/// screen.
#[derive(Default)]
pub struct Preview(OnceLock<Result<Vec<u8>, PreviewError>>);

impl Preview {
    pub fn get_or_encode(&self, frame: &Frame) -> Result<&[u8], &PreviewError> {
        self.0
            .get_or_init(|| {
                let started = Instant::now();
                let stats = frame.stats();
                if frame.rgba.is_empty() || stats.is_blank() {
                    tracing::warn!(?stats, rect = ?frame.rect, "the desktop capture is blank");
                    return Err((
                        "blank_capture",
                        "The screen capture came back empty (all black). The screenshot was cancelled."
                            .to_owned(),
                    ));
                }
                let png = encode_png_preview(frame).map_err(|e| ("io", e.to_string()));
                tracing::debug!(
                    ?stats,
                    ms = started.elapsed().as_millis() as u64,
                    bytes = png.as_ref().map_or(0, Vec::len),
                    "overlay preview encoded"
                );
                png
            })
            .as_ref()
            .map(Vec::as_slice)
    }
}

/// Decides who may show the overlay window: its page when it is ready, or the timeout, but only
/// one of them, and the timeout only for the overlay it was started for.
struct ShowGate {
    epoch: AtomicU64,
    shown: AtomicBool,
}

impl ShowGate {
    const fn new() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            shown: AtomicBool::new(false),
        }
    }

    /// A new overlay starts, hidden. Returns its id.
    fn arm(&self) -> u64 {
        self.shown.store(false, Ordering::SeqCst);
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Whether the caller may show the window now. `id` is `Some` for the timeout of overlay
    /// `id`, which has no say once another overlay was started.
    fn claim(&self, id: Option<u64>) -> bool {
        if id.is_some_and(|id| id != self.epoch.load(Ordering::SeqCst)) {
            return false;
        }
        !self.shown.swap(true, Ordering::SeqCst)
    }
}

static GATE: ShowGate = ShowGate::new();

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
    let frame =
        Arc::new(capture_virtual_desktop().map_err(|e| IpcError::new("io", e.to_string()))?);
    let rect = overlay_window_rect(frame.rect);
    let preview = Arc::new(Preview::default());
    {
        // Encode while the window and its page start up.
        let (frame, preview) = (frame.clone(), preview.clone());
        std::thread::spawn(move || {
            let _ = preview.get_or_encode(&frame);
        });
    }
    *state
        .pending_shot
        .lock()
        .map_err(|_| IpcError::new("internal", "overlay lock poisoned"))? = Some(PendingShot {
        frame,
        preview,
        offset_ms,
    });
    let id = GATE.arm();

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
    .background_color(Color(0, 0, 0, 255))
    .visible(false)
    .build()
    .map_err(|e| IpcError::new("internal", e.to_string()))?;
    let placed = window
        .set_position(PhysicalPosition::new(rect.x, rect.y))
        .and_then(|()| window.set_size(PhysicalSize::new(rect.width, rect.height)));
    if let Err(e) = placed {
        cancel(app);
        return Err(IpcError::new("internal", e.to_string()));
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SHOW_TIMEOUT);
        if GATE.claim(Some(id)) {
            tracing::warn!("overlay page not ready in time, showing the window anyway");
            if let Err(e) = show(&app) {
                tracing::warn!(error = %e, "overlay window not shown");
            }
        }
    });
    Ok(())
}

/// Show the overlay window and bring it to the front.
///
/// The window is created on the primary monitor and moved afterwards; with monitors of
/// different scale factors Windows may adjust the size while the window changes monitor, so the
/// result is read back and applied once more when it differs.
fn show(app: &AppHandle) -> tauri::Result<()> {
    let Some(w) = app.get_webview_window(LABEL) else {
        return Ok(());
    };
    w.show()?;
    let rect = app.try_state::<AppState>().and_then(|state| {
        let pending = state.pending_shot.lock().ok()?;
        pending.as_ref().map(|s| overlay_window_rect(s.frame.rect))
    });
    if let Some(rect) = rect {
        let placed =
            |w: &tauri::WebviewWindow| -> tauri::Result<bool> {
                let (pos, size) = (w.outer_position()?, w.outer_size()?);
                Ok((pos.x, pos.y, size.width, size.height)
                    == (rect.x, rect.y, rect.width, rect.height))
            };
        if !placed(&w)? {
            tracing::warn!(
                ?rect,
                "overlay window was moved or resized by the system; placing it again"
            );
            w.set_position(PhysicalPosition::new(rect.x, rect.y))?;
            w.set_size(PhysicalSize::new(rect.width, rect.height))?;
            if !placed(&w)? {
                tracing::warn!(?rect, "overlay window does not cover the desktop exactly");
            }
        }
    }
    w.set_focus()
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

/// The frozen capture as PNG bytes, for the overlay page. Async so that waiting for the
/// encoder never blocks the thread that draws the windows. A blank capture is refused with the
/// code `blank_capture`.
#[tauri::command]
pub async fn overlay_image(state: State<'_, AppState>) -> IpcResult<Response> {
    let (frame, preview) = {
        let pending = state
            .pending_shot
            .lock()
            .map_err(|_| IpcError::new("internal", "overlay lock poisoned"))?;
        let shot = pending
            .as_ref()
            .ok_or_else(|| IpcError::new("internal", "no screenshot in progress"))?;
        (shot.frame.clone(), shot.preview.clone())
    };
    let png = tauri::async_runtime::spawn_blocking(move || {
        preview
            .get_or_encode(&frame)
            .map(<[u8]>::to_vec)
            .map_err(Clone::clone)
    })
    .await
    .map_err(|e| IpcError::new("internal", e.to_string()))?
    .map_err(|(code, message)| IpcError::new(code, message))?;
    Ok(Response::new(png))
}

/// The overlay page has the frozen desktop decoded (or gave up): show the window.
#[tauri::command]
pub fn overlay_ready(app: AppHandle) -> IpcResult<()> {
    if GATE.claim(None) {
        show(&app).map_err(|e| IpcError::new("internal", e.to_string()))?;
    }
    Ok(())
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
    fn the_overlay_is_shown_once() {
        let gate = ShowGate::new();
        let id = gate.arm();
        assert!(gate.claim(None), "the page is ready first");
        assert!(!gate.claim(Some(id)), "the timeout finds it shown");
        assert!(!gate.claim(None), "a second ready changes nothing");
    }

    #[test]
    fn the_timeout_shows_a_page_that_never_got_ready() {
        let gate = ShowGate::new();
        let id = gate.arm();
        assert!(gate.claim(Some(id)));
        assert!(!gate.claim(None), "a late ready finds it shown");
    }

    #[test]
    fn the_timeout_of_an_earlier_overlay_does_nothing() {
        let gate = ShowGate::new();
        let first = gate.arm();
        let second = gate.arm();
        assert!(!gate.claim(Some(first)));
        assert!(gate.claim(Some(second)));
    }

    #[test]
    fn the_preview_is_encoded_once_and_reused() {
        let frame = Frame::new(Rect::new(0, 0, 8, 4), vec![200; 8 * 4 * 4]).unwrap();
        let preview = Preview::default();
        let first = preview.get_or_encode(&frame).unwrap();
        assert_eq!(&first[1..4], b"PNG");
        let again = preview.get_or_encode(&frame).unwrap();
        assert!(std::ptr::eq(first, again));
    }

    #[test]
    fn a_blank_capture_has_no_preview_and_says_so() {
        let preview = Preview::default();
        for rgba in [[0, 0, 0, 255], [200, 100, 50, 0]] {
            let frame = Frame::new(Rect::new(0, 0, 4, 2), rgba.repeat(8)).unwrap();
            let fresh = Preview::default();
            let (code, message) = fresh.get_or_encode(&frame).unwrap_err();
            assert_eq!(*code, "blank_capture");
            assert!(message.contains("empty"), "{message}");
        }
        let black = Frame::new(Rect::new(0, 0, 4, 2), [0, 0, 0, 255].repeat(8)).unwrap();
        assert_eq!(
            preview.get_or_encode(&black).unwrap_err().0,
            "blank_capture"
        );
        assert_eq!(
            preview.get_or_encode(&black).unwrap_err().0,
            "blank_capture"
        );
    }

    #[test]
    fn a_frame_that_cannot_be_encoded_reports_the_error_every_time() {
        let bad = Frame {
            rect: Rect::new(0, 0, 2, 2),
            rgba: vec![0; 3],
        };
        let preview = Preview::default();
        assert!(preview.get_or_encode(&bad).is_err());
        assert!(preview.get_or_encode(&bad).is_err());
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
