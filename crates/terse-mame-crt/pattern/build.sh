#!/bin/sh
# Builds pattern.terse and text.terse from pattern.c and text.c and programs/lib with clang (riscv32
# target), ld.lld, llvm-objcopy and the workspace's `terse pack`, as
# programs/sketch/build.sh does.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo; CLANG, LLD and OBJCOPY
# name the tools.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
lib="$here/../../../programs/lib"
out=$(cd "${1:-$here}" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if [ -n "${TERSE:-}" ]; then
    terse() { "$TERSE" "$@"; }
else
    terse() { "$here/../../../scripts/cargo" run -q --manifest-path "$here/../../../Cargo.toml" -p terse-cli --bin terse -- "$@"; }
fi

cd "$tmp"
cflags="--target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin
    -ffunction-sections -fdata-sections -I $lib"
"${CLANG:-clang}" $cflags -c "$lib/crt0.S" -o 0.o
"${CLANG:-clang}" $cflags -c "$lib/terse.c" -o 2.o
for name in pattern text; do
    "${CLANG:-clang}" $cflags -c "$here/$name.c" -o 1.o
    "${LLD:-ld.lld}" -T "$lib/terse.ld" --gc-sections 0.o 1.o 2.o -o out.elf
    "${OBJCOPY:-llvm-objcopy}" -O binary out.elf $name.bin
    terse pack --rom $name.bin --title $name --ram-pages 1 --device 2:0 --device 3:0 -o "$out/$name.terse"
done
