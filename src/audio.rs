//! Device audio: the ALSA taps, the stream geometry they read, and the two
//! callbacks a mod uses to hear the input and mix into the output.
//!
//! The app resolves these four symbols by name, so the SDK defines them with
//! `#[no_mangle]`.  Everything a mod needs is in [`OutputInfo`] and the input
//! callbacks; the device's own handles never reach the mod.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};

/// The input and output taps carry independent geometry and scratch storage, so
/// they are serialised independently: a blocking output write must not make the
/// input tap skip a block (or the other way around).
static OUT_TAP_ACTIVE: AtomicBool = AtomicBool::new(false);
static IN_TAP_ACTIVE: AtomicBool = AtomicBool::new(false);
static IN_PCM: AtomicUsize = AtomicUsize::new(0);
static OUT_PCM: AtomicUsize = AtomicUsize::new(0);
static IN_RATE: AtomicU32 = AtomicU32::new(44_100);
static OUT_RATE: AtomicU32 = AtomicU32::new(44_100);

/// One direction's tap guard.  Reentry forwards the app's call untouched instead
/// of aliasing the tap's scratch or geometry.
struct TapGuard(&'static AtomicBool);
impl TapGuard {
    fn enter(flag: &'static AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self(flag))
    }
}
impl Drop for TapGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// The PCM format code for signed 32-bit little-endian samples.
const FORMAT_S32_LE: i32 = 10;
/// The largest device buffer the SDK will believe, in frames.
const DEVICE_MAX_FRAMES: u64 = 65536;
/// The largest device channel count the SDK will believe.
const DEVICE_MAX_CHANNELS: usize = 32;

/// What the output tap knows when a mod mixes into a block.
#[derive(Clone, Copy, Debug)]
pub struct OutputInfo {
    /// The app's PCM handle, for a mod that needs its own ALSA query.
    pub pcm: usize,
    /// Interleaved channel count.
    pub channels: usize,
    /// Stream rate in Hz.
    pub rate: u32,
    /// Frames in this block.
    pub block_frames: usize,
    /// The device's own reported delay, in frames.
    pub device_delay: u32,
    /// The output ring's size in frames, zero until read.
    pub buffer_out: u32,
    /// The input ring's size in frames, zero until read.
    pub buffer_in: u32,
    /// The input stream's own backlog, in frames.
    pub input_delay: u32,
    /// The loudest output sample so far, as bits.
    pub output_peak_bits: u32,
}

impl OutputInfo {
    /// The device latency this block implies, and the source byte the app's
    /// latency display expects.
    ///
    /// Uses the two ring sizes when both are known, otherwise the measured
    /// delay, the block and the input backlog.
    #[must_use]
    pub fn latency(&self, backlog: u32, extra: u32) -> (u32, u8) {
        let (measured, source) = if self.buffer_out > 0 && self.buffer_in > 0 {
            (u64::from(self.buffer_out) + u64::from(self.buffer_in), b'B')
        } else {
            (
                u64::from(self.device_delay)
                    + u64::try_from(self.block_frames).unwrap_or(0)
                    + u64::from(self.input_delay)
                    + u64::from(backlog),
                b'E',
            )
        };
        let total =
            u32::try_from(measured.saturating_add(u64::from(extra)).min(16_384)).unwrap_or(16_384);
        (total, source)
    }
}

/// The ALSA entry points, resolved once from the next object in the search order.
type HwMalloc = unsafe extern "C" fn(*mut *mut libc::c_void) -> i32;
type HwCurrent = unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_void) -> i32;
type HwBufsize = unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_ulong) -> i32;
type HwChannels = unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_uint) -> i32;
type HwFormat = unsafe extern "C" fn(*mut libc::c_void, *mut i32) -> i32;
type HwFree = unsafe extern "C" fn(*mut libc::c_void);

struct Alsa {
    malloc: HwMalloc,
    current: HwCurrent,
    bufsize: HwBufsize,
    channels: HwChannels,
    format: HwFormat,
    free: HwFree,
}

/// Resolve one symbol from the object after this one.
fn look_up<T: Copy>(name: &core::ffi::CStr) -> Option<T> {
    // SAFETY: the name is NUL-terminated and the search order is the loader's.
    let address = unsafe { libc::dlsym(libc::RTLD_NEXT, name.as_ptr()) };
    if address.is_null() {
        return None;
    }
    // SAFETY: the caller names the type the symbol actually has.
    Some(unsafe { core::mem::transmute_copy::<*mut libc::c_void, T>(&address) })
}

