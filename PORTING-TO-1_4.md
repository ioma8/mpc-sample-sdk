# Porting the SDK to MPC Sample 1.4.0.8 — handoff

This document is the handoff for the 1.4 port: what exists, what was measured,
what is still open, how to reproduce it, and which approaches were dead ends.
It assumes the reader can use a terminal and has the firmware update.

**Status:** the SDK builds and installs on the stock 1.4.0.8 app in the QEMU lab. Extract, generated audio, CHOP marker replacement, DATA encoder identity, and pad-pressure event delivery have been exercised live. Both firmware profiles pass their host checks. No physical-device run has been made.

Everything below was either measured on this machine against the shipped
1.4.0.8 image or is explicitly marked **not established**. Do not treat the
unmarked items as verified.

---

## 0. Quick orientation

| path | what it is |
|---|---|
| `src/firmware.rs` | the shared `Entry`/`Purpose`/`fingerprints()` machinery and the profile switch |
| `src/firmware/v1_3.rs` | 1.3.0.12 table (63 fingerprinted words + 4 data entries) |
| `src/firmware/v1_4.rs` | 1.4.0.8 table (63 fingerprinted words + 4 data entries) |
| `scripts/check-firmware-table.py` | verifies a table against a binary; auto-detects the build or `--fw` |
| `VALIDATION-1.4.0.8.md` | the port report: inputs, table derivation, layout evidence, live checks, negative results |
| `APP-EXTRACTION.md` | how to get `usr/bin/MPC Sample` out of an official updater |
| `docs/17-firmware.md` | user-facing firmware/loader documentation |
| `HYPOTHESES.csv` | the hypothesis ledger from the Extract investigation (see §11) |
| `$LAB` = a `virtual-mpc-sample` checkout | the QEMU lab: the stock rootfs running unmodified, panel in the browser |

The evidence and historical investigation are recorded in this document and the validation report.

---

## 1. What the port is

The SDK is a Rust `cdylib` loaded into the stock app with `LD_PRELOAD`. It
patches 22 hook sites, uses 25 call/data entries (21 functions plus 4 vtables),
and draws on the app's objects. Every address and every object offset is build-specific:

* the SDK carries **one address table per stock build**;
* exactly one profile is compiled, selected by a Cargo feature
  (`fw-1-3` default, `fw-1-4`);
* both profiles export the same constant names, so no module branches on the
  firmware in code;
* the loader fingerprints the image and refuses to install on anything else.

The 1.4 port therefore has two halves, both required:

1. **Addresses** — every hook/call/guard/data site, recovered from the binary.
2. **Layouts** — every object offset the SDK reads, recovered by comparing the
   matched function bodies and by live measurement.

Both halves are done for 1.4.0.8. The Extract logic is the one place where the
*semantics* (not the addresses) still needs work.

---

## 2. Getting the firmware and the app

### 2.1 The two builds this SDK targets

| | 1.3.0.12 | 1.4.0.8 |
|---|---|---|
| updater | `MPC Sample 1.3.0 Updater.exe` (PE32 7-Zip SFX) | `MPC Sample 1.4.0 Updater.app.zip` (macOS app) |
| image inside | `update.img` (169,207,872 B) | `MPC Sample 1.4.0 Updater.app/Contents/Resources/Update.img` (242,687,968 B) |
| image snapshot | `SNAPSHOT-20260402131256` | `SNAPSHOT-20261123112003` |
| app | `usr/bin/MPC Sample`, 119,243,440 B | `usr/bin/MPC Sample`, 128,398,464 B |
| app version | `1.3.0.12 (40e46ca7991)` | `1.4.0.8 (d87ac8c12e9)` |
| app BuildID | `1dac7f481a0fe8d128e435ace88d7e6aece0c9c9` | `1b51367882445eaaedc154aa43ff25caf3b9bd91` |
| app SHA-256 | `b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508` | `473fb7260e597e61cc55d9d30595eb9d9aa1304ff1b88960e633ad19457dc91a` |
| app first 4 KiB | `44507c7a7d3029c411b2e3780ea286ca3e08b7e3ffd7b5047d52f73bf5299d1f` | `2b1ce171e951c90bf08cf375d7db9065f1782745b07a436bb2d22fdaabba0af0` |

The first-4 KiB hash is the `--firmware-sha256` value `build-mod.sh --pack`
puts into an autorun package.

### 2.2 1.3: unpack the installer (for reference)

The full walkthrough with the SFX offset, the `AZ0x` header fields and the
record table is [APP-EXTRACTION.md](APP-EXTRACTION.md). In short:

```sh
7z x -y -ompc-installer "MPC Sample 1.3.0 Updater.exe" Config.json update.img
```

### 2.3 1.4: unpack the macOS updater

The 1.4 updater is a zipped macOS app bundle. Only one file is needed:

```sh
unzip "MPC Sample 1.4.0 Updater.app.zip" \
  "MPC Sample 1.4.0 Updater.app/Contents/Resources/Update.img"
cd fw14
cp "/…/MPC Sample 1.4.0 Updater.app/Contents/Resources/Update.img" update.img
```

