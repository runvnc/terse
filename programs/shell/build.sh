#!/bin/sh
# Builds shell.terse from shell.c and programs/lib with
# clang (riscv32), ld.lld, llvm-objcopy and the workspace's `terse pack`.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; CLANG, LLD and OBJCOPY name the tools (defaults: clang,
# ld.lld, llvm-objcopy). The flags and the order of the objects are those
# of crates/terse-cli/tests/common/mod.rs, which checks that shell.terse is
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
    terse() { "${CARGO:-cargo}" run -q --manifest-path "$here/../../Cargo.toml" -p terse-cli --bin terse -- "$@"; }
fi

cd "$tmp"
cflags="--target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin -ffunction-sections -fdata-sections"
n=0
for src in "$lib/crt0.S" "$here/shell.c" "$lib/terse.c"; do
    "${CLANG:-clang}" $cflags -I "$lib" -c "$src" -o "$n.o"
    n=$((n + 1))
done
"${LLD:-ld.lld}" -T "$lib/terse.ld" --gc-sections 0.o 1.o 2.o -o shell.elf
"${OBJCOPY:-llvm-objcopy}" -O binary shell.elf shell.bin
terse pack --rom shell.bin --title shell --ram-pages 3 --device 2:0 --device 3:0 --device 4:0 --device 7:0 --device 8:0 -o "$out/shell.terse"
