# SDK ↔ firmware validation — MPC Sample 1.4.0.8

The SDK now carries two firmware profiles. The **1.4.0.8 address table is
complete and verified** (63/63 words, every site on a function entry, every
vtable by its destructor pair), and the **object-layout changes measured below
are applied** behind the same `fw-1-4` feature.

## 1. Inputs

| step | value |
|---|---|
| updater | `MPC Sample 1.4.0 Updater.app.zip` (245,106,350 B) |
| image | `Contents/Resources/Update.img`, 242,687,968 B, snapshot `SNAPSHOT-20261123112003` |
| app | `usr/bin/MPC Sample`, 128,398,464 B |
| version | `.note.mpc.version` = `MPC 1.4.0.8 (d87ac8c12e9)` |
| BuildID | `1b51367882445eaaedc154aa43ff25caf3b9bd91` |
| full SHA-256 | `473fb7260e597e61cc55d9d30595eb9d9aa1304ff1b88960e633ad19457dc91a` |
| first 4 KiB | `2b1ce171e951c90bf08cf375d7db9065f1782745b07a436bb2d22fdaabba0af0` |

Extraction follows [APP-EXTRACTION.md](APP-EXTRACTION.md); the `AZ0x` records
are at `0x90` as before, and the 1.4 payloads are `BOOT` at `0x298`, `PARR` at
`0x1AF670`, the kernel `PART` at `0xD89B00`, and the rootfs `PART` at
`0x18B0538`.

## 2. How the table was derived

1. Function census from `.eh_frame`: 193,907 functions in 1.3.0.12, 207,505 in
   1.4.0.8.
2. Normalized body hashing (PC-relative immediates and ADRP-derived data
   immediates masked) to find unchanged functions.
3. For functions that changed, a signature of referenced `.rodata` strings,
   PLT imports and callee-body hashes.
4. Vtables located by their destructor pair, then confirmed through the
   constructor that stores them at `[object]`.
5. Remaining guards resolved by mapped callers/callees and by the neighbouring
   functions, then checked by hand.

Every mapped site was re-checked against the binary: it is an `.eh_frame`
function entry with the expected first word. `CC` is the one deliberate
mid-function hook (`+0x10` into `0x1c34350`, word `0x39402003`).

## 3. Verified table

The table lives in `src/firmware/v1_4.rs`. Selected with:

```toml
mpc-sample-sdk = { version = "0.9", default-features = false, features = ["fw-1-4"] }
```

Highlights (full list in the file):

| kind | site | 1.3.0.12 | 1.4.0.8 |
|---|---|---|---|
| hook | `NOTE_ON` | `0x01e27360` | `0x0248d4d0` |
| hook | `NOTE_OFF` | `0x01e270d0` | `0x0248d240` |
| hook | `CC` | `0x0159f230` | `0x01c34360` |
| hook | `EXTRACT_CTOR` | `0x0143ed40` | `0x01a44220` |
| hook | `EXTRACT_EXEC` | `0x0143f0b0` | `0x01a44590` |
| call | `EXTRACT_HANDLER` | `0x016580d0` | `0x01ccadb0` |
| call | `BUILD_SAMPLE` | `0x01df4ac0` | `0x01e807d0` |
| call | `FREE_PAD` | `0x0286dd00` | `0x02d7cbe0` |
| call | `SELECT_SLICE` | `0x016680a0` | `0x01cddf70` |
| data | `SAMPLE_PAGE_VTABLE` | `0x06e212a0` | `0x076f6170` |
| data | `PROJECT_SAVE_SCREEN_VTABLE` | `0x06e1faa0` | `0x076f4998` |
| data | `TEXT_EDITOR_VTABLE` | `0x06e18e20` | `0x076e0880` |
| data | `LIGHTING_SENDER_VTABLE` | `0x06d8fa60` | `0x076247a0` |
| probe | `APP_PROBE_OFFSET` | `0x00b55b00` | `0x00bca5c0` |

Verification:

```
$ python3 scripts/check-firmware-table.py "/tmp/fw14/.../MPC Sample"
ok: firmware 1.4 (63 entries match)
firmware 1.3: MISMATCH hook 0x1545a50: ... (63 of 63 differ)

$ python3 scripts/check-firmware-table.py "/tmp/mpc-fw/.../MPC Sample"
ok: firmware 1.3 (63 entries match)
firmware 1.4: MISMATCH hook 0x1c01a20: ... (59 of 63 differ)
```

