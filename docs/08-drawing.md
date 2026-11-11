# Drawing

The app paints first; `on_paint` is your chance to draw on top. You receive a
small `Graphics` handle that wraps the app's own `setColour`, `fillRect`,
`Font` and `drawText`.

```rust
pub struct Graphics { /* private */ }

impl Graphics {
    pub unsafe fn colour(self, value: u32);
    pub unsafe fn rect(self, x: i32, y: i32, width: i32, height: i32);
    pub unsafe fn text(self, line: &str, x: i32, y: i32, width: i32, height: i32, size: f32);
}
```

The screen is **320 × 240**. Colours are `0xAARRGGBB`, for example
`0xffff_cc00` is opaque amber.

## Example: a small panel

```rust
use mpc_sample_sdk::{draw::Graphics, Mod, PaintEvent};

#[derive(Default)]
struct Draw;

impl Mod for Draw {
    fn on_paint(&mut self, graphics: &mut Graphics, event: PaintEvent) {
        if !event.sample_page {
            return;
        }
        // SAFETY: this is the live Graphics for the component the app is
        // painting on the UI thread.
        unsafe {
            graphics.colour(0xff10_1014);
            graphics.rect(8, 8, 180, 44);
            graphics.colour(0xffff_cc00);
            graphics.text("MY MOD", 16, 12, 160, 16, 13.0);
            graphics.colour(0xffff_ffff);
            graphics.text("ready", 16, 30, 160, 14, 11.0);
        }
    }
}

mpc_sample_sdk::export_mod!(Draw);
```

Drawing is immediate: every call reaches the app's graphics object during the
paint. Do not keep a `Graphics` value after `on_paint` returns.

## Toasts

A toast is one retained line the SDK draws for about a second:

```rust
use mpc_sample_sdk::{ButtonEvent, ButtonFlow, Mod};

#[derive(Default)]
struct Toast;

impl Mod for Toast {
    fn on_button(&mut self, event: ButtonEvent) -> ButtonFlow {
        if event.code == 0x6f && event.pressed {
            mpc_sample_sdk::draw::show_toast("confirmed");
            mpc_sample_sdk::draw::repaint_painted();
        }
        ButtonFlow::Forward
    }
}
```

`show_toast` stores the text; `repaint_painted()` asks the app for the frame that
shows it. From inside `on_paint` the repaint is unnecessary.

## Repaint pump

For a live display (meters, clocks, progress), ask for a repaint regularly:

```rust
use mpc_sample_sdk::{draw, Mod};

#[derive(Default)]
struct Live;

impl Mod for Live {
    fn on_start(&mut self) {
        // SAFETY: `painted` is the component the paint hook last observed, and
        // it returns zero instead of a stale value when nothing was painted.
        unsafe { draw::start_repaint_pump(draw::painted) };
    }
}
```

`start_repaint_pump` is idempotent and runs for the process. Pass a function that
returns the component to repaint, or `0` to skip a tick. `draw::painted()` is the
last painted component.

If you start a worker, call `draw::repaint_painted()` when it finishes so an
idle UI notices the result.
