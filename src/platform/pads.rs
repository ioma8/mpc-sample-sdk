//! Pad occupancy as the app's own pad finder sees it.
//!
//! Extract needs a free pad, and the mod has to know how many there are before
//! and after it runs an extraction: the difference is the only reliable signal
//! that the app actually assigned the sample.

use crate::image;

/// The app's first-free-pad finder.
const FREE_PAD: usize = crate::firmware::EXTRACT_FREE_PAD.offset;
/// The app's "is this pad state busy" entry.
const PAD_BUSY: usize = crate::firmware::PAD_BUSY.offset;
/// `[program + 0x2808]` is the first pad state, `+0x2810` the last
/// (`+0x28b0` / `+0x28b8` on 1.4.0.8).
#[cfg(feature = "fw-1-4")]
const PAD_STATES: usize = 0x28b0;
#[cfg(feature = "fw-1-3")]
const PAD_STATES: usize = 0x2808;
/// The app never has more pads than this.
const MAX_PADS: usize = 128;
/// `[surface + 0x2808]` is the first pad state, `+0x2810` the last
/// (`+0x28b0` / `+0x28b8` on 1.4.0.8).
#[cfg(feature = "fw-1-4")]
const SURFACE_PADS: usize = 0x28b0;
#[cfg(feature = "fw-1-3")]
const SURFACE_PADS: usize = 0x2808;
#[cfg(feature = "fw-1-4")]
const SURFACE_PADS_END: usize = 0x28b8;
#[cfg(feature = "fw-1-3")]
const SURFACE_PADS_END: usize = 0x2810;
/// The surface layout of the other stock build, tried when the profile's
/// offset does not describe a pad table. The surface object cannot be reached
/// in the lab, so this proves the table instead of trusting the guess.
#[cfg(feature = "fw-1-4")]
const ALTERNATE_SURFACE_PADS: usize = 0x2808;
#[cfg(feature = "fw-1-3")]
const ALTERNATE_SURFACE_PADS: usize = 0x28b0;
#[cfg(feature = "fw-1-4")]
const ALTERNATE_SURFACE_PADS_END: usize = 0x2810;
#[cfg(feature = "fw-1-3")]
const ALTERNATE_SURFACE_PADS_END: usize = 0x28b8;

/// The surface's pad table, validated: an ordered vector of at most
/// [`MAX_PADS`] pointers.
pub(crate) fn surface_pad_table(surface: usize) -> Option<(usize, usize)> {
    for (step, (begin_offset, end_offset)) in [
        (SURFACE_PADS, SURFACE_PADS_END),
        (ALTERNATE_SURFACE_PADS, ALTERNATE_SURFACE_PADS_END),
    ]
    .into_iter()
    .enumerate()
    {
        let (Some(begin), Some(end)) = (
            image::read_pointer(surface.checked_add(begin_offset)?),
            image::read_pointer(surface.checked_add(end_offset)?),
        ) else {
            continue;
        };
        if begin == 0 || end <= begin {
            continue;
        }
        let bytes = end.checked_sub(begin)?;
        if bytes % 8 != 0 || bytes / 8 > MAX_PADS {
            continue;
        }
        // The profile's offset is trusted; the other build's offset is only a
        // fallback, and a parameter or listener vector has the same shape as a
        // pad table, so prove its first slots hold live pad objects.
        if step == 0 {
            return Some((begin, end));
        }
        let count = bytes / 8;
        let mut plausible = true;
        for index in 0..count.min(4) {
            match image::read_u64(begin + index * 8) {
                Some(0) => {}
                Some(value) if image::is_app_object(value as usize) => {}
                _ => {
                    plausible = false;
                    break;
                }
            }
        }
        if plausible {
            return Some((begin, end));
        }
    }
    None
}
/// `InstrumentLayer::active`, confirmed in the firmware's native serializer.
const LAYER_ACTIVE: usize = 0xb18;

