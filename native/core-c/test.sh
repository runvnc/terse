#!/bin/sh
# Tests tcore against the expected files and, where none exist, against the
# reference host (`terse run`) as a black box. Every run uses a temporary
# HOME, store, config and data directory; every driver tcore starts is
# reaped by tcore itself.
#   TERSE_DRIVERS  directory of terse-dev-* and terse (default ../../target/release)
#   FUZZ           number of random CPU images (default 300; 0 skips)
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
P=$root/programs
D=${TERSE_DRIVERS:-$root/target/release}
TC=$here/tcore
FUZZ=${FUZZ:-300}
T=$(mktemp -d "${TMPDIR:-/tmp}/tcore-test.XXXXXX")
trap 'rm -rf "$T"' EXIT INT TERM
mkdir -p "$T/store" "$T/config" "$T/data" "$T/home"
export HOME="$T/home" XDG_CONFIG_HOME="$T/home/config" XDG_CACHE_HOME="$T/home/cache" XDG_DATA_HOME="$T/home/data"
export TERSE_DRIVERS="$D" TMPDIR="$T"
CONTENT="4=terse-dev-content --store $T/store --config $T/config --data $T/data"
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "ok   $*"; }
no() { fail=$((fail + 1)); echo "FAIL $*"; }
tc() { "$TC" "$@" --driver "$CONTENT"; }
ref() { "$D/terse" run "$@" --relay none --store "$T/store" --config "$T/config" --data "$T/data"; }
# same <name> <args...>: tcore and the reference print the same hashes and exit status
same() {
    name=$1; shift
    tc "$@" > "$T/a" 2> "$T/ae"; sa=$?
    ref "$@" > "$T/b" 2> "$T/be"; sb=$?
    if [ $sa -eq $sb ] && cmp -s "$T/a" "$T/b" && [ -s "$T/a" ]; then
        ok "$name ($(wc -l < "$T/a") hashes, status $sa)"
    else
        no "$name: status $sa vs $sb"; diff "$T/a" "$T/b" | head -4; head -3 "$T/ae" "$T/be"
    fi
}

echo "== STATE.md test vectors (hello)"
h=$("$TC" "$P/hello/hello.terse" --frames 0 --hash)
[ "$h" = 2eb8158b7d3ea45b4c7c598a4d506b8865cabd054046bc2a720d621104c6db18 ] && ok "hello at reset" || no "hello at reset: $h"
h=$("$TC" "$P/hello/hello.terse" --frames 1 --hash)
[ "$h" = 8de47f0a7bdfdaaf9c804e35927611384962a308e3044ed82269785947bc6b42 ] && ok "hello after 1 frame" || no "hello 1: $h"
h=$("$TC" "$P/hello/hello.terse" --frames 30 --hash)
[ "$h" = cb3d8c6473a9b608c48e77db48c7f19724fbe2c354094c837f8b79fb8b923c9d ] && ok "hello after 30 frames" || no "hello 30: $h"
"$TC" "$P/hello/hello.terse" --frames 30 --dump --events "$here/tests/type.events" > "$T/a"
ref "$P/hello/hello.terse" --frames 30 --dump --events "$here/tests/type.events" > "$T/b"
cmp -s "$T/a" "$T/b" && ok "hello typed into: --dump is the reference's" || no "hello screen"

echo "== conformance images with their scripted input (expected files)"
for c in conform-program conform conform-reading; do
    [ -f "$P/$c/$c.terse" ] || continue
    tc "$P/$c/$c.terse" --events "$P/$c/$c.events" --frames 200 --hash-every 10 > "$T/h" 2> "$T/e"
    if cmp -s "$T/h" "$P/$c/expected-hashes.txt"; then ok "$c: 20 of 20 hashes"
    else no "$c: $(diff "$T/h" "$P/$c/expected-hashes.txt" | grep -c '^<') of 20 differ, first: $(diff "$T/h" "$P/$c/expected-hashes.txt" | sed -n 2p)"; head -2 "$T/e"; fi
done
tc "$P/conform-program/conform-program.terse" --events "$P/conform-program/conform-program.events" --frames 200 --dump > "$T/s"
head -30 "$P/conform-program/expected-screen.txt" | cmp -s - "$T/s" && ok "conform-program screen" || no "conform-program screen"
tc "$P/conform-program/conform-program.terse" --events "$P/conform-program/conform-program.events" --frames 200 --hash-every 10 --no-batch \
    | cmp -s - "$P/conform-program/expected-hashes.txt" && ok "conform-program, no batching (drivers agree with the call tables)" || no "conform-program --no-batch"

