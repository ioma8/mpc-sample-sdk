//! Every firmware address the SDK depends on, in one table.
//!
//! Each entry names one place in the stock app: a hook the SDK patches, a
//! function the SDK calls, data it reads, or a guard word that proves the image
//! is the tested firmware. Modules refer to these constants instead of spelling
//! addresses, so one firmware bump edits one file.
//!
//! An address has one name. Where the same address is both patched and called
//! (the string constructor), the hook's name is the only one.

/// What the SDK does with an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// The SDK patches this address with a branch.
    Hook,
    /// The SDK calls this address; it may be a data address for a vtable or font.
    Call,
    /// An image guard with no caller in this crate.
    Probe,
}

/// One firmware location.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Entry {
    /// Offset from the image base.
    pub(crate) offset: usize,
    /// What the SDK does with the address; read by the table's own tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) purpose: Purpose,
    /// The instruction word at `offset` in the tested firmware, when it has been
    /// fingerprinted. `None` means the address is recorded but not yet verified.
    pub(crate) word: Option<u32>,
}

const fn hook(offset: usize, word: u32) -> Entry {
    Entry {
        offset,
        purpose: Purpose::Hook,
        word: Some(word),
    }
}

const fn call(offset: usize) -> Entry {
    Entry {
        offset,
        purpose: Purpose::Call,
        word: None,
    }
}

const fn probe(offset: usize, word: u32) -> Entry {
    Entry {
        offset,
        purpose: Purpose::Probe,
        word: Some(word),
    }
}

/// `paciasp`, the usual first word of a function.
const PACIASP: u32 = 0xd503_233f;
/// `bti c`, the landing pad at the start of an indirect target.
const BTI_C: u32 = 0xd503_245f;

// --- Hooks: the 18 sites the SDK patches, in install order. ---

pub(crate) const SIXTEEN_CTOR: Entry = hook(0x0154_5a50, PACIASP);
pub(crate) const SIXTEEN_PROC: Entry = hook(0x0214_72f0, PACIASP);
pub(crate) const DATA_HOOK: Entry = hook(0x0140_dbc0, PACIASP);
pub(crate) const NOTE_ON: Entry = hook(0x01e2_7360, PACIASP);
pub(crate) const SURFACE: Entry = hook(0x023a_ff60, PACIASP);
pub(crate) const NOTE_OFF: Entry = hook(0x01e2_70d0, PACIASP);
pub(crate) const CC: Entry = hook(0x0159_f230, 0x3940_2003);
pub(crate) const PITCH: Entry = hook(0x027e_4400, PACIASP);
pub(crate) const BUTTONS: Entry = hook(0x0159_f2f0, BTI_C);
pub(crate) const REPAINT: Entry = hook(0x0114_9880, PACIASP);
/// The label constructor. Draw and Extract call this same address, and the
/// reentry guard stops the call from recursing into the hook.
pub(crate) const STRING_FROM_C: Entry = hook(0x00f5_35c0, PACIASP);
pub(crate) const EXTRACT_CTOR: Entry = hook(0x0143_ed40, PACIASP);
pub(crate) const EXTRACT_EXEC: Entry = hook(0x0143_f0b0, PACIASP);
pub(crate) const WRITER: Entry = hook(0x01d8_aff0, PACIASP);
pub(crate) const NOTE: Entry = hook(0x0211_b090, PACIASP);
pub(crate) const CONTROL: Entry = hook(0x0196_8020, PACIASP);
pub(crate) const SAMPLE_SERVICE_CTOR: Entry = hook(0x01df_3330, PACIASP);
pub(crate) const LIGHTING: Entry = hook(0x0159_f910, PACIASP);

// --- Calls: functions and data the SDK uses, recorded but not yet fingerprinted. ---

