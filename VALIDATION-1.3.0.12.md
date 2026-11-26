# SDK ↔ firmware validation — MPC Sample 1.3.0.12

Validated `mpc-sample-sdk` **0.8.0** (`510e798`) against the stock app extracted
from the official updater. Result: **the whole fingerprint gate and every
hard-coded site matches**; the defects found are documentation/structure drift,
plus one runtime semantic that cannot be settled statically.

## 1. Inputs and identity chain

| step | artifact | evidence |
|---|---|---|
| updater | `MPC Sample 1.3.0 Updater.exe` | PE32, 7-Zip SFX, `signed=true`; SHA-256 `417dc79c…ea8b73` |
| image | `update.img` (169,207,872 B) | `AZ0x` container, `SNAPSHOT-20260402131256`, device `AC50` |
| partitions | p2 `ext2` (12,644,540 B XZ), p3 `ext4` (142,377,060 B XZ) | both SHA-256 match the `PART` table |
| app | `usr/bin/MPC Sample` | ELF64 aarch64 PIE, stripped, 119,243,440 B |

Identity confirmations for the app:

* `.note.mpc.version` = `MPC\0 1.3.0.12 (40e46ca7991)` — exactly the target
  version in the README.
* size = **119,243,440 B**, the size documented in `docs/17-firmware.md`.
* first 4 KiB SHA-256 = `44507c7a7d3029c411b2e3780ea286ca3e08b7e3ffd7b5047d52f73bf5299d1f`
  — byte-for-byte the `--firmware-sha256` documented in `docs/22-build-and-install.md`.
* full SHA-256 = `b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508`.

## 2. Install gate — 63/63 words

`firmware_ok()` compares 63 words: 22 hook sites + 21 called functions + 20
guards. All 63 match the image.

```
$ python3 scripts/check-firmware-table.py "usr/bin/MPC Sample"
ok: 63 fingerprinted entries match
```

Negative control (one prologue word mutated in a copy):

```
$ python3 scripts/check-firmware-table.py app-mut.bin
MISMATCH hook 0xf535c0: expected 0xd503233f, found 0xd503201f   (exit 1)
```

So the gate is both passing and discriminating on this image.

## 3. Site map

`.eh_frame` lists 193,907 function starts, an independent second reader for
"is this an entry point".

* **62 of 63** code entries are exact FDE function starts.
* **CC** (`0x0159f230`) is deliberately **mid-function**: `+0x10` inside the
  function at `0x0159f220`. The fingerprinted word `0x39402003` is
  `ldrb w3, [x0, #8]`; the handler reads the same control byte at `event + 8`,
  and the function itself dispatches on it (`cmp w3, #0x13` … `#0x14`, `#0x64`).
  This is consistent, not drift.
* Hook install order in `sites::table()` equals `firmware::HOOKS` (22/22, same
  order, all offsets distinct).

Trampoline safety (no PC-relative instruction inside the displaced span):

| site | displacement | site | displacement |
|---|---|---|---|
| SIXTEEN_CTOR | 4 | EXTRACT_EXEC | 4 |
| SIXTEEN_PROC | 3 | WRITER | 2 |
| DATA_HOOK | 2 | NOTE | 4 |
| NOTE_ON | 2 | CONTROL | 4 |
| SURFACE | 4 | SAMPLE_SERVICE_CTOR | 4 |
| NOTE_OFF | 2 | LIGHTING | 4 |
| CC | 4 | SAMPLE_SERVICE_DTOR | 1 |
| PITCH | 4 | SAMPLE_SERVICE_DELETE | 1 |
| BUTTONS | 4 | LIGHTING_DTOR | 2 |
| REPAINT | 4 | LIGHTING_DELETE | 2 |
| STRING_FROM_C | 4 | EXTRACT_CTOR | 2 |

Every site's first word is non-PC-relative, so `hook::displaced_words` never
starts with an illegal copy.

## 4. Platform contracts

* `APP_PROBE_OFFSET = 0x00b55b00` is exactly the start of `.text`, inside the
  main executable's first `PT_LOAD` (`0x0 … 0x06d3aff8`), and the loader only
  accepts an unnamed image — correct rule for an `LD_PRELOAD`ed mod.
* `LIGHTING_SENDER_VTABLE = 0x06d8fa60` is the address point of a vtable whose
  first two slots are exactly `LIGHTING_DTOR (0x01439e60)` and
  `LIGHTING_DELETE (0x01439fd0)`; `offset-to-top = 0`, typeinfo at `-8`.
* `SAMPLE_PAGE_VTABLE = 0x06e212a0` and `PROJECT_SAVE_SCREEN_VTABLE = 0x06e1faa0`
  are real vtable address points (`header[-2] = 0`, code in every slot), and
  their constructors store them at `[obj]`:
  `adrp x1, 0x6e21000; add x1, #0x2a0; str x1, [x20]` (page) and the analogous
  store at `0x164ad58`. That is exactly what `page::is_sample_page()` compares.
* `TEXT_EDITOR_VTABLE = 0x06e18e20` (in `sites.rs`) is also a real vtable; the
  editor's constructor materialises it at `0x15be204` — right value, **wrong place** (see
  findings).
* The sample-service destructor pair `0x01df6d40 / 0x01df6ed0` is a vtable pair
  at `0x6e600a8` in `.data.rel.ro`.
* Relay window `0x07900000`: the image ends at `0x072a4cb0`, so the first
  candidate page does not overlap the app; the farthest site is 105.7 MiB away
  (STRING_FROM_C), inside the ±128 MiB `b` range.

## 5. Semantic spot checks (role labels)

