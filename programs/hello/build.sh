#!/bin/sh
# Builds hello.terse from hello.S with clang (riscv32 target), ld.lld,
# llvm-objcopy and the workspace's `terse pack`.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; CLANG, LLD and OBJCOPY name the tools (defaults: clang,
# ld.lld, llvm-objcopy). The flags are those of
# crates/terse-cli/tests/common/mod.rs, which checks that hello.terse is up
# to date.
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
"${CLANG:-clang}" $cflags -I "$lib" -c "$here/hello.S" -o 0.o
"${LLD:-ld.lld}" -T "$lib/terse.ld" --gc-sections 0.o -o hello.elf
"${OBJCOPY:-llvm-objcopy}" -O binary hello.elf hello.bin
terse pack --rom hello.bin --title hello --ram-pages 1 --device 2:0 --optional 10:0 -o "$out/hello.terse"
