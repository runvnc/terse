#!/bin/sh
# Pictures of MAME's pipeline (terse-dev-mame) and of the reference tube
# (terse-bus's own, with the reference drivers), the same frames at the
# same sizes, for comparison:
#   crates/terse-mame-crt/pictures.sh <out dir>
# Needs a release build (scripts/cargo build --release -p terse-bus
# -p terse-devices -p terse-mame-crt) and ImageMagick (`magick`) for the
# side-by-side and strip pictures. Runs with scratch store, configuration
# and data directories and no relay: never the user's.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
rel="$root/target/release"
out=$(mkdir -p "$1" && cd "$1" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/store" "$tmp/config" "$tmp/data"
P="$root/programs"
PT="$here/pattern"

# shot <mame|mame100|ours> <name> <WxH> <image> [args...]
shot() {
    mode=$1; name=$2; size=$3; img=$4; shift 4
    case $mode in
        mame) set -- "$@" --driver "3,9=$rel/terse-dev-mame" --driver "2=$rel/terse-dev-mame" ;;
        mame100) set -- "$@" --driver "3,9=$rel/terse-dev-mame" --driver "2=$rel/terse-dev-mame"; export TERSE_MAME_PERSISTENCE=100 ;;
    esac
    "$rel/terse-bus" "$img" --store "$tmp/store" --config "$tmp/config" --data "$tmp/data" --relay none \
        --screenshot "$out/$mode-$name.png" --size "$size" "$@" 2>>"$tmp/log"
    unset TERSE_MAME_PERSISTENCE
}

for m in mame ours; do
    shot $m pattern-640 640x480 "$PT/pattern.terse" --frames 200
    shot $m pattern-1280 1280x960 "$PT/pattern.terse" --frames 200
    shot $m sparks-640 640x480 "$P/sparks/sparks.terse" --events "$P/sparks/sessions/phosphor.txt" --frames 300 --without 10
    shot $m sparks-1280 1280x960 "$P/sparks/sparks.terse" --events "$P/sparks/sessions/phosphor.txt" --frames 300 --without 10
    shot $m hello-640 640x480 "$P/hello/hello.terse" --events "$P/hello/hello.events" --frames 40 --force-phosphor --without 10
    shot $m holo-640 640x480 "$P/holo/holo.terse" --frames 120 --force-phosphor
    for s in 640x480 1280x960 1920x1440; do
        shot $m text-${s%%x*} $s "$PT/text.terse" --frames 30
    done
    # Decay: six frames in a row of the pattern (the moving dot, the
    # sweeping line) and of sparks.
    for f in 200 201 202 203 204 205; do
        shot $m pattern-f$f 640x480 "$PT/pattern.terse" --frames $f
        shot $m sparks-f$((f + 100)) 640x480 "$P/sparks/sparks.terse" --events "$P/sparks/sessions/phosphor.txt" --frames $((f + 100)) --without 10
    done
done
# MAME at its longest persistence (the slider's 100 ms) for the decay.
for f in 200 201 202 203 204 205; do
    shot mame100 pattern-f$f 640x480 "$PT/pattern.terse" --frames $f
done
shot mame100 pattern-640 640x480 "$PT/pattern.terse" --frames 200
shot mame100 sparks-640 640x480 "$P/sparks/sparks.terse" --events "$P/sparks/sessions/phosphor.txt" --frames 300 --without 10

if command -v magick >/dev/null; then
    cd "$out"
    for n in pattern-640 pattern-1280 sparks-640 sparks-1280 hello-640 holo-640 text-640 text-1280 text-1920; do
        magick mame-$n.png ours-$n.png +append sbs-$n.png
    done
    # Strips: the dot and the sweep (pattern) and the middle of sparks, frame
    # by frame, MAME (2 ms), MAME (100 ms) and ours, one row each.
    for m in mame mame100 ours; do
        magick $(for f in 200 201 202 203 204 205; do echo $m-pattern-f$f.png; done) -crop 160x40+120+360 +repage -filter point -resize 200% +append strip-dot-$m.png
        magick $(for f in 200 201 202 203 204 205; do echo $m-pattern-f$f.png; done) -crop 140x140+490+40 +repage +append strip-sweep-$m.png
    done
    magick strip-dot-mame.png strip-dot-mame100.png strip-dot-ours.png -append strip-dot.png
    magick strip-sweep-mame.png strip-sweep-mame100.png strip-sweep-ours.png -append strip-sweep.png
    magick $(for f in 300 301 302 303 304 305; do echo mame-sparks-f$f.png; done) +append strip-sparks-mame.png
    magick $(for f in 300 301 302 303 304 305; do echo ours-sparks-f$f.png; done) +append strip-sparks-ours.png
    magick strip-sparks-mame.png strip-sparks-ours.png -append strip-sparks.png
    # Text at 1x, 2x, 3x: the same 160 x 64 units of the screen, enlarged
    # without smoothing to 3x's size.
    magick mame-text-640.png -crop 213x96+0+224 +repage -filter point -resize 300% zoom-text-mame-1x.png
    magick mame-text-1280.png -crop 427x192+0+448 +repage -filter point -resize 150% zoom-text-mame-2x.png
    magick mame-text-1920.png -crop 640x288+0+672 +repage zoom-text-mame-3x.png
    magick zoom-text-mame-1x.png zoom-text-mame-2x.png zoom-text-mame-3x.png -append zoom-text-mame.png
fi
cp "$tmp/log" "$out/run.log"
