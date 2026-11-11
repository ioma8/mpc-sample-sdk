//! The firmware's own effects, applied to a whole sample offline.
//!
//! The sampler ships a reverb, a vinyl simulator and a decimator, each a C++
//! object created through a factory and driven through a vtable.  Rather than
//! reimplement them, the mod creates an instance, feeds it the sample in blocks
//! and reads the result back — the same code path the app uses, so the result is
//! the firmware's own sound.
//!
//! Everything here is a call into the loaded image, so the module is only ever
//! used after the image has been recognised and checked against the firmware
//! site table in `glue`.

use std::ffi::c_void;
use std::ptr;

use crate::image::{self, Firmware};

/// Frames per processing block: the firmware's own audio block size.
const BLOCK: usize = 512;
/// Reverb tail rendered after the sample, in seconds.
const REVERB_TAIL_SECONDS: usize = 4;
/// Factory of the reverb effect.
const REVERB_FACTORY: usize = 0x03c4_45b0;
/// Factory of the vinyl effect.
const VINYL_FACTORY: usize = 0x03c4_3cf0;
/// Factory of the decimator effect.
const DECIMATOR_FACTORY: usize = 0x03c4_57a0;
/// Offset of the parameter array inside an effect object.
const PARAMETERS_OFFSET: usize = 0x18;
/// How far past the load address a vtable or method can lie and still be the
/// app's own.
const IMAGE_SPAN: usize = 0x1200_0000;
/// Peak the rendered audio is normalised to.
const PEAK: f32 = 0.98;

/// Which firmware effect to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Effect {
    /// Stereo reverb with a natural tail.
    Reverb = 0,
    /// Vinyl coloration.
    Vinyl = 1,
    /// Bit-depth and sample-rate reduction.
    Decimator = 2,
}

impl Effect {
    /// The effect a menu action selected, or `None` for an unknown code.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Reverb),
            1 => Some(Self::Vinyl),
            2 => Some(Self::Decimator),
            _ => None,
        }
    }

    const fn factory_offset(self) -> usize {
        match self {
            Self::Reverb => REVERB_FACTORY,
            Self::Vinyl => VINYL_FACTORY,
            Self::Decimator => DECIMATOR_FACTORY,
        }
    }

    /// How much tail the effect needs after the sample, in seconds.
    const fn tail_seconds(self) -> usize {
        match self {
            Self::Reverb => REVERB_TAIL_SECONDS,
            Self::Vinyl | Self::Decimator => 0,
        }
    }
}

/// The parameters the firmware's `AudioContext` carries.
#[repr(C)]
struct AudioContext {
    sample_rate: f32,
    reserved: u32,
    block_size: i32,
}

type Factory = unsafe extern "C" fn(*mut c_void, *const AudioContext) -> *mut c_void;
type Setup = unsafe extern "C" fn(*mut c_void, i64);
type Process =
    unsafe extern "C" fn(*mut c_void, *const *const f32, *const *mut f32, i64, *mut u64, i64);
type Destroy = unsafe extern "C" fn(*mut c_void);

/// The three vtable entries of the firmware's effect base class.
#[derive(Clone, Copy)]
struct Methods {
    setup: Setup,
    process: Process,
    destroy: Destroy,
}

/// An effect object owned by this module; destroyed when it goes out of scope.
struct Instance {
    object: *mut c_void,
    methods: Methods,
}