impl Alsa {
    /// Resolve every entry point, or nothing if one is missing.
    fn load() -> Option<&'static Self> {
        static SLOT: std::sync::OnceLock<Option<Alsa>> = std::sync::OnceLock::new();
        SLOT.get_or_init(|| {
            Some(Self {
                malloc: look_up(c"snd_pcm_hw_params_malloc")?,
                current: look_up(c"snd_pcm_hw_params_current")?,
                bufsize: look_up(c"snd_pcm_hw_params_get_buffer_size")?,
                channels: look_up(c"snd_pcm_hw_params_get_channels")?,
                format: look_up(c"snd_pcm_hw_params_get_format")?,
                free: look_up(c"snd_pcm_hw_params_free")?,
            })
        })
        .as_ref()
    }
}

/// The device stream's geometry: its buffer in frames, its channels and whether
/// it carries 32-bit samples.
///
/// # Safety
///
/// `pcm` must be the app's live PCM handle; the out pointers may be null.
pub unsafe fn stream_geometry(pcm: usize, channels: *mut u32, s32: *mut i32) -> u32 {
    let Some(alsa) = Alsa::load() else {
        return 0;
    };
    if pcm == 0 {
        return 0;
    }
    let mut params: *mut libc::c_void = core::ptr::null_mut();
    // SAFETY: the entry point allocates the parameters.
    if unsafe { (alsa.malloc)(core::ptr::from_mut(&mut params)) } < 0 || params.is_null() {
        return 0;
    }
    let mut frames = 0_u32;
    // SAFETY: the handle is live and `params` is ours.
    if unsafe { (alsa.current)(pcm as *mut libc::c_void, params) } >= 0 {
        let mut size = 0_u64;
        // SAFETY: see above.
        if unsafe { (alsa.bufsize)(params, core::ptr::from_mut(&mut size)) } >= 0
            && size < DEVICE_MAX_FRAMES
        {
            frames = u32::try_from(size).unwrap_or(0);
        }
        if !channels.is_null() {
            let mut count = 0_u32;
            // SAFETY: see above.
            if unsafe { (alsa.channels)(params, core::ptr::from_mut(&mut count)) } >= 0
                && count > 0
                && count as usize <= DEVICE_MAX_CHANNELS
            {
                // SAFETY: the caller passed a writable slot.
                unsafe { *channels = count };
            }
        }
        if !s32.is_null() {
            let mut format = -1;
            // SAFETY: see above.
            let known = unsafe { (alsa.format)(params, core::ptr::from_mut(&mut format)) } >= 0;
            // SAFETY: the caller passed a writable slot.
            unsafe { *s32 = i32::from(known && format == FORMAT_S32_LE) };
        }
    }
    // SAFETY: the parameters came from the malloc above.
    unsafe { (alsa.free)(params) };
    frames
}

/// Frames the output tap keeps: two seconds at the device rate.
pub const OUTTAP_FRAMES: usize = 44_100 * 2;

/// The last output delay the device reported, in frames.
pub static mut G_OUT_DELAY: u32 = 0;
/// The same, smoothed, which is what a take uses.
pub static mut G_OUT_DELAY_AVG: u32 = 0;
/// The loudest sample written since the last reset, as bits.
pub static mut G_OUT_PEAK_BITS: u32 = 0;
/// How many buffers have been written.
pub static mut G_OUT_WRITE_CALLS: u32 = 0;
/// The output tap: the last two seconds, channel zero.
#[unsafe(no_mangle)]
pub static mut g_outtap: [f32; OUTTAP_FRAMES] = [0.0; OUTTAP_FRAMES];
/// Where the tap is writing.
#[unsafe(no_mangle)]
pub static mut g_outtap_w: u32 = 0;

/// Whether the device rings need re-reading.
static BUF_DIRTY: AtomicI32 = AtomicI32::new(0);
/// The output ring's size in frames.
static OUT_BUFFER: AtomicU32 = AtomicU32::new(0);
/// The input ring's size in frames.
static IN_BUFFER: AtomicU32 = AtomicU32::new(0);
/// The input stream's backlog, in frames.
static IN_DELAY: AtomicU32 = AtomicU32::new(0);

/// Ask the audio taps to re-read the device rings.
pub fn mark_buffers_stale() {
    BUF_DIRTY.store(3, Ordering::Release);
}

/// Whether the audio taps still have to re-read the device rings.
#[must_use]
pub fn buffers_stale() -> bool {
    BUF_DIRTY.load(Ordering::Acquire) != 0
}

/// Whether a tap must re-read its stream geometry.
///
/// The cached handle matters: a second stream (or a planar capture after an
/// interleaved one) must not inherit the first stream's format or channels.
fn needs_refresh(buffer: u32, dirty: i32, cached_pcm: usize, pcm: usize, bit: i32) -> bool {
    buffer == 0 || dirty & bit != 0 || cached_pcm != pcm
}

