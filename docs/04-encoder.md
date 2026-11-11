# The DATA encoder

`on_encoder` receives every turn of the DATA encoder. The event has the surface
object and a signed delta.

```rust
use mpc_sample_sdk::{EncoderEvent, EncoderFlow, Mod};

#[derive(Default)]
struct Encoder {
    row: i32,
}

impl Mod for Encoder {
    fn on_encoder(&mut self, event: EncoderEvent) -> EncoderFlow {
        if event.delta == 0 {
            return EncoderFlow::Forward;
        }
        self.row = (self.row + event.delta.signum()).rem_euclid(8);
        mpc_sample_sdk::draw::show_toast(&format!("row {}", self.row));
        EncoderFlow::Consume
    }
}

mpc_sample_sdk::export_mod!(Encoder);
```

`delta` is positive for clockwise, negative for counter-clockwise. The raw value
can be more than one; use `.signum()` if you only want one step per event.

## Only when the encoder belongs to your UI

The encoder event carries the control surface. The `ButtonEvent` carries the
same surface; store it when your UI opens, and compare:

```rust
use mpc_sample_sdk::{EncoderEvent, EncoderFlow, Mod};

#[derive(Default)]
struct OwnedEncoder {
    surface: usize,
    open: bool,
}

impl Mod for OwnedEncoder {
    fn on_encoder(&mut self, event: EncoderEvent) -> EncoderFlow {
        if !self.open || event.surface != self.surface {
            return EncoderFlow::Forward;
        }
        // ... change the selection ...
        EncoderFlow::Consume
    }
}
```

## Encoder push

The push is a button, not an encoder event. In `on_button` it is code `0x6f`:

```rust
use mpc_sample_sdk::{ButtonEvent, ButtonFlow, Mod};

#[derive(Default)]
struct Push;

impl Mod for Push {
    fn on_button(&mut self, event: ButtonEvent) -> ButtonFlow {
        if event.code == mpc_sample_sdk::event::button::DATA && event.pressed {
            mpc_sample_sdk::draw::show_toast("confirm");
        }
        ButtonFlow::Forward
    }
}
```