echo "== other programs against the reference host"
same "holo (device 9 without 3)" "$P/holo/holo.terse" --frames 240 --hash-every 20
same "sketch (optional content granted)" "$P/sketch/sketch.terse" --frames 120 --hash-every 20
same "sparks, keys session" "$P/sparks/sparks.terse" --events "$P/sparks/sessions/keys.txt" --frames 600 --hash-every 50
same "sparks, pad session" "$P/sparks/sparks.terse" --events "$P/sparks/sessions/pad.txt" --frames 400 --hash-every 50
same "sparks without device 10" "$P/sparks/sparks.terse" --frames 60 --hash-every 20 --without 10
same "sparks without device 4" "$P/sparks/sparks.terse" --frames 60 --hash-every 20 --without 4
same "shell run alone (lists device 8: absent)" "$P/shell/shell.terse" --frames 60 --hash-every 10
same "hostile (every kind of call, full budget)" "$P/hostile/hostile.terse" --frames 480 --hash-every 60
same "hello, typed into" "$P/hello/hello.terse" --events "$here/tests/type.events" --frames 30 --hash-every 1
same "hello, Esc exits with 0" "$P/hello/hello.terse" --events "$here/tests/esc.events" --frames 30 --hash

echo "== optional devices whose driver is not installed"
"$TC" "$P/sketch/sketch.terse" --frames 60 --hash --driver "4=/nonexistent/terse-dev-content" > "$T/a" 2>&1
ref "$P/sketch/sketch.terse" --frames 60 --hash --without 4 > "$T/b" 2>&1
cmp -s "$T/a" "$T/b" && ok "sketch, no content driver = reference --without 4" || no "sketch without content driver"

same "opt with its optional devices granted" "$here/tests/opt.terse" --frames 4 --hash
same "opt, terminal granted, content withheld" "$here/tests/opt.terse" --frames 4 --hash --without 4

echo "== a required device with no driver is refused"
"$TC" "$P/hello/hello.terse" --frames 1 --driver "2=/nonexistent/terse-dev-terminal" > /dev/null 2> "$T/e"
s=$?; [ $s -eq 2 ] && grep -q "needs a device" "$T/e" && ok "hello without a terminal driver: refused (status 2)" || no "missing terminal driver: status $s"

echo "== images of tests/ (System only, faults, exits): no drivers at all"
for img in "$here"/tests/*.terse; do
    n=$(basename "$img" .terse)
    w=; [ "$n" = opt ] && w="--without 2 --without 4 --without 10"   # opt lists them as optional
    "$TC" "$img" --frames 4 --hash --drivers /nonexistent > "$T/a" 2> "$T/ae"; sa=$?
    ref "$img" --frames 4 --hash $w > "$T/b" 2> "$T/be"; sb=$?
    if [ $sa -eq $sb ] && cmp -s "$T/a" "$T/b"; then ok "$n (status $sa)"; else no "$n: status $sa vs $sb"; cat "$T/ae" "$T/be" | head -4; fi
done

echo "== image headers (tests/headers.py): refused or run as the reference does"
python3 -I "$here/tests/headers.py" "$T/hdr"
for img in "$T"/hdr/*.terse; do
    n=$(basename "$img" .terse)
    a=$("$TC" "$img" --frames 2 --hash 2>/dev/null); sa=$?
    b=$(ref "$img" --frames 2 --hash 2>/dev/null); sb=$?
    [ $sa -eq $sb ] && [ "$a" = "$b" ] && ok "$n (status $sa)" || no "$n: status $sa vs $sb"
done

if [ "$FUZZ" -gt 0 ]; then
    echo "== $FUZZ random RV32IMC images (tests/fuzz.py), 3 frames each"
    python3 -I "$here/tests/fuzz.py" "$T/fuzz" "$FUZZ"
    bad=0
    for img in "$T"/fuzz/*.terse; do
        a=$("$TC" "$img" --frames 3 --hash 2>/dev/null); b=$(ref "$img" --frames 3 --hash 2>/dev/null)
        [ "$a" = "$b" ] || { bad=$((bad + 1)); [ $bad -le 3 ] && echo "     differs: $img"; }
    done
    [ $bad -eq 0 ] && ok "fuzz: $FUZZ of $FUZZ images" || no "fuzz: $bad of $FUZZ images differ"
fi

echo "== every driver started was reaped"
left=$(grep -l "TERSE_BUS_SOCKET=$T" /proc/[0-9]*/environ 2>/dev/null | wc -l)
[ "$left" -eq 0 ] && ok "no driver process left" || no "$left driver processes left"

echo "$pass passed, $fail failed"
[ $fail -eq 0 ]