Negative control: a one-word edit to a copy fails the exact profile with exit 1.

## 4. Object layouts moved in 1.4

Measured by aligning the matched function bodies and comparing the memory
immediates at the same instruction.

| object | change | evidence |
|---|---|---|
| screen | **+0x40** | `EXTRACT_HANDLER`: `[x19+0x178]→[x19+0x1b8]`, `[0x180]→[0x1c0]`, `[0x100]→[0x140]` |
| pad state | **+0xc0** | `PAD_BUSY`: `[x0+0x22e8]→[x0+0x23a8]`, `[0x22f0]→[0x23b0]`; `EXTRACT_HANDLER`: `[x20+0x2310]→[x20+0x23d0]` |
| pad list | **+0xa8** | `FREE_PAD`: `[arg+0x2808]→[arg+0x28b0]`, vector end `0x2810→0x28b8` |
| pad layer element | **stride 0xb30 → 0xc90** | `PAD_BUSY` loop |
| binding | unchanged (`0x60`) | `EXTRACT_HANDLER` |
| slice / selection | unchanged | `SLICE_INSERT` 821/821, `SLICE_RESET` 67/67, `SLICE_SET_END` 88/88, `SELECT_SLICE` 119/119 instructions align, only GOT slots move |
| chop predicate | unchanged | 30/30 instructions align |
| layer helper | `[x0+0x4b0]` unchanged | 72/72 align |

## 5. Layout offsets applied

Each value below is used by the `fw-1-4` build; the 1.3 value stays in the
`fw-1-3` build. The measurements come from the app's own instructions (section
4), not from guesswork about the class names.

| file | constant | 1.3 | 1.4 |
|---|---|---|---|
| `descent.rs` | `PAGE_BINDING` | `0x178` | `0x1b8` |
| `descent.rs` | `STATE_LAYERS_OFFSET` | `0x22e8` | `0x23a8` |
| `descent.rs` | `STATE_PROGRAM` | `0x2310` | `0x23d0` |
| `page.rs` | chop mode | visible view at `0x1ee0` | controller at `screen+0x158`, flag `+0x53` bit 0 |
| `pads.rs` | `PAD_STATES` | `0x2808` | `0x28b0` |
| `pads.rs` | `SURFACE_PADS` / `_END` | `0x2808` / `0x2810` | `0x28b0` / `0x28b8` (program pad tables), fallback probed |
| `seed.rs` | `SURFACE_PAD_LIST` / `_END` | `0x2808` / `0x2810` | `0x28b0` / `0x28b8` |
| `slice.rs` | `SCREEN_SELECTION` | `0x180` | `0x1c0` |
| `slice.rs` | `SCREEN_BINDING` | `0x168` | `0x1a8` |
| `event.rs` | `DATA_TARGET_OFFSET` | `0x2da8` | unchanged; observed on the live surface |

Deliberately unchanged, with the evidence:

| field | why |
|---|---|
| `BINDING_STATE` `0x60` and the binding flags/owner/slots | the Extract handler still reads `[binding+0x60]`, and the slice functions that use the binding align 100% |
| `LAYERS_SAMPLE` `0x4a8`, `LAYER_ACTIVE` `0xb18` | the layer object's early fields are unchanged (`0x2814930` helper 72/72 identical, including `+0x4b0`); a 1.4 function using both `+0x4b0` and `+0xb18` is byte-identical, so the `+0x160` growth was appended |
| selection `0x618`, range `0x78`/`0xd0`, slice fields | `SLICE_INSERT`, `SLICE_RESET`, `SLICE_SET_END`, `SELECT_SLICE` align instruction for instruction, only GOT slots move |
| sample holder `0x170`, holder `0x18`/`0x28`, metadata `0x130`/`0x3e4` | no measured change in the sample paths; the sample-service constructor and writer align 100% |
| effect parameter block `0x18` | the effect factories are byte-identical |
| surface layout | the surface object is only reachable through a device pressure event, so the SDK probes both candidate slots instead of trusting either; see section 6 |

## 6. Live validation on 1.4.0.8

