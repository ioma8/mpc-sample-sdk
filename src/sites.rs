//! The hook sites: the only code in the SDK that runs inside the app.
//!
//! Each site decodes its arguments into an event, calls the mod, and then either
//! forwards the app's own displaced function or swallows the event.  The
//! originals are stored here and never leave this module.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::draw::{self, Graphics, Repaint};
use crate::event::{
    self, ButtonEvent, CcEvent, EncoderEvent, Flow, PadAction, PadEvent, PaintEvent, SurfaceEvent,
};
use crate::hook;
use crate::image;
use crate::loader::with_mod;
use crate::midi::{self, NoteEvent};
use crate::platform::{control, extract, page, sample, writer};

/// One site: its image offset and the handler to put there.
struct Site {
    offset: usize,
    handler: *const (),
}

/// Held by `note_on` and `note_off` so a mod can see them by name.
static NOTE_ON_OK: AtomicBool = AtomicBool::new(false);
/// Held by `note_off`.
static NOTE_OFF_OK: AtomicBool = AtomicBool::new(false);
/// The recipient of each physical button's press. Keep this outside the mod
/// mutex so a busy/reentrant callback cannot route the matching release alone.
static BUTTON_OWNERS: [AtomicU8; 256] = [const { AtomicU8::new(0) }; 256];

/// Whether both note hooks are installed.
#[must_use]
pub(crate) fn notes_ready() -> bool {
    NOTE_ON_OK.load(Ordering::Acquire) && NOTE_OFF_OK.load(Ordering::Acquire)
}

/// The displaced function of every installed site, each typed by its signature.
///
/// A field is `None` when its site was not installed. The value is written once,
/// before the branches are published, and cleared only if installation fails
/// before any branch is live.
#[derive(Clone, Copy, Default)]
pub(crate) struct Originals {
    pub(crate) sixteen_ctor: Option<unsafe extern "C" fn(usize, usize, usize, usize) -> usize>,
    pub(crate) sixteen_proc: Option<unsafe extern "C" fn(usize, i32, usize)>,
    pub(crate) data: Option<unsafe extern "C" fn(usize, i32)>,
    pub(crate) note_on: Option<unsafe extern "C" fn(usize, i32, i32, usize, f32)>,
    pub(crate) surface: Option<unsafe extern "C" fn(usize, i32, usize)>,
    pub(crate) note_off: Option<unsafe extern "C" fn(usize, i32, i32)>,
    pub(crate) cc: Option<unsafe extern "C" fn(usize, u32, f32, f32)>,
    pub(crate) pitch: Option<midi::PitchOffset>,
    pub(crate) buttons: Option<unsafe extern "C" fn(usize)>,
    pub(crate) repaint: Option<Repaint>,
    pub(crate) str_assign: Option<unsafe extern "C" fn(usize, *const u8)>,
    pub(crate) extract_ctor: Option<extract::Constructor>,
    pub(crate) extract_exec: Option<extract::Command>,
    pub(crate) writer: Option<writer::Writer>,
    pub(crate) note: Option<unsafe extern "C" fn(usize, usize, usize, usize)>,
    pub(crate) control: Option<control::SetControl>,
    pub(crate) sample_service: Option<unsafe extern "C" fn(usize, usize, usize, usize, usize)>,
    pub(crate) lighting: Option<unsafe extern "C" fn(usize, *mut u64, u8)>,
    pub(crate) sample_service_dtor: Option<unsafe extern "C" fn(usize)>,
    pub(crate) sample_service_delete: Option<unsafe extern "C" fn(usize)>,
    pub(crate) lighting_dtor: Option<unsafe extern "C" fn(usize)>,
    pub(crate) lighting_delete: Option<unsafe extern "C" fn(usize)>,
}

/// The trampolines `hook::prepare` returned, in table order.
const SITES: usize = 22;

