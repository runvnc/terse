#!/bin/sh
# Builds conform.terse from crt0.S, conform.c and cpu.S with clang
# (riscv32), ld.lld, llvm-objcopy and the workspace's `terse pack`.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; CLANG, LLD and OBJCOPY name the tools (defaults: clang,
# ld.lld, llvm-objcopy). crates/terse-cli/tests/conform.rs builds the image
# the same way and checks that conform.terse is up to date.
# The image is self-contained (its own crt0 and linker script, no
# programs/lib), needs devices 2, 3, 4 and 7, and 16 RAM pages (RAM_PAGES in
# conform.c).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$(cd "${1:-$here}" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if [ -n "${TERSE:-}" ]; then
    terse() { "$TERSE" "$@"; }
else
    terse() { "${CARGO:-cargo}" run -q --manifest-path "$here/../../Cargo.toml" -p terse-cli --bin terse -- "$@"; }
fi

cd "$tmp"
cflags="--target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin -ffunction-sections -fdata-sections"
n=0
for src in "$here/crt0.S" "$here/conform.c" "$here/cpu.S"; do
    "${CLANG:-clang}" $cflags -c "$src" -o "$n.o"
    n=$((n + 1))
done
"${LLD:-ld.lld}" -T "$here/conform.ld" --gc-sections 0.o 1.o 2.o -o conform.elf
"${OBJCOPY:-llvm-objcopy}" -O binary conform.elf conform.bin
terse pack --rom conform.bin --title conform --ram-pages 16 --device 2:0 --device 3:0 --device 4:0 --device 7:0 -o "$out/conform.terse"
