# Build and install a mod

The SDK ships one build script: [`build-mod.sh`](../build-mod.sh). A mod does
not need its own copy.

## What it needs

- Rust/cargo for the host
- `zig` and the SDK's `scripts/zig-cc-aarch64-linux-gnu` (the script uses the
  copy in the SDK when no linker is configured)
- `nm`, `llvm-objcopy`, `llvm-objdump`
- `python3` and `zip` for the SD package

Add the released SDK to the mod's `Cargo.toml` from crates.io:

```toml
[dependencies]
mpc-sample-sdk = "0.8"
```

Use a Git dependency only when building against an unreleased SDK change.

## Build a `.so`

From the SDK checkout:

```sh
./build-mod.sh \
  --manifest-path /path/to/my-mod/Cargo.toml \
  --package my-mod \
  --name my-mod
```

This writes:

```text
build/my-mod.elf   unstripped ELF (for nm/tests)
build/my-mod.so    stripped, preloadable aarch64 library
build/my-mod.map   address + symbol map
```

The script also checks for unresolved imports and refuses a library that needs a
newer glibc than the MPC Sample's 2.39.

Useful options:

```sh
# SDK-style test symbol checks
--require-symbol g_screen --require-symbol g_pads

# different output
--output-dir /tmp/my-mod-build
--target aarch64-unknown-linux-gnu
```

## Build the SD package

Add `--pack` and the app's first-4KiB SHA-256:

```sh
./build-mod.sh \
  --manifest-path /path/to/my-mod/Cargo.toml \
  --package my-mod \
  --name my-mod \
  --pack \
  --firmware-sha256 44507c7a7d3029c411b2e3780ea286ca3e08b7e3ffd7b5047d52f73bf5299d1f \
  --display-name "My Mod"
```

`--firmware-sha256` is the guard the generated `autorun.sh` checks before it
loads the library. Get it from the firmware build you target; the value above is
the one for MPC Sample 1.3.0.12 used by minimod.

Optional `--env KEY=VALUE` adds a systemd `Environment=` line (safe values only),
for example:

```sh
--env MY_MOD_DEBUG=1
```

The package is `build/my-mod.zip` with this layout:

```text
autorun/
  autorun.json
  autorun.sh
  autorun.check
  my-mod.so
```

## Install

### Virtual lab

```sh
python3 /path/to/virtual-mpc-sample/lab.py preload build/my-mod.so
```

The lab copies the library, adds the `/run` drop-in, and restarts the app. The
mod is unloaded by `lab.py preload --off` or a VM restart.

### MPC Sample over SD

1. Unzip `build/my-mod.zip`.
2. Copy the **`autorun` folder itself** to the **top level** of the SD card —
   the card should contain `autorun/autorun.sh`, not
   `autorun/my-mod/autorun.sh`.
3. Insert the card and power on.
4. The generated `autorun.sh` checks the app's first 4 KiB, copies the `.so` to
   `/run`, installs an `ac50.service` drop-in with `LD_PRELOAD`, and restarts
   the app.

To uninstall, delete `autorun` from the card and reboot. Nothing on the device's
disk is changed.

## minimod

[minimod](https://github.com/ioma8/mpc-minimod) uses this script through its
thin `mod/build.sh` wrapper:

```sh
cd mod
./build.sh      # builds and packs through mpc-sample-sdk/build-mod.sh
./pack.sh       # alias for the same build
```