`Update.img` is the same `AZ0x` container as 1.3: fixed header to `0x90`, four
64-byte records (`BOOT`, `PARR`, `PART`, `PART`), a 264-byte signature block,
then the payloads. The records are **positional** — they carry offset, size and
SHA-256, not partition names.

1.4 record layout (the script prints these; the values are what this port used):

| record | payload offset | decoded |
|---|---|---|
| `BOOT` | `0x298` | raw DTB (`bootupd for ac50`) |
| `PARR` | `0x1AF670` | XZ → ext2, `kernel.fit` (April kernel, recovery) |
| `PART` | `0xD89B00` | XZ → ext2, `kernel.fit` (December kernel, boot) |
| `PART` | `0x18B0538` | XZ → ext4, rootfs |

The record sizes and SHA-256 values are read from the records themselves; do
not hardcode them from the 1.3 image (the two images have different payload
sizes). `extract.py` prints each size and verifies each hash before writing.

The generic `extract.py` from APP-EXTRACTION.md works unchanged: it reads the
record count at `0x26`, walks the records at `0x90`, slices each payload and
refuses a SHA-256 mismatch. Save it next to `update.img` and run it:

```sh
cd fw14
python3 extract.py
# MPC Sample image | device AC50 | SNAPSHOT-20261123112003 | 4 records
# slots: recovery, boot, rootfs, akai-1
# Sizes are read from the records; do not copy them from the 1.3 image.
# boot-298.bin      <size> B  sha256 ok
# parr-1af670.bin   <size> B  sha256 ok
# part-d89b00.bin   <size> B  sha256 ok
# part-18b0538.bin  <size> B  sha256 ok
```

Decode the XZ kernels and the rootfs, then take the app out:

```sh
xz -dc part-1af670.bin > parr-1af670.raw     # ext2, kernel.fit
xz -dc part-d89b00.bin > part-d89b00.raw     # ext2, kernel.fit
xz -dc part-18b0538.bin > part-18b0538.raw   # ext4 rootfs
7z x -y -orootfs part-18b0538.raw "usr/bin/MPC Sample"
```

On Linux, `debugfs` gives exact modes/ownership/symlinks; `7z` is enough for
reading files. `7z` may warn `There are data after the end of archive` on the
ext4 image — that is its parser reporting unused tail blocks, not corruption.

Verify the app and the table:

```sh
shasum -a 256 "rootfs/usr/bin/MPC Sample" | cut -c1-64
# 473fb7260e597e61cc55d9d30595eb9d9aa1304ff1b88960e633ad19457dc91a
head -c 4096 "rootfs/usr/bin/MPC Sample" | shasum -a 256 | cut -c1-64
# 2b1ce171e951c90bf08cf375d7db9065f1782745b07a436bb2d22fdaabba0af0
python3 scripts/check-firmware-table.py "rootfs/usr/bin/MPC Sample"
# ok: firmware 1.4 (63 entries match)
# firmware 1.3: MISMATCH … (63 of 63 differ)
```

A one-word edit to a copy must fail with exit 1 — use it as the negative
control whenever you touch the checker.

### 2.4 What the image contains (1.4)

* The rootfs is Yocto scarthgap (`az0x` base distribution), systemd, dm-verity
  protected. The app lives at `/usr/bin/MPC Sample`; the factory content is a
  separate partition (`/content`).
* The 1.4 kernel command line (including the dm-verity table and root hash) is
  carried in the kernel FIT, not in the image header, so anything that boots
  the 1.4 rootfs must regenerate that command line. The lab's
  `scripts/verity_cmdline.py` does it from the FIT; the build printed
  `rootfs sha256 source: c24727870ed2c817` for the 1.4 rootfs used here.
* The app is a stripped ELF64 aarch64 PIE. Its first `PT_LOAD` has file offset
  == virtual address, which is why the table offsets are image-relative and
  equal file offsets. The vtables live in the second `PT_LOAD`, where
  `p_offset` and `p_vaddr` differ by `0x10000` — see `docs/17-firmware.md`.

---

## 3. What changed between 1.3.0.12 and 1.4.0.8

### 3.1 Addresses

Every site moved; there is no single delta. The table was rebuilt from scratch.
Selected sites (full list in `src/firmware/v1_4.rs`):

| kind | site | 1.3.0.12 | 1.4.0.8 |
|---|---|---|---|
| hook | `NOTE_ON` | `0x01e27360` | `0x0248d4d0` |
| hook | `NOTE_OFF` | `0x01e270d0` | `0x0248d240` |
| hook | `CC` (mid-function `+0x10`) | `0x0159f230` | `0x01c34360` |
| hook | `EXTRACT_CTOR` | `0x0143ed40` | `0x01a44220` |
| hook | `EXTRACT_EXEC` | `0x0143f0b0` | `0x01a44590` |
| call | `EXTRACT_HANDLER` | `0x016580d0` | `0x01ccadb0` |
| call | `BUILD_SAMPLE` | `0x01df4ac0` | `0x01e807d0` |
| call | `FREE_PAD` | `0x0286dd00` | `0x02d7cbe0` |
| call | `PAD_BUSY` | `0x028047b0` | `0x02d16c10` |
| call | `SELECT_SLICE` | `0x016680a0` | `0x01cddf70` |
| data | `SAMPLE_PAGE_VTABLE` | `0x06e212a0` | `0x076f6170` |
| data | `PROJECT_SAVE_SCREEN_VTABLE` | `0x06e1faa0` | `0x076f4998` |
| data | `TEXT_EDITOR_VTABLE` | `0x06e18e20` | `0x076e0880` |
| data | `LIGHTING_SENDER_VTABLE` | `0x06d8fa60` | `0x076247a0` |
| probe | `APP_PROBE_OFFSET` | `0x00b55b00` | `0x00bca5c0` |