/// The output ring's size in frames, zero until read.
#[must_use]
pub fn output_buffer() -> u32 {
    OUT_BUFFER.load(Ordering::Acquire)
}

/// The input ring's size in frames, zero until read.
#[must_use]
pub fn input_buffer() -> u32 {
    IN_BUFFER.load(Ordering::Acquire)
}

/// The input stream's backlog in frames, zero when unknown.
#[must_use]
pub fn input_delay() -> u32 {
    IN_DELAY.load(Ordering::Acquire)
}

/// The calibration trim, in frames; the device harness reads it by name.
#[unsafe(export_name = "g_lat_extra")]
pub static G_LAT_EXTRA: AtomicI32 = AtomicI32::new(0);

/// The calibration trim, as an unsigned frame count.
#[must_use]
pub fn extra_latency() -> u32 {
    // SAFETY: written by the UI thread and the config load.
    u32::try_from(G_LAT_EXTRA.load(Ordering::Acquire).max(0)).unwrap_or(0)
}

/// Set the calibration trim in frames.
pub fn set_extra_latency(frames: i32) {
    // SAFETY: written by the UI thread and the config load.
    G_LAT_EXTRA.store(frames.max(0), Ordering::Release);
}

/// The output stream's channel count, zero until read.
pub static mut G_OUT_CH: u32 = 0;
/// Whether the output stream carries 32-bit samples.
pub static mut G_OUT_S32: i32 = 0;
/// The input stream's channel count, zero until read.
pub static mut G_IN_CH: u32 = 0;
/// Whether the input stream carries 32-bit samples.
pub static mut G_IN_S32: i32 = 0;
/// The input ring's size in frames, as last read (exported for the device tests).
#[unsafe(no_mangle)]
pub static mut g_buf_in: u32 = 0;
/// The output ring's size in frames, as last read (exported for the device tests).
#[unsafe(no_mangle)]
pub static mut g_buf_out: u32 = 0;

/// The app's own `snd_pcm_writei`.
static REAL_WRITEI: std::sync::OnceLock<
    Option<unsafe extern "C" fn(usize, *const u8, u64) -> i64>,
> = std::sync::OnceLock::new();
/// The app's own `snd_pcm_delay`.
static REAL_DELAY: std::sync::OnceLock<Option<unsafe extern "C" fn(usize, *mut i64) -> i64>> =
    std::sync::OnceLock::new();

// Scratch is protected by OUT_TAP_ACTIVE and never exposed outside one write.
// ALSA accepts a const input buffer; mixing must not mutate caller-owned data.
static mut OUTPUT_SCRATCH: [i32; DEVICE_MAX_FRAMES as usize * DEVICE_MAX_CHANNELS] =
    [0; DEVICE_MAX_FRAMES as usize * DEVICE_MAX_CHANNELS];

/// The output tap: let the mod mix, measure the device's latency, then write.
///
/// # Safety
///
/// `pcm` must be live, and `buffer` must hold `size` complete frames in the
/// stream format, as required by ALSA.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_writei(pcm: usize, buffer: *const i32, size: u64) -> i64 {
    // SAFETY: the static is this module's.
    let real = *REAL_WRITEI.get_or_init(|| look_up(c"snd_pcm_writei"));
    let Some(write) = real else {
        return -1;
    };
    let Some(_guard) = TapGuard::enter(&OUT_TAP_ACTIVE) else {
        // SAFETY: forward the caller's original ALSA arguments unchanged.
        return unsafe { write(pcm, buffer.cast(), size) };
    };
    if needs_refresh(
        output_buffer(),
        BUF_DIRTY.load(Ordering::Acquire),
        OUT_PCM.load(Ordering::Acquire),
        pcm,
        1,
    ) {
        // SAFETY: `pcm` is the live handle.
        unsafe { refresh_output(pcm) };
    }
    let mut buffer = buffer;
    // SAFETY: the output guard protects the stream geometry and scratch buffer.
    let (channels, s32) = unsafe { (G_OUT_CH, G_OUT_S32) };
    if !buffer.is_null() && size != 0 && channels != 0 && s32 != 0 && size < DEVICE_MAX_FRAMES {
        let delay = measure_delay(pcm);
        let block = usize::try_from(size).unwrap_or(0);
        let frames = block * channels as usize;
        // SAFETY: validated geometry bounds frames to the scratch capacity.
        // The ALSA caller supplies readable input; the tap guard excludes reentry.
        let samples = unsafe {
            let scratch = core::ptr::addr_of_mut!(OUTPUT_SCRATCH).cast::<i32>();
            core::ptr::copy_nonoverlapping(buffer, scratch, frames);
            buffer = scratch;
            core::slice::from_raw_parts_mut(scratch, frames)
        };
        let info = OutputInfo {
            pcm,
            channels: channels as usize,
            rate: OUT_RATE.load(Ordering::Acquire),
            block_frames: block,
            device_delay: delay,
            buffer_out: output_buffer(),
            buffer_in: input_buffer(),
            input_delay: input_delay(),
            // SAFETY: the audio thread updates the peak after each write.
            output_peak_bits: unsafe { G_OUT_PEAK_BITS },
        };
        crate::loader::with_mod_audio(|module| module.on_output(samples, &info));
    }
    // SAFETY: the app's own writer takes these.
    let written = unsafe { write(pcm, buffer.cast::<u8>(), size) };
    if recordable(size, written, channels, s32) && !buffer.is_null() {
        // SAFETY: the format and geometry were just validated and the buffer
        // holds the S32 frames the app's writer reported.
        unsafe { record_output(buffer, written, channels) };
    }
    written
}

