#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Structural check for the built PE32+ EFI application."""

import struct
import sys
from pathlib import Path

def u16(b, o): return struct.unpack_from("<H", b, o)[0]
def u32(b, o): return struct.unpack_from("<I", b, o)[0]

def main(path: Path) -> int:
    b = path.read_bytes()
    if b[:2] != b"MZ":
        raise SystemExit("not a PE image: missing MZ")
    pe = u32(b, 0x3C)
    if b[pe:pe + 4] != b"PE\0\0":
        raise SystemExit("not a PE image: missing PE signature")
    machine = u16(b, pe + 4)
    nsect = u16(b, pe + 6)
    opt_size = u16(b, pe + 20)
    opt = pe + 24
    magic = u16(b, opt)
    subsystem = u16(b, opt + 68)
    if machine != 0x8664:
        raise SystemExit(f"wrong machine {machine:#x}, want x86-64")
    if magic != 0x20B:
        raise SystemExit(f"wrong optional-header magic {magic:#x}, want PE32+")
    if subsystem != 10:
        raise SystemExit(f"wrong subsystem {subsystem}, want EFI application (10)")
    sections = []
    sec = opt + opt_size
    for i in range(nsect):
        off = sec + i * 40
        name = b[off:off + 8].rstrip(b"\0").decode("ascii", "replace")
        sections.append(name)
    if ".reloc" not in sections:
        raise SystemExit("no .reloc section; EFI image is not relocatable")
    print(f"{path}: PE32+ EFI application, {len(b)} bytes, sections {sections}")
    return 0

if __name__ == "__main__":
    raise SystemExit(main(Path(sys.argv[1])))