`CC` is deliberately hooked at `+0x10` into the function (first word
`0x39402003`), not at its entry — the entry is a shared thunk.

### 3.2 Object layouts

Measured by aligning the matched bodies and comparing the memory immediates at
the same instruction, then confirmed live. Applied behind `fw-1-4`:

| object | change | evidence |
|---|---|---|
| page/screen | **+0x40** | `EXTRACT_HANDLER`: `[x19+0x178]→[x19+0x1b8]`, `[0x180]→[0x1c0]`, `[0x100]→[0x140]` |
| pad state | **+0xc0** | `PAD_BUSY`: `[x0+0x22e8]→[x0+0x23a8]`, `[0x22f0]→[0x23b0]` |
| program → pad list | **+0xa8** | `FREE_PAD`: `[arg+0x2808]→[arg+0x28b0]`, vector end `0x2810→0x28b8` |
| pad layer element | **stride `0xb30` → `0xc90`** | `PAD_BUSY` loop |
| binding | unchanged (`0x60`) | the Extract handler still reads `[binding+0x60]` |
| layer sample / holder | unchanged (`0x4a8` / `0x4b0`) | 1.4 helper `0x2814930` is 72/72 identical |
| layer active | unchanged (`0xb18`) | a 1.4 function using both `+0x4b0` and `+0xb18` is byte-identical |
| selection `0x618`, range `0x78`/`0xd0`, slice fields | unchanged | `SLICE_INSERT` 821/821, `SLICE_RESET` 67/67, `SLICE_SET_END` 88/88, `SELECT_SLICE` 119/119 instructions align |
| chop predicate | unchanged | 30/30 instructions align |

Two traps worth knowing:

* **Pad tables are owned by the program, at `+0x28b0` in 1.4.** The same
  objects also have a vector at `+0x2808` in 1.4, but that one is a 19-element
  parameter vector and is *not* the pads. In 1.3 the pad table was at
  `+0x2808`. This is why `pads::surface_pad_table` trusts the profile offset
  and validates the alternate one (ordered vector, ≤128 entries, first slots
  null or live app objects) instead of blindly following either.
* The page connector map gained entries: the 1.4 page has
  `0x1a0 AmpEnvelopeConnector`, `0x1a8 ChopPropertiesConnector`,
  `0x1b0 FilterEnvelopeConnector`, `0x1b8 InstrumentConnector`,
  `0x1c0 LayerConnector`, `0x1c8 SliceConnector`. A uniform `+0x40` shift does
  not describe the slice selection; that is why `SCREEN_SELECTION` is `0x1c0`
  (Layer) and the slice connector is `0x1c8`.

### 3.3 ABI changes

* **`FREE_PAD` returns a structure on 1.4.** The return is
  `{ u32 index; u8 found; … }` in `x0`, and the app's own callers test byte 4.
  1.3 returned a plain `int`. `pads::first_free_pad` reads the structure and
  returns `None` when `found == 0`; the 1.3 profile keeps the `int` path.
* **The 1.4 Extract handler is bigger and has a different call graph**:
  `0x9f4` bytes in 1.3, `0xac8` in 1.4, with a new cluster of calls around
  `0x1100cxx` and reads of the page connectors at `0x1b8/0x1c0/0x1c8`. It
  still ends at the same constructor (`EXTRACT_CTOR`), which has the same
  argument shape as 1.3 (`x4` is the page state).
* The rootfs is dm-verity protected and the verity command line is in the FIT
  (see §2.4), so lab images must be rebuilt, not patched in place.

---

## 4. How the table was derived (the method that worked)

1. **Function census from `.eh_frame`**: 193,907 functions in 1.3.0.12,
   207,505 in 1.4.0.8.
2. **Normalized body hashing**: mask PC-relative immediates and ADRP-derived
   data immediates, hash the instruction sequence, match equal bodies.
3. **Signatures for changed functions**: referenced `.rodata` strings, PLT
   imports, and the hashes of their callees.
4. **Vtables by destructor pair**: find the two destructor variants in the
   vtable, then confirm through the constructor that stores the vtable at
   `[object]`.
5. **Remaining guards** by mapped callers/callees and neighbouring functions,
   then hand-checked.

Every mapped site was re-checked against the binary: it is an `.eh_frame`
function entry with the expected first word. `scripts/check-firmware-table.py`
automates that check; it auto-detects the profile, or takes `--fw 1.3|1.4`.

**Layout changes** were derived with semantic-token diffs: match a function
body in both builds, align it instruction by instruction, and diff the
immediates that are not PC-relative (memory offsets, small constants). A
changed `[x0+0x22e8]` → `[x0+0x23a8]` is the layout delta, with the exact
instruction as evidence.

