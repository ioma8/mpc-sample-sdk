# Firmware and the loader

The SDK is built for **MPC Sample 1.3.0.12** and **1.4.0.8**. It carries one
address table per build, compiles against exactly one, and refuses to install on
any other image instead of patching the wrong offsets.

| Cargo feature | stock build |
|---|---|
| `fw-1-3` (default) | MPC Sample 1.3.0.12 (`40e46ca7991`) |
| `fw-1-4` | MPC Sample 1.4.0.8 (`d87ac8c12e9`) |

Both tables use the same names, so a mod selects the firmware once, in its
manifest, and never branches in code:

```toml
mpc-sample-sdk = { version = "0.9", default-features = false, features = ["fw-1-4"] }
```

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
checks all 63 fingerprint words of the selected profile. Every hook, guard and called function is in one
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

Without `--fw` the script tries both profiles and reports the one that matches;
`--fw 1.3` or `--fw 1.4` checks a single table.

The binary is `usr/bin/MPC Sample` from the rootfs of the official updater's
`Update.img` (1.3.0.12: 119,243,440 bytes, 1.4.0.8: 128,398,464 bytes). The table's offsets are
image-relative addresses. In the app's first `PT_LOAD` they equal file offsets,
which is where every fingerprinted instruction lives, so the checker can read the
binary on disk directly. The vtables sit in the second `PT_LOAD`, where
`p_offset` and `p_vaddr` differ by `0x10000`; they are only ever reached through
the loaded image (`base + offset`).

## 1.4.0.8 layout status

The 1.4 build moved three app objects and left the slice, selection and layer
internals byte-identical. Both layouts ship, selected by the same feature:

| object field | 1.3.0.12 | 1.4.0.8 |
|---|---|---|
| screen binding | `0x178` | `0x1b8` |
| screen selection / chop binding | `0x180` / `0x168` | `0x1c0` / `0x1a8` |
| chop mode | view `0x1ee0` | controller at `screen+0x158`, flag `+0x53` bit 0 |
| state layers / state program | `0x22e8` / `0x2310` | `0x23a8` / `0x23d0` |
| pad tables (program and surface) | `0x2808` / `0x2810` | `0x28b0` / `0x28b8`, probed at runtime |
| encoder data target | `0x2da8` | `0x2da8` (verified against live button and encoder events) |
| binding state, selection, slice, layer and sample fields | unchanged | unchanged |

The DATA encoder is an embedded object at `+0x2da8` relative to a button
event's surface; it is not a pointer stored at that offset. Pad pressure
events carry a different surface object. Pad tables themselves live on program
objects: `DrumProgram`/`AuxProgram` carry 128 `Instrument` pointers at
`+0x28b0` in 1.4 (`+0x2808` in 1.3), so a surface that is not a program yields
no table rather than a wrong one.

The other values were measured from the app's own instructions by aligning the
functions that use them, then checked live: the 1.4.0.8 app runs in a QEMU lab
built from this update image, and a probe mod walks the live object graph
(`SampleModeScreen -> InstrumentConnector -> Instrument -> InstrumentLayer ->
Sample -> metadata`). The method and evidence are in
[VALIDATION-1.4.0.8.md](../VALIDATION-1.4.0.8.md). The test suite runs against
both profiles.

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
