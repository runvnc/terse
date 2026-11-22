#!/bin/sh
# Builds the example experimental device (dice.c, example.dice) and the
# program that uses it (demo.c), then checks spec/experimental.md end to
# end on terse-bus: granted (results, determinism, snapshots, a replay),
# not granted (terse run, terse-bus without the driver), installed with
# terse drivers, and the refusals. Every run uses a temporary HOME and
# temporary store, config and data directories, and no network.
#   TERSE_DRIVERS  directory of terse, terse-bus and the terse-dev-* drivers
#                  (default ../../target/release; build them first with
#                  scripts/cargo build --release -p terse-cli -p terse-devices -p terse-bus)
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
D=${TERSE_DRIVERS:-$root/target/release}
T=$(mktemp -d "${TMPDIR:-/tmp}/driver-experimental.XXXXXX")
trap 'rm -rf "$T"' EXIT INT TERM
mkdir -p "$T/home"
export HOME="$T/home" XDG_CONFIG_HOME="$T/home/config" XDG_CACHE_HOME="$T/home/cache" XDG_DATA_HOME="$T/home/data"
export TMPDIR="$T"
unset TERSE_DICE_BAD
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "ok   $*"; }
no() { fail=$((fail + 1)); echo "FAIL $*"; }
dirs="--store $T/store --config $T/config --data $T/data --relay none"
bus() { "$D/terse-bus" "$@" $dirs; }

X=$T/dice
${CC:-cc} -std=c11 -O2 -Wall -Wextra -Werror -o "$X" "$here/dice.c" || { echo "FAIL build"; exit 1; }
ok "dice.c built with no warnings ($(wc -l < "$here/dice.c") lines)"
P=$T/demo.terse
"$D/terse" build "$here/demo.c" --optional 240:0=example.dice -o "$P" 2> "$T/build" \
    && ok "demo.c built: $(cut -d, -f1 "$T/build")" || { no "terse build demo.c"; cat "$T/build"; exit 1; }
"$D/terse" info "$P" | grep -q "^experimental: 240=example.dice" && ok "terse info names the device" || no "terse info"

echo "== refusals: an experimental device is only optional, and named"
"$D/terse" build "$here/demo.c" --device 240:0 -o "$T/x.terse" 2> /dev/null; s=$?
[ $s -ne 0 ] && [ ! -e "$T/x.terse" ] && ok "terse build --device 240:0: refused" || no "--device 240:0 status $s"
"$D/terse" build "$here/demo.c" --optional 240:0 -o "$T/x.terse" 2> /dev/null; s=$?
[ $s -ne 0 ] && ok "terse build --optional 240:0 without a name: refused" || no "no name: status $s"
"$D/terse" build "$here/demo.c" --optional 7:0=example.dice -o "$T/x.terse" 2> /dev/null; s=$?
[ $s -ne 0 ] && ok "a name on a numbered slot: refused" || no "name on slot 7: status $s"
# An image that requires slot 240 anyway (its header patched): every host refuses it.
python3 -I -c 'import sys; b=bytearray(open(sys.argv[1],"rb").read()); i=b.index(bytes([240,128]),32); b[i+1]=0; open(sys.argv[2],"wb").write(b)' "$P" "$T/req.terse"
bus "$T/req.terse" --frames 1 --driver "example.dice=$X" > /dev/null 2>&1; a=$?
"$D/terse" run "$T/req.terse" --frames 1 $dirs > /dev/null 2>&1; b=$?
[ $a -eq 2 ] && [ $b -eq 2 ] && ok "an image requiring slot 240 is refused (terse-bus, terse run: status 2)" || no "required 240: $a $b"

echo "== granted: example.dice on terse-bus"
bus "$P" --driver "example.dice=$X" --frames 200 --hash-every 10 > "$T/h1" 2> "$T/e1"; a=$?
bus "$P" --driver "example.dice=$X" --frames 200 --hash-every 10 > "$T/h2" 2>&1
[ $a -eq 0 ] && [ "$(wc -l < "$T/h1")" -eq 20 ] && cmp -s "$T/h1" "$T/h2" && ok "two runs: the same 20 hashes" || { no "two runs (status $a)"; cat "$T/e1"; }
bus "$P" --driver "example.dice=$X" --frames 200 --hash-every 10 --no-batch > "$T/h3" 2>&1
bus "$P" --driver "example.dice=$X" --frames 200 --hash-every 10 --transport socket > "$T/h4" 2>&1
cmp -s "$T/h1" "$T/h3" && cmp -s "$T/h1" "$T/h4" && ok "--no-batch and the socket transport: the same hashes (seed's charge computed by the core = the driver's)" || no "batched vs not"
bus "$P" --driver "example.dice=$X" --frames 200 --dump > "$T/screen" 2>&1
grep -q "Granted, version 0" "$T/screen" && grep -q "roll(300) = FFFFFFFE" "$T/screen" \
    && grep -q "^turn 6: " "$T/screen" && grep -q "rolls 18, last 8:" "$T/screen" \
    && ok "the program's screen: granted, a bad argument refused by the table, 6 turns of 3 dice" || { no "screen"; cat "$T/screen"; }
sed -n 's/^turn 1: \(.*\) = .*/\1/p' "$T/screen" | grep -Eq '^[1-6] \+ [1-6] \+ [1-6]$' && ok "dice in 1-6: $(grep '^turn 1:' "$T/screen")" || no "dice range"