---

## 5. What the port implements

* `src/firmware.rs`: `Purpose`, `Entry`, the helper constructors, the
  fingerprint list, the profile `mod` selection, and the compile errors for
  "both features" / "neither feature".
* `src/firmware/v1_3.rs` and `v1_4.rs`: 22 hooks, 21 calls, 20 guards (63
  fingerprinted instruction words) and 4 data entries (vtables, no word), plus
  `APP_PROBE_OFFSET` — 67 table entries per profile, same names. The checker's
  "63 entries match" counts the word-bearing entries.
* Layout constants are `#[cfg(feature = "fw-1-4")]` pairs in
  `platform/descent.rs` (`PAGE_BINDING`, `STATE_LAYERS_OFFSET`,
  `STATE_PROGRAM`), `platform/page.rs` (chop-controller flag), `platform/pads.rs`
  (`PAD_STATES`, `SURFACE_PADS`, `_END`, and the validated alternate),
  `platform/slice.rs` (`SCREEN_SELECTION`, `SCREEN_BINDING`), and
  `event.rs` (`DATA_TARGET_OFFSET`).
* `pads::pad_slot(program, index)` and the destination check in
  `extract::run_inner` so an Extract can be judged from the program's pad
  table on a host that never delivers a `SurfaceEvent`.
* `scripts/check-firmware-table.py` understands both profiles.
* `Cargo.toml` 0.9.0 with the two features; `README.md`,
  `docs/17-firmware.md`, `docs/22-build-and-install.md` updated.

Tests: `cargo test` with the default profile and with
`--no-default-features --features fw-1-4` both pass; at the time of the port
that was 129 tests per profile. `cargo clippy --all-targets` and `cargo fmt`
are clean on both.

---

## 6. What is validated

### 6.1 Static

* `check-firmware-table.py` reports `ok: firmware 1.4 (63 entries match)` and
  `ok: firmware 1.3 (63 entries match)`; the negative control fails.
* Every hook site is an `.eh_frame` function entry with the expected first
  word; `CC` is the one deliberate mid-function hook (`+0x10`,
  word `0x39402003`).
* The vtable entries are identified by their destructor pair and confirmed
  through the constructor that stores them.

### 6.2 Live, on the real 1.4.0.8 app in the lab

The lab booted the stock 1.4.0.8 rootfs built from the parts above; the running
binary was confirmed as `473fb726…457dc91a`, i.e. this update image and not the
lab's default image.

A probe mod built with `fw-1-4` installed through the SDK and walked the live
page on the UI thread:

| observation | result |
|---|---|
| page | `SampleModeScreen`, vtable at the 1.4 `SAMPLE_PAGE_VTABLE` |
| `page+0x1b8` | `InstrumentConnector` (1.3's `Sequencer` at `0x178` is a different field) |
| `+0x60` | `Instrument` (the page state) |
| `state+0x23a8` | `InstrumentLayer` |
| `layer+0x4a8` | `Sample`; `+0x130` = `{2 ch, 15,129 frames, 44,100 Hz}` |
| `layer+0x4b0` | `shared_ptr<Sample>` holder; refcount 2–4 |
| `state+0x23d0` | `DrumProgram`; the old `0x2310` is empty |
| `program+0x28b0` / `+0x28b8` | pad vector; `PAD_BUSY` true on pad 0; 128/128 busy in the factory project |
| heap scan | both `DrumProgram` and `AuxProgram` own 128-element pad tables of `Instrument*` at `+0x28b0`; `+0x2808` on those objects is the 19-element parameter vector |
| `FREE_PAD` ABI | a synthetic program sharing the real vtable returned `{index=0, found=1}`; the all-busy program returned `found=0` |
| `SCREEN_BINDING 0x1a8` | `ChopPropertiesConnector`; `chop_kind` moved 0 → 1 when CHOP was pressed |
| DATA encoder | button-event surface plus `0x2da8` matched the actual DATA encoder event object |
| Extract handler prologue | reads `[x19+0x1b8]`, matching `PAGE_BINDING` |

That earlier scan did not identify the encoder target: subsequent live button and DATA events proved that the encoder is an embedded object at `+0x2da8`, while pad-pressure events use a different surface object. The pad-table fallback still validates its candidate vector.

### 6.3 The 1.4 Extract, proven with the debugger

Using QEMU's gdbstub (§9.4), with a genuinely free pad (see §9.5) and the
factory sample loaded:

* breakpoint at `EXTRACT_HANDLER` (`base+0x1ccadb0`) **hit** — the SDK's Extract
  really calls the app handler;
* breakpoint at `EXTRACT_CTOR` (`base+0x1a44220`) **hit** — the handler really
  constructs the Extract command;
* after those stops the app entered **“Saving changes”**, consistent with the
  command modifying the project. The SDK's own result for that run was not
  captured (the probe log needs the guest filesystem), so this is evidence
  that the command path runs, not yet proof of the final pad contents.

This supersedes the earlier belief that the 1.4 handler aborts before the
constructor (§7.3 explains why that belief was wrong).

---

## 7. Live follow-up and limits

A fresh probe on the 1.4 app cleared pad A14 through the stock UI and generated a one-second mono signal. Extract returned `Ok`, the destination's existing instrument retained a nonempty layer and acquired a new sample, and the saved stereo WAV contained all 44,100 expected frames with matching channels. Triggering the pad produced nonzero PCM. This exposed a channel-count mismatch in the writer, now corrected by converting the rendered buffer to the native source's channel count. An artificially emptied layer vector previously produced a misleading success; the destination check now rejects a stale sample outside that vector.

On the CHOP screen a probe replaced four markers, selected slice 3, replaced them again, and observed the selection settle to slice 1. The app's selection model is embedded at `screen+0x1c0` connector `+0x618`. The CHOP controller flag at `screen+0x158` then `+0x53` bit 0 tracked off/on/off along with the SDK paint event.

Raw panel MIDI delivered note-on, note-off and poly-pressure surface events. The pad-event surface is distinct from the button-event surface. A button-event surface plus `0x2da8` matched the DATA encoder event's object; the latter is embedded there, not a pointer field. The prior runtime pointer probe was removed.

There has been no physical-device run. The lab exercises the unmodified stock app and emulated PCM, not physical hardware timing. The lab's guest network can still drop and a debugger that disconnects without detaching can leave QEMU stopped (§9.7).

## 8. Verification

Run `cargo test` and `cargo clippy --all-targets` on each profile, `cargo fmt --check`, and the table checker on each stock binary. Include a one-word mutation as a negative control. See [VALIDATION-1.4.0.8.md](VALIDATION-1.4.0.8.md) for the measured live results and earlier failed approaches.

---

## 9. The lab: setup and use

The lab is a separate repository (`virtual-mpc-sample`). It boots the stock
rootfs byte-identically through QEMU (HVF on Apple Silicon), with the panel in
the browser. Nothing here modifies the SDK.

### 9.1 Build the lab image from the 1.4 parts

The lab's `lab.py build images` verifies the stock 1.3 inputs against
`inputs.sha256` and will refuse the 1.4 parts. Drive the build script directly;
it accepts the three filesystem images and regenerates the verity command line:

```sh
cd "$LAB"
docker run --rm -v mpclab-build:/build -v "$LAB":/lab \
  -v /path/to/fw14/part-d89b00.raw:/fw/boot.ext2:ro \
  -v /path/to/fw14/part-18b0538.raw:/fw/rootfs.ext4:ro \
  -v /path/to/fw14/parr-1af670.raw:/fw/recovery.ext2:ro \
  -e REBUILD_EMMC=1 mpclab-tools sh /lab/scripts/build-images.sh
```

`part-d89b00.raw` is the December kernel ext2 (boot), `parr-1af670.raw` the
April kernel (recovery), `part-18b0538.raw` the rootfs. The build prints
`rootfs sha256 source: …`; for the 1.4 image used here it was
`c24727870ed2c817`.

Content library (the app's browser reads `/content`):

```sh
CONTENT_DIR=/path/to/wavs python3 lab.py build content
```

### 9.2 Run and drive the app

```sh
python3 lab.py run --headless     # QEMU; panel on http://127.0.0.1:8765/
python3 lab.py status             # ports: cs 5551, ext_midi 5552, console 5553, ssh 2022
python3 lab.py ssh                # root shell (when the guest network is up)
python3 lab.py stop
python3 lab.py reset              # delete the run overlay; next boot is a fresh /data
```

Panel API (no SSH needed):

```sh
curl -s http://127.0.0.1:8765/screen.png -o screen.png
curl -s -X POST http://127.0.0.1:8765/control -d '{"name":"SAMPLE SELECT","action":"press"}'
curl -s -X POST http://127.0.0.1:8765/control -d '{"name":"SAMPLE SELECT","action":"release"}'
```

Useful control names (45 total in `panel/controls.json`): `SAMPLE`,
`SAMPLE SELECT`, `DATA`, `PAD 1`..`PAD 16`, `SCREEN 1/2/3`, `CHOP`, `ERASE`,
`SHIFT`, `SEQ`, `PLAY`, `STOP`, `PAD FX`, `KNOB FX`.

The encoder is `DATA` press + release ~80 ms later to click; a `DATA` turn is
a press/release pair in the panel API.

A fresh boot loads the factory project with a sample on the Trim page; all 128
pads are busy. Load another sample with `SAMPLE SELECT` → `DATA` (open the
folder) → `DATA` click (select) → `PAD 1` (load). The browser list is
`/content/Samples`.

### 9.3 Load a probe into the app

Normal path (needs SSH):

```sh
python3 lab.py preload /path/to/probe.so
```

The lab copies the `.so` to `/run/mpclab-preload/` and writes a systemd drop-in
with `LD_PRELOAD`, then restarts `ac50`.

Without SSH (the guest network is sometimes dead; see §9.7), bake the probe
into the lab layer. The initramfs mounts `out/lab-layer.ext4` as an overlay
lower over the stock rootfs, so files in the layer appear in the guest's `/`:

```sh
mkdir -p "$LAB/overlay/lab-layer/opt/mpclab/lib"
cp probe.so "$LAB/overlay/lab-layer/opt/mpclab/lib/probe.so"
mkdir -p "$LAB/overlay/lab-layer/etc/systemd/system/ac50.service.d"
printf '[Service]\nEnvironment=LD_PRELOAD=/opt/mpclab/lib/probe.so\n' \
  > "$LAB/overlay/lab-layer/etc/systemd/system/ac50.service.d/mpclab-preload.conf"
# rebuild the layer (same docker command as §9.1)
```

The app then starts with the probe on every boot. Verify with the debugger
instead of SSH: read the hook word at `base+EXTRACT_CTOR`; `0xd503233f`
(PACIASP) means the probe did **not** install, anything else (typically a `B`
such as `0x15852fd0`) means it did.

**Remove these two files before treating the lab as clean.**

### 9.4 Debug the app with QEMU's gdbstub

```sh
LAB_GDB=1 python3 lab.py run --headless    # -gdb tcp::1234
```

lldb works on the host:

```sh
lldb -b -o "gdb-remote 127.0.0.1:1234" \
       -o "memory read -c 16 0xaaaaaaaa0000" \
       -o "breakpoint set -a 0xaaaaac76adb0" \
       -o "detach" -o "quit"
```

The app is a PIE but consistently loads at **`0xaaaaaaaa0000`** in this lab
(confirmed on every boot checked, from `/proc/<pid>/maps`), so offsets map directly:

| site | runtime address |
|---|---|
| `EXTRACT_HANDLER` | `0xaaaaac76adb0` |
| `EXTRACT_CTOR` | `0xaaaaac4e4220` |
| `EXTRACT_EXEC` | `0xaaaaac4e4590` |
| handler exit blocks | `0xaaaaac76b244` … `0xaaaaac76b4f8` |

The gdbstub stops the whole VM on attach; memory reads and breakpoints only
work when the current CPU's MMU maps the app. At attach the stub may halt in a
kernel context. Catch the app context by continuing and interrupting until a
read at `0xaaaaaaaa0000` returns the ELF magic (`7f 45 4c 46`).

A minimal RSP client (no lldb process to wedge the VM):

```python
#!/usr/bin/env python3
"""Minimal GDB-remote client for the lab gdbstub. Usage: rsp.py handler|exits"""
import socket, sys, time

BASE = 0xaaaaaaaa0000
HANDLER, CTOR, EXEC = 0x1ccadb0, 0x1a44220, 0x1a44590
EXITS = [0x1ccb244, 0x1ccb25c, 0x1ccb26c, 0x1ccb2a8, 0x1ccb2b0, 0x1ccb2f8,
         0x1ccb320, 0x1ccb340, 0x1ccb39c, 0x1ccb3ec, 0x1ccb40c, 0x1ccb42c,
         0x1ccb47c, 0x1ccb49c, 0x1ccb4f8]
mode = sys.argv[1] if len(sys.argv) > 1 else "handler"
sites = [HANDLER] if mode == "handler" else ([CTOR, EXEC] + EXITS)

class R:
    def __init__(self):
        self.s = socket.create_connection(("127.0.0.1", 1234), timeout=5)
        self.s.settimeout(2); self.buf = b""
    def send(self, p):
        if isinstance(p, str): p = p.encode()
        self.s.sendall(b"$" + p + b"#" + f"{sum(p) % 256:02x}".encode())
    def recv(self, t=2):
        self.s.settimeout(t)
        while True:
            i = self.buf.find(b"$"); j = self.buf.find(b"#", i + 1) if i >= 0 else -1
            if i >= 0 and j >= 0 and len(self.buf) >= j + 3:
                pkt = self.buf[i + 1:j]; self.buf = self.buf[j + 3:]
                self.s.sendall(b"+"); return pkt.decode(errors="replace")
            d = self.s.recv(4096)
            if not d: raise EOFError
            self.buf += d

r = R(); r.send("qSupported:multiprocess+")
try: r.recv()
except Exception: pass
def mem(a, n, t=1):
    r.send(f"m{a:x},{n}"); return r.recv(t)
def reg(n, t=2):
    r.send(f"p{n:x}"); return r.recv(t)

# catch the app's MMU context: continue, interrupt, read the ELF magic
for _ in range(80):
    r.send("vCont;c"); time.sleep(0.06); r.s.sendall(b"\x03")
    try:
        while True:
            pkt = r.recv(1)
            if pkt and pkt[0] in "TS": break
    except socket.timeout:
        pass
    if mem(BASE, 4).startswith("7f454c46"):
        break
else:
    raise SystemExit("could not catch the app context")

for off in sites:
    r.send(f"Z0,{BASE + off:x},4"); assert r.recv(2) == "OK", hex(off)

r.send("vCont;c")
deadline = time.time() + 120
while time.time() < deadline:
    try: pkt = r.recv(1)
    except socket.timeout: continue
    if pkt and pkt[0] in "TS":
        # p20 = pc on aarch64; the g reply's register order is not portable
        print("STOP", pkt, "PC", hex(int.from_bytes(bytes.fromhex(reg(0x20)), "little")), flush=True)
        break
else:
    print("no stop", flush=True)
r.send("D")   # always detach
```

Notes:

* `p20` is the aarch64 PC register. Do **not** parse the `g` block by a fixed
  offset unless you have checked the stub's register order; a wrong offset
  reads a different register and looks like a kernel address.
* Always send `D` (detach). A client that dies while attached can leave QEMU
  stopped, which looks like a dead guest (SSH/panel time out, the serial still
  “accepts” connections).
* If the VM is wedged, find the QEMU PID whose command line contains
  `out/run/emmc.qcow2` and kill it, then `lab.py run` again.
* Breakpoints are software (`Z0`), i.e. QEMU patches guest memory. A breakpoint
  at a site the SDK also hooks (`EXTRACT_CTOR`) temporarily replaces the SDK's
  branch; QEMU restores the original byte on removal/detach.
* Do not set breakpoints before the app is loaded; the page is not mapped and
  the stub answers `E22`.

### 9.5 Creating a free pad for diagnostics

For end-to-end validation, clear a pad through the app: stop playback in Sample mode, hold ERASE and press the desired pad. The synthetic vector edit below can reach the constructor, but leaves an empty layer vector and must not be used to prove assignment or playback.

The factory project has all 128 pads busy, so `FREE_PAD` finds nothing. A pad
is busy when `PAD_BUSY` (`0x2d16c10`) finds any layer in its layer vector
(`[pad+0x23a8] … [pad+0x23b0]`). Freeing a pad *validly* means keeping the pad
object and emptying its layer vector:

```rust
let table = read_u64(program + 0x28b0);       // first slot address
let pad = read_u64(table);                     // Instrument*
if pad != 0 {
    let layers = read_u64(pad + 0x23a8);
    write_u64(pad + 0x23b0, layers);           // end = begin: no layers
}
```

Then `first_free_pad(program)` returns that pad while the slot stays a live
`Instrument`. This is what made the handler reach the ctor.

Nulling the slot (`write_u64(table, 0)`) also makes `FREE_PAD` return the pad,
but the pad object is gone and the handler's own pad lookup fails — see §10.

### 9.6 A probe skeleton

The probe used for the live checks was a throwaway crate outside the SDK
(`/tmp/probe-mod`), lost with the temp directory. Recreate it as a `cdylib`
with a path dependency on the SDK:

```toml
[package]
name = "probe-mod"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
mpc-sample-sdk = { path = "/…/mpc-sample-sdk", default-features = false, features = ["fw-1-4"] }
```

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use mpc_sample_sdk::draw::Graphics;
use mpc_sample_sdk::image;
use mpc_sample_sdk::platform::{extract, pads};
use mpc_sample_sdk::{Mod, PaintEvent};

static GO: AtomicBool = AtomicBool::new(false);

fn log(line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/run/probe.log") {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
    // also visible on the guest console and in out/run/console.log
    if let Ok(mut k) = std::fs::OpenOptions::new().write(true).open("/dev/kmsg") {
        use std::io::Write;
        let _ = write!(k, "<3>PROBE {line}\n");
    }
}

#[derive(Default)]
struct Probe;

impl Mod for Probe {
    fn on_start(&mut self) {
        log("installed");
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(45));
            GO.store(true, Ordering::Release);
            log("timer fired");
        });
    }

    fn on_paint(&mut self, _g: &mut Graphics, event: PaintEvent) {
        if !event.sample_page || !GO.swap(false, Ordering::AcqRel) {
            return;
        }
        let screen = event.component;
        // page + 0x1b8 -> binding; binding + 0x60 -> state
        let binding = image::read_u64(screen + 0x1b8).unwrap_or(0) as usize;
        let state = image::read_u64(binding + 0x60).unwrap_or(0) as usize;
        let program = image::read_u64(state + 0x23d0).unwrap_or(0) as usize;
        let table = image::read_u64(program + 0x28b0).unwrap_or(0) as usize;
        let pad = image::read_u64(table).unwrap_or(0) as usize;
        if pad != 0 {
            let layers = image::read_u64(pad + 0x23a8).unwrap_or(0);
            // SAFETY: a live pad object; only its layer-vector end is written.
            unsafe { let _ = image::write(pad + 0x23b0, &layers.to_le_bytes()); }
        }
        let free = unsafe { pads::first_free_pad(program) };
        log(&format!("free={free:?} sample={state:#x}"));
        if let Some(destination) = free {
            let audio: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.01).sin() * 0.1).collect();
            let result = unsafe {
                extract::run(extract::Source::Generated(extract::Generated {
                    screen,
                    name: c"ProbeGen",
                    audio: &audio,
                    channels: 2,
                    destination,
                }))
            };
            log(&format!("pad={destination} result={result:?}"));
        }
    }
}

