#!/bin/sh
# Builds painted-test.terse: actor.py paints the traveller's poses
# (actor.paths), scene.py compiles the painted scene (scene.paths, which
# includes them) into scene_data.h, then `terse build` compiles
# painted-test.c with it.
#   ./build.sh [<out dir>]     default: this directory
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; PYTHON the Python 3 to run scene.py with (default python3).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$(cd "${1:-$here}" && pwd)
if [ -n "${TERSE:-}" ]; then
    terse() { "$TERSE" "$@"; }
else
    terse() { "${CARGO:-cargo}" run -q --manifest-path "$here/../../Cargo.toml" -p terse-cli --bin terse -- "$@"; }
fi

"${PYTHON:-python3}" "$here/actor.py" -o "$here/actor.paths"
"${PYTHON:-python3}" "$here/scene.py" "$here/scene.paths" -o "$here/scene_data.h"
terse build "$here/painted-test.c" --title painted-test -o "$out/painted-test.terse" --ram-pages 2 \
    --device 2:0 --device 3:0 --device 7:0
