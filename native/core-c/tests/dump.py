#!/usr/bin/env python3
"""Writes <dir>/dump.terse: an image (device 2) whose first frame puts, with
`cells_write`, the cells that STATE.md 6.3's dump rules are about: a wide
pair's continuation (skipped), a continuation after a continuation and one
in column 0 (spaces), control codes (as they are) and a surrogate code
(U+FFFD). Then it waits forever. `--dump` of it must match `terse run`'s."""
import os
import struct
import sys


def addi(rd, rs1, imm):
    return (imm & 0xFFF) << 20 | rs1 << 15 | rd << 7 | 0x13


def lui(rd, imm20):
    return imm20 << 12 | rd << 7 | 0x37


def jal0(off):
    i = off & 0x1FFFFF
    return (i >> 20 & 1) << 31 | (i >> 1 & 0x3FF) << 21 | (i >> 11 & 1) << 20 | (i >> 12 & 0xFF) << 12 | 0x6F


A0, A1, A2, A7 = 10, 11, 12, 17
code = [lui(A1, 0x10), addi(A1, A1, 36), addi(A0, 0, 0), addi(A2, 0, 82), addi(A7, 0, 0x207), 0x73,
        addi(A7, 0, 0x100), 0x73, jal0(-8)]
cells = [0x41, 0xFFFF, 0xFFFF, 0x01, 0xD800, 0x4F60, 0xFFFF, 0x85] + [0x20] * 72 + [0xFFFF, 0x43]
rom = b"".join(struct.pack("<I", c) for c in code) + b"".join(struct.pack("<HBB", c, 7, 0) for c in cells)
assert len(code) * 4 == 36
header = b"TERS" + bytes([0, 1]) + len(rom).to_bytes(3, "little") + bytes([1, 1]) + bytes(5) + b"dump".ljust(16, b"\0")
os.makedirs(sys.argv[1], exist_ok=True)
with open(os.path.join(sys.argv[1], "dump.terse"), "wb") as f:
    f.write(header + bytes([2, 0]) + rom)
