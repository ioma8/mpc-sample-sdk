//! Finding the audible bounds of a recording.
//!
//! The trim page trims a recording to the first and last frame above a
//! threshold.  The threshold is relative to the file's own peak - so quiet
//! material is trimmed as well - but never below ~-51 dBFS, so a nearly silent
//! file is not trimmed to nothing.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::platform::wav::{self, Encoding};

/// Bytes read per block; one read per frame froze the UI thread on long samples.
const BLOCK_BYTES: usize = 16_384;
/// Threshold used when the caller does not override it: 3 % of the peak.
const DEFAULT_DB: f64 = -30.457_574_905_606_75;
/// Floor of the relative threshold (~-51 dBFS).
const MIN_THRESHOLD: f64 = 90.0;
/// Ceiling of the relative threshold.
const MAX_THRESHOLD: f64 = 30_000.0;

/// Where the sample data lives and how to read it.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// File offset of the first sample.
    pub data_offset: u64,
    /// Interleaved channels.
    pub channels: u16,
    /// Bytes per sample of one channel.
    pub width: u16,
    /// Integer or float samples.
    pub encoding: Encoding,
}

impl Layout {
    /// Bytes of one interleaved frame.
    fn frame_bytes(&self) -> usize {
        usize::from(self.width) * usize::from(self.channels)
    }
}

/// The first and last audible frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    /// First frame above the threshold.
    pub first: u32,
    /// Last frame above the threshold.
    pub last: u32,
}

/// Find the audible bounds of `path`, scanning `frames` interleaved frames.
///
/// `ratio` overrides the relative threshold (0.0 means "use the default, or
/// `MINIMOD_TRIM_DB` when it is set").
#[must_use]
pub fn edges(path: &Path, layout: Layout, frames: u32, ratio: f64) -> Option<Edges> {
    if frames == 0
        || !(1..=2).contains(&layout.channels)
        || !matches!(
            (layout.encoding, layout.width),
            (Encoding::Pcm, 2..=4) | (Encoding::Float, 4 | 8)
        )
    {
        return None;
    }
    let frame_bytes = layout.frame_bytes();
    let file = File::open(path).ok()?;
    let end = layout
        .data_offset
        .checked_add(u64::from(frames) * frame_bytes as u64)?;
    if end > file.metadata().ok()?.len() {
        return None;
    }
    let threshold = threshold(&file, &layout, frames, frame_bytes, ratio)?;
    let first = first_loud(&file, &layout, frames, frame_bytes, threshold)?;
    let last = last_loud(&file, &layout, frames, frame_bytes, threshold, first)?;
    Some(Edges { first, last })
}

/// The amplitude a frame must exceed to count as audible.
fn threshold(
    file: &File,
    layout: &Layout,
    frames: u32,
    frame_bytes: usize,
    ratio: f64,
) -> Option<i32> {
    Some(threshold_for(
        peak(file, layout, frames, frame_bytes)?,
        ratio,
    ))
}

