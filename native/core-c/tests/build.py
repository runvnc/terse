#!/usr/bin/env python3
"""Builds the test images of this directory: each X.S, plus the one-line
fault and CPU cases below, assembled with clang and packed by `terse pack`.
Usage: build.py <terse binary>.  Needs clang, ld.lld, llvm-objcopy."""
import os, subprocess, sys, tempfile

here = os.path.dirname(os.path.abspath(__file__))
terse = sys.argv[1]
ONE_LINE = {
    "illegal": ".half 0",                      # c.unimp
    "ebreak": "ebreak",
    "cebreak": "c.ebreak",
    "load": "lw a0, 0(zero)",
    "storerom": "lui a0, 0x10; sw a0, 0(a0)",
    "misaligned": "lw a0, 2(s0)",
    "misout": "lw a0, 2(zero)",                # misaligned and outside memory
    "fetch": "lui a0, 0x200; jr a0",
    "csr": "csrr a0, cycle",
    "wide": ".half 0x003f; .half 0; .half 0",   # a 48-bit encoding
    "fencei": "fence.i",
    "div": "li a0, -1; li a1, 0; div a2, a0, a1; rem a3, a0, a1; divu t0, a0, a1; remu t1, a0, a1;"
           " lui a4, 0x80000; li a5, -1; div a6, a4, a5; rem a7, a4, a5;"
           " mulh t2, a4, a5; mulhsu t3, a4, a5; mulhu t4, a4, a5; ebreak",
    "hint": "c.addi x0, 5; c.slli x0, 1; slli x0, x0, 3; c.li x0, 3; c.mv x0, a0; fence; fence.tso;"
            " pause; li a0, 7; li a7, 0x106; ecall",
    "jalrodd": "la a0, 1f; addi a0, a0, 1; jalr a1, 0(a0); 1: li a7, 0x106; ecall",
    "exit": "li a0, 0x1FF; li a7, 0x106; ecall",
    "spin": "1: addi t0, t0, 1; xor t1, t1, t0; j 1b",   # for measuring speed
}
for n in range(62494, 62499):   # exit with 1 to 5 units left: the budget after it
    ONE_LINE["exitneg%d" % (n - 62494)] = "li t0, %d; 1: addi t0, t0, -1; bnez t0, 1b; li a0, 1; li a7, 0x106; ecall" % n
DEVICES = {"opt": ["--optional", "2:0", "--optional", "4:0", "--optional", "10:0"]}
FLAGS = ["--target=riscv32-unknown-elf", "-march=rv32imc", "-mabi=ilp32", "-c", "-I", here]

with tempfile.TemporaryDirectory() as tmp:
    srcs = {n[:-2]: os.path.join(here, n) for n in os.listdir(here) if n.endswith(".S")}
    for n, body in ONE_LINE.items():
        p = os.path.join(tmp, n + ".S")
        with open(p, "w") as f:
            f.write('#include "common.inc"\n    START\n    %s\n' % body)
        srcs[n] = p
    ld = os.path.join(tmp, "t.ld")
    with open(ld, "w") as f:
        f.write("SECTIONS { . = 0x10000; .text : { *(.text.start) *(.text*) *(.rodata*) } }\n")
    for n, src in sorted(srcs.items()):
        o, elf, bin_ = (os.path.join(tmp, n + e) for e in (".o", ".elf", ".bin"))
        subprocess.run(["clang", *FLAGS, src, "-o", o], check=True)
        subprocess.run(["ld.lld", "-T", ld, o, "-o", elf], check=True)
        subprocess.run(["llvm-objcopy", "-O", "binary", elf, bin_], check=True)
        subprocess.run([terse, "pack", "--rom", bin_, "--title", n, "--ram-pages", "1",
                        *DEVICES.get(n, []), "-o", os.path.join(here, n + ".terse")],
                       check=True, stdout=subprocess.DEVNULL)
