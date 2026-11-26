# APP-EXTRACTION.md

How to get the stock app out of the official **MPC Sample 1.3.0 Updater.exe**,
plus a map of what the firmware image contains.

Validated with p7zip 17.05 and `xz` on macOS against the 1.3.0.12 build
(`.note.mpc.version` = `1.3.0.12 (40e46ca7991)`). Linux works the same; replace
`shasum -a 256` with `sha256sum`.

Nothing here runs or modifies the installer. Extraction is read-only.

## 0. Prerequisites and space

* `7z` (p7zip), `xz`, `python3`
* about 1.1 GB free: 168 MB installer + 169 MB image + 427 MB rootfs + 119 MB app

```sh
brew install p7zip xz          # macOS
sudo apt install p7zip-full xz-utils   # Debian/Ubuntu
```

## 1. Unpack the installer

The updater is a PE32 7-Zip self-extracting archive (installer SHA-256
`417dc79c1baa9e35ed173757c15dc8e7249224775ee9389739814f657fea8b73`).

```sh
7z l "MPC Sample 1.3.0 Updater.exe"
7z x -y -ompc-installer "MPC Sample 1.3.0 Updater.exe" Config.json update.img
```

| file | size (B) | what it is |
|---|---|---|
| `update.img` | 169,207,872 | the firmware image (the only thing needed here) |
| `FirmwareUpdater.exe` | 3,341,392 | the Windows flasher (JUCE app, libusb) |
| `libusb-1.0.dll` | 756,224 | USB transport |
| `Background.png` | 8,243 | UI asset |
| `Config.json` | 845 | UI strings and update options |

`update.img` SHA-256 is
`5c49811b80b75c246b203896a581aecdcd6dd813db6f28f9d1f2183ba85d8c99`.

The SFX config block sits at `0x34400` and is 69 bytes:

```
;!@Install@!UTF-8!
RunProgram="FirmwareUpdater.exe"
;!@InstallEnd@!
```

The embedded 7z therefore starts at `0x34445`. The file continues past the end
of the archive with the PE certificate table (the installer is
Authenticode-signed), so open it with `7z` rather than slicing at an offset.

Do not run the `.exe`: it is a Windows-only flasher that writes over USB.

## 2. What `update.img` is

All remaining steps run inside `mpc-installer/`, where `update.img` now lives.

`AZ0x` container, version 1. Fixed header up to `0x90`, then four 64-byte
records, then a 264-byte signature block, then the payloads.

| header field | value (1.3.0.12) |
|---|---|
| magic `0x00` | `AZ0x` |
| version `0x04` | `1` |
| records `0x26` (u16) | `4` |
| snapshot `0x38` | `SNAPSHOT-20260402131256` |
| product `0x51` | `MPC Sample image` |
| device `0x62` | `AC50` |
| slot names `0x67` | `recovery`, `boot`, `rootfs`, `akai-1` |
| record table `0x90` | 4 × 64 B |
| trailer `0x190` | 8 B + 256 B (an RSA-2048-sized signature; not verified here) |

Record layout, 64 bytes each:

| offset | size | field |
|---|---|---|
| `+0x00` | 4 | magic: `BOOT`, `PARR` or `PART` |
| `+0x04` | 4 | index (`0` for `BOOT`, `1` otherwise) |
| `+0x08` | 8 | payload offset |
| `+0x10` | 8 | payload size |
| `+0x18` | 8 | undecoded (0 for `BOOT`, `0x2F`/`0x38`/`0x3D` otherwise) |
| `+0x20` | 32 | SHA-256 of the payload |

Records are positional: they carry no partition names, only offset/size/hash.
Payloads are 8-byte aligned; zero padding may follow a record's data.

```
0x000000  header + 4 records + signature block        0x298
0x000298  BOOT   bootupd DTB, raw                     1,754,600
0x1AC880  PARR   XZ -> ext2, kernel.fit (2025-04)    12,431,000
0xD87718  PART   XZ -> ext2, kernel.fit (2025-12)    12,644,540
0x19967D8 PART   XZ -> ext4, rootfs 1.3.0.12        142,377,060
0xA15E83C end (+4 bytes zero pad)
```

## 3. Slice and verify the payloads

