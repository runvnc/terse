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

- `run <image|id> [--frames N] [--dump] [--relay ws://...] [--store <dir>]` runs
  headless (no input events, Unix time 0, random bytes 00h) for N frames or until
  the program exits (capped at 36,000 frames), prints the 80x30 grid with `--dump`
  (a wide character's continuation cell is skipped), and exits with the machine's
  exit code. Content requests (device 4) are carried out between frames and always
  complete at the start of the next frame, so runs are reproducible.
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

`terse-host <image|id> [--relay ws://...] [--store <dir>]` opens a window and runs
the image at 60 frames/s of virtual time, with keyboard, text and pointer input.
Content requests run on a background thread and complete at the first frame after
they finish. The window closes when the program writes the exit port.
`--screenshot <out.png> --frames N` renders offscreen instead and writes a PNG.

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
  up: every subscription answered, downloads done, then 0.5 s with nothing new.
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
