#!/usr/bin/env python3
"""Check every fingerprinted entry of the firmware profiles against a binary.

Usage: check-firmware-table.py <MPC Sample binary> [--fw 1.3|1.4]

Without `--fw`, every profile is tried and the ones that match are reported.
The table's offsets are image-relative addresses; every fingerprinted
instruction lives in the app's first PT_LOAD, where they equal file offsets,
so the binary can be read as it is on disk.
"""

import argparse
import re
import struct
import sys
from pathlib import Path

NAMED_WORDS = {"PACIASP": 0xD503233F, "BTI_C": 0xD503245F}
ENTRY = re.compile(r"(hook|call|probe)\((0x[0-9a-fA-F_]+),\s*(0x[0-9a-fA-F_]+|PACIASP|BTI_C)\)")
PROFILES = {"1.3": "v1_3.rs", "1.4": "v1_4.rs"}


def word_value(text: str) -> int:
    return NAMED_WORDS.get(text) or int(text.replace("_", ""), 16)


def entries(source: Path):
    return [(kind, int(off.replace("_", ""), 16), word_value(word))
            for kind, off, word in ENTRY.findall(source.read_text())]


def check(binary: bytes, table: Path):
    bad = []
    entries_ = entries(table)
    for kind, offset, expected in entries_:
        found = struct.unpack_from("<I", binary, offset)[0]
        if found != expected:
            bad.append((kind, offset, expected, found))
    return entries_, bad


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--fw", choices=sorted(PROFILES))
    parser.add_argument("--source", default="src/firmware")
    args = parser.parse_args()

    binary = Path(args.binary).read_bytes()
    root = Path(args.source)
    wanted = [args.fw] if args.fw else sorted(PROFILES)

    matched = []
    for fw in wanted:
        entries_, bad = check(binary, root / PROFILES[fw])
        if not bad:
            print(f"ok: firmware {fw} ({len(entries_)} entries match)")
            matched.append(fw)
        else:
            kind, offset, expected, found = bad[0]
            print(f"firmware {fw}: MISMATCH {kind} {offset:#x}: "
                  f"expected {expected:#010x}, found {found:#010x} "
                  f"({len(bad)} of {len(entries_)} differ)")
    return 0 if matched else 1


if __name__ == "__main__":
    sys.exit(main())
