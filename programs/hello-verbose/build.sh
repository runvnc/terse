#!/bin/sh
# Builds hello.terse: verbose.py translates the sources (and the include
# files) to plain assembly, then GNU as, ld.lld, llvm-objcopy and the
# workspace's `terse pack`.
#   ./build.sh [<out dir>]     default: this directory
# KEEP=<dir> also keeps the translated files there, to read what as sees.
# Set TERSE to a built `terse` binary to skip cargo. AS, LLD and OBJCOPY
# name the tools (defaults: riscv64-linux-gnu-as, ld.lld, llvm-objcopy).
set -eu
here=$(cd "$(dirname "$0")" && pwd)

# --- What to build ---------------------------------------------------------
sources="hello.asm print.asm"           # each one becomes an object file
includes="terse.inc"                    # pulled in with .include
translator="$here/verbose.py"
linker_script="$here/../lib/terse.ld"
output="hello.terse"
title="hello-verbose"                   # at most 16 bytes
ram_pages=1
devices="--device 2:0"                  # 2: terminal (1: system, always there)
march="rv32imc"
mabi="ilp32"
# ---------------------------------------------------------------------------

out=$(cd "${1:-$here}" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if [ -n "${TERSE:-}" ]; then
    terse() { "$TERSE" "$@"; }
else
    terse() { "${CARGO:-$here/../../scripts/cargo}" run -q --manifest-path "$here/../../Cargo.toml" -p terse-cli --bin terse -- "$@"; }
fi

cd "$tmp"
inputs=""
for f in $includes $sources; do inputs="$inputs $here/$f"; done
python3 "$translator" -o src $inputs
if [ -n "${KEEP:-}" ]; then mkdir -p "$KEEP" && cp src/* "$KEEP"/; fi

objects=""
for f in $sources; do
    name=${f%.asm}
    "${AS:-riscv64-linux-gnu-as}" -march=$march -mabi=$mabi -I src "src/$name.s" -o "$name.o"
    objects="$objects $name.o"
done
"${LLD:-ld.lld}" -T "$linker_script" --gc-sections $objects -o out.elf
"${OBJCOPY:-llvm-objcopy}" -O binary out.elf out.bin
terse pack --rom out.bin --title "$title" --ram-pages $ram_pages $devices -o "$out/$output"
