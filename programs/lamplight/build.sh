#!/bin/sh
# Builds lamplight.terse: story.py compiles the story (story/*.story) into
# story_data.h, then `terse build` compiles lamplight.c with it.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; CLANG and LLD name the tools; PYTHON the Python 3 to run
# story.py with (default python3). STORY names another story directory.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$(cd "${1:-$here}" && pwd)
if [ -n "${TERSE:-}" ]; then
    terse() { "$TERSE" "$@"; }
else
    terse() { "${CARGO:-cargo}" run -q --manifest-path "$here/../../Cargo.toml" -p terse-cli --bin terse -- "$@"; }
fi

"${PYTHON:-python3}" "$here/story.py" "${STORY:-$here/story}" -o "$here/story_data.h"
terse build "$here/lamplight.c" --title lamplight -o "$out/lamplight.terse" --ram-pages 9 \
    --device 2:0 --device 3:0 --optional 4:0 --optional 6:0
