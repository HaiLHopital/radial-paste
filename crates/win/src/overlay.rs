//! The radial menu on screen: a layered window that never takes the focus, drawn with
//! `tiny-skia` and shown with `UpdateLayeredWindow`.
//!
//! The window is created on the input thread, on the first `show`. It is `WS_EX_NOACTIVATE`,
//! is shown with `SW_SHOWNOACTIVATE`, and answers `WM_MOUSEACTIVATE` with `MA_NOACTIVATE`, so
//! the app being pasted into keeps the focus. The centre is moved inward so that the ring stays
//! inside the monitor's work area ([`place`]), and the monitor's DPI gives the `Placement`'s
//! scale, which the core divides cursor offsets by.

use std::f32::consts::PI;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::last_error;
use ab_glyph::{Font, FontRef, GlyphId, OutlineCurve, PxScale, PxScaleFactor, ScaleFont};
use radial_app::ports::{MenuPresenter, Placement};
use radial_domain::{Layout, Rect, Snippet, Vec2, place};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Shader, Stroke, Transform};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
    GetMonitorInfoW, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, HWND_TOPMOST, MA_NOACTIVATE, RegisterClassW, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow, ULW_ALPHA,
    UpdateLayeredWindow, WM_MOUSEACTIVATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Error as WinError, HSTRING, PCWSTR};

/// The system UI font. Without it the ring is drawn without labels (open decision 4).
const FONT_PATH: &str = "C:\\Windows\\Fonts\\segoeui.ttf";
/// The class and window title of the overlay.
const CLASS_NAME: &str = "radial-paste overlay";
/// The height of the labels, in logical pixels.
const LABEL_PX: f32 = 14.0;

#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    #[error("could not register the overlay window class: {0}")]
    Class(WinError),
    #[error("could not create the overlay window: {0}")]
    Window(#[from] WinError),
    #[error("could not allocate the drawing surface")]
    Pixmap,
}

/// One shown menu: everything [`Overlay::highlight`] needs to redraw it.
#[derive(Clone, Debug)]
struct Frame {
    centre: Vec2,
    scale: f64,
    layout: Layout,
    snippets: Vec<Snippet>,
    highlighted: Option<usize>,
}

/// The state of loading [`FONT_PATH`]: untried, ready to be parsed per draw, or missing, which
/// means the ring is drawn without labels and the file is not probed again.
#[derive(Debug, Default)]
enum Fonts {
    #[default]
    Untried,
    Ready(Vec<u8>),
    Missing,
}

#[derive(Debug, Default)]
pub struct Overlay {
    window: Option<HWND>,
    fonts: Fonts,
    frame: Option<Frame>,
}

// SAFETY: the window handle is only created on the input thread, in `show`, and every use of
// `Overlay` happens there: `input::spawn` moves the overlay to that thread while the handle is
// still `None`.
unsafe impl Send for Overlay {}

extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    // SAFETY: the window keeps no state, so the default procedure loses nothing.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Registers the window class once. Called on the input thread, the only thread that creates
/// the window.
fn register_class() -> Result<(), OverlayError> {
    static REGISTERED: AtomicBool = AtomicBool::new(false);
    if REGISTERED.load(Ordering::Acquire) {
        return Ok(());
    }
    // SAFETY: `None` asks for this executable's module handle, which always exists.
    let module: HMODULE = unsafe { GetModuleHandleW(None) }?;
    // The class name buffer must outlive the call, which only reads it: the `HSTRING` is bound
    // to a local, not chained into the pointer, so that it cannot be dropped early.
    let class_name = HSTRING::from(CLASS_NAME);
    let class = WNDCLASSW {
        lpfnWndProc: Some(overlay_proc),
        hInstance: HINSTANCE(module.0),
        lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
        ..Default::default()
    };
    // SAFETY: `class` and the class name it points at live through the call, which only reads
    // them.
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(OverlayError::Class(last_error()));
    }
    REGISTERED.store(true, Ordering::Release);
    Ok(())
}

