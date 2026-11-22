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
D=$(cd "${TERSE_DRIVERS:-$root/target/release}" && pwd) || exit 1
# Programs older than their sources would test something else: refuse them.
python3 "$root/scripts/check-built.py" "$D" terse terse-bus terse-dev-terminal terse-dev-vector terse-dev-content terse-dev-page terse-dev-sound || exit 1
FUZZ=${FUZZ:-300}
T=$(mktemp -d "${TMPDIR:-/tmp}/tcore-test.XXXXXX") || exit 1
trap 'rm -rf "$T"' EXIT INT TERM
# The tcore tested is built here from the current sources, into this run's
# directory: not native/core-c's own, which may be older than its sources
# or being built again by another test (or a make there) as this one runs.
mkdir -p "$T/tcore" && cp "$here"/*.c "$here"/*.h "$here/Makefile" "$T/tcore/" \
    && (cd "$T/tcore" && nice -n 15 make -s -j4) || { echo "FAIL building tcore"; exit 1; }
TC=$T/tcore/tcore
mkdir -p "$T/store" "$T/config" "$T/data" "$T/home"
export HOME="$T/home" XDG_CONFIG_HOME="$T/home/config" XDG_CACHE_HOME="$T/home/cache" XDG_DATA_HOME="$T/home/data"
export TERSE_DRIVERS="$D" TMPDIR="$T"
CONTENT="4=$D/terse-dev-content --store $T/store --config $T/config --data $T/data"
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
[ "$h" = f82b7079daea85863f079eaba67e0ad2816b905e9a1a494e7dab587889a4c058 ] && ok "hello at reset" || no "hello at reset: $h"
h=$("$TC" "$P/hello/hello.terse" --frames 1 --hash)
[ "$h" = 2c8994cf8408449f927b16377cf44750419682c7fcc744d84748459c7ec8f9cf ] && ok "hello after 1 frame" || no "hello 1: $h"
h=$("$TC" "$P/hello/hello.terse" --frames 30 --hash)
[ "$h" = 819432152b38626f8fc54b43f50d5d4c501dc99e6904315b61dc78b796c611e2 ] && ok "hello after 30 frames" || no "hello 30: $h"
# the exit vector: STATE.md 3 gives the image's bytes and both hashes
x=5445525300010a000001000000000000657869740000000000000000000000001d459308601073000000
python3 -I -c "import sys; open(sys.argv[1], 'wb').write(bytes.fromhex(sys.argv[2]))" "$T/exit7.terse" "$x"
h0=$("$TC" "$T/exit7.terse" --frames 0 --hash --drivers /nonexistent)
h1=$("$TC" "$T/exit7.terse" --frames 1 --hash --drivers /nonexistent); s=$?
if grep -q "$x" "$root/STATE.md" && grep -q "$h0" "$root/STATE.md" && grep -q "$h1" "$root/STATE.md" && [ $s -eq 7 ] \
    && [ "$h0" = 9cbfa0da982561dd1c5dbaf1d5fde2121dee78c097d754aa0c2a350584bbeb5a ] \
    && [ "$h1" = 3272a879392d8179997dc398726430bf92bbd413b89653be8c9af4472656fe9b ]
then ok "exit vector (a0 = 0 after exit, status 7)"; else no "exit vector: $h0 $h1 status $s"; fi
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

echo "== optional devices whose driver is not installed (DEVICES.md 2.1)"
mkdir -p "$T/drv" "$T/none"
for d in terse-dev-terminal terse-dev-vector terse-dev-sound terse-dev-page; do ln -s "$D/$d" "$T/drv/$d"; done
"$TC" "$P/sketch/sketch.terse" --frames 60 --hash --drivers "$T/drv" > "$T/a" 2>&1
ref "$P/sketch/sketch.terse" --frames 60 --hash --without 4 > "$T/b" 2>&1
cmp -s "$T/a" "$T/b" && ok "sketch, no content driver installed = reference --without 4" || no "sketch without content driver"

same "opt with its optional devices granted" "$here/tests/opt.terse" --frames 4 --hash
same "opt, terminal granted, content withheld" "$here/tests/opt.terse" --frames 4 --hash --without 4

echo "== a required device with no driver, or a --driver that cannot start, refuses the run"
"$TC" "$P/hello/hello.terse" --frames 1 --drivers "$T/none" > /dev/null 2> "$T/e"
s=$?; [ $s -eq 2 ] && grep -q "needs a device" "$T/e" && ok "hello without a terminal driver: refused (status 2)" || no "missing terminal driver: status $s"
for img in hello/hello sketch/sketch; do
    # (Not a case inside $(...): macOS's /bin/sh, bash 3.2, cannot parse it.)
    case $img in hello*) slot=2;; *) slot=4;; esac
    "$TC" "$P/$img.terse" --frames 1 --driver "$slot=/nonexistent" > /dev/null 2>&1; sa=$?
    "$D/terse-bus" "$P/$img.terse" --frames 1 --driver "$slot=/nonexistent" > /dev/null 2>&1; sb=$?
    [ $sa -eq 2 ] && [ $sb -eq 2 ] && ok "$img with a --driver that cannot start: refused (status 2, as terse-bus)" || no "$img --driver /nonexistent: status $sa vs $sb"
done
"$TC" "$P/hello/hello.terse" --frames 1 --driver "2=$D/terse-dev-vector" > /dev/null 2>&1; sa=$?
"$D/terse-bus" "$P/hello/hello.terse" --frames 1 --driver "2=$D/terse-dev-vector" > /dev/null 2>&1; sb=$?
[ $sa -eq 2 ] && [ $sb -eq 2 ] && ok "a driver answering for the wrong slots: refused (status 2, as terse-bus)" || no "wrong slots: status $sa vs $sb"

echo "== the events file and the dump (STATE.md 6.2, 6.3)"
same "every form of the events file (spin keeps its queue)" "$here/tests/spin.terse" --events "$here/tests/script.events" --frames 6 --hash-every 1
sed 's/$/\r/' "$here/tests/script.events" > "$T/crlf.events"
same "the same with CR LF line ends" "$here/tests/spin.terse" --events "$T/crlf.events" --frames 6 --hash-every 1
for bad in "1 pointer 1 2 8" "1 pointer -1 2" "1 wheel -128" "1 wheel 1 -128" "1 pad 0 0 -128 0" "1 pad 0 10000 0 0" "1 pad 4 0 0 0" \
           "1 pad 0 0 0" "1 press nokey" "1 press 256" "1 down a hyper" "1 jump" "x press a" "1"; do
    printf '%s\n' "$bad" > "$T/bad.events"
    "$TC" "$here/tests/spin.terse" --events "$T/bad.events" --frames 1 --drivers /nonexistent > /dev/null 2>&1; sa=$?
    ref "$here/tests/spin.terse" --events "$T/bad.events" --frames 1 > /dev/null 2>&1; sb=$?
    [ $sa -eq 2 ] && [ $sb -eq 2 ] && ok "refused: $bad" || no "events line '$bad': status $sa vs $sb"
done
python3 -I "$here/tests/dump.py" "$T/dump"
"$TC" "$T/dump/dump.terse" --frames 2 --dump > "$T/a"
ref "$T/dump/dump.terse" --frames 2 --dump > "$T/b"
cmp -s "$T/a" "$T/b" && ok "--dump: continuations, control codes, a surrogate" || no "--dump differs"
"$D/terse-bus" "$T/dump/dump.terse" --frames 2 --dump > "$T/c" 2>/dev/null
cmp -s "$T/c" "$T/b" && ok "--dump: terse-bus prints the same" || no "terse-bus --dump differs"
sb=$(sh -c '"$0" "$1" --frames 2 --hash > /dev/null 2>&1; echo $?' "$D/terse-bus" "$here/tests/load.terse" 2> /dev/null)
ref "$here/tests/load.terse" --frames 2 --hash > /dev/null 2>&1; sr=$?
[ $sb -eq 143 ] && [ $sr -eq 143 ] && ok "a fault ends terse-bus and terse run by SIGTERM" || no "fault status: terse-bus $sb, terse run $sr"

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
# The sizes' boundaries (SPEC §profile): what the statuses must be, not only equal.
for c in ok-rom-max:9 ok-pages-112:9 ok-ram-last:90 bad-rom-over:2 bad-pages-113:2 bad-pages-128:2; do
    n=${c%:*} want=${c#*:}
    "$TC" "$T/hdr/$n.terse" --frames 2 > /dev/null 2>&1; s=$?
    [ $s -eq "$want" ] && ok "$n: status $want" || no "$n: status $s, not $want"
done
"$TC" "$T/hdr/fault-ram-past.terse" --frames 2 > /dev/null 2> "$T/e"
grep -q "001e0000" "$T/e" && ok "fault-ram-past: a store fault at 001e0000h" || no "fault-ram-past: $(head -1 "$T/e")"

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

echo "== identities (DEVICES.md 1.1: version and build at the end of HELLO)"
v=$("$TC" --version); s=$?
echo "$v" | grep -Eq '^tcore 0\.1\.0 b3:[0-9a-f]{16}$' && [ $s -eq 0 ] && ok "--version: $v" || no "--version: status $s, '$v'"
${CC:-cc} -std=c11 -O2 -o "$T/example" "$root/native/driver-example/terminal.c" 2> /dev/null
# a line per driver: its lowest slot, name, then version and build or -
ID='^driver [0-9]+ .+ (-|[[:graph:]]{1,32}) (-|[a-z0-9]+:[0-9a-f]{16}|unknown)$'
tc "$P/sketch/sketch.terse" --frames 1 --hash --identities --driver "2=$T/example" > /dev/null 2> "$T/ids"; s=$?
n=$(grep -c '^driver ' "$T/ids")
if [ $s -eq 0 ] && [ "$n" -ge 2 ] && [ "$(grep -Ec "$ID" "$T/ids")" -eq "$n" ] \
    && grep -Eq '^driver 2 terminal \(example\) 0\.1\.0 fnv:[0-9a-f]{16}$' "$T/ids" \
    && grep -Eq '^driver 4 .* (- -|0\.1\.0 b3:[0-9a-f]{16})$' "$T/ids"
then ok "--identities: $n drivers (the example's fnv: build)"; else no "--identities: status $s"; cat "$T/ids"; fi
tc "$P/holo/holo.terse" --frames 1 --hash --identities --driver "3,9=python3 -I $root/crates/terse-devices/python/vector.py" > /dev/null 2> "$T/ids"; s=$?
[ $s -eq 0 ] && grep -Eq '^driver 3 .* 0\.1\.0 b2:[0-9a-f]{16}$' "$T/ids" && ok "--identities: vector.py's b2: build" || { no "--identities vector.py: status $s"; cat "$T/ids"; }
printf 'driver 2 x - -\n' | grep -Eq "$ID" && printf 'driver 3 terse-dev-vector 0.1.0 b3:0123456789abcdef\n' | grep -Eq "$ID" \
    && ok "--identities line pattern takes absent and b3: identities" || no "identity pattern"

echo "== every driver started was reaped"
if [ -d /proc/self ]; then
    left=$(grep -l "TERSE_BUS_SOCKET=$T" /proc/[0-9]*/environ 2>/dev/null | wc -l)
else # macOS: ps -e shows each process's environment after its command
    left=$(ps -axeww -o command= 2>/dev/null | grep "TERSE_BUS_SOCKET=$T" | grep -vc grep)
fi
[ "$left" -eq 0 ] && ok "no driver process left" || no "$left driver processes left"

echo "$pass passed, $fail failed"
[ $fail -eq 0 ]
