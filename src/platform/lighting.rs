//! Per-pad RGB overrides for the AC50 pad LEDs.
//!
//! The app sends every pad colour through one native sender. The SDK hooks its
//! entry and substitutes a colour for pads a mod owns; all other pads keep the
//! app's native colour. The sender is called again on every native lighting
//! refresh, so an override persists until it is cleared.
//!
//! Colours are `0xRRGGBB`; brightness is `0.0..=1.0`. The native protocol keeps
//! 7 bits per channel.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// No override is installed for this pad.
const NONE: u64 = u64::MAX;
/// The AC50 has sixteen pads.
pub const PADS: usize = 16;
/// Pads whose colour is replaced (`RGB | brightness_float << 32`).
static OVERRIDES: [AtomicU64; PADS] = [const { AtomicU64::new(NONE) }; PADS];
/// The last sender object seen by the native sender.
static SENDER: AtomicUsize = AtomicUsize::new(0);
/// The app's last native light for each pad, before any override.
static NATIVE: [AtomicU64; PADS] = [const { AtomicU64::new(0) }; PADS];

/// Replace one pad's colour at full brightness.
///
/// Returns `false` for an invalid pad or a colour outside `0xRRGGBB`.
#[must_use]
pub fn set_pad_rgb(pad: u8, rgb: u32) -> bool {
    set_pad_light(pad, rgb, 1.0)
}

/// Replace one pad's colour and brightness.
///
/// `brightness` is clamped to `0.0..=1.0`; zero turns the pad off. Returns
/// `false` for an invalid pad or a colour outside `0xRRGGBB`.
#[must_use]
pub fn set_pad_light(pad: u8, rgb: u32, brightness: f32) -> bool {
    let index = usize::from(pad);
    if index >= PADS || rgb > 0x00ff_ffff || !brightness.is_finite() {
        return false;
    }
    let lit = (u64::from(brightness.clamp(0.0, 1.0).to_bits()) << 32) | u64::from(rgb);
    OVERRIDES[index].store(lit, Ordering::Release);
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

/// Send one pad's light immediately and keep the override for later refreshes.
///
/// Returns `false` for an invalid pad/colour or before the app has produced a
/// sender; the stored override still applies on the next native callback.
#[must_use]
pub fn send_pad_light(pad: u8, rgb: u32, brightness: f32) -> bool {
    if !set_pad_light(pad, rgb, brightness) {
        return false;
    }
    let Some(light) = override_light(pad) else {
        return false;
    };
    send(pad, light)
}

/// Clear one pad's override and immediately restore the app's native light.
pub fn restore_pad_rgb(pad: u8) {
    clear_pad_rgb(pad);
    let index = usize::from(pad);
    if index < PADS {
        let _ = send(pad, NATIVE[index].load(Ordering::Acquire));
    }
}

/// Clear every pad override and restore all native lights.
pub fn restore_pad_rgbs() {
    clear_pad_rgbs();
    for pad in 0..PADS {
        let Ok(pad) = u8::try_from(pad) else {
            continue;
        };
        let _ = send(pad, NATIVE[pad as usize].load(Ordering::Acquire));
    }
}

/// Call the original per-pad callback with one `PadLight`.
fn send(pad: u8, light: u64) -> bool {
    let sender = SENDER.load(Ordering::Acquire);
    if sender == 0 {
        return false;
    }
    crate::sites::send_light(sender, light, pad)
}

/// The native sender object, or zero before the app has sent a colour.
#[must_use]
pub fn sender() -> usize {
    SENDER.load(Ordering::Acquire)
}

/// Record the sender and the native light observed by the native hook.
pub(crate) fn observe(sender: usize, pad: u8, native: u64) {
    SENDER.store(sender, Ordering::Release);
    let index = usize::from(pad);
    if index < PADS {
        NATIVE[index].store(native, Ordering::Release);
    }
}

/// The `PadLight` a pad should use, or `None` to keep the native one.
///
/// Called by the SDK's native lighting hook.
pub(crate) fn override_light(pad: u8) -> Option<u64> {
    let index = usize::from(pad);
    if index >= PADS {
        return None;
    }
    let value = OVERRIDES[index].load(Ordering::Acquire);
    (value != NONE).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::{PADS, clear_pad_rgb, clear_pad_rgbs, override_light, set_pad_light, set_pad_rgb};

    #[test]
    fn an_override_replaces_the_native_light_until_cleared() {
        clear_pad_rgbs();
        assert!(set_pad_light(3, 0x12_34_56, 0.25));
        let lit = override_light(3).expect("override");
        assert_eq!(lit as u32, 0x12_34_56);
        assert_eq!((lit >> 32) as u32, 0.25_f32.to_bits());
        assert_eq!(override_light(2), None);
        clear_pad_rgb(3);
        assert_eq!(override_light(3), None);
        clear_pad_rgbs();
    }

    #[test]
    fn native_lights_are_remembered_before_an_override() {
        clear_pad_rgbs();
        super::observe(0x2000, 4, 0x0011_2233);
        assert_eq!(super::sender(), 0x2000);
        assert_eq!(
            super::NATIVE[4].load(core::sync::atomic::Ordering::Acquire),
            0x0011_2233
        );
        clear_pad_rgbs();
    }

    #[test]
    fn rgb_is_full_brightness_and_zero_is_valid() {
        assert!(set_pad_rgb(1, 0x00_ff_00));
        let lit = override_light(1).expect("override");
        assert_eq!((lit >> 32) as u32, 1.0_f32.to_bits());
        assert!(set_pad_light(0, 0x00_00_ff, 0.0));
        let lit = override_light(0).expect("override");
        assert_eq!((lit >> 32) as u32, 0.0_f32.to_bits());
        clear_pad_rgbs();
    }

    #[test]
    fn invalid_pads_colours_and_brightness_are_refused() {
        assert!(!set_pad_rgb(PADS as u8, 0));
        assert!(!set_pad_rgb(0, 0x01_00_00_00));
        assert!(!set_pad_light(0, 0, f32::NAN));
        assert_eq!(override_light(PADS as u8), None);
    }
}
