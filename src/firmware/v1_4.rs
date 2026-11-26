//! MPC Sample 1.4.0.8 (`d87ac8c12e9`).
//!
//! Offsets are image-relative addresses into the app's `usr/bin/MPC Sample`.

use super::{BTI_C, Entry, PACIASP, call, data, hook, probe};

// --- Hooks: the 22 sites the SDK patches, in install order. ---

pub(crate) const SIXTEEN_CTOR: Entry = hook(0x01c0_1a20, PACIASP);
pub(crate) const SIXTEEN_PROC: Entry = hook(0x0261_2d20, PACIASP);
pub(crate) const DATA_HOOK: Entry = hook(0x0144_18f0, PACIASP);
pub(crate) const NOTE_ON: Entry = hook(0x0248_d4d0, PACIASP);
pub(crate) const SURFACE: Entry = hook(0x0283_e610, PACIASP);
pub(crate) const NOTE_OFF: Entry = hook(0x0248_d240, PACIASP);
pub(crate) const CC: Entry = hook(0x01c3_4360, 0x3940_2003);
pub(crate) const PITCH: Entry = hook(0x02c2_fc40, PACIASP);
pub(crate) const BUTTONS: Entry = hook(0x01c3_4420, BTI_C);
pub(crate) const REPAINT: Entry = hook(0x011d_b5a0, PACIASP);
/// The label constructor. Draw and Extract call this same address, and the
/// reentry guard stops the call from recursing into the hook.
pub(crate) const STRING_FROM_C: Entry = hook(0x00fe_4b70, PACIASP);
pub(crate) const EXTRACT_CTOR: Entry = hook(0x01a4_4220, PACIASP);
pub(crate) const EXTRACT_EXEC: Entry = hook(0x01a4_4590, PACIASP);
pub(crate) const WRITER: Entry = hook(0x01e6_1820, PACIASP);
pub(crate) const NOTE: Entry = hook(0x025e_5d00, PACIASP);
pub(crate) const CONTROL: Entry = hook(0x021f_1fc0, PACIASP);
pub(crate) const SAMPLE_SERVICE_CTOR: Entry = hook(0x01e7_f040, PACIASP);
pub(crate) const LIGHTING: Entry = hook(0x01c3_4a40, PACIASP);
/// The two destructor variants in the Temporary Samples service vtable.
pub(crate) const SAMPLE_SERVICE_DTOR: Entry = hook(0x01e8_2cb0, PACIASP);
pub(crate) const SAMPLE_SERVICE_DELETE: Entry = hook(0x01e8_2e40, PACIASP);
/// The two destructors from the active lighting sender's vtable.
pub(crate) const LIGHTING_DTOR: Entry = hook(0x01ae_6970, PACIASP);
pub(crate) const LIGHTING_DELETE: Entry = hook(0x01ae_6b90, PACIASP);
/// Vtable of the sender whose destructor hooks are installed above.
pub(crate) const LIGHTING_SENDER_VTABLE: Entry = data(0x0762_47a0);

// --- Calls: functions and data the SDK uses. Code entries are fingerprinted by
// `firmware_ok`; data entries are identified at run time by comparing a live
// object's vptr, not by an instruction word. ---

pub(crate) const SET_COLOUR: Entry = call(0x010b_ea80, PACIASP);
pub(crate) const FILL_RECT: Entry = call(0x010b_f4a0, PACIASP);
pub(crate) const SET_FONT: Entry = call(0x010b_f140, PACIASP);
pub(crate) const DRAW_TEXT: Entry = call(0x0111_2cb0, PACIASP);
pub(crate) const FONT: Entry = call(0x0111_3d10, PACIASP);
pub(crate) const FONT_DROP: Entry = call(0x0110_2260, BTI_C);
pub(crate) const STRING_DROP: Entry = call(0x00f9_b720, BTI_C);
pub(crate) const FREE_PAD: Entry = call(0x02d7_cbe0, PACIASP);
pub(crate) const PAD_BUSY: Entry = call(0x02d1_6c10, PACIASP);
pub(crate) const SLICE_RESET: Entry = call(0x02c8_2d80, PACIASP);
pub(crate) const SLICE_INSERT: Entry = call(0x02c8_3000, PACIASP);
pub(crate) const SLICE_SET_END: Entry = call(0x02c8_2e90, PACIASP);
pub(crate) const SELECT_SLICE: Entry = call(0x01cd_df70, PACIASP);
pub(crate) const SET_CHOP_MODE: Entry = call(0x01c5_52d0, PACIASP);
pub(crate) const SAMPLE_PAGE_VTABLE: Entry = data(0x076f_6170);
/// Vtable of the native Save Project name screen.
pub(crate) const PROJECT_SAVE_SCREEN_VTABLE: Entry = data(0x076f_4998);
/// Vtable of the native multi-line text editor.
pub(crate) const TEXT_EDITOR_VTABLE: Entry = data(0x076e_0880);
pub(crate) const CHOP_PREDICATE: Entry = call(0x011c_1890, PACIASP);
pub(crate) const REVERB_FACTORY: Entry = call(0x0409_aee0, PACIASP);
pub(crate) const VINYL_FACTORY: Entry = call(0x0409_a620, PACIASP);
pub(crate) const DECIMATOR_FACTORY: Entry = call(0x0409_c1b0, PACIASP);
pub(crate) const BUILD_SAMPLE: Entry = call(0x01e8_07d0, PACIASP);
pub(crate) const RELEASE_SHARED: Entry = call(0x012d_0370, PACIASP);
pub(crate) const EXTRACT_HANDLER: Entry = call(0x01cc_adb0, PACIASP);

/// Offset inside the stock app's text segment, used to recognise which loaded
/// image is the app. Any `PT_LOAD` that covers it belongs to firmware 1.4.0.8.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const APP_PROBE_OFFSET: usize = 0x00bc_a5c0;

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
    probe(0x0145_c8a0, PACIASP),
    probe(0x0146_8280, PACIASP),
    probe(0x01ac_a6a0, PACIASP),
    probe(0x01ad_fa20, BTI_C),
    probe(0x01da_01b0, PACIASP),
    probe(0x021e_6b60, PACIASP),
    probe(0x021e_6de0, PACIASP),
    probe(0x021e_a650, PACIASP),
    probe(0x021e_a8b0, PACIASP),
    probe(0x0257_f910, PACIASP),
    probe(0x0258_2d80, PACIASP),
    probe(0x0258_5aa0, PACIASP),
    probe(0x0258_66c0, PACIASP),
    probe(0x02d1_74a0, PACIASP),
    probe(0x02d2_0ff0, PACIASP),
    probe(0x02d2_3590, PACIASP),
    probe(0x0172_c580, BTI_C),
    probe(0x02d2_8540, PACIASP),
    probe(0x02d3_5e50, PACIASP),
    probe(0x02c7_46f0, BTI_C),
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
