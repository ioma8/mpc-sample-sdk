//! Per-pad RGB overrides for the AC50 pad LEDs.
//!
//! The app sends every pad colour through one native sender. The SDK hooks its
//! entry and substitutes a colour for pads a mod owns; all other pads keep the
//! app's native colour. The app only sends when its own lighting state changes,
//! so the API also sends a changed pad immediately.
//!
//! The sender is a native object the app may replace or destroy. Native calls
//! run inside a gate that the two firmware destructor hooks close and drain
//! before the object is freed, and one send lock serializes callers, so a light
//! update can never overlap its own teardown.
//!
//! Colours are `0xRRGGBB`; brightness is `0.0..=1.0`. The native protocol keeps
//! 7 bits per channel.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// No override is installed for this pad.
const NONE: u64 = u64::MAX;
/// The AC50 has sixteen pads.
pub const PADS: usize = 16;
/// Pads whose colour is replaced (`RGB | brightness_float << 32`).
static OVERRIDES: [AtomicU64; PADS] = [const { AtomicU64::new(NONE) }; PADS];
/// The last sender object seen by the native sender.
static SENDER: AtomicUsize = AtomicUsize::new(0);
/// Tracking the sender requires both firmware destruction hooks, so a stale
/// object can be forgotten even when the app replaces it.
static LIFECYCLE_READY: AtomicBool = AtomicBool::new(false);
/// The app's last native light for each pad, before any override.
static NATIVE: [AtomicU64; PADS] = [const { AtomicU64::new(0) }; PADS];
/// Updates that still need a live sender.
static PENDING: AtomicU32 = AtomicU32::new(0);
/// Prevent nested or concurrent flushes from reordering LED updates.
static FLUSHING: AtomicBool = AtomicBool::new(false);
/// Set by a destructor hook before the firmware frees the sender.
static CLOSED: AtomicBool = AtomicBool::new(false);
/// Native sends in flight; a destructor waits for this to reach zero.
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
/// Serializes native sends from different feature threads.
static SEND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// Keep one pad's override and send it now.
///
/// Returns `false` for an invalid pad/colour or before the app has produced a
/// sender. The send is serialized with other callers and with the sender's own
/// destructor, so it runs now from any thread.
#[must_use]
pub fn send_pad_light(pad: u8, rgb: u32, brightness: f32) -> bool {
    if !set_pad_light(pad, rgb, brightness) {
        return false;
    }
    request_send(pad)
}

/// Clear one pad's override and restore the app's native light.
pub fn restore_pad_rgb(pad: u8) {
    clear_pad_rgb(pad);
    let _ = request_send(pad);
}

/// Clear every pad override and restore all native lights.
pub fn restore_pad_rgbs() {
    clear_pad_rgbs();
    PENDING.fetch_or((1_u32 << PADS) - 1, Ordering::AcqRel);
    flush_pending();
}

/// Queue one pad update and send it now.
fn request_send(pad: u8) -> bool {
    if usize::from(pad) >= PADS {
        return false;
    }
    PENDING.fetch_or(1_u32 << pad, Ordering::AcqRel);
    let known = SENDER.load(Ordering::Acquire) != 0;
    flush_pending();
    known
}

/// Send at most one update per pad; failed sends remain pending.
fn flush_pending() {
    if FLUSHING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    loop {
        let pending = PENDING.swap(0, Ordering::AcqRel);
        let mut failed = 0;
        for (pad, native) in NATIVE.iter().enumerate() {
            let bit = 1_u32 << pad;
            if pending & bit == 0 {
                continue;
            }
            let Ok(pad_byte) = u8::try_from(pad) else {
                continue;
            };
            let light = override_light(pad_byte).unwrap_or_else(|| native.load(Ordering::Acquire));
            if !send(pad_byte, light) {
                failed |= bit;
            }
        }
        if failed != 0 {
            PENDING.fetch_or(failed, Ordering::Release);
        }
        FLUSHING.store(false, Ordering::Release);
        if PENDING.load(Ordering::Acquire) & !failed == 0
            || FLUSHING
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return;
        }
    }
}

/// Call the original per-pad callback with one `PadLight`.
///
/// The in-flight count is what the destructor hook drains, so a send can never
/// overlap the firmware freeing the object.
fn send(pad: u8, light: u64) -> bool {
    let sender = SENDER.load(Ordering::Acquire);
    if sender == 0 || CLOSED.load(Ordering::Acquire) {
        return false;
    }
    IN_FLIGHT.fetch_add(1, Ordering::AcqRel);
    if CLOSED.load(Ordering::Acquire) || SENDER.load(Ordering::Acquire) != sender {
        IN_FLIGHT.fetch_sub(1, Ordering::Release);
        return false;
    }
    let serial = SEND_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let sent = forward_light(sender, light, pad);
    drop(serial);
    IN_FLIGHT.fetch_sub(1, Ordering::Release);
    sent
}

/// The native sender object, or zero before the app has sent a colour.
#[must_use]
pub fn sender() -> usize {
    SENDER.load(Ordering::Acquire)
}

/// Record the sender and the native light observed by the native hook, then
/// apply any queued overrides through it.
///
/// The caller is inside the app's own lighting callback, so the sender is live.
pub(crate) fn observe(sender: usize, pad: u8, native: u64) {
    let expected = crate::image::base().checked_add(crate::firmware::LIGHTING_SENDER_VTABLE.offset);
    let tracked = LIFECYCLE_READY.load(Ordering::Acquire)
        && expected.is_some_and(|vtable| crate::image::read_pointer(sender) == Some(vtable));
    if tracked {
        CLOSED.store(false, Ordering::Release);
    }
    SENDER.store(if tracked { sender } else { 0 }, Ordering::Release);
    let index = usize::from(pad);
    if index < PADS {
        NATIVE[index].store(native, Ordering::Release);
    }
    if tracked {
        flush_pending();
    }
}

