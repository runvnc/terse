# A device driver to copy

`terminal.c` is a complete Terse32 terminal (device 2, `SPEC.md` §terminal)
as a device-bus driver (`DEVICES.md`), written to be read and copied: one
file of C11 and POSIX, 570 lines, about 160 for the device, 290 for the
bus and 90 for its identity and a slow mode for tests; socket transport
only, drawing nothing. Machines run with it have the
reference terminal driver's state hashes, under the reference core and
under `native/core-c`'s independent one:

    cc -std=c11 -O2 -Wall -Wextra -o terminal terminal.c
    terse-bus programs/hello/hello.terse --hash --driver 2=$PWD/terminal
    tcore     programs/hello/hello.terse --hash --driver 2=$PWD/terminal
    ./test.sh                         # both cores, against the reference driver
    terse drivers add terminal        # installed: terse-bus uses it from now on

`terse drivers add` installs it in your drivers folder (`DEVICES.md`
2.2), so that `terse-bus` uses it without `--driver`; `terse drivers use 2
default` goes back to the reference terminal driver.

`test.sh` builds it (no warnings with `-Werror`), checks `STATE.md` 3's
`hello` vectors, then runs `hello` (idle and typed into),
`conform-program` with its script (its expected hashes and its expected
screen, every line PASS) and `hostile` (every kind of call at full budget)
under `terse-bus` and under `tcore`, batched and not, and compares every
hash with the reference terminal driver's. It also works under
`terse-bus --shell`: it serves any number of machines.

## Why a terminal

The question was how hard it is to write a device once and snap it into
another implementation. The bus answers it for the core side
(`native/core-c` reuses every reference driver unchanged); this is the
other side. Three candidates were possible; DEVICES.md settles which is
conformant:

- **A partial sound driver** (slot 6, some calls) is not: a driver that
  answers for a slot gives the whole device in the version it states
  (DEVICES.md 13, last paragraph). A program learns from `device` that it
  has sound version 0 and may then use every call; their results, charges
  and state are all specified and all in the state hash (`STATE.md` 1.12).
  There is no partial device.
- **A small made-up device on an unused slot** is not either: slots 0, 5
  and 11–239 are no devices in v0 (SPEC §image), so an image that lists
  one optionally runs without it and one that requires it is refused, and
  neither core asks a driver for such a slot, even one named by
  `--driver` (DEVICES.md 2.1, rule 6). A new device is a SPEC change: a
  device document, and a row in every core's state table (DEVICES.md 1.7).
  (Since then, slots 240–255 are for exactly this: an experimental device,
  which its driver declares; `../driver-experimental` is one.)
- **A complete terminal that draws nothing** is: it has every kind of
  call (deferred and answered, buffers read and written, a checked
  argument, a second result in `a1`), a state section, and an exact
  reference to compare with. Drawing is not part of the device: a driver
  that declares no layers is never asked to present.

The repository already had two drivers outside the Rust workspace's
framework. `crates/terse-devices/c/terminal.c` (1,019 lines) is the same
device but complete as a production driver: shared memory with futexes
and adaptive spinning, RESTORE, rows kept as a ring. It proves a C driver
can match the reference at full speed, but it is not where to start.
`crates/terse-devices/python/vector.py` (303 lines) serves the vector
displays from Python and publishes properties. This file is the smallest
complete one, with every step of the protocol marked.

## Writing a driver: the 10 things you must do

These are DEVICES.md 13's steps; `STEP n` in `terminal.c` marks each.

1. **Connect.** The core starts you with `TERSE_BUS_SOCKET` set; connect
   to that Unix socket. Every message is a u32 length, an 8-byte address
   (instance u32, slot u8, 3 reserved bytes), then the body, whose first
   byte is its type. A reply carries the address of its request.
2. **Answer HELLO** with protocol 1, transport 0 (the socket, even if
   shared memory is offered), your name, native size 0 × 0 if you draw
   nothing, and for **exactly** the slots you were asked for: the slot,
   its device version, no layers, and its **call table** (DEVICES.md 5:
   one 64-byte entry per call, saying where its buffer is, which argument
   is checked, and, for a deferred call, the rule for its result and
   charge). Then three zero counts: no properties, subscriptions or
   services. Then, optionally, your identity: a version and a build
   identifier, two strings (here `0.1.0` and `fnv:` with the FNV-1a 64-bit
   hash of the executable, which `terminal --version` also prints). If you
   cannot serve the slots asked, reply ERROR.
3. **Answer OPEN**: a new machine (instance), with your device at its
   reset state; keep one state per instance. Reply `01h`, or `00h` and a
   reason (a device version you do not implement).
4. **Handle CLOSE**: drop the machine. No reply.
5. **Answer CALL** with RESULT: `a0`, `a1` if the table says the call sets
   it, the charge on top of the base charge, and the bytes the call
   writes, only inside its write buffer. The core has already checked the
   buffer and the checked argument and sent you the bytes you read.
   Handle every function of your table, deferred ones too: a core that
   does not batch sends them as CALLs (`tcore --no-batch` does, and checks
   your answer against your table).
6. **Carry out BATCH**: the deferred calls, each a CALL message, in
   order, possibly for several machines; reply to none. Their results and
   charges were computed by the core from your table, so the table must
   say exactly what your code does.
7. **Answer BEGIN** with INPUTS. With no inputs: charge 0, no events, no
   writes, an empty record. BEGIN's replay field is always there.
8. **Handle END**: the frame is over; do what your device does between
   frames. No reply.
9. **Answer STATE** with your slot's section of the state serialization,
   byte for byte as `STATE.md` says (here 1.6, 10,379 bytes less the
   present flag the core writes).
10. **Exit on SHUTDOWN** or when the socket closes; reply ERROR to any
    message you do not handle and exit.

Then check yourself: run a program that uses your device with
`--hash-every 1` under `terse-bus`, once with the reference driver and
once with `--driver <slot>=<your program>`, and compare; then the same
under `tcore`. The first frame whose hash differs is where to look, and
the state section of that frame (`tcore --state`, or `terse run
--save-state`) shows which byte.

What this driver leaves out, and what it would take: shared memory
(DEVICES.md 4: speed only); layers and PRESENT (section 7: a picture);
properties (7.2: only if another device or the core needs a fact of
yours); RESTORE (1.7: snapshots; parse your section back and refuse one
that would not serialize the same); input records (section 8: only a
device with inputs). The reference core never sends RESET, but the
driver answers it anyway: it is three lines.

## A deliberately slow driver, for tests

Three environment variables make it a test of how a core copes with a
late or starved driver; none changes a state hash.

- `TERSE_EXAMPLE_SLOW=<kind>:<ms>:<from>[:<to>]`: at END of every frame
  whose counter `f` (END's frame counter, after the frame) has
  `from <= f < to` (`to` left out: forever), wait `ms` milliseconds
  (fractional allowed). `sleep` sleeps (blocked, off the CPU); `spin`
  busy-loops for `ms` of wall time; `cpu` busy-loops until its own CPU
  time has advanced by `ms`, a fixed amount of work however long the
  scheduler takes to give it.
- `TERSE_BUS_CPU=<n>` (DEVICES.md 2): pin itself to CPU `n` before
  connecting.
- `TERSE_EXAMPLE_IDLE=1`: run as `SCHED_IDLE`, so a CPU hog on the same
  CPU starves it (a busy computer).

For example, `TERSE_EXAMPLE_SLOW=sleep:30:5:8` makes frames 5, 6 and 7
each about 30 ms late, and `test.sh` checks that the hashes stay the
reference's.
