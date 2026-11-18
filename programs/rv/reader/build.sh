#!/bin/sh
# Builds reader.terse (profile 2) from reader.c and programs/rv/lib with
# clang (riscv32), ld.lld, llvm-objcopy and the workspace's `terse pack`.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; CLANG, LLD and OBJCOPY name the tools (defaults: clang,
# ld.lld, llvm-objcopy). The flags and the order of the objects are those
# of crates/terse-rv/tests/common/mod.rs, which checks that reader.terse is
# up to date.
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
cflags="--target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin -ffunction-sections -fdata-sections"
n=0
for src in "$lib/crt0.S" "$here/reader.c" "$lib/terse.c"; do
    "${CLANG:-clang}" $cflags -I "$lib" -c "$src" -o "$n.o"
    n=$((n + 1))
done
"${LLD:-ld.lld}" -T "$lib/terse.ld" --gc-sections 0.o 1.o 2.o -o reader.elf
"${OBJCOPY:-llvm-objcopy}" -O binary reader.elf reader.bin
terse pack --profile 2 --rom reader.bin --title reader --ram-pages 10 --device 2:0 --device 4:0 -o "$out/reader.terse"