The address and layout tables were exercised against the real app, not only
statically: the 1.4.0.8 partitions were written into the virtual MPC Sample lab
(its build script already expands the 1.4 verity cmdline), QEMU booted the stock
rootfs, and the running binary was confirmed as
`473fb726…457dc91a` — this update image, not the 1.4.0.7 build the lab ships.

A probe mod built with `fw-1-4` installed through the SDK and walked the live
page on the UI thread:

| observation | result |
|---|---|
| page | `SampleModeScreen`, vtable at the 1.4 `SAMPLE_PAGE_VTABLE` |
| `page+0x1b8` | `InstrumentConnector` (1.3's `Sequencer` at `0x178` is a different field) |
| `+0x60` | `Instrument` (state) |
| `+0x23a8` | `InstrumentLayer` |
| `+0x4a8` | `Sample` |
| `+0x130` | `{2 channels, 15 129 frames, 44 100 Hz}` |
| `layer+0x4b0` | `shared_ptr<Sample>` (holder) |
| `layer+0xb18` | active byte readable |
| `sample::root_note` / `sample::memory` | `60` / frames and buffer resolved |
| `state+0x23d0` | `DrumProgram`; old `0x2310` empty |
| `program+0x28b0` / `+0x28b8` | pad vector; `PAD_BUSY` true on pad 0; 128/128 busy, so `first_free_pad` is `None` |
| heap scan | `DrumProgram` and `AuxProgram` own 128-element pad tables of `Instrument` pointers at `+0x28b0`; the same objects' `+0x2808` is a 19-element parameter vector, not pads |
| `SCREEN_BINDING 0x1a8` | `ChopPropertiesConnector`; `chop_kind` moved 0 → 1 when CHOP was pressed |
| `FREE_PAD` ABI | a synthetic program sharing the real vtable returned `index=0, found=1`; the real all-busy program returned `found=0` |
| Extract handler | its prologue reads `[x19+0x1b8]`, matching `PAGE_BINDING` |

Two ABI details came out of the run:

* `FREE_PAD` on 1.4 returns `{u32 index; u8 found}` in `x0`, and the app's own
  callers test byte 4. `first_free_pad` now reads the structure and returns
  `None` when `found` is zero; on 1.3 it keeps the plain `int` return. Both
  paths ran: a synthetic program with a null pad slot returned `{0, 1}`, the
  real all-busy program returned `{_, 0}`.
* The heap scan proves the pad-table model: pad tables live on programs at
  `+0x28b0` (1.4) / `+0x2808` (1.3) with 128 `Instrument` elements, and other
  objects have vectors of the same shape at those offsets. That is why the
  fallback offset in `pads::surface_pad_table` also requires its first slots to
  be live app objects, while the profile offset is trusted.
* The DATA encoder hook receives an embedded object, not the button event's
  surface itself. A live button event's surface plus `0x2da8` equalled the
  corresponding DATA encoder object. Pad-pressure events use a separate object.

## 7. Slice, chop and Extract attempts (negative results)

These were exercised live and did **not** succeed; they are recorded so the
offsets are not mistaken for verified ones.

Later debugger evidence supersedes the earlier constructor-hook diagnosis below:
with a valid free instrument the handler reached the Extract constructor. The
execute hook ran through the SDK on a pad cleared by the app. The instrument
pointer stayed unchanged while its active layer acquired the generated sample.
The saved stereo WAV held all 44,100 expected frames, with both channels equal
to the generated mono input within 0.000018; triggering the pad produced
nonzero lab PCM. The SDK converts channels to the native writer's format and
requires a nonempty layer vector when checking the sample. A probe with an
artificially empty layer vector exposed the original false positive and is not
valid evidence of a playable pad.

On the visible CHOP screen, a live call wrote four markers and the UI displayed
them; the selection settled to index zero after the native asynchronous update.
The stock handler uses the connector at `screen+0x1c0` and its selection model
at `+0x618`. CHOP off/on/off changed bit 0 at controller `+0x53`, and the
updated SDK's `chop_mode` and paint event followed those transitions. Raw panel
MIDI delivered note-on, note-off and poly-pressure surface events. A separate
button event's surface plus `0x2da8` matched the DATA encoder event object; the
encoder is embedded there, rather than stored as a pointer. The pad-event and
button-event surface objects are distinct.

| attempt | result | what it means |
|---|---|---|
| `slice::write_markers` with `SCREEN_SELECTION 0x1c0` | `false`, `verify` `false` | `0x1c0` is a `LayerConnector`, not the selection object |
| `slice::write_markers` with `SCREEN_SELECTION 0x1c8` | `false`, `verify` `false` | `0x1c8` is a `SliceConnector`; its `+0x618` reads as a pointer, so the index is not there either |
| `slice::set_kind(1)` then `page::chop_mode` | kind changes, `chop_mode` stays `false` | the binding kind moves, the chop predicate does not |
| chop predicate on the live `AC50ChopController` (`page+0x158`) | `false`, flags `+0xa8 = 0` | no active chop view in this lab state |
| `extract::run(Source::Generated{destination:5})` | `Err(NoFreePad)` | the Extract machinery ran, the app refused for lack of a free pad |
| free pad after nulling a program slot | `first_free_pad = Some(0)`, `{index=0, found=1}` | free pads can be created; `FREE_PAD` sees them |
| `extract::run(Source::Selection{buffer, destination: None})` on the free pad | `Err(NotApplied)`; the pad stayed free | the command ran but assigned nothing; the fresh factory project has no loaded source (the descent sample chain does not resolve) |
| `extract::run(Source::Generated{destination: 0})` with a recorded pad slot | `Err(NotApplied)`; slot `0x0 -> 0x0` | the app did not create the pad; the rebuilt lab has no factory content library, so the Extract seed has no source sample |

The SDK no longer infers assignment from the control surface alone: for a
destination it also compares the pad slot in the program's pad table
(`pads::pad_slot`), so an Extract is reported correctly on a host that never
delivers a `SurfaceEvent`; this earlier conclusion was not established live.

With a content library and a live sample (2 ch, 220 500 frames) the Extract
still returned `NotApplied` and left the destination slot null. The command's
own fields (`+0x78` start, `+0x88` end, `+0x58` destination) are byte-identical
in both builds, and patching the command's range directly (in addition to the
settings object) changed nothing. The failure is therefore inside the 1.4
Extract handler/command path, not in the range offsets; `EXTRACT_HANDLER`
grew from `0x9f4` to `0xac8` bytes.