| entry | evidence |
|---|---|
| EXTRACT_CTOR / EXTRACT_EXEC | reference `"Extract slice"` |
| EXTRACT_HANDLER | strings `"Extracted to:"`, `"Extract Failed"`, `"No Empty Pads"`, `"Chop"`; calls `EXTRACT_CTOR` and `SELECT_SLICE` |
| SAMPLE_SERVICE_CTOR | string `"Temporary Samples"` |
| SAMPLE_SERVICE_DTOR/DELETE | consecutive vtable pair (above) |
| SLICE_INSERT | string `vector::_M_realloc_insert`; calls `SLICE_SET_END` |
| FONT | strings `Regular`, `Bold`, `Bold Italic`, `Italic` + libpng strings |
| FREE_PAD → PAD_BUSY | direct call |
| CHOP_PREDICATE | reads `[x0+0xa8]` flags and calls a virtual at vtable `+0x70` — matches `page::chop_mode` passing `screen + 0x1ee0` |
| drawing calls | `SET_COLOUR`/`SET_FONT`/`FILL_RECT` size 0x58–0x138, `DRAW_TEXT` 0x224 — consistent with primitives |

## 6. Findings

| # | severity | finding |
|---|---|---|
| 1 | doc | README and `docs/17-firmware.md` say **59** fingerprint words; `docs/01-lifecycle.md` says **37**; the code checks **63**. |
| 2 | doc | `docs/17-firmware.md` says “the one data entry, the Sample page vtable”; there are three data constants (`SAMPLE_PAGE_VTABLE`, `PROJECT_SAVE_SCREEN_VTABLE`, `LIGHTING_SENDER_VTABLE`). |
| 3 | doc | `docs/17-firmware.md`: “The offsets are file offsets, so the binary can be read as it is on disk.” True only for the first `PT_LOAD`; the three vtables sit in the second `PT_LOAD`, where `p_offset ≠ p_vaddr` by `0x10000`. Harmless today (data entries have no word to check), but a future fingerprinted entry there would read the wrong bytes. |
| 4 | structure | `TEXT_EDITOR_VTABLE = 0x06e18e20` is hard-coded in `src/sites.rs`, outside `src/firmware.rs`, contradicting “Every firmware address the SDK depends on, in one table” and the docs’ “one table”. It is not fingerprinted. |
| 5 | structure | `LIGHTING_SENDER_VTABLE` is defined as a `data()` `Entry` but belongs to none of `HOOKS`/`CALLS`/`GUARDS`, so the table’s own tests (`no_address_is_listed_twice`, `the_fingerprint_set_covers_every_word…`) never see it. |
| 6 | doc | `src/firmware.rs` heading “Calls: … recorded but not yet fingerprinted” is stale — the calls are fingerprinted and checked. |
| 7 | runtime unknown | `CcEvent.low` is `s0` at the hook point. The app sets `s1 = 127.0` (`mov w1,#0x42fe0000; fmov s1,w1`) but leaves `s0` for the displaced `scvtf s0, w2` at `+0xc`; so `low` is the caller’s register, not a controller minimum. The documented `event.low / event.range` example is therefore suspect and needs a device check. |

Nothing above breaks installation on 1.3.0.12: findings 1–6 are doc/table
hygiene, finding 7 affects how a mod interprets CC data, not whether the hooks
land.

## 7. What was not verified

* Live behaviour on hardware (hook entry, repaint, audio taps, Extract) —
  static analysis only; no device or emulator was run.
* The `descent.rs` object walk (`page+0x178 → +0x60 → +0x22e8 → +0x4a8 →
  +0x130`) and the other struct offsets. `EXTRACT_HANDLER` does read
  `[x19+0x178]` and `[x20+0x2310]`, but the full chain is not uniquely
  identifiable statically.
* The Authenticode signature of the updater was not cryptographically verified
  (the PE is flagged `signed`); the integrity chain used here is the updater’s
  metadata plus the `PART` hashes and the documented first-4 KiB app hash.

## 8. Reproduce

```sh
7z x "MPC Sample 1.3.0 Updater.exe" Config.json update.img
tail -c +$((0x19967d8+1)) update.img | head -c $((0x87c8064)) > p3.xz
xz -dk p3.xz
7z x p3 "usr/bin/MPC Sample"
python3 scripts/check-firmware-table.py "usr/bin/MPC Sample"   # ok: 63
head -c 4096 "usr/bin/MPC Sample" | shasum -a 256              # 44507c7a…
```

## 9. Resolution (applied)

Findings 1–6 are fixed in this checkout:

| # | change |
|---|---|
| 1 | `README.md`, `docs/01-lifecycle.md`, `docs/17-firmware.md` now say **63** words. |
| 2 | `docs/17` now names all four run-time vtables instead of “the one data entry”. |
| 3 | `docs/17` now explains that offsets are image-relative, equal to file offsets only in the first `PT_LOAD`, and that the vtables are reached via `base + offset`. |
| 4 | `TEXT_EDITOR_VTABLE` moved from `src/sites.rs` into `src/firmware.rs` and into `CALLS`. |
| 5 | `LIGHTING_SENDER_VTABLE` added to `CALLS`, so the table's completeness and duplicate-address tests cover it. |
| 6 | The stale “recorded but not yet fingerprinted” heading now describes code vs data entries. |

Verification after the edits:

```
cargo fmt --all --check          # clean
cargo clippy --all-targets       # clean (all/pedantic/cargo denied)
cargo test                       # 126 passed, 0 failed + doctest
scripts/check-firmware-table.py  # ok: 63 fingerprinted entries match
same script on mutated copy      # MISMATCH, exit 1
```

Finding 7 (`CcEvent.low`) is unchanged: it is a run-time question about the
app's caller, not a table or documentation defect.
