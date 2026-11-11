# Audio input and output

The SDK taps the device's PCM stream and calls the hooks you return from
`Mod::audio`:

```rust
#[derive(Clone, Copy, Default)]
pub struct AudioHooks {
    pub output: Option<fn(&mut [i32], &OutputInfo)>,
    pub input_interleaved: Option<fn(&[i32], usize, u32)>,
    pub input_planar: Option<fn(&[&[i32]], u32)>,
}
```

The samples are **signed 32-bit**. `output` is called just before the app writes
the block; add to `frames` to mix into the output. The input hooks are for
meters, recording, analysis or a custom effect.

## The audio-thread rule

The hooks run on the audio thread **without the module lock**: the SDK copies
the function pointers once at install, so a slow UI callback (Extract, a file
write, a paint) can never drop an audio block. Do not allocate, lock, draw, show
a toast, or block. Keep any state they touch in atomics or another
interior-mutable static — never in the mod's `&mut self` UI state.

`Mod::audio` is called once, at install. A mod that returns `AudioHooks::default`
gets no audio callbacks.

## Example: mix a quiet tone into the output

```rust
use core::sync::atomic::{AtomicU32, Ordering};
use mpc_sample_sdk::audio::{self, OutputInfo};
use mpc_sample_sdk::{AudioHooks, Mod};

const MAX_FRAMES: usize = 4096;

/// One turn of the tone, as f32 bits.
static PHASE: AtomicU32 = AtomicU32::new(0);

fn tone_output(frames: &mut [i32], info: &OutputInfo) {
    if info.channels == 0 {
        return;
    }
    let count = (frames.len() / info.channels).min(MAX_FRAMES);
    let step = 440.0 / info.rate as f32;
    let mut phase = f32::from_bits(PHASE.load(Ordering::Relaxed));
    let mut left = [0.0_f32; MAX_FRAMES];
    let mut right = [0.0_f32; MAX_FRAMES];
    for index in 0..count {
        let value = (phase * core::f32::consts::TAU).sin() * 0.05;
        phase = (phase + step).fract();
        left[index] = value;
        right[index] = value;
    }
    PHASE.store(phase.to_bits(), Ordering::Relaxed);
    let planes: [&[f32]; 2] = [&left[..count], &right[..count]];
    let _ = audio::add_planes_i32(frames, info.channels, &planes);
}

#[derive(Default)]
struct Tone;

impl Mod for Tone {
    fn audio(&self) -> AudioHooks {
        AudioHooks {
            output: Some(tone_output),
            ..AudioHooks::default()
        }
    }
}

mpc_sample_sdk::export_mod!(Tone);
```

`add_planes_i32` converts f32 planes to the device's S32 format, interleaves
them, saturates, and leaves the rest of the block alone. It returns `false` for
unusable geometry.

## Reading input

```rust
use core::sync::atomic::{AtomicI32, Ordering};
use mpc_sample_sdk::{AudioHooks, Mod};

/// The last interleaved input block's peak.
static PEAK: AtomicI32 = AtomicI32::new(0);

fn meter_input(frames: &[i32], _channels: usize, _rate: u32) {
    let peak = frames.iter().map(|sample| sample.abs()).max().unwrap_or(0);
    PEAK.store(peak, Ordering::Relaxed);
}

#[derive(Default)]
struct InputMeter;

impl Mod for InputMeter {
    fn audio(&self) -> AudioHooks {
        AudioHooks {
            input_interleaved: Some(meter_input),
            ..AudioHooks::default()
        }
    }
}
```

The planar hook gives one S32 slice per channel when the app uses a planar
path. Use whichever hook the stream selects; do not assume both run.

## Latency

`OutputInfo::latency(backlog, extra)` returns the combined device latency in
frames and the source byte the app's display expects. The firmware's own
latency is one period less than each ring, `buffer - period`; use the
directional accessors when the two sides need different compensation:

```rust
fn latency_output(_frames: &mut [i32], info: &mpc_sample_sdk::audio::OutputInfo) {
    let input = info.input_latency();   // capture: buffer_in - period_in
    let output = info.output_latency(); // playback: buffer_out - period_out
    let (total, source) = info.latency(0, mpc_sample_sdk::audio::extra_latency());
    let _ = (input, output, total, source);
}
```

A recording drops `input_latency` frames; an overdub shifts by
`input_latency + output_latency`. `period_in`/`period_out` are read with
`snd_pcm_hw_params_get_period_size` on the stream's own thread.

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
