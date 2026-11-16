//! Region screenshots: capture of the whole virtual desktop, cropping and PNG output.
//!
//! The app freezes the desktop with [`capture_virtual_desktop`], shows it in an overlay, lets
//! the user drag a rectangle and saves the crop with [`write_png`]. Everything except the
//! capture itself is pure logic and builds on every platform.
//!
//! All coordinates are virtual-desktop pixels. On a multi-monitor setup the origin is the
//! top-left corner of the primary monitor, so monitors left of or above it have negative
//! coordinates.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{RecorderError, Result};

/// Number of bytes per pixel of a [`Frame`] (RGBA8).
const BYTES_PER_PIXEL: usize = 4;

/// An axis-aligned rectangle in virtual-desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge (negative for monitors left of the primary one).
    pub x: i32,
    /// Top edge (negative for monitors above the primary one).
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Rect {
    /// Build a rectangle from its origin and size.
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// True when the rectangle covers no pixels.
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Exclusive right edge, widened so that it cannot overflow.
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    /// Exclusive bottom edge, widened so that it cannot overflow.
    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// The overlap of two rectangles. Rectangles that do not overlap give an empty result
    /// (see [`Rect::is_empty`]).
    pub fn intersect(&self, other: Rect) -> Rect {
        let left = i64::from(self.x).max(i64::from(other.x));
        let top = i64::from(self.y).max(i64::from(other.y));
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= left || bottom <= top {
            return Rect::new(
                left.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                0,
                0,
                0,
            );
        }
        // Both edges lie inside the range of one of the inputs, so the casts are lossless.
        Rect::new(
            left as i32,
            top as i32,
            (right - left) as u32,
            (bottom - top) as u32,
        )
    }
}

/// A captured image: RGBA8, row-major, no padding, fully opaque for screen captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Where the image sits on the virtual desktop.
    pub rect: Rect,
    /// Pixel data, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Number of bytes a pixel buffer of this size needs, or `None` on overflow.
fn buffer_len(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(BYTES_PER_PIXEL)
}

impl Frame {
    /// Build a frame, checking that the buffer matches the rectangle.
    pub fn new(rect: Rect, rgba: Vec<u8>) -> Result<Self> {
        let frame = Self { rect, rgba };
        frame.validate()?;
        Ok(frame)
    }

    /// Check that the buffer length matches the rectangle.
    fn validate(&self) -> Result<()> {
        match buffer_len(self.rect.width, self.rect.height) {
            Some(len) if len == self.rgba.len() => Ok(()),
            _ => Err(RecorderError::InvalidArgument(format!(
                "frame buffer has {} bytes, expected {}x{}x4",
                self.rgba.len(),
                self.rect.width,
                self.rect.height
            ))),
        }
    }

    /// Cut `region` (virtual-desktop coordinates) out of this frame. The region is clipped to
    /// the frame; it is an error when nothing is left after clipping.
    pub fn crop(&self, region: Rect) -> Result<Frame> {
        self.validate()?;
        let clipped = self.rect.intersect(region);
        if clipped.is_empty() {
            return Err(RecorderError::InvalidArgument(format!(
                "crop region {}x{} at ({}, {}) is outside the frame",
                region.width, region.height, region.x, region.y
            )));
        }
        // The clipped region lies inside the frame, so every offset below is non-negative and
        // bounded by the frame size.
        let frame_width = self.rect.width as usize;
        let left = (i64::from(clipped.x) - i64::from(self.rect.x)) as usize;
        let top = (i64::from(clipped.y) - i64::from(self.rect.y)) as usize;
        let row_bytes = clipped.width as usize * BYTES_PER_PIXEL;
        let mut rgba = Vec::with_capacity(row_bytes * clipped.height as usize);
        for row in 0..clipped.height as usize {
            let start = ((top + row) * frame_width + left) * BYTES_PER_PIXEL;
            rgba.extend_from_slice(&self.rgba[start..start + row_bytes]);
        }
        Ok(Frame {
            rect: clipped,
            rgba,
        })
    }
}

/// Encode a frame as an 8-bit RGBA PNG.
pub fn encode_png(frame: &Frame) -> Result<Vec<u8>> {
    encode_png_with(frame, png::Compression::default())
}

/// Encode a frame as a lossless PNG as fast as is sensible, for a preview that is shown at
/// once: the file is larger than [`encode_png`] makes it, but a desktop-sized frame takes a
/// fraction of the time.
pub fn encode_png_fast(frame: &Frame) -> Result<Vec<u8>> {
    encode_png_with(frame, png::Compression::Fast)
}

