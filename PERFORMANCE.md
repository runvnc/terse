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
  §vector.ink-cap and `spec/vector3d.md` §vector3d.caps.

**Requirements and targets.** A requirement is stated in units that any
implementation can be tested against on any machine and network: frames
(at 60 a second), round trips (messages a relay receives), and what may
not wait on what ("asks no relay", "does not wait for a relay"). It
passes or fails. A target is in milliseconds on the reference machine
below; it is information for implementers and a guard against
regressions of the reference, never a pass or fail for another
implementation.

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
| Machine host | R12 | The host work a whole system can demand is bounded: the machines a shell shows share the system ink, 691,200 square units of the shell's screen (their pictures' areas first, then their lists' ink times their scale; a picture that does not fit is a plate, a list is cut where the ink runs out), the shell's own screen drawn within its own caps (`spec/machines.md` §machines.ink) | square screen units | `terse perf` W rows `sys-*`: a shell and 8 machines, each with every device at its worst, tiled, overlapping and as panels, with the system ink and without (target: the machines add no more than about one machine's worst); `cargo test -p terse-core --test system_ink` (plates, scales, where lists are cut); `cargo test -p terse-cli --test shell` (`the_layouts_stay_within_the_system_ink`: the bundled layouts are never cut) |

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

## 2. Running the tests

**On the reference** (a release build, the Go reader built if you want
its rows):

    cargo build --release -p terse-cli -p terse-host
    (cd native/reader-go && go build -o tersereader .)
    target/release/terse perf

`terse perf` runs every scenario headless, in temporary directories,
with relays it starts on ephemeral loopback ports (never 7878, never your
config, cache or data directories), and prints one line per scenario:
its id, what it measures, the measurement, the requirement (R1–R12, §1)
or target (T), and PASS, FAIL, `info` (a target) or `skipped` (terse-host or the Go
reader not found). It exits with 1 if a requirement failed. `--only
machine|client|host` runs one group, `--runs n` changes the 5 runs of the
timed scenarios (the rendering ones run 3 times). The machine scenarios run the content service and the
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

## 3. Targets on the reference machine

Measured with `terse perf --runs 5` on the machine above, release build,
2026-11-21, load average 12 (another build was running; rows H1, H2 and H4 to H6
measured again after the fonts were built in, with --runs 3, load average 3 to 4): the median,
with the lowest and highest run where there were several. Every
requirement passed.

| Id | Scenario | Measured | Target |
|---|---|---|---|
| M1a–c | held get (fetch's message): no relay / a connected relay / a relay that never answers | 1 frame each; the service answers in 0.1 ms / 7 ms / 0.07 ms; no relay asked | — (R6: 2 frames) |
| M2 | get of a blob on a connected local relay | 1 Get; 6.2 ms; fetch's get done in 1 frame | — |
| M5 | shell start to the launcher on screen | 4 frames (the shell's own) | — |
| M6 | Enter on fetch in the launcher to fetch's first frame | 1 frame | — (R9: 2 frames) |
| M7 | Enter on fetch to its message on the screen | 2 frames (33 ms) | ≤ 6 frames |
| M8, M9 | reader: M to the manual's index; Enter on a link to the article | 1 frame each | ≤ 6 frames |
| M10 | conform-perf in the real-time loop | every line PASS: 1 frame, 1 frame, 60 60 60 60 frames a second | — |
| C1 | `terse feed sync`: 50 entries and their articles | 59 ms; 1 Subscribe, 50 Get | ≤ 500 ms |
| C2, C3 | `terse get` of a held article (the whole process); relay connected / never answering | 7.1 ms (6.9–8.0) / 6.6 ms | ≤ 50 ms |
| C4 | `terse feed list`, relay never answering | 5.4 ms | ≤ 50 ms |
| C5 | `terse get` of an article on a local relay | 7.9 ms (7.9–10.0); 1 Get | ≤ 50 ms |
| G1 | `tersereader sync`: 50 entries and their articles | 303 ms; 1 Subscribe, 50 Get | ≤ 500 ms |
| G2, G3 | `tersereader --dump` of a held article; relay connected / never answering | 12.2 ms (11.9–18.1) / 12.6 ms | ≤ 50 ms |
| G4 | `tersereader list`, relay never answering | 12.3 ms | ≤ 50 ms |
| G5 | the same through the Go node on the local socket: `--dump`, `list` | 1.9 ms (1.8–2.3), 2.6 ms | ≤ 50 ms |
| H1 | `terse-host` start to hello's first frame drawn (software, offscreen) | 22 ms (20–28; before the built-in fonts: 202 ms, 150–513) | ≤ 500 ms |
| H2 | fetch's slowest frame (its first Greek, Cyrillic and symbols, 50 ms after start) | 1.0 ms (before, its CJK and Hangul line: 102 ms) | ≤ 50 ms |
| H4 | the first frame showing 2,400 glyphs of the full repertoire never drawn before, 1 thread (`term-full`) | 9.2 ms (the first frame of basic text, 90 glyphs: 4.4 ms) | ≤ 16.7 ms |
| H5 | H1 with a 19 MB CJK collection listed as an extra font | 19 ms (19–21) | as H1 |
| H6 | 1,200 CJK glyphs never drawn before, each frame, from that font, 1 thread (`term-cjk`) | p50 22 ms, max 26 ms | ≤ 16.7 ms |
| H3 | programs/conform's stress frames 49–108 on the software renderer, 1 / 12 threads | p50 6.4 / 1.6 ms, max 8.7 / 2.3 ms (before the ink caps: p99 1,678 / 361 ms) | ≤ 16.7 ms |
| W | each device's worst legal frame, and all of them at once (§4) | every cap held (PASS); 1 thread at most 6.5 ms a device, 15.8 ms all at once | ≤ 16.7 ms |

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

## 6. Not yet specified

- Presenting: the time from the end of a frame to its picture on the
  screen, and the frame time budget for drawing. The reference host's
  drawing times are in README "Software renderer" as targets.
- Input-to-photon latency, which depends on the display.
- Times over real networks (round trips are specified, milliseconds
  are not).
- Start-up of the host itself (the reference's is a target, H1).
