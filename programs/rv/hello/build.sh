#!/bin/sh
# Builds hello.terse (profile 2) from hello.S with clang's RISC-V assembler,
# ld.lld, llvm-objcopy and the workspace's `terse pack`.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; CLANG, LLD and OBJCOPY name the tools (defaults: clang,
# ld.lld, llvm-objcopy).
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
"${CLANG:-clang}" --target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -c "$here/hello.S" -o hello.o
"${LLD:-ld.lld}" -T "$lib/terse.ld" --gc-sections hello.o -o hello.elf
"${OBJCOPY:-llvm-objcopy}" -O binary hello.elf hello.bin
terse pack --profile 2 --rom hello.bin --title hello --ram-pages 4 --device 2:0 -o "$out/hello.terse"