fn encode_png_with(frame: &Frame, compression: png::Compression) -> Result<Vec<u8>> {
    frame.validate()?;
    if frame.rect.is_empty() {
        return Err(RecorderError::InvalidArgument(
            "cannot encode an empty frame".into(),
        ));
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, frame.rect.width, frame.rect.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(compression);
    let mut writer = encoder
        .write_header()
        .map_err(|e| RecorderError::screenshot("write PNG header", e))?;
    writer
        .write_image_data(&frame.rgba)
        .map_err(|e| RecorderError::screenshot("write PNG data", e))?;
    writer
        .finish()
        .map_err(|e| RecorderError::screenshot("finish PNG", e))?;
    Ok(out)
}

/// Encode a frame and write it to `path` atomically: the data goes to a temporary file next
/// to the target, which is then renamed over it, so a crash never leaves a partial image.
pub fn write_png(frame: &Frame, path: &Path) -> Result<()> {
    let bytes = encode_png(frame)?;
    let tmp = temp_path(path)?;
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        if let Err(cleanup) = std::fs::remove_file(&tmp) {
            tracing::debug!(path = %tmp.display(), error = %cleanup, "temporary screenshot not removed");
        }
        return Err(e.into());
    }
    tracing::debug!(path = %path.display(), bytes = bytes.len(), "screenshot written");
    Ok(())
}

/// Sibling path used while a file is being written.
fn temp_path(path: &Path) -> Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        RecorderError::InvalidArgument(format!("not a file path: {}", path.display()))
    })?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(".tmp");
    Ok(path.with_file_name(tmp_name))
}

/// File name of a screenshot: a four-digit sequence number followed by the offset from the
/// recording start as `HH-MM-SS`, for example `0001-00-12-41.png`. Sequence numbers above 9999
/// and hours above 99 simply get more digits.
pub fn screenshot_file_name(seq: u32, offset_ms: u64) -> String {
    let total_seconds = offset_ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds / 60) % 60;
    let seconds = total_seconds % 60;
    format!("{seq:04}-{hours:02}-{minutes:02}-{seconds:02}.png")
}

