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

- `run <image|id> [--frames N] [--dump] [--events <file>] [--relay ws://...] [--store <dir>] [--config <dir>]`
  runs headless (input only from `--events`, see [Scripted input](#scripted-input);
  Unix time 0, random bytes 00h, no controllers) for N frames or until the program
  exits (capped at 36,000 frames), prints the 80x30 grid with `--dump` (a wide
  character's continuation cell is skipped), and exits with the machine's exit code.
  Content requests (device 4) are carried out between frames and always complete at
  the start of the next frame, so runs are reproducible. For an image with device 4
  the feeds (SPEC 7.7) are those subscribed in `--config` (see [Feeds](#feeds)):
  with `--relay`, `run` first syncs them until caught up (NETWORK 4.3; a warning and
  the local entries if that fails within 60 s), then keeps following them while the
  machine runs; an entry stored during frame N is announced by a feeds-updated event
  in frame N + 1.
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
window and runs the image at 60 frames/s of virtual time. Input: keyboard and text;
the pointer in screen units (SPEC 5.4: window pixels map to 640x480 through the
transform that places the grid, clamped at the edges, movement at most once a
frame); the wheel (line and pixel deltas, one step per terminal row of travel);
game controllers through [gilrs](https://crates.io/crates/gilrs), numbered in
order of connection. Content requests run on a background thread and complete at
the first frame after they finish; with `--relay` the subscribed feeds sync in the
background and new entries are announced with a feeds-updated event. The window
closes when the program writes the exit port. `--screenshot <out.png> --frames N
[--events <file>]` runs headless exactly like `terse run` and writes a PNG.

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

    crates/terse-core/   the machine: image parsing, memory, frame loop, devices 0, 1, 2
                         and 4. No GUI, threads, clock, I/O or OS randomness; inputs
                         (events, time, random bytes, content completions) are passed in.
                         wide.rs holds the Unicode 16.0 East Asian Width W/F table.
    crates/terse-net/    host side of device 4 and feeds: blob store, identities, entries,
                         feed store, relay protocol, relay client (behind a small
                         Transport trait), content service thread
    crates/terse-relay/  `terse-relay`: the WebSocket blob and feed relay
    crates/terse-cli/    `terse`: headless runner, image info/pack, blobs, feeds, assembler
                         front end
    crates/terse-host/   `terse-host`: winit + wgpu window; renderer.rs is the
                         window-independent terminal renderer (one fragment-shader pass)
    crates/rustzx-z80/   vendored Z80 core (rustzx-z80 0.16.0, MIT) with a small patch,
                         see crates/rustzx-z80/PATCHES.md
    programs/hello/      demo program: hello.asm, build.sh, the built hello.terse and
                         the golden screen used by the tests
    programs/fetch/      content device demo: gets message.txt by its ID and puts a
                         buffer; build.sh writes message_id.inc, then fetch.terse
    programs/lib/        C support for SDCC: crt0.s, terse.h, terse.c
    programs/reader/     the feed reader, in C: reader.c, build.sh, the built
                         reader.terse and the golden screens in expected/

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

`<key>` is a USB HID usage ID (`40` or `0x28`) or a name: `a`-`z`, `0`-`9`,
`f1`-`f12`, `enter`, `esc`, `backspace`, `tab`, `space`, `up`, `down`, `left`,
`right`, `home`, `end`, `pageup`, `pagedown`, `insert`, `delete`. The events of a
frame are delivered in the order written. A cell (column, row) starts at screen
unit (column × 8, row × 16).

## C programs

`programs/lib` holds what a C program needs with [SDCC](https://sdcc.sourceforge.net)
(4.x, the `z80` port; the reader is built with 4.6.0):

- `crt0.s` (sdasz80): the reset entry at 0000h (DI, stack at the top of RAM), the
  RST 38h frame-interrupt handler (it counts frames in `terse_ticks`), copying of
  initialised globals, IM 1 + EI, `call _main`, and main's return value written to
  the exit port. RAM is zero at reset, so nothing is cleared.
- `terse.h` / `terse.c`: ports as SDCC `__sfr __at` variables, events, waiting for a
  frame (HALT), the frame counter and time, controllers, terminal helpers (put
  character and string, a code point as UTF-8, a run of ASCII, cursor, colours,
  commands, cell-pointer writes, the wide-character test), content requests (the
  request block, get, the feed operations, linear RAM addresses).

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
