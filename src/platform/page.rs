//! What the Sample page is currently showing.
//!
//! Two questions decide most of a mod's behaviour: is the Sample page the visible
//! screen, and is it in chop mode?  Both are answered by the page's own
//! predicate, not by guessing from state the mod tracks itself.

use crate::image;
use core::sync::atomic::{AtomicUsize, Ordering};

/// The Sample page's vtable, as an offset from the image base.
pub const SAMPLE_PAGE_VTABLE: usize = 0x6e2_12a0;
/// `[screen + 0x1ee0]` is the chop-mode view the predicate takes.
const CHOP_VIEW: usize = 0x1ee0;
/// The app's predicate for "this view is in chop mode".
const CHOP_PREDICATE: usize = 0x112_fd50;

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
    // SAFETY: same predicate, applied to the page's chop view.
    unsafe {
        image::int_of_pointer(
            image::base() + CHOP_PREDICATE,
            screen.wrapping_add(CHOP_VIEW),
        ) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::{CHOP_VIEW, SAMPLE_PAGE_VTABLE, chop_mode, screen, set_screen, visible};

    #[test]
    fn a_null_screen_is_never_a_sample_page() {
        // SAFETY: null is rejected without native calls.
        assert!(!unsafe { visible(0) });
        // SAFETY: null is rejected without native calls.
        assert!(!unsafe { chop_mode(0) });
        assert_eq!(SAMPLE_PAGE_VTABLE, 0x6e2_12a0);
        assert_eq!(CHOP_VIEW, 0x1ee0);
    }

    #[test]
    fn the_last_painted_screen_is_read_back() {
        set_screen(0x1234);
        assert_eq!(screen(), 0x1234);
        set_screen(0);
    }
}