/// Whether a completed write may be recorded as S32.
///
/// An S16 (or otherwise formatted) stream must be forwarded and ignored: reading
/// its block as `i32` would run past the delivered frame's end.
fn recordable(size: u64, written: i64, channels: u32, s32: i32) -> bool {
    let Ok(frames) = u64::try_from(written) else {
        return false;
    };
    s32 != 0
        && frames > 0
        && frames <= size
        && channels > 0
        && channels as usize <= DEVICE_MAX_CHANNELS
}

/// Read the output ring's geometry.
///
/// # Safety
///
/// `pcm` is the app's live handle.
unsafe fn refresh_output(pcm: usize) {
    let mut channels = 0_u32;
    let mut s32 = 0_i32;
    // SAFETY: the geometry reads the stream's own parameters.
    let frames = unsafe {
        stream_geometry(
            pcm,
            core::ptr::from_mut(&mut channels),
            core::ptr::from_mut(&mut s32),
        )
    };
    OUT_BUFFER.store(frames, Ordering::Release);
    // SAFETY: this module's statics.
    unsafe {
        g_buf_out = frames;
        G_OUT_CH = channels;
        G_OUT_S32 = s32;
    }
    OUT_RATE.store(stream_rate(pcm), Ordering::Release);
    OUT_PCM.store(pcm, Ordering::Release);
    if frames != 0 {
        BUF_DIRTY.fetch_and(!1, Ordering::AcqRel);
    }
}

/// Measure the device's reported delay.
fn measure_delay(pcm: usize) -> u32 {
    // SAFETY: this module's statics.
    unsafe {
        let real = *REAL_DELAY.get_or_init(|| look_up(c"snd_pcm_delay"));
        let Some(delay_of) = real else {
            return G_OUT_DELAY_AVG;
        };
        let mut delay = 0_i64;
        // SAFETY: the app's own query writes our local.
        if delay_of(pcm, core::ptr::from_mut(&mut delay)) != 0 || delay < 0 {
            return G_OUT_DELAY_AVG;
        }
        G_OUT_DELAY = u32::try_from(delay).unwrap_or(0);
        let average = G_OUT_DELAY_AVG;
        G_OUT_DELAY_AVG = if average == 0 {
            G_OUT_DELAY
        } else {
            (average * 7 + G_OUT_DELAY) / 8
        };
        G_OUT_DELAY_AVG
    }
}

/// The stream's rate, cached once.
fn stream_rate(pcm: usize) -> u32 {
    {
        type Malloc = unsafe extern "C" fn(*mut *mut libc::c_void) -> i32;
        type Current = unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_void) -> i32;
        type Rate = unsafe extern "C" fn(*mut libc::c_void, *mut u32, *mut i32) -> i32;
        type Free = unsafe extern "C" fn(*mut libc::c_void);
        let (Some(malloc), Some(current), Some(rate), Some(free)) = (
            look_up::<Malloc>(c"snd_pcm_hw_params_malloc"),
            look_up::<Current>(c"snd_pcm_hw_params_current"),
            look_up::<Rate>(c"snd_pcm_hw_params_get_rate"),
            look_up::<Free>(c"snd_pcm_hw_params_free"),
        ) else {
            return 44_100;
        };
        let mut params: *mut libc::c_void = core::ptr::null_mut();
        // SAFETY: the entry point allocates the parameters.
        if unsafe { malloc(core::ptr::from_mut(&mut params)) } < 0 || params.is_null() {
            return 44_100;
        }
        let mut hertz = 0_u32;
        let mut direction = 0_i32;
        // SAFETY: the handle is live and `params` is ours.
        let known = unsafe {
            current(pcm as *mut libc::c_void, params) >= 0
                && rate(
                    params,
                    core::ptr::from_mut(&mut hertz),
                    core::ptr::from_mut(&mut direction),
                ) >= 0
        };
        // SAFETY: the parameters came from the malloc above.
        unsafe { free(params) };
        if known && (8_000..=192_000).contains(&hertz) {
            hertz
        } else {
            44_100
        }
    }
}