fn create_window() -> Result<HWND, OverlayError> {
    register_class()?;
    // As in `register_class`: the buffer must outlive the call below.
    let class_name = HSTRING::from(CLASS_NAME);
    let name = PCWSTR::from_raw(class_name.as_ptr());
    // SAFETY: the class is registered above; the zero size is fine because `UpdateLayeredWindow`
    // sizes and positions the window on every `show`; with no parent this is a top-level window.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            name,
            name,
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )?
    };
    Ok(hwnd)
}

/// The monitor the cursor is on. `MonitorFromPoint` cannot fail: it returns the nearest one.
fn monitor_at(cursor: Vec2) -> HMONITOR {
    // SAFETY: reads a plain point and never returns null with `MONITOR_DEFAULTTONEAREST`.
    unsafe {
        MonitorFromPoint(
            POINT {
                x: cursor.x as i32,
                y: cursor.y as i32,
            },
            MONITOR_DEFAULTTONEAREST,
        )
    }
}

/// The monitor's work area in physical pixels: the whole screen minus the taskbar.
fn work_area(monitor: HMONITOR) -> Result<Rect, OverlayError> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a valid out parameter with `cbSize` set.
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        Ok(Rect {
            left: f64::from(info.rcWork.left),
            top: f64::from(info.rcWork.top),
            right: f64::from(info.rcWork.right),
            bottom: f64::from(info.rcWork.bottom),
        })
    } else {
        Err(OverlayError::Window(last_error()))
    }
}

/// The monitor's DPI over 96: the number the core divides cursor offsets by.
fn monitor_scale(monitor: HMONITOR) -> Result<f64, OverlayError> {
    let mut dpi_x = 96;
    let mut dpi_y = 96;
    // SAFETY: both out parameters are valid; the call can only fail with an invalid monitor.
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)? };
    Ok(f64::from(dpi_x) / 96.0)
}

impl Overlay {
    fn window(&mut self) -> Result<HWND, OverlayError> {
        if let Some(hwnd) = self.window {
            return Ok(hwnd);
        }
        let hwnd = create_window()?;
        self.window = Some(hwnd);
        Ok(hwnd)
    }

    /// The bytes of [`FONT_PATH`], loaded once. `None` means no labels.
    fn fonts(&mut self) -> Option<&[u8]> {
        if matches!(self.fonts, Fonts::Untried) {
            self.fonts = match std::fs::read(FONT_PATH) {
                Ok(data) => Fonts::Ready(data),
                Err(e) => {
                    log::warn!("could not load the overlay font; drawing without labels: {e}");
                    Fonts::Missing
                }
            };
        }
        match &self.fonts {
            Fonts::Ready(data) => Some(data),
            _ => None,
        }
    }

    fn show_frame(&mut self, frame: Frame) -> Result<(), OverlayError> {
        let hwnd = self.window()?;
        let fonts = self.fonts();
        // Draw first: if it fails, no stale menu is left on screen.
        draw(hwnd, &frame, fonts)?;
        self.frame = Some(frame);
        // SAFETY: the window exists; both calls keep it from taking the focus.
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        if let Err(e) = unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        } {
            log::warn!("could not raise the menu: {e}");
        }
        Ok(())
    }
}

impl MenuPresenter for Overlay {
    fn show(&mut self, cursor: Vec2, snippets: &[Snippet], layout: Layout) -> Placement {
        let monitor = monitor_at(cursor);
        let area = match work_area(monitor) {
            Ok(area) => area,
            Err(e) => {
                log::error!("could not query the monitor; not showing the menu: {e}");
                self.frame = None;
                return Placement {
                    centre: cursor,
                    scale: 1.0,
                };
            }
        };
        let scale = match monitor_scale(monitor) {
            Ok(scale) => scale,
            Err(e) => {
                log::warn!("could not query the monitor's DPI; assuming 100%: {e}");
                1.0
            }
        };
        let centre = place(cursor, area, layout.radius * scale);
        let frame = Frame {
            centre,
            scale,
            layout,
            snippets: snippets.to_vec(),
            highlighted: None,
        };
        log::debug!(
            "showing {} wedges at ({}, {}), scale {scale}",
            frame.snippets.len(),
            centre.x,
            centre.y
        );
        match self.show_frame(frame) {
            Ok(()) => Placement { centre, scale },
            Err(e) => {
                log::error!("could not show the menu: {e}");
                self.frame = None;
                Placement {
                    centre: cursor,
                    scale,
                }
            }
        }
    }

