#!/bin/sh
# Builds <name>.terse, <name> being this directory's name (stillwind; to
# rename the program, rename the directory, <name>.c and the test files):
# actor.py paints Ysolde's poses (scenes/actor.paths), story.py compiles
# the story (story/*.story) into story_data.h and each room's scene
# (scenes/*.paths, through paint.py) into a blob in blobs/, then `terse
# build` compiles <name>.c with it.
#   ./build.sh [<out dir>]     default: this directory
# The scenes are not in the image: the program fetches them by their
# hashes through the content device. The image lists them (--blob, SPEC
# §image.blobs), so a host that gets the image from a relay gets them with
# it; to play from the file, put them in the store the host uses (or on a
# relay the host asks), with the image:
#   terse put stillwind.terse blobs/*.scene --store <store> --relay none
# Set TERSE to a built `terse` binary to skip cargo, or CARGO if cargo is
# not on PATH; PYTHON the Python 3 to run story.py with (default python3).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$(cd "${1:-$here}" && pwd)
name=$(basename "$here")
if [ -n "${TERSE:-}" ]; then
    terse() { "$TERSE" "$@"; }
else
    terse() { "${CARGO:-cargo}" run -q --manifest-path "$here/../../Cargo.toml" -p terse-cli --bin terse -- "$@"; }
fi

"${PYTHON:-python3}" "$here/actor.py" -o "$here/scenes/actor.paths"
"${PYTHON:-python3}" "$here/story.py" "$here/story" -o "$here/story_data.h"
# The scenes' blobs, listed in the image (in file name order).
set --
for f in "$here"/blobs/*.scene; do
    set -- "$@" --blob "$f"
done
terse build "$here/$name.c" --title "$name" -o "$out/$name.terse" --ram-pages 28 \
    --device 2:0 --device 3:0 --device 7:0 --optional 4:0 "$@"
