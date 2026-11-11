//! Which sample the Sample page has selected.
//!
//! Everything the sample actions do starts here: find the screen's binding, walk
//! to the instrument, its layer list and the selected layer's sample, then read
//! the sample's own geometry.  A stale pointer anywhere in that chain means "no
//! sample", never a guess.

use crate::image;

/// The Sample page's vtable, as an offset from the image base.
pub const SCREEN_VTABLE: usize = 0x6e2_12a0;
/// `[screen + 0x178]` is the page's binding.
const SCREEN_BINDING: usize = 0x178;
/// `[binding + 0x60]` is the state the instrument lives in.
const BINDING_STATE: usize = 0x60;
/// `[state + 0x22e8]` is the layer list.
const STATE_LAYERS: usize = 0x22e8;
/// `[layers + 0x4a8]` is the selected sample.
const LAYERS_SAMPLE: usize = 0x4a8;
/// `[sample + 0x130]` is its metadata block.
const SAMPLE_METADATA: usize = 0x130;
/// The app's "is this page showing a sample" predicate.
const PAGE_HAS_SAMPLE: usize = 0x112_fd50;
/// `[state + 0x2310]` is the app's program object.
const STATE_PROGRAM: usize = 0x2310;
/// Shortest and longest sample rate the app accepts.
const RATE_RANGE: core::ops::RangeInclusive<u32> = 8_000..=192_000;

/// Why the selected sample's audio could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioReadError {
    /// No exact WAV path could be reconstructed.
    NoPath,
    /// The WAV exists but could not be decoded.
    DecodeFailed,
    /// The decoded length does not match the selection.
    LengthMismatch,
}

/// The selected sample and where it was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The screen the walk started from.
    pub(crate) screen: usize,
    /// The page state the instrument lives in.
    pub(crate) state: usize,
    /// The selected sample object.
    pub(crate) sample: usize,
    /// Its length in frames.
    pub(crate) frames: u32,
    /// Its channel count.
    pub(crate) channels: u16,
    /// Its sample rate.
    pub(crate) rate: u32,
}

/// The state behind a Sample page, even when its selected pad is empty.
///
/// Synth actions do not read a source sample, so they only need the page
/// binding/state to hand Extract the right program.  Unlike [`current`], this
/// deliberately does not require a selected sample.
#[must_use]
pub fn page_state(screen: usize) -> Option<usize> {
    if screen == 0 {
        return None;
    }
    if image::read_pointer(screen)? != image::base() + SCREEN_VTABLE {
        return None;
    }
    let binding = image::read_pointer(screen.checked_add(SCREEN_BINDING)?)?;
    image::read_pointer(binding.checked_add(BINDING_STATE)?)
}

/// The sample the Sample page has selected, or `None`.
///
/// # Safety
///
/// `screen` must be the live Sample page recorded on its owning UI thread.  The
/// vtable check rejects a foreign object, but cannot prove that a forged address
/// or a page destroyed after recording is still a valid native page.
#[must_use]
pub unsafe fn current(screen: usize) -> Option<Selection> {
    let state = page_state(screen)?;
    // SAFETY: `image::base() + PAGE_HAS_SAMPLE` is the page's own predicate and `screen`
    // is a Sample page.
    if unsafe { image::int_of_pointer(image::base() + PAGE_HAS_SAMPLE, screen) } == 0 {
        return None;
    }
    let layers = image::read_pointer(state.checked_add(STATE_LAYERS)?)?;
    let sample = image::read_pointer(layers.checked_add(LAYERS_SAMPLE)?)?;
    let metadata = image::read_pointer(sample.checked_add(SAMPLE_METADATA)?)?;
    let channels = u16::try_from(image::read_u32(metadata)? & 0xffff).ok()?;
    let frames = image::read_u32(metadata.checked_add(4)?)?;
    let rate = image::read_u32(metadata.checked_add(8)?)?;
    if !matches!(channels, 1 | 2) || frames == 0 || !RATE_RANGE.contains(&rate) {
        return None;
    }
    Some(Selection {
        screen,
        state,
        sample,
        frames,
        channels,
        rate,
    })
}

