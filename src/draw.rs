//! Drawing on the app's own `Graphics` object.
//!
//! The app owns the screen.  A mod draws on top with the app's own calls
//! (`setColour`, `fillRect`, `Font` + `drawText`); the SDK keeps the offsets and
//! the object lifetimes in one place and exposes them as a small [`Graphics`]
//! handle.  The toast is the SDK's one piece of retained drawing state.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::image::{self, TextBox};

/// `setColour(uint32_t)`.
const SET_COLOUR: usize = 0x102_d5e0;
/// `fillRect(int, int, int, int)`.
const FILL_RECT: usize = 0x102_e000;
/// `String(const char *)`.
const STRING_FROM_C: usize = 0x0f5_35c0;
/// `Font(int, float)`.
const FONT: usize = 0x108_2810;
/// `setFont(const Font &)`.
const SET_FONT: usize = 0x102_dca0;
/// `drawText(const String &, int, int, int, int, int, int, float)`.
const DRAW_TEXT: usize = 0x108_17b0;
/// `Font::~Font()`.
const FONT_DROP: usize = 0x107_0d60;
/// `String::~String()`.
const STRING_DROP: usize = 0x0f0_a290;

/// A live `Graphics` object the app is painting with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Graphics {
    handle: usize,
}

impl Graphics {
    /// The app's graphics object.
    #[must_use]
    pub const fn new(handle: usize) -> Self {
        Self { handle }
    }

    /// The raw pointer the app passed.
    #[must_use]
    pub const fn handle(self) -> usize {
        self.handle
    }

    /// Set the fill colour.
    ///
    /// # Safety
    /// The graphics object must be live on the UI thread and the firmware initialized.
    pub unsafe fn colour(self, value: u32) {
        // SAFETY: the caller guarantees a live graphics object.
        unsafe { colour(self.handle, value) };
    }

    /// Fill a rectangle in the current colour.
    ///
    /// # Safety
    /// The graphics object must be live on the UI thread and the firmware initialized.
    pub unsafe fn rect(self, x: i32, y: i32, width: i32, height: i32) {
        // SAFETY: the caller guarantees a live graphics object.
        unsafe { rect(self.handle, x, y, width, height) };
    }

    /// Draw one line of text in the current colour.
    ///
    /// # Safety
    /// The graphics object must be live on the UI thread and the firmware initialized.
    pub unsafe fn text(self, line: &str, x: i32, y: i32, width: i32, height: i32, size: f32) {
        // SAFETY: the caller guarantees a live graphics object.
        unsafe { text(self.handle, line, x, y, width, height, size) };
    }
}

/// Set the fill colour.
///
/// # Safety
/// The graphics object must be live on the UI thread and the firmware initialized.
pub unsafe fn colour(graphics: usize, value: u32) {
    // SAFETY: `graphics` is the app's object and the offset its own call.
    unsafe { image::void_of_colour(image::base() + SET_COLOUR, graphics, value) };
}

/// Fill a rectangle in the current colour.
///
/// # Safety
/// The graphics object must be live on the UI thread and the firmware initialized.
pub unsafe fn rect(graphics: usize, x: i32, y: i32, width: i32, height: i32) {
    // SAFETY: as above.
    unsafe { image::void_of_five(image::base() + FILL_RECT, graphics, x, y, width, height) };
}

/// Draw one line of text in the current colour.
///
/// # Safety
/// The graphics object must be live on the UI thread and the firmware initialized.
pub unsafe fn text(
    graphics: usize,
    line: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    size: f32,
) {
    let mut bytes = [0_u8; 96];
    let mut take = line.len().min(bytes.len() - 1);
    while !line.is_char_boundary(take) {
        take -= 1;
    }
    bytes[..take].copy_from_slice(&line.as_bytes()[..take]);
    let base = image::base();
    let mut string_word = [0_u64; 2];
    let mut font = [0_u64; 2];
    // SAFETY: the string is NUL-terminated and both objects are live for the
    // calls below, which is how the app builds them.
    unsafe {
        image::string_from_c(
            base + STRING_FROM_C,
            string_word.as_mut_ptr() as usize,
            bytes.as_ptr() as usize,
        );
        let font_fn: unsafe extern "C" fn(usize, i32, f32) = core::mem::transmute(base + FONT);
        font_fn(font.as_mut_ptr() as usize, 0, size);
        let set_font: unsafe extern "C" fn(usize, usize) = core::mem::transmute(base + SET_FONT);
        set_font(graphics, font.as_ptr() as usize);
        image::void_of_eight(
            base + DRAW_TEXT,
            graphics,
            string_word.as_ptr() as usize,
            TextBox {
                x,
                y,
                width,
                height,
                flags: 0x24,
                align: 1,
                opacity: 0.7,
            },
        );
        let drop_font: unsafe extern "C" fn(usize) = core::mem::transmute(base + FONT_DROP);
        drop_font(font.as_mut_ptr() as usize);
        let drop_string: unsafe extern "C" fn(usize) = core::mem::transmute(base + STRING_DROP);
        drop_string(string_word.as_mut_ptr() as usize);
    }
}

/// The app's repaint: component and two packed rectangles.
pub type Repaint = unsafe extern "C" fn(usize, u64, u64);

/// The component the page last painted.
static PAINTED: AtomicUsize = AtomicUsize::new(0);

/// Record the component the page painted.
pub(crate) fn set_painted(component: usize) {
    PAINTED.store(component, Ordering::Release);
}

/// The component the page last painted, or zero.
#[must_use]
pub fn painted() -> usize {
    PAINTED.load(Ordering::Acquire)
}

