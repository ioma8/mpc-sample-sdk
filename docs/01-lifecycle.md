# Lifecycle: a minimal mod

A mod is one type that implements [`Mod`]. Every callback has a default, so a
minimal mod only overrides the one it needs. `Mod` requires `Send`: the SDK
stores the module behind one process-wide slot and calls it from the UI or audio
thread, never both at once. `export_mod!` gives the type a constructor in
`.init_array`; `install` checks the firmware, patches the hooks, and then calls
`on_start`.

## The smallest useful mod

```rust
use mpc_sample_sdk::{Mod, PadEvent, PadFlow};

#[derive(Default)]
struct Hello;

impl Mod for Hello {
    fn on_start(&mut self) {
        mpc_sample_sdk::draw::show_toast("Hello from the SDK");
    }

    fn on_pad(&mut self, _event: PadEvent) -> PadFlow {
        // Forward means "the app still gets this event".
        PadFlow::Forward
    }
}

mpc_sample_sdk::export_mod!(Hello);
```

## What the SDK does for you

- Finds `/usr/bin/MPC Sample` and checks the 37-word firmware fingerprint.
- Patches the 17 hook sites; the displaced originals stay private.
- Decodes every event into the types in `event.rs`.
- Forwards the event unless you return `Consume`.
- Calls `on_start` only after installation succeeded.

If the firmware is wrong, `install` returns `InstallError` and the mod does
nothing. `export_mod!` ignores that error; use `loader::install` directly only
if you need to report it.

## Overriding several callbacks

```rust
use mpc_sample_sdk::{ButtonEvent, ButtonFlow, Mod, PadEvent, PadFlow};

#[derive(Default)]
struct Demo;

impl Mod for Demo {
    fn on_button(&mut self, event: ButtonEvent) -> ButtonFlow {
        if event.code == 0x6f && event.pressed {
            mpc_sample_sdk::draw::show_toast("DATA pressed");
        }
        ButtonFlow::Forward
    }

    fn on_pad(&mut self, event: PadEvent) -> PadFlow {
        if event.pad == 4 {
            mpc_sample_sdk::draw::show_toast("pad 5");
            return PadFlow::Consume;
        }
        PadFlow::Forward
    }
}

mpc_sample_sdk::export_mod!(Demo);
```

## Build

```toml
[lib]
crate-type = ["cdylib"]
```

Build with the SDK script (it finds the SDK's cross linker, strips the library
and checks it):

```bash
/path/to/mpc-sample-sdk/build-mod.sh \
  --manifest-path Cargo.toml --package demo --name demo
python3 /path/to/virtual-mpc-sample/lab.py preload build/demo.so
```

Add `--pack --firmware-sha256 <hash>` to produce an SD zip; see
[Build and install](22-build-and-install.md).

The installed mod lives for the process. Keep state in `self` or in
`state::Global`, not in a stack temporary that a callback cannot reach.
