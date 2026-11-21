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
  §content.submit.

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
thousand font faces installed (which matters for text in scripts the
monospace font lacks, §3). Times are the median of 5 runs, with the
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

What the machine already guarantees needs no row: a frame's work for a
program is bounded by the budget (`SPEC.md` §frames, §charges), so no
program can make a frame slow on a host that keeps up with a frame of
instructions.

## 2. Running the tests

**On the reference** (a release build, the Go reader built if you want
its rows):

    cargo build --release -p terse-cli -p terse-host
    (cd native/reader-go && go build -o tersereader .)
    target/release/terse perf

`terse perf` runs every scenario headless, in temporary directories,
with relays it starts on ephemeral loopback ports (never 7878, never your
config, cache or data directories), and prints one line per scenario:
its id, what it measures, the measurement, the requirement (R1–R9, §1)
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
2026-11-21, load average 12 (another build was running): the median,
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
| H1 | `terse-host` start to hello's first frame drawn (software, offscreen) | 173 ms (168–1,556: the slow run read the fonts from disk) | ≤ 500 ms |
| H2 | fetch's slowest frame (its CJK and Hangul line, 50 ms after start) | 93 ms (91–104) | ≤ 150 ms |
| H3 | programs/conform's frames 60–660 on the software renderer, 1 / 12 threads | p99 1,678 / 361 ms (p50 0.01 ms) | none: a deliberate worst case |

The machine rows' milliseconds are those of the content service; the
frames are what a program sees. Drawing times of the renderers for the
usual scenes are in README "Software renderer".

**First text in another script.** A frame that first shows characters
the monospace font lacks (fetch's Japanese, Chinese and Korean line)
took 0.95 to 1.7 s on this machine, and the host grew from 35 MB to
360 MB: in that frame the host looked for a font with each such
character by reading the font files one by one (about 0.1 s), then had
fontdue parse the font it found, a 19 MB CJK collection, whole (0.7 s
and 300 MB). Now every font's character map is read once, on a thread
of its own as the host starts (about 0.1 s for 1,000 faces), and a
fallback font's glyphs are drawn one at a time with swash as they are
needed: that frame takes 90 ms when fetch runs 50 ms after start (most
of it waiting for the map), and fetch uses 71 MB.

**Requests with a relay connected.** The content service polls each
connected relay's socket with a 5 ms read timeout, so a request that
arrives while it is polling waits up to 5 ms to be started (M1b: 7 ms
for a held blob, still within the frame). A poll used to wait 5 ms more
after the last message it received, which added 5 ms to every reply
(M2: 12 ms before, 6 ms now).

## 4. Not yet specified

- Presenting: the time from the end of a frame to its picture on the
  screen, and the frame time budget for drawing. The reference host's
  drawing times are in README "Software renderer" as targets.
- Input-to-photon latency, which depends on the display.
- Times over real networks (round trips are specified, milliseconds
  are not).
- Start-up of the host itself (the reference's is a target, H1).
