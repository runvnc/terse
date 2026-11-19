#!/bin/sh
# Builds hostile.terse from hostile.S (as programs/hello/build.sh does).
#   ./build.sh [<out dir>]     default: this directory
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
cflags="--target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin -ffunction-sections -fdata-sections"
"${CLANG:-clang}" $cflags -I "$lib" -c "$here/hostile.S" -o 0.o
"${LLD:-ld.lld}" -T "$lib/terse.ld" --gc-sections 0.o -o hostile.elf
"${OBJCOPY:-llvm-objcopy}" -O binary hostile.elf hostile.bin
terse pack --rom hostile.bin --title hostile --ram-pages 2 --device 2:0 --device 3:0 --device 7:0 -o "$out/hostile.terse"