impl Originals {
    /// Type each prepared trampoline by the signature of its site.
    ///
    /// # Safety
    ///
    /// Each address must be the trampoline `hook::prepare` returned for the site
    /// at that index of [`table`].
    unsafe fn from_trampolines(trampolines: &[Option<usize>; SITES]) -> Self {
        // SAFETY: the caller's contract; each field names its site's signature.
        unsafe {
            Self {
                sixteen_ctor: typed(trampolines[0]),
                sixteen_proc: typed(trampolines[1]),
                data: typed(trampolines[2]),
                note_on: typed(trampolines[3]),
                surface: typed(trampolines[4]),
                note_off: typed(trampolines[5]),
                cc: typed(trampolines[6]),
                pitch: typed(trampolines[7]),
                buttons: typed(trampolines[8]),
                repaint: typed(trampolines[9]),
                str_assign: typed(trampolines[10]),
                extract_ctor: typed(trampolines[11]),
                extract_exec: typed(trampolines[12]),
                writer: typed(trampolines[13]),
                note: typed(trampolines[14]),
                control: typed(trampolines[15]),
                sample_service: typed(trampolines[16]),
                lighting: typed(trampolines[17]),
                sample_service_dtor: typed(trampolines[18]),
                sample_service_delete: typed(trampolines[19]),
                lighting_dtor: typed(trampolines[20]),
                lighting_delete: typed(trampolines[21]),
            }
        }
    }
}

/// Reinterpret a prepared trampoline as the displaced function's type.
///
/// # Safety
///
/// `T` must be the signature of the function at this address.
unsafe fn typed<T: Copy>(address: Option<usize>) -> Option<T> {
    // SAFETY: the caller names the signature; both types are one pointer wide.
    address.map(|address| unsafe { core::mem::transmute_copy::<usize, T>(&address) })
}

/// The static every hook reads its displaced function from.
static mut ORIGINALS: Option<Originals> = None;

/// The displaced functions of the installed sites.
#[must_use]
pub(crate) fn originals() -> Originals {
    // SAFETY: written once before any branch is published, and never while a
    // handler can read it.
    unsafe { ORIGINALS }.unwrap_or_default()
}

/// Install every site and return whether any of them took.
///
/// # Safety
/// `base` must be the verified firmware image. Run once before app threads start.
pub(crate) unsafe fn install(base: usize) -> bool {
    let mut trampolines = [None; SITES];
    for (site, slot) in table().iter().zip(trampolines.iter_mut()) {
        // SAFETY: every offset is from the tested image and every handler has
        // the site's signature.  This only prepares the trampoline; the site
        // stays untouched until `hook::commit`.
        *slot = unsafe { hook::prepare(base, site.offset, site.handler as usize) };
    }
    let installed = trampolines.iter().flatten().count();
    if installed == 0 {
        hook::abort();
        return false;
    }
    // SAFETY: each address is the trampoline prepared for its site above.
    unsafe { ORIGINALS = Some(Originals::from_trampolines(&trampolines)) };
    // Seal the code pages first, then publish every branch.  A failure here
    // restores any site already patched and leaves the app unmodified.
    let committed = hook::commit();
    if !committed && !hook::touched() {
        // SAFETY: no branch is live, so no handler reads the originals.
        unsafe { ORIGINALS = None };
        return false;
    }
    // A rollback failure can leave a branch live even though installation
    // reports failure.  Its handler still forwards through ORIGINALS, so finish
    // the bookkeeping and let the loader refuse the mod; the app keeps working.
    // SAFETY: the site table has this entry.
    if originals().note_on.is_some() {
        NOTE_ON_OK.store(true, Ordering::Release);
    }
    // SAFETY: the site table has this entry.
    if originals().note_off.is_some() {
        NOTE_OFF_OK.store(true, Ordering::Release);
    }
    // SAFETY: the trampolines are the app's own functions.
    unsafe {
        extract::record(originals().extract_ctor, originals().extract_exec);
        control::record(originals().control);
    }
    sample::set_lifecycle_ready(
        originals().sample_service.is_some()
            && originals().sample_service_dtor.is_some()
            && originals().sample_service_delete.is_some(),
    );
    crate::platform::lighting::set_lifecycle_ready(
        originals().lighting.is_some()
            && originals().lighting_dtor.is_some()
            && originals().lighting_delete.is_some(),
    );
    install_paint(base);
    install_project_save_paint(base);
    committed
}

