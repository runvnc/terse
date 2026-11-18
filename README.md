# Terse

A small virtual computer: a Zilog Z80 at a fixed 300,000 T-states per frame,
paged ROM/RAM, and host devices reached through I/O ports. `SPEC.md` is the
authoritative design and `NETWORK.md` describes how hosts move blobs and feeds
through a relay; this repository is the first prototype of both.

## Build, run, test

Requires a Rust toolchain (edition 2024). If `cargo` is not on your `PATH`
(rustup puts it in `~/.cargo/bin`), add it for the current shell:

    # bash / zsh
    export PATH="$HOME/.cargo/bin:$PATH"

    # fish (the export line above breaks PATH in fish; this also persists)
    fish_add_path ~/.cargo/bin

    cargo build --workspace
    cargo test --workspace

    cargo run -p terse-cli -- info programs/hello/hello.terse
    cargo run -p terse-cli -- run programs/hello/hello.terse --frames 30 --dump
    cargo run -p terse-host -- programs/hello/hello.terse
    cargo run -p terse-host -- programs/hello/hello.terse --screenshot hello.png --frames 30

`terse` commands:

- `run <image|id> [--frames N] [--dump [--dump-vectors]] [--events <file>] [--relay ws://...] [--store <dir>] [--config <dir>] [--hash] [--hash-every N] [--bench]`
  runs headless (input only from `--events`, see [Scripted input](#scripted-input);
  Unix time 0, random bytes 00h, controllers only from `pad` lines) for N frames or
  until the program exits (capped at 36,000 frames), prints the 80x30 grid with
  `--dump` (a wide character's continuation cell is skipped), and exits with the
  machine's exit code. `--dump-vectors` adds the display list device 3 holds, after
  a `-- vectors: N list bytes --` line, one decoded primitive per line (see
  [Vector display](#vector-display)).
  Content requests (device 4) are carried out between frames and always complete at
  the start of the next frame, so runs are reproducible. For an image with device 4
  the feeds (SPEC 7.7) are those subscribed in `--config` (see [Feeds](#feeds)):
  with `--relay`, `run` first syncs them until caught up (NETWORK 4.3; a warning and
  the local entries if that fails within 60 s), then keeps following them while the
  machine runs; an entry stored during frame N is announced by a feeds-updated event
  in frame N + 1.
  `--hash` prints the machine's state hash ([STATE.md](STATE.md)) after the last
  frame as 64 hex digits; `--hash-every N` prints `<frame> <hash>` after every frame
  whose frame counter is a multiple of N (after frames N, 2N, …). `--bench` prints,
  on stderr, the emulation time per frame (p50, p99, max; the machine and the
  headless content service) and how many times faster than real time the run was.
- `info <image|id>` prints the header fields and the image ID (BLAKE3-256).
- `put <file>` stores a blob (and sends it to `--relay`) and prints its ID;
  `get <id> [-o <file>]` fetches one; `hash <file>` prints a file's blob ID.

An `<id>` is 64 hex digits, looked up in the blob store and then on the relay;
the bytes are verified against the ID. `--store` defaults to
`$XDG_CACHE_HOME/terse/blobs` (else `~/.cache/terse/blobs`). Only `ws://` relays
are supported.
- `pack --rom <raw.bin> --title <t> --ram-pages <n> [--device <slot:version>]... -o <out.terse>`
  builds an image from a raw ROM.
- `asm <source.asm> -o <out.bin>` assembles Z80 source (see below).
- `key new|show` and `feed …`: feeds, see [Feeds](#feeds).
- The arrival order of entries (the timeline, SPEC 7.7) is kept in
  `<store>/feeds/arrivals`.

`terse-host <image|id> [--relay ws://...] [--store <dir>] [--config <dir>]` opens a
window and runs the image at 60 frames/s of virtual time. The screen (SPEC 5.4) is
shown at 4:3, so screen units are square: a cell is exactly 1:2 pixels (11 × 22 at
the default font size; the glyphs are rasterised at that size, so text is crisp at
the native size and at whole multiples of it), and the grid is scaled uniformly,
centred and letterboxed in a window of any other shape. Input: keyboard and text;
the pointer in screen units (window pixels map to 640x480 through the same
rectangle, clamped at the edges, movement at most once a frame); the wheel (line and pixel deltas, one step per terminal row of travel);
game controllers through [gilrs](https://crates.io/crates/gilrs), numbered in
order of connection. Content requests run on a background thread and complete at
the first frame after they finish; with `--relay` the subscribed feeds sync in the
background and new entries are announced with a feeds-updated event. The window
closes when the program writes the exit port. `--screenshot <out.png> --frames N
[--events <file>] [--size <W>x<H>]` runs headless exactly like `terse run` and writes a
PNG, of the grid's native size or `W` × `H` pixels with the screen letterboxed in it.
`--selftest <image|id> [--frames N] [--events <file>] [--csv <file>]` runs N frames
(default 200) in the window with vsync, with only the scripted input (default: the
image's file name with `.events`, if it exists; keyboard, mouse and gamepads are
ignored), and prints timings: see [Conformance image](#conformance-image).

`terse-relay [--listen 127.0.0.1:7878] --store <dir> [--max-store-bytes N] [--feed-entries N]`
serves a blob store and feeds over WebSocket (NETWORK.md sections 3 and 4.3), one
thread per connection. It keeps the newest `--feed-entries` entries of each feed
(default 1024, at least 256) in `<dir>/feeds`, and listens on loopback unless told
otherwise.

## Trying the relay locally

    cargo build --workspace
    target/debug/terse-relay --store /tmp/relay-store &          # ws://127.0.0.1:7878
    target/debug/terse put programs/fetch/message.txt --relay ws://127.0.0.1:7878 --store /tmp/a
    target/debug/terse put programs/fetch/fetch.terse --relay ws://127.0.0.1:7878 --store /tmp/a
    # prints fetch.terse's image ID; run it from an empty store:
    target/debug/terse run <id> --relay ws://127.0.0.1:7878 --store /tmp/b --frames 10 --dump
    target/debug/terse-host <id> --relay ws://127.0.0.1:7878 --store /tmp/b

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

Two users on one machine (each with their own store and config), with
`target/debug/terse-relay --store /tmp/relay-store` running:

    # terminal 1: the author
    T=target/debug/terse; A="--relay ws://127.0.0.1:7878 --store /tmp/author --config /tmp/author-config"
    $T key new $A                                  # prints the feed key
    echo "Hello, feeds." > /tmp/hello.txt
    $T feed publish /tmp/hello.txt --title "Hello" $A

    # terminal 2: the reader
    T=target/debug/terse; R="--relay ws://127.0.0.1:7878 --store /tmp/reader --config /tmp/reader-config"
    $T feed subscribe <feed key> $R
    $T feed sync $R                                 # prints the entry; article downloaded
    $T feed list $R
    $T feed read <feed key> 1 $R
    $T feed sync --watch $R                         # leave running

    # terminal 1 again: appears in terminal 2 at once
    $T feed publish /tmp/hello.txt --title "Again" $A

(The variable lines are bash/zsh; in fish write `set T target/debug/terse` and
`set A --relay ws://127.0.0.1:7878 --store /tmp/author --config /tmp/author-config`.)

## Layout

    crates/terse-core/   the machine: image parsing, memory, frame loop, devices 0-4. No
                         GUI, threads, clock, I/O or OS randomness; inputs (events,
                         time, random bytes, controllers, content completions) are passed
                         in. wide.rs holds the Unicode 16.0 East Asian Width W/F table;
                         vector.rs is device 3 and the one display-list decoder.
    crates/terse-net/    host side of device 4 and feeds: blob store, identities, entries,
                         feed store, relay protocol, relay client (behind a small
                         Transport trait), content service thread
    crates/terse-relay/  `terse-relay`: the WebSocket blob and feed relay
    crates/terse-cli/    `terse`: headless runner, image info/pack, blobs, feeds, assembler
                         front end
    crates/terse-host/   `terse-host`: winit + wgpu window; renderer.rs is the
                         window-independent renderer (terminal backgrounds, vectors.rs's
                         shapes, then glyphs: SPEC 8.6)
    crates/rustzx-z80/   vendored Z80 core (rustzx-z80 0.16.0, MIT) with a small patch,
                         see crates/rustzx-z80/PATCHES.md
    STATE.md             the state hash: the byte layout of the machine state, for
                         comparing implementations frame by frame (proposed for SPEC 9)
    programs/hello/      demo program: hello.asm, build.sh, the built hello.terse and
                         the golden screen used by the tests
    programs/fetch/      content device demo: gets message.txt by its ID and puts a
                         buffer; build.sh writes message_id.inc, then fetch.terse
    programs/lib/        C support for SDCC: crt0.s, terse.h, terse.c
    programs/reader/     the feed reader, in C: reader.c, build.sh, the built
                         reader.terse and the golden screens in expected/
    programs/sparks/     a game for the vector display, in C: sparks.c, build.sh, the
                         built sparks.terse, recorded sessions in sessions/ and the
                         golden screens in expected/
    programs/conform/    the conformance and responsiveness image: conform.asm,
                         build.sh, conform.terse, its input conform.events, and
                         expected-hashes.txt and expected-screen.txt

## Assembling programs

Programs are assembled with the [`rsasm`](https://crates.io/crates/rsasm) library
(MIT, Z80 Zilog syntax) through `terse asm`; no external assembler is needed.
`programs/hello/build.sh` assembles `hello.asm` and packs `hello.terse`; set
`CARGO` if cargo is not on `PATH`. `programs/fetch/build.sh` also regenerates
`message_id.inc` (included from `fetch.asm`; `include` paths are relative to the
source file). Tests check that the checked-in images match their sources.

rsasm 0.1.3 rejects a lone symbol in parentheses (`out (PORT),a`,
`ld a,(var)`), so `terse asm` rewrites those to `(0+PORT)` before assembling.
Sources must start with `org 0`.

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

A `pad` line connects controller `n` and sets its whole state (SPEC 5.3) until the
next `pad` line for it; controllers are disconnected until their first one.
`<buttons>` is hexadecimal, byte 1 in bits 0-7 and byte 2 in bits 8-15 (`1` up,
`2` down, `4` left, `8` right, `10` A, `20` B, `2000` Start, `14` A and left); the
sticks are -127 to 127, x then y, and the right stick defaults to `0 0`.

## C programs

`programs/lib` holds what a C program needs with [SDCC](https://sdcc.sourceforge.net)
(4.x, the `z80` port; the reader is built with 4.6.0):

- `crt0.s` (sdasz80): the reset entry at 0000h (DI, stack at the top of RAM), the
  RST 38h frame-interrupt handler (it counts frames in `terse_ticks`), copying of
  initialised globals, IM 1 + EI, `call _main`, and main's return value written to
  the exit port. RAM is zero at reset, so nothing is cleared.
- `terse.h` / `terse.c`: ports as SDCC `__sfr __at` variables, events, waiting for a
  frame (HALT), the frame counter and time, controllers (`terse_pad_read`, `PAD_*`
  button bits), terminal helpers (put character and string, a code point as UTF-8,
  a run of ASCII, cursor, colours, commands, cell-pointer writes, the
  wide-character test), the vector display (a display-list builder and submit, see
  below), content requests (the request block, get, the feed operations, linear RAM
  addresses).

Memory: code in ROM from 0000h (vectors) and 0100h (`--code-loc`), data and the
stack in RAM from 8000h (`--data-loc`; slots 4-7 show RAM pages 80h-83h at reset).
The build (see `programs/reader/build.sh`):

    sdasz80 -g -o crt0.rel programs/lib/crt0.s
    sdcc -mz80 --std-c11 --opt-code-size -Iprograms/lib -c programs/lib/terse.c -o terse.rel
    sdcc -mz80 --std-c11 --opt-code-size -Iprograms/lib -c prog.c -o prog.rel
    sdcc -mz80 --no-std-crt0 --nostdlib --code-loc 0x0100 --data-loc 0x8000 \
        -L /usr/share/sdcc/lib/z80 -l z80 crt0.rel prog.rel terse.rel -o prog.ihx
    makebin -p prog.ihx prog.bin
    terse pack --rom prog.bin --title prog --ram-pages 4 --device 2:0 --device 4:0 -o prog.terse

`crt0.rel` must come first. Of SDCC's library only the support routines the code
calls are linked from `z80.lib`; for the reader they are 16- and 32-bit
multiplication and division (`mul`, `_mullong`, `__muluint2ulong`, `divunsigned`,
`modunsigned`, `_divulong`, `_modulong`) and `crtenter` (function entry). There is
no C library beyond that: no `printf`, no heap.

Things to know: SDCC 4.x passes the first argument in A (8 bits) or HL (16 bits)
and a following 8-bit argument after a 16-bit one on the stack (`sdcccall(1)`),
returns 8 bits in A and 16 in DE; locals live in an IX frame, so hot loops are
several times slower than hand-written Z80 (`term_put_ascii` and the reader's
`scan_ascii`/`decode_fast` are in assembly for that reason); dividing 32-bit
numbers costs about 12,000 T-states.

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
(read in 16 KiB gets; it says where it stops). It uses 10 RAM pages: 4 for its
variables, the timeline and the line index, 6 for the article.

Rebuild it with `programs/reader/build.sh` (needs SDCC; set `CARGO` if cargo is not
on `PATH`, or `TERSE` to a built `terse`). A test fails if `reader.terse` is out of
date with its sources (and is skipped without SDCC).

### Walkthrough

Two authors and a reader on one machine, each with their own store and config, in
fish (in bash/zsh write `T=target/release/terse` and `R="--relay …"`):

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

Measured with the golden-test fixture (`cargo test -p terse-cli --test reader timing
-- --nocapture`): the list of 8 entries is drawn by frame 2; an already-local 6 KB
article shows its first page 8 frames after Enter; the busiest frame of that session
uses 72% of the frame's 300,000 T-states and no frame's work runs into the next.

## Vector display

Device 3 (SPEC 8) draws a display list of lines, circle outlines, discs,
rectangles and triangles. A program builds the list in RAM and submits it (ports
30h-35h, 1 T-state per byte); the device keeps its copy and the host draws it every
frame until the next submit or clear. Coordinates are sixteenths of a screen unit,
so the screen is 10240 × 7680. An image gets the device with `terse pack … --device
3:0`; without it ports 30h-3Fh read FFh and nothing is drawn.

`terse_core::vector::decode` turns a list into resolved primitives (origin added,
colour and width tracked, the list's end rules applied); `terse-host`, `terse run
--dump-vectors` and the tests all draw or print from it. `terse-host` draws lines,
outlines and discs as instanced quads shaded by distance (round ends,
antialiased), rectangles and triangles as plain triangles, blended in list order,
clipped to the screen, and re-uploads them only when the list changes. The screen
is the terminal grid's rectangle, which is 4:3, so a circle is round (a radius-200
disc measures 550 × 550 pixels at the default size; before cells were made 1:2 it
was 550 × 574). Shapes are blended in sRGB space, like the terminal's glyphs. Layers
(SPEC 8.6): cell backgrounds, then shapes, then glyphs and the cursor, so text is
always readable over shapes. A program without device 3 looks exactly as before.

From C (`programs/lib/terse.h`), the builder appends to a buffer you own:

    static uint8_t list[512];
    vec_begin(list);
    vec_colour(255, 200, 0, 255);           /* r, g, b, opacity */
    vec_width(32);                          /* two screen units */
    vec_move(160 * VEC_UNIT, 120 * VEC_UNIT);
    vec_line(480 * VEC_UNIT, 120 * VEC_UNIT);
    vec_disc(10 * VEC_UNIT);                /* around the current point */
    vec_submit_list(list);                  /* VEC_DONE or VEC_REJECTED */

Each builder call is a few dozen to 150 T-states (assembly); `vec_tri`, `vec_rect`,
`vec_circle`, `vec_origin`, `vec_end` and `vec_clear()` are the rest.

## Sparks

`programs/sparks` is a small game for the vector display, in C. Steer a small
craft around the arena: it turns, thrusts and drifts with inertia, and bounces
softly off the walls. Fly over the drifting sparks to collect them; the sweepers,
bars turning about fixed pivots, cost a life when they touch you (three lives, then
a moment of invulnerability after each hit). Every ten sparks another sweeper
appears, up to six. Until you start, a demo plays itself with six sweepers.

    cargo run --release -p terse-host -- programs/sparks/sparks.terse

- Keyboard: Left/Right turn, Up thrusts, Enter starts (and restarts after game
  over), Esc quits.
- Controller 0: d-pad left/right or the left stick turns, A, d-pad up or the stick
  pushed up thrusts, A or Start starts.

The HUD (row 0) and the title and game-over boxes are terminal text on coloured
cell backgrounds, so the layer order shows: shapes pass over the backgrounds and
under the text. The Z80 does all the geometry: a 256-entry sine table and an
assembly multiply (`fmul`, about 500 T-states) rotate the sweepers and the craft,
in sixteenths so motion is smooth; the device only receives finished coordinates.
Randomness is a xorshift generator stirred with port 1Ch, so a run is a function
of its recorded inputs.

Rebuild with `programs/sparks/build.sh` (needs SDCC; set `CARGO` or `TERSE` as for
the reader); a test fails if `sparks.terse` is out of date. The tests
(`crates/terse-cli/tests/sparks.rs`) replay two recorded 3,600-frame sessions in
`programs/sparks/sessions/` (`keys.txt` with the keyboard, `pad.txt` with `pad`
lines), recorded by a player that only looks at the display list (`cargo test -p
terse-cli --test sparks record_sessions -- --ignored`). They compare the terminal
and the decoded display list with `programs/sparks/expected/` (`UPDATE_GOLDEN=1`
rewrites them), check a replay is identical, and that no frame overruns. To watch a
recording headless:

    target/debug/terse run programs/sparks/sparks.terse --events programs/sparks/sessions/keys.txt --frames 900 --dump --dump-vectors
    target/debug/terse-host programs/sparks/sparks.terse --events programs/sparks/sessions/keys.txt --frames 3060 --screenshot /tmp/sparks.png

Measured with `cargo test -p terse-cli --test sparks budget -- --nocapture`: the
busiest frame is frame 0 (setting up, 37% of the 300,000 T-states); after that at
most 27%, the demo with six sweepers and eight sparks at most 24%, the median frame
15%. The display list is about 320 bytes (at most 410). ROM 6,990 bytes, 4 RAM
pages.

## Conformance image

`programs/conform/conform.terse` (assembly, `conform.asm`) is for whoever writes a
new host: run it with its scripted input `conform.events` and compare. It needs
devices 2, 3 and 4 and 16 RAM pages. Two uses:

**Conformance (headless, deterministic).** Run it for 200 frames with an empty
store and no subscriptions, printing the state hash ([STATE.md](STATE.md)) every
10 frames, and compare with `expected-hashes.txt`:

    mkdir -p /tmp/conform/store /tmp/conform/config
    cargo run -p terse-cli -- run programs/conform/conform.terse --events programs/conform/conform.events --store /tmp/conform/store --config /tmp/conform/config --frames 200 --hash-every 10
    cargo run -p terse-cli -- run programs/conform/conform.terse --events programs/conform/conform.events --store /tmp/conform/store --config /tmp/conform/config --frames 200 --dump --dump-vectors

The second prints the final screen (`expected-screen.txt`): one PASS or FAIL line
per section the program can check itself, and the display list. A FAIL line says
how many checks failed and where the first one was (the address of the test op in
`conform.asm`'s listing, or a checkpoint number in sections written as code). The
hashes are the authoritative check: they also cover what the program cannot see
(the palette, the display list, the CPU's internal state, every T-state). `cargo
test -p terse-cli --test conform` does both and checks `conform.terse` is built
from `conform.asm` (`programs/conform/build.sh` rebuilds it; `UPDATE_GOLDEN=1`
rewrites the expected files). Esc exits with the number of failed sections as the
exit code.

What it does, by frame (the scripted input is in the frames named):

- frame 0: paging (every slot shows every RAM page, ROM pages 0-3, ROM beyond the
  image and unmapped pages, with interrupts off and the test running from RAM for
  slot 0); system ports and absent devices (`FFh`), the latched counter and time,
  the upper port address byte ignored;
- frames 2-5 (read at 6): key down/up with modifiers, text including U+1F600,
  pointer at both corners with all buttons, a click, wheel steps, two
  feeds-updated events in one frame (one kept); frame 8 (read at 9): 70 text events,
  64 kept; frames 10-13: controllers 0 and 2, selection, wrap-around, the read
  position carried across frames, disconnection;
- timing: the iterations of a 35-T-state loop between two frame interrupts in
  modes 1, 2 and 0 (8,566, 8,564 and 8,566: worked out by hand from SPEC 1.3 and
  the instruction timings; mode 2 accepts in 19 T-states and its handler is longer),
  three frames of a loop that scrolls the terminal so up to 646 T-states run over
  into the next frame (464, 463, 463; a host that drops the negative remainder gets
  464 every time), the interrupt's return address after HALT and the advance of R,
  device charges (seven 600-T-state clears and scrolls, a 1,000-byte submit and a
  500-byte put: 8,395), and a frame that never halts with interrupts off for 2.5
  frames (one interrupt after EI, and not before the instruction after it);
- the terminal: every command and control character, pending wrap and what clears
  it, wide characters at columns 78 and 79 and on the last row, overwriting half of
  a wide pair, malformed UTF-8 (the WHATWG cases, U+FFFF, above U+FFFF), the cell
  pointer (wrap, values of 9,600 and more), palette writes and index wrap;
- the vector display: a list with every opcode (and bytes after End), every
  rejection (length 0 and 16,385, ranges past the end of RAM, unknown commands), a
  16,384-byte list ending at the last byte of RAM, lists cut short and ending at an
  unknown opcode;
- the content device without a network: a put, then gets of the same blob (whole,
  from offset 90 and at the end), a get of a blob nobody has, twelve kinds of bad
  request, blocks outside RAM, eight pending requests and a ninth rejected, and
  the subscriptions and timeline operations (only their status is checked, so a
  host with feeds still passes);
- frames 47-125, the stress phase: every frame uses its whole budget (no HALT),
  rewrites all 2,400 cells (rows of 40 CJK characters never shown before, rows of
  80 Latin Extended-A or Cyrillic letters, colours changing) and submits a new
  16,384-byte display list: frames 47-106 a list of 4,257 lines, rectangles,
  triangles, circles and discs, frames 107-125 one of 5,456 translucent discs that
  each cover the whole screen (the fill-rate worst case);
- from frame 126: the results, over a display list whose circle of radius 200
  units at the centre must be round.

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

Measured on the development machine (Linux, RTX 2060, Vulkan, 60 Hz), release
build: emulation of a full-budget stress frame about 0.9 ms (headless: p99 0.9-1.3
ms, 40 times real time over the 200 frames); render-submit 0.4 ms for the test
screens, about 8 ms in the stress frames (almost all of it rasterising some 600
new CJK glyphs a frame), 0.66 s once when the CJK fallback font is first loaded;
the full-screen-disc frames are limited by the GPU (a present every 60-80 ms, so
most of those frames are not presented). A debug build takes about 75 ms per
stress frame. Nothing passes or fails on these numbers.
