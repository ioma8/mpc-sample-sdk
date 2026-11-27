//! Which sample the Sample page has selected.
//!
//! Everything the sample actions do starts here. The walk itself lives in
//! `descent`; this module turns a validated walk into a [`Selection`]
//! and reads the selected sample's audio. A stale pointer anywhere in the chain
//! means "no sample", never a guess.

use super::descent::{Page, State};
use crate::image;

#[cfg(feature = "fw-1-3")]
const TRIM_START_OFFSET: usize = 0x7b8;
#[cfg(feature = "fw-1-3")]
const TRIM_END_OFFSET: usize = 0x810;
#[cfg(feature = "fw-1-3")]
const TRIM_START_MIRROR_OFFSET: usize = 0x868;

/// The app's "is this page showing a sample" predicate.
const PAGE_HAS_SAMPLE: usize = crate::firmware::CHOP_PREDICATE.offset;

/// Why the selected sample's audio could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioReadError {
    /// No exact WAV path could be reconstructed.
    NoPath,
    /// The WAV exists but could not be decoded.
    DecodeFailed,
    /// The decoded length does not match the selection.
    LengthMismatch,
    /// The Sample page moved on, or the sample changed, while the audio was read.
    Stale,
}

/// The selected sample and where it was found.
///
/// A selection is a snapshot: [`Selection::is_current`] says whether the page
/// still shows this exact sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    screen: usize,
    state: usize,
    sample: usize,
    frames: u32,
    channels: u16,
    rate: u32,
}

/// The state behind a Sample page, even when its selected pad is empty.
///
/// Synth actions do not read a source sample, so they only need the page
/// binding/state to hand Extract the right program. Unlike [`current`], this
/// deliberately does not require a selected sample.
#[must_use]
pub fn page_state(screen: usize) -> Option<usize> {
    Page::bind(screen)?.state().map(State::address)
}

/// The sample the Sample page has selected, or `None`.
///
/// # Safety
///
/// `screen` must be the live Sample page recorded on its owning UI thread. The
/// vtable check rejects a foreign object, but cannot prove that a forged address
/// or a page destroyed after recording is still a valid native page.
#[must_use]
pub unsafe fn current(screen: usize) -> Option<Selection> {
    let page = Page::bind(screen)?;
    let state = page.state()?;
    // SAFETY: `image::base() + PAGE_HAS_SAMPLE` is the page's own predicate and
    // `screen` is a Sample page.
    if unsafe { image::int_of_pointer(image::base() + PAGE_HAS_SAMPLE, screen) } == 0 {
        return None;
    }
    let sample = state.layers()?.sample()?;
    let geometry = super::descent::geometry(sample)?;
    Some(Selection {
        screen,
        state: state.address(),
        sample,
        frames: geometry.frames,
        channels: geometry.channels,
        rate: geometry.rate,
    })
}

/// The program object behind a page state, or `None`.
#[must_use]
pub fn program(state: usize) -> Option<usize> {
    State::at(state).program()
}

impl Selection {
    /// Resolve the native control context and program for the visible page.
    ///
    /// # Safety
    /// The selection must be live on the UI thread.
    #[cfg(feature = "fw-1-4")]
    #[must_use]
    pub unsafe fn trim_control(&self) -> Option<(usize, usize)> {
        // SAFETY: the caller guarantees the page is live on its UI thread.
        if !unsafe { self.is_current() } {
            return None;
        }
        let first = image::read_pointer(self.screen.checked_add(0x110)?)?;
        let second = image::read_pointer(self.screen.checked_add(0x168)?)?;
        let control = image::read_pointer(first.checked_add(0x50)?)?;
        if control == 0 || image::read_pointer(second.checked_add(0x20)?)? != control {
            return None;
        }
        if !image::is_app_object(control) {
            return None;
        }
        Some((control, program(self.state)?))
    }
    /// A selection with the given fields, for tests that never touch the app.
    #[cfg(test)]
    pub(crate) const fn fixture(
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

    /// Set the selected sample's inclusive trim bounds in its live layer.
    ///
    /// This is for the case where the page has not yet sent a knob event, so
    /// the SDK has no native control-surface context to call its setter with.
    /// The Sample page model stores these three fields directly; the caller
    /// should request a repaint after this succeeds.
    ///
    /// # Safety
    /// The page must remain live and this must run on its owning UI thread.
    #[cfg(feature = "fw-1-3")]
    #[must_use]
    pub unsafe fn set_trim_bounds(&self, first: u32, last: u32) -> bool {
        // SAFETY: `self` came from `current`; this also rejects a page whose
        // selection changed before the write.
        if !unsafe { self.is_current() } || first > last || last >= self.frames {
            return false;
        }
        let Some(layers_field) = self.state.checked_add(super::descent::STATE_LAYERS_OFFSET) else {
            return false;
        };
        let Some(layers) = image::read_pointer(layers_field) else {
            return false;
        };
        let Some(sample_slot) = layers.checked_add(super::descent::LAYERS_SAMPLE_OFFSET) else {
            return false;
        };
        if image::read_pointer(sample_slot) != Some(self.sample) {
            return false;
        }
        let first = first.to_le_bytes();
        let last = last.to_le_bytes();
        // SAFETY: selection identity and the live layer were checked above;
        // these are the documented u32 trim fields in that layer.
        unsafe {
            image::write(layers + TRIM_START_OFFSET, &first)
                && image::write(layers + TRIM_END_OFFSET, &last)
                && image::write(layers + TRIM_START_MIRROR_OFFSET, &first)
        }
    }

    /// Whether this exact source is still the selected, visible Sample page.
    #[must_use]
    /// # Safety
    /// The page must remain live and this must run on its owning UI thread.
    pub unsafe fn is_current(&self) -> bool {
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
    /// The selection must still be current before and after the copy. The UI
    /// may replace the sample while the read runs, so a read that cannot be
    /// confirmed is refused rather than returned.
    ///
    /// # Errors
    ///
    /// Returns why the audio is unavailable: the selection is stale, there is no
    /// exact path, the decode failed, or the length does not match.
    /// # Safety
    /// The page and sample must remain live throughout the read, which must run
    /// on their owning UI thread.
    pub unsafe fn read_current(&self) -> Result<Vec<f32>, AudioReadError> {
        // SAFETY: the caller guarantees the page is live on its UI thread.
        if !unsafe { self.is_current() } {
            return Err(AudioReadError::Stale);
        }
        let audio = self.copy_audio()?;
        // SAFETY: the caller guarantees the page remains live.
        if !unsafe { self.is_current() } {
            return Err(AudioReadError::Stale);
        }
        Ok(audio)
    }

    /// The copy `read_current` brackets: the loaded buffer, else the WAV.
    fn copy_audio(&self) -> Result<Vec<f32>, AudioReadError> {
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
    use super::current;

    #[test]
    fn a_zero_screen_is_never_a_selection() {
        // SAFETY: null is rejected without native calls.
        assert!(unsafe { current(0) }.is_none());
    }
}