/// The site table, in install order.
fn table() -> [Site; SITES] {
    [
        Site {
            offset: crate::firmware::SIXTEEN_CTOR.offset,
            handler: sixteen_ctor as *const (),
        },
        Site {
            offset: crate::firmware::SIXTEEN_PROC.offset,
            handler: sixteen_proc as *const (),
        },
        Site {
            offset: crate::firmware::DATA_HOOK.offset,
            handler: data as *const (),
        },
        Site {
            offset: crate::firmware::NOTE_ON.offset,
            handler: note_on as *const (),
        },
        Site {
            offset: crate::firmware::SURFACE.offset,
            handler: surface as *const (),
        },
        Site {
            offset: crate::firmware::NOTE_OFF.offset,
            handler: note_off as *const (),
        },
        Site {
            offset: crate::firmware::CC.offset,
            handler: cc as *const (),
        },
        Site {
            offset: crate::firmware::PITCH.offset,
            handler: pitch as *const (),
        },
        Site {
            offset: crate::firmware::BUTTONS.offset,
            handler: buttons as *const (),
        },
        Site {
            offset: crate::firmware::REPAINT.offset,
            handler: repaint as *const (),
        },
        Site {
            offset: crate::firmware::STRING_FROM_C.offset,
            handler: str_assign as *const (),
        },
        Site {
            offset: crate::firmware::EXTRACT_CTOR.offset,
            handler: extract::ctor_entry as *const (),
        },
        Site {
            offset: crate::firmware::EXTRACT_EXEC.offset,
            handler: extract::exec_entry as *const (),
        },
        Site {
            offset: crate::firmware::WRITER.offset,
            handler: writer_entry as *const (),
        },
        Site {
            offset: crate::firmware::NOTE.offset,
            handler: note as *const (),
        },
        Site {
            offset: crate::firmware::CONTROL.offset,
            handler: control_entry as *const (),
        },
        Site {
            offset: crate::firmware::SAMPLE_SERVICE_CTOR.offset,
            handler: sample_service_ctor as *const (),
        },
        Site {
            offset: crate::firmware::LIGHTING.offset,
            handler: lighting as *const (),
        },
        Site {
            offset: crate::firmware::SAMPLE_SERVICE_DTOR.offset,
            handler: sample_service_dtor as *const (),
        },
        Site {
            offset: crate::firmware::SAMPLE_SERVICE_DELETE.offset,
            handler: sample_service_delete as *const (),
        },
        Site {
            offset: crate::firmware::LIGHTING_DTOR.offset,
            handler: lighting_dtor as *const (),
        },
        Site {
            offset: crate::firmware::LIGHTING_DELETE.offset,
            handler: lighting_delete as *const (),
        },
    ]
}

/// The AC50 per-pad lighting callback: apply the mod's override, then forward.
///
/// The callback receives the sender, a pointer to the native `PadLight`
/// (`RGB | brightness_float << 32`) and the zero-based pad.
///
/// # Safety
///
/// The app calls this with its own sender, `PadLight` and pad index.
unsafe extern "C" fn lighting(sender: usize, light: *mut u64, pad: u8) {
    let native = if light.is_null() {
        0
    } else {
        // SAFETY: the app passes its live PadLight for this callback.
        unsafe { *light }
    };
    crate::platform::lighting::observe(sender, pad, native);
    if let Some(forward) = originals().lighting {
        if light.is_null() {
            // SAFETY: forward exactly the null pointer supplied by the app.
            unsafe { forward(sender, light, pad) };
        } else {
            // SAFETY: the app supplies its live, exclusively writable value.
            let value = unsafe { &mut *light };
            with_light_override(
                value,
                crate::platform::lighting::override_light(pad),
                |value| {
                    // SAFETY: sender and light came from this native callback.
                    unsafe { forward(sender, core::ptr::from_mut(value), pad) };
                },
            );
        }
    }
}

/// Forward the override without changing the app's retained native light.
fn with_light_override(value: &mut u64, replacement: Option<u64>, forward: impl FnOnce(&mut u64)) {
    let original = *value;
    if let Some(replacement) = replacement {
        *value = replacement;
    }
    forward(value);
    *value = original;
}

/// Invalidate the sender before either native destructor tears it down.
unsafe extern "C" fn lighting_dtor(sender: usize) {
    crate::platform::lighting::forget_sender(sender);
    if let Some(forward) = originals().lighting_dtor {
        // SAFETY: the firmware supplied this object to its original destructor.
        unsafe { forward(sender) };
    }
}

unsafe extern "C" fn lighting_delete(sender: usize) {
    crate::platform::lighting::forget_sender(sender);
    if let Some(forward) = originals().lighting_delete {
        // SAFETY: the firmware supplied this object to its original destructor.
        unsafe { forward(sender) };
    }
}