/// Record what was written: the peak and the tap.
///
/// # Safety
///
/// `buffer` holds `written * channels` samples of 32-bit audio.
unsafe fn record_output(buffer: *const i32, written: i64, channels: u32) {
    const SCALE: f32 = 1.0 / 2_147_483_648.0;
    // SAFETY: this module's statics and the device's buffer.
    unsafe {
        G_OUT_WRITE_CALLS = G_OUT_WRITE_CALLS.wrapping_add(1);
        let count = usize::try_from(written).unwrap_or(0) * channels as usize;
        let mut peak = 0.0_f32;
        for index in 0..count {
            let value = (*buffer.add(index) as f32) * SCALE;
            peak = peak.max(value.abs());
        }
        if peak > f32::from_bits(G_OUT_PEAK_BITS) {
            G_OUT_PEAK_BITS = peak.to_bits();
        }
        let mut write = g_outtap_w;
        let frames = usize::try_from(written).unwrap_or(0);
        let tap = core::ptr::addr_of_mut!(g_outtap).cast::<f32>();
        for index in 0..frames {
            let slot = write as usize % OUTTAP_FRAMES;
            *tap.add(slot) = (*buffer.add(index * channels as usize) as f32) * SCALE;
            write = write.wrapping_add(1);
        }
        g_outtap_w = write;
    }
}

/// Whether a completed read may be decoded as S32 input.
///
/// The format, channel count and delivered frame count all have to describe the
/// buffer the app's reader just filled.  An S16 stream is ignored rather than
/// interpreted as `i32`.
fn input_usable(frames: usize, size: u64, channels: u32, s32: i32) -> bool {
    s32 != 0
        && frames > 0
        && channels > 0
        && channels as usize <= DEVICE_MAX_CHANNELS
        && u64::try_from(frames).is_ok_and(|read| read <= size)
}

/// How many times the app asked for input audio.
pub static mut G_AUDIO_IN_CALLS: u32 = 0;

/// The app's own `snd_pcm_readi`.
static REAL_READI: std::sync::OnceLock<Option<unsafe extern "C" fn(usize, *mut u8, u64) -> i64>> =
    std::sync::OnceLock::new();
/// The app's own `snd_pcm_avail`.
static REAL_AVAIL: std::sync::OnceLock<Option<unsafe extern "C" fn(usize) -> i64>> =
    std::sync::OnceLock::new();

/// The input tap: let the mod hear the interleaved block the app just read.
///
/// # Safety
///
/// `pcm` must be live, and `buffer` must be writable for `size` complete frames
/// in the stream format, as required by ALSA.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_readi(pcm: usize, buffer: *mut i32, size: u64) -> i64 {
    // SAFETY: the static is this module's.
    let real = *REAL_READI.get_or_init(|| look_up(c"snd_pcm_readi"));
    let Some(capture) = real else {
        return -1;
    };
    let Some(_guard) = TapGuard::enter(&IN_TAP_ACTIVE) else {
        // SAFETY: forward the caller's original ALSA arguments unchanged.
        return unsafe { capture(pcm, buffer.cast(), size) };
    };
    // SAFETY: this module's statics are protected by the guard.
    unsafe { measure_input_delay(pcm) };
    if needs_refresh(
        input_buffer(),
        BUF_DIRTY.load(Ordering::Acquire),
        IN_PCM.load(Ordering::Acquire),
        pcm,
        2,
    ) {
        // SAFETY: `pcm` is the live handle.
        unsafe { refresh_input(pcm) };
    }
    // SAFETY: the app's own reader takes these.
    let got = unsafe { capture(pcm, buffer.cast::<u8>(), size) };
    if got > 0 {
        // SAFETY: this module's statics.
        unsafe {
            G_AUDIO_IN_CALLS = G_AUDIO_IN_CALLS.wrapping_add(1);
            let frames = usize::try_from(got).unwrap_or(0);
            if !buffer.is_null() && input_usable(frames, size, G_IN_CH, G_IN_S32) {
                let channels = G_IN_CH as usize;
                let samples = core::slice::from_raw_parts(buffer.cast_const(), frames * channels);
                crate::loader::with_mod_audio(|module| {
                    module.on_input_interleaved(samples, channels, IN_RATE.load(Ordering::Acquire));
                });
            }
        }
    }
    got
}

