# Firmware and the loader

The SDK is built for **MPC Sample 1.3.0.12**. It refuses to install on any other
image instead of patching the wrong offsets.

## The normal path

```rust
use mpc_sample_sdk::{Mod, PadEvent, Flow};

#[derive(Default)]
struct ModType;

impl Mod for ModType {
    fn on_start(&mut self) {
        mpc_sample_sdk::draw::show_toast("firmware verified");
    }

    fn on_pad(&mut self, _event: PadEvent) -> Flow {
        Flow::Forward
    }
}

mpc_sample_sdk::export_mod!(ModType);
```

`export_mod!` emits the constructor. At load time it calls `loader::install`,
which:

1. finds the app's image,
2. checks the firmware instruction fingerprints,
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
checks all 63 fingerprint words. Every hook, guard and called function is in one
table (`src/firmware.rs`); the table also carries the four vtables the SDK
identifies at run time (Sample page, Save Project name screen, text editor and
the lighting sender). A vtable has no instruction word to compare, so it is
checked by comparing the live object's vptr, for example in `page::bind`.

## Checking the table against a binary

`scripts/check-firmware-table.py` reads every fingerprinted word from a firmware
binary at the table's offsets. Run it on each firmware bump, before the hooks are
changed:

```bash
python3 scripts/check-firmware-table.py "usr/bin/MPC Sample"
```

The binary is `usr/bin/MPC Sample` from the rootfs of the official updater's
`Update.img` (1.3.0.12: 119,243,440 bytes). The table's offsets are
image-relative addresses. In the app's first `PT_LOAD` they equal file offsets,
which is where every fingerprinted instruction lives, so the checker can read the
binary on disk directly. The vtables sit in the second `PT_LOAD`, where
`p_offset` and `p_vaddr` differ by `0x10000`; they are only ever reached through
the loaded image (`base + offset`).

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
crate-internal: it only represents an address and does not authenticate firmware
or authorize calls. Use `Firmware::find`, which checks the unnamed image covering
the tested firmware's probe offset.

Hook installation is two phase. `hook::prepare` writes the relay slots and
trampolines into writable, non-executable pages; `hook::commit` seals them
executable, then patches each site and verifies the word. A sealing or patching
failure rolls back every published site and releases the pages, so installation
never reports success while a branch targets non-executable memory.
