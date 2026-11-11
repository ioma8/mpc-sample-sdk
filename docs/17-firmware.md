# Firmware and the loader

The SDK is built for **MPC Sample 1.3.0.12**. It refuses to install on any other
image instead of patching the wrong offsets.

## The normal path

```rust
use mpc_sample_sdk::{Mod, PadEvent, PadFlow};

#[derive(Default)]
struct ModType;

impl Mod for ModType {
    fn on_start(&mut self) {
        mpc_sample_sdk::draw::show_toast("firmware verified");
    }

    fn on_pad(&mut self, _event: PadEvent) -> PadFlow {
        PadFlow::Forward
    }
}

mpc_sample_sdk::export_mod!(ModType);
```

`export_mod!` emits the constructor. At load time it calls `loader::install`,
which:

1. finds the app's image,
2. checks the 37 firmware words,
3. installs the hook sites,
4. calls `on_start`.

If any step fails, the mod stays inert.

## Checking from a diagnostic build

```rust
use mpc_sample_sdk::loader;

let base = loader::find_main();
if base == 0 {
    // The stock app is not loaded.
} else if loader::firmware_ok(base) {
    // Safe to use the image helpers.
} else {
    // Different firmware: do not patch anything.
}
```

`loader::find_main()` returns the load address, or zero. `firmware_ok(base)`
checks all 37 site words.

## Manual installation

`loader::install` is unsafe because it writes the module slot and must run once
from `.init_array`. `export_mod!` is the supported wrapper; most mods never call
`install` directly.



## Image helpers

After installation, `image::base()` is the app's load address. Do not read
firmware memory through raw pointers in feature code; use the typed platform
helpers (`page`, `selection`, `sample`, `pads`, `slice`, `control`, `image`).
The raw `image::read`/`read_u32`/`read_pointer` helpers exist for the SDK and for
advanced diagnostics.

`image::write` is unsafe: a successful kernel write does not establish Rust
aliasing safety or firmware object validity. The caller must establish exclusive
access and preserve the target object’s invariants. `Firmware::from_base` is
unsafe and only represents an address; it does not authenticate firmware or
authorize calls. Use `Firmware::find`, which checks the unnamed image covering
the tested firmware's probe offset.

Hook installation is two phase. `hook::prepare` writes the relay slots and
trampolines into writable, non-executable pages; `hook::commit` seals them
executable, then patches each site and verifies the word. A sealing or patching
failure rolls back every published site and releases the pages, so installation
never reports success while a branch targets non-executable memory.