Save this as `extract.py` next to `update.img`, then run it. It prints the
header identity, writes one file per record, and refuses a hash mismatch.

```python
#!/usr/bin/env python3
"""Slice the payload regions out of an AZ0x update.img and verify their hashes."""
import hashlib
import struct
from pathlib import Path

img = Path("update.img").read_bytes()
assert img[:4] == b"AZ0x", "not an AZ0x image"

def u32(o: int) -> int:
    return struct.unpack_from("<I", img, o)[0]

def u64(o: int) -> int:
    return struct.unpack_from("<Q", img, o)[0]

def cstr(o: int) -> str:
    while img[o] == 0:
        o += 1
    return img[o:img.index(b"\0", o)].decode()

count = struct.unpack_from("<H", img, 0x26)[0]
print(f"{cstr(u32(0x20))} | device {cstr(0x62)} | {cstr(0x38)} | {count} records")

slots, p = [], img.index(b"\0", 0x62) + 1
while p < 0x88 and img[p]:
    end = img.index(b"\0", p)
    slots.append(img[p:end].decode())
    p = end + 1
print("slots:", ", ".join(slots))

records, o = [], 0x90
for _ in range(count):
    magic = img[o:o + 4]
    if magic not in (b"BOOT", b"PARR", b"PART"):
        raise SystemExit(f"bad record magic at {o:#x}: {magic!r}")
    records.append((magic.decode(), u64(o + 8), u64(o + 16), img[o + 32:o + 64]))
    o += 64

for name, off, size, digest in records:
    blob = img[off:off + size]
    ok = hashlib.sha256(blob).digest() == digest
    out = Path(f"{name.lower()}-{off:x}.bin")
    out.write_bytes(blob)
    print(f"{out.name:<18} {size:>10,} B  sha256 {'ok' if ok else 'MISMATCH'}")
    if not ok:
        raise SystemExit(1)
```

Expected output:

```
MPC Sample image | device AC50 | SNAPSHOT-20260402131256 | 4 records
slots: recovery, boot, rootfs, akai-1
boot-298.bin        1,754,600 B  sha256 ok
parr-1ac880.bin    12,431,000 B  sha256 ok
part-d87718.bin    12,644,540 B  sha256 ok
part-19967d8.bin  142,377,060 B  sha256 ok
```

If you prefer not to script it, the same slices with standard tools
(`macOS`/Linux; note `dd bs=1` is unusably slow, so use `tail`/`head`):

```sh
tail -c +$((0x1AC880+1)) update.img | head -c 12431000  > parr.bin
tail -c +$((0xD87718+1)) update.img | head -c 12644540  > kernel.bin
tail -c +$((0x19967D8+1)) update.img | head -c 142377060 > rootfs.bin
```

## 4. Decode the payloads

`BOOT` is raw; the other three are XZ streams.

```sh
for f in *.bin; do
  case "$(file -b "$f")" in
    XZ*) xz -dc "$f" > "${f%.bin}.raw" ;;
    *)   cp "$f" "${f%.bin}.raw" ;;
  esac
done
file *.raw
```

Expected:

```
boot-298.raw:     Device Tree Blob version 17
parr-1ac880.raw:  Linux rev 1.0 ext2 filesystem data
part-d87718.raw:  Linux rev 1.0 ext2 filesystem data
part-19967d8.raw: Linux rev 1.0 ext4 filesystem data
```

Both ext2 images contain a single file, `kernel.fit`. The ext4 image is the
root filesystem.

`7z` can list and extract ext2/ext4 directly:

```sh
7z l part-19967d8.raw
```

`7z` may warn `There are data after the end of archive` on the ext4 image; that
is its parser reporting unused tail blocks, not corruption. On Linux you can
also `mount -o loop` or use `debugfs` when you need exact ownership, modes and
symlinks (7z extraction is fine for reading files out).

## 5. Extract the app

```sh
mkdir -p rootfs
7z x -y -orootfs part-19967d8.raw "usr/bin/MPC Sample"
```

The binary is at `usr/bin/MPC Sample` (yes, with a space). Verify it:

```sh
shasum -a 256 "rootfs/usr/bin/MPC Sample" | cut -c1-64
# b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508

head -c 4096 "rootfs/usr/bin/MPC Sample" | shasum -a 256 | cut -c1-64
# 44507c7a7d3029c411b2e3780ea286ca3e08b7e3ffd7b5047d52f73bf5299d1f
```

