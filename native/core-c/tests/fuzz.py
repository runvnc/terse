#!/usr/bin/env python3
"""Writes N random images (no devices, 1 RAM page) for comparing CPUs:
random 16-bit parcels and random encodings of every RV32IMC 32-bit format,
with a prologue that points s0 at RAM so that many loads and stores land.
Usage: fuzz.py <out dir> <N> [seed]. Deterministic for a seed."""
import os, random, struct, sys

out, n = sys.argv[1], int(sys.argv[2])
rng = random.Random(int(sys.argv[3]) if len(sys.argv) > 3 else 1)
os.makedirs(out, exist_ok=True)

def r(): return rng.choice([1, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0, 31])

def ins32():
    k = rng.random()
    rd, a, b = r(), r(), r()
    if k < 0.30:   # OP and M, mostly valid
        f3 = rng.randrange(8)
        f7 = rng.choice([0, 0, 1, 1, 0x20 if f3 in (0, 5) else 0, rng.randrange(128) if rng.random() < 0.1 else 1])
        return f7 << 25 | b << 20 | a << 15 | f3 << 12 | rd << 7 | 0x33
    if k < 0.50:   # OP-IMM, shifts mostly valid
        f3 = rng.randrange(8)
        imm = rng.randrange(1 << 12)
        if f3 == 1 or (f3 == 5 and rng.random() < 0.9):
            imm = (imm & 31) | (0x400 if f3 == 5 and rng.random() < 0.5 else 0)
        return imm << 20 | a << 15 | f3 << 12 | rd << 7 | 0x13
    if k < 0.62:   # loads, stores from s0 (x8) or anywhere
        base = 8 if rng.random() < 0.8 else a
        off = rng.randrange(-64, 2048)
        f3 = rng.choice([0, 1, 2, 4, 5]) if rng.random() < 0.9 else rng.randrange(8)
        if rng.random() < 0.5:
            return (off & 0xFFF) << 20 | base << 15 | f3 << 12 | rd << 7 | 0x03
        o = off & 0xFFF
        f3 = f3 % 3 if rng.random() < 0.9 else f3
        return (o >> 5) << 25 | b << 20 | base << 15 | f3 << 12 | (o & 31) << 7 | 0x23
    if k < 0.70:   # short forward branches
        off = rng.randrange(1, 16) * 2
        return (off >> 5 & 0x3F) << 25 | b << 20 | a << 15 | rng.choice([0, 1, 4, 5, 6, 7, 2]) << 12 | ((off >> 1 & 0xF) << 1 | (off >> 11 & 1)) << 7 | 0x63
    if k < 0.75:   # LUI, AUIPC
        return rng.randrange(1 << 20) << 12 | rd << 7 | rng.choice([0x37, 0x17])
    if k < 0.80 or k >= 0.86:   # JAL forward
        off = rng.randrange(1, 16) * 2
        return ((off >> 1 & 0x3FF) << 21) | rd << 7 | 0x6F
    if k < 0.83:   # MISC-MEM, SYSTEM, JALR
        return rng.randrange(1 << 25) << 7 | rng.choice([0x0F, 0x73, 0x67])
    return rng.randrange(1 << 32) | 3  # anything with low bits 11

def ins16():   # mostly from the quadrants and functions that have valid forms
    while True:
        v = rng.randrange(1 << 16)
        q, f3 = v & 3, v >> 13
        if q == 3:
            continue
        if rng.random() < 0.1 or (q == 0 and f3 in (0, 2, 6)) or q == 1 or (q == 2 and f3 in (0, 2, 4, 6)):
            if q == 2 and f3 in (0,) or (q == 1 and f3 == 4 and v >> 10 & 3 < 2):
                v &= ~0x1000 if rng.random() < 0.9 else 0xFFFF    # RV32 shift amounts
            return v

for i in range(n):
    rom = bytearray(struct.pack("<I", 0x00100437))   # lui s0, 0x100
    while len(rom) < rng.randrange(64, 600):
        if rng.random() < 0.45:
            rom += struct.pack("<H", ins16())
        else:
            rom += struct.pack("<I", ins32())
    if rng.random() < 0.5:
        rom += struct.pack("<II", 0x10600893, 0x00000073)  # li a7, 0x106; ecall
    hdr = b"TERS" + bytes([0, 1]) + len(rom).to_bytes(3, "little") + bytes([1, 0]) + bytes(5)
    hdr += ("fuzz%d" % i).encode().ljust(16, b"\0")
    with open(os.path.join(out, "fuzz%04d.terse" % i), "wb") as f:
        f.write(hdr + rom)
