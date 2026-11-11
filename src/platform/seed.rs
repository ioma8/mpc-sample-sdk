//! Native silent sample borrowing for Extract.

use crate::image;
use crate::platform::{control, pads, sample, selection};
use std::sync::{Mutex, MutexGuard, PoisonError};

mod layout {
    pub(super) const SURFACE_PAD_LIST: usize = 0x2808;
    pub(super) const SURFACE_PAD_LIST_END: usize = 0x2810;
    pub(super) const STATE_LAYERS: usize = 0x22e8;
    pub(super) const LAYERS_SAMPLE: usize = 0x4a8;
    pub(super) const HOLDER_FROM_SAMPLE: usize = 0x4b0 - LAYERS_SAMPLE;
}

/// The live sample/holder borrowed while Extract runs (a silent seed), plus the
/// pads that were empty when the borrow was made.
///
/// Only the UI callbacks touch this, and the SDK serialises them, so one mutex
/// is both the state and the lock instead of four atomics that could be observed
/// as a torn record.
#[derive(Clone, Copy)]
struct Borrow {
    /// The layer slot that was overwritten.
    slot: usize,
    /// The sample the slot held before the borrow.
    sample: usize,
    /// The holder the slot held before the borrow.
    holder: usize,
    /// The sample that was borrowed.
    provider: usize,
    /// Pads that were empty when the borrow was made.
    empty: [bool; 128],
}

impl Borrow {
    const fn new() -> Self {
        Self {
            slot: 0,
            sample: 0,
            holder: 0,
            provider: 0,
            empty: [false; 128],
        }
    }
}

/// The borrow record; the mutex makes the four pointers one consistent state.
static BORROW: Mutex<Borrow> = Mutex::new(Borrow::new());

/// Lock the borrow record, recovering from a poisoned mutex.
fn borrow_state() -> MutexGuard<'static, Borrow> {
    BORROW.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Write one pointer-sized field of a live sample/layer object.
fn write_slot(slot: usize, value: u64) -> bool {
    // SAFETY: `slot` is a field of the app's live sample/layer object and the
    // value is a sample or holder pointer the app itself stores there.
    unsafe { image::write(slot, &value.to_le_bytes()) }
}

/// Put the slot back to the sample/holder the page had before the borrow.
fn restore_borrowed(borrow: &mut Borrow) -> bool {
    let restored = borrow.slot == 0 || {
        let sample = write_slot(borrow.slot, borrow.sample as u64);
        let holder = write_slot(
            borrow.slot + layout::HOLDER_FROM_SAMPLE,
            borrow.holder as u64,
        );
        sample && holder
    };
    borrow.slot = 0;
    borrow.sample = 0;
    borrow.holder = 0;
    borrow.provider = 0;
    restored
}

/// Restore the borrow when the slot still points at the borrowed provider, and
/// return the provider that was borrowed.
fn clear_borrow() -> (usize, bool) {
    let mut borrow = borrow_state();
    let provider = borrow.provider;
    let restored = if borrow.slot == 0 {
        true
    } else {
        match image::read_u64(borrow.slot) {
            Some(current) if current == provider as u64 => restore_borrowed(&mut borrow),
            Some(_) => {
                borrow.slot = 0;
                borrow.sample = 0;
                borrow.holder = 0;
                borrow.provider = 0;
                true
            }
            None => false,
        }
    };
    (provider, restored)
}

/// Record only pads whose empty state can be read before borrowing.
fn capture_empty_pads(borrow: &mut Borrow, surface: usize) {
    borrow.empty.fill(false);
    let (Some(first), Some(end)) = (
        image::read_pointer(surface + layout::SURFACE_PAD_LIST),
        image::read_pointer(surface + layout::SURFACE_PAD_LIST_END),
    ) else {
        return;
    };
    for (index, item) in borrow
        .empty
        .iter_mut()
        .enumerate()
        .take((end.saturating_sub(first) / 8).min(128))
    {
        let Some(state) = image::read_u64(first + index * 8) else {
            continue;
        };
        if state == 0 {
            *item = true;
            continue;
        }
        let Some(layers) = image::read_pointer(state as usize + layout::STATE_LAYERS) else {
            continue;
        };
        *item = image::read_u64(layers + layout::LAYERS_SAMPLE) == Some(0);
    }
}

/// Borrow a native silent seed for Extract.
///
/// # Safety
/// The screen and surface must remain live on the owning UI thread until cleanup.
#[must_use]
pub unsafe fn seed_provider(screen: usize) -> Option<(usize, u32)> {
    let mut borrow = borrow_state();
    if borrow.slot != 0 {
        return None;
    }
    let state = selection::page_state(screen)?;
    let slot = image::read_pointer(state.checked_add(layout::STATE_LAYERS)?)?
        .checked_add(layout::LAYERS_SAMPLE)?;
    let old_sample = image::read_u64(slot)?;
    let old_holder = image::read_u64(slot + layout::HOLDER_FROM_SAMPLE)?;
    let seed = sample::silent_seed()?;
    let frames = seed.frames();
    capture_empty_pads(&mut borrow, control::surface());
    if !write_slot(slot, seed.sample() as u64) {
        return None;
    }
    if !write_slot(slot + layout::HOLDER_FROM_SAMPLE, seed.holder() as u64) {
        let _ = write_slot(slot, old_sample);
        return None;
    }
    borrow.provider = seed.sample();
    borrow.slot = slot;
    borrow.sample = old_sample as usize;
    borrow.holder = old_holder as usize;
    Some((state, frames))
}

/// Remove only new provider copies, preserving every previously populated pad.
fn clear_extra_provider_pads(provider: usize, destination: i32) {
    if provider == 0 {
        return;
    }
    let empty = borrow_state().empty;
    let surface = control::surface();
    for (index, was_empty) in empty.iter().enumerate() {
        let Ok(pad) = i32::try_from(index) else {
            continue;
        };
        if *was_empty && pad != destination && pads::sample_for_pad(surface, pad) == Some(provider)
        {
            // SAFETY: `pad` is a live pad on the surface recorded by the
            // control hook, on its owning UI thread.
            unsafe { control::clear_pad(pad) };
        }
    }
}

/// Restore the borrowed sample and remove extra seed copies.
///
/// # Safety
/// Call on the owning UI thread while the original page and surface remain live.
/// Returns false if the original layer could not be restored.
#[must_use]
pub unsafe fn finish(destination: i32) -> bool {
    let (provider, restored) = clear_borrow();
    if restored {
        clear_extra_provider_pads(provider, destination);
    }
    restored
}

#[cfg(test)]
mod tests {
    use super::layout::{HOLDER_FROM_SAMPLE, LAYERS_SAMPLE};

    #[test]
    fn holder_is_adjacent_to_the_sample_slot() {
        assert_eq!(LAYERS_SAMPLE + HOLDER_FROM_SAMPLE, 0x4b0);
    }
}
