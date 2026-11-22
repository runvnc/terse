# Terse responsiveness — v0 (draft)

Terse is meant to be light and quick where the web is not: what you
already have opens at once, and what you do not have costs one round
trip. This document collects the responsiveness requirements, says how
each is tested, and gives the reference implementation's times on a
named machine as information.

**The requirements live in each specification**, so that an implementer
still reads only their own document; this one repeats them in one table,
with the method:

- a reading client: `NETWORK.md` section 6.1 (and section 2);
- a node and a reader on the local socket: `LOCAL.md` section 4.5;
- a machine host: `SPEC.md` §responsiveness and `spec/content.md`
  §content.submit; what a frame may ask of a host's drawing: `SPEC.md`
  §vector.ink-cap and `spec/vector3d.md` §vector3d.caps;
- what a host does about its own slow components (R13–R16): `SPEC.md`
  §stopping and §responsiveness, `DEVICES.md` 1.1 and 10, and the
  record's format, `RECORD.md`.

**Three tiers: requirements, ceilings and targets.**

- A **requirement** (R) is stated in units that any implementation can
  be tested against on any machine and network: frames (at 60 a
  second), round trips (messages a relay receives), and what may not
  wait on what ("asks no relay", "does not wait for a relay"). It passes
  or fails.
- A **ceiling** (C) is in milliseconds (or frames), set far above what
  any correct implementation takes on any reasonable computer: a few
  times the target, and more than the reference would take on the
  hardware floor below. It fails on any machine: a time over its ceiling
  means something is wrong (a wait on the network or the disk, a search,
  a lost cap), not that the computer is slow.
- A **target** (T) is the reference implementation's time on the
  reference machine below, with some room. It is information for
  implementers and a guard against regressions of the reference: it
  fails only with `terse perf --strict`, which the reference machine
  (and anyone holding their build to these numbers) runs.

Ceilings and targets judge the **median** of several runs (5 by
default), so one slow run on a busy machine fails nothing and a
consistently slow one does. Every timed scenario has both (§1.1).

**The hardware floor** the ceilings assume: a 2-core, 2 GHz x86-64 or
ARM64 computer from the last ten years (an Intel Core i5-6200U laptop,
or a Raspberry Pi 5), with 4 GB of RAM and an SSD, and a GPU driver if
the GPU renderer is used. Its single-thread speed is at worst about 2.5
times slower than the reference machine's (5 GHz turbo against 2 GHz on
the same Skylake core; a Raspberry Pi 5 is in the same class), its
multi-thread speed about 7 times. Each ceiling is at least 3 times its
target, and was checked by running `terse perf` on the reference slowed
to that floor (§1.1).

**An implementation is expected to run `terse perf`**, or its
equivalent, and pass: every requirement and every ceiling. A host or a
reader other than the reference plugs in with `--host <terse-host>` and
`--reader <tersereader>` (the rows run the same scenarios through it, so
it must take `terse-host`'s or `tersereader`'s command line, §2); one
that cannot runs the same scenarios its own way and holds itself to the
same limits.

**The reference machine**: an Intel Core i7-10750H laptop (6 cores, 12
threads), 15 GB of RAM, NVIDIA RTX 2060, Linux (CachyOS), with about a
thousand font faces installed (which no longer matters: the host draws
text only with the fonts it loads at start, §3). Times are the median of 5 runs, with the
spread, from `terse perf` on a release build.

## 1. Requirements

