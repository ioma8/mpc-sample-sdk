//! The events a mod handles and the flows it returns.
//!
//! Every hook the SDK installs is decoded into one of these values; the mod
//! decides whether the event is consumed or handed back to the app.  Passing the
//! app's own original call is the SDK's job, so a mod never stores or types a
//! displaced function.

/// Hardware button codes reported by the control surface.
pub mod button {
    /// SHIFT.
    pub const SHIFT: u32 = 0x31;
    /// ERASE.
    pub const ERASE: u32 = 0x09;
    /// SAMPLE RECORD.
    pub const SAMPLE_RECORD: u32 = 0x49;
    /// DATA encoder press.
    pub const DATA: u32 = 0x6f;
    /// B3.
    pub const B3: u32 = 0x0e;
    /// PLUS.
    pub const PLUS: u32 = 0x36;
    /// MINUS.
    pub const MINUS: u32 = 0x37;
    /// 16 LEVELS.
    pub const LEVELS: u32 = 0x28;
    /// CHOP.
    pub const CHOP: u8 = 0x4b;
    /// Raw button press value.
    pub const PRESSED: u32 = 0x7f;
}

/// Control-surface controller numbers.
pub mod cc {
    /// Master volume.
    pub const MASTER: u16 = 0x13;
    /// Knob 1.
    pub const KNOB1: u16 = 0x10;
    /// Knob 2.
    pub const KNOB2: u16 = 0x11;
    /// Knob 3.
    pub const KNOB3: u16 = 0x12;
    /// DATA encoder.
    pub const DATA: u16 = 0x64;
}

use core::ffi::CStr;

use crate::draw::Graphics;
pub use crate::midi::NoteEvent;

/// Whether a pad event is a press or a release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadAction {
    /// The pad went down.
    Press,
    /// The pad came up.
    Release,
}

/// One pad/note event from the app's input path.
#[derive(Clone, Copy, Debug)]
pub struct PadEvent {
    /// Whether the pad was pressed or released.
    pub action: PadAction,
    /// The input object the event came from.
    pub input: usize,
    /// The source id the app reported.
    pub source: i32,
    /// The pad index.
    pub pad: i32,
    /// The event's property block.
    pub properties: usize,
    /// The velocity, zero on a release.
    pub velocity: f32,
}

/// A control-surface event.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceEvent {
    /// The surface object.
    pub surface: usize,
    /// The event code (the pad index for a pad event).
    pub code: i32,
    /// The payload pointer.
    pub payload: usize,
    /// The payload's MIDI status nibble (`0x80`, `0x90` or `0xa0`), or zero.
    pub kind: u8,
    /// The payload's note byte.
    pub note: u8,
    /// The payload's velocity byte.
    pub velocity: u8,
}

impl SurfaceEvent {
    /// Whether this is a pad note or pressure event.
    #[must_use]
    pub const fn is_pad_event(&self) -> bool {
        matches!(self.kind, 0x80 | 0x90 | 0xa0)
    }
    /// Whether this is a nonzero-velocity pad press.
    #[must_use]
    pub const fn is_pad_press(&self) -> bool {
        self.kind == 0x90 && self.velocity > 0
    }
}

thread_local! {
    static CURRENT_SURFACE: core::cell::Cell<Option<SurfaceEvent>> = const { core::cell::Cell::new(None) };
}

/// The surface event currently being forwarded on this thread, if any.
///
/// The firmware may call a pad hook synchronously while handling a surface
/// event. This identifies the physical pad even when the pad hook's own index
/// refers to a different object, as with Chop releases.
#[must_use]
pub fn current_surface_event() -> Option<SurfaceEvent> {
    CURRENT_SURFACE.with(core::cell::Cell::get)
}

pub(crate) fn with_surface_event<T>(event: SurfaceEvent, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<SurfaceEvent>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT_SURFACE.with(|current| current.set(self.0));
        }
    }
    let previous = CURRENT_SURFACE.with(|current| current.replace(Some(event)));
    let _restore = Restore(previous);
    f()
}

