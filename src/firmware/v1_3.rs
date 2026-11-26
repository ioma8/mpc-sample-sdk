//! MPC Sample 1.3.0.12 (`40e46ca7991`).
//!
//! Offsets are image-relative addresses into the app's `usr/bin/MPC Sample`.

use super::{BTI_C, Entry, PACIASP, call, data, hook, probe};

// --- Hooks: the 22 sites the SDK patches, in install order. ---

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
/// The two destructor variants in the Temporary Samples service vtable.
pub(crate) const SAMPLE_SERVICE_DTOR: Entry = hook(0x01df_6d40, PACIASP);
pub(crate) const SAMPLE_SERVICE_DELETE: Entry = hook(0x01df_6ed0, PACIASP);
/// The two destructors from the active lighting sender's vtable.
pub(crate) const LIGHTING_DTOR: Entry = hook(0x0143_9e60, PACIASP);
pub(crate) const LIGHTING_DELETE: Entry = hook(0x0143_9fd0, PACIASP);
/// Vtable of the sender whose destructor hooks are installed above.
pub(crate) const LIGHTING_SENDER_VTABLE: Entry = data(0x06d8_fa60);

// --- Calls: functions and data the SDK uses. Code entries are fingerprinted by
// `firmware_ok`; data entries are identified at run time by comparing a live
// object's vptr, not by an instruction word. ---

pub(crate) const SET_COLOUR: Entry = call(0x102_d5e0, PACIASP);
pub(crate) const FILL_RECT: Entry = call(0x102_e000, PACIASP);
pub(crate) const SET_FONT: Entry = call(0x102_dca0, PACIASP);
pub(crate) const DRAW_TEXT: Entry = call(0x108_17b0, PACIASP);
pub(crate) const FONT: Entry = call(0x108_2810, PACIASP);
pub(crate) const FONT_DROP: Entry = call(0x107_0d60, BTI_C);
pub(crate) const STRING_DROP: Entry = call(0xf0_a290, BTI_C);
pub(crate) const FREE_PAD: Entry = call(0x286_dd00, PACIASP);
pub(crate) const PAD_BUSY: Entry = call(0x280_47b0, PACIASP);
pub(crate) const SLICE_RESET: Entry = call(0x295_0aa0, PACIASP);
pub(crate) const SLICE_INSERT: Entry = call(0x295_0d20, PACIASP);
pub(crate) const SLICE_SET_END: Entry = call(0x295_0bb0, PACIASP);
pub(crate) const SELECT_SLICE: Entry = call(0x166_80a0, PACIASP);
pub(crate) const SET_CHOP_MODE: Entry = call(0x15d_2840, PACIASP);
pub(crate) const SAMPLE_PAGE_VTABLE: Entry = data(0x6e2_12a0);
/// Vtable of the native Save Project name screen.
pub(crate) const PROJECT_SAVE_SCREEN_VTABLE: Entry = data(0x6e1_faa0);
/// Vtable of the native multi-line text editor.
pub(crate) const TEXT_EDITOR_VTABLE: Entry = data(0x6e1_8e20);
pub(crate) const CHOP_PREDICATE: Entry = call(0x112_fd50, PACIASP);
pub(crate) const REVERB_FACTORY: Entry = call(0x03c4_45b0, PACIASP);
pub(crate) const VINYL_FACTORY: Entry = call(0x03c4_3cf0, PACIASP);
pub(crate) const DECIMATOR_FACTORY: Entry = call(0x03c4_57a0, PACIASP);
pub(crate) const BUILD_SAMPLE: Entry = call(0x01df_4ac0, PACIASP);
pub(crate) const RELEASE_SHARED: Entry = call(0x0123_bca0, PACIASP);
pub(crate) const EXTRACT_HANDLER: Entry = call(0x0165_80d0, PACIASP);

/// Offset inside the stock app's text segment, used to recognise which loaded
/// image is the app. Any `PT_LOAD` that covers it belongs to firmware 1.3.0.12.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const APP_PROBE_OFFSET: usize = 0x00b5_5b00;

/// Every function and data entry the modules use, in one list.
pub(crate) const CALLS: [Entry; 25] = [
    SET_COLOUR,
    FILL_RECT,
    SET_FONT,
    DRAW_TEXT,
    FONT,
    FONT_DROP,
    STRING_DROP,
    FREE_PAD,
    PAD_BUSY,
    SLICE_RESET,
    SLICE_INSERT,
    SLICE_SET_END,
    SELECT_SLICE,
    SET_CHOP_MODE,
    SAMPLE_PAGE_VTABLE,
    PROJECT_SAVE_SCREEN_VTABLE,
    TEXT_EDITOR_VTABLE,
    LIGHTING_SENDER_VTABLE,
    CHOP_PREDICATE,
    REVERB_FACTORY,
    VINYL_FACTORY,
    DECIMATOR_FACTORY,
    BUILD_SAMPLE,
    RELEASE_SHARED,
    EXTRACT_HANDLER,
];

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

/// The 22 hook sites, in install order.
pub(crate) const HOOKS: [Entry; 22] = [
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
    SAMPLE_SERVICE_DTOR,
    SAMPLE_SERVICE_DELETE,
    LIGHTING_DTOR,
    LIGHTING_DELETE,
];