/// How many frames the device has waiting, when a mod needs to know.
///
/// # Safety
///
/// `pcm` is the app's live handle.
unsafe fn measure_input_delay(pcm: usize) {
    // SAFETY: this module's statics.
    unsafe {
        let real = *REAL_AVAIL.get_or_init(|| look_up(c"snd_pcm_avail"));
        if let Some(avail_of) = real {
            let avail = avail_of(pcm);
            if avail >= 0 {
                let delay = u32::try_from(avail).unwrap_or(0);
                IN_DELAY.store(delay, Ordering::Release);
            }
        }
    }
}

/// Read the input ring's geometry.
///
/// # Safety
///
/// `pcm` is the app's live handle.
unsafe fn refresh_input(pcm: usize) {
    let mut channels = 0_u32;
    let mut s32 = 0_i32;
    // SAFETY: the geometry reads the stream's own parameters.
    let frames = unsafe {
        stream_geometry(
            pcm,
            core::ptr::from_mut(&mut channels),
            core::ptr::from_mut(&mut s32),
        )
    };
    IN_BUFFER.store(frames, Ordering::Release);
    // SAFETY: this module's statics.
    unsafe {
        g_buf_in = frames;
        G_IN_CH = channels;
        G_IN_S32 = s32;
    }
    IN_RATE.store(stream_rate(pcm), Ordering::Release);
    IN_PCM.store(pcm, Ordering::Release);
    if frames != 0 {
        BUF_DIRTY.fetch_and(!2, Ordering::AcqRel);
    }
}

/// The app's own `snd_pcm_readn`.
static REAL_READN: std::sync::OnceLock<
    Option<unsafe extern "C" fn(usize, *mut *mut i32, u64) -> i64>,
> = std::sync::OnceLock::new();
/// `snd_pcm_hw_params_set_channels(pcm, params, channels)`, as ALSA declares it.
type SetChannels = unsafe extern "C" fn(usize, usize, u32) -> i32;

/// The app's own `snd_pcm_hw_params_set_channels`.
static REAL_SET_CHANNELS: std::sync::OnceLock<Option<SetChannels>> = std::sync::OnceLock::new();

// The exported symbol must keep ALSA's declaration: `(pcm, params, channels)`.
const _: SetChannels = snd_pcm_hw_params_set_channels;

/// Forward one channel request to the real ALSA setter, preserving every
/// argument's position.
///
/// # Safety
///
/// `pcm` and `params` must be the live objects the caller received, and `set`
/// must be the real `snd_pcm_hw_params_set_channels` entry point.
unsafe fn forward_set_channels(set: SetChannels, pcm: usize, params: usize, channels: u32) -> i32 {
    // SAFETY: the caller guarantees both the entry point and its objects.
    unsafe { set(pcm, params, channels) }
}

/// Apply a channel request through the resolved setter, if any.
///
/// # Safety
///
/// `pcm` and `params` must be live ALSA objects.
unsafe fn set_channels_with(
    real: Option<SetChannels>,
    pcm: usize,
    params: usize,
    channels: u32,
) -> i32 {
    match real {
        Some(set) => {
            // SAFETY: the entry point and objects come from the caller's ALSA call.
            let result = unsafe { forward_set_channels(set, pcm, params, channels) };
            if result >= 0 {
                mark_buffers_stale();
            }
            result
        }
        None => -1,
    }
}

