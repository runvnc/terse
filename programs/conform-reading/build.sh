#!/bin/sh
# Builds conform-reading.terse, the conformance image of the machine's
# reading profile (devices 2, 4 and 7), from programs/conform's sources
# with -DREADING: crt0.S, conform.c and cpu.S, linked with conform.ld.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; CLANG, LLD and OBJCOPY name the tools (defaults: clang,
# ld.lld, llvm-objcopy). crates/terse-cli/tests/conform.rs builds the image
# the same way and checks that conform-reading.terse is up to date.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
src="$here/../conform"
out=$(cd "${1:-$here}" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if [ -n "${TERSE:-}" ]; then
    terse() { "$TERSE" "$@"; }
else
    terse() { "${CARGO:-cargo}" run -q --manifest-path "$here/../../Cargo.toml" -p terse-cli --bin terse -- "$@"; }
fi

cd "$tmp"
cflags="--target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin -ffunction-sections -fdata-sections -DREADING"
n=0
for f in "$src/crt0.S" "$src/conform.c" "$src/cpu.S"; do
    "${CLANG:-clang}" $cflags -c "$f" -o "$n.o"
    n=$((n + 1))
done
"${LLD:-ld.lld}" -T "$src/conform.ld" --gc-sections 0.o 1.o 2.o -o conform.elf
"${OBJCOPY:-llvm-objcopy}" -O binary conform.elf conform.bin
terse pack --rom conform.bin --title conform-reading --ram-pages 16 --device 2:0 --device 4:0 --device 7:0 -o "$out/conform-reading.terse"