echo "== snapshots and a recording"
bus "$P" --driver "example.dice=$X" --frames 100 --save-state "$T/s" > /dev/null 2>&1
bus "$P" --driver "example.dice=$X" --load-state "$T/s" --frames 100 --hash-every 10 > "$T/h5" 2>&1; a=$?
tail -10 "$T/h1" > "$T/h1b"
[ $a -eq 0 ] && cmp -s "$T/h1b" "$T/h5" && ok "snapshot at frame 100, restored: frames 110-200 as the run that never stopped" || { no "snapshot restore ($a)"; head -3 "$T/h5"; }
v=$(od -An -tu1 -j4 -N1 "$T/s" | tr -d ' ')
[ "$v" = 1 ] && ok "the snapshot file says it is experimental (version byte 1)" || no "snapshot version $v"
"$D/terse" run "$P" --load-state "$T/s" --frames 1 $dirs > /dev/null 2>&1; a=$?
bus "$P" --load-state "$T/s" --frames 1 > /dev/null 2>&1; b=$?
[ $a -ne 0 ] && [ $b -ne 0 ] && ok "a host without the driver refuses to restore it (terse run $a, terse-bus $b)" || no "restore without the driver: $a $b"
bus "$P" --driver "example.dice=$X" --frames 120 --record "$T/r" > /dev/null 2>&1
bus "$P" --driver "example.dice=$X" --replay "$T/r" --hash > "$T/hr" 2>&1; a=$?
[ $a -eq 0 ] && [ "$(cat "$T/hr")" = "$(sed -n 12p "$T/h1" | cut -d' ' -f2)" ] && ok "a recording replays, every frame's hash checked" || { no "replay ($a)"; cat "$T/hr"; }
v=$(od -An -tu1 -j4 -N1 "$T/r" | tr -d ' ')
[ "$v" = 2 ] && ok "the recording says it is experimental (version 2)" || no "recording version $v"

echo "== not granted"
h=$("$D/terse" run "$P" --frames 30 --hash $dirs 2> /dev/null)
a=$(bus "$P" --frames 30 --hash 2> "$T/e")
b=$(bus "$P" --frames 30 --hash --driver "example.dice=$X" --without 240 2> /dev/null)
[ -n "$h" ] && [ "$h" = "$a" ] && [ "$h" = "$b" ] && ok "terse run, terse-bus without a driver and --without 240: the same hash" || no "not granted: '$h' '$a' '$b'"
if (cd "$root/native/core-c" && nice -n 15 make -s -j4 > /dev/null 2>&1); then
    c=$("$root/native/core-c/tcore" "$P" --frames 30 --hash --drivers "$D" 2> /dev/null)
    [ "$c" = "$h" ] && ok "tcore (no experimental devices, DEVICES.md 12): the same hash" || no "tcore: '$c'"
fi
grep -q "no driver for the experimental device example.dice" "$T/e" && ok "terse-bus says why: $(head -1 "$T/e" | cut -c1-80)" || no "no message"
"$D/terse" run "$P" --frames 30 --dump $dirs 2> /dev/null > "$T/screen"
grep -q "Not granted here" "$T/screen" && grep -q "A call to it now gives FFFFFFFF" "$T/screen" && ok "the program says so; a call returns FFFF_FFFFh" || { no "not-granted screen"; cat "$T/screen"; }

echo "== installed with terse drivers"
"$D/terse" drivers add "$X" --config "$T/config" > "$T/add" 2>&1; a=$?
[ $a -eq 0 ] && grep -q "example.dice (experimental, version 0" "$T/add" && grep -q "^experimental example.dice 0" "$T/config/drivers/dice-example/manifest" \
    && ok "terse drivers add: installed as dice-example" || { no "drivers add ($a)"; cat "$T/add"; }
bus "$P" --frames 200 --hash-every 10 > "$T/h6" 2>&1
cmp -s "$T/h1" "$T/h6" && ok "installed: granted with no --driver, the same hashes" || { no "installed run"; head -3 "$T/h6"; }
"$D/terse" drivers list --config "$T/config" | grep -q "the experimental device example.dice" && ok "terse drivers list shows it" || no "drivers list"
TERSE_DICE_BAD=name bus "$P" --frames 30 --hash > "$T/h7" 2> "$T/e"; a=$?
[ $a -eq 0 ] && [ "$(cat "$T/h7")" = "$h" ] && grep -q "not granted" "$T/e" && ok "an installed driver that answers with another name: not used, not granted, the run goes on" || { no "installed, wrong name ($a)"; cat "$T/e"; }
cp "$X" "$T/dice2"
"$D/terse" drivers add "$T/dice2" --name other --config "$T/config" > /dev/null 2>&1; a=$?
[ $a -ne 0 ] && ok "a second driver for example.dice is refused" || no "second driver: $a"

echo "== a driver that breaks the rules (DEVICES.md 15.2, 15.3)"
for c in "table 2" "name 2" "charge 3" "inputs 3" "state 3"; do
    set -- $c
    TERSE_DICE_BAD=$1 bus "$P" --driver "example.dice=$X" --frames 60 --hash > /dev/null 2> "$T/e"; s=$?
    [ $s -eq $2 ] && ok "TERSE_DICE_BAD=$1: status $s ($(grep -o 'DEVICES.md 15.[0-9]\|not example.dice\|more than 4096\|a charge above\|buffer of up to [0-9]*' "$T/e" | head -1))" || { no "TERSE_DICE_BAD=$1: status $s, want $2"; cat "$T/e"; }
done

echo "== every driver started was reaped"
left=$(grep -l "TERSE_BUS_SOCKET=$T" /proc/[0-9]*/environ 2>/dev/null | wc -l)
[ "$left" -eq 0 ] && ok "no driver process left" || no "$left driver processes left"

echo "$pass passed, $fail failed"
[ $fail -eq 0 ]
