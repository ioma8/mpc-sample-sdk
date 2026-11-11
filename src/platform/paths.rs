//! Finding the file behind the sample the app has selected.
//!
//! A sample object does not carry a path directly: the path lives in a
//! `std::string` somewhere in its object graph, and a freshly recorded sample
//! has no path at all until it is saved.  The resolution order is therefore:
//!
//! 1. the holder the sample object points at (`sample + 0x30` → `+0x120`);
//! 2. a scan of the object's pointer graph for a path naming a WAV of exactly
//!    the expected length;
//! 3. only for an explicit recording context ([`for_recording`]), the one file
//!    in that recording's own project directory whose geometry matches.
//!
//! [`for_sample`] never scans other projects: a geometry match alone is not
//! proof that a temporary file is the selected recording.
//!
//! A path is only accepted when its WAV header matches the sample's own
//! geometry, so a stale string in a reused object cannot point at the wrong
//! file.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::image;
use crate::platform::wav;

/// Where the app keeps its loaded copies and recordings.
pub const TEMP_PROJECTS: &str = "/data/TempProjects";
/// Roots a path in the app's heap can start with.
const ROOTS: [&[u8]; 4] = [b"/data/", b"/media/", b"/usr/share/", b"/factory/"];
/// Shortest and longest path the search accepts.
const MIN_PATH: usize = 6;
const MAX_PATH: usize = 500;

/// The path of the WAV that backs `sample`, from the sample's own object graph.
///
/// This never guesses across temporary projects; a freshly recorded sample has
/// no path until it is saved, and callers with explicit recording context use
/// [`for_recording`] instead.
#[must_use]
pub fn for_sample(sample: usize, frames: u32, channels: u16, rate: u32) -> Option<PathBuf> {
    if let Some(path) = from_object(sample, frames)
        && matches(&path, frames, channels, rate)
    {
        return Some(path);
    }
    if let Some(path) = from_object_graph(sample, frames)
        && matches(&path, frames, channels, rate)
    {
        return Some(path);
    }
    None
}

/// The path of a recording that has no stored path yet.
///
/// `project` must be the project directory the recording belongs to; only that
/// directory is searched, and only when exactly one WAV has the requested
/// geometry.  This is the explicit recording context [`for_sample`] refuses to
/// invent.
#[must_use]
pub fn for_recording(
    sample: usize,
    frames: u32,
    channels: u16,
    rate: u32,
    project: &Path,
) -> Option<PathBuf> {
    for_sample(sample, frames, channels, rate).or_else(|| unique(project, frames, channels, rate))
}

/// The path the sample object points at directly.
#[must_use]
pub fn from_object(sample: usize, frames: u32) -> Option<PathBuf> {
    let holder = image::read_pointer(sample.checked_add(0x30)?)?;
    let text = image::read_pointer(holder.checked_add(0x120)?)?;
    read_text(text, frames)
}

/// The path found by walking the object's pointer graph, one and two levels
/// deep, then the raw bytes around the object.
#[must_use]
pub fn from_object_graph(object: usize, frames: u32) -> Option<PathBuf> {
    for offset in (0..0x2000_usize).step_by(8) {
        let Some(pointer) = image::read_pointer(object.wrapping_add(offset)) else {
            continue;
        };
        if let Some(path) = read_text(pointer, frames) {
            return Some(path);
        }
        for inner in (0..0x400_usize).step_by(8) {
            let Some(deeper) = image::read_pointer(pointer.wrapping_add(inner)) else {
                continue;
            };
            if let Some(path) = read_text(deeper, frames) {
                return Some(path);
            }
        }
    }
    // The string may also sit inline in the object rather than behind a
    // pointer, so the raw bytes around it are worth one pass.
    for delta in (-0x2000_isize..=0x2000).step_by(8) {
        let address = object.wrapping_add_signed(delta);
        let mut buffer = [0_u8; 512];
        if !image::read(address, &mut buffer[..511]) {
            continue;
        }
        let printable = buffer[..511]
            .iter()
            .all(|byte| *byte == 0 || (32..=126).contains(byte));
        if printable && let Some(path) = in_text(&buffer, frames) {
            return Some(path);
        }
    }
    None
}

/// The one existing file under `root` with exactly this geometry.
///
/// This finds an existing matching file; it does not create a path or a file.
/// Frame count alone is not a safe identity, so the answer is `None` unless
/// exactly one file matches, and callers should pass the recording's own project
/// directory rather than all temporary projects.
#[must_use]
pub fn unique(root: &Path, frames: u32, channels: u16, rate: u32) -> Option<PathBuf> {
    let mut found: Option<PathBuf> = None;
    let mut matches = 0_usize;
    for path in wav_files(root) {
        let Some(info) = wav::Info::read(&path) else {
            continue;
        };
        if info.frames != frames || info.channels != channels || info.rate != rate {
            continue;
        }
        matches += 1;
        if matches == 1 {
            found = Some(path);
        }
    }
    if matches == 1 { found } else { None }
}

/// The first path in `text` whose WAV has exactly `frames` frames.
#[must_use]
pub fn in_text(text: &[u8], frames: u32) -> Option<PathBuf> {
    in_text_under(text, frames, &ROOTS)
}

