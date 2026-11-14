#!/usr/bin/env python3
"""Check every fingerprinted entry of src/firmware.rs against a firmware binary.

Usage: check-firmware-table.py <MPC Sample binary> [src/firmware.rs]

The table's offsets are file offsets into `usr/bin/MPC Sample` (1.3.0.12), so
each entry's instruction word is read straight from the file. Exits 1 on the
first mismatch so a firmware bump fails loudly before any hook is installed.
"""

import re
import struct
import sys
from pathlib import Path

NAMED_WORDS = {"PACIASP": 0xD503233F, "BTI_C": 0xD503245F}
ENTRY = re.compile(r"(hook|call|probe)\((0x[0-9a-fA-F_]+),\s*(0x[0-9a-fA-F_]+|PACIASP|BTI_C)\)")


def word_value(text: str) -> int:
    return NAMED_WORDS.get(text) or int(text.replace("_", ""), 16)


def main() -> int:
    binary = Path(sys.argv[1]).read_bytes()
    source = Path(sys.argv[2] if len(sys.argv) > 2 else "src/firmware.rs").read_text()
    entries = ENTRY.findall(source)
    for kind, offset_text, word_text in entries:
        offset = int(offset_text.replace("_", ""), 16)
        expected = word_value(word_text)
        found = struct.unpack_from("<I", binary, offset)[0]
        if found != expected:
            print(f"MISMATCH {kind} {offset:#x}: expected {expected:#010x}, found {found:#010x}")
            return 1
    print(f"ok: {len(entries)} fingerprinted entries match")
    return 0


if __name__ == "__main__":
    sys.exit(main())