The second value is the `--firmware-sha256` the SDK documents for
`build-mod.sh --pack`. Then run the SDK's own table check:

```sh
python3 scripts/check-firmware-table.py "rootfs/usr/bin/MPC Sample"
# ok: 63 fingerprinted entries match
```

App facts:

| property | value |
|---|---|
| size | 119,243,440 B |
| format | ELF64 aarch64 PIE, dynamically linked, stripped |
| interpreter | `/usr/lib/ld-linux-aarch64.so.1` |
| BuildID | `1dac7f481a0fe8d128e435ace88d7e6aece0c9c9` |
| `.note.mpc.version` | `MPC 1.3.0.12 (40e46ca7991)` |
| full SHA-256 | `b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508` |

## 6. Firmware overview

The device is `AC50` / `az07`; this image is a full 1.3.0.12 update.

| record | offset | on-disk size | decoded | contents |
|---|---|---|---|---|
| `BOOT` | `0x298` | 1,754,600 | raw DTB | `bootupd for ac50` FIT config, compatible `inmusic,ac50` / `inmusic,az07`; 0xA44 used, rest zero padding |
| `PARR` | `0x1AC880` | 12,431,000 | ext2, 28,303,360 | `kernel.fit`, Linux `6.12.23-imb-2025-04-10-rt9`, built 2025-04-10 |
| `PART` | `0xD87718` | 12,644,540 | ext2, 28,577,792 | `kernel.fit`, Linux `6.12.60-imb-2025-12-03-rt13` PREEMPT_RT, built 2025-12-03 |
| `PART` | `0x19967D8` | 142,377,060 | ext4, 427,117,568 | root filesystem |

Payload hashes, from the record table:

| record | SHA-256 |
|---|---|
| `BOOT` | `d3b42e33ad0c1b5fdce12374448dcaf275ca1e4550f59045827a7b18377ed0ca` |
| `PARR` | `588690178c8eb990d53e2263c1ac28c60bc3aa7e2be914cc64a3b6cf09b8efbd` |
| `PART` @`0xD87718` | `d2409e515acf9bc1f21fd9594b4d44eb9a954b6eea73476654e2588bfc478532` |
| `PART` @`0x19967D8` | `3455f58e10d2e69665954f6cda9d7eab6dc18a4ed18d4ce4b6ca3aac1aa21731` |

Notes:

* The header lists four slot names — `recovery`, `boot`, `rootfs`, `akai-1` —
  but the records are positional and carry no name. Reading by content, the
  older April kernel is consistent with a recovery slot, the December kernel
  with the active boot slot, and the ext4 is `rootfs`.
* The rootfs is Yocto scarthgap: `az0x (inMusic Brands AZ0x base distribution)
  5.0.14` (`/usr/lib/os-release`), systemd, with the app at
  `/usr/bin/MPC Sample` and helpers such as `/usr/bin/test-app-launcher`.
* Both ext2 kernel slots hold only `kernel.fit`; the filesystem exists to give
  the bootloader a file to read, not a full userland.
* The image is signed: a 264-byte trailer (8-byte header + 256-byte blob) and
  the updater imports `BCryptVerifySignature` and carries the string
  `sha256,rsa2048`. This was not verified here; extraction is unaffected, but an
  image repacked with different bytes will fail the updater's check.

## 7. Pitfalls

* `dd bs=1 skip=...` on a 169 MB file takes minutes; use the script or
  `tail -c +N | head -c M`.
* `7z` reports the ext4 "data after end of archive" — normal.
* The SDK's table offsets are image-relative addresses. In the app's first
  `PT_LOAD` they equal file offsets, and every fingerprinted instruction is
  there; the vtables live in the second `PT_LOAD`, where `p_offset` and
  `p_vaddr` differ by `0x10000`. See `docs/17-firmware.md`.
* The app is stripped and its `.gnu_debuglink` names `MPC Sample`, but the
  debug file does not ship in the rootfs.
* Keep the extracted `update.img` and `.raw` files read-only; the flasher and
  the SDK expect the shipped bytes, not a re-saved copy.