/// The toast: text plus a deadline, drawn by the paint hook so it costs nothing
/// when idle.
#[derive(Clone, Copy)]
struct Toast {
    until: i64,
    text: [u8; 48],
}

static TOAST: std::sync::Mutex<Toast> = std::sync::Mutex::new(Toast {
    until: 0,
    text: [0; 48],
});

/// Show a toast and keep the screen repainting until it expires.
///
/// # Safety
///
/// `line` must be readable through its NUL terminator (at most 47 bytes read).
/// `repaint` must be the app's repaint and `component` a live component for the
/// immediate repaint.
pub unsafe fn toast_line(line: *const u8, repaint: Option<Repaint>, component: usize) {
    if line.is_null() {
        return;
    }
    let now = crate::platform::clock::now_ms();
    // SAFETY: only the UI thread shows toasts.
    let mut toast = TOAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut length = 0;
    // SAFETY: the caller passes a NUL-terminated string.
    unsafe {
        while length < toast.text.len() - 1 && *line.add(length) != 0 {
            length += 1;
        }
        for (slot, byte) in toast
            .text
            .iter_mut()
            .zip(core::slice::from_raw_parts(line, length))
        {
            *slot = *byte;
        }
        toast.text[length] = 0;
    }
    toast.until = now + 1000;
    drop(toast);
    redraw(repaint, component);
    std::thread::spawn(move || {
        std::thread::sleep(core::time::Duration::from_millis(1120));
        // Re-read the component the SDK most recently saw painted rather than
        // holding the one captured above: a later page may own the screen now.
        let current = if painted() != 0 { painted() } else { screen() };
        // SAFETY: the app's repaint is safe to call from this thread, which is
        // what the glue's own expiry thread did; `current` is the component the
        // paint hook last observed.
        redraw(repaint, current);
    });
}

/// Ask the app to repaint the screen.
fn redraw(repaint: Option<Repaint>, component: usize) {
    if let Some(repaint) = repaint
        && component != 0
    {
        // SAFETY: the caller guarantees this is the app's repaint.
        unsafe { repaint(component, 0, (2000_u64 << 32) | 0x7d0) };
    }
}

/// Show a toast for the standard one second, and repaint when it expires.
pub fn show_toast(text: &str) {
    // The C entry point reads a NUL-terminated string, so hand it one.
    let mut line = text.as_bytes().to_vec();
    line.push(0);
    // The paint hook may not run again after this call, so ask for the expiry
    // repaint here; otherwise the toast pixels stay until the next user action.
    let component = if painted() != 0 { painted() } else { screen() };
    // SAFETY: the buffer is NUL-terminated and lives for the call; `repaint`
    // is the app's own repaint and `component` is on the current page.
    unsafe {
        toast_line(line.as_ptr(), crate::sites::repaint_function(), component);
    }
}

/// The toast's text, or an empty string.
#[must_use]
pub fn toast_text() -> Vec<u8> {
    // The snapshot is owned, so a later toast cannot change bytes the caller is
    // still holding.
    let toast = TOAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let end = toast.text.iter().position(|&byte| byte == 0).unwrap_or(0);
    toast.text[..end].to_vec()
}

/// Whether a toast is still on screen.
#[must_use]
pub fn toast_visible(now: i64) -> bool {
    let toast = TOAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    toast.until > now && toast.text[0] != 0
}

pub use crate::platform::page::{g_screen, screen};

/// Repaint the component the page last painted, or the page itself.
pub fn repaint_painted() {
    let painted = painted();
    let component = if painted != 0 { painted } else { screen() };
    // SAFETY: the paint hook recorded this live component, and the page stays
    // alive while the mod handles a UI callback.
    unsafe { crate::sites::repaint_component(component) };
}

/// Start a steady repaint pump once.
///
/// `component` is asked for a component on every tick; zero means "nothing to
/// repaint" and skips the tick.  The pump runs for the life of the process.
///
/// # Safety
///
/// `component` must return only components that are live at the moment of the
/// call, or zero.  Prefer [`painted`], which the paint hook maintains.
pub unsafe fn start_repaint_pump(component: fn() -> usize) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("mod-repaint".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(core::time::Duration::from_millis(60));
                let component = component();
                if component != 0 {
                    // SAFETY: the caller's function returns live components, or
                    // zero which is skipped.
                    unsafe { crate::sites::repaint_component(component) };
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{Graphics, TOAST, Toast, toast_line, toast_text, toast_visible};

    /// The toast is process-wide, so the tests that write it serialise.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn reset() {
        *TOAST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Toast {
            until: 0,
            text: [0; 48],
        };
    }

    #[test]
    fn nothing_is_drawn_before_a_toast() {
        let _serial = serial();
        reset();
        assert!(!toast_visible(0));
    }

    #[test]
    fn a_toast_snapshot_is_not_aliased_by_a_later_toast() {
        let _serial = serial();
        reset();
        // SAFETY: both strings are NUL-terminated, and a null repaint/zero
        // component skips the firmware call.
        unsafe {
            toast_line(c"first".as_ptr().cast(), None, 0);
        }
        assert!(toast_visible(crate::platform::clock::now_ms()));
        let snapshot = toast_text();
        // SAFETY: as above.
        unsafe {
            toast_line(c"second".as_ptr().cast(), None, 0);
        }
        assert_eq!(snapshot, b"first", "the old snapshot must stay intact");
        assert_eq!(toast_text(), b"second");
        reset();
    }

    #[test]
    fn a_graphics_handle_keeps_its_pointer() {
        let graphics = Graphics::new(0x1234);
        assert_eq!(graphics.handle(), 0x1234);
    }
}
