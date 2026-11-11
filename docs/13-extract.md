# Creating pads with Extract

The supported way to create a pad is to let the app's own Extract command write
the WAV and assign the pad. The SDK arms a rendered buffer and calls Extract; the
app keeps ownership of the file, the pad and its undo state.

## The request

```rust
use mpc_sample_sdk::platform::extract::{self, Request};

fn create_pad() {
let screen = mpc_sample_sdk::platform::page::screen();
// SAFETY: `screen` is the live Sample page recorded by the paint hook.
let Some(selected) = unsafe { mpc_sample_sdk::platform::selection::current(screen) } else {
    return;
};

let audio = vec![0.0_f32; selected.frames() as usize * selected.channels() as usize];
let request = Request {
    screen,
    state: selected.state(),
    name: c"Demo",
    buffer: Some(&audio),
    source_frames: selected.frames(),
    channels: u32::from(selected.channels()),
    start: 0,
    end: selected.frames(),
    destination: None,
};

// SAFETY: the request uses the live page/state from this UI callback and the
// buffer outlives the call; no other code mutates the program meanwhile.
match unsafe { extract::run(&request) } {
    Ok(extracted) => {
        mpc_sample_sdk::platform::status::set_status(&format!(
            "created pad {}",
            extracted.pad + 1
        ));
    }
    Err(error) => {
        mpc_sample_sdk::platform::status::set_status(&format!("extract: {error}"));
    }
}
}
```

`source_frames` is the source sample length, and `start..end` is the range the
app should copy. The buffer may be longer than the range for an effect tail; the
SDK's writer substitutes it block by block.

The SDK copies the rendered buffer into an owned `Arc`. The writer substitutes
on the command thread and requires the native callback to consume each block
before returning. After the native execute method returns, the SDK checks the
frame count and callback state once; it reports `NotApplied` if they do not show
a complete write. In-flight callbacks retain their own audio reference.

`destination` controls where the new sample lands:

- `None`: the app uses its next free pad (normal Extract behavior).
- `Some(pad)`: the command writes to that pad, overwriting it whether it already
  holds a sample or is empty. The SDK patches and restores the native command’s
  destination around execute.

When `destination` is set, no free pad is required. For `run` on an empty page, provide a populated source sample for the Extract
range; `generated` instead supplies a temporary native seed. The source is only used to drive the command, not to shape the
generated audio.

## Generated audio without a source sample

Use `extract::generated` for a generated buffer on a specific destination pad.
It validates audio geometry, temporarily borrows the SDK's native seed, runs
Extract and restores the previous sample and holder. The destination is
overwritten, so choose a free pad first if existing audio must be preserved.

```rust
use mpc_sample_sdk::platform::extract::{self, Generated};

fn write_generated(screen: usize, audio: &[f32], destination: i32) {
    let request = Generated { screen, name: c"Generated", audio, channels: 2, destination };
    // SAFETY: the caller must supply the live Sample page and control surface
    // on the UI thread, with exclusive access to the page and program.
    let _ = unsafe { extract::generated(&request) };
}
```

The command is constructed synchronously by the firmware handler. If it has
still pending after the handler returns, the seeded call waits for it to finish
before restoring the native layer. A stuck native call or pending command can
therefore block the UI indefinitely. The SDK does not poll for an assignment or
writer callback after a synchronous command has returned. Raw `Request`,
`run`, and `run_seeded` remain available for custom ranges and source behavior.

## Check room first

```rust
use mpc_sample_sdk::platform::{pads, selection};

fn has_room() {
let screen = mpc_sample_sdk::platform::page::screen();
// SAFETY: `screen` is the live Sample page recorded by the paint hook.
let Some(selected) = unsafe { selection::current(screen) } else {
    return;
};
let program = selection::program(selected.state());

// SAFETY: `program` came from the live selection in this UI callback.
if !unsafe { pads::has_room(program, 1) } {
    mpc_sample_sdk::platform::status::set_status("need 1 empty pad");
    return;
}
}
```

`has_room` returns `false` for an unknown program. That is deliberate: the mod
cannot prove there is room.

## Effects and synth takes

- **Effect:** pass the source frame range and a buffer that contains the rendered
  source plus the effect tail. The app extends the last block.
- **Synth:** the take may be longer or shorter than the source. Set `end` from
  the rendered frame count so the result does not inherit the source length.
  Use `destination: Some(selected_pad)` to overwrite the current pad; the source
  sample is only a provider for Extract. On an empty page, use `generated` to
  supply the temporary native seed.
- **HPSS:** run Extract twice, once for each part; check `pads::has_room` for
  two pads before rendering.

`extract::run` invokes the native handler on the UI thread and may block for as
long as that call takes; the SDK cannot interrupt it. With `destination:
Some(_)`, success is detected by the destination pad's sample changing, not by
the free-pad count. Use a worker for rendering and file reads, then commit on
the UI thread; this reduces work before the native call but does not make the
native commit asynchronous.