pub(crate) fn set_lifecycle_ready(ready: bool) {
    LIFECYCLE_READY.store(ready, Ordering::Release);
    if !ready {
        SENDER.store(0, Ordering::Release);
        CLOSED.store(true, Ordering::Release);
    }
}

/// Stop new sends through `sender` and wait for a call already inside the gate.
///
/// Called from the two firmware destructor hooks before the object is freed.
pub(crate) fn forget_sender(sender: usize) {
    CLOSED.store(true, Ordering::Release);
    if SENDER
        .compare_exchange(sender, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        // The native call in flight is a bounded LED write; wait it out so the
        // destructor cannot free the object under it.
        while IN_FLIGHT.load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
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

/// Send one already-packed `PadLight` through the original callback.
fn forward_light(sender: usize, light: u64, pad: u8) -> bool {
    // SAFETY: site 17 is the installed lighting callback.
    let Some(forward) = crate::sites::originals().lighting else {
        return false;
    };
    let mut light = light;
    // SAFETY: the original callback converts and sends this live `PadLight`.
    unsafe { forward(sender, core::ptr::from_mut(&mut light), pad) };
    true
}

#[cfg(test)]
mod tests {
    use super::{
        PADS, clear_pad_rgb, clear_pad_rgbs, override_light, send_pad_light, set_pad_light,
        set_pad_rgb,
    };
    use core::sync::atomic::Ordering;

    /// The overrides, the sender and the owner are process globals; tests that
    /// touch them take turns.
    static GLOBALS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn globals() -> std::sync::MutexGuard<'static, ()> {
        GLOBALS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn a_send_without_an_observed_sender_is_only_queued() {
        let _globals = globals();
        clear_pad_rgbs();
        super::set_lifecycle_ready(false);
        super::PENDING.store(0, Ordering::Release);
        assert!(!send_pad_light(2, 0x00_12_34, 1.0));
        assert_ne!(super::PENDING.load(Ordering::Acquire) & (1 << 2), 0);
        assert_eq!(super::sender(), 0);
        assert!(
            override_light(2).is_some(),
            "the queued override still applies on the next refresh"
        );
        super::PENDING.store(0, Ordering::Release);
        clear_pad_rgbs();
    }

    #[test]
    fn a_closed_sender_is_never_called() {
        let _globals = globals();
        super::set_lifecycle_ready(true);
        super::SENDER.store(0x2000, Ordering::Release);
        super::CLOSED.store(false, Ordering::Release);
        super::CLOSED.store(true, Ordering::Release);
        assert!(!super::send(0, 0), "a closed sender must not be used");
        super::SENDER.store(0, Ordering::Release);
        super::CLOSED.store(false, Ordering::Release);
        super::set_lifecycle_ready(false);
    }

    #[test]
    fn a_destructor_waits_for_a_send_already_in_the_gate() {
        let _globals = globals();
        super::set_lifecycle_ready(true);
        super::SENDER.store(0x2000, Ordering::Release);
        super::CLOSED.store(false, Ordering::Release);
        super::IN_FLIGHT.store(1, Ordering::Release);
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                super::forget_sender(0x2000);
                super::IN_FLIGHT.load(Ordering::Acquire)
            });
            std::thread::sleep(std::time::Duration::from_millis(20));
            assert_eq!(super::sender(), 0, "the pointer is dropped first");
            super::IN_FLIGHT.store(0, Ordering::Release);
            assert_eq!(waiter.join().expect("waiter"), 0);
        });
        super::set_lifecycle_ready(false);
    }

    #[test]
    fn an_override_replaces_the_native_light_until_cleared() {
        let _globals = globals();
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
        let _globals = globals();
        clear_pad_rgbs();
        super::set_lifecycle_ready(true);
        let vtable = crate::image::base() + crate::firmware::LIGHTING_SENDER_VTABLE.offset;
        crate::image::test_memory::with(vec![(0x2000, vtable.to_le_bytes().to_vec())], || {
            super::observe(0x2000, 4, 0x0011_2233);
        });
        assert_eq!(super::sender(), 0x2000);
        super::forget_sender(0x3000);
        assert_eq!(super::sender(), 0x2000);
        crate::image::test_memory::with(
            vec![(0x3000, 0x4000_usize.to_le_bytes().to_vec())],
            || {
                super::observe(0x3000, 4, 0x0011_2233);
            },
        );
        assert_eq!(
            super::sender(),
            0,
            "untracked sender must not reuse the old one"
        );
        crate::image::test_memory::with(vec![(0x2000, vtable.to_le_bytes().to_vec())], || {
            super::observe(0x2000, 4, 0x0011_2233);
        });
        super::forget_sender(0x2000);
        assert_eq!(super::sender(), 0);
        super::set_lifecycle_ready(false);
        assert_eq!(super::NATIVE[4].load(Ordering::Acquire), 0x0011_2233);
        clear_pad_rgbs();
    }

    #[test]
    fn rgb_is_full_brightness_and_zero_is_valid() {
        let _globals = globals();
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
        let _globals = globals();
        assert!(!set_pad_rgb(PADS as u8, 0));
        assert!(!set_pad_rgb(0, 0x01_00_00_00));
        assert!(!set_pad_light(0, 0, f32::NAN));
        assert_eq!(override_light(PADS as u8), None);
    }
}
