//! Build Rust mods for the Akai MPC Sample.
//!
//! `mpc-sample-sdk` provides the runtime boundary between a Rust mod and the
//! stock MPC Sample application: it verifies the supported firmware, installs
//! in-memory hooks, turns native input into Rust events, and exposes typed APIs
//! for audio, drawing, MIDI, the sample browser, and app-owned sample objects.
//! A mod supplies feature behavior; this crate supplies the firmware bindings.
//!
//! # A minimal mod
//!
//! A mod implements [`Mod`] and exports its entry point with [`export_mod!`].
//! Return [`Flow::Forward`] to let the stock app handle an event, or
//! [`Flow::Consume`] when the mod has handled it.
//!
//! ```no_run
//! use mpc_sample_sdk::{ButtonEvent, Flow, Mod};
//!
//! #[derive(Default)]
//! struct MyMod;
//!
//! impl Mod for MyMod {
//!     fn on_button(&mut self, event: ButtonEvent) -> Flow {
//!         if event.code == mpc_sample_sdk::event::button::SAMPLE_RECORD
//!             && event.pressed
//!         {
//!             mpc_sample_sdk::draw::show_toast("Sample Record pressed");
//!             Flow::Consume
//!         } else {
//!             Flow::Forward
//!         }
//!     }
//! }
//!
//! mpc_sample_sdk::export_mod!(MyMod);
//! ```
//!
//! The macro creates the process entry point that installs the mod when the
//! library loads. For an explicit lifecycle, [`loader::install`] is available;
//! it must be called once before the app starts its threads.
//!
//! # Runtime model
//!
//! The SDK verifies the loaded MPC Sample firmware before patching any code.
//! If verification or hook installation fails, the mod is not activated. The
//! SDK keeps displaced firmware functions private and forwards events according
//! to the [`Flow`] returned by the mod.
//!
//! Most [`Mod`] callbacks run with exclusive mutable access to the mod and must
//! return promptly. Audio callbacks and optional [`InputHooks`] run without that
//! module lock; they can run concurrently, so their state must use atomics or
//! other appropriate synchronization. Audio callbacks are real-time paths: do
//! not block, allocate, or call back into hooked app functions from them.
//!
//! Firmware-owned memory and native calls have explicit `unsafe` contracts.
//! Prefer the safe event and platform APIs, and use a small documented unsafe
//! block only when its stated lifetime, thread-ownership, and aliasing
//! requirements can be guaranteed. See [`state`] for process-wide UI state and
//! the [safety migration guide] for the API's safety model.
//!
//! # API map
//!
//! | Module | Use it for |
//! | --- | --- |
//! | [`event`] | [`Mod`] callbacks, button/pad/encoder events, callback flow |
//! | [`audio`] | ALSA input/output hooks, stream geometry, latency |
//! | [`draw`] | Paint callbacks, graphics, toasts, repaint requests |
//! | [`platform`] | Typed access to the app's pages, samples, pads, slices, WAVs, and effects |
//! | [`browser`] | Virtual directories and successful file-open observation |
//! | [`midi`] | MIDI note events and controlled note replay |
//! | [`task`] and [`render`] | Background work and audio-render handoff |
//! | [`loader`] | Firmware verification and explicit installation |
//!
//! # Target and compatibility
//!
//! This SDK targets the MPC Sample's **1.3.0.12 firmware** and its
//! **`AArch64` Linux** process. A mod is built as a Rust `cdylib` and loaded into the stock
//! application with `LD_PRELOAD`; this is not a general-purpose desktop audio
//! library. The SDK repository's [build and install guide] covers cross-building
//! and packaging an SD-card `autorun` directory. The [hands-on guide] walks
//! through each API with examples.
//!
//! ```toml
//! [lib]
//! crate-type = ["cdylib"]
//!
//! [dependencies]
//! mpc-sample-sdk = "0.7"
//! ```
//!
//! [safety migration guide]: https://github.com/ioma8/mpc-sample-sdk/blob/main/docs/24-safety-migration.md
//! [build and install guide]: https://github.com/ioma8/mpc-sample-sdk/blob/main/docs/22-build-and-install.md
//! [hands-on guide]: https://github.com/ioma8/mpc-sample-sdk/blob/main/docs/README.md

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
pub use sites::replace_native_text_editor;