unsafe extern "C" fn sample_service_ctor(
    service: usize,
    first: usize,
    second: usize,
    third: usize,
    fourth: usize,
) {
    // SAFETY: site 16 is this constructor and the app supplies its arguments.
    let forward = originals().sample_service;
    if let Some(forward) = forward {
        // SAFETY: this forwards the original constructor arguments unchanged.
        unsafe { forward(service, first, second, third, fourth) };
        sample::remember_service(service);
    }
}

/// Invalidate the observed service before the firmware destroys it.
unsafe extern "C" fn sample_service_dtor(service: usize) {
    sample::forget_service(service);
    if let Some(forward) = originals().sample_service_dtor {
        // SAFETY: the firmware supplied this object to its original destructor.
        unsafe { forward(service) };
    }
}

/// The deleting destructor has a separate inlined body on this firmware.
unsafe extern "C" fn sample_service_delete(service: usize) {
    sample::forget_service(service);
    if let Some(forward) = originals().sample_service_delete {
        // SAFETY: the firmware supplied this object to its original destructor.
        unsafe { forward(service) };
    }
}

/// The app's 16 Levels view constructor: pass it through.
///
/// # Safety
///
/// The app calls this with its own arguments.
unsafe extern "C" fn sixteen_ctor(a: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: the original is the app's own constructor.
    unsafe {
        originals()
            .sixteen_ctor
            .map_or(0, |forward| forward(a, b, c, d))
    }
}

/// The app's 16 Levels note handler: pass it through.
///
/// # Safety
///
/// The app calls this with its own arguments.
unsafe extern "C" fn sixteen_proc(app: usize, index: i32, out: usize) {
    // SAFETY: the original is the app's own handler.
    unsafe {
        if let Some(forward) = originals().sixteen_proc {
            forward(app, index, out);
        }
    }
}

/// Ask the mod for its decision on one event; forward when no mod is there.
fn decide(call: impl FnOnce(&mut dyn event::Mod) -> Flow) -> Flow {
    with_mod(call).unwrap_or(Flow::Forward)
}

/// Keep a button's release with the recipient of its press, even when the mod
/// callback is unavailable for one edge.
fn button_flow(code: u32, pressed: bool, flow: Flow) -> Flow {
    let Some(owner) = usize::try_from(code)
        .ok()
        .and_then(|code| BUTTON_OWNERS.get(code))
    else {
        return flow;
    };
    if pressed {
        owner.store(if flow == Flow::Forward { 1 } else { 2 }, Ordering::Release);
        flow
    } else {
        match owner.swap(0, Ordering::AcqRel) {
            1 => Flow::Forward,
            2 => Flow::Consume,
            _ => flow,
        }
    }
}

/// The encoder: let the mod decide, then forward.
///
/// # Safety
///
/// The app calls this with its own surface and delta.
unsafe extern "C" fn data(surface: usize, delta: i32) {
    let event = EncoderEvent { surface, delta };
    let flow = decide(|module| module.on_encoder(event));
    if flow == Flow::Forward {
        // SAFETY: the original is the app's own handler.
        unsafe {
            if let Some(forward) = originals().data {
                forward(surface, delta);
            }
        }
    }
}

/// Dispatch a pad event and complete its token after native forwarding.
fn dispatch_pad(
    hooks: event::InputHooks,
    event: PadEvent,
    fallback: impl FnOnce(PadEvent) -> Flow,
    forward: impl FnOnce() -> bool,
) {
    let response = hooks.pad.map_or_else(
        || event::PadResponse {
            flow: fallback(event),
            token: 0,
        },
        |handle| handle(event),
    );
    let sent = response.flow == Flow::Forward && forward();
    if response.token != 0
        && let Some(complete) = hooks.pad_complete
    {
        complete(event, response.token, sent);
    }
}

/// A pad press: let the mod decide, then forward.
///
/// # Safety
///
/// The app calls this with its own input object.
unsafe extern "C" fn note_on(
    input: usize,
    source: i32,
    pad: i32,
    properties: usize,
    velocity: f32,
) {
    let event = PadEvent {
        action: PadAction::Press,
        input,
        source,
        pad,
        properties,
        velocity,
    };
    dispatch_pad(
        crate::loader::input_hooks(),
        event,
        |event| decide(|module| module.on_pad(event)),
        || {
            if let Some(forward) = originals().note_on {
                // SAFETY: the firmware supplied these arguments to this hook.
                unsafe { forward(input, source, pad, properties, velocity) };
                true
            } else {
                false
            }
        },
    );
}

