# Terse

A small virtual computer: a 32-bit RISC-V CPU (RV32IMC) with a fixed budget
of 125,000 units of work per frame, up to 128 KiB of ROM and 1 MiB of RAM,
and host devices reached through system calls (`ecall`): a text terminal, a
2D vector display, and a content device that fetches and publishes blobs and
reads feeds. `SPEC.md` is the authoritative design, `STATE.md` defines the
machine state hash, and `NETWORK.md` describes how hosts move blobs and feeds
through a relay; this repository is the first prototype of all three.

Shell commands below are written for fish; they also work in bash and zsh
unless they set variables (`set X value` in fish is `X=value` in bash).

## Build, run, test

Requires a Rust toolchain (edition 2024). If `cargo` is not on your `PATH`
(rustup puts it in `~/.cargo/bin`), add it once:

    fish_add_path ~/.cargo/bin

Then:

    cargo build --workspace
    cargo test --workspace

    cargo run -p terse-cli -- info programs/hello/hello.terse
    cargo run -p terse-cli -- run programs/hello/hello.terse --frames 30 --dump
    cargo run -p terse-host -- programs/hello/hello.terse
    cargo run -p terse-host -- programs/hello/hello.terse --screenshot hello.png --frames 30

Building the programs needs clang with the riscv32 target, ld.lld and
llvm-objcopy (see [Writing programs](#writing-programs)); the tests that need
them are skipped, with a message, when they are missing. The checked-in
images were built with clang and LLD 23.1.4.

`terse` commands:

- `run <image|id> [--frames N] [--dump [--dump-vectors]] [--events <file>] [--relay ws://...] [--store <dir>] [--config <dir>] [--hash] [--hash-every N] [--bench]`
  runs headless (input only from `--events`, see [Scripted input](#scripted-input);
  Unix time 0, no random bytes (so `random` gives zeros), controllers only from
  `pad` lines) for N frames or until the program stops (capped at 36,000
  frames), prints the 80x30 grid with `--dump` (a wide character's continuation
  cell is skipped). `--dump-vectors` adds the display list device 3 holds, after
  a `-- vectors: N list bytes --` line, one shape per line as the host draws it
  (see [Vector display](#vector-display)); when the style or fill mode is not
  the default a `-- vector style: … --` line comes first.
  Content requests (device 4) are carried out between frames and always complete at
  the start of the next frame, so runs are reproducible. For an image with device 4
  the feeds (SPEC 9.5) are those subscribed in `--config` (see [Feeds](#feeds)):
  with `--relay`, `run` first syncs them until caught up (NETWORK 4.3; a warning and
  the local entries if that fails within 60 s), then keeps following them while the
  machine runs; an entry stored during frame N is announced by a feeds-updated event
  in frame N + 1.
  `--hash` prints the machine's state hash ([STATE.md](STATE.md)) after the last
  frame as 64 hex digits; `--hash-every N` prints `<frame> <hash>` after every frame
  whose frame counter is a multiple of N (after frames N, 2N, …). `--bench` prints,
  on stderr, the emulation time per frame (p50, p99, max; the machine and the
  headless content service) and how many times faster than real time the run was.
- **Exit status of `run`.** When the program exits, `terse run` exits with its
  exit code (0–255). When the machine stops with a fault (SPEC 1.5), it prints
  `terse: fault: <kind> at pc <pc>, address <addr>` on stderr and ends itself
  with signal SIGTERM. Every status from 0 to 255 is a possible exit code, so
  this is as distinct as a process can be: fish reports `terminated by signal
  SIGTERM`, a parent that waits for the process sees the signal, and only a
  shell's numeric status (143 in fish and bash) could be mistaken for a program
  that exits with 143.
- `info <image|id>` prints the header fields and the image ID (BLAKE3-256).
- `put <file>` stores a blob (and sends it to `--relay`) and prints its ID;
  `get <id> [-o <file>]` fetches one; `hash <file>` prints a file's blob ID.
- `pack --rom <raw.bin> --title <t> --ram-pages <n> [--device <slot:version>]... -o <out.terse>`
  builds an image from a raw ROM (the bytes loaded at `0001_0000h`).
- `key new|show` and `feed …`: feeds, see [Feeds](#feeds).

An `<id>` is 64 hex digits, looked up in the blob store and then on the relay;
the bytes are verified against the ID. `--store` defaults to
`$XDG_CACHE_HOME/terse/blobs` (else `~/.cache/terse/blobs`). Only `ws://` relays
are supported. The arrival order of entries (the timeline, SPEC 9.5) is kept in
`<store>/feeds/arrivals`.

`terse-host <image|id> [--no-trails] [--force-phosphor] [--relay ws://...] [--store <dir>] [--config <dir>]`
opens a window and runs the image at 60 frames/s of virtual time. The screen
(SPEC 7) is shown at 4:3, so screen units are square: a cell is exactly 1:2 pixels
(11 × 22 at the default font size; the glyphs are rasterised at that size, so
text is crisp at the native size and at whole multiples of it), and the grid is
scaled uniformly, centred and letterboxed in a window of any other shape. Input:
keyboard and text; the pointer in screen units (window pixels map to 640x480
through the same rectangle, clamped at the edges, movement at most once a frame);
the wheel (line and pixel deltas, one step per terminal row of travel); game
controllers through [gilrs](https://crates.io/crates/gilrs), numbered in order of
connection. Content requests run on a background thread and complete at the
first frame after they finish; with `--relay` the subscribed feeds sync in the
background and new entries are announced with a feeds-updated event. Random
bytes come from the operating system; only the bytes a frame took are replaced
before the next frame. The window closes when the program exits; after a fault
it stays open, shows the fault on the bottom row (white on red) and in the title,
and the process then ends by SIGTERM like `terse run`. `--no-trails` shows the
phosphor style without its afterglow; `--force-phosphor` shows any program in
the phosphor style (for trying it out; see [Phosphor style](#phosphor-style)).
`--screenshot <out.png> --frames N [--events <file>] [--size <W>x<H>]` runs
headless exactly like `terse run` and writes a PNG, of the grid's native size or
`W` × `H` pixels with the screen letterboxed in it; in the phosphor style the
afterglow is built up over the last 600 frames, as in the window.
`--selftest <image|id> [--frames N] [--events <file>] [--csv <file>]` runs N frames
(default 200) in the window with vsync, with only the scripted input (default: the
image's file name with `.events`, if it exists; keyboard, mouse and gamepads are
ignored), and prints timings: see [Conformance image](#conformance-image).

`terse-relay [--listen 127.0.0.1:7878] --store <dir> [--max-store-bytes N] [--feed-entries N]`
serves a blob store and feeds over WebSocket (NETWORK.md sections 3 and 4.3), one
thread per connection. It keeps the newest `--feed-entries` entries of each feed
(default 1024, at least 256) in `<dir>/feeds`, and listens on loopback unless told
otherwise; `--listen 127.0.0.1:0` picks a free port and prints it.

## Trying the relay locally

    cargo build --workspace
    mkdir -p /tmp/terse-try
    target/debug/terse-relay --listen 127.0.0.1:0 --store /tmp/terse-try/relay &
    # it prints "listening on ws://127.0.0.1:PORT"; use that URL below
    set U ws://127.0.0.1:PORT
    target/debug/terse put programs/fetch/message.txt --relay $U --store /tmp/terse-try/a
    target/debug/terse put programs/fetch/fetch.terse --relay $U --store /tmp/terse-try/a
    # prints fetch.terse's image ID; run it from an empty store:
    target/debug/terse run <id> --relay $U --store /tmp/terse-try/b --frames 10 --dump
    target/debug/terse-host <id> --relay $U --store /tmp/terse-try/b

## Feeds

A feed is a numbered series of signed entries from one Ed25519 key, each pointing
to an article (a blob of UTF-8 text); see NETWORK.md section 4. All `key` and
`feed` commands take `--relay`, `--store` and `--config`:

- `--config <dir>` holds the secret key (`identity.key`, mode 0600, never printed)
  and the subscription list (`subscriptions`). Default `$XDG_CONFIG_HOME/terse`,
  else `~/.config/terse`.
- Entries are kept with the blobs, in `<store>/feeds/<feed key>/<seq>`.

Commands:

- `key new` creates your identity (refuses to replace one) and prints its feed key;
  `key show` prints it again.
- `feed publish <file> --title <t> [--reply-to <blob id>]` puts the article and
  publishes the next entry of your feed; prints `seq N blob <id>`.
- `feed subscribe <key>`, `feed unsubscribe <key>`, `feed subscriptions`.
- `feed sync [--watch] [--auto-download <bytes>]` connects to `--relay`, receives
  new entries of subscribed feeds, verifies and stores them, and downloads
  articles up to 16,384 bytes (or `--auto-download`). One line per new entry:
  time (UTC, as claimed by the author), feed key prefix, seq, size, article
  state, title. The state is `local` (in the store), `remote` (over the limit;
  `feed read` fetches it), `failed` or `size-mismatch` (the blob is not the size
  the entry claims, so it is not used). Without `--watch` it exits once caught
  up: each feed's subscribe reply gives the relay's highest `seq`, and the feed is
  caught up once that entry has arrived (NETWORK 4.3); then downloads done. It fails
  after 60 s.
  With `--watch` it stays connected and prints entries as the relay pushes them,
  reconnecting (1 s backoff, doubling to 30 s) if the connection drops.
- `feed list [<key>]` lists entries held locally, newest first.
- `feed read <key> <seq>` prints the article, fetching it from `--relay` if it
  is not in the store. Control characters are replaced on output.

Two users on one machine (each with their own store and config), with a relay
running as in [Trying the relay locally](#trying-the-relay-locally):

    # terminal 1: the author
    set T target/debug/terse
    set A --relay $U --store /tmp/terse-try/author --config /tmp/terse-try/author-config
    $T key new $A                                  # prints the feed key
    echo "Hello, feeds." > /tmp/terse-try/hello.txt
    $T feed publish /tmp/terse-try/hello.txt --title "Hello" $A

    # terminal 2: the reader
    set T target/debug/terse
    set R --relay $U --store /tmp/terse-try/reader --config /tmp/terse-try/reader-config
    $T feed subscribe <feed key> $R
    $T feed sync $R                                 # prints the entry; article downloaded
    $T feed list $R
    $T feed read <feed key> 1 $R
    $T feed sync --watch $R                         # leave running

    # terminal 1 again: appears in terminal 2 at once
    $T feed publish /tmp/terse-try/hello.txt --title "Again" $A

## Layout

    crates/terse-core/   the machine: cpu.rs (the RV32IMC interpreter), machine.rs (memory,
                         frame loop, budget, calls and charges, state serialization), image
                         parsing, and the devices: system.rs, terminal.rs, vector.rs (device
                         3 and the one display-list decoder, with the fill cap), content.rs.
                         wide.rs holds the Unicode 16.0 East Asian Width W/F table. No GUI,
                         threads, clock, I/O or OS randomness; inputs (events, time, random
                         bytes, controllers, content completions) are passed in.
    crates/terse-net/    host side of device 4 and feeds: blob store, identities, entries,
                         feed store, relay protocol, relay client (behind a small
                         Transport trait), content service thread
    crates/terse-relay/  `terse-relay`: the WebSocket blob and feed relay
    crates/terse-cli/    `terse`: headless runner, image info/pack, blobs, feeds; the
                         program tests (tests/hello.rs, fetch.rs, reader.rs, sparks.rs,
                         conform.rs)
    crates/terse-host/   `terse-host`: winit + wgpu window; renderer.rs is the
                         window-independent renderer (terminal, vectors.rs's shapes, the
                         phosphor tube: SPEC 10.5, 10.6)
    programs/lib/        what a program needs: crt0.S, terse.ld (the memory map), terse.h
                         and terse.c (the calls, a display-list builder, content requests)
    programs/hello/      demo program in assembly: hello.S, build.sh, the built hello.terse
                         and the golden screen used by the tests
    programs/fetch/      content device demo in C: gets message.txt by its ID and prints it;
                         build.sh writes message_id.h, then fetch.terse
    programs/reader/     the feed reader, in C: reader.c, build.sh, the built reader.terse
                         and the golden screens in expected/
    programs/sparks/     a game for the vector display, in C: sparks.c, build.sh, the
                         built sparks.terse, recorded sessions in sessions/ and the golden
                         screens in expected/
    programs/conform/    the conformance and responsiveness image (C and assembly),
                         build.sh, conform.terse, its input conform.events, and
                         expected-hashes.txt and expected-screen.txt

## Writing programs

A program is RV32IMC machine code in ROM from `0001_0000h`, with RAM from
`0010_0000h` (SPEC 1.2). It starts at the first ROM byte with `sp` at the end of
RAM and all RAM zero, and reaches the devices with `ecall` (SPEC 3): call number
in `a7`, arguments in `a0`–`a5`, result in `a0` (and `a1`). There are no
interrupts: a program does a frame's work and calls `wait_frame`; if it runs out
of budget it is simply paused until the next frame.

Programs are built with clang's riscv32 target, ld.lld and llvm-objcopy, then
packed with `terse pack`. For C, `programs/lib` provides:

- `crt0.S`: the entry point; copies `.data` from ROM to RAM (`.bss` is already
  zero), calls `main`, and exits with its return value.
- `terse.ld`: code and constants in ROM, data and the stack in RAM.
- `terse.h` / `terse.c`: every call as a C function (`terse_wait_frame`,
  `terse_event_pop`, `terse_random`, `terse_pad_read`, `term_write`,
  `term_goto`, `term_color`, `term_cmd`, `term_cells_write`, …), terminal helpers
  (put a string, a code point as UTF-8, a run of ASCII, fill cells, the
  wide-character test), the vector display (a display-list builder, `vec_submit`,
  `vec_style`, `vec_persistence`, `vec_fill`), content requests (the request
  block, get, the feed operations), and `memcpy`, `memset`, `memmove`, `memcmp`.
  `term_write` repeats the call until every byte is written, since a write stops
  when the frame's budget runs out (SPEC 8.2).

The build (see `programs/reader/build.sh`, which takes `CLANG`, `LLD`, `OBJCOPY`,
`TERSE` and `CARGO` from the environment):

    set flags --target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin -ffunction-sections -fdata-sections
    clang $flags -I programs/lib -c programs/lib/crt0.S -o 0.o
    clang $flags -I programs/lib -c prog.c -o 1.o
    clang $flags -I programs/lib -c programs/lib/terse.c -o 2.o
    ld.lld -T programs/lib/terse.ld --gc-sections 0.o 1.o 2.o -o prog.elf
    llvm-objcopy -O binary prog.elf prog.bin
    target/debug/terse pack --rom prog.bin --title prog --ram-pages 4 --device 2:0 -o prog.terse

`crt0.S` must come first. There is no C library beyond `terse.c` and no
compiler runtime: avoid 64-bit division and floating point, or bring the routines
(`crates/terse-core/tests/softfloat/softfloat.c` is a tested IEEE double add,
subtract, multiply and divide in integer C). The build is reproducible: the same
sources and the same clang give the same image, and each program's test fails if
its checked-in `.terse` is not what its sources build to (and is skipped when
clang lacks the riscv32 target).

Costs to keep in mind (SPEC 11): an instruction from ROM costs 1 unit, one from
RAM 4; a call costs 5 (the `ecall` and 4) plus 1 per 4 bytes it copies; `write`
costs 1 per byte, and a scroll or clear 150. Writing a whole row with one `write`
or `term_cells_write` is far cheaper than a call per character.

## Scripted input

`terse run --events <file>` and `terse-host --screenshot … --events <file>` read
input events from a text file: one event per line, a frame number, a command and
its arguments. Blank lines and text after `#` are ignored (except in `text`).

    10 down <key> [shift] [ctrl] [alt] [meta]   key down
    10 up <key> [modifiers]                     key up
    10 press <key> [modifiers]                  key down and key up in the same frame
    10 text <rest of the line>                  one text event per character
    10 pointer <x> <y> [<buttons>]              pointer in screen units (0-639, 0-479); buttons 0-7
    10 click <x> <y>                            pointer with the primary button, then released
    10 wheel <vertical> [<horizontal>]          wheel steps; positive is down and right
    10 feeds-updated                            a feeds-updated event
    10 pad <n> <buttons> <lx> <ly> [<rx> <ry>]  controller n (0-3) from this frame on
    10 pad <n> off                              controller n disconnected

`<key>` is a USB HID usage ID (`40` or `0x28`) or a name: `a`-`z`, `0`-`9`,
`f1`-`f12`, `enter`, `esc`, `backspace`, `tab`, `space`, `up`, `down`, `left`,
`right`, `home`, `end`, `pageup`, `pagedown`, `insert`, `delete`. The events of a
frame are delivered in the order written. A cell (column, row) starts at screen
unit (column × 8, row × 16).

A `pad` line connects controller `n` and sets its whole state (SPEC 6) until the
next `pad` line for it; controllers are disconnected until their first one.
`<buttons>` is hexadecimal, byte 1 in bits 0-7 and byte 2 in bits 8-15 (`1` up,
`2` down, `4` left, `8` right, `10` A, `20` B, `2000` Start, `14` A and left); the
sticks are -127 to 127, x then y, and the right stick defaults to `0 0`.

## The reader

`programs/reader` is a feed reader written in C: the timeline of the subscribed
feeds, newest first, and articles word-wrapped to the grid. Keys: Up/Down,
PageUp/PageDown, Home/End and the wheel move; Enter or a click on the selected entry
opens it; Esc, Backspace or Left go back; Esc in the list quits. A row shows the age
of the entry (or its UTC date and time when the host gives no clock), the first 8
hex digits of the feed key, `✓` if the article is in the local store, `↳` for a
reply, and the title; `•` marks entries that arrived while it runs, until opened.
New entries appear without a key press.

It holds one timeline request's worth of entries (16 KiB of records: 70 to 147
entries) and requests another range as you scroll, and up to 48 KiB of an article
(read in 16 KiB gets; it says where it stops). Rebuild it with
`programs/reader/build.sh`.

### Walkthrough

Two authors and a reader on one machine, each with their own store and config:

    cargo build --release --workspace
    set T target/release/terse
    mkdir -p /tmp/terse-demo
    target/release/terse-relay --listen 127.0.0.1:0 --store /tmp/terse-demo/relay &
    # it prints "listening on ws://127.0.0.1:PORT"; use that URL below
    set U ws://127.0.0.1:PORT

    # Two authors.
    set A --relay $U --store /tmp/terse-demo/alice --config /tmp/terse-demo/alice-config
    set B --relay $U --store /tmp/terse-demo/bob --config /tmp/terse-demo/bob-config
    set KA ($T key new $A)
    set KB ($T key new $B)
    echo "Hello from Alice." > /tmp/terse-demo/a.txt
    $T feed publish /tmp/terse-demo/a.txt --title "Hello" $A
    echo "日本語の記事です。" > /tmp/terse-demo/b.txt
    $T feed publish /tmp/terse-demo/b.txt --title "日本語のタイトル" $B
    echo "Hello, Alice." > /tmp/terse-demo/c.txt
    $T feed publish /tmp/terse-demo/c.txt --title "Re: Hello" --reply-to ($T hash /tmp/terse-demo/a.txt) $B

    # The reader subscribes to both and runs the program.
    set R --relay $U --store /tmp/terse-demo/reader --config /tmp/terse-demo/reader-config
    $T feed subscribe $KA $R
    $T feed subscribe $KB $R
    target/release/terse-host programs/reader/reader.terse $R

    # Headless, with scripted keys: select the second entry and open it.
    printf '10 press down\n12 press enter\n' > /tmp/terse-demo/keys.txt
    $T run programs/reader/reader.terse $R --events /tmp/terse-demo/keys.txt --frames 30 --dump

    # Publish while terse-host runs: the entry appears at the top, marked •.
    $T feed publish /tmp/terse-demo/a.txt --title "Live" $A

Measured with the golden-test fixture (`cargo test --release -p terse-cli --test
reader timing -- --nocapture`): an already-local 6 KB article shows its first page 2
frames after Enter (the Enter frame and the next) and is fully wrapped after 4; the
busiest frame of that session uses 62,269 units (50% of 125,000), a typical frame
277. The reader writes each row as 80 cells with one `term_cells_write` and wraps at
most 2,048 bytes of article per frame, so no frame runs out of budget. ROM 10,292
bytes, 9 RAM pages.

## Vector display

Device 3 (SPEC 10) draws a display list of lines, circle outlines, discs,
rectangles and triangles. A program builds the list in RAM and submits it (1 unit
per 4 bytes); the device keeps its copy and the host draws it every frame until
the next submit or clear. Coordinates are sixteenths of a screen unit, so the
screen is 10240 × 7680. An image gets the device with `terse pack … --device 3:0`;
without it the calls return `FFFF_FFFFh` and nothing is drawn.

`terse_core::vector::decode` turns a list into resolved primitives (origin added,
colour and width tracked, the list's end rules and the fill cap applied);
`decode_outlines` gives what the phosphor style and fill mode 1 draw (a disc as a
circle, a rectangle as four lines, a triangle as three), and `Vector::shapes`
picks between them. `terse-host`, `terse run --dump-vectors` and the tests all
draw or print from these. **Fill cap** (SPEC 10.4): the filled shapes of one list
may cover at most four screens of area (each counted by its bounding box, clipped
to the screen); the fill that would go past that, and every fill after it, is not
drawn, while lines and circles go on. **Fill mode** (`vec_fill(VEC_FILL_OFF)`)
draws every filled shape as its outline in the clean style.

`terse-host` draws lines, outlines and discs as instanced quads shaded by distance
(round ends, antialiased), rectangles and triangles as plain triangles, blended in
list order, clipped to the screen, and re-uploads them only when the list changes.
The screen is the terminal grid's rectangle, which is 4:3, so a circle is round.
Layers in the clean style (SPEC 10.5): cell backgrounds, then shapes, then glyphs
and the cursor, so text is always readable over shapes.

From C (`programs/lib/terse.h`), the builder appends to a buffer you own:

    static uint8_t list[512];
    vec_begin(list);
    vec_colour(255, 200, 0, 255);           /* r, g, b, opacity */
    vec_width(32);                          /* two screen units */
    vec_move(160 * VEC_UNIT, 120 * VEC_UNIT);
    vec_line(480 * VEC_UNIT, 120 * VEC_UNIT);
    vec_disc(10 * VEC_UNIT);                /* around the current point */
    vec_submit_list(list);                  /* VEC_DONE or VEC_REJECTED */

`vec_tri`, `vec_rect`, `vec_circle`, `vec_origin`, `vec_end` and `vec_clear()` are
the rest.

### Phosphor style

`vec_style(VEC_STYLE_PHOSPHOR)` (SPEC 10.6) turns the whole screen into a vector
monitor tube: the background is black (cell backgrounds are not drawn), shapes
are outlines that add light instead of covering, and the terminal's text is light
on the same tube. What the program shows now is drawn at full brightness each
frame; an afterglow of earlier frames fades behind it, fast at first and then
with a dim tail multiplied by persistence / 256 each frame
(`vec_persistence(n)`, 160 after reset). The style, persistence and fill mode are
machine state (in the state hash, STATE.md 1.7); the afterglow is not, and the
program cannot see it.

How `terse-host` draws it (all constants in `crates/terse-host/src/phosphor.rs`):
the beam's bright core is 0.6 of the stated width with a halo of 0.12 of the core
that is gone 1.5 units past the edge; glyphs and the cursor are drawn as light with
a faint bloom. The afterglow is two floating-point textures, a fast component (×0.46
per frame, so about 10% after three frames) and a slow tail (× persistence / 256),
each fed with 0.2 of the beam — taken as a maximum, not added, so a shape that stays
put is never brighter than 1.2 times its beam — and each losing a small epsilon per
frame so trails really reach black. When a list keeps its sequence of opcodes from
one frame to the next, the afterglow is deposited at 4 to 16 positions interpolated
between the old and new coordinates, so a moving shape leaves a continuous smear,
not separate copies (not for jumps of more than 40 units); the live beam is drawn
only where the shape is now. The sum goes through a soft tone curve, and only light
above 1.5 spills into white. The afterglow advances once per machine frame, not per
presented frame, so it does not depend on the display's refresh rate. A ring moving
12 units a frame at persistence 160 leaves a trail whose peak, in sRGB, is 237 live,
then 82, 58, 39, 23, 9, 1 and 0 in the six frames after it vanishes.

`--no-trails` turns the afterglow off, for anyone who finds it distracting;
`--force-phosphor` shows any program this way. In the clean style none of this
runs.

Try it in Sparks (P switches style):

    cargo run --release -p terse-host -- programs/sparks/sparks.terse
    cargo run --release -p terse-host -- programs/sparks/sparks.terse --no-trails
    cargo run --release -p terse-host -- programs/sparks/sparks.terse --events programs/sparks/sessions/phosphor.txt --frames 900 --screenshot /tmp/phosphor.png
    cargo run --release -p terse-host -- programs/reader/reader.terse --force-phosphor

## Sparks

`programs/sparks` is a small game for the vector display, in C. Steer a small
craft around the arena: it turns, thrusts and drifts with inertia, and bounces
softly off the walls. Fly over the drifting sparks to collect them; the sweepers,
bars turning about fixed pivots, cost a life when they touch you (three lives, then
a moment of invulnerability after each hit). Every ten sparks another sweeper
appears, up to six. Until you start, a demo plays itself with six sweepers.

    cargo run --release -p terse-host -- programs/sparks/sparks.terse

- Keyboard: Left/Right turn, Up thrusts, Enter starts (and restarts after game
  over), Esc quits, P switches between the clean and the
  [phosphor](#phosphor-style) display style, F switches filled shapes on and off
  (outlines) in the clean style.
- Controller 0: d-pad left/right or the left stick turns, A, d-pad up or the stick
  pushed up thrusts, A or Start starts, Y switches the display style, X switches
  fill.

The HUD (row 0) and the title and game-over boxes are terminal text on coloured
cell backgrounds, so the layer order shows: shapes pass over the backgrounds and
under the text. In the clean style a near-opaque grey rectangle, last in the list,
hides the shapes behind the boxes. The program does all the geometry: a 256-entry
sine table and fixed-point multiplies rotate the sweepers and the craft, in
sixteenths so motion is smooth; the device only receives finished coordinates.
Randomness is a xorshift generator stirred with `random`, so a run is a function
of its recorded inputs.

Rebuild with `programs/sparks/build.sh`. The tests
(`crates/terse-cli/tests/sparks.rs`) replay recorded 3,600-frame sessions in
`programs/sparks/sessions/` (`keys.txt` with the keyboard, `pad.txt` with `pad`
lines) and `phosphor.txt` (the first 1,000 frames of `keys.txt` with P pressed
and controller Y pressed twice, which must play the same game), compare the
terminal and the decoded display list with `programs/sparks/expected/`
(`UPDATE_GOLDEN=1` rewrites them), and check a replay is identical. To watch a
recording headless:

    target/debug/terse run programs/sparks/sparks.terse --events programs/sparks/sessions/keys.txt --frames 900 --dump --dump-vectors
    target/debug/terse-host programs/sparks/sparks.terse --events programs/sparks/sessions/keys.txt --frames 3060 --screenshot /tmp/sparks.png

Measured with `cargo test --release -p terse-cli --test sparks budget -- --nocapture`:
the busiest frame is frame 0 (setting up, 8,641 units, 6.9% of the budget); after
that at most 5,240 (4.2%), a typical frame 2,072 (1.7%), about 11 µs of host time
(p99 17 µs). ROM 5,856 bytes, 1 RAM page.

## Conformance image

`programs/conform/conform.terse` (C and assembly: `conform.c`, `cpu.S`, with its own
`crt0.S` and `conform.ld`, so changes to `programs/lib` do not touch it) is for
whoever writes a new host: run it with its scripted input `conform.events` and
compare. It needs devices 2, 3 and 4 and 16 RAM pages. Two uses:

**Conformance (headless, deterministic).** Run it for 200 frames with an empty
store and no subscriptions, printing the state hash ([STATE.md](STATE.md)) every
10 frames, and compare with `expected-hashes.txt`:

    mkdir -p /tmp/conform/store /tmp/conform/config
    cargo run -p terse-cli -- run programs/conform/conform.terse --events programs/conform/conform.events --store /tmp/conform/store --config /tmp/conform/config --frames 200 --hash-every 10
    cargo run -p terse-cli -- run programs/conform/conform.terse --events programs/conform/conform.events --store /tmp/conform/store --config /tmp/conform/config --frames 200 --dump --dump-vectors

The second prints the final screen (`expected-screen.txt`): one PASS or FAIL line
per section the program can check itself (a FAIL line says how many checks failed
and which was first), and the display list. The hashes are the authoritative
check: they also cover what the program cannot see (the palette, the display
list, every register, every unit of budget). `cargo test -p terse-cli --test
conform` does both and checks `conform.terse` is built from its sources
(`programs/conform/build.sh` rebuilds it; `UPDATE_GOLDEN=1` rewrites the expected
files). Esc exits with the number of failed sections as the exit code.

What it checks (the frame plan is in `conform.c`'s header comment):

- the CPU: every RV32I instruction class in its 32-bit encoding, the M extension's
  corner cases (`mulh*` signs, division by zero, −2³¹ / −1), the compressed forms,
  writes to `x0` and HINTs, `FENCE` forms, shifts by 0 and 31, load sign
  extension, `jalr` clearing bit 0, and that a call preserves every register but
  `a0`/`a1`;
- code in RAM: run it, patch 32- and 16-bit instructions, run again, code that
  overwrites its own next instructions, an `ecall` from RAM, an instruction pair in
  the last bytes of RAM;
- the budget, by counting a loop's calls per frame, which a model in `conform.c`
  predicts from the constants of SPEC 1.4 and 11 (a loop of period p whose call is
  k units in, starting s units into a frame of budget b, makes ⌈(b − s − k) / p⌉
  calls): a ROM loop 17,856 17,856 17,857; the same loop in RAM 7,811 each frame;
  a loop with a scroll in it 758 758 757 (the deficit carried; a host that drops it
  gets 758 every time); the bytes one frame's `write` calls take, 124,922; and the
  charge for content delivered at the start of a frame (17,856 then 17,853);
- events of every kind (frames 2–5), queue overflow (70 text events, 64 kept),
  controllers 0 and 2, disconnection, controllers that do not exist;
- every system call, unknown calls and absent devices, bad buffers (null, ROM for
  a written buffer, straddling the end of RAM) and empty ones;
- the terminal: every command and control character, pending wrap and what clears
  it, wide characters at columns 78 and 79 and on the last row, overwriting half a
  wide pair, malformed UTF-8 (the WHATWG cases, U+FFFF, above U+FFFF, sequences
  split across `write` calls), cells and palette calls with their wrap and bounds;
- the vector display: every opcode (and bytes after End), every rejection, a
  16,384-byte list ending at the last byte of RAM, lists cut short and ending at an
  unknown opcode, style, persistence and fill with rejected values, and the fill
  cap: the final list draws three translucent full-screen rectangles, then a
  fourth full-screen rectangle and a disc in opaque red that the cap drops (a host
  that ignores the cap turns the screen red), then a line and circle that are
  still drawn, with a circle of radius 200 units at the centre that must be round;
- the content device without a network: puts from RAM and from ROM, gets (whole,
  from offset 90, at and beyond the end), a blob nobody has, 18 kinds of bad
  request, blocks outside RAM, eight pending and a ninth rejected, and the
  subscriptions and timeline operations (status only, so a host with feeds still
  passes);
- frames 49–108, the stress phase: every frame uses its whole budget, rewrites all
  2,400 cells (600 CJK, Extension A and Hangul characters never shown before, Latin
  Extended-A and Cyrillic rows, colours changing) and submits a new 16,384-byte
  display list: frames 49–78 mixed shapes (about 1,900 circles and many lines; the
  fill cap stops the fills), frames 79–108 the worst legal case: fills totalling
  just under the cap (three screen-covering discs and a 639 × 480 rectangle), then
  3,268 translucent lines of width 255 across the screen, about 109 screens of line
  area (lines are not capped, SPEC 14).

**Responsiveness (window).** `terse-host --selftest` runs the image in the normal
window with vsync and the same script, and prints for each machine frame the
emulation time (`run_frame` and handing requests to the content service), the
render-submit time (uploading the grid and the list, encoding, submitting,
presenting), the wait for a swapchain image, and the interval between presents:
p50, p99 and max, how many frames' emulation plus render took over 16.7 ms and
over 4 ms, and how many machine frames were never presented (the host keeps 60
frames/s of virtual time and skips presents when drawing falls behind).
`--csv <file>` writes every frame's numbers, and `terse run --bench` measures
emulation alone, headless:

    cargo run --release -p terse-host -- --selftest programs/conform/conform.terse --store /tmp/conform/store --config /tmp/conform/config --csv /tmp/conform/frames.csv
    cargo run --release -p terse-cli -- run programs/conform/conform.terse --events programs/conform/conform.events --store /tmp/conform/store --config /tmp/conform/config --frames 200 --bench

Measured on the development machine (Linux, i7-10750H, RTX 2060, Vulkan, 60 Hz),
release build: emulation of a full-budget stress frame about 0.57 ms (headless
over 200 frames: p99 0.52 ms, max 0.86 ms, 94 times real time); render-submit
about 7 ms in the stress frames (almost all of it rasterising some 600 new CJK
glyphs a frame), 0.6–1.1 s once when the CJK fallback fonts are first loaded; GPU
render about 1.3 ms (max 3 ms) even in the worst-legal-fill frames, which present
at 60 Hz. Nothing passes or fails on these numbers.

## Measurements

Release build on the development machine (i7-10750H). Budget 125,000 units per
frame; "busiest" and "typical" (median) are units used per frame in the tests'
recorded sessions.

| Program | ROM | RAM | Busiest frame | Typical frame | Host time per frame |
|---|---|---|---|---|---|
| hello | 889 bytes | 1 page | 1,649 (1.3%, frame 0) | 152 (0.1%) | about 1 µs |
| fetch | 1,356 bytes | 1 page | 2,239 (1.8%) | 36 | about 1 µs |
| reader | 10,292 bytes | 9 pages | 62,269 (50%, wrapping an article) | 277 (0.2%) | 36 µs (with the content service) |
| sparks | 5,856 bytes | 1 page | 8,641 (6.9%, frame 0); 5,240 (4.2%) after | 2,072 (1.7%) | 11 µs (p99 17 µs) |
| conform | 34,626 bytes | 16 pages | 125,000 (stress) | — | 0.57 ms (stress) |

Host cost per budget unit, for a frame spent entirely in one kind of call
(`cargo test --release -p terse-core --test charges -- --ignored --nocapture
--test-threads 1`), against plain instructions from ROM:

| Frame spent in | ns per unit | × ROM instructions |
|---|---|---|
| plain instructions from ROM | 4.3 | 1.00 |
| `frame_count` / `set_cursor` / `style` (call only) | 3.1–3.4 | 0.73–0.79 |
| `next_event` (empty), `controller`, `random` 4 bytes | 2.8–3.5 | 0.66–0.81 |
| `random` 256 bytes | 0.7 | 0.16 |
| `write` 1 byte / 80 ASCII / 120 bytes CJK and ASCII | 4.0 / 2.4 / 4.3 | 0.92 / 0.57 / 1.01 |
| `write` of newlines on the last row (a scroll each) | 1.0 | 0.23 |
| `command` clear / scroll | 1.1 | 0.25 |
| `cells_write` 8 / 2,400 cells, `cells_read` 2,400 | 2.4 / 0.23 / 0.20 | 0.56 / 0.05 / 0.05 |
| `palette_set` 256 | 1.2 | 0.28 |
| vector `submit` 100 / 16,384 bytes | 1.4 / 0.21 | 0.32 / 0.05 |
| content put 16 KiB (8 accepted a frame) / 1 MiB | 4.1 / 1.3 | 0.94 / 0.30 |
| content get, 16 KiB delivered per completion | 4.0 | 0.93 |