impl Instance {
    /// Create and set up one effect.
    fn create(
        firmware: Firmware,
        effect: Effect,
        rate: u32,
        values: &[(usize, f32)],
    ) -> Result<Self, EffectError> {
        let context = AudioContext {
            sample_rate: rate as f32,
            reserved: 0,
            block_size: i32::try_from(BLOCK).unwrap_or(0),
        };
        // SAFETY: the caller only reaches here after the image matched the
        // tested build, so the factory offset is the function with this
        // signature.
        let create: Factory =
            unsafe { core::mem::transmute(firmware.addr(effect.factory_offset())) };
        // SAFETY: a factory call with a null owner and a live context.
        let object = unsafe { create(ptr::null_mut(), ptr::from_ref(&context)) };
        if object.is_null() {
            return Err(EffectError::FirmwareRefused);
        }
        // The factory returned an object: check its class data and slots before
        // calling through them.  A failed check leaks the object but cannot
        // corrupt the host.
        let image = firmware.base()..firmware.base().saturating_add(IMAGE_SPAN);
        let in_image = |address: usize| image.contains(&address);
        let object_address = object as usize;
        let slots = image::read_pointer(object_address)
            .filter(|vtable| in_image(*vtable))
            .and_then(|vtable| {
                Some((
                    image::read_pointer(vtable + 9 * 8)?,
                    image::read_pointer(vtable + 11 * 8)?,
                    image::read_pointer(vtable + 8)?,
                ))
            })
            .filter(|(setup, process, destroy)| {
                in_image(*setup) && in_image(*process) && in_image(*destroy)
            });
        let parameters = image::read_pointer(object_address + PARAMETERS_OFFSET)
            .filter(|parameters| parameters % 4 == 0);
        let readable = parameters.is_some_and(|parameters| {
            let mut probe = [0_u8; 24];
            image::read(parameters, &mut probe)
        });
        let (Some((setup, process, destroy)), true) = (slots, readable) else {
            return Err(EffectError::FirmwareRefused);
        };
        // SAFETY: the three slots are the documented methods of that base class,
        // and each lies inside the app's image.
        let methods = unsafe {
            Methods {
                setup: core::mem::transmute::<usize, Setup>(setup),
                process: core::mem::transmute::<usize, Process>(process),
                destroy: core::mem::transmute::<usize, Destroy>(destroy),
            }
        };
        let instance = Self { object, methods };
        instance.apply_parameters(values);
        // SAFETY: this slot takes the audio context, not a boolean; the base
        // setup forwards it to the sample-rate/block-size initialiser.
        unsafe { (methods.setup)(object, ptr::from_ref(&context) as i64) };
        Ok(instance)
    }

    /// Set the effect's own knobs to the values the mod wants.
    fn apply_parameters(&self, values: &[(usize, f32)]) {
        // SAFETY: `object + 0x18` holds the effect's parameter array pointer.
        // SAFETY: the field is pointer-aligned in the object; the read is done
        // unaligned so the address arithmetic needs no alignment claim.
        let parameters = unsafe {
            ptr::read_unaligned(
                self.object
                    .cast::<u8>()
                    .add(PARAMETERS_OFFSET)
                    .cast::<*mut f32>(),
            )
        };
        if parameters.is_null() {
            return;
        }
        // SAFETY: the parameter array has at least six entries.
        unsafe {
            for &(index, value) in values {
                if index < 6 {
                    *parameters.add(index) = value;
                }
            }
        }
    }

    /// Render `total` frames, reading at most `frames` of them from `input`.
    fn process(&self, input: &[f32], channels: usize, frames: usize, total: usize) -> Vec<f32> {
        let mut rendered = vec![0.0_f32; total * channels];
        let mut input_block = [[0.0_f32; BLOCK]; 2];
        let mut output_block = [[0.0_f32; BLOCK]; 2];
        let mut flags: u64 = 0;
        let mut start = 0;
        while start < total {
            let count = (total - start).min(BLOCK);
            // Four independent blocks, so the channel pointers do not alias.
            let (first, rest) = input_block.split_at_mut(1);
            let (first_out, rest_out) = output_block.split_at_mut(1);
            let (input_left, input_right) = (&mut first[0], &mut rest[0]);
            let (output_left, output_right) = (&mut first_out[0], &mut rest_out[0]);
            input_left.fill(0.0);
            input_right.fill(0.0);
            output_left.fill(0.0);
            output_right.fill(0.0);

            for (index, (left, right)) in input_left
                .iter_mut()
                .zip(input_right.iter_mut())
                .enumerate()
                .take(count)
            {
                let frame = start + index;
                if frame < frames {
                    let mono = input[frame * channels];
                    *left = mono;
                    *right = if channels == 2 {
                        input[frame * channels + 1]
                    } else {
                        mono
                    };
                }
            }

            let input_pointers: [*const f32; 2] = [input_left.as_ptr(), input_right.as_ptr()];
            let mut output_pointers: [*mut f32; 2] =
                [output_left.as_mut_ptr(), output_right.as_mut_ptr()];
            // SAFETY: planar blocks of `BLOCK` frames, `count <= BLOCK`, and the
            // pointers stay valid for the duration of the call.
            unsafe {
                (self.methods.process)(
                    self.object,
                    input_pointers.as_ptr(),
                    output_pointers.as_mut_ptr(),
                    i64::try_from(count).unwrap_or(0),
                    &raw mut flags,
                    0,
                );
            }

            for (index, (&left, &right)) in output_left
                .iter()
                .zip(output_right.iter())
                .enumerate()
                .take(count)
            {
                let frame = start + index;
                if channels == 2 {
                    rendered[frame * 2] = left;
                    rendered[frame * 2 + 1] = right;
                } else {
                    rendered[frame] = 0.5 * (left + right);
                }
            }
            start += count;
        }
        rendered
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the object came from the factory above and is not used again.
        unsafe { (self.methods.destroy)(self.object) };
    }
}