mpc_sample_sdk::export_mod!(Probe);
```

Build and load:

```sh
./build-mod.sh --manifest-path /tmp/probe-mod/Cargo.toml \
  --package probe-mod --name probe --output-dir /tmp/probe-mod/build
python3 "$LAB/lab.py" preload /tmp/probe-mod/build/probe.so
# or bake it into the lab layer (§9.3) when SSH is down
```

Rules that matter: run the Extract on the **UI thread** (a paint or a UI
event), never from a background thread; the app's `FREE_PAD`, the handler and
the command all expect the UI thread. The timer + paint is what makes the run
deterministic for a debugger.

### 9.7 Gotchas found the hard way

* `lab.py build images` refuses non-1.3 inputs (`inputs.sha256`); use the
  docker build script directly.
* The lab's `/data` lives in the run overlay (`out/run/emmc.qcow2`), not in the
  base image. A broken project (dangling sample, all pads busy) survives a base
  image rebuild; `lab.py reset` clears it. That is also the cheapest fix for a
  “Sample not found” modal.
* The guest network can die while serial/panel stay up (`sshd` accepts TCP and
  never sends a banner, on 2022 and 2222). The baked-layer probe and the
  gdbstub avoid SSH entirely.
* A killed debugger client can leave QEMU stopped; always detach, and kill the
  QEMU process if needed (verify `out/run/emmc.qcow2` in its command line).
* `lab.py stop` can hang on a wedged VM; killing the QEMU PID is the fallback.

---

## 10. Approaches: worked vs blind paths

### Worked

| approach | why it worked |
|---|---|
| `.eh_frame` census + normalized body hashing | matched thousands of unchanged functions; the changed ones stayed small enough to identify by signature |
| `.rodata` string / PLT import / callee-body signatures | distinguished the changed functions reliably |
| vtable by destructor pair + constructor store | identified data entries without guessing class names |
| semantic-token diffs on aligned bodies | recovered the layout deltas with the exact changed instruction as evidence |
| live probe descent on the UI thread | confirmed the object chain and the `FREE_PAD` ABI that no static read could settle |
| `check-firmware-table.py` (auto-detect + negative control) | one command proves the table against a binary and catches a wrong profile |
| QEMU gdbstub (`LAB_GDB=1`) + RSP | reached the handler/ctor, read the PC, proved the call path |
| baked lab layer (`LD_PRELOAD` drop-in) | loads the probe without SSH; survives the dead guest network |
| clear a pad through the stock UI | leaves a valid empty pad for end-to-end Extract and playback tests |

### Blind paths (do not repeat)

| path | why it failed |
|---|---|
| nulling a program pad slot to “free” a pad | `FREE_PAD` returns it, but the pad object is gone and the 1.4 handler's pad lookup fails |
| treating a missing ctor diagnostic as “the handler aborts before the ctor” | the diagnostic depended on a hook write; the debugger later hit the ctor. Verify call paths with a breakpoint, not with a side-effect file |
| in-process exit trampolines with `mmap`/`MAP_FIXED` | the stub page landed outside the ±128 MB `b` range (hint ignored, `MAP_FIXED_NOREPLACE` refused); the trampolines then crashed the app |
| copying exit blocks that start with `ADRP` into a trampoline | PC-relative instructions cannot be relocated by a naive copy |
| file-backed `mmap` for the exit id | refused in the guest; a crashing stub loses the in-process value |
| scanning for a code cave in `.bss` | no reliably free zero run; and the fallback is not worth it once the gdbstub works |
| expecting pressure from ordinary panel button presses | send raw panel poly-pressure (for example `a9 25 40`) to exercise the surface hook |
| treating the pad-table slot as the sample | slots contain instrument objects, which can remain unchanged when their active layer gets a new sample |
| interpreting connector `+0x618` as the slice index | it is the selection model; the index is passed separately to the native setter |
| `CHOP_VIEW` from `set_kind` | `chop_kind` moves, `chop_mode` stays false; the predicate is not driven by that field |

---

## 11. The hypothesis ledger

`HYPOTHESES.csv` records the Extract investigation:

* “the handler aborts at a pre-ctor exit block” — **disproved** by the
  debugger: the ctor is reached;
* “the pad lookup is the blocker” — **disproved**; a valid pad passes it;
* exit trampolines — **superseded** by the gdbstub (the trampolines crashed);
* “with a valid pad the handler reaches the ctor and only the SDK arming
  fails” — this started as a diagnostic that saw no ctor write; the debugger
  later showed the ctor **is** reached, so the diagnostic was invalid. The
  later generated-audio and playback checks proved the armed execute path (§7).

Treat the CSV as history, not as current truth; the debugger evidence in §6.3
supersedes it.

---

## 12. Reproduction commands

```sh
# 1. table check (either build)
python3 scripts/check-firmware-table.py "/path/to/rootfs/usr/bin/MPC Sample"
python3 scripts/check-firmware-table.py "/path/to/rootfs/usr/bin/MPC Sample" --fw 1.4

# 2. both profiles
cargo test
cargo test --no-default-features --features fw-1-4
cargo clippy --all-targets
cargo clippy --all-targets --no-default-features --features fw-1-4
cargo fmt --check

# 3. lab (see §9)
LAB_GDB=1 python3 lab.py run --headless
python3 lab.py preload /tmp/probe-mod/build/probe.so
python3 rsp.py handler        # save the §9.4 script as rsp.py; press SCREEN 2 to paint

# 4. firmware extraction (see §2)
unzip "MPC Sample 1.4.0 Updater.app.zip" "MPC Sample 1.4.0 Updater.app/Contents/Resources/Update.img"
cd fw14 && python3 extract.py && xz -dc part-18b0538.bin > rootfs.ext4
7z x -o rootfs rootfs.ext4 "usr/bin/MPC Sample"
```

---