pub(crate) const SET_COLOUR: Entry = call(0x102_d5e0);
pub(crate) const FILL_RECT: Entry = call(0x102_e000);
pub(crate) const SET_FONT: Entry = call(0x102_dca0);
pub(crate) const DRAW_TEXT: Entry = call(0x108_17b0);
pub(crate) const FONT: Entry = call(0x108_2810);
pub(crate) const FONT_DROP: Entry = call(0x107_0d60);
pub(crate) const STRING_DROP: Entry = call(0xf0_a290);
pub(crate) const FREE_PAD: Entry = call(0x286_dd00);
pub(crate) const PAD_BUSY: Entry = call(0x280_47b0);
pub(crate) const SLICE_RESET: Entry = call(0x295_0aa0);
pub(crate) const SLICE_INSERT: Entry = call(0x295_0d20);
pub(crate) const SLICE_SET_END: Entry = call(0x295_0bb0);
pub(crate) const SELECT_SLICE: Entry = call(0x166_80a0);
pub(crate) const SET_CHOP_MODE: Entry = call(0x15d_2840);
pub(crate) const SAMPLE_PAGE_VTABLE: Entry = call(0x6e2_12a0);
pub(crate) const CHOP_PREDICATE: Entry = call(0x112_fd50);
pub(crate) const REVERB_FACTORY: Entry = call(0x03c4_45b0);
pub(crate) const VINYL_FACTORY: Entry = call(0x03c4_3cf0);
pub(crate) const DECIMATOR_FACTORY: Entry = call(0x03c4_57a0);
pub(crate) const BUILD_SAMPLE: Entry = call(0x01df_4ac0);
pub(crate) const RELEASE_SHARED: Entry = call(0x0123_bca0);
pub(crate) const EXTRACT_HANDLER: Entry = call(0x0165_80d0);

/// Offset inside the stock app's text segment, used to recognise which loaded
/// image is the app. Any `PT_LOAD` that covers it belongs to firmware 1.3.0.12.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const APP_PROBE_OFFSET: usize = 0x00b5_5b00;

/// Fingerprint words that no module calls directly. They are image guards that
/// the loader has always checked; keep them until someone audits them.
pub(crate) const GUARDS: [Entry; 20] = [
    probe(0x013b_b930, PACIASP),
    probe(0x014a_7580, PACIASP),
    probe(0x015e_70c0, PACIASP),
    probe(0x016b_6060, BTI_C),
    probe(0x0192_d040, PACIASP),
    probe(0x0195_cf40, PACIASP),
    probe(0x0195_d1c0, PACIASP),
    probe(0x0196_0af0, PACIASP),
    probe(0x0196_0d50, PACIASP),
    probe(0x020a_3b10, PACIASP),
    probe(0x020a_7140, PACIASP),
    probe(0x020a_9fa0, PACIASP),
    probe(0x020a_a9e0, PACIASP),
    probe(0x0280_4e90, PACIASP),
    probe(0x0280_dca0, PACIASP),
    probe(0x0281_00c0, PACIASP),
    probe(0x0281_2f10, BTI_C),
    probe(0x0281_4ec0, PACIASP),
    probe(0x0282_2260, PACIASP),
    probe(0x0294_1ff0, BTI_C),
];

/// The 18 hook sites, in install order.
pub(crate) const HOOKS: [Entry; 18] = [
    SIXTEEN_CTOR,
    SIXTEEN_PROC,
    DATA_HOOK,
    NOTE_ON,
    SURFACE,
    NOTE_OFF,
    CC,
    PITCH,
    BUTTONS,
    REPAINT,
    STRING_FROM_C,
    EXTRACT_CTOR,
    EXTRACT_EXEC,
    WRITER,
    NOTE,
    CONTROL,
    SAMPLE_SERVICE_CTOR,
    LIGHTING,
];

/// Every entry whose word has been fingerprinted, which the loader verifies.
pub(crate) fn fingerprints() -> impl Iterator<Item = (usize, u32)> {
    HOOKS
        .iter()
        .chain(GUARDS.iter())
        .filter_map(|entry| entry.word.map(|word| (entry.offset, word)))
}

#[cfg(test)]
mod tests {
    use super::{CC, GUARDS, HOOKS, Purpose, fingerprints};

    #[test]
    fn every_hook_is_fingerprinted_and_marked_as_a_hook() {
        for hook in HOOKS {
            assert_eq!(hook.purpose, Purpose::Hook);
            assert!(hook.word.is_some(), "hook at {:#x}", hook.offset);
        }
    }

    #[test]
    fn guards_are_probes_and_no_address_is_listed_twice() {
        assert!(GUARDS.iter().all(|guard| guard.purpose == Purpose::Probe));
        let mut offsets: Vec<usize> = HOOKS
            .iter()
            .chain(GUARDS.iter())
            .map(|e| e.offset)
            .collect();
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(offsets.len(), HOOKS.len() + GUARDS.len());
    }

    #[test]
    fn the_fingerprint_set_covers_the_original_thirty_eight_words() {
        assert_eq!(fingerprints().count(), 38);
        assert!(fingerprints().any(|(offset, word)| offset == CC.offset && word == 0x3940_2003));
    }
}
