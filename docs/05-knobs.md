# Knobs and CC

Knobs arrive as control-change events in `on_cc`. A `CcEvent` gives the raw
controller number, the raw value, the controller's low value and its range.

```rust
pub struct CcEvent {
    pub event: usize,
    pub control: u32,
    pub value: u32,
    pub low: f32,
    pub range: f32,
}
```

## Example: three knobs

```rust
use mpc_sample_sdk::{CcEvent, CcFlow, Mod};

#[derive(Default)]
struct Knobs {
    values: [f32; 3],
}

impl Mod for Knobs {
    fn on_cc(&mut self, event: CcEvent) -> CcFlow {
        let index = match event.control {
            0x10 => 0,
            0x11 => 1,
            0x12 => 2,
            _ => return CcFlow::Forward,
        };
        let normalized = if event.range > 0.0 {
            (event.low / event.range).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.values[index] = normalized;
        mpc_sample_sdk::platform::status::set_status(&format!(
            "K1 {:.2}  K2 {:.2}  K3 {:.2}",
            self.values[0], self.values[1], self.values[2]
        ));
        CcFlow::Consume
    }
}

mpc_sample_sdk::export_mod!(Knobs);
```

The numbers `0x10..=0x12` are examples. Use the controller numbers your app
actually sends; log `event.control` first if you are unsure.

## Keep the app's value in sync

Sometimes you want the app to keep handling the knob, but also observe it:

```rust
use mpc_sample_sdk::{CcEvent, CcFlow, Mod};

#[derive(Default)]
struct Observe;

impl Mod for Observe {
    fn on_cc(&mut self, event: CcEvent) -> CcFlow {
        if event.control == 0x10 {
            mpc_sample_sdk::platform::status::set_status(&format!("raw {}", event.value));
        }
        CcFlow::Forward
    }
}
```

`Forward` passes the event to the app after your callback.

## Using the control-surface setter

The SDK also exposes the app's control setter:

```rust
use mpc_sample_sdk::platform::control;

// SAFETY: the SDK remembers the live surface from the control hook; this UI
// callback runs on its owning thread.
let _ = unsafe { control::set(4, 0, 0.5) };
```

Use this only when you need to drive the app's own control state; ordinary knob
input should stay on the `on_cc` path.
