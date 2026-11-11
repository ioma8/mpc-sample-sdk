# Process-wide state

A mod usually needs state that outlives one callback: a menu row, a mode flag, a
job handle. `state::Global<T>` preserves a static’s layout without `static mut`. Its accessors are unsafe:
the caller must enforce single-thread access and Rust’s borrowing rules.

> `Global` is **single-threaded**. Touch it from the app's UI thread only. For
> audio-thread data use atomics or a ring buffer, not `Global`.

## Example: a menu flag

```rust
use mpc_sample_sdk::state::Global;

#[derive(Clone, Copy, Default)]
struct UiState {
    open: bool,
    row: i32,
}

static UI: Global<UiState> = Global::new(UiState { open: false, row: 0 });

fn open() {
    // SAFETY: UI-thread access, with no overlapping references or reentry.
    unsafe { UI.with_mut(|state| {
        state.open = true;
        state.row = 0;
    }); }
}

fn row() -> i32 {
    // SAFETY: UI-thread access with no active mutable borrow.
    unsafe { UI.with(|state| state.row) }
}

fn close() {
    // SAFETY: UI-thread access with no overlapping references.
    unsafe { UI.get_mut().open = false; }
}
```

`get`/`get_mut` return references; `with`/`with_mut` scope the borrow and are
usually easier to read.

## Exporting a static for a test or another module

The device harness reads some statics by name. `export_static!` emits the
exported symbol while keeping the `export_name` attribute out of your code:

```rust
use mpc_sample_sdk::state::Global;

#[derive(Clone, Copy, Default)]
pub struct DebugState {
    pub jobs: i32,
    pub errors: i32,
}

mpc_sample_sdk::export_static!(
    /// Read by the lab test.
    g_my_debug: Global<DebugState> = Global::new(DebugState { jobs: 0, errors: 0 })
);
```

`g_my_debug` then exists under that exact name. Do not create an exported static
for ordinary private state; use a plain `static` for that.

## State and callbacks

```rust
use mpc_sample_sdk::{ButtonEvent, ButtonFlow, Mod};
use mpc_sample_sdk::state::Global;

#[derive(Clone, Copy, Default)]
struct Toggle {
    on: bool,
}

static TOGGLE: Global<Toggle> = Global::new(Toggle { on: false });

#[derive(Default)]
struct StateMod;

impl Mod for StateMod {
    fn on_button(&mut self, event: ButtonEvent) -> ButtonFlow {
        if event.code == 0x6f && event.pressed {
            // SAFETY: UI callback; borrows end before invoking firmware.
            let on = unsafe {
                TOGGLE.with_mut(|state| { state.on = !state.on; state.on })
            };
            mpc_sample_sdk::draw::show_toast(if on {
                "on"
            } else {
                "off"
            });
            mpc_sample_sdk::draw::repaint_painted();
        }
        ButtonFlow::Forward
    }
}

mpc_sample_sdk::export_mod!(StateMod);
```

If a worker needs to report progress, store an atomic (`AtomicI32`,
`AtomicBool`) and let the UI poll it; do not write `Global` from the worker.

Migration from v0.1.10: audit each accessor call for thread ownership, overlapping
references and callback reentry before adding its unsafe block. Never hold a
mutable state reference across a firmware call that can invoke another callback.
