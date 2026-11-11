# Audio input and output

The SDK taps the device's PCM stream and calls your mod from the audio thread:

```rust
fn on_input_interleaved(&mut self, frames: &[i32], channels: usize, rate: u32);
fn on_input_planar(&mut self, channels: &[&[i32]], rate: u32);
fn on_output(&mut self, frames: &mut [i32], info: &OutputInfo);
```

The samples are **signed 32-bit**. `on_output` is called just before the app
writes the block; add to `frames` to mix into the output. `on_input_*` is for
meters, recording, analysis or a custom effect.

## The audio-thread rule

These callbacks run on the audio thread. Do not allocate, lock a mutex, draw,
show a toast, or block. Keep a preallocated scratch buffer in your mod, or write
into an atomic/ring buffer that the UI reads.

The SDK holds one exclusive `&mut self` at a time. The audio callback never
waits: if a UI callback is already inside your mod, the audio callback is
forwarded without calling it. A UI callback waits for an in-flight audio
callback so input is not dropped, but it never waits for another UI callback.
Keep audio callbacks short and never block inside them.

## Example: mix a quiet tone into the output

```rust
use mpc_sample_sdk::audio::{self, OutputInfo};
use mpc_sample_sdk::Mod;

const MAX_FRAMES: usize = 4096;

#[derive(Default)]
struct Tone {
    phase: f32,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Mod for Tone {
    fn on_start(&mut self) {
        self.left = vec![0.0; MAX_FRAMES];
        self.right = vec![0.0; MAX_FRAMES];
    }

    fn on_output(&mut self, frames: &mut [i32], info: &OutputInfo) {
        if info.channels == 0 {
            return;
        }
        let count = (frames.len() / info.channels).min(MAX_FRAMES);
        let step = 440.0 / info.rate as f32;
        for index in 0..count {
            let value = (self.phase * core::f32::consts::TAU).sin() * 0.05;
            self.phase = (self.phase + step).fract();
            self.left[index] = value;
            self.right[index] = value;
        }
        let planes: [&[f32]; 2] = [&self.left[..count], &self.right[..count]];
        let _ = audio::add_planes_i32(frames, info.channels, &planes);
    }
}

mpc_sample_sdk::export_mod!(Tone);
```

`add_planes_i32` converts f32 planes to the device's S32 format, interleaves
them, saturates, and leaves the rest of the block alone. It returns `false` for
unusable geometry.

## Reading input

```rust
use mpc_sample_sdk::Mod;

#[derive(Default)]
struct InputMeter {
    peak: i32,
}

impl Mod for InputMeter {
    fn on_input_interleaved(&mut self, frames: &[i32], _channels: usize, _rate: u32) {
        self.peak = frames.iter().map(|sample| sample.abs()).max().unwrap_or(0);
    }
}
```

`on_input_planar` gives one S32 slice per channel when the app uses a planar
path. Use whichever callback the stream selects; do not assume both run.

## Latency

`OutputInfo::latency(backlog, extra)` returns the device latency in frames and
the source byte the app's display expects:

```rust
fn on_output(&mut self, _frames: &mut [i32], info: &mpc_sample_sdk::audio::OutputInfo) {
    let (frames, source) = info.latency(0, mpc_sample_sdk::audio::extra_latency());
    let _ = (frames, source);
}
```

The calibration trim is available as `audio::extra_latency()` and
`audio::set_extra_latency(frames)`. This is a UI-side setting; do not change it
from the audio callback.

The exported two-second output tap and `OutputInfo::output_peak_bits` are only
updated for a validated S32 stream with a plausible channel count and
`written <= requested`. An S16 (or otherwise formatted) stream is forwarded and
ignored rather than being read as S32.

## Buffer helpers

- `audio::output_buffer()` / `audio::input_buffer()` — ring sizes in frames.
- `audio::input_delay()` — input backlog.
- `audio::mark_buffers_stale()` / `audio::buffers_stale()` — geometry cache.
- `audio::stream_geometry(...)` — unsafe low-level ALSA geometry; most mods
  should use `OutputInfo` instead.
