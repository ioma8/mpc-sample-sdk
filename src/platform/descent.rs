//! The Sample page's object graph, walked in one place.
//!
//! The chain is page → binding → state → layers → selected sample → metadata.
//! Every module that needs the selection starts from these steps instead of
//! replaying the offsets, so a layout change or a sample-geometry rule is edited
//! once. Each step returns `None` for a null or unreadable pointer, so a stale
//! pointer anywhere in the chain means "no sample", never a guess.

use core::ops::RangeInclusive;

use crate::firmware;
use crate::image;

/// `[page + 0x178]` is the Sample page's binding (`+0x1b8` on 1.4.0.8).
#[cfg(feature = "fw-1-4")]
const PAGE_BINDING: usize = 0x1b8;
#[cfg(feature = "fw-1-3")]
const PAGE_BINDING: usize = 0x178;
/// `[binding + 0x60]` is the state the app's Extract handler reads.
const BINDING_STATE: usize = 0x60;
/// `[state + 0x22e8]` is the layer list (`+0x23a8` on 1.4.0.8).
#[cfg(feature = "fw-1-4")]
pub(crate) const STATE_LAYERS_OFFSET: usize = 0x23a8;
#[cfg(feature = "fw-1-3")]
pub(crate) const STATE_LAYERS_OFFSET: usize = 0x22e8;
/// `[state + 0x2310]` is the app's program object (`+0x23d0` on 1.4.0.8).
#[cfg(feature = "fw-1-4")]
const STATE_PROGRAM: usize = 0x23d0;
#[cfg(feature = "fw-1-3")]
const STATE_PROGRAM: usize = 0x2310;
/// `[layers + 0x4a8]` is the slot holding the selected sample.
/// The layers offset of the selected-sample slot, for layouts that reach past it.
pub(crate) const LAYERS_SAMPLE_OFFSET: usize = 0x4a8;
/// `[sample + 0x130]` is its metadata block.
pub(crate) const SAMPLE_METADATA_OFFSET: usize = 0x130;
/// Shortest and longest sample rate the app accepts.
pub(crate) const RATE_RANGE: RangeInclusive<u32> = 8_000..=192_000;

/// The Sample page, after its vtable has been recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Page(usize);

impl Page {
    /// Recognise a Sample page by its vtable. Any other object yields `None`.
    pub(crate) fn bind(screen: usize) -> Option<Self> {
        if screen == 0 {
            return None;
        }
        let vtable = image::read_pointer(screen)?;
        (vtable == image::base() + firmware::SAMPLE_PAGE_VTABLE.offset).then_some(Self(screen))
    }

    /// The state the page's binding drives.
    pub(crate) fn state(self) -> Option<State> {
        let binding = image::read_pointer(self.0.checked_add(PAGE_BINDING)?)?;
        image::read_pointer(binding.checked_add(BINDING_STATE)?).map(State)
    }
}

/// A page state, which holds the instrument's layers and program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct State(usize);

impl State {
    /// A state at a known address, such as one the app handed a callback.
    pub(crate) const fn at(address: usize) -> Self {
        Self(address)
    }

    /// The state's address.
    pub(crate) const fn address(self) -> usize {
        self.0
    }

    /// The layer list.
    pub(crate) fn layers(self) -> Option<Layers> {
        image::read_pointer(self.0.checked_add(STATE_LAYERS_OFFSET)?).map(Layers)
    }

    /// The program object, when the app has one.
    pub(crate) fn program(self) -> Option<usize> {
        image::read_pointer(self.0.checked_add(STATE_PROGRAM)?).filter(|program| *program != 0)
    }
}

/// A state's layer list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layers(usize);

impl Layers {
    /// A layer list at a known address, such as one a pad's state points at.
    pub(crate) const fn at(address: usize) -> Self {
        Self(address)
    }

    /// The layer list's address.
    pub(crate) const fn address(self) -> usize {
        self.0
    }

    /// The address of the slot that holds the selected sample.
    pub(crate) fn sample_slot(self) -> Option<usize> {
        self.0.checked_add(LAYERS_SAMPLE_OFFSET)
    }

    /// The selected sample object.
    pub(crate) fn sample(self) -> Option<usize> {
        image::read_pointer(self.sample_slot()?)
    }
}

/// The address of a sample's metadata block: `{u16 channels, u32 frames, u32 rate}`.
pub(crate) fn metadata_block(sample: usize) -> Option<usize> {
    image::read_pointer(sample.checked_add(SAMPLE_METADATA_OFFSET)?)
}

