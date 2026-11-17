//! `mpc-sample-sdk` — the universal mod SDK for the Akai MPC Sample.
//!
//! The SDK owns everything that is not a feature: finding and verifying the
//! loaded firmware image, the in-memory hook engine, the input/audio/MIDI event
//! plumbing, drawing on the app's own `Graphics` object, and typed access to the
//! app's own objects (page, selection, sample, pads, slices, WAV files, the
//! Extract command).
//!
//! A mod implements [`Mod`] and installs it once:
//!
//! ```no_run
//! use mpc_sample_sdk::{Mod, PadEvent, Flow};
//!
//! #[derive(Default)]
//! struct MyMod;
//!
//! impl Mod for MyMod {
//!     fn on_pad(&mut self, event: PadEvent) -> Flow {
//!         let _ = event;
//!         Flow::Forward
//!     }
//! }
//!
//! mpc_sample_sdk::export_mod!(MyMod);
//! ```
//!
//! The SDK deliberately knows nothing about any feature: no looper, no synth, no
//! menu.  Those live in the mod crate and only use the API here.

pub mod audio;
pub mod browser;
pub mod draw;
pub mod event;
mod firmware;
pub mod hook;
pub mod image;
pub mod loader;
pub mod midi;
pub mod platform;
pub mod render;
mod sites;
pub mod state;
pub mod task;

pub use event::{
    AudioHooks, ButtonEvent, CcEvent, EncoderEvent, Flow, InputHooks, Mod, NoteEvent, PadAction,
    PadEvent, PadResponse, PaintEvent, SurfaceEvent, erase, shift,
};
pub use loader::{InstallError, install};