/// A control-surface event: let the mod decide, then forward.
///
/// # Safety
///
/// The app calls this with its own surface.
unsafe extern "C" fn surface(surface: usize, code: i32, payload: usize) {
    let mut raw = [0_u8; 3];
    let decoded = payload != 0 && image::read(payload, &mut raw);
    let event = SurfaceEvent {
        surface,
        code,
        payload,
        kind: if decoded { raw[0] & 0xf0 } else { 0 },
        note: if decoded { raw[1] } else { 0 },
        velocity: if decoded { raw[2] } else { 0 },
    };
    let flow = crate::loader::input_hooks().surface.map_or_else(
        || decide(|module| module.on_surface(event)),
        |handler| handler(event),
    );
    if flow == Flow::Forward {
        // SAFETY: the original is the app's own handler.
        unsafe {
            if let Some(forward) = originals().surface {
                event::with_surface_event(event, || forward(surface, code, payload));
            }
        }
    }
}

/// A pad release: let the mod decide, then forward.
///
/// # Safety
///
/// The app calls this with its own input object.
unsafe extern "C" fn note_off(input: usize, source: i32, pad: i32) {
    let event = PadEvent {
        action: PadAction::Release,
        input,
        source,
        pad,
        properties: 0,
        velocity: 0.0,
    };
    dispatch_pad(
        crate::loader::input_hooks(),
        event,
        |event| decide(|module| module.on_pad(event)),
        || {
            if let Some(forward) = originals().note_off {
                // SAFETY: the firmware supplied these arguments to this hook.
                unsafe { forward(input, source, pad) };
                true
            } else {
                false
            }
        },
    );
}

/// A control change: let the mod decide, then forward.
///
/// # Safety
///
/// The app calls this with its own event.
unsafe extern "C" fn cc(event: usize, raw: u32, low: f32, range: f32) {
    let control = if event == 0 {
        0xffff
    } else {
        // SAFETY: the app's event keeps its control byte at +8.
        u32::from(unsafe { *(event as *const u8).add(8) })
    };
    let message = CcEvent {
        event,
        control,
        value: raw,
        low,
        range,
    };
    let flow = decide(|module| module.on_cc(message));
    if flow == Flow::Forward {
        // SAFETY: the original is the app's own handler.
        unsafe {
            if let Some(forward) = originals().cc {
                forward(event, raw, low, range);
            }
        }
    }
}

/// The 16 Levels pitch setter: let the mod shift the applied offset.
///
/// # Safety
///
/// The app calls this with its own arguments.
unsafe extern "C" fn pitch(app: usize, a: u64, b: u64, applied: i32) {
    let offset = with_mod(|module| module.on_pitch(applied)).unwrap_or(applied);
    // SAFETY: the original is the app's own setter.
    unsafe {
        if let Some(forward) = originals().pitch {
            forward(app, a, b, offset);
        }
    }
}

/// A button: let the mod decide, then forward.
///
/// # Safety
///
/// The app calls this with its own event object.
unsafe extern "C" fn buttons(event: usize) -> i32 {
    let code = if event == 0 {
        0
    } else {
        // SAFETY: the app's event keeps its key code at +8.
        u32::from(unsafe { *(event as *const u8).add(8) })
    };
    let value = if event == 0 {
        0
    } else {
        // SAFETY: and its value at +9.
        u32::from(unsafe { *(event as *const u8).add(9) })
    };
    let pressed = value == event::button::PRESSED;
    // The event's first word is its context; `+0x10` there is the surface.
    let surface = if event == 0 {
        0
    } else {
        image::read_pointer(event)
            .and_then(|context| image::read_pointer(context.wrapping_add(0x10)))
            .unwrap_or(0)
    };
    let mut bytes = [0_u8; 16];
    if event != 0 {
        // SAFETY: the app's button event is at least this long.
        unsafe { core::ptr::copy_nonoverlapping(event as *const u8, bytes.as_mut_ptr(), 16) };
    }
    let message = ButtonEvent {
        event,
        bytes,
        code,
        value,
        pressed,
        shift: event::shift(),
        erase: event::erase(),
        surface,
    };
    let flow = button_flow(code, pressed, decide(|module| module.on_button(message)));
    // Track the hold state after the mod has seen the event, so it sees the
    // state that was in effect when the key was pressed.
    if code == event::button::SHIFT {
        event::set_shift(pressed);
    }
    if code == event::button::ERASE {
        event::set_erase(pressed);
    }
    if flow == Flow::Forward {
        // SAFETY: the original is the app's own handler.
        unsafe {
            if let Some(forward) = originals().buttons {
                forward(event);
            }
        }
    }
    0
}

