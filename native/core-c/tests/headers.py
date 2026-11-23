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

def words(*ws):
    return b"".join(w.to_bytes(4, "little") for w in ws)

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
    # SPEC §profile: ROM up to 262,144 bytes, RAM up to 96 pages.
    "ok-rom-max": image([], rom=ROM.ljust(262144, b"\0")),
    "bad-rom-over": image([], rom=ROM.ljust(262145, b"\0")),
    "ok-pages-96": image([], pages=96),
    "bad-pages-97": image([], pages=97),
    "bad-pages-112": image([], pages=112),
    "bad-pages-128": image([], pages=128),
    # sp starts at the end of RAM (001C_0000h for 96 pages): its last byte
    # is written and read back (exit 5Ah); the byte after faults.
    "ok-ram-last": image([], pages=96, rom=words(0x05A00513, 0xFEA10FA3, 0xFFF14503, 0x10600893, 0x73)),
    "fault-ram-past": image([], pages=96, rom=words(0x05A00513, 0x00A10023, 0x10600893, 0x73)),
}
for name, data in CASES.items():
    with open(os.path.join(out, name + ".terse"), "wb") as f:
        f.write(data)