#[cfg(windows)]
pub use win::{capture_virtual_desktop, list_monitors, MonitorInfo};

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
        GetDC, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFO, SRCCOPY,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, MONITORINFOF_PRIMARY, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    use super::{buffer_len, Frame, Rect};
    use crate::error::{RecorderError, Result};

    /// One display attached to the virtual desktop.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct MonitorInfo {
        /// Position and size in virtual-desktop pixels.
        pub rect: Rect,
        /// Effective DPI divided by 96 (1.0 = 100 %, 1.5 = 150 %).
        pub scale_factor: f64,
        /// True for the primary monitor.
        pub primary: bool,
    }

    fn rect_from_win(r: &RECT) -> Rect {
        Rect::new(
            r.left,
            r.top,
            u32::try_from(i64::from(r.right) - i64::from(r.left)).unwrap_or(0),
            u32::try_from(i64::from(r.bottom) - i64::from(r.top)).unwrap_or(0),
        )
    }

    /// Bounds of the virtual desktop (all monitors together).
    fn virtual_screen() -> Rect {
        // SAFETY: `GetSystemMetrics` has no preconditions.
        let (x, y, w, h) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        Rect::new(
            x,
            y,
            u32::try_from(w).unwrap_or(0),
            u32::try_from(h).unwrap_or(0),
        )
    }

    /// Screen device context, released on drop.
    struct ScreenDc(HDC);

    impl ScreenDc {
        fn acquire() -> Result<Self> {
            // SAFETY: a null window handle asks for the device context of the whole screen.
            let dc = unsafe { GetDC(None) };
            if dc.is_invalid() {
                return Err(RecorderError::screenshot(
                    "get screen device context",
                    "GetDC returned null",
                ));
            }
            Ok(Self(dc))
        }
    }

    impl Drop for ScreenDc {
        fn drop(&mut self) {
            // SAFETY: the context came from `GetDC(None)` and is released exactly once.
            unsafe { ReleaseDC(None, self.0) };
        }
    }

    /// Memory device context, deleted on drop.
    struct MemDc(HDC);

    impl Drop for MemDc {
        fn drop(&mut self) {
            // SAFETY: the context came from `CreateCompatibleDC` and is deleted exactly once.
            let _ = unsafe { DeleteDC(self.0) };
        }
    }

    /// DIB section handle, deleted on drop.
    struct Bitmap(HBITMAP);

    impl Drop for Bitmap {
        fn drop(&mut self) {
            // SAFETY: the bitmap came from `CreateDIBSection`, is no longer selected into a
            // context at this point (fields drop after the guard that restores the selection)
            // and is deleted exactly once.
            let _ = unsafe { DeleteObject(HGDIOBJ(self.0 .0)) };
        }
    }

    /// Restores the previously selected GDI object when dropped.
    struct Selection {
        dc: HDC,
        previous: HGDIOBJ,
    }

    impl Drop for Selection {
        fn drop(&mut self) {
            // SAFETY: `previous` is the object that was selected before ours into this context.
            unsafe { SelectObject(self.dc, self.previous) };
        }
    }

    /// Capture every monitor as one RGBA frame positioned at the virtual-desktop origin.
    ///
    /// The result is in physical pixels only when the process is per-monitor DPI aware.
    /// This function does not change the DPI awareness: the application manifest (or Tauri,
    /// which declares it for its windows) must do that. Without it Windows virtualizes
    /// coordinates and the frame is scaled down on high-DPI setups.
    ///
    /// The capture includes layered and topmost windows (`CAPTUREBLT`). The alpha channel is
    /// forced to 255 because GDI leaves it undefined.
    pub fn capture_virtual_desktop() -> Result<Frame> {
        let rect = virtual_screen();
        if rect.is_empty() {
            return Err(RecorderError::screenshot(
                "capture virtual desktop",
                "the virtual screen has no size (no desktop available)",
            ));
        }
        let len = buffer_len(rect.width, rect.height).ok_or_else(|| {
            RecorderError::screenshot("capture virtual desktop", "frame size overflows")
        })?;
        let width = i32::try_from(rect.width)
            .map_err(|e| RecorderError::screenshot("capture virtual desktop", e))?;
        let height = i32::try_from(rect.height)
            .map_err(|e| RecorderError::screenshot("capture virtual desktop", e))?;

        let screen = ScreenDc::acquire()?;
        // SAFETY: `screen.0` is a valid device context.
        let mem_raw = unsafe { CreateCompatibleDC(Some(screen.0)) };
        if mem_raw.is_invalid() {
            return Err(RecorderError::screenshot(
                "create memory device context",
                "CreateCompatibleDC returned null",
            ));
        }
        let mem = MemDc(mem_raw);

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative height: top-down rows, so no vertical flip is needed.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: `info` describes a 32-bit top-down DIB and `bits` is a live out pointer.
        let bitmap =
            unsafe { CreateDIBSection(Some(screen.0), &info, DIB_RGB_COLORS, &mut bits, None, 0) }
                .map_err(|e| RecorderError::screenshot("create DIB section", e))?;
        let bitmap = Bitmap(bitmap);
        if bits.is_null() {
            return Err(RecorderError::screenshot(
                "create DIB section",
                "no pixel buffer returned",
            ));
        }

        // SAFETY: both handles are valid; the previous object is restored by `Selection`.
        let previous = unsafe { SelectObject(mem.0, HGDIOBJ(bitmap.0 .0)) };
        if previous.is_invalid() {
            return Err(RecorderError::screenshot(
                "select bitmap",
                "SelectObject failed",
            ));
        }
        let selection = Selection {
            dc: mem.0,
            previous,
        };

        // SAFETY: the destination bitmap is selected into `mem` and is `width` x `height`;
        // the source is the screen context.
        unsafe {
            BitBlt(
                mem.0,
                0,
                0,
                width,
                height,
                Some(screen.0),
                rect.x,
                rect.y,
                SRCCOPY | CAPTUREBLT,
            )
        }
        .map_err(|e| RecorderError::screenshot("copy screen contents", e))?;

        // SAFETY: the DIB section owns `len` bytes (32 bpp, no row padding) at `bits` and
        // stays alive until `bitmap` drops below.
        let pixels = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), len) };
        let mut rgba = Vec::with_capacity(len);
        for px in pixels.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        // Deselect before the bitmap and the context are deleted.
        drop(selection);
        drop(bitmap);
        drop(mem);
        drop(screen);
        tracing::debug!(
            x = rect.x,
            y = rect.y,
            width = rect.width,
            height = rect.height,
            "virtual desktop captured"
        );
        Frame::new(rect, rgba)
    }

    unsafe extern "system" fn collect_monitor(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> windows::core::BOOL {
        // SAFETY: `data` is the address of the `Vec` owned by `list_monitors`, which outlives
        // the enumeration and is not accessed elsewhere while it runs.
        let out = unsafe { &mut *(data.0 as *mut Vec<MonitorInfo>) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `monitor` was handed to us by the enumeration and `info.cbSize` is set.
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
            // SAFETY: the out pointers refer to live locals.
            let scale_factor = match unsafe {
                GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)
            } {
                Ok(()) if dpi_x > 0 => f64::from(dpi_x) / 96.0,
                _ => 1.0,
            };
            out.push(MonitorInfo {
                rect: rect_from_win(&info.rcMonitor),
                scale_factor,
                primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        true.into()
    }

    /// List all monitors with their bounds, scale factor and primary flag.
    ///
    /// Bounds are physical pixels and the scale factor is the effective DPI divided by 96 only
    /// when the process is per-monitor DPI aware; the application manifest (or Tauri) sets
    /// that, this function does not.
    pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
        let mut monitors: Vec<MonitorInfo> = Vec::new();
        // SAFETY: the callback only dereferences the pointer passed here, and the vector
        // outlives the synchronous enumeration.
        let ok = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(collect_monitor),
                LPARAM(std::ptr::addr_of_mut!(monitors) as isize),
            )
        };
        if !ok.as_bool() {
            return Err(RecorderError::screenshot(
                "enumerate monitors",
                "EnumDisplayMonitors failed",
            ));
        }
        Ok(monitors)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn capture_matches_virtual_screen_size() {
            let expected = virtual_screen();
            if expected.is_empty() {
                // No desktop available (for example a service session): nothing to test.
                return;
            }
            let Ok(frame) = capture_virtual_desktop() else {
                // Locked or disconnected desktops refuse the capture; treat as skipped.
                return;
            };
            assert_eq!(frame.rect, expected);
            assert_eq!(
                frame.rgba.len(),
                frame.rect.width as usize * frame.rect.height as usize * 4
            );
        }

        #[test]
        fn monitors_cover_the_virtual_screen() {
            let virtual_rect = virtual_screen();
            if virtual_rect.is_empty() {
                return;
            }
            let Ok(monitors) = list_monitors() else {
                return;
            };
            assert!(!monitors.is_empty());
            assert!(monitors.iter().any(|m| m.primary));
            for m in &monitors {
                assert!(m.scale_factor > 0.0);
                assert_eq!(virtual_rect.intersect(m.rect), m.rect);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    /// A frame whose pixel at (col, row) encodes its own position, so crops are checkable.
    fn gradient(rect: Rect) -> Frame {
        let mut rgba = Vec::new();
        for row in 0..rect.height {
            for col in 0..rect.width {
                rgba.extend_from_slice(&[col as u8, row as u8, (col + row) as u8, 255]);
            }
        }
        Frame::new(rect, rgba).unwrap()
    }

    fn pixel(frame: &Frame, col: u32, row: u32) -> [u8; 4] {
        let i = ((row * frame.rect.width + col) * 4) as usize;
        frame.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn rect_intersect_and_empty() {
        let a = Rect::new(0, 0, 10, 10);
        assert_eq!(a.intersect(Rect::new(5, 5, 10, 10)), Rect::new(5, 5, 5, 5));
        assert!(a.intersect(Rect::new(10, 0, 5, 5)).is_empty());
        assert!(a.intersect(Rect::new(-9, -9, 5, 5)).is_empty());
        assert!(Rect::new(3, 3, 0, 4).is_empty());
        let huge = Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX);
        assert_eq!(a.intersect(huge), a);
    }

    #[test]
    fn crop_inside() {
        let frame = gradient(Rect::new(0, 0, 8, 6));
        let crop = frame.crop(Rect::new(2, 1, 3, 2)).unwrap();
        assert_eq!(crop.rect, Rect::new(2, 1, 3, 2));
        assert_eq!(crop.rgba.len(), 3 * 2 * 4);
        assert_eq!(pixel(&crop, 0, 0), pixel(&frame, 2, 1));
        assert_eq!(pixel(&crop, 2, 1), pixel(&frame, 4, 2));
    }

    #[test]
    fn crop_clipped_at_edges() {
        let frame = gradient(Rect::new(0, 0, 8, 6));
        let crop = frame.crop(Rect::new(6, 4, 10, 10)).unwrap();
        assert_eq!(crop.rect, Rect::new(6, 4, 2, 2));
        assert_eq!(pixel(&crop, 1, 1), pixel(&frame, 7, 5));
        let crop = frame.crop(Rect::new(-3, -3, 5, 5)).unwrap();
        assert_eq!(crop.rect, Rect::new(0, 0, 2, 2));
        assert_eq!(pixel(&crop, 0, 0), pixel(&frame, 0, 0));
    }

    #[test]
    fn crop_with_negative_origin() {
        // A monitor to the left of and above the primary one.
        let frame = gradient(Rect::new(-4, -3, 8, 6));
        let crop = frame.crop(Rect::new(-2, -1, 4, 3)).unwrap();
        assert_eq!(crop.rect, Rect::new(-2, -1, 4, 3));
        assert_eq!(pixel(&crop, 0, 0), pixel(&frame, 2, 2));
        assert_eq!(pixel(&crop, 3, 2), pixel(&frame, 5, 4));
        let clipped = frame.crop(Rect::new(-10, -10, 8, 8)).unwrap();
        assert_eq!(clipped.rect, Rect::new(-4, -3, 2, 1));
    }

    #[test]
    fn empty_crop_is_an_error() {
        let frame = gradient(Rect::new(0, 0, 4, 4));
        assert!(frame.crop(Rect::new(1, 1, 0, 3)).is_err());
        assert!(frame.crop(Rect::new(4, 0, 2, 2)).is_err());
        assert!(frame.crop(Rect::new(-5, -5, 2, 2)).is_err());
    }

    #[test]
    fn frame_with_wrong_buffer_is_rejected() {
        assert!(Frame::new(Rect::new(0, 0, 2, 2), vec![0; 15]).is_err());
        let bad = Frame {
            rect: Rect::new(0, 0, 2, 2),
            rgba: vec![0; 3],
        };
        assert!(bad.crop(Rect::new(0, 0, 1, 1)).is_err());
        assert!(encode_png(&bad).is_err());
    }

    fn decode(bytes: &[u8]) -> (png::OutputInfo, Vec<u8>) {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info, buf)
    }

    #[test]
    fn png_round_trip() {
        let frame = gradient(Rect::new(-5, 7, 13, 9));
        let bytes = encode_png(&frame).unwrap();
        let (info, pixels) = decode(&bytes);
        assert_eq!((info.width, info.height), (13, 9));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        assert_eq!(pixels, frame.rgba);
    }

    #[test]
    fn fast_png_is_lossless() {
        let frame = gradient(Rect::new(3, -2, 64, 48));
        let (info, pixels) = decode(&encode_png_fast(&frame).unwrap());
        assert_eq!((info.width, info.height), (64, 48));
        assert_eq!(pixels, frame.rgba);
    }

    #[test]
    fn write_png_is_atomic_and_overwrites() {
        let dir = TempDir::new("screenshot");
        let path = dir.path().join("0001-00-00-01.png");
        write_png(&gradient(Rect::new(0, 0, 4, 4)), &path).unwrap();
        let second = gradient(Rect::new(0, 0, 6, 3));
        write_png(&second, &path).unwrap();
        let (info, pixels) = decode(&std::fs::read(&path).unwrap());
        assert_eq!((info.width, info.height), (6, 3));
        assert_eq!(pixels, second.rgba);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "temporary file left behind: {names:?}");
    }

    #[test]
    fn encoding_an_empty_frame_fails() {
        let frame = Frame::new(Rect::new(0, 0, 0, 5), Vec::new()).unwrap();
        assert!(encode_png(&frame).is_err());
    }

    #[test]
    fn file_name_format() {
        assert_eq!(
            screenshot_file_name(1, 12 * 60_000 + 41_000),
            "0001-00-12-41.png"
        );
        assert_eq!(screenshot_file_name(0, 0), "0000-00-00-00.png");
        assert_eq!(screenshot_file_name(42, 999), "0042-00-00-00.png");
        assert_eq!(screenshot_file_name(7, 1000), "0007-00-00-01.png");
        // More than one hour.
        assert_eq!(
            screenshot_file_name(123, (2 * 3600 + 5 * 60 + 9) * 1000 + 500),
            "0123-02-05-09.png"
        );
        // More than 99 hours widens the hour field only.
        assert_eq!(
            screenshot_file_name(10_000, 100 * 3_600_000),
            "10000-100-00-00.png"
        );
    }
}
