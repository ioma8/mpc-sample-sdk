//! The WAV files the firmware and the mod exchange.
//!
//! The sampler records and saves plain RIFF/WAVE files (PCM 16/24/32-bit, and
//! float 32/64 for the rendered effects), sometimes behind a `JUNK` chunk, so
//! the chunk walk cannot assume fixed offsets.  Everything here is pure parsing
//! over a file handle: no firmware addresses, so it is host-testable with
//! synthetic files.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

/// Most chunks the parser walks before giving up on a malformed file.
const MAX_CHUNKS: usize = 64;
/// A `fmt ` chunk larger than this is not a format description we understand.
const MAX_FORMAT_CHUNK: u32 = 40;
/// `WAVE_FORMAT_EXTENSIBLE`, whose real format lives in the sub-format GUID.
const EXTENSIBLE: u16 = 0xfffe;
/// Samples decoded per read, so a long sample never needs a matching buffer of
/// raw bytes.
const READ_BYTES: usize = 8192;

/// How the samples are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// Integer PCM.
    Pcm,
    /// IEEE float.
    Float,
}

/// What a WAV file contains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// Sample frames in the data chunk.
    pub frames: u32,
    /// Sample rate in Hz.
    pub rate: u32,
    /// Interleaved channels.
    pub channels: u16,
    /// Bytes per sample of one channel.
    pub width: u16,
    /// Integer or float samples.
    pub encoding: Encoding,
    /// File offset of the first sample.
    pub data_offset: u64,
    /// Bytes of sample data.
    pub data_bytes: u32,
}

/// The format fields of a `fmt ` chunk.
#[derive(Clone, Copy, Debug)]
struct Format {
    encoding: Encoding,
    channels: u16,
    rate: u32,
    bits: u16,
}

impl Info {
    /// Read the header of `path`.
    #[must_use]
    pub fn read(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let (format, data_offset, data_bytes) = scan(&file)?;
        Self::from_parts(format, data_offset, data_bytes)
    }

    /// Build the description from a parsed header.
    fn from_parts(format: Format, data_offset: u64, data_bytes: u32) -> Option<Self> {
        let width = format.bits / 8;
        let frame_bytes = u32::from(width).checked_mul(u32::from(format.channels))?;
        if frame_bytes == 0 {
            return None;
        }
        Some(Self {
            frames: data_bytes / frame_bytes,
            rate: format.rate,
            channels: format.channels,
            width,
            encoding: format.encoding,
            data_offset,
            data_bytes,
        })
    }

    /// Whether the file holds exactly the material the caller expects.
    #[must_use]
    pub const fn matches(&self, frames: u32, channels: u16, rate: u32) -> bool {
        self.frames == frames && self.channels == channels && self.rate == rate
    }
}

/// Decode a whole WAV file into interleaved f32.
///
/// Returns `None` when the file is not a WAV the mod understands, when its
/// length, channel count or rate differ from the expected ones, or when the data
/// chunk is shorter than its header claims.
#[must_use]
pub fn read_f32(path: &Path, frames: u32, channels: u16, rate: u32) -> Option<Vec<f32>> {
    let file = File::open(path).ok()?;
    let (format, data_offset, data_bytes) = scan(&file)?;
    let info = Info::from_parts(format, data_offset, data_bytes)?;
    let width = usize::from(info.width);
    if width == 0 || !info.matches(frames, channels, rate) {
        return None;
    }
    let cells = usize::try_from(frames)
        .ok()?
        .checked_mul(usize::from(channels))?;
    let mut samples = Vec::new();
    samples.try_reserve_exact(cells).ok()?;
    samples.resize(cells, 0.0_f32);
    let mut buffer = [0_u8; READ_BYTES];
    let mut at = 0;
    let mut offset = data_offset;
    while at < cells {
        let room = (READ_BYTES / width).min(cells - at);
        let want = room * width;
        let read = file.read_exact_at(&mut buffer[..want], offset).is_ok();
        if !read {
            break;
        }
        let got = want / width;
        for index in 0..got {
            let bytes = &buffer[index * width..][..width];
            samples[at + index] = decode(bytes, &info);
        }
        at += got;
        offset += want as u64;
    }
    if at == cells { Some(samples) } else { None }
}

