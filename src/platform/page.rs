//! What the Sample page is currently showing.
//!
//! Two questions decide most of a mod's behaviour: is the Sample page the visible
//! screen, and is it in chop mode? Both are read from the app's own state.

use crate::image;
use core::sync::atomic::{AtomicUsize, Ordering};

/// The Sample page's vtable, as an offset from the image base.
pub const SAMPLE_PAGE_VTABLE: usize = crate::firmware::SAMPLE_PAGE_VTABLE.offset;
/// `[screen + 0x1ee0]` is the 1.3 chop-mode view the predicate takes.
#[cfg(feature = "fw-1-3")]
const CHOP_VIEW: usize = 0x1ee0;
/// The app's visibility predicate.
const CHOP_PREDICATE: usize = crate::firmware::CHOP_PREDICATE.offset;

/// The Sample page the mod last painted.  It keeps its exported name because the
/// device tests find the page through it.
#[unsafe(no_mangle)]
pub static g_screen: AtomicUsize = AtomicUsize::new(0);

/// The sample page the mod last painted, or zero.
#[must_use]
pub fn screen() -> usize {
    g_screen.load(Ordering::Acquire)
}

/// Record the sample page the paint hook just saw.
pub(crate) fn set_screen(screen: usize) {
    g_screen.store(screen, Ordering::Release);
}

/// Whether `screen` is a Sample page.
#[must_use]
pub fn is_sample_page(screen: usize) -> bool {
    screen != 0
        && image::read_pointer(screen)
            .is_some_and(|vtable| vtable == image::base() + SAMPLE_PAGE_VTABLE)
}

/// Whether the Sample page is visible and showing a sample.
///
/// # Safety
/// `screen` must be a live native page on its owning UI thread.
#[must_use]
pub unsafe fn visible(screen: usize) -> bool {
    if !is_sample_page(screen) {
        return false;
    }
    // SAFETY: `base + CHOP_PREDICATE` is the page's own predicate.
    unsafe { image::int_of_pointer(image::base() + CHOP_PREDICATE, screen) != 0 }
}

/// Whether the Sample page is in chop mode.
///
/// # Safety
/// `screen` must be a live native page on its owning UI thread.
#[must_use]
pub unsafe fn chop_mode(screen: usize) -> bool {
    if !is_sample_page(screen) {
        return false;
    }
    #[cfg(feature = "fw-1-4")]
    return chop_controller_enabled(screen);
    #[cfg(feature = "fw-1-3")]
    // SAFETY: same predicate, applied to the page's chop view.
    unsafe {
        image::int_of_pointer(
            image::base() + CHOP_PREDICATE,
            screen.wrapping_add(CHOP_VIEW),
        ) != 0
    }
}

/// The 1.4 handler reads bit 0 of its chop controller's byte at +0x53.
#[cfg(feature = "fw-1-4")]
fn chop_controller_enabled(screen: usize) -> bool {
    let Some(controller) = screen.checked_add(0x158).and_then(image::read_pointer) else {
        return false;
    };
    let mut flag = [0_u8; 1];
    controller
        .checked_add(0x53)
        .is_some_and(|address| image::read(address, &mut flag))
        && flag[0] & 1 != 0
}

#[cfg(test)]
mod tests {
    use super::{chop_mode, screen, set_screen, visible};

    #[test]
    fn a_null_screen_is_never_a_sample_page() {
        // SAFETY: null is rejected without native calls.
        assert!(!unsafe { visible(0) });
        // SAFETY: null is rejected without native calls.
        assert!(!unsafe { chop_mode(0) });
    }

    #[cfg(feature = "fw-1-4")]
    #[test]
    fn chop_controller_flag_tracks_chop_mode() {
        use crate::image::test_memory;
        let screen = 0x1000_usize;
        let controller = 0x2000_usize;
        for (flag, expected) in [(0, false), (1, true), (2, false)] {
            let regions = vec![
                (screen + 0x158, controller.to_le_bytes().to_vec()),
                (controller + 0x53, vec![flag]),
            ];
            assert_eq!(
                test_memory::with(regions, || super::chop_controller_enabled(screen)),
                expected
            );
        }
    }

    #[test]
    fn the_last_painted_screen_is_read_back() {
        set_screen(0x1234);
        assert_eq!(screen(), 0x1234);
        set_screen(0);
    }
}