    fn highlight(&mut self, wedge: Option<usize>) {
        log::debug!("highlighting {wedge:?}");
        let Some(mut frame) = self.frame.clone() else {
            return;
        };
        if frame.highlighted == wedge {
            return;
        }
        frame.highlighted = wedge;
        let Some(hwnd) = self.window else {
            return;
        };
        let fonts = self.fonts();
        match draw(hwnd, &frame, fonts) {
            Ok(()) => self.frame = Some(frame),
            Err(e) => {
                log::error!("could not redraw the menu: {e}");
                self.frame = None;
            }
        }
    }

    fn hide(&mut self) {
        log::debug!("hiding the menu");
        self.frame = None;
        if let Some(hwnd) = self.window {
            // SAFETY: the window exists; `SW_HIDE` cannot take the focus from the target app.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        }
    }
}

/// Draws `frame` and puts it on screen through `UpdateLayeredWindow`.
fn draw(hwnd: HWND, frame: &Frame, fonts: Option<&[u8]>) -> Result<(), OverlayError> {
    let scale = frame.scale as f32;
    let count = frame.snippets.len();
    let rim = frame.layout.radius as f32 * scale;
    if rim <= 1.0 || count == 0 {
        return Ok(()); // Nothing sensible to draw; config validation should prevent this.
    }
    let inner = (frame.layout.dead_zone as f32 * scale).clamp(0.0, rim - 1.0);
    let line_width = 1.0f32.max(scale);
    let pad = 2.0 * scale;
    let size = (((rim + pad) * 2.0).ceil() as u32).max(1);
    let Some(mut pixmap) = Pixmap::new(size, size) else {
        return Err(OverlayError::Pixmap);
    };
    let centre = (size as f32 / 2.0, size as f32 / 2.0);

    fill_circle(&mut pixmap, centre, rim, Color::from_rgba8(24, 24, 24, 200));
    for index in 0..count {
        let (from, to) = wedge_angles(index, count);
        let color = if frame.highlighted == Some(index) {
            Color::from_rgba8(255, 255, 255, 80)
        } else {
            Color::from_rgba8(255, 255, 255, 26)
        };
        if let Some(path) = wedge_path(centre, inner, rim, from, to) {
            fill(&mut pixmap, &path, color);
        }
    }
    if count > 1 {
        for index in 0..count {
            let (from, _) = wedge_angles(index, count);
            if let Some(path) =
                line_path(point_on(centre, inner, from), point_on(centre, rim, from))
            {
                stroke(
                    &mut pixmap,
                    &path,
                    Color::from_rgba8(255, 255, 255, 90),
                    line_width,
                );
            }
        }
    }
    if let Some(path) = circle_path(centre, inner) {
        stroke(
            &mut pixmap,
            &path,
            Color::from_rgba8(255, 255, 255, 110),
            line_width,
        );
    }
    if let Some(path) = circle_path(centre, rim) {
        stroke(
            &mut pixmap,
            &path,
            Color::from_rgba8(255, 255, 255, 140),
            line_width,
        );
    }
    if let Some(data) = fonts {
        draw_labels(&mut pixmap, data, frame, centre, scale);
    }

    present(hwnd, &pixmap, frame.centre)
}

/// The wedge `index` of `count`, as clock angles in degrees: wedge 0 is centred at 12 o'clock
/// and the rest follow clockwise, matching `Layout::pick`.
fn wedge_angles(index: usize, count: usize) -> (f32, f32) {
    let step = 360.0 / count as f32;
    let centre_deg = -90.0 + index as f32 * step;
    (centre_deg - step / 2.0, centre_deg + step / 2.0)
}

fn point_on(centre: (f32, f32), radius: f32, degrees: f32) -> (f32, f32) {
    let radians = degrees.to_radians();
    (
        centre.0 + radius * radians.cos(),
        centre.1 + radius * radians.sin(),
    )
}

/// Appends a circular arc. Its first point either starts the path or connects to the path so
/// far, depending on `move_first`.
fn arc_into(
    builder: &mut PathBuilder,
    centre: (f32, f32),
    radius: f32,
    from: f32,
    to: f32,
    move_first: bool,
) {
    let steps = (((to - from).abs() / 5.0).ceil() as usize).max(2);
    for step in 0..=steps {
        let (x, y) = point_on(
            centre,
            radius,
            from + (to - from) * (step as f32 / steps as f32),
        );
        if step == 0 && move_first {
            builder.move_to(x, y);
        } else {
            builder.line_to(x, y);
        }
    }
}

/// A wedge: the ring band between the dead zone and the rim, cut to its angles.
fn wedge_path(centre: (f32, f32), inner: f32, rim: f32, from: f32, to: f32) -> Option<Path> {
    let mut builder = PathBuilder::new();
    arc_into(&mut builder, centre, rim, from, to, true);
    let (x, y) = point_on(centre, inner, to);
    builder.line_to(x, y);
    arc_into(&mut builder, centre, inner, to, from, false);
    builder.finish()
}

fn circle_path(centre: (f32, f32), radius: f32) -> Option<Path> {
    let mut builder = PathBuilder::new();
    arc_into(&mut builder, centre, radius, 0.0, 360.0, true);
    builder.finish()
}

fn line_path(from: (f32, f32), to: (f32, f32)) -> Option<Path> {
    let mut builder = PathBuilder::new();
    builder.move_to(from.0, from.1);
    builder.line_to(to.0, to.1);
    builder.finish()
}

fn fill(pixmap: &mut Pixmap, path: &Path, color: Color) {
    let paint = Paint {
        shader: Shader::SolidColor(color),
        anti_alias: true,
        ..Paint::default()
    };
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn fill_circle(pixmap: &mut Pixmap, centre: (f32, f32), radius: f32, color: Color) {
    if let Some(path) = circle_path(centre, radius) {
        fill(pixmap, &path, color);
    }
}

fn stroke(pixmap: &mut Pixmap, path: &Path, color: Color, width: f32) {
    let paint = Paint {
        shader: Shader::SolidColor(color),
        anti_alias: true,
        ..Paint::default()
    };
    let stroke = Stroke {
        width,
        ..Stroke::default()
    };
    pixmap.stroke_path(path, &paint, &stroke, Transform::identity(), None);
}

/// Draws each snippet's label at its `label_offset`, centred and cut short when the wedge is
/// too narrow for it.
fn draw_labels(pixmap: &mut Pixmap, data: &[u8], frame: &Frame, centre: (f32, f32), scale: f32) {
    let Ok(font) = FontRef::try_from_slice(data) else {
        log::warn!("could not parse the overlay font; drawing without labels");
        return;
    };
    let scaled = font.as_scaled(PxScale::from(LABEL_PX * scale));
    let factor = scaled.scale_factor();
    let count = frame.snippets.len();
    for (index, snippet) in frame.snippets.iter().enumerate() {
        let offset = frame.layout.label_offset(count, index);
        let label_centre = (
            centre.0 + offset.x as f32 * scale,
            centre.1 + offset.y as f32 * scale,
        );
        // The chord through the label's radius is the widest the wedge can show.
        let chord = 2.0 * offset.length() as f32 * (PI / count as f32).sin();
        let max_width = (chord - 10.0 * scale).max(LABEL_PX * scale);
        let text = cut_to_fit(&snippet.label, max_width, |c| {
            scaled.h_advance(scaled.glyph_id(c))
        });
        let width: f32 = text
            .chars()
            .map(|c| scaled.h_advance(scaled.glyph_id(c)))
            .sum();
        let baseline = label_centre.1 + (scaled.ascent() + scaled.descent()) / 2.0;
        let mut x = label_centre.0 - width / 2.0;
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            draw_glyph(
                pixmap,
                &font,
                factor,
                id,
                (x, baseline),
                Color::from_rgba8(240, 240, 240, 255),
            );
            x += scaled.h_advance(id);
        }
    }
}

/// Outlines one glyph into the pixmap. ab_glyph's curves are unscaled, unpositioned and point
/// up, so they are transformed into pixmap coordinates here.
fn draw_glyph(
    pixmap: &mut Pixmap,
    font: &FontRef,
    factor: PxScaleFactor,
    id: GlyphId,
    position: (f32, f32),
    color: Color,
) {
    let Some(outline) = font.outline(id) else {
        return; // Spaces and unknown characters have no outline.
    };
    let to_screen = |p: ab_glyph::Point| {
        (
            position.0 + p.x * factor.horizontal,
            position.1 - p.y * factor.vertical,
        )
    };
    let mut builder = PathBuilder::new();
    let mut previous: Option<ab_glyph::Point> = None;
    for curve in &outline.curves {
        let start = match curve {
            OutlineCurve::Line(start, _)
            | OutlineCurve::Quad(start, _, _)
            | OutlineCurve::Cubic(start, _, _, _) => *start,
        };
        // A jump back means a new contour, like the dot of an `i`.
        if !continues(previous, start) {
            let (x, y) = to_screen(start);
            builder.move_to(x, y);
        }
        match curve {
            OutlineCurve::Line(_, end) => {
                let (x, y) = to_screen(*end);
                builder.line_to(x, y);
            }
            OutlineCurve::Quad(_, control, end) => {
                let (cx, cy) = to_screen(*control);
                let (x, y) = to_screen(*end);
                builder.quad_to(cx, cy, x, y);
            }
            OutlineCurve::Cubic(_, control1, control2, end) => {
                let (c1x, c1y) = to_screen(*control1);
                let (c2x, c2y) = to_screen(*control2);
                let (x, y) = to_screen(*end);
                builder.cubic_to(c1x, c1y, c2x, c2y, x, y);
            }
        }
        previous = Some(match curve {
            OutlineCurve::Line(_, end)
            | OutlineCurve::Quad(_, _, end)
            | OutlineCurve::Cubic(_, _, _, end) => *end,
        });
    }
    if let Some(path) = builder.finish() {
        fill(pixmap, &path, color);
    }
}

/// Whether `previous` is where `start` continues from, within rounding noise.
fn continues(previous: Option<ab_glyph::Point>, start: ab_glyph::Point) -> bool {
    match previous {
        Some(p) => (p.x - start.x).abs() < 0.01 && (p.y - start.y).abs() < 0.01,
        None => false,
    }
}

/// Cuts `text` back to `max_width`, measured with `advance`, marking a cut with an ellipsis.
fn cut_to_fit(text: &str, max_width: f32, advance: impl Fn(char) -> f32) -> String {
    let width: f32 = text.chars().map(&advance).sum();
    if width <= max_width {
        return text.to_owned();
    }
    let ellipsis = advance('…');
    let mut width = 0.0;
    let mut cut = String::new();
    for c in text.chars() {
        let w = advance(c);
        if width + w + ellipsis > max_width {
            break;
        }
        width += w;
        cut.push(c);
    }
    if cut.is_empty() {
        return if ellipsis <= max_width {
            "…".to_owned()
        } else {
            String::new()
        };
    }
    cut.push('…');
    cut
}

/// tiny-skia stores premultiplied RGBA bytes; `UpdateLayeredWindow` wants premultiplied BGRA.
fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect()
}