Earlier instrumentation showed `finished=false`, `wrote=false`, `assigned=false`,
and the command constructor hook did not report firing. The later debugger hit
the constructor, so the earlier inference that the handler always returns before
constructing the command was wrong. Its call graph differs from 1.3 (a new cluster
of targets around `0x1100cxx`, and it reads the page connectors at
`0x1b8/0x1c0/0x1c8`). Its first gate is the sample lookup at `0x1ccae10` (`0x2d15980(state, 0, &out)`
must return non-null); the later gates read the Slice connector
(`screen+0x1c8`, object `+0x60`, range `+0x78/+0xd0`) and a validity check at
`0x2430550`. Seeding that slice range before the handler did not change the
result. Those observations do not establish which gate failed in that run.

The page's connector map on 1.4.0.8, read live:

```
0x1a0 AmpEnvelopeConnector   0x1a8 ChopPropertiesConnector
0x1b0 FilterEnvelopeConnector 0x1b8 InstrumentConnector
0x1c0 LayerConnector         0x1c8 SliceConnector
```

A Layer connector was inserted before the Slice connector. The stock handler
uses the selection model at `LayerConnector+0x618`, verified by its call site
and by the live marker test above.

## 8. Limits of this pass

* No physical-device run. The lab exercises the stock app, raw panel note and
  pressure events, and its emulated PCM output; it does not reproduce physical
  hardware timing or a physical controller.

## 9. Reproduce

```sh
unzip "MPC Sample 1.4.0 Updater.app.zip" \
  "MPC Sample 1.4.0 Updater.app/Contents/Resources/Update.img"
cd fw14 && python3 extract.py          # AZ0x slicer from APP-EXTRACTION.md
xz -dc part-18b0538.bin > rootfs.ext4
7z x -o rootfs rootfs.ext4 "usr/bin/MPC Sample"
python3 scripts/check-firmware-table.py "rootfs/usr/bin/MPC Sample" --fw 1.4
```