/// A sample's geometry, for the samples the SDK accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Geometry {
    /// Channel count, one or two.
    pub(crate) channels: u16,
    /// Length in frames, never zero.
    pub(crate) frames: u32,
    /// Sample rate, within [`RATE_RANGE`].
    pub(crate) rate: u32,
}

/// Read a sample's geometry. Only mono or stereo, non-empty samples at an
/// accepted rate pass; everything else is `None`.
pub(crate) fn geometry(sample: usize) -> Option<Geometry> {
    let metadata = metadata_block(sample)?;
    let channels = u16::try_from(image::read_u32(metadata)? & 0xffff).ok()?;
    let frames = image::read_u32(metadata.checked_add(4)?)?;
    let rate = image::read_u32(metadata.checked_add(8)?)?;
    let accepted = matches!(channels, 1 | 2) && frames != 0 && RATE_RANGE.contains(&rate);
    accepted.then_some(Geometry {
        channels,
        frames,
        rate,
    })
}

#[cfg(test)]
mod tests {
    use super::{Geometry, Page, RATE_RANGE, SAMPLE_METADATA_OFFSET, geometry};
    use crate::firmware;
    use crate::image::test_memory;

    /// Fixture regions for one sample whose metadata reads `channels`, `frames`
    /// and `rate`.
    fn sample_with(channels: u32, frames: u32, rate: u32) -> (usize, Vec<(usize, Vec<u8>)>) {
        let sample: usize = 0x0010_0000;
        let metadata: usize = 0x0020_0000;
        let regions = vec![
            (
                sample + SAMPLE_METADATA_OFFSET,
                metadata.to_le_bytes().to_vec(),
            ),
            (metadata, channels.to_le_bytes().to_vec()),
            (metadata + 4, frames.to_le_bytes().to_vec()),
            (metadata + 8, rate.to_le_bytes().to_vec()),
        ];
        (sample, regions)
    }

    #[test]
    fn the_rate_policy_is_the_apps_range() {
        assert!(RATE_RANGE.contains(&44_100));
        assert!(RATE_RANGE.contains(&192_000));
        assert!(!RATE_RANGE.contains(&4_000));
    }

    #[test]
    fn a_stereo_sample_at_an_accepted_rate_has_geometry() {
        let (sample, regions) = sample_with(2, 44_100, 48_000);
        let found = test_memory::with(regions, || geometry(sample));
        assert_eq!(
            found,
            Some(Geometry {
                channels: 2,
                frames: 44_100,
                rate: 48_000,
            })
        );
    }

    #[test]
    fn empty_surround_and_out_of_range_samples_have_no_geometry() {
        for (channels, frames, rate) in [(2, 0, 48_000), (6, 10, 48_000), (1, 10, 4_000)] {
            let (sample, regions) = sample_with(channels, frames, rate);
            assert_eq!(test_memory::with(regions, || geometry(sample)), None);
        }
    }

    #[test]
    fn a_sample_page_leads_to_its_selected_sample_and_geometry() {
        let screen: usize = 0x0050_0000;
        let binding: usize = 0x0051_0000;
        let state: usize = 0x0052_0000;
        let layers: usize = 0x0053_0000;
        let sample: usize = 0x0054_0000;
        let metadata: usize = 0x0055_0000;
        let pointer = |address: usize| address.to_le_bytes().to_vec();
        let regions = vec![
            (screen, pointer(firmware::SAMPLE_PAGE_VTABLE.offset)),
            (screen + super::PAGE_BINDING, pointer(binding)),
            // Extract reads this pointer at +0x60. A plausible but unrelated
            // pointer at +0x68 must not be followed.
            (binding + 0x60, pointer(state)),
            (binding + 0x68, pointer(0x0052_0001)),
            (state + super::STATE_LAYERS_OFFSET, pointer(layers)),
            (layers + super::LAYERS_SAMPLE_OFFSET, pointer(sample)),
            (sample + SAMPLE_METADATA_OFFSET, pointer(metadata)),
            (metadata, 2_u32.to_le_bytes().to_vec()),
            (metadata + 4, 44_100_u32.to_le_bytes().to_vec()),
            (metadata + 8, 48_000_u32.to_le_bytes().to_vec()),
        ];
        let walked = test_memory::with(regions, || {
            let page = Page::bind(screen)?;
            let state = page.state()?;
            let sample = state.layers()?.sample()?;
            Some((state.address(), sample, geometry(sample)?))
        });
        assert_eq!(
            walked,
            Some((
                state,
                sample,
                Geometry {
                    channels: 2,
                    frames: 44_100,
                    rate: 48_000,
                },
            ))
        );
    }
}
