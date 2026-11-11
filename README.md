# mpc-sample-sdk

**A Rust SDK for building Akai MPC Sample mods.**

The universal mod SDK for the Akai MPC Sample 1.3.0.12, written in Rust
(edition 2024, MSRV 1.97). The SDK is an ordinary Cargo crate: a mod is a Rust
`cdylib`, and the build script invokes `cargo` — no C or C++ is compiled.

## Built in Rust

Rust is not a wrapper around a C library here; it is the implementation and the
public API.

- **Typed events.** Buttons, pads, the encoder, knobs, MIDI notes and paint
  calls are Rust structs and enums, not raw event pointers.
- **Typed app objects.** Selections, samples, pads, slices, WAV files, effects
  and Extract requests have checked accessors and `Option`/`Result` errors.
- **Explicit firmware contracts.** Events and owned audio have safe APIs. Raw
  firmware calls and UI state access are unsafe where callers must establish
  object lifetime, thread ownership, or exclusive access.
- **One build tool.** [`build-mod.sh`](build-mod.sh) uses Cargo for the
  cross-build, then checks symbols, glibc and the packaged `autorun` zip.
- **Checked in CI-friendly ways.** The crate is fmt-, clippy- (`all`,
  `pedantic`, `cargo`, warnings denied) and rustdoc-clean, and its tests run on
  the host.

See [the hands-on guide](docs/README.md) and the
[minimod](https://github.com/ioma8/mpc-minimod) repository for a complete mod.

## What the SDK owns

It owns everything that is not a feature:

* **Firmware** — find the loaded app, verify the 37 fingerprint words, and map
  the image (`image`, `loader`).
* **Native sample seed** — create a silent, app-owned stereo sample in memory
  for Extract when no pad provides a source. The SDK retains one seed for the
  process lifetime; Extract writes and registers the rendered output as a WAV.
* **Hooks** — the in-memory `b`/relay/trampoline engine and all 17 sites.  The
  displaced originals stay inside `sites`; a mod never stores or types one.
* **API** — the `Mod` trait, the event types, and `export_mod!`, which emits
  the `.init_array` entry point that installs the hooks and calls the mod.
* **Audio** — the four ALSA taps the app resolves by name, the stream geometry,
  `OutputInfo` for a mod that mixes into the output or listens to the input, and
  the latency the device geometry implies.
* **Drawing** — a `Graphics` handle over the app's own `setColour`/`fillRect`/
  `Font` calls, the toast, the installed page-paint slot and the repaint pump.
* **Platform objects** — typed, bounds-checked access to the page, the selected
  sample, pads, slices, trim, WAV files, paths, the status line, the clock, the
  writer, the control surface, the effect renderer and the Extract command.
* **UI state** — `state::Global` preserves exported state layout. Its unsafe
  accessors require UI-thread ownership and non-overlapping borrows; see
  [state safety and migration](docs/15-state.md).

## Install

The crate is published from this repository:

```toml
[dependencies]
mpc-sample-sdk = { git = "https://github.com/ioma8/mpc-sample-sdk", tag = "v0.2.1" }
```

A mod builds as a `cdylib`:

```toml
[lib]
crate-type = ["cdylib"]
```

The SDK ships one build script for every mod:

```sh
./build-mod.sh --manifest-path /path/to/my-mod/Cargo.toml --package my-mod --name my-mod
```

It cross-builds, strips, checks glibc and writes `build/my-mod.so`; add `--pack`
and a firmware hash to produce the SD `autorun` zip. Full options and install
steps: [docs/22-build-and-install.md](docs/22-build-and-install.md).

## Documentation

The hands-on guide is in [`docs/`](docs/README.md). It has one short page per
topic — buttons, pads, encoder, knobs, MIDI, screens, drawing, samples, audio,
tasks, Extract, WAV files, state, firmware, build/install and the
[emulator loop](docs/23-autonomous-development.md) — and every page ends with a
code example.

## A minimal mod

```rust
use mpc_sample_sdk::{Mod, PadEvent, PadFlow};

#[derive(Default)]
struct MyMod;

impl Mod for MyMod {
    fn on_pad(&mut self, event: PadEvent) -> PadFlow {
        if event.action == mpc_sample_sdk::PadAction::Press {
            // ...
        }
        PadFlow::Forward
    }
}

mpc_sample_sdk::export_mod!(MyMod);
```

Build it as a `cdylib` and `LD_PRELOAD` it on the device.  `install` refuses a
missing app, a different firmware and a build where no site could be patched;
the mod's `on_start` runs only after the hooks are in.

## Layout

```
src/lib.rs          the public surface and the crate docs
src/event.rs        Mod, the event types and the flows
src/loader.rs       install, export_mod!, the firmware words
src/sites.rs        the 17 site adapters
src/hook.rs         the patch engine
src/image.rs        the image, memory window and firmware call helpers
src/audio.rs        the ALSA taps, OutputInfo, latency and S32 mixing
src/draw.rs         Graphics, the toast, the screen and the repaint pump
src/midi.rs         NoteEvent and replay
src/task.rs         the generic one-shot worker
src/render.rs       RenderJob, RenderOutput and source metadata
src/platform/       page, sample, pads, slices, trim, WAV, paths, status,
                    clock, writer, control, effect, extract
```

## Vocabulary

| term | meaning |
|---|---|
| **Selection** | The exact Sample-page source: screen, page state, sample, frames, channels and rate. `Selection::is_current()` answers “is this still the visible selection?”; `read_audio()` loads the app buffer first, then the WAV. |
| **Task** | One generic one-shot worker: it owns a value, moves it to a named thread, catches a panic when the runtime unwinds, and hands the value back once through `take()`. |
| **RenderJob** | One audio request: a `Selection`, a feature-defined `kind`, interleaved input and the `RenderOutput` buffers the backend fills. |
| **RenderOutput** | One interleaved f32 buffer plus its frame and channel geometry. |
| **backend** | Feature-side code that knows a DSP algorithm (`minimod/src/backend/`). The SDK never depends on it. |

The SDK does not know any feature.  See the public
[minimod](https://github.com/ioma8/mpc-minimod) project for a complete MPC Sample
mod built on it: the SAMPLE OPTIONS menu, AutoTrim, HPSS, Spectral Chop, the
firmware effects, the bass synth and the live looper.

## License

MIT. See [LICENSE](LICENSE).

## v0.2.0 safety migration

`v0.2.0` contains API-breaking safety corrections. Existing `v0.1.10` consumers
remain pinned to that release. Before adopting v0.2.0, follow
[the migration guide](docs/24-safety-migration.md); do not add blanket unsafe
blocks around feature code.
