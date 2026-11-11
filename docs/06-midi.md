# MIDI notes

MIDI arrives in `on_note` as a `&mut NoteEvent<'_>`. The SDK decodes the event,
forwards it, and restores any rewrite after the callback.

```rust
pub struct NoteEvent<'event> {
    pub note: u8,
    pub velocity: f32,
    pub external: bool,
    pub note_on: bool,
    pub note_off: bool,
    pub row: i32,
    pub context: usize,
    pub app: usize,
    pub source: u64,
    pub kind: u8,
    pub mask: u16,
    pub property: u8,
    // private: the app's event, borrowed exclusively for this callback
    event: &'event mut [u8],
}
```

The event storage is private and tied to the callback's borrow, so a mod cannot
keep the event or rewrite it out of band. The fields are read-only for normal
use; call `replay_as_pad` when you want the app to hear a different pad.

## Example: transpose external notes into pad 5

```rust
use mpc_sample_sdk::{Mod, NoteEvent, NoteFlow};

#[derive(Default)]
struct Midi;

impl Mod for Midi {
    fn on_note(&mut self, event: &mut NoteEvent<'_>) -> NoteFlow {
        if !event.note_on || !event.external {
            return NoteFlow::Forward;
        }
        let row = event.row;
        let context = event.context;
        if event.replay_as_pad(4, row, context) {
            NoteFlow::Replayed
        } else {
            NoteFlow::Forward
        }
    }
}

mpc_sample_sdk::export_mod!(Midi);
```

`Replayed` tells the SDK to forward the rewritten event and then restore it.
`Forward` sends the original event. `Consume` swallows it.

## Example: only notes in a range

```rust
use mpc_sample_sdk::{Mod, NoteEvent, NoteFlow};

#[derive(Default)]
struct LowNotes;

impl Mod for LowNotes {
    fn on_note(&mut self, event: &mut NoteEvent<'_>) -> NoteFlow {
        if event.note_on && (48..=60).contains(&event.note) {
            mpc_sample_sdk::platform::status::set_status("low note");
        }
        NoteFlow::Forward
    }
}
```

## Replaying a pad with the low-level decoder

For a custom MIDI path, the SDK exports `midi::decode` and the raw note event
helpers. Most mods should use `on_note` and `replay_as_pad` instead.
