#!/bin/sh
# Regenerates hello.terse from hello.asm using the workspace's `terse` tool.
# Set CARGO if cargo is not on PATH (e.g. CARGO=~/.cargo/bin/cargo).
set -eu
cd "$(dirname "$0")"
terse() { "${CARGO:-cargo}" run -q -p terse-cli --bin terse -- "$@"; }
bin=hello.bin
trap 'rm -f "$bin"' EXIT
terse asm hello.asm -o "$bin"
terse pack --rom "$bin" --title hello --ram-pages 4 --device 2:0 -o hello.terse