/// Uploads the pixmap as the window's layered bitmap, positioned so that its centre is the
/// frame's centre.
fn present(hwnd: HWND, pixmap: &Pixmap, centre: Vec2) -> Result<(), OverlayError> {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let mut gdi = DcGuard::new(unsafe { GetDC(None) });
    let memory = gdi.memory_dc()?;

    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // A negative height makes the rows top-down, like tiny-skia's.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `info` describes a 32-bpp DIB, and `bits` receives its address.
    let bitmap =
        unsafe { CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0) }?;
    gdi.select(bitmap);

    let bgra = rgba_to_bgra(pixmap.data());
    // SAFETY: the DIB is `width * height * 4` bytes at 32 bits per pixel, and `bgra` is exactly
    // that long, so the copy stays in bounds.
    unsafe { std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits.cast(), bgra.len()) };

    let destination = POINT {
        x: centre.x as i32 - width / 2,
        y: centre.y as i32 - height / 2,
    };
    let window_size = SIZE {
        cx: width,
        cy: height,
    };
    let source = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    // SAFETY: every handle and pointer is valid; `ULW_ALPHA` matches `AC_SRC_ALPHA`, and
    // tiny-skia's output is already premultiplied.
    unsafe {
        UpdateLayeredWindow(
            hwnd,
            Some(gdi.screen),
            Some(&destination),
            Some(&window_size),
            Some(memory),
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )?;
    }
    Ok(())
}

