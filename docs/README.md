# Using the MPC Sample SDK

`mpc-sample-sdk` is a **Rust SDK**: a mod is a Rust `cdylib`, the events and app
objects are typed Rust values, and the SDK isolates the firmware `unsafe` so the
mod itself is safe Rust. The build tool is Cargo-based (`build-mod.sh`); no C or
C++ is compiled.

This folder is a hands-on cookbook. Each page is short, explains one part of the
SDK, and ends with a complete snippet you can paste into a mod.

Start here if you have never built a mod:

1. [Lifecycle and a minimal mod](01-lifecycle.md)
2. [Buttons](02-buttons.md)
3. [Pads](03-pads.md)
4. [The DATA encoder](04-encoder.md)
5. [Knobs and CC](05-knobs.md)
6. [MIDI notes](06-midi.md)
7. [Screens and page detection](07-screens.md)
8. [Drawing](08-drawing.md)
9. [Samples and the selection](09-samples.md)
10. [Chop slices and AutoTrim](10-slices-and-trim.md)
11. [Audio input and output](11-audio.md)
12. [Tasks and render jobs](12-tasks.md)
13. [Creating pads with Extract](13-extract.md)
14. [WAV files and paths](14-wav-and-paths.md)
15. [Process-wide state](15-state.md)
16. [Status, toasts and time](16-status-and-time.md)
17. [Firmware and the loader](17-firmware.md)
18. [Advanced SDK surface](18-advanced.md)
19. [A complete example mod](19-cookbook.md)
20. [Troubleshooting](20-troubleshooting.md)
21. [Firmware effects](21-effects.md)
22. [Build and install a mod](22-build-and-install.md)
23. [Autonomous development and emulator testing](23-autonomous-development.md)

Every page has a runnable-shaped example. The examples assume:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
mpc-sample-sdk = { git = "https://github.com/ioma8/mpc-sample-sdk", tag = "v0.1.10" }
```

A mod is installed by `export_mod!`, built as a `cdylib`, and loaded with
`LD_PRELOAD` on the device (or `lab.py preload` in the virtual lab).

```rust
use mpc_sample_sdk::{Mod, PadEvent, PadFlow};

#[derive(Default)]
struct MyMod;

impl Mod for MyMod {
    fn on_pad(&mut self, _event: PadEvent) -> PadFlow {
        PadFlow::Forward
    }
}

mpc_sample_sdk::export_mod!(MyMod);
```

## The one rule

The SDK owns the firmware, hooks, raw pointers and the unsafe code. A feature
crate should contain **safe Rust only**. The examples never use `unsafe`; the
few low-level entry points are marked where they belong in the advanced section.