/// Walk the RIFF chunks and return the format description and the data chunk.
fn scan(file: &File) -> Option<(Format, u64, u32)> {
    let mut header = [0_u8; 12];
    file.read_exact_at(&mut header, 0).ok()?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return None;
    }
    let riff_end = u64::from(u32::from_le_bytes(header[4..8].try_into().ok()?)) + 8;
    if riff_end < 12 || riff_end > file.metadata().ok()?.len() {
        return None;
    }
    let mut format: Option<Format> = None;
    let mut data: Option<(u64, u32)> = None;
    let mut offset = 12_u64;
    for _ in 0..MAX_CHUNKS {
        if offset.checked_add(8)? > riff_end {
            break;
        }
        let mut chunk = [0_u8; 8];
        if file.read_exact_at(&mut chunk, offset).is_err() {
            break;
        }
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        if offset.checked_add(8)?.checked_add(u64::from(size))? > riff_end {
            return None;
        }
        if &chunk[0..4] == b"fmt " && (16..=MAX_FORMAT_CHUNK).contains(&size) {
            if let Some(parsed) = read_format(file, offset + 8, size) {
                format = Some(parsed);
            }
        } else if &chunk[0..4] == b"data" {
            data = Some((offset + 8, size));
            if format.is_some() {
                break;
            }
        }
        offset = offset
            .checked_add(8)
            .and_then(|value| value.checked_add(u64::from(size)))
            .and_then(|value| value.checked_add(u64::from(size & 1)))?;
    }
    let format = format?;
    let (data_offset, data_bytes) = data?;
    let width = u32::from(format.bits / 8);
    let frame_bytes = width.checked_mul(u32::from(format.channels))?;
    if data_bytes == 0 || format.channels == 0 || format.rate == 0 || frame_bytes == 0 {
        return None;
    }
    if !(format.channels <= 2 && (8_000..=192_000).contains(&format.rate)) {
        return None;
    }
    if data_bytes % frame_bytes != 0 {
        return None;
    }
    Some((format, data_offset, data_bytes))
}

/// Parse one `fmt ` chunk payload.
fn read_format(file: &File, offset: u64, size: u32) -> Option<Format> {
    let mut payload = [0_u8; MAX_FORMAT_CHUNK as usize];
    let size_usize = usize::try_from(size).ok()?;
    file.read_exact_at(&mut payload[..size_usize], offset)
        .ok()?;
    let mut tag = u16::from_le_bytes([payload[0], payload[1]]);
    let channels = u16::from_le_bytes([payload[2], payload[3]]);
    let rate = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let bits = u16::from_le_bytes([payload[14], payload[15]]);
    // WAVE_FORMAT_EXTENSIBLE: the real format is the first two bytes of the
    // sub-format GUID.
    if tag == EXTENSIBLE && size >= MAX_FORMAT_CHUNK {
        if u16::from_le_bytes([payload[16], payload[17]]) < 22
            || payload[26..40] != [0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]
        {
            return None;
        }
        tag = u16::from_le_bytes([payload[24], payload[25]]);
    }
    if u16::from_le_bytes([payload[12], payload[13]]) != channels.checked_mul(bits / 8)? {
        return None;
    }
    let encoding = match (tag, bits) {
        (1, 16 | 24 | 32) => Encoding::Pcm,
        (3, 32 | 64) => Encoding::Float,
        _ => return None,
    };
    Some(Format {
        encoding,
        channels,
        rate,
        bits,
    })
}