/// A repaint: pass it through.
///
/// # Safety
///
/// The app calls this with its own component and rectangles.
unsafe extern "C" fn repaint(component: usize, a: u64, b: u64) {
    // SAFETY: the original is the app's own repaint.
    unsafe {
        if let Some(forward) = originals().repaint {
            forward(component, a, b);
        }
    }
    let _ = with_mod(|module| module.on_repaint(component));
}

/// A label assignment: let the mod rename it, then forward.
///
/// # Safety
///
/// `text` must be a NUL-terminated string.
unsafe extern "C" fn str_assign(dest: usize, text: *const u8) {
    let mut chosen = text;
    if !text.is_null() {
        // SAFETY: the caller promises a NUL-terminated string.
        let label = unsafe { core::ffi::CStr::from_ptr(text.cast()) };
        if let Some(replacement) =
            with_mod(|module| module.on_text_assignment(dest, label)).flatten()
        {
            chosen = replacement.as_ptr().cast();
        }
    }
    // SAFETY: the original is the app's own assignment.
    unsafe {
        if let Some(forward) = originals().str_assign {
            forward(dest, chosen);
        }
    }
}

/// Replace text in a live MPC `ui::TextEditor` using its native key handler.
///
/// # Safety
/// `editor` must be the supported firmware's live `ui::TextEditor` and this
/// must run on the UI thread that owns it.
pub unsafe fn replace_native_text_editor(editor: usize, text: &core::ffi::CStr) -> bool {
    const TEXT_EDITOR_VTABLE: usize = 0x6e18e20;
    const KEY_PRESSED_SLOT: usize = 28;

    if !text.to_bytes().is_ascii()
        || image::read_pointer(editor) != Some(image::base() + TEXT_EDITOR_VTABLE)
    {
        return false;
    }
    let Some(vtable) = image::read_pointer(editor) else {
        return false;
    };
    let Some(address) = vtable
        .checked_add(KEY_PRESSED_SLOT * size_of::<usize>())
        .and_then(image::read_pointer)
    else {
        return false;
    };
    type KeyPressed = unsafe extern "C" fn(usize, *const NativeKeyPress) -> bool;
    // SAFETY: this slot is `TextEditor::keyPressed(const KeyPress&)` in the
    // supported firmware; the object vtable was checked above.
    let key_pressed: KeyPressed = unsafe { core::mem::transmute(address) };
    let select_all = NativeKeyPress {
        key_code: i32::from(b'a'),
        modifiers: 2, // JUCE ModifierKeys::ctrlModifier
        text_character: 0,
    };
    // SAFETY: the caller guarantees a live UI-thread-owned editor.
    if !unsafe { key_pressed(editor, &select_all) } {
        return false;
    }
    for byte in text.to_bytes() {
        let key = NativeKeyPress {
            key_code: i32::from(*byte),
            modifiers: 0,
            text_character: u32::from(*byte),
        };
        // SAFETY: same live editor and UI thread; printable ASCII is valid.
        if !unsafe { key_pressed(editor, &key) } {
            return false;
        }
    }
    true
}

#[repr(C)]
struct NativeKeyPress {
    key_code: i32,
    modifiers: u32,
    text_character: u32,
}

/// The app's writer: hand the block to the substitute.
///
/// # Safety
///
/// The app calls this with its own block.
unsafe extern "C" fn writer_entry(closure: usize, block: *mut u8) {
    // SAFETY: the original is the app's own writer.
    let forward = originals().writer;
    // SAFETY: the caller passed the app's arguments.
    unsafe { writer::substitute(closure, block, forward) };
}

