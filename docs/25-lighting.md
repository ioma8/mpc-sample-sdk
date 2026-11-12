# Pad lighting

The AC50's sixteen pads have individually addressable RGB LEDs. The app drives
them through a native per-pad callback; the SDK hooks that callback and exposes
an override API. You can light one pad, paint all sixteen, fade them, or restore
the app's own colours without touching the native lighting layer.

```rust
use mpc_sample_sdk::platform::lighting;

// Zero-based: 0 is pad 1, 15 is pad 16.
lighting::set_pad_light(0, 0x009b_30ff, 0.0);  // purple, off
lighting::set_pad_light(15, 0x009b_30ff, 1.0); // purple, full
lighting::send_pad_light(1, 0x00ff_0000, 0.25); // red, 25 %, sent now
lighting::restore_pad_rgbs();                   // back to the app's colours
```

## How it works

The native callback receives one `PadLight` per pad. Its 64 bits are:

```text
low 32 bits   RGB, 0xRRGGBB
high 32 bits  brightness, f32 bits
```

The app's callback converts that value and sends it to the AC50. The SDK
intercepts the callback before the conversion, so an override changes the same
final path the app uses. The native protocol keeps **7 bits per channel**; the
API takes 8-bit RGB and the hardware receives `R >> 1`, `G >> 1`, `B >> 1`.

The app refreshes lighting whenever its state changes (selection, playback,
mute, 16 Levels, screen changes). The SDK applies the stored override on every
one of those refreshes, so an override stays visible until you clear it. Pads
without an override keep the app's colour. The SDK also saves the app's last
native light for every pad, which is what makes restoration exact.

## API

All functions are in `mpc_sample_sdk::platform::lighting`.

| Function | Use |
|---|---|
| `set_pad_rgb(pad, rgb)` | Store an override at full brightness. |
| `set_pad_light(pad, rgb, brightness)` | Store an override with brightness `0.0..=1.0`. |
| `send_pad_light(pad, rgb, brightness)` | Store an override and send it immediately. |
| `clear_pad_rgb(pad)` | Remove one override without sending. |
| `clear_pad_rgbs()` | Remove all overrides without sending. |
| `restore_pad_rgb(pad)` | Remove one override and send the app's native light back. |
| `restore_pad_rgbs()` | Remove all overrides and restore all native lights. |
| `sender()` | The native sender object, or zero before the first callback. |

`PADS` is `16`.

`set_*` and `clear_*` are atomic and can be called from any thread. `send_*` and
`restore_*` call the app's native lighting callback, so call them on the app's
UI thread.

Brightness zero turns a pad off. The native converter clamps a small positive
brightness (up to `0.02`) to `0.02`, so a gradient that starts at zero should
jump to its first real step on pad 2.

## Lifecycle

An override is last-writer-wins per pad; there is no ownership stack. If two
features paint the same pad, the later write wins. Call `restore_pad_rgbs()` (or
the single-pad form) when leaving a screen that changed the pads. This sends the
saved native light immediately instead of waiting for the app's next refresh.

The SDK does not clear overrides automatically when a mod unloads or a screen
closes. Restoration is the feature's responsibility.

## Complete example: a brightness gradient

This is the synth page's chromatic layout: all sixteen pads share one hue, pad 1
is off, pad 16 is full, and the steps are linear. On close, every native colour
comes back.

```rust
use mpc_sample_sdk::platform::lighting;

const PURPLE: u32 = 0x009b_30ff;
const PADS: u8 = 16;

pub fn show_gradient() {
    for pad in 0..PADS {
        let brightness = f32::from(pad) / f32::from(PADS - 1);
        let _ = lighting::send_pad_light(pad, PURPLE, brightness);
    }
}

pub fn hide_gradient() {
    lighting::restore_pad_rgbs();
}
```

Call `show_gradient()` when the screen opens and `hide_gradient()` from every
close path, including cancel and the completion of a render. The synth feature
in `minimod` uses exactly this shape in `rust/crates/minimod/src/synth_lights.rs`.

## Emulator check

The virtual lab exposes the LEDs as 7-bit RGB in `/state`:

```bash
curl -s http://127.0.0.1:8765/state | jq '.pads'
```

Pad keys are zero-based; the value is `[r, g, b]` after the 7-bit conversion.
`mod/tests/emulator_synth_lights.py` checks every gradient step, the zero and
full endpoints, cancel restoration and render-close restoration.

## Limitations

- AC50 pad LEDs only; there is no equivalent API for the button LEDs.
- Sixteen pads, 7-bit channels; use `0xRRGGBB`, not a native palette index.
- Last-writer-wins per pad; the SDK does not arbitrate between features.
- No native-light change callback; the SDK stores the native value for restore
  but does not notify a mod when the app changes it.
- `send_*` and `restore_*` require the app's UI thread.