/// One sample of `bytes`, scaled to the 16-bit integer range the trim logic
/// compares against.
///
/// This is the reference's scale, not `-1.0 .. 1.0`: 16-bit samples are used as
/// they are, 24- and 32-bit ones are shifted down, and float samples are scaled
/// by 32768.
#[must_use]
pub fn scaled_sample(bytes: &[u8], width: u16, encoding: Encoding) -> i32 {
    if bytes.len() < usize::from(width) {
        return 0;
    }
    match (encoding, width) {
        (Encoding::Float, 8) => {
            let value = f64::from_le_bytes(bytes[..8].try_into().unwrap_or([0; 8]));
            if !value.is_finite() {
                return 0;
            }
            (value * 32_768.0).clamp(-32_768.0, 32_767.0) as i32
        }
        (Encoding::Float, 4) => {
            let value = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if !value.is_finite() {
                return 0;
            }
            if value >= 1.0 {
                return 32_767;
            }
            if value <= -1.0 {
                return -32_768;
            }
            (value * 32_768.0) as i32
        }
        (Encoding::Pcm, 2) => i32::from(i16::from_le_bytes([bytes[0], bytes[1]])),
        (Encoding::Pcm, 3) => {
            let value = i32::from(bytes[0])
                | (i32::from(bytes[1]) << 8)
                | (i32::from(bytes[2].cast_signed()) << 16);
            value >> 8
        }
        (Encoding::Pcm, 4) => {
            let value = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            value >> 16
        }
        _ => 0,
    }
}

