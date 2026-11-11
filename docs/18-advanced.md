# Advanced SDK surface

Most mods never need this page. It covers the remaining callbacks and the
low-level helpers that are still safe to call.

## Control surface

`on_surface` sees the raw control-surface event after the SDK decoded the payload:

```rust
use mpc_sample_sdk::{Mod, SurfaceEvent, SurfaceFlow};

#[derive(Default)]
struct Surface;

impl Mod for Surface {
    fn on_surface(&mut self, event: SurfaceEvent) -> SurfaceFlow {
        if event.kind == 0x90 && event.velocity > 0 && (0..16).contains(&event.code) {
            mpc_sample_sdk::platform::status::set_status(&format!(
                "surface pad {}",
                event.code + 1
            ));
            return SurfaceFlow::Consume;
        }
        SurfaceFlow::Forward
    }
}
```

A physical pad can arrive on both paths; use `on_pad` for ordinary pad handling
and `on_surface` only when you need the raw note/velocity.

## Labels and 16 Levels

```rust
use core::ffi::CStr;
use mpc_sample_sdk::Mod;

#[derive(Default)]
struct Labels;

impl Mod for Labels {
    fn on_label(&mut self, label: &CStr) -> Option<&'static CStr> {
        if label.to_bytes() == b"HPSS".as_slice() {
            Some(c"MY SPLIT")
        } else {
            None
        }
    }

    fn on_pitch(&mut self, applied: i32) -> i32 {
        // Add an octave to the 16 Levels pitch offset.
        applied + 12
    }
}
```

`on_label` returns a replacement or `None`; `on_pitch` returns the offset the app
should apply.

## Low-level sites

`sites` wraps the displaced app functions. They are unsafe because each takes an
app object that must still be live, and they bypass the normal event flow:

```rust
use mpc_sample_sdk::sites;

// SAFETY: `screen` is the live Sample page recorded by the paint hook.
unsafe { sites::repaint_component(mpc_sample_sdk::platform::page::screen()) };

// Are the note hooks installed?
let ready = sites::notes_ready();

// SAFETY: `input` must be the app's live input object; `0` is a placeholder.
unsafe { sites::send_note_off(0, 0, 4) };
let _ = ready;
```

`replay_button` is in [Buttons](02-buttons.md). `repaint_function` and the
`*_original` accessors are for diagnostics; they expose the app's own function
pointers and should not be stored by feature code.

`platform::writer` is the buffer substitution behind [Extract](13-extract.md).
The SDK arms and clears it for you. Do not call `writer::arm` or
`writer::clear` from feature code unless you are implementing a new extraction
command; the hooks are process-wide and a stray arm would replace another
writer's blocks.

## Image and hook internals

`image` exposes `base`, `read`, `write`, `read_u32`, `read_u64` and
`read_pointer`. They are safe bounds-checked reads/writes, but the offsets are
firmware-specific. `hook` is the patch engine; a mod should never need it
directly.

## `with_mod`

`loader::with_mod` runs a closure with the installed mod:

```rust
let installed = mpc_sample_sdk::loader::with_mod(|module| {
    // `module` is the same object the callbacks use.
    let _ = module;
}).is_some();
```

This is mainly for SDK diagnostics and tests. From a mod callback, use `self`.