#[cfg(test)]
mod surface_context_tests {
    use super::{SurfaceEvent, current_surface_event, with_surface_event};

    #[test]
    fn surface_context_is_scoped_and_nested() {
        let mut event = SurfaceEvent {
            surface: 1,
            code: 2,
            payload: 0,
            kind: 0x80,
            note: 42,
            velocity: 0,
        };
        assert!(current_surface_event().is_none());
        with_surface_event(event, || {
            assert_eq!(current_surface_event().unwrap().code, 2);
            event.code = 3;
            with_surface_event(event, || {
                assert_eq!(current_surface_event().unwrap().code, 3);
            });
            assert_eq!(current_surface_event().unwrap().code, 2);
        });
        assert!(current_surface_event().is_none());
    }
}

/// A turn of the DATA encoder.
#[derive(Clone, Copy, Debug)]
pub struct EncoderEvent {
    /// The encoder's surface object.
    pub surface: usize,
    /// The signed delta.
    pub delta: i32,
}

/// A button event.
#[derive(Clone, Copy, Debug)]
pub struct ButtonEvent {
    /// The app's event object.
    pub event: usize,
    /// A copy of the event's first 16 bytes, for a replay.
    pub bytes: [u8; 16],
    /// The key code at `event + 8`.
    pub code: u32,
    /// The raw value at `event + 9`; `0x7f` is a press.
    pub value: u32,
    /// Whether this is a press.
    pub pressed: bool,
    /// Whether SHIFT was held before this event.
    pub shift: bool,
    /// Whether ERASE was held before this event.
    pub erase: bool,
    /// The surface object behind the event, or zero.
    pub surface: usize,
}

/// A control-change event.
#[derive(Clone, Copy, Debug)]
pub struct CcEvent {
    /// The app's event object.
    pub event: usize,
    /// The controller number.
    pub control: u32,
    /// The raw value.
    pub value: u32,
    /// The controller's low value.
    pub low: f32,
    /// The controller's range.
    pub range: f32,
}

/// What the mod wants done with an event.
///
/// The same decision for every event kind. For a MIDI note that the mod
/// rewrote with [`NoteEvent::replay_as_pad`], `Forward` makes the SDK forward
/// the rewritten event and then restore it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Hand the event to the app unchanged.
    Forward,
    /// Swallow it.
    Consume,
}

/// One paint of a supported native screen.
#[derive(Clone, Copy, Debug)]
pub struct PaintEvent {
    /// The component that was painted.
    pub component: usize,
    /// The graphics object to draw on.
    pub graphics: usize,
    /// Whether the component is the Sample page.
    pub sample_page: bool,
    /// Whether the page is in chop mode.
    pub chop_mode: bool,
}

/// A planar input hook: one slice per channel, then the rate.
pub type InputPlanar = fn(&[&[i32]], u32);

/// The audio-side hooks, copied out of the mod at install.
///
/// They run on the audio thread **without** the module lock, so they must only
/// touch atomics or other interior-mutable state.  They must never use the
/// mod's `&mut self` UI state, and they must not block or allocate.  They also
/// run without a per-thread reentry marker: do not call a hooked app site or
/// [`crate::loader::with_mod`] from an audio hook.
#[derive(Clone, Copy, Default)]
pub struct AudioHooks {
    /// Interleaved S32 output is about to be written; a hook may mix into it.
    pub output: Option<fn(&mut [i32], &crate::audio::OutputInfo)>,
    /// Interleaved S32 input arrived from the device.
    pub input_interleaved: Option<fn(&[i32], usize, u32)>,
    /// Planar S32 input arrived from the device, one slice per channel.
    pub input_planar: Option<InputPlanar>,
}

/// A pad decision and an optional token to finish after native forwarding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadResponse {
    /// Whether the SDK should pass this event to the firmware.
    pub flow: Flow,
    /// Opaque value returned to `pad_complete`; zero means no completion.
    pub token: u64,
}

