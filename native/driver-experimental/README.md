# An invented device

`dice.c` is a device nobody specified: `example.dice`, a dice roller with
a memory, written as an **experimental driver** (`spec/experimental.md`,
`DEVICES.md` 15). `demo.c` is a Tersenet program that rolls its dice when
the host grants the device and says so on the terminal when it does not.
C11 and POSIX, no dependencies; about 280 lines for the driver, 40 of them
the device.

    cc -std=c11 -O2 -Wall -Wextra -o dice dice.c
    terse build demo.c --optional 240:0=example.dice
    terse-bus demo.terse --driver example.dice=$PWD/dice --frames 200 --dump
    terse run demo.terse --frames 30 --dump      # no dice: not granted
    terse drivers add dice                       # installed: no --driver needed
    ./test.sh                                    # everything below, checked

## The device

| Function | Call | Result | Class, charge |
|---|---|---|---|
| 0 | `seed(a0 = seed)` | 0 | deferred: 4 |
| 1 | `roll(a0 = sides)` | 1 to sides; 0 for 0 sides; `FFFF_FFFEh` above 255 | answered: 64 + 16 |
| 2 | `history(a0 = buffer, a1 = count 0-16)` | the number of rolls so far; the last `count` rolls written, oldest first | answered: 64 + 1 per 4 bytes |

Its state is 24 bytes: the generator (xorshift32), the number of rolls
and the last 16 rolls. Nothing in it reads a clock or a random source: a
program seeds it from the machine's own `random`, which the host
records, so the same run rolls the same dice, after a snapshot is
restored, and in a replay.

## What makes it experimental

Compared with `../driver-example/terminal.c`, a numbered device, the
driver differs in three places, marked `EXPERIMENTAL` in the file:

1. **HELLO.** The core asks for one slot from 240 to 255 and names the
   device it wants (`example.dice`); the driver answers for that slot
   and, after its identity, with the device's name and the most bytes its
   state can have. The slot is only the image's handle: another program
   could list the same device on slot 251. (An empty name is `terse
   drivers add` asking which device this is.)
2. **Charges.** Every call's charge is in its table entry, the answered
   calls' too, and the core charges by the table: a charge is a function
   of the arguments, never of the driver. `roll`'s RESULT must say 16,
   as declared, or the driver has broken the protocol.
3. **State.** The section is the driver's own layout. The core keeps it
   with its length after the numbered devices' sections (`STATE.md` 1.13)
   and gives it back in RESTORE, so snapshots and recordings work.

The image names the device in a table at the end of its ROM, which
`terse build --optional 240:0=example.dice` writes; `terse info` shows it.
An experimental device can only be optional: a host without the driver
runs the program anyway, with the device absent, which is what `terse
run` and `terse-host` always do.

## The test

`test.sh` builds both, then checks: refusals of a required or unnamed
experimental device; two runs with the same hashes, batched or not, on
shared memory or the socket; the screen; a snapshot taken at frame 100
and restored that continues as the run that never stopped, its file
marked as experimental (version 1), refused by hosts without the driver;
a recording that replays (version 2); the same image not granted on
`terse run`, on `terse-bus` without the driver and with `--without 240`,
with the same hash; installing with `terse drivers add`; and a driver
that breaks each rule on purpose (`TERSE_DICE_BAD`): a buffer over 4,096
bytes or another name refuses the run (status 2), a wrong charge, an input
at BEGIN or a state section longer than declared stop it (status 3).