| Layer | Id | Requirement | Unit | How it is tested |
|---|---|---|---|---|
| Reading client | R1 | An article or blob the client holds is shown from its store: no request to any relay, no wait for one | relay requests; no wait | `terse perf` C2, C3, G2, G3: a relay that counts the Gets it receives; a relay that accepts the connection and never answers |
| Reading client | R2 | A blob not held, on a connected relay: one round trip, nothing before it | round trips | C5, M2: the relay's count of Gets |
| Reading client | R3 | The timeline and subscriptions are shown from what is held, never after waiting for a relay | no wait | C4, G4, M4 with a relay that never answers |
| Reading client | R4 | Catching a feed up on a relay is one Subscribe; its entries are pushed | round trips | C1, G1: the relay's count of Subscribes |
| Local node | R5 | Held, Timeline, Subscriptions, Feed, Relays, Title and Get of a held blob are answered from the node's state, not behind a request waiting on the network | no wait | G5: the Go node with a relay that never answers |
| Machine host | R6 | A content request that needs no network (a held blob, the host's own state) completes within 2 frames of its submit | frames | `programs/conform-perf` in the host's frame loop (`terse perf` M10); `terse perf` M1, M3, M4; `cargo test` (below) |
| Machine host | R7 | An input event received before a frame begins is in that frame's input | frames | by construction in the reference's frame loop; scripted input (`--selftest`) |
| Machine host | R8 | Frames run at 60 a second while the host keeps up, never faster to catch up; the frame loop never waits on the network, the user or storage | frames a second | `programs/conform-perf` (frames counted against the clock over four seconds; `terse perf` M10) |
| Machine host | R9 | A held image a shell starts runs its first frame within 2 frames of the start call | frames | `terse perf` M6 |
| Machine host | R10 | The host work a frame can demand is bounded: what a machine's displays make a host draw in a frame is within the caps, counted in square screen units of ink from the lists alone (the 2D list at most 230,400, `SPEC.md` §vector.ink-cap; the 3D list at most 153,600, with its segment, face and faces × segments caps, §vector3d.caps), the terminal's 2,400 cells and the page's lines in view (its layout outside the frames, §page.layout) | square screen units; cells | `terse perf` W rows: each device's worst legal list, built to ask far more than the cap, is drawn only up to it (PASS or FAIL), and its drawing time on one thread is a target; `cargo test -p terse-core` (`vector`, `vector3`: the counts and where the lists are cut) |
| Machine host | R11 | The frame loop never waits on finding or loading a font: every font is opened when the host starts, and no file is opened, nor any font directory read, for text after that; a glyph is rasterised on first use, at a bounded cost per glyph | files opened | `cargo test -p terse-host fonts` (the fonts are loaded from bytes the host holds: built in, or files read once at start, which the test counts); `terse perf` H2 and the W row `term-full`: the first frame of the whole full repertoire |
| Host and devices | R13 | Every host and every device implementation states who it is: a name, a version and a build identifier (`DEVICES.md` 1.1's HELLO reply for a driver; a host's own for its built-in devices; `--version` of every program). No machine can read any of it | fields present | `cargo test -p terse-bus` (proto), `native/core-c/test.sh` (`--identities`), `native/driver-example/test.sh`, the drivers' `--version` |
| Machine host | R14 | The host attributes each late frame to one of its components, by that component's own time (its time less the time it waited for a CPU), and keeps a record of it (`RECORD.md`) that is bounded, written off the frame loop and never sent | frames; bytes | `cargo test -p terse-perflog` (the judge, the format, `vectors/perf-record.txt`); `cargo test -p terse-devices --test slow_driver` (a blocked driver, a spinning one and a starved one) |
| Machine host | R15 | A component that makes frames late is not tolerated: after its 3rd late frame within 60 the host warns the user and saves the machines that use it; after its 10th within 300 it saves and stops them (`SPEC.md` §stopping, stopped by the host). The host's own components are recorded and shown | late frames | `cargo test -p terse-devices --test slow_driver` (warned at the 3rd, stopped at the 10th, the snapshot restores and goes on identically, a replay stops at the same frame) |
| Machine host | R16 | A host checks at start that this computer can draw each device's worst legal frame in time, once per build and computer; where it cannot, the optional look is not offered and the device is said to be too slow here | ms (a target) | `terse-host --hardware-check`; `cargo test -p terse-host hwcheck` |
| Machine host | R12 | The host work a whole system can demand is bounded: the machines a shell shows share the system ink, 691,200 square units of the shell's screen (their pictures' areas first, then their lists' ink times their scale; a picture that does not fit is a plate, a list is cut where the ink runs out), the shell's own screen drawn within its own caps (`spec/machines.md` §machines.ink) | square screen units | `terse perf` W rows `sys-*`: a shell and 8 machines, each with every device at its worst, tiled, overlapping and as panels, with the system ink and without (target: the machines add no more than about one machine's worst); `cargo test -p terse-core --test system_ink` (plates, scales, where lists are cut); `cargo test -p terse-cli --test shell` (`the_layouts_stay_within_the_system_ink`: the bundled layouts are never cut); `cargo test -p terse-devices --test pictures` (terse-bus counts the same shares, frame by frame, and draws the same plates and cut lists) |
| Machine host | R17 | The first frame waits on nothing but the image, the settings and the fonts they list: before it a host reads only the image (a shell's built-ins), its settings, the listed fonts, its own store and data directories and its hardware check's result (R16), besides opening its window and graphics device; it waits on no relay, reads no font directory and asks the user nothing (`SPEC.md` §responsiveness item 6) | files read; no wait | `cargo test -p terse-host --test startup`: the built-in shell's first frame with a relay that never answers and the user's font directories, a fontconfig file's and the listed font's own directory holding only FIFOs (which block whoever opens them): it comes at once, and nothing is written outside the store and data; `terse perf` H1, H5, H7, H8 (ceiling 1.5 s) |

What the machine already guarantees needs no row: a frame's work for a
program is bounded by the budget (`SPEC.md` §frames, §charges), so no
program can make a frame slow on a host that keeps up with a frame of
instructions. R10 is the same for the work a call hands to the host,
which the budget does not see.

**R10's calibration** (information, not a requirement): the caps are set
so that every device at its cap at once, the worst frame a legal machine
can ask for, takes the reference software renderer (`terse-host
--renderer software --threads 1`, in its default picture of 880 × 660
pixels) on the reference machine below no more than one frame, 16.7 ms
(§4). Each device alone takes at most about 7 ms, so a host 4 times
slower than the reference falls at most a few frames behind in the very
worst case, and keeps up with every bundled program.

### 1.1 Ceilings and targets

Every timed row of `terse perf` has a ceiling (C: fails on any machine)
and a target (T: the reference machine's; fails only with `--strict`),
judged on the median of its runs (`crates/terse-cli/src/perf.rs`, the
`Limit` constants). "Reference" is the median measured on the reference
machine (`terse perf --runs 5`, 2026-11-21, load average 1.2 to 2.6);
"floor" the median with the same run on the reference slowed to the
hardware floor: 2 of its cores and 40 % of one core's time between them,
given in 1 ms slices (`systemd-run --user --scope -p AllowedCPUs=0,1 -p
CPUQuota=40% -p CPUQuotaPeriodSec=1ms terse perf`), so one thread runs
2.5 times slower (measured) and two threads 5 times: harsher than the
floor wherever more than one thread works. Every requirement and every
ceiling passed there too.

| Limit | Rows | Ceiling | Target | Reference | Floor |
|---|---|---|---|---|---|
| A held article or the timeline, shown by a whole process | C2–C5, G2–G5 | 250 ms | 25 ms | 2.0–11.6 ms | 4.7–27.2 ms |
| A feed of 50 entries synced from a local relay | C1, G1 | 2 s | 200 ms | 49 / 25 ms | 70 ms / not re-measured |
| A key to what it shows, held, real-time loop | M7–M9 | 15 frames | 3 frames | 1–2 frames | 1–2 frames |
| Host start to its first frame, software renderer | H1, H5 | 1.5 s | 50 ms | 19 / 19 ms | 48 / 57 ms |
| The same, cold: a first run, terse-host not cached | H7 | 1.5 s | 100 ms | 63 ms (warm 20) | 62 ms (warm 55) |
| The same, GPU renderer (the driver's start) | H8 | 1.5 s | 500 ms | 293 ms (276–1,102) | 1,178 ms (892–1,954) |
| A worst frame of the host's drawing, 1 thread: fetch's slowest, conform's stress, the full repertoire's first, each W device row, the phosphor look shed | H2–H4, W | 4 frames (66.7 ms) | 1 frame (16.7 ms) | at most 15.6 ms (W all) | at most 46.6 ms (W all) |
| 1,200 new CJK glyphs a frame from a listed font | H6 | 6 frames (100 ms) | 2 frames (33.3 ms) | 20.5 ms | 64.9 ms |
| A shell and 8 machines, every device at its worst | W sys-* | 12 frames (200 ms) | 4 frames (66.7 ms) | 32–60 ms | 95–148 ms |

Why these multiples. A ceiling must hold on the floor with room, and be
low enough that a real fault crosses it: a wait for a relay's 10 s
timeout, a scan of the system's fonts (a second or more on the
reference before R11), a cap not enforced (the worst legal frames took
up to a minute before R10). Start-up keeps the owner's 1.5 s: 3 to 30
times the reference, and the GPU's start on the floor (1.2 s, the
tightest row) still under it. A worst frame may take 4 frame times on
one thread: the floor takes at most 47 ms (W "all"), and a missing cap
costs far more. The system rows, 9 machines at their worst, get 12.

**Targets tightened** (2026-11-21), each to a few times what the
reference now measures: a held article or the timeline from a whole
process 50 → 25 ms (it measures 4 to 12 ms); host start-up 500 → 50 ms
(18 ms since the fonts are built in; the owner's ceiling of 1.5 s is
kept); fetch's slowest frame 50 ms → 1 frame (0.7 ms); a key to what it
shows 6 → 3 frames (1 to 2). Feed sync 500 → 400 → 200 ms (55 ms; the Go
reader's 300 ms had set 400. That was its wait for idleness, polled
every 100 ms, not the fetching, and is fixed: it measures 25 ms). **Targets loosened**: 1,200 CJK glyphs never drawn before from a
listed font, each frame (H6), 1 → 2 frames: the reference takes 21 ms,
so `--strict` would fail on the reference itself, for a cost that is
known and not bounded yet (§4, "Text in another script"). The system rows' target was
"2 × the single machine's", which tiles (41 ms against 16) missed; it is
now 4 frames on one thread. **New**: the cold start H7 (target 100 ms)
and the GPU renderer's start H8 (500 ms: the driver's start-up, about
290 ms on the reference).

**Cold start (H7).** The first start a new user has: the built-in
shell's first frame with an empty store, config and data directory and
a new HOME (so the host puts the built-in programs and the manual in
the store), and `terse-host`'s own file not in the page cache. `terse
perf` drops the executable's pages with `posix_fadvise(DONTNEED)`, which
needs no root and touches no other file's, retrying until `mincore`
shows none resident (a process that has just exited keeps them mapped
for a moment), and prints how many were resident at start. The shared
libraries it links (libc, libm, libgcc_s, libudev) are left: every
desktop's processes keep them resident. Where the pages cannot be
dropped (tmpfs, or not Linux) the row says so and reports each run as a
first run, binary cached. The warm start of the same is in the same row.
H7 runs `--bench-render`, which makes no hardware check (R16): a
window's very first start on a computer adds that check once, 58 ms on
the reference (138 ms on one thread).

## 2. Running the tests

**On the reference** (a release build, the Go reader built if you want
its rows):

    cargo build --release -p terse-cli -p terse-host
    (cd native/reader-go && go build -o tersereader .)
    target/release/terse perf

`terse perf` runs every scenario headless, in temporary directories,
with relays it starts on ephemeral loopback ports (never 7878, never your
config, cache or data directories), and prints one line per scenario:
its id, what it measures, the measurement (for a timed one the median
of its runs and their range), its requirement (R1–R17, §1), ceiling (C)
and target (T), and the result: PASS, FAIL (naming the requirement,
ceiling or target that failed), "PASS; target missed", `info` or
`skipped` (terse-host or the Go reader not found). A last line counts
the requirements, ceilings and targets judged and the failures. It exits
with 1 if anything failed: a requirement or a ceiling, and with
`--strict` a target. `--only machine|client|host` runs one group, `--runs
n` changes the 5 runs of every timed scenario. The machine scenarios run the content service and the
system as `terse-host`'s window does, a frame every 16.7 ms with each
frame given only what finished before it began (`Headless::realtime`),
not the headless way of `terse run`, where every request completes in
the next frame by definition.

**In `cargo test`**, only what does not depend on the machine's load:
the round-trip counts and "asks no relay" (`terse-net`'s node tests,
`a_held_blob_never_waits_on_the_network`), held content with a relay
that never answers finishing long before that relay's 10 s timeout
(`terse-cli --test conform_perf`), and `programs/conform-perf`'s
headless screen and state hashes. The timed scenarios stay in `terse
perf`: on a busy machine they would fail for the wrong reason.

**For another implementation:**

- *A reading client*: run it against a relay that counts what it
  receives (`terse-relay` does, through `terse_relay::Relay::received`;
  or put a counting proxy in front of any relay) and against an address
  that accepts TCP connections and never answers. Open an article it
  holds (no Get, no wait), one it does not hold (one Get), its timeline
  (no wait), and sync a feed (one Subscribe). `terse perf`'s rows C1–C5
  and G1–G5 do exactly this for the two reference clients.
- *A machine host*: run `programs/conform-perf/conform-perf.terse` in the
  host's real frame loop (a window, or a self-test with no scripted
  input): every line must say PASS. Run headless with no input (and an
  empty store, as `crates/terse-cli/tests/conform_perf.rs` does) it must
  show `programs/conform-perf/expected-screen.txt` and its state hashes
  must be `expected-hashes.txt`, like the other conformance images
  (`SPEC.md` §conformance).
- *Its times*: `terse perf --host <path>` runs the H and W rows through
  another host that takes `terse-host`'s options for them
  (`--bench-render`, which prints the frames' p50, p99 and max;
  `--renderer`, `--threads`, `--frames`, `--warmup`, `--store`,
  `--config`, `--data`, `--shell`, `--builtin`); `--reader <path>` runs
  the G rows through a reader that takes `tersereader`'s. Every
  requirement and every ceiling must pass. An implementation that takes
  other options runs the same scenarios its own way, judged by the same
  limits (§1.1) on the median of 5 runs.

## 3. Targets on the reference machine

Measured with `terse perf --runs 5` on the machine above, release build,
2026-11-21, load average 1.2 to 2.6: the median, with the lowest and
highest run. Every requirement, ceiling and target passed, also with
`--strict`. The ceilings are in §1.1.

| Id | Scenario | Measured | Target |
|---|---|---|---|
| M1a–c | held get (fetch's message): no relay / a connected relay / a relay that never answers | 1 frame each; the service answers in 0.1 ms / 6 ms / 0.07 ms; no relay asked | — (R6: 2 frames) |
| M2 | get of a blob on a connected local relay | 1 Get; 6.4 ms; fetch's get done in 1 frame | — |
| M5 | shell start to the launcher on screen | 4 frames (the shell's own) | — |
| M6 | Enter on fetch in the launcher to fetch's first frame | 1 frame | — (R9: 2 frames) |
| M7 | Enter on fetch to its message on the screen | 2 frames (33 ms) | ≤ 3 frames |
| M8, M9 | reader: M to the manual's index; Enter on a link to the article | 1 frame each | ≤ 3 frames |
| M10 | conform-perf in the real-time loop | every line PASS: 1 frame, 1 frame, 60 60 60 60 frames a second | — |
| C1 | `terse feed sync`: 50 entries and their articles | 49 ms (47–54); 1 Subscribe, 50 Get | ≤ 200 ms |
| C2, C3 | `terse get` of a held article (the whole process); relay connected / never answering | 7.0 ms (6.9–8.1) / 6.0 ms (5.9–6.0) | ≤ 25 ms |
| C4 | `terse feed list`, relay never answering | 4.1 ms (4.0–4.4) | ≤ 25 ms |
| C5 | `terse get` of an article on a local relay | 7.4 ms (6.9–8.0); 1 Get | ≤ 25 ms |
| G1 | `tersereader sync`: 50 entries and their articles | 25 ms (24–32); 1 Subscribe, 50 Get | ≤ 200 ms |
| G2, G3 | `tersereader --dump` of a held article; relay connected / never answering | 11.6 ms (11.0–16.6) / 10.2 ms | ≤ 25 ms |
| G4 | `tersereader list`, relay never answering | 10.7 ms | ≤ 25 ms |
| G5 | the same through the Go node on the local socket: `--dump`, `list` | 2.0 ms (1.7–2.1), 2.3 ms | ≤ 25 ms |
| H1 | `terse-host` start to hello's first frame drawn (software, offscreen) | 19 ms (18–27; before the built-in fonts: 202 ms, 150–513) | ≤ 50 ms |
| H7 | cold start: the built-in shell's first frame on a first run, terse-host not in the page cache (§1.1) | 63 ms (61–82); warm 20 ms | ≤ 100 ms |
| H8 | H1 on the GPU renderer (RTX 2060, the driver's start) | 293 ms (276–1,102) | ≤ 500 ms |
| H2 | fetch's slowest frame (its first Greek, Cyrillic and symbols, 50 ms after start) | 0.7 ms (before, its CJK and Hangul line: 102 ms) | ≤ 1 frame |
| H4 | the first frame showing 2,400 glyphs of the full repertoire never drawn before, 1 thread (`term-full`) | 8.7 ms (the first frame of basic text, 90 glyphs: 4.4 ms) | ≤ 1 frame |
| H5 | H1 with a 19 MB CJK collection listed as an extra font | 19 ms (18–32) | ≤ 50 ms |
| H6 | 1,200 CJK glyphs never drawn before, each frame, from that font, 1 thread (`term-cjk`) | p50 20.5 ms, max 24 ms | ≤ 2 frames |
| H3 | programs/conform's stress frames 49–108 on the software renderer, 1 / 12 threads | p50 6.3 / 1.6 ms, max 8.5 / 2.0 ms (before the ink caps: p99 1,678 / 361 ms) | ≤ 1 frame (p50) |
| W | each device's worst legal frame, and all of them at once (§4) | every cap held (PASS); 1 thread at most 6.5 ms a device, 15.6 ms all at once | ≤ 1 frame (p50) |
| W sys-* | a shell and 8 machines at their worst (§5) | 32 to 60 ms | ≤ 4 frames |

The machine rows' milliseconds are those of the content service; the
frames are what a program sees. Drawing times of the renderers for the
usual scenes are in README "Software renderer".

**Fonts (R11).** A frame that first showed characters the monospace
font lacked (fetch's Japanese, Chinese and Korean line) once took 0.95
to 1.7 s, the host looking through the system's fonts one file at a
time and parsing the one it found whole; later 90 ms, waiting for a map
of every installed font's characters built as the host started. The
host now draws only with fonts it loads at start (`SPEC.md`
§terminal.text: three built in, and those the user lists) and never
looks for another: start-up fell from 202 ms to 22 ms (no scan of a
thousand font files), and a glyph costs only its rasterising, once.
The worst frame of the full repertoire, 2,400 glyphs none drawn before
(H4), takes 9.2 ms on one thread, within a frame; it can happen once
per screen size, since there are only 2,461 of them. A font the user
adds is memory-mapped and checked, not read: listing a 19 MB CJK
collection adds nothing measurable to start-up (H5 against H1) and
nothing to the memory until its glyphs are drawn. Those glyphs cost
what they did before (H6: 1,200 new CJK glyphs a frame, 22 ms, about
18 µs a glyph), which misses a frame by 5 to 9 ms in that worst case; a
real page of CJK text uses far fewer new characters a frame. Memory
(peak, 30 frames): hello 21 MB, fetch 22 MB (71 MB before, with the
font map), the CJK frames 28 MB as placeholders and 43 MB from the
listed font.

**Requests with a relay connected.** The content service polls each
connected relay's socket with a 5 ms read timeout, so a request that
arrives while it is polling waits up to 5 ms to be started (M1b: 7 ms
for a held blob, still within the frame). A poll used to wait 5 ms more
after the last message it received, which added 5 ms to every reply
(M2: 12 ms before, 6 ms now).

## 4. What a frame can ask of a host (R10)

Measured 2026-11-21 on the reference machine, release build, software
renderer at 880 × 660 (the default picture of `--bench-render`), GPU
renderer on the RTX 2060, load average 2 to 5 (other work running): the
median of 3 runs of 30 frames (the slowest scenarios before the caps: 1
run of 2 frames). `terse perf --only host` runs the W rows; `terse perf
--write-worst <dir>` writes their images (`terse_core::worst`, made
without a compiler) and `terse run --work` prints the most any machine's
displays asked for in a frame.

**Before the ink caps** (the fill cap of 4 screens of filled shapes, the
3D length cap of 65,536 units; nothing on lines and outlines), the worst
legal frames found, drawing time per frame:

| Device | Worst legal list | 1 thread | 12 threads | GPU | What bounded it |
|---|---|---|---|---|---|
| 2D | 5,454 circle outlines 16 units wide (3 bytes each) | 1,854 ms | 294 ms | 43 ms | nothing (lines and outlines had no cap) |
| 2D | 3,267 lines 16 units wide across the screen (conform's list B) | 1,217 ms | 204 ms | 3.5 ms | nothing |
| 2D | fill mode 1: 3,275 screen-sized rectangles as 16-unit outlines | 3,112 ms | 788 ms | 9.0 ms | nothing (outlines were not under the fill cap) |
| 2D | phosphor style: the circles; the lines | 57,513 ms; 21,901 ms | 7,215; 3,141 ms | 324; 113 ms | nothing |
| 2D | screen-sized translucent discs; rectangles, at the fill cap | 32.6 ms; 16.9 ms | 7.1; 2.9 ms | 0.5 ms | fill cap, 4 screens |
| 3D | lines 16 units wide up to the length cap | 39.6 ms | 7.3 ms | 0.5 ms | length, 65,536 units |
| 3D | 16,384 points 16 units wide | 20.5 ms | 4.9 ms | 1.1 ms | segments, 16,384 |
| 3D | phosphor: those lines; points | 699 ms; 1,291 ms | 121; 180 ms | 5.4; 9.2 ms | the same |
| 3D | 4 Surfaces of the whole screen | 20.6 ms | 3.9 ms | 1.2 ms | 4 Surfaces |
| 3D | hidden lines: 256 faces × 512 segments; 8 × 16,384 | 2.1; 5.0 ms | 1.4; 2.0 ms | 1.0; 0.5 ms | faces × segments, 131,072 |
| Terminal | every cell rewritten each frame, basic text | 2.7 ms | 0.7 ms | 0.4 ms | 2,400 cells |
| Terminal | full text: 1,200 CJK characters never shown before, each frame (from an extra font, since the built-in fonts) | 23 ms | 21 ms | 31 ms | 2,400 cells (not the font work) |
| Page | 65,536 bytes of size-6 text, scrolled every frame | 3.6 ms | 1.8 ms | 1.8 ms | lines in view; layout outside the frames |

Sound needs no row: its mixing is the machine's own, inside the frame's
budget (16 voices, 800 pairs a frame, `spec/sound.md` §sound.render),
not host work. The GPU renderer was never the problem (at most 0.3 s,
for 2D phosphor circles); the software renderer took up to a minute.

**The caps.** One unit for every display, square screen units of
**ink**, computed from the list alone: a filled shape half its box, a
stroke (L + w) × (w + 3 units) or its box on the screen if smaller (the
3 units are the edge a host smooths: before they were added, hair lines
cost the reference three times what wide ones did per unit counted), a
Surface its box on the glass. Per unit of ink the reference costs about
the same whatever the list is made of: 4.5 to 6.5 ms per 230,400 units
on one thread. The caps: a 2D list ¾ of a screen (230,400), a 3D list ½
(153,600), with the 3D display's earlier caps kept (the length cap can no
longer be reached: a segment's ink is more than 3 times its length). A
list is cut where it passes its cap, the convention the fill cap and the
3D caps already had, so nothing is charged and the machine's state does
not change. The software renderer also takes the inside of a wide line
or disc as covered without measuring each pixel (the same picture:
`soft::shapes` tests it pixel by pixel), which halved its cost per unit
for wide strokes.

**After**, the same worst cases, each asking 2 to 3,600 times its cap
and drawn only up to it:

| Device | Worst legal list | 1 thread | 12 threads | GPU |
|---|---|---|---|---|
| 2D | half-screen translucent rectangles to the cap | 6.5 ms | 1.5 ms | 0.3 ms |
| 2D | discs to the cap | 5.4 ms | 1.1 ms | 0.3 ms |
| 2D | lines 1/16, 2 and 16 units wide; circles; outlines | 4.5, 3.8, 3.4; 3.5; 4.5 ms | 1.0 to 2.1 ms | 0.3 ms |
| 3D | points 16 units wide; behind 256 faces; lines | 3.9; 4.1; 2.8 ms | 1.3; 1.4; 0.8 ms | 0.4 to 0.5 ms |
| 3D | hidden lines, 256 faces; Surfaces to the cap | 1.2; 3.4 ms | 1.1; 1.4 ms | 0.6 ms |
| Terminal, page | as above | 2.6, 3.4 ms | 0.8, 1.7 ms | 0.3, 1.5 ms |
| **All at once** | terminal rewritten, page scrolled, 2D rectangles and 3D points behind faces at their caps | **15.8 ms** (15.8 to 16.2; slowest frame 16.6 to 17.7) | 4.1 ms | 1.9 ms |

The bundled programs, the most their displays ask for in any frame
(`terse run --work`) against the caps: sparks 187,843 of 230,400 (its
red flash over the arena, half a screen of fill, with its sparks; about 40,000
without the flash); sketch with a drawing it calls full, 130,037 (a
drawing of fast strokes 60 units long, 480,767: its last strokes are not
drawn, the one bundled program the 2D cap cuts); holo's city 50,615 and
cockpit 95,745 (its Surfaces) of 153,600, its other scenes under 31,000;
the shell's own lists in its gallery of 5 machines 9,020 (2D) and 56,296 (3D: its ring and labels; its Screens are not
counted, below); reader, fetch, hello and tune draw no vector list.

**Not bounded yet, by design or for a decision:**

- *The look.* The phosphor style's glow, afterglow and smear are the
  host's choice (`SPEC.md` §vector.phosphor) and are not counted; a host
  must draw only plain lines, which cost what the clean style does. The
  reference sheds the look by the ink a frame's light draws (as the caps
  count it, so the same picture on every run): above a quarter of a
  screen no smear (3 to 15 more copies of each moving shape), above a
  third no halo, above two fifths plain lines, flat strokes of their
  width (`phosphor::shed_level`); the bundled programs draw at most about
  33,000 (holo) and keep the whole look. In a window it also sheds one
  level more after 3 frames in a row that took longer than a frame to
  draw, and one less after 120 that took under half a frame
  (`phosphor::Governor`; never for `--screenshot` or `--bench-render`).
  `--no-shed` keeps the whole look. At the caps, p50 shed (in full):

  | Worst list, phosphor style | 1 thread | 12 threads | GPU |
  |---|---|---|---|
  | 2D lines 16 units wide | 3.7 (103.8) ms | 2.1 (55.1) ms | 0.6 (1.7) ms |
  | 2D circles | 3.7 (93.0) ms | 1.0 (16.9) ms | 0.5 (1.6) ms |
  | 3D lines | 6.4 (85.7) ms | 1.7 (16.1) ms | 0.6 (1.7) ms |
  | 3D points 16 units wide | 11.1 (70.8) ms | 3.3 (14.3) ms | 1.0 (2.7) ms |

  Before, with only the smear shed: 24.7, 19.7, 28.8 and 75.6 ms on one
  thread. The GPU sheds the smear and the halo; its beam costs it
  little, so it draws the plain level as the halo one.
- *A system of machines* is bounded by R12, §5.
- *Text in another script.* The terminal's 2,400 cells are bounded, and
  the full repertoire's worst frame fits (9.2 ms, §3). With a CJK font
  the user added, a frame of 1,200 CJK characters the host has not drawn
  before costs it 23 ms (glyphs rasterised; nothing is looked for) on
  any renderer. A rule that lets a host show a new glyph a few frames
  late, as the page may show a figure late, would bound it.

## 5. What a system can ask of a host (R12)

Each machine's caps (R10) bound one machine; a shell and 8 machines
whose pictures each cover the screen could ask 9 times as much, and the
shell's panels were not counted at all. The system ink (`spec/machines.md`
§machines.ink) gives the machines the shell shows one budget together,
what one machine shown over the whole screen may count: 640 × 480 +
230,400 + 153,600 = 691,200 square units. Pictures count their area
first (the terminal, the page and the backgrounds a host draws there
cost in proportion to it, about as much per unit as ink); then the
lists, each counting its ink times its picture's scale. The scale, not
its square: measured, a list drawn in a tile a third of the screen wide
costs about a third of what it costs over the whole screen for lines,
points and outlines (the smoothed edge does not shrink), a ninth only
for filled shapes. The shell is never counted: its screen is the one the
user needs to regain control, and stays within its own caps.

Measured 2026-11-21 on the reference machine (release build, software
renderer at 880 × 660, load average about 3): the median of 3 runs of 30
frames, each scenario a shell with every device at its worst (as the W
row "all") and 8 machines each with every device at its worst.
`TERSE_SYSTEM_INK=off` in terse-host's environment draws every machine
within its own caps only, as before (for measuring; not conforming).

| Scenario | 1 thread, before / after | 12 threads | GPU |
|---|---|---|---|
| One machine, every device at its worst (W "all") | 16.4 ms | 6.5 ms | 1.8 ms |
| 8 machines in a 3 × 3 grid of tiles | 47.6 / 40.8 ms | 21.6 / 20.1 ms | 14.8 / 15.2 ms |
| 8 machines each over the whole screen | 137.2 / 31.8 ms | 38.1 / 10.6 ms | 15.9 / 6.6 ms |
| 8 machines as panels of the whole screen | 187.9 / 51.3 ms | 59.1 / 18.2 ms | 17.2 / 6.0 ms |

What the machines add to the shell's own frame is now at most about one
and a half times one machine's worst: 24 ms in tiles, 15 ms overlapping,
15 ms as panels (over 36 ms for the same shell showing static panels).
Two costs are not in proportion to a picture's area and remain:

- *Text in small pictures.* A page of the smallest text scrolled every
  frame costs about 1.7 ms in a tile a ninth of the screen, half of what
  it costs over the whole screen: the glyphs in view are as many at any
  size. Eight such tiles cost 14 ms whatever the system ink says. The
  bound for them is the number of machines, 8.
- *The shell's scene.* A shell at its own caps with Screens in effect
  (its 3D list drawn over the panels, the scene put together again every
  frame) takes the software renderer 36 ms on one thread, twice a single
  machine's worst: R10's calibration counted the shell's Screens as
  nothing. This is the shell's own cost, not the machines'; the software
  renderer's scene is the place to make it cheaper.

The bundled shell's layouts with the bundled programs (six programs, and
fetch, three sparks and four holo; master, grid, the gallery with a
machine faced and back, zoom) count at most 416,744 of the 691,200 (the
gallery with a machine faced, the panels behind it counted too); master
and grid at most 168,401; no picture is a plate and no list is cut
(`the_layouts_stay_within_the_system_ink`).

## 6. Accountability: who was slow, and what then (R13–R16)

The budget, the ink caps (R10), the system ink (R12) and the fonts loaded
at start (R11) bound what a *program* can ask: on a host that meets the
calibration, no program can make a frame late. What remains is the host's
own components: its device drivers, its renderer, its core. A component
that is slow on many people's computers (one that scans fonts, leaks
memory, waits on a disk) should not be distributed; these requirements
make its slowness visible, attributable and not tolerated. Programs need
nothing new: they are bounded by construction.

**R13, identities.** A host and every device implementation state a
name, a version and a **build identifier**: a short tag naming a hash, a
colon and the first 8 bytes of that hash of the program's own file in hex
(`b3:` BLAKE3 for the reference; `fnv:` FNV-1a 64 for the C drivers,
`b2:` BLAKE2b for the Python driver, `s2:` SHA-256 for the Go reader),
`unknown` if the file cannot be read. The hash of the file, not of the
source revision, so that one build is told from another however it was
made, with nothing baked in at build time. It is self-reported: for
accountability among honest implementations, not a security measure (a
driver can say anything). Drivers give it in the HELLO reply (`DEVICES.md`
1.1, a trailing optional section: an older driver's reply simply ends
before it, and older cores read past it); a host with built-in devices
reports its own identity for each; `terse-host`, `terse-bus`, every
driver, `tcore`, `tersereader` and `terse` print it with `--version`. No
call of any device returns it: it is the host's, not machine state, and
a program cannot fingerprint the host by it. Measured: hashing takes
about 1 ms per MB of program (4 ms for the terminal driver, 12 ms for
terse-bus, 20 ms for terse-host); a driver does it before its HELLO reply,
a host in the record's writer thread, never on the frame loop.

**R14, attribution.** For each frame the host attributes the frame's
time (from its start to its picture presented) to its components
(`RECORD.md` 2): `core`, `compose`, `present`, `outside` and each device
driver. A component is **late** in a frame only if (`RECORD.md` 4):

1. the frame overran its deadline, one frame;
2. it would still have overrun had nobody waited for a CPU (the frame's
   time less every component's and the frame loop's starvation);
3. the system was not stalled on memory for a quarter of a frame;
4. the component's **own time** (its time less its starvation: on a CPU
   or blocked) is over its **allowance**, half a frame (8.3 ms), and the
   most of any component's.

Blocking on a disk, a lock or a sleep inside the frame loop is the
component's fault (R8, R11 forbid it); being descheduled is not. The
allowance is a share of the frame, not milliseconds of the reference,
so that it is the same on every computer; what depends on the hardware
is whether a legal frame fits in it, which R16 checks at start. Half a
frame because the reference's worst legal frame of one device is about 7
ms on one thread (§4): a device within its caps on a computer that meets
the calibration stays within it, and two components cannot both be the
one over it.

- *The bus* (`terse-bus`) times each driver objectively: it hands the
  driver its work and waits for the reply, so the time it waited for
  each driver is that driver's, request by request (BEGIN, calls, END's
  work as the next request finds it, PRESENT, GET, the state it is asked
  for); starting a machine (OPEN and a machine's first two frames), host
  services and the snapshots the rule takes are recorded but never
  blamed. Each driver process's CPU time and its threads' run-queue wait
  come from the operating system where it says: Linux
  (`clock_getcpuclockid`, schedstat, and PSI for the memory stall);
  macOS (`proc_pid_rusage`: CPU time and run-queue wait, no memory
  stall; the frame loop's own wait is its process's); Windows
  (`GetProcessTimes`: CPU time only). What a host could read is in the
  record (`readings`, `RECORD.md` 2), and what it may conclude without a
  reading is `RECORD.md` 4's.
- *A host with built-in devices* (`terse-host`) can only time its own
  stages: the emulation (the devices' calls within it), taking the
  screens and drawing, presenting, and how late a frame began after its
  tick; starvation is its frame loop thread's. This is less independent:
  a slow device is blamed as `core` or `compose`, not by name, and one
  stage that slows another (a lock, the caches) is not told apart. Every
  component is the host's own: it records and shows (R15).
- *Memory* is sampled every 600 frames (10 s) per process, so a leak
  shows as a trend (`terse perf log`); *start-up* is the time from the
  process's start to its first frame presented, with whether it was the
  first run of the build or since the computer started.

The cost of watching, measured on the reference (release, the shell
headless, 600 frames, 5 drivers): 11 µs at the start and 11 µs at the end
of each frame (p50; p99 40 µs), 0.13 % of a frame; and the waits' clock
readings add 0.012 ms to the emulation of a frame (0.019 against 0.007
ms p50). The record's writer is a thread of its own.

**R15, the rule.** In order:

1. **Shed optional work first.** A host sheds what it draws beyond what
   a device requires (the reference: the phosphor look, §4 "The look";
   a driver with a look of its own is told the level, `core.shed`,
   `DEVICES.md` 7.1).
2. **Warn.** After 3 late frames of one component within 60 frames (one
   second), the host tells the user, naming the device and its identity,
   and saying what happens next; the reference shows, as its own drawing
   over the bottom rows (like the consent prompt: no program can draw,
   cover or answer it; it takes no key and goes after 5 seconds), "device
   7 (terse-dev-page 0.1.0 b3:…) is slow: your work is saved; if it goes
   on, the machines that use it are stopped". At the warning the host
   **saves at once** the snapshot of each machine that uses the device
   (below), while the device still answers. A component is warned about
   once until it has gone 300 frames without a late frame.
3. **Stop.** After 10 late frames of one component within 300 frames
   (five seconds), the host takes a snapshot of each machine that uses
   that device, then stops those machines: they are **halted**, a
   stopped state of their own (`SPEC.md` §stopping), recorded in their
   state like an exit or a fault, with the device's slot. A shell sees it
   through the machine changed event and `info` (`spec/machines.md`,
   state 7); a machine alone ends the run with exit status 4 and the
   message "stopped by the host: device n did not keep up" (`STATE.md`
   6.4). Stopping the machine, not removing the device, keeps replay
   exact: the stop is an input, recorded in a recording at the frame
   after which it happened (`DEVICES.md` 8), and a replay stops there.
   A device serves every machine of a system, so a shell that uses it is
   stopped too: the system stops, its machines saved first. **A host
   stops only for certain blame**: late frames in which the operating
   system said how long the driver's threads and the frame loop waited
   for a CPU (condition 2 of R14 could be checked). Where it cannot say
   (Windows; Linux without schedstats; a driver of another user), a
   device is blamed by its waits alone, which on a busy computer may be
   the computer's doing: the host warns and saves (step 2) as usual,
   records every late frame, and never stops a machine for it
   (`RECORD.md` 4).
4. **The host's own components** (the core, composing, presenting): a
   host cannot stop itself usefully. It records every late frame, sheds
   (step 1) and shows the warning **once**, as for a device: again only
   after the component has gone 300 frames without a late frame (the
   rule never asks for a stop of one). The reference prints, besides,
   how many late frames each had when the run ends. A drawing that cannot
   keep up at the window's size is answered by shedding, not by
   announcing it frame after frame.

**Resizing.** When the window's surface changes size (the user or a
tiling compositor resizes it, a move to another monitor, the first
frame), every driver makes new layer files and buffers and draws whole
pictures, and the host composes and copies the whole window: that work
is the host's request, not a component's misbehaviour. Two measures, in
order:

- **Hold while it changes.** While the size keeps changing, a host holds
  its machines: no frame runs, nothing is drawn or presented by a driver,
  and the window shows the last picture scaled to the new size. A frame
  is held when the surface changed size within the last 6 frame periods
  (0.1 s: it keeps still that long before anything is drawn at it), for
  at most 60 periods in a row, and not again until 60 frames have run
  since. Holding is exact: a frame not run is like a frame run late
  (virtual time lags, SPEC §frames); the input waiting is given to the
  first frame run. So a drag costs one reallocation at its end, not one
  per frame, and nobody is measured while nothing runs.
- **Excuse the frames just after.** So a frame that
would be blamed (R14) and is the first composed at a new size or one of
the two after it is recorded with the verdict `resize` (`RECORD.md` 2)
and counts toward neither the warning nor the stop. Only a change of
the size the host composes counts (a configure that leaves it as it was
excuses nothing), and the excuse is bounded: at most **120 frames within
any 300** (two seconds of resizing in five). Three frames because the
first is the reallocation and the next two the buffers a GPU or the
system touch lazily and the window's double buffer at the new size (as
a machine's first two frames are not counted). The bound because a
window resized without end (a long drag, a compositor animating its
tiles) changes size in every frame: without one, every frame would be
excused. With it, a device late in every frame is warned about at most
123 frames run and stopped at most 130 frames run after a resize began,
however long the resizing goes on (rather than at its 3rd and 10th);
with the holds in between, within about 7 s of resizing without end
(measured: `terse-bus --selftest --resize`, the slow example driver,
warned at frame 146 and stopped at 154). A device late only while
reallocating is not warned about for ordinary resizing. A driver cannot
cause a resize, so it cannot use either measure to hide.

Pausing alone was the other way: it cannot be unbounded (a compositor
that resizes without end would stop the machines for ever), and the
frames after it still reallocate; excusing alone lets every frame of a
drag reallocate and be excused, which a slow device shares in. Together
the work is done once and the excuse covers only that once.

Why these numbers: one or two late frames are a hiccup a person barely
sees, and the frames in which a machine starts may be slow for reasons
that are not misbehaviour (they are not counted, R14, but caches warm up
in the frames after). Three within a second is a stutter anyone sees: tell
the user while the device still works and save their work. Ten within
five seconds (one late frame every half second, sustained) is a device
that does not keep up: if a slow device were tolerated, every frame of
every program using it would be at its mercy, contradicting the one
promise of R6–R12. A late frame every 40 frames is never warned about,
one every 30 frames sustained is stopped (`cargo test -p terse-perflog`).

**The snapshot before a stop.** The host asks the drivers for the
machine's state with a time limit of one second (the slow device must
serialize its own state, and it still answers, slowly); one that does
not answer in time has failed (`DEVICES.md` 10) and the run stops. The
snapshot goes, with a shell, into the machine's **automatic slot** (slot
15 + its handle: slots 16–23 are the host's, written only by it, read,
restored and deleted by the shell like its own, so no snapshot of the
user's is ever overwritten); without one (a machine alone) into a file
in the record's directory, `stopped-<image>.tsnp`, whose path the host
prints. Since the warning already saved one, the user loses at most what
the machine did between the warning and the stop (at most 300 frames,
five seconds) if the stop's snapshot cannot be taken; otherwise nothing.
A periodic snapshot was the other way: about 1.4 MB copied per machine
each time, for every run, to cover a case the warning's snapshot already
covers.

**R16, the hardware check.** A host draws each device's worst legal frame
(the generators behind the W rows, `terse_core::worst`) with the renderer
it will use, when it first starts on a computer, and keeps the result
(the reference: `$XDG_CACHE_HOME/terse/hwcheck-<size>-<mtime>.txt`, keyed
by the program file and checked against the computer's `env` line), so
it runs once per build and computer, not at every start. Where the look's
worst frame takes more than its allowance (half a frame), the look is
not offered (shed to plain lines from the start); where a device's worst
frame takes more than a frame, the computer cannot keep up with it. A host
**should** then not provide that device (an image requiring it is refused
cleanly, an optional use is simply absent); the reference reports it
(`slow`, in the record and on standard error) and still provides it, a
decision left to the owner. With the check, a late frame at run time
means misbehaviour, not weak hardware. Recommended, not required: a host
that skips it is judged by the same rule. Measured on the reference
(release, software renderer, 880 × 660): 58 ms for the five frames on 12
threads (138 ms on one thread, where the 3D points in the phosphor style,
12.3 ms, are over the look's allowance: the look would not be offered);
every device's plain worst frame is within 5 ms. `terse-host
--hardware-check` runs and prints it.

**Reports.** A user may publish a summary of their record and of a
`terse perf` run as a performance report (`terse perf report`, never
automatically), and anyone may aggregate the published reports with
`programs/reports` or `terse perf aggregate`: a verdict per host, device
and `terse perf` build in a stated sample, "NON-CONFORMANT IN SAMPLE"
when at least 1 in 3 of at least 5 reporters had it halted (R15) or a
requirement or ceiling failed. `RECORD.md` section 5 is the format, the
publishing and the rule.

## 7. Not yet specified

- Presenting: the time from the end of a frame to its picture on the
  screen, and the frame time budget for drawing. The reference host's
  drawing times are in README "Software renderer" as targets.
- Input-to-photon latency, which depends on the display.
- Times over real networks (round trips are specified, milliseconds
  are not).
