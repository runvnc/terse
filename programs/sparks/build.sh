#!/bin/sh
# Builds sparks.terse from sparks.c and programs/lib with SDCC (4.x, z80
# port) and the workspace's `terse pack`.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH (e.g. CARGO=~/.cargo/bin/cargo).
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
sdasz80 -g -o crt0.rel "$lib/crt0.s"
cflags="-mz80 --std-c11 --opt-code-size -I$lib"
sdcc $cflags -c "$lib/terse.c" -o terse.rel
sdcc $cflags -c "$here/sparks.c" -o sparks.rel
# Code from 0100h after crt0's vectors; data in RAM from 8000h (crt0.s has
# the memory map).
sdcc -mz80 --no-std-crt0 --nostdlib --code-loc 0x0100 --data-loc 0x8000 \
    -L /usr/share/sdcc/lib/z80 -l z80 \
    crt0.rel sparks.rel terse.rel -o sparks.ihx
makebin -p sparks.ihx sparks.bin
size=$(wc -c < sparks.bin)
if [ "$size" -gt 32768 ]; then
    echo "sparks.bin is $size bytes; it must end below 8000h" >&2
    exit 1
fi
terse pack --rom sparks.bin --title sparks --ram-pages 4 --device 2:0 --device 3:0 -o "$out/sparks.terse"
