//! Geometry accessors for the app's sample and instrument objects.
//!
//! Both are pure reads of firmware memory, so they live here as small typed
//! helpers instead of raw pointer arithmetic in the glue.  The offsets are the
//! reverse-engineered ones and change with the firmware.

use crate::image;

/// `[sample + 0x130]` is the sample's metadata block.
const SAMPLE_METADATA: usize = 0x130;
/// Shortest and longest sample rate the app accepts.
const RATE_RANGE: core::ops::RangeInclusive<u32> = 8_000..=192_000;

/// The frame count of the sample object, or zero when its metadata is not a
/// usable sample description.
#[must_use]
pub fn frames(sample: usize) -> u32 {
    if sample == 0 {
        return 0;
    }
    let Some(metadata) = sample
        .checked_add(SAMPLE_METADATA)
        .and_then(image::read_pointer)
    else {
        return 0;
    };
    let Some(channels) = image::read_u32(metadata) else {
        return 0;
    };
    let Ok(channels) = u16::try_from(channels & 0xffff) else {
        return 0;
    };
    if channels != 1 && channels != 2 {
        return 0;
    }
    let (Some(frames), Some(rate)) = (image::read_u32(metadata + 4), image::read_u32(metadata + 8))
    else {
        return 0;
    };
    if !RATE_RANGE.contains(&rate) {
        return 0;
    }
    frames
}

/// `[sample + 0x170]` is the object that holds the audio in memory.
const SAMPLE_HOLDER: usize = 0x170;
/// `[holder + 0x18]` points back at the sample's metadata block.
const HOLDER_BACK: usize = 0x18;
/// `[holder + 0x28]` is the buffer's owner; its first word is the buffer.
const HOLDER_BUFFER: usize = 0x28;
/// `[metadata + 0x3e4]` is the note the sample plays at its original speed.
const METADATA_ROOT: usize = 0x3e4;
/// The root note when the app's value cannot be read: C3.
pub const DEFAULT_ROOT: i32 = 60;
/// 256 MB of float32: larger than any sample the firmware holds in memory.
const MEMORY_AUDIO_MAX_CELLS: usize = 64 * 1024 * 1024;
/// Bytes copied from the app per read.
const READ_CHUNK: usize = 64 * 1024;

/// A sample the app keeps in memory as interleaved float32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    /// Address of the first sample.
    pub buffer: usize,
    /// Length in frames.
    pub frames: u32,
    /// Interleaved channels, one or two.
    pub channels: u16,
    /// Sample rate.
    pub rate: u32,
}

/// Whether the object's class name (without the length prefix) is `want`.
fn class_is(object: usize, want: &str) -> bool {
    let Some(vtable) = image::read_pointer(object) else {
        return false;
    };
    let Some(info) = vtable.checked_sub(8).and_then(image::read_pointer) else {
        return false;
    };
    let Some(name) = info.checked_add(8).and_then(image::read_pointer) else {
        return false;
    };
    let mut text = [0_u8; 63];
    if !image::read(name, &mut text) {
        return false;
    }
    let end = text
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(text.len());
    let name = &text[..end];
    let digits = name.iter().take_while(|byte| byte.is_ascii_digit()).count();
    &name[digits..] == want.as_bytes()
}