/// The app's first free pad, or `None` when all pads are busy.
///
/// # Safety
///
/// `program` must be the live program object on its owning UI thread.
#[must_use]
pub unsafe fn first_free_pad(program: usize) -> Option<i32> {
    // 1.4.0.8 returns `{u32 index, u8 found}` in x0; the app's own callers test
    // byte 4 before using the index. 1.3.0.12 returns a plain `int`.
    #[cfg(feature = "fw-1-4")]
    let pad: i64 = {
        #[repr(C)]
        struct FreePad {
            index: u32,
            found: u8,
            padding: [u8; 3],
        }
        // SAFETY: `base + FREE_PAD` is the entry the app itself calls with its
        // program object, and the caller guarantees the object.
        let find: unsafe extern "C" fn(usize) -> FreePad =
            unsafe { core::mem::transmute(image::base() + FREE_PAD) };
        // SAFETY: as above.
        let result = unsafe { find(program) };
        if result.found == 0 {
            return None;
        }
        i64::from(result.index)
    };
    #[cfg(feature = "fw-1-3")]
    let pad: i64 = {
        // SAFETY: `base + FREE_PAD` is the entry the app itself calls with its
        // program object, and the caller guarantees the object.
        let pad = unsafe { image::int_of_pointer(image::base() + FREE_PAD, program) };
        i64::from(pad)
    };
    usize::try_from(pad)
        .ok()
        .filter(|index| *index < MAX_PADS)
        .and_then(|index| i32::try_from(index).ok())
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

/// The program's pad slot, or `None` when the vector or index is unavailable.
///
/// The slot contains an instrument, which can remain unchanged when its sample changes.
#[must_use]
pub fn pad_slot(program: usize, index: i32) -> Option<usize> {
    let begin = image::read_pointer(program.checked_add(PAD_STATES)?)?;
    let end = image::read_pointer(program.checked_add(PAD_STATES + 8)?)?;
    if begin == 0 || end <= begin {
        return None;
    }
    let index = usize::try_from(index).ok()?;
    if index >= MAX_PADS || (end - begin) / 8 <= index {
        return None;
    }
    image::read_u64(begin + index * 8).map(|slot| slot as usize)
}

/// Read a pad's sample through the program, without requiring a surface event.
pub(crate) fn program_sample(program: usize, index: i32) -> Option<usize> {
    let state = pad_slot(program, index).filter(|state| *state != 0)?;
    let layers = super::descent::State::at(state).layers()?;
    let end = image::read_pointer(state.checked_add(super::descent::STATE_LAYERS_OFFSET + 8)?)?;
    if layers.address() == 0 || end <= layers.address() {
        return None;
    }
    layers.sample().filter(|sample| *sample != 0)
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
    let (array, end) = surface_pad_table(surface)?;
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
    let (array, end) = surface_pad_table(surface)?;
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
    let (array, end) = surface_pad_table(surface)?;
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
    // SAFETY: forwarded from the caller.
    unsafe { set_active_layer(layer, active) }
}

/// Enable or disable a pad through its live program's pad table.
///
/// # Safety
/// The program and its pad must be live and exclusively accessible on the UI thread.
#[must_use]
pub unsafe fn set_program_layer_active(program: usize, pad: i32, active: bool) -> bool {
    let Some(layer) = pad_slot(program, pad)
        .and_then(|state| super::descent::State::at(state).layers())
        .map(super::descent::Layers::address)
    else {
        return false;
    };
    // SAFETY: forwarded from the caller.
    unsafe { set_active_layer(layer, active) }
}

unsafe fn set_active_layer(layer: usize, active: bool) -> bool {
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
    use super::{
        ALTERNATE_SURFACE_PADS, ALTERNATE_SURFACE_PADS_END, MAX_PADS, PAD_STATES, SURFACE_PADS,
        SURFACE_PADS_END, has_room, sample_for_pad, surface_pad_table,
    };

    #[test]
    fn an_unknown_program_has_no_proven_room() {
        // SAFETY: no program is handed to native code in either case.
        unsafe {
            assert!(!has_room(None, 1));
            assert!(!has_room(Some(0), 1));
        }
    }

    #[test]
    fn the_surface_pad_table_is_probed() {
        use crate::image::test_memory;

        fn put(bytes: &mut [u8], offset: usize, value: u64) {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }

        let surface = 0x1000_0000_usize;
        let mut bytes = vec![0_u8; 0x2900];

        // Profile table: an empty (all-null) table inside the fixture.
        put(&mut bytes, SURFACE_PADS, (surface + 0x1000) as u64);
        put(&mut bytes, SURFACE_PADS_END, (surface + 0x1008) as u64);
        assert_eq!(
            test_memory::with(vec![(surface, bytes.clone())], || surface_pad_table(
                surface
            )),
            Some((surface + 0x1000, surface + 0x1008))
        );

        // Profile slot empty, the other layout's slot valid.
        put(&mut bytes, SURFACE_PADS, 0);
        put(&mut bytes, SURFACE_PADS_END, 0);
        put(
            &mut bytes,
            ALTERNATE_SURFACE_PADS,
            (surface + 0x1100) as u64,
        );
        put(
            &mut bytes,
            ALTERNATE_SURFACE_PADS_END,
            (surface + 0x1108) as u64,
        );
        assert_eq!(
            test_memory::with(vec![(surface, bytes.clone())], || surface_pad_table(
                surface
            )),
            Some((surface + 0x1100, surface + 0x1108))
        );

        // An element that is not a live app object rejects the offset.
        put(
            &mut bytes,
            ALTERNATE_SURFACE_PADS,
            (surface + 0x1200) as u64,
        );
        put(
            &mut bytes,
            ALTERNATE_SURFACE_PADS_END,
            (surface + 0x1210) as u64,
        );
        put(&mut bytes, 0x1200, 1);
        assert_eq!(
            test_memory::with(vec![(surface, bytes.clone())], || surface_pad_table(
                surface
            )),
            None
        );

        // An inverted range is rejected too.
        put(&mut bytes, 0x1200, 0);
        put(
            &mut bytes,
            ALTERNATE_SURFACE_PADS,
            (surface + 0x1210) as u64,
        );
        put(
            &mut bytes,
            ALTERNATE_SURFACE_PADS_END,
            (surface + 0x1200) as u64,
        );
        assert_eq!(
            test_memory::with(vec![(surface, bytes)], || surface_pad_table(surface)),
            None
        );
    }

    #[test]
    fn the_offsets_are_the_documented_ones() {
        assert_eq!(
            PAD_STATES,
            if cfg!(feature = "fw-1-4") {
                0x28b0
            } else {
                0x2808
            }
        );
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
    fn program_pad_exposes_sample_when_instrument_slot_stays_the_same() {
        use crate::image::test_memory;
        let (program, table, state, layers, sample) = (
            0x0080_0000_usize,
            0x0081_0000_usize,
            0x0082_0000_usize,
            0x0083_0000_usize,
            0x0084_0000_usize,
        );
        let pointer = |address: usize| address.to_le_bytes().to_vec();
        let regions = vec![
            (program + PAD_STATES, pointer(table)),
            (program + PAD_STATES + 8, pointer(table + 8)),
            (table, pointer(state)),
            (
                state + crate::platform::descent::STATE_LAYERS_OFFSET,
                pointer(layers),
            ),
            (
                state + crate::platform::descent::STATE_LAYERS_OFFSET + 8,
                pointer(layers),
            ),
            (
                layers + crate::platform::descent::LAYERS_SAMPLE_OFFSET,
                pointer(sample),
            ),
        ];
        assert_eq!(
            test_memory::with(regions.clone(), || super::program_sample(program, 0)),
            None
        );
        let mut regions = regions;
        regions
            .iter_mut()
            .find(|(address, _)| {
                *address == state + crate::platform::descent::STATE_LAYERS_OFFSET + 8
            })
            .unwrap()
            .1 = pointer(layers + 0xc90);
        assert_eq!(
            test_memory::with(regions, || super::program_sample(program, 0)),
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
