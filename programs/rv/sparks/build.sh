#!/bin/sh
# Builds sparks.terse (profile 2) from sparks.c and programs/rv/lib with
# clang (riscv32 target), ld.lld, llvm-objcopy and the workspace's
# `terse pack`.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; CLANG, LLD and OBJCOPY name the tools. The flags and the
# order of the objects must stay those of crates/terse-rv/tests/common
# (`build`), which checks that sparks.terse is up to date.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
lib="$here/../lib"
out=$(cd "${1:-$here}" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if [ -n "${TERSE:-}" ]; then
    terse() { "$TERSE" "$@"; }
else
    terse() { (cd "$here" && "${CARGO:-cargo}" run -q -p terse-cli --bin terse -- "$@"); }
fi

cd "$tmp"
cflags="--target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin
    -ffunction-sections -fdata-sections -I $lib"
"${CLANG:-clang}" $cflags -c "$lib/crt0.S" -o 0.o
"${CLANG:-clang}" $cflags -c "$here/sparks.c" -o 1.o
"${CLANG:-clang}" $cflags -c "$lib/terse.c" -o 2.o
"${LLD:-ld.lld}" -T "$lib/terse.ld" --gc-sections 0.o 1.o 2.o -o out.elf
"${OBJCOPY:-llvm-objcopy}" -O binary out.elf sparks.bin
terse pack --profile 2 --rom sparks.bin --title sparks --ram-pages 4 --device 2:0 --device 3:0 -o "$out/sparks.terse"