/// [`in_text`] with an explicit set of roots, so the rules can be tested
/// against a temporary directory.
#[must_use]
pub fn in_text_under(text: &[u8], frames: u32, roots: &[&[u8]]) -> Option<PathBuf> {
    for root in roots {
        let Some(start) = find(text, root) else {
            continue;
        };
        let end = text[start..]
            .iter()
            .position(|byte| !(32..127).contains(byte))
            .map_or(text.len(), |offset| start + offset);
        let slice = &text[start..end];
        if !(MIN_PATH..=MAX_PATH).contains(&slice.len()) {
            continue;
        }
        if !contains_ignore_case(slice, b".wav") {
            continue;
        }
        let path = PathBuf::from(OsStr::from_bytes(slice));
        if let Some(info) = wav::Info::read(&path)
            && info.frames == frames
        {
            return Some(path);
        }
    }
    None
}

/// Whether the file at `path` holds exactly this material.
fn matches(path: &Path, frames: u32, channels: u16, rate: u32) -> bool {
    wav::Info::read(path).is_some_and(|info| info.matches(frames, channels, rate))
}

/// Read up to 511 bytes at `address` and look for a path in them.
fn read_text(address: usize, frames: u32) -> Option<PathBuf> {
    let mut buffer = [0_u8; 512];
    if !image::read(address, &mut buffer[..511]) {
        return None;
    }
    in_text(&buffer, frames)
}

/// Every `.wav` one level below `root`, in directory order.
fn wav_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(projects) = std::fs::read_dir(root) else {
        return files;
    };
    for project in projects.flatten() {
        if project.file_name().as_bytes().first() == Some(&b'.') {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let bytes = name.as_bytes();
            if bytes.len() < 5 || !bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".wav") {
                continue;
            }
            files.push(entry.path());
        }
    }
    files
}

/// Find `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Case-insensitive ASCII containment.
fn contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    if haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use super::{contains_ignore_case, find, for_recording, for_sample, in_text_under, unique};

    /// Make a project directory holding one WAV with the given geometry.
    fn wav(name: &str, frames: u32, channels: u16) -> PathBuf {
        let root = std::env::temp_dir().join(format!("minimod-paths-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("Temp-Project_[ProjectData]");
        std::fs::create_dir_all(&project).expect("project dir");
        let path = project.join(format!("{name}.wav"));
        let data = vec![0_u8; usize::try_from(frames * u32::from(channels) * 2).unwrap_or(0)];
        let mut body = Vec::new();
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&16_u32.to_le_bytes());
        body.extend_from_slice(&1_u16.to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes());
        body.extend_from_slice(&44_100_u32.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&(channels * 2).to_le_bytes());
        body.extend_from_slice(&16_u16.to_le_bytes());
        body.extend_from_slice(b"data");
        body.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_le_bytes());
        body.extend_from_slice(&data);
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&u32::try_from(body.len() + 4).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(b"WAVE");
        file.extend_from_slice(&body);
        std::fs::write(&path, &file).expect("write wav");
        root
    }

    #[test]
    fn a_path_in_the_heap_must_name_a_wav_of_the_right_length() {
        let root = wav("in-text", 100, 1);
        let path = root.join("Temp-Project_[ProjectData]/in-text.wav");
        let text = format!("\u{1}\u{2}{}\u{0}trailing", path.display());
        let bytes = text.as_bytes();
        let roots: Vec<&[u8]> = vec![root.as_os_str().as_bytes()];
        assert_eq!(in_text_under(bytes, 100, &roots), Some(path.clone()));
        assert_eq!(in_text_under(bytes, 101, &roots), None, "wrong frame count");
        // A path that is not a WAV is refused.
        let other = b"/data/notes.txt\0";
        assert_eq!(in_text_under(other, 0, &[b"/data/"]), None);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unique_needs_exactly_one_match() {
        let root = wav("unique", 64, 2);
        let project = root.join("Temp-Project_[ProjectData]");
        assert_eq!(
            unique(&root, 64, 2, 44_100),
            Some(project.join("unique.wav"))
        );
        assert_eq!(unique(&root, 64, 1, 44_100), None, "wrong channels");
        assert_eq!(unique(&root, 65, 2, 44_100), None, "wrong frames");
        // A second file with the same geometry makes the answer ambiguous.
        std::fs::copy(project.join("unique.wav"), project.join("copy.wav")).expect("copy");
        assert_eq!(unique(&root, 64, 2, 44_100), None, "two matches");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_recording_fallback_is_scoped_to_its_own_project() {
        let root = wav("scoped", 64, 2);
        let project = root.join("Temp-Project_[ProjectData]/scoped.wav");
        // The explicit context finds it; an unrelated project directory does not.
        assert_eq!(
            for_recording(0, 64, 2, 44_100, &root),
            Some(project.clone())
        );
        let other = std::env::temp_dir().join("minimod-paths-unrelated");
        let _ = std::fs::remove_dir_all(&other);
        std::fs::create_dir_all(&other).expect("other dir");
        assert_eq!(for_recording(0, 64, 2, 44_100, &other), None);
        // Without an explicit recording context there is no cross-project scan.
        assert_eq!(for_sample(0, 64, 2, 44_100), None);
        std::fs::remove_dir_all(&other).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_missing_root_is_not_a_crash() {
        let missing = Path::new("/nonexistent/minimod/paths");
        assert_eq!(unique(missing, 1, 1, 44_100), None);
    }

    #[test]
    fn helpers_match_the_reference_rules() {
        assert_eq!(find(b"abcabc", b"bc"), Some(1));
        assert_eq!(find(b"abc", b"zz"), None);
        assert!(contains_ignore_case(b"Kick.WAV", b".wav"));
        assert!(!contains_ignore_case(b"Kick.WAV", b".aif"));
    }
}