/// The GDI resources of one `present` call. By the time this drops, `UpdateLayeredWindow` has
/// copied what it needs, so deleting then is safe.
struct DcGuard {
    screen: HDC,
    memory: HDC,
    selected: Option<(HBITMAP, HGDIOBJ)>,
}

impl DcGuard {
    fn new(screen: HDC) -> Self {
        Self {
            screen,
            memory: HDC(std::ptr::null_mut()),
            selected: None,
        }
    }

    fn memory_dc(&mut self) -> Result<HDC, OverlayError> {
        // SAFETY: the screen DC is valid; the memory DC is our own.
        self.memory = unsafe { CreateCompatibleDC(Some(self.screen)) };
        if self.memory.0.is_null() {
            return Err(OverlayError::Window(last_error()));
        }
        Ok(self.memory)
    }

    fn select(&mut self, bitmap: HBITMAP) {
        // SAFETY: both handles are valid; the DC's previous bitmap is restored on drop.
        let previous = unsafe { SelectObject(self.memory, HGDIOBJ(bitmap.0)) };
        self.selected = Some((bitmap, previous));
    }
}

impl Drop for DcGuard {
    fn drop(&mut self) {
        if let Some((bitmap, previous)) = self.selected.take() {
            // SAFETY: restore the bitmap the memory DC had before ours was selected, then delete
            // our bitmap, which is no longer selected anywhere.
            unsafe { SelectObject(self.memory, previous) };
            let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
        }
        if !self.memory.0.is_null() {
            // SAFETY: our own memory DC, created in `memory_dc`.
            let _ = unsafe { DeleteDC(self.memory) };
        }
        // SAFETY: the screen DC came from `GetDC(None)`.
        let _ = unsafe { ReleaseDC(None, self.screen) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wedges_start_at_twelve_and_go_clockwise() {
        assert_eq!(wedge_angles(0, 4), (-135.0, -45.0));
        assert_eq!(wedge_angles(1, 4), (-45.0, 45.0));
        assert_eq!(wedge_angles(2, 4), (45.0, 135.0));
    }

    #[test]
    fn rgba_becomes_bgra() {
        assert_eq!(
            rgba_to_bgra(&[1, 2, 3, 4, 5, 6, 7, 8]),
            [3, 2, 1, 4, 7, 6, 5, 8]
        );
    }

    #[test]
    fn cut_to_fit_keeps_text_that_fits() {
        let advance = |_: char| 1.0;
        assert_eq!(cut_to_fit("hello", 5.0, advance), "hello");
    }

    #[test]
    fn cut_to_fit_marks_the_cut_with_an_ellipsis() {
        let advance = |_: char| 1.0;
        assert_eq!(cut_to_fit("hello", 4.0, advance), "hel…");
    }

    #[test]
    fn cut_to_fit_handles_a_very_narrow_wedge() {
        let advance = |c: char| if c == '…' { 3.0 } else { 1.0 };
        assert_eq!(cut_to_fit("hello", 2.0, advance), "");
        assert_eq!(cut_to_fit("hello", 3.0, advance), "…");
    }
}
