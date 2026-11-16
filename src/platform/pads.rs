//! Pad occupancy as the app's own pad finder sees it.
//!
//! Extract needs a free pad, and the mod has to know how many there are before
//! and after it runs an extraction: the difference is the only reliable signal
//! that the app actually assigned the sample.

use crate::image;

/// The app's first-free-pad finder.
const FREE_PAD: usize = crate::firmware::FREE_PAD.offset;
/// The app's "is this pad state busy" entry.
const PAD_BUSY: usize = crate::firmware::PAD_BUSY.offset;
/// `[program + 0x2808]` is the first pad state, `+0x2810` the last.
const PAD_STATES: usize = 0x2808;
/// The app never has more pads than this.
const MAX_PADS: usize = 128;
/// `[surface + 0x2808]` is the first pad state, `+0x2810` the last.
const SURFACE_PADS: usize = 0x2808;
const SURFACE_PADS_END: usize = 0x2810;
/// `InstrumentLayer::active`, confirmed in the firmware's native serializer.
const LAYER_ACTIVE: usize = 0xb18;

/// The app's first free pad, or `None` when all pads are busy.
///
/// # Safety
///
/// `program` must be the live program object on its owning UI thread.
#[must_use]
pub unsafe fn first_free_pad(program: usize) -> Option<i32> {
    // SAFETY: `base + FREE_PAD` is the entry the app itself calls with its
    // program object, and the caller guarantees the object.
    let pad = unsafe { image::int_of_pointer(image::base() + FREE_PAD, program) };
    usize::try_from(pad)
        .is_ok_and(|index| index < MAX_PADS)
        .then_some(pad)
}

/// Whether a known program has at least `required` free pads.
///
/// An unknown program (`None` or zero) cannot be proven to have room, so the
/// answer is `false`; the caller reports the same "need N empty pads" message.
///
/// # Safety
///
/// A supplied program must be the live program object on its owning UI thread.
#[must_use]
pub unsafe fn has_room(program: Option<usize>, required: usize) -> bool {
    program
        .filter(|program| *program != 0)
        // SAFETY: the caller guarantees the live program object.
        .is_some_and(|program| unsafe { free_count(program) } >= required)
}

/// How many pads the app considers free.
///
/// Uses the same busy predicate as the app's first-free-pad finder.
///
/// # Safety
///
/// `program` must be the live program object on its owning UI thread.
#[must_use]
pub unsafe fn free_count(program: usize) -> usize {
    let Some(first) = image::read_pointer(program + PAD_STATES) else {
        return 0;
    };
    let Some(last) = image::read_pointer(program + PAD_STATES + 8) else {
        return 0;
    };
    if first == 0 || last < first {
        return 0;
    }
    let count = ((last - first) / 8).min(MAX_PADS);
    let mut free = 0;
    for index in 0..count {
        // A null slot is an empty pad, not the end of the table.
        let Some(state) = image::read_u64(first + index * 8) else {
            break;
        };
        let Ok(state) = usize::try_from(state) else {
            break;
        };
        if state == 0 {
            free += 1;
            continue;
        }
        // SAFETY: the caller supplied the live program, and `state` is read out
        // of its own pad table; `base + PAD_BUSY` is the app's own predicate.
        let busy = unsafe { image::int_of_pointer(image::base() + PAD_BUSY, state) };
        if busy == 0 {
            free += 1;
        }
    }
    free
}

/// The layer object at a pad, read through the surface's own tables.
fn layer_for_pad(surface: usize, pad: i32) -> Option<usize> {
    if surface == 0 {
        return None;
    }
    let array = image::read_pointer(surface.checked_add(SURFACE_PADS)?)?;
    let end = image::read_pointer(surface.checked_add(SURFACE_PADS_END)?)?;
    if array == 0 || end < array {
        return None;
    }
    let index = usize::try_from(pad).ok()?;
    if index >= MAX_PADS || (end - array) / 8 <= index {
        return None;
    }
    let state = image::read_pointer(array.checked_add(index.checked_mul(8)?)?)?;
    if state == 0 {
        return None;
    }
    super::descent::State::at(state)
        .layers()
        .map(super::descent::Layers::address)
}

/// The pad whose state is `state`, read through the surface's own tables.
///
/// The page's selected instrument state is the same object as one entry in the
/// surface's pad table, so this resolves the selected pad without relying on a
/// recent pad press.
#[must_use]
pub fn pad_for_state(surface: usize, state: usize) -> Option<i32> {
    if surface == 0 || state == 0 {
        return None;
    }
    let array = image::read_pointer(surface.checked_add(SURFACE_PADS)?)?;
    let end = image::read_pointer(surface.checked_add(SURFACE_PADS_END)?)?;
    if array == 0 || end < array {
        return None;
    }
    let count = ((end - array) / 8).min(MAX_PADS);
    for index in 0..count {
        if image::read_pointer(array.checked_add(index.checked_mul(8)?)?) == Some(state) {
            return i32::try_from(index).ok();
        }
    }
    None
}

/// The first pad whose sample is `sample`.
///
/// This is the fallback when the selected state is not in the surface table
/// (for example a temporary page state); the sample identity is unique enough
/// for the trim target.
#[must_use]
pub fn pad_for_sample(surface: usize, sample: usize) -> Option<i32> {
    if surface == 0 || sample == 0 {
        return None;
    }
    let array = image::read_pointer(surface.checked_add(SURFACE_PADS)?)?;
    let end = image::read_pointer(surface.checked_add(SURFACE_PADS_END)?)?;
    if array == 0 || end < array {
        return None;
    }
    let count = ((end - array) / 8).min(MAX_PADS);
    for index in 0..count {
        let pad = i32::try_from(index).ok()?;
        if sample_for_pad(surface, pad) == Some(sample) {
            return Some(pad);
        }
    }
    None
}