/// A MIDI note: let the mod decide, then forward (and restore).
///
/// # Safety
///
/// The app calls this with its own event.
unsafe extern "C" fn note(app: usize, context: usize, event: *mut u8, source: usize) {
    // SAFETY: the original is the app's own setter, when the site is installed.
    let pitch_off = originals().pitch;
    // The event is the app's writable object for the duration of this hook.
    let storage: &mut [u8] = if event.is_null() {
        &mut []
    } else {
        // SAFETY: the app promises exclusive writability for one event.
        unsafe { core::slice::from_raw_parts_mut(event, midi::EVENT_BYTES) }
    };
    // SAFETY: the site supplies the app's own displaced pitch setter.
    let mut message = unsafe { NoteEvent::new(app, context, storage, source as u64, pitch_off) };
    let flow = if let Some(handler) = crate::loader::input_hooks().note {
        handler(&mut message)
    } else {
        decide(|module| module.on_note(&mut message))
    };
    match flow {
        Flow::Forward if message.is_replayed() => {
            // SAFETY: the original is the app's own dispatcher.
            unsafe {
                if let Some(forward) = originals().note {
                    forward(
                        app,
                        message.forward_context(),
                        message.pointer() as usize,
                        usize::try_from(message.forward_source()).unwrap_or(usize::MAX),
                    );
                }
            }
            message.restore();
        }
        Flow::Forward => {
            message.restore();
            // SAFETY: the original is the app's own dispatcher.
            unsafe {
                if let Some(forward) = originals().note {
                    forward(app, context, message.pointer() as usize, source);
                }
            }
        }
        Flow::Consume => message.restore(),
    }
}

/// The control-surface setter: learn the surface and pass the call on.
///
/// # Safety
///
/// The app calls this with its own objects.
unsafe extern "C" fn control_entry(
    control: usize,
    surface: usize,
    id: i32,
    op: i32,
    value: f32,
) -> usize {
    // SAFETY: the app's own objects.
    unsafe { control::hook(control, surface, id, op, value) }
}

/// The app's paint, displaced by the SDK.
static mut ORIGINAL_PAINT: Option<unsafe extern "C" fn(usize, usize)> = None;
static mut ORIGINAL_PROJECT_SAVE_PAINT: Option<unsafe extern "C" fn(usize, usize)> = None;

/// The page's paint: let the app draw, then the mod.
///
/// # Safety
///
/// The app calls this with its own component and graphics.
unsafe extern "C" fn page_paint(component: usize, graphics: usize) {
    // SAFETY: the original is the app's own paint.
    unsafe {
        if let Some(original) = *core::ptr::addr_of!(ORIGINAL_PAINT) {
            original(component, graphics);
        }
    }
    draw::set_painted(component);
    let sample_page = page::is_sample_page(component);
    if sample_page {
        page::set_screen(component);
    }
    // SAFETY: the app is painting this live component on the UI thread.
    let chop_mode = sample_page && unsafe { page::chop_mode(component) };
    let event = PaintEvent {
        component,
        graphics,
        sample_page,
        chop_mode,
    };
    let mut handle = Graphics::new(graphics);
    with_mod(|module| module.on_paint(&mut handle, event));
}

/// Install the page paint over the app's vtable slot.
fn install_paint(base: usize) -> bool {
    let address = base + crate::firmware::SAMPLE_PAGE_VTABLE.offset;
    // SAFETY: the pointer is only compared, never dereferenced.
    if unsafe { !image::mapped(address as *const u8) } {
        return false;
    }
    // SAFETY: the address is inside the app's screen vtable.
    let slot = unsafe { (address as *mut usize).add(27) };
    // SAFETY: see above.
    let original = unsafe { *slot };
    if original == 0 {
        return false;
    }
    // SAFETY: the original is the app's own paint.
    unsafe {
        ORIGINAL_PAINT =
            core::mem::transmute::<usize, Option<unsafe extern "C" fn(usize, usize)>>(original);
    }
    let page = address & !0xfff;
    // SAFETY: the page is inside the app's image.
    let unlocked = unsafe {
        libc::mprotect(
            page as *mut libc::c_void,
            0x1000,
            libc::PROT_READ | libc::PROT_WRITE,
        )
    } == 0;
    if !unlocked {
        return false;
    }
    // SAFETY: the page is writable now.
    unsafe {
        *slot = page_paint as *const () as usize;
        libc::mprotect(page as *mut libc::c_void, 0x1000, libc::PROT_READ);
    }
    true
}

