#!/bin/sh
# Builds tune.terse from tune.c and programs/lib with
# clang (riscv32 target), ld.lld, llvm-objcopy and the workspace's
# `terse pack`.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; CLANG, LLD and OBJCOPY name the tools. The flags and the
# order of the objects are those of crates/terse-cli/tests/common/mod.rs
# (`build_rom`), which checks that tune.terse is up to date.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
lib="$here/../lib"
out=$(cd "${1:-$here}" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if [ -n "${TERSE:-}" ]; then
    terse() { "$TERSE" "$@"; }
else
    terse() { "${CARGO:-cargo}" run -q --manifest-path "$here/../../Cargo.toml" -p terse-cli --bin terse -- "$@"; }
fi

cd "$tmp"
cflags="--target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin
    -ffunction-sections -fdata-sections -I $lib"
"${CLANG:-clang}" $cflags -c "$lib/crt0.S" -o 0.o
"${CLANG:-clang}" $cflags -c "$here/tune.c" -o 1.o
"${CLANG:-clang}" $cflags -c "$lib/terse.c" -o 2.o
"${LLD:-ld.lld}" -T "$lib/terse.ld" --gc-sections 0.o 1.o 2.o -o out.elf
"${OBJCOPY:-llvm-objcopy}" -O binary out.elf tune.bin
terse pack --rom tune.bin --title tune --ram-pages 1 --device 2:0 --device 6:0 -o "$out/tune.terse"
