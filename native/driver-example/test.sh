#!/bin/sh
# Builds the example terminal driver and checks that machines run with it
# give the reference terminal driver's state hashes, under the reference
# core (terse-bus, headless) and under native/core-c's tcore. Every run uses
# a temporary HOME; every driver is started and reaped by the core.
#   TERSE_DRIVERS  directory of terse-bus and the terse-dev-* drivers
#                  (default ../../target/release; build them first with
#                  scripts/cargo build --release -p terse-devices -p terse-bus)
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
P=$root/programs
D=${TERSE_DRIVERS:-$root/target/release}
T=$(mktemp -d "${TMPDIR:-/tmp}/driver-example.XXXXXX")
trap 'rm -rf "$T"' EXIT INT TERM
mkdir -p "$T/home"
export HOME="$T/home" XDG_CONFIG_HOME="$T/home/config" XDG_CACHE_HOME="$T/home/cache" XDG_DATA_HOME="$T/home/data"
export TMPDIR="$T"
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "ok   $*"; }
no() { fail=$((fail + 1)); echo "FAIL $*"; }

X=$T/terminal
${CC:-cc} -std=c11 -O2 -Wall -Wextra -Werror -o "$X" "$here/terminal.c" || { echo "FAIL build"; exit 1; }
ok "built with no warnings ($(wc -l < "$here/terminal.c") lines)"
(cd "$root/native/core-c" && nice -n 15 make -s -j4) || { echo "FAIL building tcore"; exit 1; }
TC=$root/native/core-c/tcore

# check <name> <args...>: the reference driver under terse-bus, then the
# example under terse-bus, tcore and tcore --no-batch: the same output
check() {
    name=$1; shift
    "$D/terse-bus" "$@" > "$T/ref" 2>&1
    "$D/terse-bus" "$@" --driver "2=$X" > "$T/bus" 2>&1; a=$?
    "$TC" "$@" --drivers "$D" --driver "2=$X" > "$T/tc" 2>&1; b=$?
    "$TC" "$@" --drivers "$D" --driver "2=$X" --no-batch > "$T/tcn" 2>&1; c=$?
    if [ -s "$T/ref" ] && cmp -s "$T/ref" "$T/bus" && cmp -s "$T/ref" "$T/tc" && cmp -s "$T/ref" "$T/tcn" && [ $a -eq $b ] && [ $b -eq $c ]; then
        ok "$name: $(wc -l < "$T/ref") hashes as the reference driver's (terse-bus, tcore, tcore --no-batch)"
    else
        no "$name: statuses $a $b $c"; diff "$T/ref" "$T/bus" | head -3; diff "$T/ref" "$T/tc" | head -3
    fi
}

echo "== STATE.md 3: the hello vectors"
for f in 0 1 30; do
    case $f in 0) want=f82b7079daea85863f079eaba67e0ad2816b905e9a1a494e7dab587889a4c058;;
                1) want=2c8994cf8408449f927b16377cf44750419682c7fcc744d84748459c7ec8f9cf;;
                *) want=819432152b38626f8fc54b43f50d5d4c501dc99e6904315b61dc78b796c611e2;; esac
    a=$("$D/terse-bus" "$P/hello/hello.terse" --frames $f --hash --driver "2=$X")
    b=$("$TC" "$P/hello/hello.terse" --frames $f --hash --drivers "$D" --driver "2=$X")
    [ "$a" = "$want" ] && [ "$b" = "$want" ] && ok "hello after $f frames" || no "hello after $f frames: $a $b"
done

echo "== against the reference terminal driver"
check "hello, 30 frames" "$P/hello/hello.terse" --frames 30 --hash-every 1
check "hello, typed into" "$P/hello/hello.terse" --frames 30 --hash-every 1 --events "$root/native/core-c/tests/type.events"
check "conform-program, scripted" "$P/conform-program/conform-program.terse" --events "$P/conform-program/conform-program.events" --frames 200 --hash-every 10
cmp -s "$T/tc" "$P/conform-program/expected-hashes.txt" && ok "conform-program: its expected hashes" || no "conform-program expected hashes"
"$TC" "$P/conform-program/conform-program.terse" --events "$P/conform-program/conform-program.events" --frames 200 --dump \
    --drivers "$D" --driver "2=$X" > "$T/screen"
head -30 "$P/conform-program/expected-screen.txt" | cmp -s - "$T/screen" && ok "conform-program: its expected screen (every line PASS)" || no "conform-program screen"
check "hostile (every call, full budget)" "$P/hostile/hostile.terse" --frames 480 --hash-every 60

echo "== every driver started was reaped"
left=$(grep -l "TERSE_BUS_SOCKET=$T" /proc/[0-9]*/environ 2>/dev/null | wc -l)
[ "$left" -eq 0 ] && ok "no driver process left" || no "$left driver processes left"

echo "$pass passed, $fail failed"
[ $fail -eq 0 ]
