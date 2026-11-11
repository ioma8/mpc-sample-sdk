# Buttons

Buttons arrive in `Mod::on_button`. The SDK already decoded the key code, the
press/release value, the SHIFT and ERASE state, and the control surface behind
the event.

```rust
pub struct ButtonEvent {
    pub event: usize,       // the app's object; rarely needed
    pub bytes: [u8; 16],    // copy, useful for a replay
    pub code: u32,          // key code
    pub value: u32,         // raw value; 0x7f is a press
    pub pressed: bool,      // value == 0x7f
    pub shift: bool,        // SHIFT held before this event
    pub erase: bool,        // ERASE held before this event
    pub surface: usize,     // zero when there is no surface
}
```

## Key codes used by the stock app

| code | key |
|---|---|
| `0x31` | SHIFT |
| `0x09` | ERASE |
| `0x6f` | DATA / encoder push |
| `0x0e` | B3 (trim cycle) |
| `0x36` | PLUS |
| `0x37` | MINUS |
| `0x28` | 16 LEVELS |

## Example: a chord and a toast

```rust
use mpc_sample_sdk::{ButtonEvent, ButtonFlow, Mod};

#[derive(Default)]
struct Buttons;

impl Mod for Buttons {
    fn on_button(&mut self, event: ButtonEvent) -> ButtonFlow {
        if event.code == 0x6f && event.pressed && event.shift {
            mpc_sample_sdk::draw::show_toast("SHIFT + DATA");
            return ButtonFlow::Consume;
        }
        if event.code == 0x31 {
            mpc_sample_sdk::platform::status::set_status(if event.pressed {
                "SHIFT down"
            } else {
                "SHIFT up"
            });
        }
        ButtonFlow::Forward
    }
}

mpc_sample_sdk::export_mod!(Buttons);
```

`Forward` means the app still handles the key. `Consume` means the mod owns it.

## Replaying a button

`ButtonEvent::bytes` is a copy of the original event. The SDK can replay a key
through the app's own handler, which is useful when a mod synthesizes a press:

```rust
use mpc_sample_sdk::{ButtonEvent, ButtonFlow, Mod};

#[derive(Default)]
struct Replay;

impl Mod for Replay {
    fn on_button(&mut self, event: ButtonEvent) -> ButtonFlow {
        if event.code == 0x6f && event.pressed {
            // SAFETY: `event` was captured in this callback, so its context and
            // surface are still live while we replay it.
            let _ = unsafe { mpc_sample_sdk::sites::replay_button(&event.bytes, 0x4b) };
        }
        ButtonFlow::Forward
    }
}
```

Do not replay the same event recursively without a guard; keep a flag in your
mod or a `Global`.