/// Rendered audio, interleaved like the input.
#[derive(Clone, Debug)]
pub struct Rendered {
    /// Interleaved f32 frames.
    pub audio: Vec<f32>,
    /// Usable length in frames; the tail was trimmed to where it went quiet.
    pub frames: usize,
}

/// Why an effect could not be rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectError {
    /// No audio was supplied.
    Empty,
    /// The buffer does not divide evenly into channels.
    RaggedBuffer,
    /// Only mono and stereo are supported.
    UnsupportedChannels,
    /// The sample rate is outside the supported 8 kHz to 192 kHz range.
    UnsupportedRate,
    /// The firmware did not create the effect.
    FirmwareRefused,
    /// The effect produced a NaN or an infinity.
    NonFinite,
}

impl core::fmt::Display for EffectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => f.write_str("no audio to process"),
            Self::RaggedBuffer => f.write_str("buffer length is not a whole number of frames"),
            Self::UnsupportedChannels => f.write_str("only mono and stereo are supported"),
            Self::UnsupportedRate => f.write_str("unsupported sample rate"),
            Self::FirmwareRefused => f.write_str("the firmware did not create the effect"),
            Self::NonFinite => f.write_str("the effect produced a non-finite sample"),
        }
    }
}

impl std::error::Error for EffectError {}

/// Run one firmware effect over an interleaved f32 buffer.
///
/// The output carries the effect's own tail (four seconds for the reverb),
/// normalised to a peak of 0.98 and trimmed to where it went quiet.
///
/// # Errors
///
/// Returns [`EffectError`] when the buffer or rate is unsupported, or when the
/// firmware refuses to create or process the effect.
pub fn render(
    effect: Effect,
    input: &[f32],
    channels: usize,
    rate: u32,
) -> Result<Rendered, EffectError> {
    let firmware = Firmware::find().ok_or(EffectError::FirmwareRefused)?;
    render_at(firmware, effect, input, channels, rate, &[])
}

/// Render one effect selected by the app's effect code.
///
/// Returns `None` for an unknown code, a bad buffer, or an image that cannot host
/// the effect.
#[must_use]
pub fn render_code(
    code: i32,
    input: &[f32],
    channels: u32,
    rate: u32,
    parameters: &[(usize, f32)],
) -> Option<Vec<f32>> {
    let effect = Effect::from_code(code)?;
    let firmware = Firmware::find()?;
    let channels = usize::try_from(channels).ok()?;
    render_at(firmware, effect, input, channels, rate, parameters)
        .ok()
        .map(|rendered| rendered.audio)
}

