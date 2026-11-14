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

Use the named constants in `mpc_sample_sdk::event::button`; `code` remains available for raw device codes.

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
use mpc_sample_sdk::{ButtonEvent, Flow, Mod};

#[derive(Default)]
struct Buttons;

impl Mod for Buttons {
    fn on_button(&mut self, event: ButtonEvent) -> Flow {
        if event.code == mpc_sample_sdk::event::button::DATA && event.pressed && event.shift {
            mpc_sample_sdk::draw::show_toast("SHIFT + DATA");
            return Flow::Consume;
        }
        if event.code == mpc_sample_sdk::event::button::SHIFT {
            mpc_sample_sdk::platform::status::set_status(if event.pressed {
                "SHIFT down"
            } else {
                "SHIFT up"
            });
        }
        Flow::Forward
    }
}

mpc_sample_sdk::export_mod!(Buttons);
```

`Forward` means the app still handles the key. `Consume` means the mod owns it.

## Replaying a button

`ButtonEvent::bytes` is a copy of the original event. The SDK can replay a key
through the app's own handler, which is useful when a mod synthesizes a press:

```rust
use mpc_sample_sdk::{ButtonEvent, Flow, Mod};

#[derive(Default)]
struct Replay;

impl Mod for Replay {
    fn on_button(&mut self, event: ButtonEvent) -> Flow {
        if event.code == mpc_sample_sdk::event::button::DATA && event.pressed {
            // SAFETY: `event` was captured in this callback, so its context and
            // surface are still live while we replay it.
            let _ = unsafe { mpc_sample_sdk::event::replay_button(&event, mpc_sample_sdk::event::button::CHOP) };
        }
        Flow::Forward
    }
}
```

Do not replay the same event recursively without a guard; keep a flag in your
mod or a `Global`.

To press a key from a *later* callback, keep the 16 bytes and call
`replay_button_live`. It first checks that the event's context still looks like a
live app object, so a feature can undo a page mode the app changed while a native
command ran without dereferencing a stale object:

```rust
// SAFETY: `bytes` came from the page that is still up, and this runs on its UI
// thread; the SDK re-checks the context before calling the app.
let _ = unsafe { mpc_sample_sdk::event::replay_button_live(&bytes, mpc_sample_sdk::event::button::CHOP) };
```

The check proves the vtable belongs to the app image, not which object it is, so
only replay events captured on the page that is current now.
