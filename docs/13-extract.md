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

`destination` controls where the new sample lands:

- `None`: the app uses its next free pad (normal Extract behavior).
- `Some(pad)`: the command writes to that pad, overwriting it whether it already
  holds a sample or is empty. The SDK patches and restores the native command’s
  destination around execute.

When `destination` is set, no free pad is required. For an empty page, provide a
populated source sample for the Extract range; the destination can still be the
empty pad. The source is only used to drive the command, not to shape the
generated audio.

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
  sample is only a provider for Extract. If the current page is empty, seed the
  command with any populated layer sample and keep the empty pad as the
  destination.
- **HPSS:** run Extract twice, once for each part; check `pads::has_room` for
  two pads before rendering.

`extract::run` may block for up to about three seconds while the app assigns the
pad. With `destination: Some(_)`, success is detected by the destination pad’s
sample changing, not by the free-pad count. Do not call it from a paint callback for a large job; use the task pattern
and keep the UI showing progress.
