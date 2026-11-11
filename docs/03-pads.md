# Pads

Pads are the 16 physical pads. They arrive through `on_pad`; the same physical
press may also appear on the control-surface path (`on_surface`) depending on
what the app is doing. If you only need “is pad 5 down?”, use `on_pad`.

```rust
pub struct PadEvent {
    pub action: PadAction,  // Press or Release
    pub input: usize,       // the app's input object
    pub source: i32,        // the app's source id
    pub pad: i32,           // 0-based pad index
    pub properties: usize,  // the app's property block
    pub velocity: f32,      // 0 on release
}
```

## Example: own pad 5 on the Sample page

```rust
use mpc_sample_sdk::{Mod, PadAction, PadEvent, PadFlow};

#[derive(Default)]
struct Pads;

impl Mod for Pads {
    fn on_pad(&mut self, event: PadEvent) -> PadFlow {
        if event.pad != 4 {
            return PadFlow::Forward;
        }
        match event.action {
            PadAction::Press => {
                mpc_sample_sdk::draw::show_toast(&format!(
                    "pad 5, velocity {:.2}",
                    event.velocity
                ));
            }
            PadAction::Release => {
                mpc_sample_sdk::draw::show_toast("pad 5 released");
            }
        }
        PadFlow::Consume
    }
}

mpc_sample_sdk::export_mod!(Pads);
```

`Consume` stops the app from triggering the pad's normal sample playback.

## Example: only while a specific screen is visible

```rust
use mpc_sample_sdk::{Mod, PadEvent, PadFlow};

#[derive(Default)]
struct ScopedPads;

impl Mod for ScopedPads {
    fn on_pad(&mut self, event: PadEvent) -> PadFlow {
        let screen = mpc_sample_sdk::platform::page::screen();
        // SAFETY: `screen` is the live page recorded by the paint hook on this
        // UI thread.
        if !unsafe { mpc_sample_sdk::platform::page::visible(screen) } {
            return PadFlow::Forward;
        }
        if event.pad == 0 {
            return PadFlow::Consume;
        }
        PadFlow::Forward
    }
}
```

## Pad → sample lookup

When you need to know which sample a pad points at, use the surface table:

```rust
use mpc_sample_sdk::platform::{control, pads};

let pad = 4;
let surface = control::surface();
if let Some(sample) = pads::sample_for_pad(surface, pad) {
    let frames = mpc_sample_sdk::platform::sample::frames(sample);
    mpc_sample_sdk::platform::status::set_status(&format!("pad 5: {frames} frames"));
}
```

`sample_for_pad` takes the **surface object**, not the screen component.
`control::surface()` is the surface the SDK learned from the app's control hook.

## Activate a populated layer

`set_layer_active` updates the pad's native `InstrumentLayer::active` byte. It
returns `false` when the pad does not have a populated layer or the update
cannot be confirmed:

```rust
use mpc_sample_sdk::platform::{control, pads};

let surface = control::surface();
let pad = 0; // A01, zero-based
// SAFETY: this is the live surface from the current UI callback, this thread
// owns it, and no other code mutates the layer during the write.
let activated = unsafe { pads::set_layer_active(surface, pad, true) };
```

On firmware 1.3.0.12, Ghidra identifies this byte at `InstrumentLayer + 0xb18`;
setting it on a newly rendered layer makes playback audible immediately and
persists as `active: true` when the project is saved.
