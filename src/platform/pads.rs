//! Pad occupancy as the app's own pad finder sees it.
//!
//! Extract needs a free pad, and the mod has to know how many there are before
//! and after it runs an extraction: the difference is the only reliable signal
//! that the app actually assigned the sample.

use crate::image;

/// The app's "count free pads" entry, as an offset into the program object's
/// vtable call.
const FREE_PAD: usize = 0x286_dd00;
/// The app's "is this pad state busy" entry.
const PAD_BUSY: usize = 0x280_47b0;
/// `[program + 0x2808]` is the first pad state, `+0x2810` the last.
const PAD_STATES: usize = 0x2808;
/// The app never has more pads than this.
const MAX_PADS: usize = 128;
/// `[surface + 0x2808]` is the first pad state, `+0x2810` the last.
const SURFACE_PADS: usize = 0x2808;
const SURFACE_PADS_END: usize = 0x2810;
/// `[pad state + 0x22e8]` is the layer list, `[layers + 0x4a8]` its sample.
const STATE_LAYERS: usize = 0x22e8;
const LAYER_SAMPLE: usize = 0x4a8;
/// `InstrumentLayer::active`, confirmed in the firmware's native serializer.
const LAYER_ACTIVE: usize = 0xb18;

/// The app's count of free pads.
#[must_use]
pub fn free_count(program: usize) -> i32 {
    // SAFETY: `base + FREE_PAD` is the entry the app itself calls with its
    // program object.
    unsafe { image::int_of_pointer(image::base() + FREE_PAD, program) }
}

/// Whether a known program has at least `required` free pads.
///
/// An unknown program (`None` or zero) cannot be proven to have room, so the
/// answer is `false`; the caller reports the same "need N empty pads" message.
#[must_use]
pub fn has_room(program: Option<usize>, required: usize) -> bool {
    program
        .filter(|program| *program != 0)
        .is_some_and(|program| free_estimate(program) >= required)
}

/// How many pads the app's pad states say are free.
///
/// The same two checks the native finder uses: a null pad state, or a pad state
/// the app reports as not busy.
#[must_use]
pub fn free_estimate(program: usize) -> usize {
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
        let Some(state) = image::read_pointer(first + index * 8) else {
            break;
        };
        if state == 0 {
            free += 1;
            continue;
        }
        // SAFETY: `base + PAD_BUSY` is the app's own predicate for a pad state.
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
    image::read_pointer(state.checked_add(STATE_LAYERS)?)
}

/// The sample a pad holds, read through the surface's own tables.
#[must_use]
pub fn sample_for_pad(surface: usize, pad: i32) -> Option<usize> {
    let layer = layer_for_pad(surface, pad)?;
    image::read_pointer(layer.checked_add(LAYER_SAMPLE)?).filter(|sample| *sample != 0)
}

/// Enable or disable a populated pad's sample layer.
///
/// Returns `false` if the pad table, layer, or memory write is unavailable. The
/// update is visible to pad playback immediately and is saved with the project.
#[must_use]
pub fn set_layer_active(surface: usize, pad: i32, active: bool) -> bool {
    let Some(layer) = layer_for_pad(surface, pad) else {
        return false;
    };
    let Some(sample_address) = layer.checked_add(LAYER_SAMPLE) else {
        return false;
    };
    if image::read_pointer(sample_address).is_none_or(|sample| sample == 0) {
        return false;
    }
    let Some(address) = layer.checked_add(LAYER_ACTIVE) else {
        return false;
    };
    let expected = [u8::from(active)];
    if !image::write(address, &expected) {
        return false;
    }
    let mut actual = [0];
    image::read(address, &mut actual) && actual == expected
}

#[cfg(test)]
mod tests {
    use super::{FREE_PAD, MAX_PADS, PAD_BUSY, PAD_STATES, has_room};

    #[test]
    fn an_unknown_program_has_no_proven_room() {
        assert!(!has_room(None, 1));
        assert!(!has_room(Some(0), 1));
    }

    #[test]
    fn the_offsets_are_the_documented_ones() {
        assert_eq!(FREE_PAD, 0x286_dd00);
        assert_eq!(PAD_BUSY, 0x280_47b0);
        assert_eq!(PAD_STATES, 0x2808);
        assert_eq!(MAX_PADS, 128);
    }
}
