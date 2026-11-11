//! The app's own objects and services.
//!
//! Every module here is a typed view over the loaded firmware: no policy, no
//! feature state.  A mod composes them through the [`crate::Mod`] trait or calls
//! them directly from its own feature code.

pub mod clock;
pub mod control;
pub mod effect;
pub mod extract;
pub mod pads;
pub mod page;
pub mod paths;
pub mod sample;
pub mod seed;
pub mod selection;
pub mod slice;
pub mod status;
pub mod trim;
pub mod wav;
pub mod writer;