/// Independent live input callbacks, called without the UI module lock.
///
/// These callbacks may run concurrently or re-enter. They must keep their own
/// bounded, nonblocking state and must not access the mod's UI-owned fields.
/// When present they replace the corresponding `Mod` callback.
#[derive(Clone, Copy, Default)]
pub struct InputHooks {
    /// Hardware pad press and release decisions.
    pub pad: Option<fn(PadEvent) -> PadResponse>,
    /// Complete a nonzero pad token after the native handler has returned.
    /// The boolean says whether the native handler was actually called.
    pub pad_complete: Option<fn(PadEvent, u64, bool)>,
    /// Control-surface pad and pressure events.
    pub surface: Option<fn(SurfaceEvent) -> Flow>,
    /// MIDI note decisions, including any in-place replay.
    pub note: Option<fn(&mut NoteEvent<'_>) -> Flow>,
}

/// A mod built on this SDK.
///
/// Every method has a default, so a mod overrides only what it needs.  The SDK
/// installs the hooks, decodes the events, forwards whatever the mod does not
/// consume, and keeps the app's originals private.
///
/// The app dispatches from several threads, so the module must be `Send`.  The
/// SDK still guarantees one exclusive `&mut self` at a time; an overlapping or
/// reentrant UI callback is forwarded without invoking the mod.  [`Mod::audio`]
/// is copied out once at install and its function pointers run on the audio
/// thread without the module lock.
pub trait Mod: Send + 'static {
    /// The hooks are installed and the firmware has been verified.
    fn on_start(&mut self) {}

    /// A pad was pressed or released.
    fn on_pad(&mut self, _event: PadEvent) -> Flow {
        Flow::Forward
    }

    /// A control-surface event arrived.
    fn on_surface(&mut self, _event: SurfaceEvent) -> Flow {
        Flow::Forward
    }

    /// The DATA encoder turned.
    fn on_encoder(&mut self, _event: EncoderEvent) -> Flow {
        Flow::Forward
    }

    /// A button was pressed or released.
    fn on_button(&mut self, _event: ButtonEvent) -> Flow {
        Flow::Forward
    }

    /// A control change arrived.
    fn on_cc(&mut self, _event: CcEvent) -> Flow {
        Flow::Forward
    }

    /// A MIDI note event arrived.
    fn on_note(&mut self, _event: &mut NoteEvent) -> Flow {
        Flow::Forward
    }

    /// The app is assigning a label; return a replacement or `None`.
    fn on_label(&mut self, _label: &CStr) -> Option<&'static CStr> {
        None
    }

    /// Observe or replace one native `juce::String` assignment.
    ///
    /// `dest` is the live native string object receiving `label`. Returning a
    /// replacement uses the same lifetime contract as [`Self::on_label`].
    fn on_text_assignment(&mut self, _dest: usize, label: &CStr) -> Option<&'static CStr> {
        self.on_label(label)
    }

    /// Observe a component repaint on the app's UI thread.
    fn on_repaint(&mut self, _component: usize) {}

    /// The 16 Levels renderer computed a pitch offset; return the one to apply.
    fn on_pitch(&mut self, applied: i32) -> i32 {
        applied
    }

    /// The fine tune to add to the offset [`Self::on_pitch`] returned, in steps
    /// of a tenth of a semitone.
    ///
    /// The app's pitch parameter carries a fine tune next to the semitones
    /// (its Tune page's K2), and the setter writes one property slot per call:
    /// slot 0 is the semitones, slot 1 the fine tune.  Measured on firmware
    /// 1.3.0.12, one step of that slot is a tenth of a semitone, and the slot
    /// spans ±12 semitones — so half a semitone is `5`, not `50`.  The SDK
    /// writes the slot only when this is non-zero, so a mod that tunes in whole
    /// semitones never touches it.  `applied` is the offset the renderer
    /// computed, the same value `on_pitch` received.
    fn on_pitch_fine(&mut self, _applied: i32) -> i32 {
        0
    }

    /// A screen painted.  The app has already drawn; `event.sample_page` says
    /// whether it was the Sample page.
    fn on_paint(&mut self, _graphics: &mut Graphics, _event: PaintEvent) {}

    /// Independent callbacks for time-sensitive input, copied at installation.
    ///
    /// These callbacks run without the UI lock; implementations must synchronize
    /// shared state without blocking, including when hooks run concurrently.
    fn input(&self) -> InputHooks {
        InputHooks::default()
    }

    /// The audio-side hooks, called on the audio thread without the module lock.
    ///
    /// They are copied out once at install; return only functions that read and
    /// write atomics or other interior-mutable state.
    fn audio(&self) -> AudioHooks {
        AudioHooks::default()
    }
}

