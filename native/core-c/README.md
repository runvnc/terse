# tcore: a second, tiny Terse32 core in C

`tcore` is an independent implementation of the Terse32 machine's **core**:
the RV32IMC CPU, ROM and RAM, frames and the 125,000-unit budget with its
exact charges, faults and exit, the System device (device 1) with its
event queue and controllers, optional-device grants and `device`, and the
state hash. Every other device stays in the **reference drivers**
(`terse-dev-terminal`, `terse-dev-vector`, `terse-dev-content`,
`terse-dev-page`), which `tcore` starts as separate processes and talks to
over the device bus's socket transport (`DEVICES.md`). It is headless: no
window, no sound.

About 1,520 lines of C11, five files, nothing but libc and POSIX:

| File | Lines | What |
|---|---|---|
| `cpu.c` | 262 | RV32IMC, every faulting encoding of SPEC §cpu |
| `machine.c` | 195 | image, memory map, System device, queue, state serialization |
| `bus.c` | 614 | the core side of the bus: drivers, call tables, batching, frames, STATE |
| `main.c` | 309 | command line, the events file (STATE.md 6.2), `--dump` (6.3), `--bench` |
| `blake3.c` | 76 | BLAKE3-256, one-shot |
| `core.h` | 68 | the machine structure |

The core proper (CPU, machine, BLAKE3) is about 530 lines; the bus client
is the largest part.

## What it proves

It was written in a few hours from the documents alone, without reading
the reference implementation (see "Documents" below), to test two claims:
that someone can build "just the core" of the machine and check it against
the reference, and that the device bus lets a core reuse the reference
devices unchanged. Both hold:

- The `hello` test vectors of `STATE.md` 3, and all 20 expected hashes of
  `programs/conform-program`, `programs/conform` (content, page, 3D) and
  `programs/conform-reading`, frame by frame, with their scripted input.
- The same hashes and exit status as `terse run` for `holo` (device 9
  without device 3), `sketch` and `sparks` (optional content, granted or
  not, sessions with keys and a controller), `shell` run alone, `hostile`
  (every kind of call at full budget), `hello` typed into.
- Images with only the System device, and no driver at all; faults of every
  kind; exits; images with optional devices whose driver is missing or
  withheld; header edge cases (refused or run as the reference does);
  thousands of random RV32IMC programs (`tests/fuzz.py`).
- The deferred calls' results and charges computed from the drivers' call
  tables agree with what the drivers themselves answer (`--no-batch`).

`SPEC-FINDINGS.md` lists every question the documents left open (31; three
first guesses were wrong, found by comparing with the reference). All 31
are now settled in the documents, and `tcore` follows each written rule
(its "Resolved" table says where).

## Building and running

    make -j4                       # cc -O2 -Wall -Wextra, no warnings
    ./tcore programs/hello/hello.terse --frames 30 --dump --drivers target/release

The reference drivers are built with the workspace
(`scripts/cargo build --release -p terse-devices -p terse-cli -p terse-bus`).
`tcore` looks for them as DEVICES.md 2.1 says: in one directory,
`--drivers <dir>` (or `TERSE_DRIVERS`), by default its own; never on
`PATH`. A `--driver` program is a path or a name on `PATH`.

    tcore <image.terse> [--frames N] [--events <file>] [--hash] [--hash-every N]
          [--dump] [--state <file>] [--drivers <dir>] [--driver <slots>=<command line>]...
          [--without <slot>]... [--no-batch] [--bench]

- `--frames N` (default 30), stopping earlier if the program exits or faults.
- `--events <file>`: the scripted input of `terse run --events`
  (STATE.md 6.2): `down`, `up`, `press`, `text`, `pointer`, `click`,
  `wheel`, `feeds-updated`, `home` (dropped: no shell), `pad`. Time is 0
  and there are no random bytes, as in `terse run` (STATE.md 6.1).
- `--hash`, `--hash-every N`: as `terse run` (STATE.md 3). `--state <file>`
  writes the final serialization itself.
- `--dump`: the terminal's grid after the last frame, from the terminal
  driver's STATE section, as STATE.md 6.3 says.