/// One sample of `bytes`, scaled into `-1.0 .. 1.0`.
fn decode(bytes: &[u8], info: &Info) -> f32 {
    match (info.encoding, info.width) {
        (Encoding::Float, 4) => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        (Encoding::Float, 8) => {
            let value = f64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]);
            value as f32
        }
        (Encoding::Pcm, 2) => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_768.0,
        (Encoding::Pcm, 3) => {
            let value = i32::from(bytes[0])
                | (i32::from(bytes[1]) << 8)
                | (i32::from(bytes[2].cast_signed()) << 16);
            value as f32 / 8_388_608.0
        }
        (Encoding::Pcm, 4) => {
            let value = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            (f64::from(value) / 2_147_483_648.0) as f32
        }
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Encoding, Info, read_f32};

    /// Write a WAV with the given chunks; `junk` puts a `JUNK` chunk in front of
    /// `fmt `, the way the sampler's own recordings do.
    fn write_wav(
        name: &str,
        junk: bool,
        tag: u16,
        channels: u16,
        rate: u32,
        bits: u16,
        payload: &[u8],
    ) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("minimod-wav-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("sample.wav");
        let mut body: Vec<u8> = Vec::new();
        if junk {
            body.extend_from_slice(b"JUNK");
            body.extend_from_slice(&8_u32.to_le_bytes());
            body.extend_from_slice(&[0; 8]);
        }
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        let block = u32::from(channels) * u32::from(bits / 8);
        fmt.extend_from_slice(&(rate * block).to_le_bytes());
        fmt.extend_from_slice(&u16::try_from(block).unwrap_or(0).to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&u32::try_from(fmt.len()).unwrap_or(0).to_le_bytes());
        body.extend_from_slice(&fmt);
        if fmt.len() % 2 == 1 {
            body.push(0);
        }
        body.extend_from_slice(b"data");
        body.extend_from_slice(&u32::try_from(payload.len()).unwrap_or(0).to_le_bytes());
        body.extend_from_slice(payload);

        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&u32::try_from(body.len() + 4).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(b"WAVE");
        file.extend_from_slice(&body);
        std::fs::write(&path, &file).expect("write wav");
        path
    }

    #[test]
    fn float64_trim_scale_matches_float32() {
        for value in [-2.0_f64, -0.5, 0.0, 0.5, 2.0, f64::NAN] {
            assert_eq!(
                super::scaled_sample(&value.to_le_bytes(), 8, Encoding::Float),
                super::scaled_sample(&(value as f32).to_le_bytes(), 4, Encoding::Float)
            );
        }
    }

    #[test]
    fn rejects_partial_frames_and_truncated_payloads() {
        let path = write_wav("partial-frame", false, 1, 2, 44100, 16, &[0; 3]);
        assert!(Info::read(&path).is_none());
        let path = write_wav("short-payload", false, 1, 1, 44100, 16, &[0; 4]);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(file.metadata().unwrap().len() - 2).unwrap();
        assert!(Info::read(&path).is_none());
        assert_eq!(super::scaled_sample(&[0], 4, Encoding::Float), 0);
    }

    fn pcm16(values: &[i16]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    #[test]
    fn the_trim_scale_matches_the_reference() {
        use super::{Encoding, scaled_sample};
        assert_eq!(
            scaled_sample(&(-32_768_i16).to_le_bytes(), 2, Encoding::Pcm),
            -32_768
        );
        assert_eq!(
            scaled_sample(&32_767_i16.to_le_bytes(), 2, Encoding::Pcm),
            32_767
        );
        // 24-bit full scale shifts down to 16 bits.
        assert_eq!(scaled_sample(&[0x00, 0x00, 0x40], 3, Encoding::Pcm), 16_384);
        assert_eq!(
            scaled_sample(&[0x00, 0x00, 0xC0], 3, Encoding::Pcm),
            -16_384
        );
        // Float is clamped before scaling, like the reference.
        assert_eq!(
            scaled_sample(&1.0_f32.to_le_bytes(), 4, Encoding::Float),
            32_767
        );
        assert_eq!(
            scaled_sample(&(-1.0_f32).to_le_bytes(), 4, Encoding::Float),
            -32_768
        );
        assert_eq!(
            scaled_sample(&f32::NAN.to_le_bytes(), 4, Encoding::Float),
            0
        );
    }

    #[test]
    fn reads_pcm16_behind_a_junk_chunk() {
        let path = write_wav(
            "pcm16",
            true,
            1,
            2,
            44_100,
            16,
            &pcm16(&[0, -32_768, 16_384, 32_767]),
        );
        let info = Info::read(&path).expect("info");
        assert_eq!(info.frames, 2);
        assert_eq!(info.channels, 2);
        assert_eq!(info.rate, 44_100);
        assert_eq!(info.width, 2);
        assert_eq!(info.encoding, Encoding::Pcm);
        let samples = read_f32(&path, 2, 2, 44_100).expect("decoded");
        assert_eq!(samples.len(), 4);
        assert!((samples[0]).abs() < f32::EPSILON);
        assert!((samples[1] + 1.0).abs() < f32::EPSILON);
        assert!((samples[2] - 0.5).abs() < f32::EPSILON);
        assert!(samples[3] > 0.999);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn reads_pcm24_and_float32() {
        let pcm24 = [0x00_u8, 0x00, 0x40, 0x00, 0x00, 0xC0];
        let path = write_wav("pcm24", false, 1, 1, 48_000, 24, &pcm24);
        let samples = read_f32(&path, 2, 1, 48_000).expect("decoded");
        assert!((samples[0] - 0.5).abs() < 1e-6, "{samples:?}");
        assert!((samples[1] + 0.5).abs() < 1e-6, "{samples:?}");

        let floats: Vec<u8> = [0.25_f32, -0.75]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let path = write_wav("float32", false, 3, 1, 44_100, 32, &floats);
        let samples = read_f32(&path, 2, 1, 44_100).expect("decoded");
        assert!((samples[0] - 0.25).abs() < f32::EPSILON);
        assert!((samples[1] + 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn refuses_mismatched_or_truncated_files() {
        let path = write_wav("mismatch", false, 1, 2, 44_100, 16, &pcm16(&[1, 2, 3, 4]));
        assert!(read_f32(&path, 3, 2, 44_100).is_none(), "wrong frame count");
        assert!(read_f32(&path, 2, 1, 44_100).is_none(), "wrong channels");
        assert!(read_f32(&path, 2, 2, 48_000).is_none(), "wrong rate");
        // A header that promises more data than the file holds.
        let mut bytes = std::fs::read(&path).expect("read");
        bytes.truncate(bytes.len() - 4);
        std::fs::write(&path, &bytes).expect("write");
        assert!(read_f32(&path, 2, 2, 44_100).is_none(), "truncated data");
        // Not a WAV at all.
        std::fs::write(&path, b"nope").expect("write");
        assert!(Info::read(&path).is_none());
        assert!(read_f32(&path, 2, 2, 44_100).is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_missing_file_is_not_an_error_to_panic_on() {
        let missing = Path::new("/nonexistent/minimod/none.wav");
        assert!(Info::read(missing).is_none());
        assert!(read_f32(missing, 1, 1, 44_100).is_none());
    }
}