/// Whether SHIFT is held, tracked by the SDK from the button hook.
static SHIFT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Whether ERASE is held, tracked by the SDK from the button hook.
static ERASE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The encoder's data target inside a surface object.
pub const DATA_TARGET_OFFSET: usize = 0x2da8;

/// The data target for an app surface, or zero when there is no surface.
#[must_use]
pub fn data_target(surface: usize) -> usize {
    if surface == 0 {
        0
    } else {
        surface.saturating_add(DATA_TARGET_OFFSET)
    }
}

/// Whether SHIFT is held right now.
#[must_use]
pub fn shift() -> bool {
    SHIFT.load(core::sync::atomic::Ordering::Acquire)
}

/// Whether ERASE is held right now.
#[must_use]
pub fn erase() -> bool {
    ERASE.load(core::sync::atomic::Ordering::Acquire)
}

/// Record the SHIFT state; the button site calls this after the mod decided.
pub(crate) fn set_shift(held: bool) {
    SHIFT.store(held, core::sync::atomic::Ordering::Release);
}

/// Record the ERASE state; the button site calls this after the mod decided.
pub(crate) fn set_erase(held: bool) {
    ERASE.store(held, core::sync::atomic::Ordering::Release);
}

/// The app's own button dispatcher, for a mod that replays an event.
#[must_use]
fn buttons_original() -> Option<unsafe extern "C" fn(usize)> {
    // SAFETY: written once at install.
    crate::sites::originals().buttons
}

/// Replay one key press/release on a button event captured on the current page.
///
/// Unlike [`replay_button`] this may run in a later callback, for example to
/// undo a page mode the app changed while a command ran. It first checks that
/// the event's context still looks like a live app object; the caller must
/// still ensure the event came from the page that is up now.
///
/// Returns false when the event has no context, the context is not a live app
/// object, or the button hook is missing.
///
/// # Safety
///
/// `event` must be the first 16 bytes of a button event the app dispatched on
/// the page that is current now, and this must run on that page's UI thread.
#[must_use]
pub unsafe fn replay_button_live(event: &[u8; 16], key: u8) -> bool {
    let Ok(word) = <[u8; 8]>::try_from(&event[..8]) else {
        return false;
    };
    let Ok(context) = usize::try_from(u64::from_le_bytes(word)) else {
        return false;
    };
    if !crate::image::is_app_object(context) {
        return false;
    }
    // SAFETY: the caller guarantees the event belongs to the current page and
    // the context check above proved it is still a live app object.
    unsafe { replay_button(event, key) }
}

/// Replay one key press/release on a captured button event.
///
/// Returns false when no event was captured or the button hook is missing.
///
/// # Safety
///
/// `event` must be the first 16 bytes of a button event captured during the
/// current callback, whose context and surface are still live.  Replaying it
/// later would dereference a stale app object.  A caller that keeps an event
/// across callbacks must use [`replay_button_live`], which checks the context.
#[must_use]
pub unsafe fn replay_button(event: &[u8; 16], key: u8) -> bool {
    let Some(original) = buttons_original() else {
        return false;
    };
    // A blank event has no context pointer; replaying it would dereference zero.
    if event[..8] == [0; 8] {
        return false;
    }
    let mut bytes = *event;
    bytes[8] = key;
    bytes[9] = 0x7f;
    // SAFETY: the captured event is the app's own and the original is its own
    // dispatcher.
    unsafe { original(bytes.as_ptr() as usize) };
    bytes[9] = 0;
    // SAFETY: as above.
    unsafe { original(bytes.as_ptr() as usize) };
    true
}
