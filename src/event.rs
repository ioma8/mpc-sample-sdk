//! The events a mod handles and the flows it returns.
//!
//! Every hook the SDK installs is decoded into one of these values; the mod
//! decides whether the event is consumed or handed back to the app.  Passing the
//! app's own original call is the SDK's job, so a mod never stores or types a
//! displaced function.

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

/// What the mod wants done with a pad event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadFlow {
    /// Hand the event to the app unchanged.
    Forward,
    /// Swallow it.
    Consume,
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

/// What the mod wants done with a control-surface event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceFlow {
    /// Hand the event to the app unchanged.
    Forward,
    /// Swallow it.
    Consume,
}

/// A turn of the DATA encoder.
#[derive(Clone, Copy, Debug)]
pub struct EncoderEvent {
    /// The encoder's surface object.
    pub surface: usize,
    /// The signed delta.
    pub delta: i32,
}

/// What the mod wants done with an encoder turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderFlow {
    /// Hand the event to the app unchanged.
    Forward,
    /// Swallow it.
    Consume,
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

/// What the mod wants done with a button event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonFlow {
    /// Hand the event to the app unchanged.
    Forward,
    /// Swallow it.
    Consume,
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

/// What the mod wants done with a control-change event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CcFlow {
    /// Hand the event to the app unchanged.
    Forward,
    /// Swallow it.
    Consume,
}

/// What the mod wants done with a MIDI note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteFlow {
    /// Forward the event with its original context and source.
    Forward,
    /// Forward the rewritten event as the selected pad.
    Replayed,
    /// Swallow it.
    Consume,
}

/// One paint of the Sample page.
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

/// A mod built on this SDK.
///
/// Every method has a default, so a mod overrides only what it needs.  The SDK
/// installs the hooks, decodes the events, forwards whatever the mod does not
/// consume, and keeps the app's originals private.
///
/// The app dispatches from several threads, so the module must be `Send`.  The
/// SDK still guarantees one exclusive `&mut self` at a time; an overlapping or
/// reentrant callback is forwarded without invoking the mod.
pub trait Mod: Send + 'static {
    /// The hooks are installed and the firmware has been verified.
    fn on_start(&mut self) {}

    /// A pad was pressed or released.
    fn on_pad(&mut self, _event: PadEvent) -> PadFlow {
        PadFlow::Forward
    }

    /// A control-surface event arrived.
    fn on_surface(&mut self, _event: SurfaceEvent) -> SurfaceFlow {
        SurfaceFlow::Forward
    }

    /// The DATA encoder turned.
    fn on_encoder(&mut self, _event: EncoderEvent) -> EncoderFlow {
        EncoderFlow::Forward
    }

    /// A button was pressed or released.
    fn on_button(&mut self, _event: ButtonEvent) -> ButtonFlow {
        ButtonFlow::Forward
    }

    /// A control change arrived.
    fn on_cc(&mut self, _event: CcEvent) -> CcFlow {
        CcFlow::Forward
    }

    /// A MIDI note event arrived.
    fn on_note(&mut self, _event: &mut NoteEvent) -> NoteFlow {
        NoteFlow::Forward
    }

    /// The app is assigning a label; return a replacement or `None`.
    fn on_label(&mut self, _label: &CStr) -> Option<&'static CStr> {
        None
    }

    /// The 16 Levels renderer computed a pitch offset; return the one to apply.
    fn on_pitch(&mut self, applied: i32) -> i32 {
        applied
    }

    /// A screen painted.  The app has already drawn; `event.sample_page` says
    /// whether it was the Sample page.
    fn on_paint(&mut self, _graphics: &mut Graphics, _event: PaintEvent) {}

    /// Interleaved S32 input arrived from the device.
    fn on_input_interleaved(&mut self, _frames: &[i32], _channels: usize, _rate: u32) {}

    /// Planar S32 input arrived from the device, one slice per channel.
    fn on_input_planar(&mut self, _channels: &[&[i32]], _rate: u32) {}

    /// Interleaved S32 output is about to be written; a mod may mix into it.
    fn on_output(&mut self, _frames: &mut [i32], _info: &crate::audio::OutputInfo) {}
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
