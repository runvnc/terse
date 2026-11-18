#!/bin/sh
# Regenerates conform.terse from conform.asm using the workspace's `terse` tool.
# Set CARGO if cargo is not on PATH (e.g. CARGO=~/.cargo/bin/cargo).
set -eu
cd "$(dirname "$0")"
terse() { "${CARGO:-cargo}" run -q -p terse-cli --bin terse -- "$@"; }
bin=conform.bin
trap 'rm -f "$bin"' EXIT
terse asm conform.asm -o "$bin"
terse pack --rom "$bin" --title conform --ram-pages 16 --device 2:0 --device 3:0 --device 4:0 -o conform.terse