/// The planar input tap.
///
/// # Safety
///
/// `pcm` must be live, and `buffers` must contain one writable buffer per
/// channel, each holding `size` samples in the stream format.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_readn(pcm: usize, buffers: *mut *mut i32, size: u64) -> i64 {
    // SAFETY: the static is this module's.
    let real = *REAL_READN.get_or_init(|| look_up(c"snd_pcm_readn"));
    let Some(capture) = real else {
        return -1;
    };
    let Some(_guard) = TapGuard::enter(&IN_TAP_ACTIVE) else {
        // SAFETY: forward the caller's original ALSA arguments unchanged.
        return unsafe { capture(pcm, buffers, size) };
    };
    // SAFETY: pcm is the caller's live stream.
    unsafe { measure_input_delay(pcm) };
    if needs_refresh(
        input_buffer(),
        BUF_DIRTY.load(Ordering::Acquire),
        IN_PCM.load(Ordering::Acquire),
        pcm,
        2,
    ) {
        // SAFETY: pcm is the caller's live stream.
        unsafe { refresh_input(pcm) };
    }
    // SAFETY: the app's own reader takes these.
    let got = unsafe { capture(pcm, buffers, size) };
    if got <= 0 || buffers.is_null() {
        return got;
    }
    // SAFETY: this module's statics.
    let (channels, s32) = unsafe { (G_IN_CH, G_IN_S32) };
    if s32 == 0 || channels == 0 {
        return got;
    }
    let frames = usize::try_from(got).unwrap_or(0);
    if !input_usable(frames, size, channels, s32) {
        return got;
    }
    let count = (channels as usize).min(DEVICE_MAX_CHANNELS);
    let mut planes: [&[i32]; DEVICE_MAX_CHANNELS] = [&[]; DEVICE_MAX_CHANNELS];
    let mut live = 0;
    for index in 0..count {
        // SAFETY: the caller provides `count` channel buffers of `frames`.
        let pointer = unsafe { *buffers.add(index) };
        if pointer.is_null() {
            break;
        }
        // SAFETY: see above.
        planes[live] = unsafe { core::slice::from_raw_parts(pointer.cast_const(), frames) };
        live += 1;
    }
    if live != 0 {
        crate::loader::with_mod_audio(|module| {
            module.on_input_planar(&planes[..live], IN_RATE.load(Ordering::Acquire));
        });
    }
    got
}

/// Pass the app's channel request on.
///
/// # Safety
///
/// `pcm` and `params` must be live ALSA objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_hw_params_set_channels(
    pcm: usize,
    params: usize,
    channels: u32,
) -> i32 {
    // SAFETY: the static is this module's.
    let real: Option<SetChannels> =
        *REAL_SET_CHANNELS.get_or_init(|| look_up(c"snd_pcm_hw_params_set_channels"));
    // SAFETY: the caller passes the live objects ALSA handed it.
    unsafe { set_channels_with(real, pcm, params, channels) }
}

/// Force the four ALSA symbols into the final shared object.
///
/// The app resolves them by name, and link-time garbage collection would
/// otherwise drop functions nothing in Rust calls.  The loader calls this once.
pub fn keep_exports() {
    let kept = [
        snd_pcm_writei as *const () as usize,
        snd_pcm_readi as *const () as usize,
        snd_pcm_readn as *const () as usize,
        snd_pcm_hw_params_set_channels as *const () as usize,
        core::ptr::addr_of!(g_outtap) as usize,
        core::ptr::addr_of!(g_outtap_w) as usize,
        core::ptr::addr_of!(g_buf_in) as usize,
        core::ptr::addr_of!(g_buf_out) as usize,
    ];
    core::hint::black_box(kept);
}

