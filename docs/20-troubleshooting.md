# Troubleshooting

## The mod does nothing

1. Is the library mapped? `grep minimod.so /proc/$(pidof "MPC Sample")/maps`.
2. Did the firmware check fail? `export_mod!` ignores `InstallError`; a direct
   `loader::install` build can print it.
3. Are you on the Sample page? `on_paint`/`selection` only work there.
4. Is the event already consumed by the app? Return `Forward` and inspect the
   event first.

## A diagnostic `on_start`

```rust
use mpc_sample_sdk::Mod;

#[derive(Default)]
struct Diagnostic;

impl Mod for Diagnostic {
    fn on_start(&mut self) {
        let base = mpc_sample_sdk::loader::find_main();
        let notes = mpc_sample_sdk::sites::notes_ready();
        let screen = mpc_sample_sdk::platform::page::screen();
        mpc_sample_sdk::platform::status::set_status(&format!(
            "base {base:#x} notes {notes} screen {screen:#x}"
        ));
    }
}

mpc_sample_sdk::export_mod!(Diagnostic);
```

## The UI freezes

You are doing the work on the UI thread. Move rendering to `Task` (see
[Tasks](12-tasks.md)) and call `draw::repaint_painted()` when it finishes.

## The worker finished but nothing changed

The worker must repaint. `Task` only stores the result; the UI has to ask for a
frame and call `Task::take()` from `on_paint`.

## “need N empty pads”

`pads::has_room` can prove room only for a known program. Free the pads first, or
check `selection::program` before starting. An unknown program deliberately
returns `false` instead of writing over a pad.

## Toast appears only after another action

`draw::show_toast` stores the text. Call `draw::repaint_painted()` after it.

## Clicks or pads stop reaching the app

A callback returned `Consume` and never releases. Return `Forward` unless the
mod genuinely owns the event.

## Audio callback problems

Do not allocate, lock, draw or call `status` from the `AudioHooks` functions
(`output`, `input_interleaved`, `input_planar`). Use atomics or a preallocated
buffer and let the UI poll.

## The mod panics

Release builds use `panic = "abort"`; a panic kills the app instead of becoming
`TaskError::Panicked`. Test the logic on the host with `cargo test` and use
`Option`/`Result` instead of `unwrap` on app data.