/// The native Save Project screen's paint, displaced by the SDK.
unsafe extern "C" fn project_save_paint(component: usize, graphics: usize) {
    // SAFETY: the original is the app's own paint.
    unsafe {
        if let Some(original) = *core::ptr::addr_of!(ORIGINAL_PROJECT_SAVE_PAINT) {
            original(component, graphics);
        }
    }
    draw::set_painted(component);
    let event = PaintEvent {
        component,
        graphics,
        sample_page: false,
        chop_mode: false,
    };
    let mut handle = Graphics::new(graphics);
    with_mod(|module| module.on_paint(&mut handle, event));
}

/// Install a post-paint callback on the native Save Project name screen.
fn install_project_save_paint(base: usize) -> bool {
    let address = base + crate::firmware::PROJECT_SAVE_SCREEN_VTABLE.offset;
    // SAFETY: only check that the firmware vtable address is mapped.
    if unsafe { !image::mapped(address as *const u8) } {
        return false;
    }
    // SAFETY: the address is the supported firmware's ProjectSaveScreen vtable.
    let slot = unsafe { (address as *mut usize).add(27) };
    // SAFETY: the vtable address is mapped and belongs to the app image.
    let original = unsafe { *slot };
    if original == 0 || original == project_save_paint as *const () as usize {
        return false;
    }
    // SAFETY: ProjectSaveScreen::paint has the same ABI as page_paint.
    unsafe {
        ORIGINAL_PROJECT_SAVE_PAINT =
            Some(core::mem::transmute::<usize, unsafe extern "C" fn(usize, usize)>(original));
    }
    let page = address & !0xfff;
    // SAFETY: the vtable page is inside the app's image.
    let unlocked = unsafe {
        libc::mprotect(
            page as *mut libc::c_void,
            0x1000,
            libc::PROT_READ | libc::PROT_WRITE,
        )
    } == 0;
    if !unlocked {
        // SAFETY: the hook is not installed.
        unsafe { ORIGINAL_PROJECT_SAVE_PAINT = None };
        return false;
    }
    // SAFETY: the page is writable and the displaced method is saved above.
    unsafe {
        *slot = project_save_paint as *const () as usize;
        libc::mprotect(page as *mut libc::c_void, 0x1000, libc::PROT_READ);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{button_flow, notes_ready, table};

    #[test]
    fn button_release_keeps_the_press_recipient_when_callback_availability_changes() {
        use crate::Flow;

        assert_eq!(button_flow(240, true, Flow::Consume), Flow::Consume);
        assert_eq!(button_flow(240, false, Flow::Forward), Flow::Consume);

        assert_eq!(button_flow(241, true, Flow::Forward), Flow::Forward);
        assert_eq!(button_flow(241, false, Flow::Consume), Flow::Forward);

        // An unmatched release keeps the callback's decision.
        assert_eq!(button_flow(242, false, Flow::Consume), Flow::Consume);
    }

    #[test]
    fn independent_pad_hook_completes_after_native_forwarding() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static ORDER: AtomicUsize = AtomicUsize::new(0);
        let event = crate::PadEvent {
            action: crate::PadAction::Press,
            input: 1,
            source: 2,
            pad: 3,
            properties: 0,
            velocity: 0.5,
        };
        let hooks = crate::InputHooks {
            pad: Some(|_| crate::PadResponse {
                flow: crate::Flow::Forward,
                token: 7,
            }),
            pad_complete: Some(|_, token, sent| {
                assert_eq!(token, 7);
                assert!(sent);
                assert_eq!(ORDER.load(Ordering::Acquire), 1);
                ORDER.store(2, Ordering::Release);
            }),
            ..crate::InputHooks::default()
        };
        ORDER.store(0, Ordering::Release);
        super::dispatch_pad(
            hooks,
            event,
            |_| panic!("UI fallback used"),
            || {
                ORDER.store(1, Ordering::Release);
                true
            },
        );
        assert_eq!(ORDER.load(Ordering::Acquire), 2);
    }

    #[test]
    fn lighting_override_does_not_replace_the_native_value() {
        let mut native = 42_u64;
        super::with_light_override(&mut native, Some(99), |sent| assert_eq!(*sent, 99));
        assert_eq!(native, 42);
    }

    #[test]
    fn the_table_has_one_entry_per_site() {
        assert_eq!(table().len(), 22);
        assert!(!notes_ready());
    }
}
