# Screens and page detection

The app is one window with several pages. The SDK tracks the component that is
currently painting and gives you the same checks the app uses.

## The basics

```rust
use mpc_sample_sdk::platform::page;

let screen = page::screen();          // the current component
let is_sample = page::is_sample_page(screen);
// SAFETY: `screen` is the live page recorded by the paint hook on this thread.
let visible = unsafe { page::visible(screen) };
// SAFETY: as above.
let chopping = unsafe { page::chop_mode(screen) };
```

`page::screen()` is updated by the paint hook. It is zero before the first paint.
Use `visible` before reading selection data: it checks the vtable **and** the
app's “this page has a sample” predicate.

## Example: paint only on the Sample page

```rust
use mpc_sample_sdk::{draw::Graphics, Mod, PaintEvent};

#[derive(Default)]
struct ScreenAware;

impl Mod for ScreenAware {
    fn on_paint(&mut self, graphics: &mut Graphics, event: PaintEvent) {
        if !event.sample_page {
            return;
        }
        if event.chop_mode {
            // SAFETY: the app is painting this live component on the UI thread.
            unsafe {
                graphics.colour(0xffff_cc00);
                graphics.rect(4, 4, 120, 18);
                graphics.colour(0xff00_0000);
                graphics.text("CHOP", 10, 6, 100, 14, 12.0);
            }
        }
    }
}

mpc_sample_sdk::export_mod!(ScreenAware);
```

`PaintEvent.sample_page` is the cheap check inside the paint callback.
`unsafe { page::visible(page::screen()) }` is the equivalent outside it, with
the same live-page contract.

## Example: react to a page change

The SDK does not send a “page changed” event. Compare the last value in your
paint callback:

```rust
use mpc_sample_sdk::{draw::Graphics, Mod, PaintEvent};

#[derive(Default)]
struct PageChange {
    last: usize,
}

impl Mod for PageChange {
    fn on_paint(&mut self, _graphics: &mut Graphics, event: PaintEvent) {
        if event.sample_page && event.component != self.last {
            self.last = event.component;
            mpc_sample_sdk::draw::show_toast("Sample page");
        }
    }
}
```

## Screen-related constants

```rust
use mpc_sample_sdk::platform::page;

assert_eq!(page::SAMPLE_PAGE_VTABLE, 0x6e2_12a0);
let screen = page::screen();
if page::is_sample_page(screen) {
    mpc_sample_sdk::platform::status::set_status("on the Sample page");
}
```

The vtable constant is firmware-specific. If you support another firmware
version, verify it before using the page helpers.