- `--driver 4="/path/terse-dev-content --store … --config … --data …"`:
  replaces the driver of those slots (the content driver needs its
  directories: without them it uses the user's own). A `--driver` that
  cannot be started refuses the run (status 2).
- `--without <slot>`: withholds an optional device. An optional device whose
  default driver is not installed is not granted; a required one is
  refused (status 2).
- `--no-batch`: send deferred calls one at a time and check the driver's
  answer against the call table.
- `--bench`: frames per second, units per second, messages by type.

Exit status (STATE.md 6.4): the program's exit code; 143 after a fault
(with the fault on standard error); 2 if the run did not start (a refused
image, a bad events file, a missing required driver, a driver whose
handshake failed); 3 if a driver fails while the machine runs.

`./test.sh` (or `make test`) runs everything above against the expected
files and the reference `terse run`, in a temporary directory (HOME, store,
config, data), and checks that every driver process was reaped.
`tests/build.py` rebuilds the hand-made images from `tests/*.S` (clang,
ld.lld, llvm-objcopy, `terse pack`).

## Speed

On the development machine (headless, socket transport):
`conform-program`'s 200 frames take about 0.06 s (≈3,300 frames/s; frames
49–108 spend their whole budget); a ROM loop runs at about 88 million
instructions per second, ≈700 full frames per second, against ≈1,600 for
`terse run`. The interpreter is a plain `switch` with no decoding cache.
Answered calls cost a socket round trip each, ≈10 µs (`hostile` runs at
≈180 frames/s).

## What it omits

Everything DEVICES.md lets a headless single machine leave out: shared
memory, presentation (PRESENT, layers, the window, the fault line),
properties (GET/SET: only needed before presenting, and for device 8), host
services (a shell's), RESET, CLOSE and RESTORE (no snapshots), recording and
replay. It runs one machine, never a shell (device 8 is absent, as SPEC
§image says for a machine run alone), and supplies no clock, randomness or
controllers but the script's. The messages it uses: HELLO, OPEN, CALL and
RESULT, READ and DATA (the content driver reads its data ranges), BATCH,
BEGIN and INPUTS, END, STATE and SHUTDOWN.

## Documents it was written from

`SPEC.md` (the core machine), `STATE.md`, `DEVICES.md`,
`docs/manual/machine.txt`, `state.txt`, `writing-programs.txt`; the program
images, their events and expected files; `terse info`, `terse run`,
`terse-bus` and the drivers as black boxes; the RISC-V unprivileged
specification for encodings. Not the reference sources, the C programs or
their library, nor the device documents of `spec/`.

## How to write your own core

The order that worked, with the traps:

1. **BLAKE3 and the serialization first.** With the CPU at reset and the
   terminal's STATE from its driver, `hello`'s reset hash (STATE.md 3) tests
   the image ID, the header, the RAM and the section layout before a single
   instruction runs.
2. **The CPU.** Follow SPEC §cpu's fault list literally; the compressed
   reserved encodings (zero immediates, `rd = 0`, RV32 shift amounts) are
   where it goes wrong. Fault means: not executed, not charged, `pc` stays.
   Charge 1 or 4 by where the instruction was fetched.
3. **The frame loop and System calls.** Budget: add 125,000, run while
   positive, keep a negative remainder, drop a positive one. `wait_frame`
   returns the next frame counter. `exit` leaves `a0` = 0 (a trap). A buffer
   of length 0 is valid anywhere; a written buffer must be in RAM.
4. **The bus.** DEVICES.md 12 is the list: HELLO, OPEN, then per frame
   BEGIN (its replay field is always there), the calls, END; STATE when
   hashing, in the order of DEVICES.md 1.7's table. Apply the call table's
   checks yourself and charge 4 or 64 even when a check fails. Ask the
   vector driver for slots 3 and 9 together, and the terminal's for slot
   10 when the image lists it.
5. **Events and the queue**: 64 at most, one feeds-updated a frame, drivers'
   events by position. Then `conform-program` with its script: if a PASS line
   fails the screen says which section; if a hash differs, `--state` and a
   byte diff against `terse run --save-state` find the field.
6. **Batching** last, for speed only: everything matches without it.