/// Where the app keeps the sample's audio, when it holds it in memory.
///
/// Layout verified on 1.3.0.12: the sample's holder (RTTI `12MemorySample`) points
/// back at the metadata block `{u16 channels, u32 frames, u32 rate, u32 bits}`
/// and owns the interleaved float32 buffer.  A streamed sample has none, and the
/// caller falls back to the WAV.
#[must_use]
pub fn memory(sample: usize) -> Option<Memory> {
    let metadata = image::read_pointer(sample.checked_add(SAMPLE_METADATA)?)?;
    let holder = image::read_pointer(sample.checked_add(SAMPLE_HOLDER)?)?;
    if !class_is(holder, "MemorySample") {
        return None;
    }
    if image::read_pointer(holder.checked_add(HOLDER_BACK)?)? != metadata {
        return None;
    }
    let owner = image::read_pointer(holder.checked_add(HOLDER_BUFFER)?)?;
    let buffer = image::read_pointer(owner)?;
    let mut header = [0_u8; 16];
    if !image::read(metadata, &mut header) {
        return None;
    }
    let word = |at: usize| {
        i32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    let (channels, frames, rate, bits) = (word(0), word(4), word(8), word(12));
    if !(1..=2).contains(&channels) || frames <= 0 || bits != 32 {
        return None;
    }
    let rate = u32::try_from(rate)
        .ok()
        .filter(|rate| RATE_RANGE.contains(rate))?;
    Some(Memory {
        buffer,
        frames: u32::try_from(frames).ok()?,
        channels: u16::try_from(channels).ok()?,
        rate,
    })
}

impl Memory {
    /// Copy the audio out of the app.
    #[must_use]
    pub fn read(&self) -> Option<Vec<f32>> {
        let cells = (self.frames as usize).checked_mul(usize::from(self.channels))?;
        if cells == 0 || cells > MEMORY_AUDIO_MAX_CELLS {
            return None;
        }
        let mut out = vec![0.0_f32; cells];
        let mut at = 0;
        let mut bytes = vec![0_u8; READ_CHUNK];
        while at < cells {
            let count = (cells - at).min(READ_CHUNK / 4);
            let chunk = &mut bytes[..count * 4];
            if !image::read(self.buffer.checked_add(at * 4)?, chunk) {
                return None;
            }
            for (slot, raw) in out[at..at + count].iter_mut().zip(chunk.chunks_exact(4)) {
                *slot = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
            }
            at += count;
        }
        Some(out)
    }
}

/// The note the sample plays at its original speed; C3 when it cannot be read.
///
/// A chromatic note is measured from this, not from C3: on 1.3.0.12 pad A01's
/// kick has root 88 and most others 60.
#[must_use]
pub fn root_note(sample: usize) -> i32 {
    sample
        .checked_add(SAMPLE_METADATA)
        .and_then(image::read_pointer)
        .and_then(|metadata| metadata.checked_add(METADATA_ROOT))
        .and_then(image::read_u32)
        .map(u32::cast_signed)
        .filter(|root| (0..=127).contains(root))
        .unwrap_or(DEFAULT_ROOT)
}

/// A sample's frames, channels and rate, when its metadata is usable.
#[must_use]
pub fn geometry(sample: usize) -> Option<(u32, u16, u32)> {
    let frames = frames(sample);
    if frames == 0 {
        return None;
    }
    let metadata = image::read_pointer(sample.checked_add(SAMPLE_METADATA)?)?;
    let channels = u16::try_from(image::read_u32(metadata)? & 0xffff).ok()?;
    let rate = image::read_u32(metadata.checked_add(8)?)?;
    Some((frames, channels, rate))
}

// The app's Temporary Samples service and one native seed for Extract.
const BUILD_SAMPLE: usize = 0x01df_4ac0;
const RELEASE_SHARED: usize = 0x0123_bca0;
/// One and a half seconds at 44.1 kHz: the longest synth take must fit inside
/// the seed, because Extract's range may not exceed the source sample.
const SEED_FRAMES: u32 = 66_150;
static SERVICE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
// The app can retain references after Extract; keep one seed alive until process exit.
static SEED: std::sync::OnceLock<Seed> = std::sync::OnceLock::new();
static SEED_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn remember_service(service: usize) {
    SERVICE.store(service, core::sync::atomic::Ordering::Release);
}

/// One owning reference to a real, unregistered native sample.
pub struct Seed {
    sample: usize,
    owner: usize,
    holder: usize,
}
impl Seed {
    /// Sample pointer for the temporary layer.
    #[must_use]
    pub const fn sample(&self) -> usize {
        self.sample
    }
    /// Sample holder pointer for the temporary layer.
    #[must_use]
    pub const fn holder(&self) -> usize {
        self.holder
    }
    /// Number of seed frames.
    #[must_use]
    pub const fn frames(&self) -> u32 {
        SEED_FRAMES
    }
}
impl Drop for Seed {
    fn drop(&mut self) {
        if self.owner != 0 {
            // SAFETY: the builder returned one owning shared reference; this releases it exactly once.
            unsafe { image::void_of_pointer(image::base() + RELEASE_SHARED, self.owner) };
        }
    }
}

/// Build a silent stereo sample to supply the stock Extract command.
#[must_use]
pub fn silent_seed() -> Option<&'static Seed> {
    if let Some(seed) = SEED.get() {
        return Some(seed);
    }
    let _guard = SEED_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(seed) = SEED.get() {
        return Some(seed);
    }
    let seed = build_seed()?;
    let _ = SEED.set(seed);
    SEED.get()
}

fn build_seed() -> Option<Seed> {
    let context = SERVICE.load(core::sync::atomic::Ordering::Acquire);
    if context == 0 {
        return None;
    }
    #[cfg(target_arch = "aarch64")]
    {
        let audio = vec![0.0_f32; SEED_FRAMES as usize * 2];
        let start = audio.as_ptr() as usize;
        let end = start + audio.len() * size_of::<f32>();
        let vector = [start, end, end];
        let mut pair = [0_usize; 2];
        let address = image::base() + BUILD_SAMPLE;
        // SAFETY: this is the 1.3.0.12 C++ builder. Its nontrivial return uses x8;
        // the audio/vector live throughout the call and the builder copies them.
        unsafe {
            core::arch::asm!("blr {func}", func = in(reg) address, in("x0") context,
            in("x1") vector.as_ptr(), in("x2") 2_usize, in("x3") 44_100_usize,
            in("x8") pair.as_mut_ptr(), clobber_abi("C"));
        }
        let mut seed = Seed {
            sample: pair[0],
            owner: pair[1],
            holder: 0,
        };
        if seed.sample == 0 || seed.owner == 0 {
            return None;
        }
        seed.holder = image::read_pointer(seed.sample + 0x30)?;
        (seed.holder != 0).then_some(seed)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = BUILD_SAMPLE;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::RATE_RANGE;

    #[test]
    fn a_seed_is_unavailable_without_the_app_service() {
        assert!(super::silent_seed().is_none());
    }

    #[test]
    fn the_rate_range_matches_the_app() {
        assert!(RATE_RANGE.contains(&44_100));
        assert!(RATE_RANGE.contains(&8_000));
        assert!(RATE_RANGE.contains(&192_000));
        assert!(!RATE_RANGE.contains(&7_999));
        assert!(!RATE_RANGE.contains(&192_001));
    }
}