/// The program object behind a page state, or `None`.
#[must_use]
pub fn program(state: usize) -> Option<usize> {
    image::read_pointer(state.checked_add(STATE_PROGRAM)?).filter(|program| *program != 0)
}

impl Selection {
    /// Build a selection for a synthetic source, such as a synth take.
    ///
    /// This is the explicit constructor for actions that only need the page and
    /// state to drive Extract; no selected sample is required.
    ///
    /// # Safety
    ///
    /// `screen` and `state` must be live app objects on their owning UI thread.
    /// The geometry must describe the audio the caller will submit; it is not
    /// read back from a sample object.
    #[must_use]
    pub const unsafe fn from_parts(
        screen: usize,
        state: usize,
        sample: usize,
        frames: u32,
        channels: u16,
        rate: u32,
    ) -> Self {
        Self {
            screen,
            state,
            sample,
            frames,
            channels,
            rate,
        }
    }

    /// The screen the selection was read from.
    #[must_use]
    pub const fn screen(&self) -> usize {
        self.screen
    }

    /// The page state the instrument lives in.
    #[must_use]
    pub const fn state(&self) -> usize {
        self.state
    }

    /// The selected sample object.
    #[must_use]
    pub const fn sample(&self) -> usize {
        self.sample
    }

    /// The sample's length in frames.
    #[must_use]
    pub const fn frames(&self) -> u32 {
        self.frames
    }

    /// The sample's channel count.
    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }

    /// The sample's rate in Hz.
    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.rate
    }

    /// Whether this exact source is still the selected, visible Sample page.
    #[must_use]
    pub fn is_current(&self) -> bool {
        // SAFETY: `screen` came from a validated selection and this sample is a
        // live app object while the mod handles the callback.
        if self.screen == 0 || !unsafe { super::page::visible(self.screen) } {
            return false;
        }
        // SAFETY: `self.screen` came from a validated selection and the sample
        // is live while the mod handles the callback.
        unsafe { current(self.screen) }.is_some_and(|now| now == *self)
    }

    /// Read this selection's audio, preferring the loaded buffer over its WAV.
    ///
    /// The selection is a value snapshot, but the read touches the app's sample
    /// buffer.  A caller that reads off the UI thread must revalidate with
    /// [`Self::is_current`] before using the result; the UI may replace the
    /// sample while the read runs.
    ///
    /// # Errors
    ///
    /// Returns why the audio is unavailable: no exact path, a decode failure, or
    /// a length that does not match the selection.
    pub fn read_audio(&self) -> Result<Vec<f32>, AudioReadError> {
        let cells = usize::try_from(self.frames)
            .ok()
            .and_then(|frames| frames.checked_mul(usize::from(self.channels)));
        let Some(cells) = cells else {
            return Err(AudioReadError::LengthMismatch);
        };
        if let Some(audio) = super::sample::memory(self.sample)
            .filter(|memory| {
                memory.frames == self.frames
                    && memory.channels == self.channels
                    && memory.rate == self.rate
            })
            .and_then(|memory| memory.read())
        {
            return if audio.len() == cells {
                Ok(audio)
            } else {
                Err(AudioReadError::LengthMismatch)
            };
        }
        // A sample without a stored path stays unresolved: guessing across
        // temporary projects could read an unrelated recording of equal length.
        let path = super::paths::for_sample(self.sample, self.frames, self.channels, self.rate)
            .ok_or(AudioReadError::NoPath)?;
        let audio = super::wav::read_f32(&path, self.frames, self.channels, self.rate)
            .ok_or(AudioReadError::DecodeFailed)?;
        if audio.len() == cells {
            Ok(audio)
        } else {
            Err(AudioReadError::LengthMismatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RATE_RANGE, SCREEN_VTABLE};

    #[test]
    fn the_screen_vtable_is_the_documented_one() {
        assert_eq!(SCREEN_VTABLE, 0x6e2_12a0);
        assert!(RATE_RANGE.contains(&44_100));
        assert!(!RATE_RANGE.contains(&4_000));
        // A zero screen is never a selection.
        // SAFETY: null is rejected without native calls.
        assert!(unsafe { super::current(0) }.is_none());
    }
}
