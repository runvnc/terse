#!/usr/bin/env python3
"""Writes images with unusual headers (SPEC §image) to <out dir>: some a
host must refuse, some it must run. The ROM exits with code 9."""
import os, sys

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
ROM = bytes.fromhex("1345900093086010" "73000000")  # li a0, 9; li a7, 0x106; ecall

def image(devs, rom=ROM, version=0, profile=1, pages=1, reserved=bytes(5), extra=b""):
    h = b"TERS" + bytes([version, profile]) + len(rom).to_bytes(3, "little") + bytes([pages, len(devs)])
    h += reserved + b"t".ljust(16, b"\0")
    for s, v in devs:
        h += bytes([s, v])
    return h + rom + extra

CASES = {
    "ok-none": image([]),
    "ok-system-listed": image([(1, 0)]),
    "ok-optional-11": image([(11, 0x80)]),          # no such device: not granted
    "ok-optional-5": image([(5, 0x80)]),
    "ok-optional-2-v3": image([(2, 0x83)]),         # a version no host has
    "ok-required-8": image([(8, 0)]),               # the shell's, absent elsewhere
    "bad-required-5": image([(5, 0)]),
    "bad-required-0": image([(0, 0)]),
    "bad-required-2-v1": image([(2, 1)]),
    "bad-system-v1": image([(1, 1)]),
    "bad-optional-1": image([(1, 0x80)]),
    "bad-optional-8": image([(8, 0x80)]),
    "bad-twice": image([(2, 0), (2, 0x80)]),
    "bad-reserved": image([], reserved=bytes([0, 0, 1, 0, 0])),
    "bad-long": image([], extra=b"\0"),
    "bad-profile": image([], profile=2),
    "bad-pages": image([], pages=0),
}
for name, data in CASES.items():
    with open(os.path.join(out, name + ".terse"), "wb") as f:
        f.write(data)