/// The sample a pad holds, read through the surface's own tables.
#[must_use]
pub fn sample_for_pad(surface: usize, pad: i32) -> Option<usize> {
    let layer = layer_for_pad(surface, pad)?;
    super::descent::Layers::at(layer)
        .sample()
        .filter(|sample| *sample != 0)
}

/// Whether a pad has no sample, or `None` if its state cannot be read.
#[must_use]
pub fn pad_is_empty(surface: usize, pad: i32) -> Option<bool> {
    let layer = layer_for_pad(surface, pad)?;
    let slot = super::descent::Layers::at(layer).sample_slot()?;
    image::read_u64(slot).map(|sample| sample == 0)
}

/// Enable or disable a populated pad's sample layer.
///
/// Returns `false` if the pad table, layer, or memory write is unavailable. The
/// update is visible to pad playback immediately and is saved with the project.
///
/// # Safety
///
/// `surface` must be the live control surface on its owning UI thread, and the
/// caller must have exclusive access to the layer while it is written.
#[must_use]
pub unsafe fn set_layer_active(surface: usize, pad: i32, active: bool) -> bool {
    let Some(layer) = layer_for_pad(surface, pad) else {
        return false;
    };
    let Some(sample_address) = super::descent::Layers::at(layer).sample_slot() else {
        return false;
    };
    if image::read_pointer(sample_address).is_none_or(|sample| sample == 0) {
        return false;
    }
    let Some(address) = layer.checked_add(LAYER_ACTIVE) else {
        return false;
    };
    let expected = [u8::from(active)];
    // SAFETY: the caller guarantees the live layer and exclusive access.
    if !unsafe { image::write(address, &expected) } {
        return false;
    }
    let mut actual = [0];
    image::read(address, &mut actual) && actual == expected
}

#[cfg(test)]
mod tests {
    use super::{MAX_PADS, PAD_STATES, SURFACE_PADS, SURFACE_PADS_END, has_room, sample_for_pad};

    #[test]
    fn an_unknown_program_has_no_proven_room() {
        // SAFETY: no program is handed to native code in either case.
        unsafe {
            assert!(!has_room(None, 1));
            assert!(!has_room(Some(0), 1));
        }
    }

    #[test]
    fn the_offsets_are_the_documented_ones() {
        assert_eq!(PAD_STATES, 0x2808);
        assert_eq!(MAX_PADS, 128);
    }

    #[test]
    fn free_count_counts_the_empty_pad_slots_of_a_program() {
        use crate::image::test_memory;
        // A program whose pad table holds three empty slots.
        let program: usize = 0x0030_0000;
        let table: usize = 0x0040_0000;
        let regions = vec![
            (program + PAD_STATES, table.to_le_bytes().to_vec()),
            (
                program + PAD_STATES + 8,
                (table + 24).to_le_bytes().to_vec(),
            ),
            (table, vec![0_u8; 24]),
        ];
        // SAFETY: the fixture stands in for the program; no native call is made,
        // because every slot is empty.
        let free = test_memory::with(regions, || unsafe { super::free_count(program) });
        assert_eq!(free, 3);
    }

    #[test]
    fn a_pad_leads_through_its_state_and_layers_to_its_sample() {
        use crate::image::test_memory;
        let surface: usize = 0x0070_0000;
        let array: usize = 0x0071_0000;
        let state: usize = 0x0072_0000;
        let layers: usize = 0x0073_0000;
        let sample: usize = 0x0074_0000;
        let pointer = |address: usize| address.to_le_bytes().to_vec();
        let regions = vec![
            (surface + SURFACE_PADS, pointer(array)),
            (surface + SURFACE_PADS_END, pointer(array + 8)),
            (array, pointer(state)),
            (
                state + crate::platform::descent::STATE_LAYERS_OFFSET,
                pointer(layers),
            ),
            (
                layers + crate::platform::descent::LAYERS_SAMPLE_OFFSET,
                pointer(sample),
            ),
        ];
        assert_eq!(
            test_memory::with(regions, || sample_for_pad(surface, 0)),
            Some(sample)
        );
    }
    #[test]
    fn pad_emptiness_distinguishes_empty_occupied_and_unreadable() {
        use crate::image::test_memory;
        let (surface, array, state, layers) = (
            0x0070_0000_usize,
            0x0071_0000_usize,
            0x0072_0000_usize,
            0x0073_0000_usize,
        );
        let pointer = |value: usize| value.to_le_bytes().to_vec();
        let base = vec![
            (surface + SURFACE_PADS, pointer(array)),
            (surface + SURFACE_PADS_END, pointer(array + 8)),
            (array, pointer(state)),
            (
                state + crate::platform::descent::STATE_LAYERS_OFFSET,
                pointer(layers),
            ),
        ];
        assert_eq!(
            test_memory::with(base.clone(), || super::pad_is_empty(surface, 0)),
            None
        );
        for (sample, expected) in [(0, true), (0x0074_0000, false)] {
            let mut regions = base.clone();
            regions.push((
                layers + crate::platform::descent::LAYERS_SAMPLE_OFFSET,
                pointer(sample),
            ));
            assert_eq!(
                test_memory::with(regions, || super::pad_is_empty(surface, 0)),
                Some(expected)
            );
        }
    }
}
