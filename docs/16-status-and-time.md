# Status, toasts and time

Three small conveniences make a mod feel native.

## Status line

`platform::status` is the one-line text at the bottom of the Sample page. It is
fixed at 96 bytes; long strings are truncated by the app.

```rust
use mpc_sample_sdk::platform::status;

status::set_status("ready");
// -> "ready"

status::set_prefixed("Reverb", "done");
// -> "Reverb: done"

let current = status::text();
let _ = current;
```

## Toast

A toast is a temporary centred message. Store it, then ask for the frame that
shows it:

```rust
use mpc_sample_sdk::{ButtonEvent, ButtonFlow, Mod};

#[derive(Default)]
struct Toasts;

impl Mod for Toasts {
    fn on_button(&mut self, event: ButtonEvent) -> ButtonFlow {
        if event.code == mpc_sample_sdk::event::button::DATA && event.pressed {
            mpc_sample_sdk::draw::show_toast("saved");
            mpc_sample_sdk::draw::repaint_painted();
        }
        ButtonFlow::Forward
    }
}
```

A toast normally lasts about one second. Calling `show_toast` again replaces it.

## Time

Use the app's clock for timeouts and envelope-like behaviour:

```rust
use mpc_sample_sdk::platform::clock;

let started = clock::now_ms();
// ... later, in a paint or timer callback ...
let elapsed = clock::now_ms() - started;
if elapsed > 500 {
    mpc_sample_sdk::platform::status::set_status("timeout");
}
```

`now_ms` returns milliseconds as `i64`; it is the same clock the app uses.

## Latency

```rust
use mpc_sample_sdk::audio;

let trim = audio::extra_latency();
let _ = trim; // frames, for a latency display or a recording offset
```

Do not block the UI waiting for time to pass. Poll `now_ms` from a paint or a
pump, or do the waiting on a `Task`.