/// The threshold for a file whose loudest sample is `peak`, in the trim scale.
fn threshold_for(peak: i32, ratio: f64) -> i32 {
    let ratio = if ratio > 0.0 {
        ratio
    } else {
        let db = std::env::var("MINIMOD_TRIM_DB")
            .ok()
            .and_then(|text| text.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(DEFAULT_DB)
            .clamp(-90.0, -6.0);
        10.0_f64.powf(db / 20.0)
    };
    if peak == 0 {
        return 328;
    }
    let scaled = (f64::from(peak) * ratio).clamp(MIN_THRESHOLD, MAX_THRESHOLD);
    scaled as i32
}

/// A float sample in the trim scale: the 16-bit integer range.
fn scaled_float(value: f32) -> i32 {
    if !value.is_finite() {
        0
    } else if value >= 1.0 {
        32_767
    } else if value <= -1.0 {
        -32_768
    } else {
        (value * 32_768.0) as i32
    }
}

/// The audible bounds of audio already in memory, `channels` interleaved.
///
/// The same scan as [`edges`] over the app's own float32 buffer; the caller has
/// already chosen `ratio`.  `None` when the audio is silent.
#[must_use]
pub fn memory_edges(samples: &[f32], channels: usize, ratio: f64) -> Option<Edges> {
    if channels == 0 || !samples.len().is_multiple_of(channels) {
        return None;
    }
    let loud_at = |frame: usize, threshold: i32| {
        samples[frame * channels..][..channels]
            .iter()
            .any(|value| scaled_float(*value).abs() > threshold)
    };
    let frames = samples.len() / channels;
    let peak = samples
        .iter()
        .map(|value| scaled_float(*value).abs())
        .max()
        .unwrap_or(0);
    if peak == 0 {
        return None;
    }
    let threshold = threshold_for(peak, ratio);
    let first = (0..frames).find(|frame| loud_at(*frame, threshold))?;
    let last = (first..frames)
        .rev()
        .find(|frame| loud_at(*frame, threshold))
        .unwrap_or(first);
    Some(Edges {
        first: u32::try_from(first).ok()?,
        last: u32::try_from(last).ok()?,
    })
}

/// The loudest sample in the file, in the trim scale.
fn peak(file: &File, layout: &Layout, frames: u32, frame_bytes: usize) -> Option<i32> {
    let mut buffer = [0_u8; BLOCK_BYTES];
    let per_block = BLOCK_BYTES / frame_bytes;
    let mut peak = 0;
    let mut at = 0_u32;
    while at < frames {
        let read = read_frames(
            file,
            layout,
            at,
            per_block.min((frames - at) as usize),
            &mut buffer,
        )?;
        for frame in 0..read {
            for channel in 0..usize::from(layout.channels) {
                let offset =
                    (frame * usize::from(layout.channels) + channel) * usize::from(layout.width);
                let value = wav::scaled_sample(
                    &buffer[offset..offset + usize::from(layout.width)],
                    layout.width,
                    layout.encoding,
                )
                .abs();
                peak = peak.max(value);
            }
        }
        at += u32::try_from(read).unwrap_or(0);
        if read < per_block {
            break;
        }
    }
    Some(peak)
}

/// The first frame that is louder than `threshold`.
fn first_loud(
    file: &File,
    layout: &Layout,
    frames: u32,
    frame_bytes: usize,
    threshold: i32,
) -> Option<u32> {
    let mut buffer = [0_u8; BLOCK_BYTES];
    let per_block = BLOCK_BYTES / frame_bytes;
    let mut at = 0_u32;
    while at < frames {
        let read = read_frames(
            file,
            layout,
            at,
            per_block.min((frames - at) as usize),
            &mut buffer,
        )?;
        for frame in 0..read {
            if loud(layout, &buffer, frame, threshold) {
                return Some(at + u32::try_from(frame).unwrap_or(0));
            }
        }
        at += u32::try_from(read).unwrap_or(0);
        if read < per_block {
            break;
        }
    }
    None
}

/// The last frame that is louder than `threshold`, never before `first`.
fn last_loud(
    file: &File,
    layout: &Layout,
    frames: u32,
    frame_bytes: usize,
    threshold: i32,
    first: u32,
) -> Option<u32> {
    let mut buffer = [0_u8; BLOCK_BYTES];
    let per_block = BLOCK_BYTES / frame_bytes;
    let last = first;
    let mut end = frames;
    while end > 0 {
        let count = per_block.min(usize::try_from(end).unwrap_or(0));
        let start = end - u32::try_from(count).unwrap_or(0);
        let written = file.read_exact_at(
            &mut buffer[..count * frame_bytes],
            layout.data_offset + u64::from(start) * u64::try_from(frame_bytes).unwrap_or(0),
        );
        written.ok()?;
        for frame in (0..count).rev() {
            if loud(layout, &buffer, frame, threshold) {
                return Some(start + u32::try_from(frame).unwrap_or(0));
            }
        }
        end = start;
    }
    Some(last)
}

/// Read up to `frames` interleaved frames at `at`.
fn read_frames(
    file: &File,
    layout: &Layout,
    at: u32,
    frames: usize,
    buffer: &mut [u8],
) -> Option<usize> {
    let frame_bytes = layout.frame_bytes();
    let want = frames * frame_bytes;
    let offset = layout.data_offset + u64::from(at) * u64::try_from(frame_bytes).ok()?;
    file.read_exact_at(&mut buffer[..want], offset).ok()?;
    Some(frames)
}

/// Whether any channel of `frame` is louder than `threshold`.
fn loud(layout: &Layout, buffer: &[u8], frame: usize, threshold: i32) -> bool {
    let width = usize::from(layout.width);
    let channels = usize::from(layout.channels);
    (0..channels).any(|channel| {
        let offset = (frame * channels + channel) * width;
        wav::scaled_sample(
            &buffer[offset..offset + width],
            layout.width,
            layout.encoding,
        )
        .abs()
            > threshold
    })
}

#[cfg(test)]
mod memory_tests {
    use super::{Edges, memory_edges};

    #[test]
    fn memory_audio_trims_like_the_file_scan() {
        // Silence, a burst from 100 to 199, silence.
        let mut audio = vec![0.0_f32; 400];
        for value in &mut audio[100..200] {
            *value = 0.5;
        }
        assert_eq!(
            memory_edges(&audio, 1, 0.03),
            Some(Edges {
                first: 100,
                last: 199
            })
        );
    }

    #[test]
    fn silent_or_empty_memory_has_no_edges() {
        assert_eq!(memory_edges(&[0.0; 64], 1, 0.03), None);
        assert_eq!(memory_edges(&[], 2, 0.03), None);
    }

    #[test]
    fn both_channels_count() {
        // Stereo: only the right channel carries the burst.
        let mut audio = vec![0.0_f32; 400];
        for frame in 50..60 {
            audio[frame * 2 + 1] = 0.8;
        }
        assert_eq!(
            memory_edges(&audio, 2, 0.03),
            Some(Edges {
                first: 50,
                last: 59
            })
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Edges, Layout, edges};
    use crate::platform::wav::Info;

    /// Write a 16-bit mono WAV whose `frames` are `loud` inside `first..last`.
    fn padded(name: &str, frames: u32, first: u32, last: u32) -> (PathBuf, Layout) {
        let mut payload = Vec::new();
        for frame in 0..frames {
            let value: i16 = if frame >= first && frame <= last {
                12_000
            } else {
                0
            };
            payload.extend_from_slice(&value.to_le_bytes());
        }
        let directory = std::env::temp_dir().join(format!("minimod-trim-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("take.wav");
        let mut body = Vec::new();
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&16_u32.to_le_bytes());
        body.extend_from_slice(&1_u16.to_le_bytes());
        body.extend_from_slice(&1_u16.to_le_bytes());
        body.extend_from_slice(&44_100_u32.to_le_bytes());
        body.extend_from_slice(&88_200_u32.to_le_bytes());
        body.extend_from_slice(&2_u16.to_le_bytes());
        body.extend_from_slice(&16_u16.to_le_bytes());
        body.extend_from_slice(b"data");
        body.extend_from_slice(&u32::try_from(payload.len()).unwrap_or(0).to_le_bytes());
        body.extend_from_slice(&payload);
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&u32::try_from(body.len() + 4).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(b"WAVE");
        file.extend_from_slice(&body);
        std::fs::write(&path, &file).expect("write");
        let info = Info::read(&path).expect("info");
        (path, layout_of(&info))
    }

    fn layout_of(info: &Info) -> Layout {
        Layout {
            data_offset: info.data_offset,
            channels: info.channels,
            width: info.width,
            encoding: info.encoding,
        }
    }

    #[test]
    fn ignores_audio_beyond_requested_frames() {
        let (path, layout) = padded("range-limit", 1000, 100, 800);
        assert_eq!(edges(&path, layout, 50, 0.03), None);
        assert_eq!(
            edges(&path, layout, 150, 0.03),
            Some(Edges {
                first: 100,
                last: 149
            })
        );
        assert_eq!(edges(&path, layout, 1001, 0.03), None);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn float64_audio_and_invalid_layouts() {
        let (path, _) = padded("float64", 4, 0, 0);
        let data: Vec<u8> = [0.0_f64, 0.5, -0.5, 0.0]
            .into_iter()
            .flat_map(f64::to_le_bytes)
            .collect();
        std::fs::write(&path, data).unwrap();
        let layout = Layout {
            data_offset: 0,
            channels: 1,
            width: 8,
            encoding: crate::platform::wav::Encoding::Float,
        };
        assert_eq!(
            edges(&path, layout, 4, 0.03),
            Some(Edges { first: 1, last: 2 })
        );
        assert_eq!(
            edges(
                &path,
                Layout {
                    channels: 0,
                    ..layout
                },
                4,
                0.03
            ),
            None
        );
        assert_eq!(edges(&path, Layout { width: 3, ..layout }, 4, 0.03), None);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn finds_the_padded_audio() {
        let (path, layout) = padded("bounds", 1_000, 100, 800);
        assert_eq!(
            edges(&path, layout, 1_000, 0.03),
            Some(Edges {
                first: 100,
                last: 800
            })
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_silent_file_has_no_edges() {
        let (path, layout) = padded("silent", 500, 1, 0);
        assert_eq!(edges(&path, layout, 500, 0.03), None);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_higher_ratio_trims_tighter() {
        let (path, layout) = padded("ratio", 1_000, 200, 700);
        // A ratio above 1 makes the threshold higher than the peak, so nothing
        // is loud enough.
        assert_eq!(edges(&path, layout, 1_000, 2.0), None);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
