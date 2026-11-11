# WAV files and paths

The SDK has a small WAV reader and the path rules the app uses. These are the
fallbacks when a sample is streamed instead of held in RAM.

## Read a WAV header

```rust
use mpc_sample_sdk::platform::wav;

let path = std::path::Path::new(
    "/data/TempProjects/Temp-Project_[ProjectData]/kick.wav",
);
if let Some(info) = wav::Info::read(path) {
    mpc_sample_sdk::platform::status::set_status(&format!(
        "{} frames, {} ch, {} Hz, {} bytes/sample",
        info.frames, info.channels, info.rate, info.width
    ));
}
```

## Read samples as f32

```rust
use mpc_sample_sdk::platform::wav;

let path = std::path::Path::new(
    "/data/TempProjects/Temp-Project_[ProjectData]/kick.wav",
);
let audio = wav::read_f32(path, 10_501, 2, 44_100);
if let Some(samples) = audio {
    assert_eq!(samples.len(), 10_501 * 2);
}
```

The reader accepts PCM 16/24/32-bit and IEEE float 32/64; other widths or
encodings return `None`. Reads are capped to the requested frame count, so extra
RIFF chunks or trailing audio cannot change the result.

Pass the geometry you expect. `read_f32` returns `None` when the header, length
or format does not match; it does not guess.

## Find the WAV for a sample

```rust
use mpc_sample_sdk::platform::{paths, sample};

fn sample_path(sample_object: usize) -> Option<std::path::PathBuf> {
    let frames = sample::frames(sample_object);
    paths::for_sample(sample_object, frames, 2, 44_100)
}
```

`paths::for_sample` uses only the sample's own object graph. It never scans
temporary projects: a matching frame count does not prove that a temporary file
is this recording. `paths::from_object` is the object-only variant.

A freshly recorded sample has no stored path yet. Pass its project directory
explicitly:

```rust
use mpc_sample_sdk::platform::paths;

let project = std::path::Path::new(
    "/data/TempProjects/Temp-Project_[ProjectData]",
);
let path = paths::for_recording(sample_object, frames, 2, 44_100, project);
```

`for_recording` searches only `project` and accepts the result only when exactly
one WAV has the requested frames, channels and rate.

To inspect the single match yourself:

```rust
use mpc_sample_sdk::platform::paths;

let project = std::path::Path::new(
    "/data/TempProjects/Temp-Project_[ProjectData]",
);
if let Some(path) = paths::unique(project, 44_100, 2, 44_100) {
    mpc_sample_sdk::platform::status::set_status(&format!("{}", path.display()));
}
```

`unique` finds the one existing file with exactly that geometry. It does not
create a path or a file; the SDK does not write the WAV for you. Use `extract`
for pads or write your own file under the same project.

## Reading text paths

```rust
use mpc_sample_sdk::platform::paths;

let project_text = b"/data/TempProjects/My Project_[ProjectData]/kick.wav\0";
if let Some(path) = paths::in_text(project_text, 10_501) {
    mpc_sample_sdk::platform::status::set_status(&format!("{}", path.display()));
}
```

`in_text` searches a NUL-terminated project path blob; `in_text_under` restricts
the search to given roots.
