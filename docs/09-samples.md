# Samples and the selection

`Selection` is the safe, typed description of what the Sample page currently
has selected. Everything that reads or replaces a sample starts there.

```rust
use mpc_sample_sdk::platform::selection;

fn describe_selection() {
    let screen = mpc_sample_sdk::platform::page::screen();
    // SAFETY: `screen` is the live Sample page recorded by the paint hook.
    let Some(selected) = unsafe { selection::current(screen) } else {
        mpc_sample_sdk::platform::status::set_status("no sample selected");
        return;
    };

    let frames = selected.frames();
    let channels = selected.channels();
    let rate = selected.rate();
    let sample = selected.sample();
    let _ = (frames, channels, rate, sample);
}
```

`current` returns `None` for a stale page, a non-Sample page, an empty pad, or a
sample whose geometry does not validate.

## Reading the audio

`Selection::read_audio` is the normal way to get samples:

```rust
use mpc_sample_sdk::platform::selection;

fn copy_selected_audio() -> Option<Vec<f32>> {
    let screen = mpc_sample_sdk::platform::page::screen();
    // SAFETY: `screen` is the live Sample page recorded by the paint hook.
    let selected = unsafe { selection::current(screen) }?;
    match selected.read_audio() {
        Ok(audio) => Some(audio),
        Err(error) => {
            mpc_sample_sdk::platform::status::set_status(&format!("{error:?}"));
            None
        }
    }
}
```

It prefers the app's in-memory float buffer and falls back to the exact WAV file
for a streamed sample. The returned vector is interleaved `f32`, length
`frames * channels`.

## Checking that a source is still valid

A sample can change between the moment you read it and the moment you write it.
Before writing, check:

```rust
use mpc_sample_sdk::platform::selection;

let screen = mpc_sample_sdk::platform::page::screen();
// SAFETY: `screen` is the live Sample page recorded by the paint hook.
let Some(selected) = unsafe { selection::current(screen) } else { return };
// ... do work ...
if !selected.is_current() {
    mpc_sample_sdk::platform::status::set_status("selection changed");
    return;
}
```

`is_current` checks the page is visible **and** that every field of the
selection still matches.

## Sample metadata without a selection

```rust
use mpc_sample_sdk::platform::sample;

let sample = 0x1234_5678; // a sample object from your own walk
let frames = sample::frames(sample);
let geometry = sample::geometry(sample);
let root = sample::root_note(sample);

if let Some((frames, channels, rate)) = geometry {
    mpc_sample_sdk::platform::status::set_status(&format!(
        "{frames} frames, {channels} ch, {rate} Hz, root {root}"
    ));
}
```

`sample::memory` returns a `Memory` only when the app holds the sample in RAM;
otherwise use `paths` + `wav` (see [WAV files and paths](14-wav-and-paths.md)).

## Pad room

```rust
use mpc_sample_sdk::platform::{pads, selection};

let screen = mpc_sample_sdk::platform::page::screen();
// SAFETY: `screen` is the live Sample page recorded by the paint hook.
if let Some(selected) = unsafe { selection::current(screen) } {
    if let Some(program) = selection::program(selected.state()) {
        // SAFETY: `program` came from the live selection in this UI callback.
        if !unsafe { pads::has_room(Some(program), 2) } {
            mpc_sample_sdk::platform::status::set_status("need 2 empty pads");
        }
    }
}
```

An unknown program (`None` or zero) is treated as “no proven room” and returns
`false`; check before doing an expensive render that creates pads.

## Native silent seed

`platform::sample::silent_seed()` returns an app-owned silent stereo sample
when the firmware's Temporary Samples service has been captured. It returns
`None` before that service is available or if native construction fails. The
SDK keeps one seed alive for the process lifetime because Extract can retain
references to it. The seed is not inserted into the sample pool; it is meant
as a temporary Extract source, not as a sample to assign directly. Stock
Extract writes and registers the final WAV.
