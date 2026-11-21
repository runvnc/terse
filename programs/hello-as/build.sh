#!/bin/sh
# Builds hello.terse from hello.S and print.S with GNU as, ld.lld,
# llvm-objcopy and the workspace's `terse pack`.
#   ./build.sh [<out dir>]     default: this directory
# Each .S file is assembled on its own; the linker joins the objects.
# Set TERSE to a built `terse` binary to skip cargo. AS, LLD and OBJCOPY
# name the tools (defaults: riscv64-linux-gnu-as, ld.lld, llvm-objcopy).
# The riscv64 assembler makes 32-bit code with the -march/-mabi flags below.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
lib="$here/../lib"
out=$(cd "${1:-$here}" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if [ -n "${TERSE:-}" ]; then
    terse() { "$TERSE" "$@"; }
else
    terse() { "${CARGO:-$here/../../scripts/cargo}" run -q --manifest-path "$here/../../Cargo.toml" -p terse-cli --bin terse -- "$@"; }
fi

cd "$tmp"
asflags="-march=rv32imc -mabi=ilp32 -I $here"
"${AS:-riscv64-linux-gnu-as}" $asflags "$here/hello.S" -o hello.o
"${AS:-riscv64-linux-gnu-as}" $asflags "$here/print.S" -o print.o
"${LLD:-ld.lld}" -T "$lib/terse.ld" --gc-sections hello.o print.o -o hello.elf
"${OBJCOPY:-llvm-objcopy}" -O binary hello.elf hello.bin
terse pack --rom hello.bin --title hello-as --ram-pages 1 --device 2:0 -o "$out/hello.terse"
