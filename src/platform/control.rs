//! The app's control-surface setter, recorded and replayed.
//!
//! The Sample page drives its trim through this entry and passes it the live
//! surface object.  The SDK remembers that object and the displaced setter, so a
//! mod can apply a control change exactly as the page would.

use crate::image;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Firmware operation for clearing a pad.
pub const PAD_CLEAR: i32 = 0x40d;
/// Firmware operation for the trim start frame.
pub const TRIM_START: i32 = 0x462;
/// Firmware operation for the exclusive trim end frame.
pub const TRIM_END: i32 = 0x466;

/// The app's control setter, as the page's knobs call it.
pub type SetControl = unsafe extern "C" fn(usize, usize, i32, i32, f32) -> usize;

/// The setter the SDK displaced.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The control context that came with the surface.
static CONTROL: AtomicUsize = AtomicUsize::new(0);

/// The control-surface object the app last handed to the setter.
#[unsafe(no_mangle)]
pub static g_surface: AtomicUsize = AtomicUsize::new(0);

/// How far past the load address a vtable can point and still be the app's.
const IMAGE_SPAN: usize = 0x1200_0000;

/// Record the displaced setter, at install time.
///
/// # Safety
///
/// Called once, before the hook can run.
pub unsafe fn record(original: usize) {
    ORIGINAL.store(original, Ordering::Release);
}

/// The app's setter, as a hook: learn the live surface, then pass the call on.
///
/// # Safety
///
/// The app calls this with its own objects.
pub unsafe extern "C" fn hook(
    control: usize,
    surface: usize,
    id: i32,
    op: i32,
    value: f32,
) -> usize {
    let base = image::base();
    let looks_live = surface != 0
        && image::read_pointer(surface)
            .is_some_and(|vtable| (base..=base + IMAGE_SPAN).contains(&vtable));
    // SAFETY: only the UI thread calls the setter.
    unsafe {
        if looks_live {
            g_surface.store(surface, Ordering::Release);
            CONTROL.store(control, Ordering::Release);
        }
        // The setter tail-calls the app's own, so its result is its caller's.
        match original() {
            Some(original) => original(control, surface, id, op, value),
            None => 0,
        }
    }
}

/// The surface the page last used, or zero.
#[must_use]
pub fn surface() -> usize {
    g_surface.load(Ordering::Acquire)
}

/// The control context the page last used, or zero.
#[must_use]
pub fn control() -> usize {
    CONTROL.load(Ordering::Acquire)
}

/// The displaced setter, if the site was installed.
#[must_use]
pub fn original() -> Option<SetControl> {
    let address = ORIGINAL.load(Ordering::Acquire);
    // SAFETY: record requires the matching native setter signature.
    (address != 0).then(|| unsafe { core::mem::transmute::<usize, SetControl>(address) })
}

/// Clear a pad through the native control surface.
///
/// # Safety
/// The remembered surface must be live on the UI thread and `pad` valid.
pub unsafe fn clear_pad(pad: i32) {
    // SAFETY: guaranteed by the caller.
    let _ = unsafe { set(pad, PAD_CLEAR, 0.0) };
}

/// Set the exclusive trim end on the remembered surface. Frame values above
/// 2^24 may be rounded by the firmware's floating-point control API.
///
/// # Safety
/// The remembered surface must be live on the UI thread and `pad` valid.
pub unsafe fn set_trim_end(pad: i32, end: u32) {
    // SAFETY: guaranteed by the caller.
    let _ = unsafe { set(pad, TRIM_END, end as f32) };
}

/// Set trim bounds using the ordering expected by the firmware.
///
/// # Safety
/// Both objects must be live on the UI thread, with a valid pad and bounds.
pub unsafe fn set_trim(control: usize, surface: usize, pad: i32, first: u32, end: u32) {
    // SAFETY: guaranteed by the caller; reset start before changing end.
    unsafe {
        let _ = apply(control, surface, pad, TRIM_START, 0.0);
        let _ = apply(control, surface, pad, TRIM_END, end as f32);
        let _ = apply(control, surface, pad, TRIM_START, first as f32);
    }
}

/// Apply one control op to an explicit surface and context.
///
/// # Safety
/// Objects must be live, matching native objects on their owning UI thread;
/// the pad, operation and value must be valid for the firmware.
///
/// Returns zero when the setter is not installed.
#[must_use]
pub unsafe fn apply(control: usize, surface: usize, pad: i32, op: i32, value: f32) -> usize {
    match original() {
        // SAFETY: these are the app's own setter and objects.
        Some(original) => unsafe { original(control, surface, pad, op, value) },
        None => 0,
    }
}

/// Apply one control op to `pad` on the surface the page last used.
///
/// # Safety
/// The remembered objects must remain live on their owning UI thread;
/// the pad, operation and value must be valid for the firmware.
///
/// Returns zero when the setter is not installed or the surface is unknown.
#[must_use]
pub unsafe fn set(pad: i32, op: i32, value: f32) -> usize {
    let (surface, control, original) = (surface(), control(), original());
    match original {
        // SAFETY: these are the app's own setter and objects.
        Some(original) if surface != 0 => unsafe { original(control, surface, pad, op, value) },
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{control, original, set, surface};

    #[test]
    fn an_uninstalled_setter_is_harmless() {
        assert!(original().is_none());
        // SAFETY: the uninstalled setter returns without calling firmware.
        assert_eq!(unsafe { set(0, 0x462, 0.0) }, 0);
        let _ = (surface(), control());
    }
}
