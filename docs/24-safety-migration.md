# Migrating to v0.2.0

v0.2.0 corrects firmware-boundary defects found in the post-release review. It
is **API-breaking**: raw firmware operations now demand an explicit `safety`
contract, and several unchanged-looking calls move behind one. Existing
`v0.1.10` consumers stay pinned to that release until they audit the call sites
below.

The SDK is not "safe Rust around a C ABI" in the sense that arbitrary pointers
become trustworthy. It is safe Rust **after** you establish the lifetime, thread
ownership and exclusivity of the app object you pass in. Most feature code needs
no `unsafe`; only the firmware boundary does.

## Changed APIs

| API | Change | Why |
| --- | --- | --- |
| `image::write` | now `unsafe` | a kernel write does not establish Rust aliasing or object validity |
| `Firmware::from_base` | now `unsafe` | a number is not an authenticated firmware image; use `Firmware::find` |
| `pads::free_count`, `pads::free_estimate`, `pads::has_room` | now `unsafe` | each calls the app's own predicate with a caller-supplied program |
| `selection::current` | now `unsafe` | calls the page predicate with a caller-supplied screen |
| `Selection` fields | private; use `screen()`, `state()`, `sample()`, `frames()`, `channels()`, `rate()` | only `current` or the unsafe `from_parts` can construct a validated selection |
| `Selection::from_parts` | new `unsafe` constructor | explicit synthetic source for synth actions that have no selected sample |
| `pads::set_layer_active` | now `unsafe` | writes a live native layer and requires exclusive access |
| `slice::write_markers`, `slice::set_manual` | now `unsafe` | drive the app's native Reset/Insert and binding callbacks |
| `sites::repaint_component`, `sites::replay_button`, `sites::send_note_on`, `sites::send_note_off` | now `unsafe` | take a component, event or input object that must still be live |
| `Graphics::colour`, `Graphics::rect`, `Graphics::text` | now `unsafe` | the handle must be the live app Graphics on the UI thread |
| `draw::start_repaint_pump` | now `unsafe` | the supplied function must return a live component on every tick |
| `draw::toast_text` | returns `Vec<u8>` | the old `&'static [u8]` aliased storage a later toast overwrites |
| `page::set_screen` / `draw::set_screen` | removed from the public API | a mod could point the repaint at an arbitrary component |
| `NoteEvent` | now `NoteEvent<'_>`; `event` is private | the event is borrowed, exclusively, for the callback |
| `Mod` | now requires `Send` | the SDK stores one module behind an exclusive dispatch slot |
| `loader::with_mod` / `with_mod_audio` | `with_mod` plus `Mod::audio` hooks | audio never takes the module slot; UI never waits for audio and forwards if another UI owns it; reentry forwards |
| `paths::for_sample` | no longer scans temporary projects | a geometry match does not prove recording identity |
| `paths::for_recording` | new explicit recording context | searches only the recording's own project directory |
| `hook::install` / `hook::seal` | renamed to `hook::prepare` / `hook::commit` | installation is prepare → seal → publish, with rollback |

`extract::run`, `control::apply`, `control::set`, `page::visible`,
`page::chop_mode` and `draw::toast_line` were already `unsafe`; their contracts
are unchanged.

## v0.3.0: audio hooks

`Mod::audio` now returns the audio hooks as function pointers, copied once at
install.  Audio callbacks never take the module lock; a slow UI callback can no
longer drop an audio block.  Replace `on_output`/`on_input_*` with the matching
`AudioHooks` fields; the functions are free functions over atomics, not methods
on `&mut self`.

## Dispatch and reentrancy

Audio and UI no longer share one module slot:

* **Audio** calls the function pointers from `Mod::audio`, copied once at
  install. It never takes the module lock, so a slow UI callback cannot drop an
  audio block.
* **UI** takes the slot without waiting. Audio never owns it, so there is
  nothing to wait for.
* **UI never waits for another UI callback.** A native operation called from
  `on_paint` may itself be waiting for that callback; the second one is
  forwarded instead, which avoids a lock-inversion deadlock.
* **Reentry on the same thread** (for example a draw callback reached through
  the hooked string constructor) observes `None` and forwards.

No two callbacks ever hold `&mut self` at the same time, so a mod's plain fields
are sufficient inside a callback:

```rust
fn on_pad(&mut self, event: PadEvent) -> PadFlow {
    self.last_pad = event.pad; // exclusive access, no atomics required
    PadFlow::Forward
}
```

Do not block inside an audio callback: it must return to the device. Publish
results to the UI through atomics or a synchronized queue, as before. The SDK
calls the `AudioHooks` function pointers from the audio taps and `with_mod` from
UI hooks, so a mod never sees the dispatch.

The input and output audio taps are serialised independently, so a blocking
output `writei` does not suppress the input callback.

## Raw pointer audit

Before adding an `unsafe` block, answer all three questions in a comment:

1. **Lifetime** - is the object live for the whole call, including any deferred
   repaint or callback it can trigger?
2. **Thread** - is this its owning UI thread (or the audio thread for audio
   data)?
3. **Exclusivity** - is there another `&mut` to the same object, or a second
   writer that could run concurrently?

```rust
// SAFETY: `surface` is the live surface from this UI callback, this thread
// owns it, and no other code mutates the layer during the write.
let activated = unsafe { pads::set_layer_active(surface, pad, true) };
```

Prefer the validated handles where they exist: `Firmware::find()`, `Selection`
itself, `sample::Memory`, `wav::Info`, `slice::Slices`, and `render::RenderJob`.
`selection::current` is the unsafe constructor that builds a `Selection`; pass it
the screen the paint hook recorded. Do not reconstruct handles from bare integers
unless the integer came from the same callback's app object.

## Extract and substituted audio

The substitution plan is bound to the operation that armed it, the thread that
runs that operation and the first writer closure on that thread. Another thread
cannot read, advance or clear it, and another writer closure is forwarded. The
native writer hook does not expose the owning command, so an unrelated writer
callback on the same executor thread *before* the command's own first callback
cannot be distinguished; the plan is armed immediately before execute to keep
that window as small as the firmware allows.

`extract::run` keeps the rendered buffer alive until the writer reports every
frame and no callback remains in flight; a timeout reports `NotApplied` and keeps
the operation published rather than freeing audio the executor may still use.

## Hook installation

`hook::prepare` writes relay slots and trampolines into writable,
non-executable pages. `hook::commit` seals them executable and only then patches
the sites, verifying each word and rolling every already-published site back if
one fails. A sealing failure leaves the app unmodified: installation no longer
reports success while live branches target non-executable memory.

## Path resolution

`paths::for_sample` uses only the sample's own object graph. A recording that
has no stored path needs its project directory:

```rust
let project = std::path::Path::new("/data/TempProjects/Temp-Project_[ProjectData]");
let path = paths::for_recording(selected.sample(), selected.frames(), selected.channels(),
                                selected.rate(), project);
```

`paths::unique` finds the single existing file with exactly the requested
geometry in one directory. It does not create a path or a file.

## Verification expectations

The host tests exercise dispatch, writer ownership, replay restoration, slice
pre-validation, path scoping, trim bounds, WAV float64 handling and the ALSA
channel-setter argument order. They cannot establish firmware ABI correctness or
runtime safety. On the device, re-run the Extract, chop, toast and audio checks
in [Autonomous development](23-autonomous-development.md), including a delayed
writer callback and a deliberately failing seal.
