# Advanced SDK surface

Most mods never need this page. It covers the remaining callbacks and the
low-level helpers that are still safe to call.

## Control surface

`on_surface` sees the raw control-surface event after the SDK decoded the payload:

```rust
use mpc_sample_sdk::{Mod, SurfaceEvent, Flow};

#[derive(Default)]
struct Surface;

impl Mod for Surface {
    fn on_surface(&mut self, event: SurfaceEvent) -> Flow {
        if event.kind == 0x90 && event.velocity > 0 && (0..16).contains(&event.code) {
            mpc_sample_sdk::platform::status::set_status(&format!(
                "surface pad {}",
                event.code + 1
            ));
            return Flow::Consume;
        }
        Flow::Forward
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

    fn on_pitch_fine(&mut self, _applied: i32) -> i32 {
        // A quarter tone on top: the app has a cents slot next to the semitones.
        50
    }
}
```

`on_label` returns a replacement or `None`; `on_pitch` returns the offset the app
should apply, and `on_pitch_fine` adds cents to it.  The app's pitch parameter is
`Octave`/`Semitones`/`Cents` and its setter writes one property slot per call —
slot 0 the semitones, slot 1 the cents — so the SDK makes a second call only
when `on_pitch_fine` returns a non-zero value.  That is how a mod reaches
sub-semitone pitch: whole semitones alone are the finest the semitone slot can
express.  Cents land on the pad's own note, so `on_pitch_fine` is the place for
any tuning that is not a whole semitone.

## Low-level sites

The displaced app functions are private to the SDK. The few calls a mod may
need live with the concept they act on. Each takes an app object that must still
be live, and they bypass the normal event flow:

```rust
use mpc_sample_sdk::{draw, midi};

// SAFETY: `screen` is the live Sample page recorded by the paint hook.
unsafe { draw::repaint_component(mpc_sample_sdk::platform::page::screen()) };

// Are the note hooks installed?
let ready = midi::notes_ready();

// SAFETY: `input` must be the app's live input object; `0` is a placeholder.
unsafe { midi::send_note_off(0, 0, 4) };
let _ = ready;
```

`event::replay_button` is in [Buttons](02-buttons.md). `draw::repaint_function`
returns the app's own repaint pointer for diagnostics; it should not be stored by
feature code.

The buffer substitution behind [Extract](13-extract.md) is internal to the SDK.
`extract::run` arms and clears it for you, and no feature code can reach it: a
stray arm would replace another writer's blocks.

## Image and hook internals

`image` exposes `base`, `read`, `write`, `read_u32`, `read_u64` and
`read_pointer`. The reads are safe bounds-checked queries, but the offsets are
firmware-specific. `image::write` is `unsafe`: the caller must establish
exclusive access and preserve the target object's invariants. `hook` is the
patch engine; a mod should never need it directly, and installation is the
SDK's `prepare -> seal -> commit` sequence.

## `with_mod`

`loader::with_mod` runs a closure with the installed mod from the UI side:

```rust
let installed = mpc_sample_sdk::loader::with_mod(|module| {
    // `module` is the same object the callbacks use.
    let _ = module;
}).is_some();
```

It never waits for audio: the audio hooks are function pointers copied at
install and do not take the module slot. It also never waits for another UI
callback: if another UI closure owns the slot, it returns `None` so the hook can
forward. `loader::with_mod_audio` no longer exists. `with_mod` is mainly for SDK
diagnostics and tests; from a mod callback, use `self`. See the
[v0.2.0 migration guide](24-safety-migration.md) for the full dispatch policy.
