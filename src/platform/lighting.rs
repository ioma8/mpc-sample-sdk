//! Per-pad RGB overrides for the AC50 pad LEDs.
//!
//! The app sends every pad colour through one native sender. The SDK hooks its
//! entry and substitutes a colour for pads a mod owns; all other pads keep the
//! app's native colour. The sender is called again on every native lighting
//! refresh, so an override persists until it is cleared.
//!
//! Colours are `0xRRGGBB`. The native protocol keeps 7 bits per channel.

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// No override is installed for this pad.
const NONE: u32 = u32::MAX;
/// The AC50 has sixteen pads.
pub const PADS: usize = 16;
/// Pads whose colour is replaced.
static OVERRIDES: [AtomicU32; PADS] = [const { AtomicU32::new(NONE) }; PADS];
/// The last sender object seen by the native sender.
static SENDER: AtomicUsize = AtomicUsize::new(0);

/// Replace one pad's colour.
///
/// Returns `false` for an invalid pad or a colour outside `0xRRGGBB`.
#[must_use]
pub fn set_pad_rgb(pad: u8, rgb: u32) -> bool {
    let index = usize::from(pad);
    if index >= PADS || rgb > 0x00ff_ffff {
        return false;
    }
    OVERRIDES[index].store(rgb, Ordering::Release);
    true
}

/// Stop replacing one pad's colour.
pub fn clear_pad_rgb(pad: u8) {
    let index = usize::from(pad);
    if index < PADS {
        OVERRIDES[index].store(NONE, Ordering::Release);
    }
}

/// Clear every pad override.
pub fn clear_pad_rgbs() {
    for slot in &OVERRIDES {
        slot.store(NONE, Ordering::Release);
    }
}

/// The native sender object, or zero before the app has sent a colour.
#[must_use]
pub fn sender() -> usize {
    SENDER.load(Ordering::Acquire)
}

/// Record the sender and return the colour the sender should use.
///
/// Called by the SDK's native sender hook.
pub(crate) fn observed(sender: usize, pad: u8, native: u32) -> u32 {
    SENDER.store(sender, Ordering::Release);
    let index = usize::from(pad);
    if index >= PADS {
        return native;
    }
    let replacement = OVERRIDES[index].load(Ordering::Acquire);
    if replacement == NONE {
        native
    } else {
        replacement
    }
}

#[cfg(test)]
mod tests {
    use super::{PADS, clear_pad_rgb, clear_pad_rgbs, observed, set_pad_rgb};

    #[test]
    fn an_override_replaces_the_native_colour_until_cleared() {
        clear_pad_rgbs();
        assert!(set_pad_rgb(3, 0x12_34_56));
        assert_eq!(observed(0x1000, 3, 0), 0x12_34_56);
        assert_eq!(observed(0x1000, 2, 0xaa_bb_cc), 0xaa_bb_cc);
        clear_pad_rgb(3);
        assert_eq!(observed(0x1000, 3, 0xaa_bb_cc), 0xaa_bb_cc);
        clear_pad_rgbs();
    }

    #[test]
    fn invalid_pads_and_colours_are_refused() {
        assert!(!set_pad_rgb(PADS as u8, 0));
        assert!(!set_pad_rgb(0, 0x01_00_00_00));
        assert_eq!(observed(0, PADS as u8, 0x11_22_33), 0x11_22_33);
    }
}