/// Render through a specific image handle, for advanced callers.
///
/// A [`Firmware`] value is only constructible from [`Firmware::find`] or from an
/// unsafe, verified-base call, so this safe function never calls through an
/// unauthenticated address.
///
/// # Errors
///
/// Returns [`EffectError`] for the same reasons as [`render`].
pub fn render_at(
    firmware: Firmware,
    effect: Effect,
    input: &[f32],
    channels: usize,
    rate: u32,
    parameters: &[(usize, f32)],
) -> Result<Rendered, EffectError> {
    if input.is_empty() {
        return Err(EffectError::Empty);
    }
    if !(1..=2).contains(&channels) {
        return Err(EffectError::UnsupportedChannels);
    }
    if !(8_000..=192_000).contains(&rate) {
        return Err(EffectError::UnsupportedRate);
    }
    if !input.len().is_multiple_of(channels) {
        return Err(EffectError::RaggedBuffer);
    }
    if firmware.base() == 0 {
        return Err(EffectError::FirmwareRefused);
    }

    let frames = input.len() / channels;
    let total = frames + rate as usize * effect.tail_seconds();
    let instance = Instance::create(firmware, effect, rate, parameters)?;
    let mut rendered = instance.process(input, channels, frames, total);
    drop(instance);

    if rendered.iter().any(|value| !value.is_finite()) {
        return Err(EffectError::NonFinite);
    }
    let peak = normalise(&mut rendered);
    let length = trim_tail(&rendered, channels, frames, total, peak, rate);
    rendered.truncate(length * channels);
    Ok(Rendered {
        audio: rendered,
        frames: length,
    })
}

/// Scale the buffer down to [`PEAK`] when it clips, and report the peak.
fn normalise(audio: &mut [f32]) -> f32 {
    let mut peak = audio
        .iter()
        .fold(0.0_f32, |peak, value| peak.max(value.abs()));
    if peak > PEAK {
        let scale = PEAK / peak;
        for value in audio.iter_mut() {
            *value *= scale;
        }
        peak = PEAK;
    }
    peak
}

/// Where the rendered audio really ends: the last audible frame of the tail plus
/// a short margin, or the whole buffer when the tail never goes quiet.
fn trim_tail(
    audio: &[f32],
    channels: usize,
    frames: usize,
    total: usize,
    peak: f32,
    rate: u32,
) -> usize {
    let threshold = (peak * 1e-4).max(1e-5);
    let mut last = frames;
    for frame in frames..total {
        if (0..channels).any(|channel| audio[frame * channels + channel].abs() > threshold) {
            last = frame + 1;
        }
    }
    let margin = rate as usize / 10;
    if last < total && margin < total - last {
        last + margin
    } else {
        total
    }
}

#[cfg(test)]
mod tests {
    use super::{Effect, EffectError, render_at};
    use crate::image::Firmware;

    #[test]
    fn effect_codes_map_to_the_menu_entries() {
        assert_eq!(Effect::from_code(0), Some(Effect::Reverb));
        assert_eq!(Effect::from_code(1), Some(Effect::Vinyl));
        assert_eq!(Effect::from_code(2), Some(Effect::Decimator));
        assert_eq!(Effect::from_code(3), None);
        assert_eq!(Effect::from_code(-1), None);
    }

    #[test]
    fn the_tail_belongs_to_the_reverb_only() {
        assert_eq!(Effect::Reverb.tail_seconds(), 4);
        assert_eq!(Effect::Vinyl.tail_seconds(), 0);
        assert_eq!(Effect::Decimator.tail_seconds(), 0);
    }

    #[test]
    fn invalid_buffers_are_refused_before_any_firmware_call() {
        // SAFETY: zero is deliberately not an image; every call below must be
        // refused before reaching firmware.
        let firmware = unsafe { Firmware::from_base(0) };
        assert_eq!(
            render_at(firmware, Effect::Reverb, &[], 2, 44_100, &[]).unwrap_err(),
            EffectError::Empty
        );
        assert_eq!(
            render_at(firmware, Effect::Reverb, &[0.0; 8], 3, 44_100, &[]).unwrap_err(),
            EffectError::UnsupportedChannels
        );
        assert_eq!(
            render_at(firmware, Effect::Reverb, &[0.0; 8], 2, 100, &[]).unwrap_err(),
            EffectError::UnsupportedRate
        );
        // Ragged: three samples do not divide into two channels.
        assert_eq!(
            render_at(firmware, Effect::Reverb, &[0.0; 3], 2, 44_100, &[]).unwrap_err(),
            EffectError::RaggedBuffer
        );
        // A zero base is not a firmware image, so the call is refused instead of
        // jumping into unmapped memory.
        assert_eq!(
            render_at(firmware, Effect::Reverb, &[0.0; 8], 2, 44_100, &[]).unwrap_err(),
            EffectError::FirmwareRefused
        );
    }
}
