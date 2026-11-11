# A complete example mod

This mod opens a small panel with **SHIFT + DATA**, consumes pad 1 while it is
open, prints the selected sample's geometry, and closes with the same chord.

```rust
use mpc_sample_sdk::platform::{page, selection, status};
use mpc_sample_sdk::{
    draw::{self, Graphics},
    ButtonEvent, ButtonFlow, Mod, PadAction, PadEvent, PadFlow, PaintEvent,
};

const KEY_SHIFT: u32 = 0x31;
const KEY_DATA: u32 = 0x6f;

#[derive(Default)]
struct MyMod {
    open: bool,
}

impl Mod for MyMod {
    fn on_start(&mut self) {
        status::set_status("MyMod ready");
    }

    fn on_button(&mut self, event: ButtonEvent) -> ButtonFlow {
        if event.code == KEY_SHIFT {
            return ButtonFlow::Forward;
        }
        if event.code == KEY_DATA && event.pressed && event.shift {
            self.open = !self.open;
            if self.open {
                status::set_status("MyMod open");
            } else {
                status::set_status("MyMod closed");
            }
            draw::repaint_painted();
            return ButtonFlow::Consume;
        }
        ButtonFlow::Forward
    }

    fn on_pad(&mut self, event: PadEvent) -> PadFlow {
        if !self.open || event.pad != 0 || event.action != PadAction::Press {
            return PadFlow::Forward;
        }
        let screen = page::screen();
        // SAFETY: `screen` is the live Sample page recorded by the paint hook.
        if let Some(selected) = unsafe { selection::current(screen) } {
            let message = format!(
                "{} frames, {} ch, {} Hz",
                selected.frames(), selected.channels(), selected.rate()
            );
            draw::show_toast(&message);
            draw::repaint_painted();
        } else {
            draw::show_toast("no sample");
            draw::repaint_painted();
        }
        PadFlow::Consume
    }

    fn on_paint(&mut self, graphics: &mut Graphics, event: PaintEvent) {
        if !event.sample_page || !self.open {
            return;
        }
        // SAFETY: the app is painting this live Graphics on the UI thread.
        unsafe {
            graphics.colour(0xff10_1014);
            graphics.rect(8, 8, 180, 54);
            graphics.colour(0xffff_cc00);
            graphics.text("MyMod", 16, 12, 160, 16, 13.0);
            graphics.colour(0xffff_ffff);
            graphics.text("pad 1: inspect", 16, 30, 160, 14, 11.0);
        }
    }
}

mpc_sample_sdk::export_mod!(MyMod);
```

## What each part shows

- `on_button` owns a chord and returns `Consume`.
- `on_pad` scopes itself to the mod's own panel and consumes pad 1.
- `unsafe { selection::current(screen) }` gets the typed source; the screen
  comes from the paint hook, so no raw pointer is guessed.
- `draw::show_toast` reports a result; `repaint_painted()` makes it visible.
- `on_paint` draws after the app, so it composes with the stock screen.

## Next steps

- Replace the toast with a `Task` + `RenderJob` from [Tasks](12-tasks.md).
- Write the result through [Extract](13-extract.md).
- Add real values on the panel from [Knobs](05-knobs.md) or
  [Audio](11-audio.md).
- Use [State](15-state.md) for UI-thread state; use atomics or a synchronized
  queue for flags shared with the audio thread.