/// Add f32 planes into an interleaved S32 block with saturation.
///
/// `dst` is one block; `planes` are planar f32 buffers of at least the block's
/// frame count.  Returns `false` when the geometry is unusable.
pub fn add_planes_i32(dst: &mut [i32], channels: usize, planes: &[&[f32]]) -> bool {
    if dst.is_empty() || channels == 0 || !dst.len().is_multiple_of(channels) || planes.is_empty() {
        return false;
    }
    let frames = dst.len() / channels;
    if frames == 0 || planes.iter().any(|plane| plane.len() < frames) {
        return false;
    }
    for frame in 0..frames {
        for channel in 0..planes.len().min(channels) {
            let value = f64::from(planes[channel][frame].clamp(-1.0, 1.0)) * 2_147_483_647.0;
            let added = value as i64;
            let slot = &mut dst[frame * channels + channel];
            let sum = i64::from(*slot).saturating_add(added);
            *slot = sum.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{
        OutputInfo, TapGuard, buffers_stale, extra_latency, input_usable, mark_buffers_stale,
        needs_refresh, output_buffer, recordable, set_channels_with, set_extra_latency,
    };

    /// Records the arguments a mock ALSA setter received.
    static SEEN: Mutex<Option<(usize, usize, u32)>> = Mutex::new(None);

    unsafe extern "C" fn mock_set_channels(pcm: usize, params: usize, channels: u32) -> i32 {
        *SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((pcm, params, channels));
        0
    }

    #[test]
    fn set_channels_forwards_every_alsa_argument_in_declaration_order() {
        *SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        // SAFETY: the mock has ALSA's declared `(pcm, params, channels)` ABI.
        let result = unsafe { set_channels_with(Some(mock_set_channels), 0x11, 0x22, 6) };
        assert_eq!(result, 0);
        assert_eq!(
            *SEEN
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            Some((0x11, 0x22, 6)),
            "pcm and params must not be shifted into the channel slot"
        );
        // SAFETY: the mock has ALSA's declared ABI.
        assert_eq!(unsafe { set_channels_with(None, 1, 2, 3) }, -1);
    }

    #[test]
    fn recording_is_gated_on_s32_format_and_geometry() {
        assert!(recordable(512, 512, 2, 1));
        assert!(!recordable(512, 512, 2, 0), "S16 must not be read as S32");
        assert!(!recordable(512, 513, 2, 1), "wrote past the block");
        assert!(!recordable(512, -1, 2, 1), "error return");
        assert!(!recordable(512, 0, 2, 1), "no frames");
        assert!(!recordable(512, 512, 0, 1), "unknown channels");
        assert!(!recordable(512, 512, 33, 1), "implausible channels");
    }

    #[test]
    fn geometry_is_refreshed_for_a_new_stream_or_a_stale_tap() {
        // A first callback, a marked-stale ring, and a second PCM handle all
        // force a refresh; the same live handle with a clean bit does not.
        assert!(needs_refresh(0, 0, 0, 5, 2), "no geometry yet");
        assert!(needs_refresh(1024, 2, 5, 5, 2), "marked stale");
        assert!(needs_refresh(1024, 0, 4, 5, 2), "different stream");
        assert!(!needs_refresh(1024, 0, 5, 5, 2), "same live stream");
    }

    #[test]
    fn input_decoding_is_gated_on_format_and_delivered_frames() {
        assert!(input_usable(512, 512, 2, 1));
        assert!(!input_usable(512, 512, 2, 0), "S16 must not be read as S32");
        assert!(!input_usable(513, 512, 2, 1), "more frames than requested");
        assert!(!input_usable(0, 512, 2, 1), "no frames");
        assert!(!input_usable(512, 512, 0, 1), "unknown channels");
        assert!(!input_usable(512, 512, 33, 1), "implausible channels");
    }

    #[test]
    fn the_input_and_output_taps_do_not_block_each_other() {
        static OUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        static IN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let out = TapGuard::enter(&OUT).expect("output tap");
        let input = TapGuard::enter(&IN).expect("input tap while output is active");
        drop((out, input));
        assert!(TapGuard::enter(&OUT).is_some());
    }

    #[test]
    fn the_calibration_trim_round_trips_and_clamps() {
        set_extra_latency(-7);
        assert_eq!(extra_latency(), 0);
        set_extra_latency(123);
        assert_eq!(extra_latency(), 123);
        set_extra_latency(0);
    }

    #[test]
    fn a_tap_guard_refuses_reentry_until_dropped() {
        static FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let first = TapGuard::enter(&FLAG).expect("first entry");
        assert!(TapGuard::enter(&FLAG).is_none(), "reentry must be refused");
        drop(first);
        assert!(TapGuard::enter(&FLAG).is_some(), "the guard must release");
    }

    #[test]
    fn the_buffer_flag_is_readable_and_settable() {
        mark_buffers_stale();
        assert!(buffers_stale());
        let _ = output_buffer();
    }

    #[test]
    fn an_output_info_is_copy_and_names_its_fields() {
        let info = OutputInfo {
            pcm: 1,
            channels: 2,
            rate: 44_100,
            block_frames: 512,
            device_delay: 0,
            buffer_out: 0,
            buffer_in: 0,
            input_delay: 0,
            output_peak_bits: 0,
        };
        let copy = info;
        assert_eq!(copy.channels, 2);
    }

    #[test]
    fn plane_addition_is_interleaved_and_saturating() {
        let left = [0.5_f32, -0.5];
        let right = [0.0_f32, 1.0];
        let mut dst = [0_i32; 4];
        assert!(super::add_planes_i32(&mut dst, 2, &[&left, &right]));
        assert_eq!(dst[0], 1_073_741_823);
        assert_eq!(dst[1], 0);
        assert_eq!(dst[2], -1_073_741_823);
        assert_eq!(dst[3], 2_147_483_647);
        assert!(!super::add_planes_i32(&mut dst, 2, &[&left[..1]]));
    }

    #[test]
    fn latency_prefers_the_ring_sizes_and_falls_back_to_the_measured_delay() {
        let mut info = OutputInfo {
            pcm: 0,
            channels: 2,
            rate: 44_100,
            block_frames: 64,
            device_delay: 100,
            buffer_out: 1024,
            buffer_in: 512,
            input_delay: 32,
            output_peak_bits: 0,
        };
        assert_eq!(info.latency(8, 4), (1_540, b'B'));
        info.buffer_out = 0;
        assert_eq!(info.latency(8, 1), (205, b'E'));
    }
}
