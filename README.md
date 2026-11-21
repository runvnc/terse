# Terse

First and foremost, Terse is a way to share information: formatted text,
with diagrams where they help, without the bloat of web browsers
(JavaScript, ads, tracking) and without the centralised control of
websites like Reddit. Secondly, and completely optionally, it can be
used to share very lightweight programs, at most 1 MiB of memory each,
that run on tiny isolated virtual machines.

## Install

Download the file for your system from the
[latest release](https://github.com/runvnc/terse/releases/latest), unpack it
and run `terse-host`: the window opens on the launcher. Choose **reader**,
press **Enter**, then **M**: that is the Terse manual. Nothing else needs to be
installed. Each download also has `terse` (the command-line tool),
`terse-relay` (for sharing) and a `README.txt`.

- **Windows 10 or newer**: `terse-<version>-windows-x86_64.zip`. Extract all,
  double-click `terse-host.exe`. The files are not signed, so SmartScreen may
  say "Windows protected your PC": click **More info**, then **Run anyway**.
- **macOS 11 or newer** (Apple silicon and Intel): `terse-<version>-macos.zip`.
  The files are not notarised by Apple, so macOS blocks them the first time.
  Unzip, then in Terminal run
  `xattr -dr com.apple.quarantine ~/Downloads/terse-<version>-macos` and
  double-click `Terse.app` (or Control-click `Terse.app`, Open; on macOS 15 or
  newer: try once, then System Settings > Privacy & Security > Open Anyway).
- **Linux** (x86_64 or aarch64, glibc 2.35 or newer: Ubuntu 22.04, Debian 12,
  Fedora 36 or newer): `terse-<version>-linux-x86_64.tar.gz` (or `-aarch64`).
  `tar xzf` it and run `./terse-<version>-linux-x86_64/terse-host`. It needs
  Wayland or X11, and draws with Vulkan or OpenGL if it can, with the CPU
  otherwise. `terse` and `terse-relay` are static and run on any Linux.
- **A terminal reader** (Linux, macOS): `tersereader-<version>-<system>.tar.gz`,
  the Go reader of `native/reader-go/`, one static file.

To check a download, compare it with `SHA256SUMS.txt` from the same release:
`sha256sum -c --ignore-missing SHA256SUMS.txt` (Linux),
`shasum -a 256 -c --ignore-missing SHA256SUMS.txt` (macOS), or
`Get-FileHash <file>` in PowerShell (Windows), compared by eye.

To build from source instead, see [Getting started](#getting-started) and
[Build, run, test](#build-run-test). `RELEASING.md` says how the downloads are
made.

The Terse machine is a 32-bit RISC-V CPU (RV32IMC) with a fixed budget
of 125,000 units of work per frame, up to 128 KiB of ROM and 1 MiB of RAM,
and host devices reached through system calls (`ecall`): a text terminal, a
2D vector display, a page of typeset proportional text, and a content device
that fetches and publishes blobs and reads feeds. Several machines can run
side by side as one system: a shell program starts them and lays their screens
out as tiles.

Terse has three layers, each with its own specification:

- **(a) The Terse network**, `NETWORK.md`: blobs named by their hash, relays,
  signed feeds and their entries, profiles and names, search.
- **(b) Terse articles**, `ARTICLE.md`: the article format and what a reader
  must do with it.
- **(c) The Terse machine**: `SPEC.md`, the core machine (the CPU, the
  System, Terminal and 2D vector devices: everything a host that only runs
  programs needs, with a one-page summary of it at the top); one document per
  further device in `spec/` (content, page, machines, 3D vector display,
  sound); and `STATE.md`, the machine state hash that conformance compares.
  Sections are cited by stable ids, `SPEC §terminal.put`
  (`spec/RENUMBERING.md` maps the old section numbers).

(a) and (b) are **the core**: a native reader with no virtual machine that
implements them is a complete, conforming Terse client. The machine is optional
and reuses the core: programs are blobs published in feeds, and the reader that
comes with this repository is a machine program that shows articles on the page
device. A machine host that only runs that reader needs just the machine's
**reading profile** (devices 1, 2, 4 and 7; SPEC §profiles); one that runs programs needs
just the **program profile** (devices 1, 2 and 3), since a program lists the other
devices it can do without as optional (SPEC §image). This repository is the first
prototype of all three layers.

Each specification also says how quick an implementation must be, in units any
implementation can be tested against (round trips, frames, "never waits for a
relay"): `PERFORMANCE.md` collects those requirements, says how to test them and
gives the reference's times; see [Performance](#performance).

**A second reader.** `native/reader-go/` is such a native reader: a terminal
program in Go (`tersereader`), no virtual machine, written from `NETWORK.md`,
`ARTICLE.md` and `vectors/` alone. Its `README.md` says how to build and use it;
its `SPEC-FINDINGS.md` lists where the specifications left it guessing.

**A second core.** `native/core-c/` (`tcore`) is the machine's core alone,
about 1,450 lines of C written from `SPEC.md`, `STATE.md` and `DEVICES.md`:
the CPU, frames and budget, the System device and the state hash, with every
other device left to the reference drivers, started as processes on the
device bus. It gives the expected hashes of `conform-program`, `conform` and
`conform-reading` frame by frame; its `SPEC-FINDINGS.md` lists where the
documents left it guessing.

Shell commands below are written for fish; they also work in bash and zsh
unless they set variables (`set X value` in fish is `X=value` in bash).

## Getting started

With a Rust toolchain (see [Build, run, test](#build-run-test) if `cargo` is
not on your `PATH`), one command builds everything, sets up four sample
articles and opens the shell:

    cargo run --release -p terse-host -- --demo

The first time, this creates your feed key (`~/.config/terse/identity.key`),
subscribes you to your own feed, and publishes the four articles to it. It
uses a relay found as described below, starting one inside `terse-host` if
none is running. Running `--demo` again publishes nothing new. After that,
`cargo run --release -p terse-host` (no image named) opens the shell. If your
feed got the first three samples from an earlier `--demo`, publish the fourth
(drawings in an article, see [Writing an article](#writing-an-article)) yourself:

    cargo run --release -p terse-cli --bin terse -- feed publish crates/terse-host/demo/pictures.txt --title "Pictures in articles"

**The shell.** The window opens on the launcher: the programs built into
`terse-host` (reader, sparks, fetch, hello, sketch, holo). Up and Down choose one, Enter
starts it as a machine of its own, and it gets the keyboard. Start more the
same way: **F12 is the home key**, which always brings you back to the
launcher (no program ever sees F12). The bar at the top shows each machine:
its handle, title and share of the frame budget, the focused one in blue.
In the launcher, Esc goes back to the selected machine, Tab selects the next
one, X stops it, P pauses it, Z zooms it, L changes the layout (master and
stack, grid, gallery, zoom); a click on a machine's picture gives it the keyboard.
In the gallery the machines' screens stand on a turning ring in 3D; Left and Right
turn it, Enter flies the front one onto the screen, F12 sends it back.
See [The shell](#the-shell).

In the reader: Up and Down choose an article, Enter opens it, Esc goes back
(and quits from the timeline); M opens the Terse manual; F manages the feeds you follow; ? (or F1)
shows help and your own feed key; T (on the timeline, the feeds view or the help
page) publishes a test article. See [The reader](#the-reader).

**The consent prompt.** A program may ask the host to subscribe to a feed,
unsubscribe from one, publish an article or a document in your name, publish your
profile (your public name) or set a nickname for a feed (SPEC §content.feeds). The
host then shows its own prompt over the bottom four rows of the screen, over
every machine's picture, naming the program and what it asks (a feed key such as `3fa9c2…c21e07`;
the article's or document's title and size; the name and whether search may find
it; or the feed key and the nickname). Press Y to allow, N or Esc to refuse;
Enter does nothing. While the prompt is up, the keyboard goes to the host,
not to the program, and the program can neither see nor draw over the
prompt. Several requests are asked about one at a time.

**Which relays.** Without `--relay`, `terse-host` uses, in this order: the
URLs listed in the file `relays` in the config directory, one per line
(`terse relay add <url>` writes it; an older single-URL `relay` file is read
if there is no list); a relay already accepting connections on
`ws://127.0.0.1:7878`; or one it starts itself on that port, for as long as
it runs, with its store in `~/.local/share/terse/relay`
(`$XDG_DATA_HOME/terse/relay`). It prints one line on stderr saying which.
`--relay` may be given several times; `--relay none` runs without a network.
The host connects to all its relays (at most 8): puts and publishes go to
each, subscriptions are made on each, and a blob is asked of them in turn.
A dropped connection is retried after 1 s, doubling up to 30 s. For a feed
whose profile names relays (NETWORK.md section 4.6), the host also follows
it on up to two of those, unless `--no-feed-relays`. The `terse` commands
that use relays (`put`, `get`, `info`, `feed publish`, `feed sync`, `feed
read`, `profile set`, `find`, `manual publish`) look for them the same way, but never start
one.

**Sharing with someone else.** One of you runs a relay that the other can
reach, listening on all interfaces (it listens on loopback only by default),
before starting `terse-host`, which then finds it on port 7878:

    cargo run --release -p terse-relay -- --listen 0.0.0.0:7878 --store ~/terse-relay

The other names it once, then runs the shell (and the reader in it) as usual:

    cargo run --release -p terse-cli --bin terse -- relay add ws://their-host:7878
    cargo run --release -p terse-host

Exchange feed addresses (press ? in the reader to see yours in full: your
key, then `@` and your public relays, if any; `terse feed address` prints it
too; read it out or send it), then in the reader press F, then S, type or
paste the other's address (Ctrl+V or Shift+Insert pastes into the program),
press Enter and answer Y. An address with a relay (`key@their-host:7878`,
NETWORK.md 4.9) lets your host find the feed there even if you never added
that relay: it connects to it only if your own relays do not hold the
feed, and only to a public address. Everything the other has published, and everything they publish
from then on, appears in your timeline. The relay protocol is plain
WebSocket (`ws://`) without encryption or accounts: anyone who can reach
the relay can read and publish through it; entries are signed, so nobody
can publish in your name.

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

- `run --shell [<image|id>] [--builtin <image|id>]... [--hash-all]` with the options
  below runs a system (SPEC §machines): the image (default: the built-in shell,
  `programs/shell`) is the shell, with device 8. The built-in programs (reader,
  sparks, fetch, hello, sketch, holo, then each `--builtin` image) are put into the store
  (`--store`; use a temporary one for tests) and offered to the shell. The
  script's events are the host's input and the system routes them: `home` is the
  home key, pointer positions are in units of the whole screen, keys and text go to
  the machine with the focus. Images the shell starts are loaded between frames, so
  a machine started in frame N runs from frame N + 1. `--dump` prints the shell's
  screen, then for each machine with a rectangle a line `-- machine <handle>
  <state> <x> <y> <w> <h> <title> --` and its screen (and vectors and page with
  `--dump-vectors`, `--dump-page`); `--hash` prints the shell's state hash, and
  `--hash-all` prints it and then, after the last frame only, `<handle> <hash>` for
  each machine the shell has, in handle order (one it stopped is not listed; `-`
  for one that has no machine: its image or snapshot is still being fetched, or its
  start failed). `--hash-every` prints the shell's hashes. The exit status is the shell's. Without `--shell`, an image runs alone,
  as follows.
- `run <image|id> [--frames N] [--dump [--dump-vectors] [--dump-page]] [--events <file>] [--relay ws://...] [--store <dir>] [--config <dir>] [--data <dir>] [--consent allow|deny] [--hash] [--hash-every N] [--bench] [--save-state <file>] [--load-state <file>] [--record <file>] [--profile reading|program] [--with <slot>]... [--without <slot>]... [--no-feed-relays] [--no-program-download]`
  runs headless (input only from `--events`, see [Scripted input](#scripted-input);
  Unix time 0, no random bytes (so `random` gives zeros), controllers only from
  `pad` lines) for N frames or until the program stops (capped at 36,000
  frames), prints the 80x30 grid with `--dump` (a wide character's continuation
  cell is skipped). `--dump-vectors` adds the display list device 3 holds, after
  a `-- vectors: N list bytes --` line, one shape per line as the host draws it
  (see [Vector display](#vector-display)); when the style or fill mode is not
  the default a `-- vector style: … --` line comes first. For an image with the 3D
  display (device 9) the list follows as commands, one per line, after a `-- 3d
  vectors: N list bytes --` line, and a line `-- 3d: <n> segments, length <l> units
  --` says what the reference eye sees (see [3D vector display](#3d-vector-display)).
  `--dump-page` adds the
  page (device 7) as the reference layout lays it out (see [Page](#page)).
  The page is laid out with the reference layout between frames, so its height
  event arrives in the frame after a change, the answers to `locate` come with
  it (or in the next frame, if the document is laid out already), and a
  scripted press over anchored text is followed by its anchor event.
  Content requests (device 4) are carried out between frames and always complete at
  the start of the next frame, so runs are reproducible. For an image with device 4
  the feeds (SPEC §content.feeds) are those subscribed in `--config` (see [Feeds](#feeds)):
  with `--relay`, `run` first syncs them until caught up (NETWORK 4.3; a warning and
  the local entries if that fails within 60 s), then keeps following them while the
  machine runs; an entry stored during frame N is announced by a feeds-updated event
  in frame N + 1. A program's requests to subscribe, unsubscribe, publish, set the profile or set a nickname (SPEC §content.feeds)
  cannot be asked about headless: `--consent` says how they are answered (default
  `deny`: status Refused). `run` uses a relay only with `--relay`.
  `--hash` prints the machine's state hash ([STATE.md](STATE.md)) after the last
  frame as 64 hex digits; `--hash-every N` prints `<frame counter> <hash>` after every
  frame that leaves the frame counter at a multiple of N (from reset: after the first
  N frames, 2N frames, …; from `--load-state`, by the counter). `--bench` prints,
  on stderr, the emulation time per frame (p50, p99, max; the machine and the
  headless content service) and how many times faster than real time the run was.
  `--save-state <file>` writes the machine's snapshot (STATE.md 4) after the last
  frame; `--load-state <file>` starts from one instead of from reset (the image is
  `<image|id>` if given, else the one the snapshot names, by ID), so `--frames 0
  --hash` prints the hash it was saved with and a continued run has the hashes of an
  uninterrupted one. Both are for a single machine, not `--shell`.
  `--record <file>` writes every input of every frame, and the state hash after it,
  to a file (STATE.md 5); `run <image> --replay <file> [--frames N] [--dump …]
  [--hash] [--hash-every N] [--bench] [--save-state <file>]` runs that recording
  again from the image file alone, with no network, store, data directory or events
  (it takes no other option), checking every frame's hash: it stops with an error at
  the first frame that differs, and refuses a recording of another image.
  `--profile reading` runs the machine with only its reading profile (SPEC §profiles):
  devices 1, 2, 4, 7 and 10 (full text), and of the content operations only get, subscriptions,
  timeline, names, identity, subscribe and unsubscribe; every other content request
  completes with status 05h (Refused) without asking. `programs/conform-reading` is
  the conformance image for it. `--profile program` runs it with only the program
  profile, devices 1, 2 and 3 (`programs/conform-program` is its conformance image):
  its terminal draws only the basic repertoire, so a machine there shows any other
  character as a placeholder box (SPEC §terminal.text). `--with <slot>` adds a
  device to a profile (`--profile program --with 10`: with full text), and
  `--without <slot>` withholds one more device. An image that requires a device
  the run does not provide is refused; one it lists as optional is absent, and the
  program asks the `device` call (SPEC §system) whether it has it. A snapshot (or a
  recording) keeps the devices it was taken with: one taken with content does not
  restore under `--profile program`.
- **Exit status of `run`.** When the program exits, `terse run` exits with its
  exit code (0–255). When the machine stops with a fault (SPEC §stopping), it prints
  `terse: fault: <kind> at pc <pc>, address <addr>` on stderr and ends itself
  with signal SIGTERM. Every status from 0 to 255 is a possible exit code, so
  this is as distinct as a process can be: fish reports `terminated by signal
  SIGTERM`, a parent that waits for the process sees the signal, and only a
  shell's numeric status (143 in fish and bash) could be mistaken for a program
  that exits with 143.
- `info <image|id>` prints the header fields, the required and the optional
  devices, and the image ID (BLAKE3-256).
- `put <file>` stores a blob (and sends it to the relay) and prints its ID;
  `get <id> [-o <file>]` fetches one; `hash <file>` prints a file's blob ID.
  `info`, `put` and `get` take `--relay ws://...|none`, `--store` and `--config`, and
  find a relay as [Getting started](#getting-started) says when `--relay` is not given.
- `pack --rom <raw.bin> --title <t> --ram-pages <n> [--device <slot:version>]... [--optional <slot:version>]... -o <out.terse>`
  builds an image from a raw ROM (the bytes loaded at `0001_0000h`): `--device`
  lists a device the program requires, `--optional` one it can run without (SPEC §image:
  the top bit of the version byte; required devices first, each in the order given).
- `key new|show`, `feed …`, `profile …` and `find`: feeds, names and search, see
  [Feeds](#feeds); `feed site` writes a feed as a static website, see
  [Any feed as a website](#any-feed-as-a-website).
- `manual build|check [--dir <docs/manual>]`, `manual site --out <dir>` and `manual
  publish`: the manual, see [Manual](#manual) and [The website](#the-website).
- `vectors [--dir <dir>]` writes the core's test vectors (default `vectors/`):
  blob IDs, signed entries valid and invalid, profile documents, a relay session
  and an article corpus with the expected parse, for anyone implementing
  NETWORK.md or ARTICLE.md in any language (`vectors/README` has the formats). The
  checked-in copy is generated by it; `cargo test -p terse-cli --test vectors` checks
  that it is up to date, that a relay answers the session's messages as written, and
  that the reader shows every corpus article as the reference parser
  (`terse-net`'s `article_parse`) does.
- `relay add <ws://host:port>` and `relay remove <url>` change the relay list in
  `<config>/relays` (starting from an older `<config>/relay` file if there is no
  list); `relay set <url>` replaces the list with one URL; `relay list` (or `show`)
  prints it (exit status 1 if it is empty).

An `<id>` is 64 hex digits, looked up in the blob store and then on the relays;
the bytes are verified against the ID. `--store` defaults to
`$XDG_CACHE_HOME/terse/blobs` (else `~/.cache/terse/blobs`). Only `ws://` relays
are supported. `--relay` may be given several times, and every command that takes it
also takes `--no-feed-relays`. The arrival order of entries (the timeline, SPEC §content.feeds) is kept in
`<store>/feeds/arrivals`.

`terse-host [<image|id> | --shell <image|id>] [--builtin <image|id>]... [--demo] [--consent ask|allow|deny] [--no-trails] [--force-phosphor] [--font <family|file>] [--size <W>x<H>] [--page-sans <family|file>] [--page-serif <family|file>] [--page-mono <family|file>] [--relay ws://...|none]... [--no-feed-relays] [--no-prefetch] [--no-program-download] [--store <dir>] [--config <dir>] [--data <dir>] [--parallax off|pointer] [--eye <dx>,<dy>] [--panels perspective|flat|auto]`
opens a window and runs at 60 frames/s of virtual time. With no image it runs the
shell built into the binary (`programs/shell/shell.terse`, see [The shell](#the-shell)),
which starts the built-in programs (reader, sparks, fetch, hello, sketch and holo, also built in, and
each `--builtin` image), all put into the store at start; `--shell <image>` uses
another image as the shell. With an image named it runs that image alone, as a single
machine, with every key (F12 included) going to it. With `--demo` it first sets up the
sample articles ([Getting started](#getting-started)). `--size` is the window's size
in pixels.
The relays are found or started as Getting started says. `--no-prefetch` turns off
fetching the `terse:` links of automatically downloaded articles (NETWORK 4.4),
`--no-program-download` the download of the current versions of the programs of
followed feeds; `--data` is where named records are kept
([Named records](#named-records)). `--consent` answers the
program's requests to subscribe, unsubscribe, publish, set its profile and set a
nickname: `ask` (the default) shows the
host's prompt, `allow` and `deny` answer every one without asking. Keys go to the
host's prompt first, then F12 (the home key, with a shell), then to the machine with
the focus. Ctrl+V and
Shift+Insert paste: the clipboard's text goes to the focused program as text events (SPEC §events),
newlines kept and other control characters dropped, fed in only as fast as the event
queue has room, so none is lost. The screen
(SPEC §screen) is shown at 4:3, so screen units are square: a cell is exactly 1:2 pixels
(11 × 22 at the default font size; the glyphs are rasterised at that size, so
text is crisp at the native size and at whole multiples of it), and the grid is
scaled uniformly, centred and letterboxed in a window of any other shape. With a shell,
the shell's screen fills the window at 4:3 whatever its size, and each machine's
picture is drawn in its rectangle with glyphs rasterised for its size, its own page
layout (laid out for 640 units and scaled) and its own phosphor tube. Input:
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
The 3D vector display is seen from an eye that follows the pointer (parallax; see
[3D vector display](#3d-vector-display)); `--parallax off` keeps it still, at `--eye`.
`--font` draws the grid with another font (see [Fonts](#fonts)); `--page-sans`,
`--page-serif` and `--page-mono` choose the page's three faces (see [Page](#page)).
Every form takes `--renderer gpu|software|auto`: the picture is drawn by the GPU
(wgpu) or by the CPU (see [Software renderer](#software-renderer)); `auto`, the
default, uses the GPU if a hardware graphics adapter is found and the CPU otherwise.
`--threads <n>` sets the software renderer's drawing threads (default one per core);
`--tube xy|p31|amber|vectrex` and `--vignette` choose how the phosphor style is drawn
(see [Phosphor style](#phosphor-style)); `--panels flat`
makes it draw the shell's panels upright instead of in perspective (`auto`, the
default, is perspective; the GPU always draws them in perspective).
`--screenshot <out.png> --frames N [--events <file>] [--size <W>x<H>] [--consent ask|allow|deny] [--demo] [--eye <dx>,<dy>]`
(with no image or `--shell [<image>]`: the shell and its machines) runs
headless like `terse run` (relay only with `--relay`; consent `deny` unless given; with
`ask` a prompt is left unanswered and shown in the picture) (but the page is laid out with this host's fonts, between
frames, so the picture shows the finished layout) and writes a PNG, of the grid's native size or
`W` × `H` pixels with the screen letterboxed in it; in the phosphor style the
afterglow is built up over the last 600 frames, as in the window.
`--selftest <image|id> [--frames N] [--events <file>] [--csv <file>] [--consent allow|deny]` (or
`--selftest --shell [<image>]`, the system) runs N frames
(default 200) in the window with vsync, with only the scripted input (default: the
image's file name with `.events`, if it exists; keyboard, mouse and gamepads are
ignored), and prints timings: see [Conformance image](#conformance-image).
`--bench-render [<image|id> | --shell [<image>]] --frames N [--warmup N] [--size <W>x<H>] [--events <file>] [--renderer gpu|software] [--threads n] [--sweep-eye]`
runs headless like `--screenshot` but with the page's scroll animated as in a window,
draws every frame offscreen and prints the drawing time per frame (p50, p99, max,
mean, frames over 16.7 ms) over the N frames after the warm-up (default 60); a GPU
frame is timed until the GPU has finished it. It opens no window. `--sweep-eye` moves
the eye every frame (round an ellipse 160 units to either side and 80 up and down,
once every 120 frames), as a pointer moving over a window would, so 3D lists are
projected again each frame.

`terse-relay [--listen 127.0.0.1:7878] --store <dir> [--max-store-bytes N] [--feed-entries N] [--peer <ws://host:port>]...`
serves a blob store and feeds over WebSocket (NETWORK.md sections 3, 4.3 and 4.7), one
thread per connection. It keeps the newest `--feed-entries` entries of each feed
(default 1024, at least 256), and always each feed's latest profile, in `<dir>/feeds`,
and listens on loopback unless told otherwise; `--listen 127.0.0.1:0` picks a free port
and prints it. With `--peer` (repeatable) it mirrors other relays (NETWORK.md section
5): it keeps a connection to each, sends them every entry and every blob up to 16,384
bytes it newly stores, follows on them the feeds its clients follow, and asks them for
a blob a client wants that it does not hold. Two relays only need one of them to name
the other. It answers searches (`terse find`) from what it holds, articles in the
order it stored them, newest first, and refuses a connection's fifth search within
a second.

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

- `--config <dir>` holds the secret key (`identity.key`, mode 0600, never printed),
  the subscription list (`subscriptions`), your nicknames for feeds (`nicknames`) and
  the relay list (`relays`). Default
  `$XDG_CONFIG_HOME/terse`, else `~/.config/terse`. A host creates the identity
  itself when a program first publishes.
- Entries are kept with the blobs, in `<store>/feeds/<feed key>/<seq>`, the seq as 10 decimal digits (`0000000003`).

Commands:

- `key new` creates your identity (refuses to replace one) and prints its feed key;
  `key show` prints it again.
- `feed publish <file> --title <t> [--reply-to <blob id>]` puts the article and
  publishes the next entry of your feed; prints `seq N blob <id>`.
- `feed publish --program <image.terse> [--name <n>]` checks the image and publishes
  it as a program (NETWORK.md 4.2) named `<n>` (default: the image's title; the rules of
  a display name); prints `seq N program <name> image <id>`. Publishing another image
  under the same name makes it the current version; programs keep their named records
  from one version to the next (see [Named records](#named-records)).
- `feed programs [<key>]` lists the current version of each program held, of one feed
  or of all.
- `feed subscribe <address>`, `feed unsubscribe <address>`, `feed subscriptions` (each
  feed's key, then its name if known: your nickname, else the name its profile declares,
  then the hints it was subscribed with). An address is a key, optionally `@` and one or
  two relays (`host` or `host:port`, NETWORK.md 4.9), also as a `terse:feed/` link; the
  hints are kept in `<config>/subscription-hints`. `feed sync` follows a feed on its
  hinted relays when your relays hold none of it (public addresses only; not with
  `--no-feed-relays`), and says which feeds no relay held. `feed address` prints your
  own address: your key and up to two public relays from your profile, then your relay
  list.
- `feed nickname <key> [<name>]` sets your own name for a feed, kept only in
  `<config>/nicknames`; without a name it removes it. It is shown instead of the
  declared name.
- `profile set --name <n> [--listed|--unlisted] [--description <file>]` publishes your
  profile (NETWORK.md section 4.6): a display name (1 to 48 bytes, no control
  characters, no space at either end), whether search may find it (default: as your
  current profile says, else not), a description (default: the current one), and a
  `relay` line for each of your relays (the `--relay` options, else your list) that is
  not a loopback address. `profile show [<key>]` prints a profile held locally (yours
  by default). Names are self-declared and not unique: every listing shows the key
  beside the name.
- `find <text>` asks your relays for listed feeds whose name contains the text
  (ASCII letters match either case; names starting with it first) and prints key
  and name; `find --entries <text>` finds articles and documents of listed feeds by title,
  `find --programs <text>` programs by name (current versions only).
- `feed sync [--watch] [--auto-download <bytes>] [--no-prefetch] [--no-program-download]` connects to the relay, receives
  new entries of subscribed feeds, verifies and stores them, and downloads
  articles up to 16,384 bytes (or `--auto-download`), and with them the first
  eight `terse:` links in each (not with `--no-prefetch`; NETWORK 4.4). Once a feed
  is caught up it also downloads the current version of each of its programs, whatever
  their size (up to the largest image), not older versions (not with
  `--no-program-download`); a line ending `(downloaded)` reports each. One line per new entry:
  time (UTC, as claimed by the author), feed key prefix, seq, size, article
  state, title. The state is `local` (in the store), `remote` (over the limit;
  `feed read` fetches it), `failed` or `size-mismatch` (the blob is not the size
  the entry claims, so it is not used). Without `--watch` it exits once caught
  up: each feed's subscribe reply gives the relay's highest `seq`, and the feed is
  caught up once that entry has arrived (NETWORK 4.3); then downloads done. It fails
  after 60 s.
  With `--watch` it stays connected and prints entries as the relay pushes them,
  reconnecting (1 s backoff, doubling to 30 s) if the connection drops.
- `feed list [<key>]` lists entries held locally, newest first, with the feed's name
  after the key prefix when known; a profile entry shows as `[profile: <name>]`, a
  program entry as `[program: <name>]`, a document entry as `[document: <name>]`.
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
                         3 and the one display-list decoder, with the fill cap), vector3.rs
                         (device 9: its decoder and the one projection every renderer
                         draws), content.rs,
                         page.rs (device 7: the document decoder, the reference layout, when
                         to report the height), sound.rs (device 6: the synthesizer, integer
                         and bit-exact), machines.rs (device 8); shell.rs: the
                         System (the shell and its machines: frame order, latching, input
                         routing), driven by a host's Driver.
                         wide.rs holds the Unicode 16.0 East Asian Width W/F table. No GUI,
                         threads, clock, I/O or OS randomness; inputs (events, time, random
                         bytes, controllers, content completions) are passed in.
    crates/terse-net/    host side of device 4 and feeds: blob store, identities, entries,
                         feed store, relay protocol, relay client (behind a small
                         Transport trait), node.rs (several relays, feed relays, search),
                         profile.rs (display names, profile documents, nicknames), content
                         service thread (with the consent policy and the queue of requests
                         waiting for an answer; requests tagged by machine; HeadlessSystem),
                         local.rs (finding relays: the config file, the local port),
                         builtin.rs (the shell and built-in programs every host carries)
    crates/terse-relay/  `terse-relay`: the WebSocket blob and feed relay, with peers and
                         search (a library too: `terse-host` runs one in-process when it
                         finds none)
    crates/terse-cli/    `terse`: headless runner, image info/pack, blobs, feeds, perf.rs
                         (`terse perf`, the scenarios of PERFORMANCE.md); the
                         program tests (tests/hello.rs, fetch.rs, reader.rs, sparks.rs,
                         sketch.rs, holo.rs, shell.rs, conform.rs)
    crates/terse-host/   `terse-host`: winit window, drawn by the GPU or the CPU behind
                         display.rs's Display trait; renderer.rs is the wgpu renderer of
                         one screen (terminal, vectors.rs's shapes, the phosphor tube:
                         SPEC §vector.layers, §vector.phosphor, and the host's overlay); screens.rs draws a
                         system's screens together (SPEC §machines.screens); soft/ is the software
                         renderer of the same picture (see Software renderer); page.rs
                         lays the page out with cosmic-text on a worker thread and draws
                         it; consent.rs the consent prompt; setup.rs the relay choice and
                         --demo, whose articles are in demo/
    programs/lib/        what a program needs: crt0.S, terse.ld (the memory map), terse.h
                         and terse.c (the calls, display-list builders, content requests)
    programs/hello/      demo program in assembly: hello.S, build.sh, the built hello.terse
                         and the golden screen used by the tests
    programs/fetch/      content device demo in C: gets message.txt by its ID and prints it;
                         build.sh writes message_id.h, then fetch.terse
    programs/reader/     the feed reader, in C: reader.c, build.sh, the built reader.terse
                         and the golden screens in expected/
    programs/shell/      the shell (device 8), in C: shell.c, build.sh, the built
                         shell.terse and the golden screens in expected/
    programs/sparks/     a game for the vector display, in C: sparks.c, build.sh, the
                         built sparks.terse, recorded sessions in sessions/ and the golden
                         screens in expected/
    programs/sketch/     a drawing program for the vector display and documents, in C
    programs/holo/       scenes on the 3D vector display (device 9), in C: holo.c,
                         build.sh, the built holo.terse and the golden screens in expected/
    programs/tune/       a short piece for the sound device (device 6), in C: tune.c,
                         build.sh, tune.terse and its golden samples, expected-audio.txt
    programs/conform/    the conformance and responsiveness image (C and assembly),
                         build.sh, conform.terse, its input conform.events, and
                         expected-hashes.txt and expected-screen.txt
    programs/conform-reading/, programs/conform-program/
                         the same source built for the reading and the program
                         profile, each with its build.sh, events and expected files
    programs/conform-perf/
                         the machine host's responsiveness checks (PERFORMANCE.md):
                         conform-perf.c, build.sh and its expected files

## Writing programs

A program is RV32IMC machine code in ROM from `0001_0000h`, with RAM from
`0010_0000h` (SPEC §memory). It starts at the first ROM byte with `sp` at the end of
RAM and all RAM zero, and reaches the devices with `ecall` (SPEC §calls): call number
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
  block, get, the feed operations), the page (`page_submit`, `page_region`,
  `page_scroll`, `page_background`, `page_clear`, and a document builder:
  `doc_begin(buf, cap)`, `doc_text`, `doc_font`, `doc_size`, `doc_colour`,
  `doc_paragraph`, `doc_rule`, `doc_anchor`, `doc_end`, which never writes past
  the capacity and leaves a clean prefix when it fills, setting `doc_overflow`),
  `terse_call4` for calls with four arguments, and `memcpy`, `memset`,
  `memmove`, `memcmp`.
  `term_write` repeats the call until every byte is written, since a write stops
  when the frame's budget runs out (SPEC §terminal.calls).

The build (see `programs/reader/build.sh`, which takes `CLANG`, `LLD`, `OBJCOPY`,
`TERSE` and `CARGO` from the environment):

    set flags --target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin -ffunction-sections -fdata-sections
    clang $flags -I programs/lib -c programs/lib/crt0.S -o 0.o
    clang $flags -I programs/lib -c prog.c -o 1.o
    clang $flags -I programs/lib -c programs/lib/terse.c -o 2.o
    ld.lld -T programs/lib/terse.ld --gc-sections 0.o 1.o 2.o -o prog.elf
    llvm-objcopy -O binary prog.elf prog.bin
    target/debug/terse pack --rom prog.bin --title prog --ram-pages 4 --device 2:0 -o prog.terse

`--device` lists a device the program requires: a host without it refuses the
image. A device the program can do without is listed with `--optional` instead
(`--optional 4:0`, `--optional 6:0`): the program runs on hosts that lack it, and
asks at start with `terse_has_device(4)` (the `device` call, SPEC §system) whether it
has it. A program meant to run everywhere requires only devices 2 and 3, the
program profile (SPEC §profiles).

**Text.** Every host's terminal draws the basic repertoire, the same 361
characters as the basic text of articles (ARTICLE.md 1.1, `vectors/basic-text.txt`):
printable ASCII, the Latin-1 Supplement (U+00A0–U+00FF), – — ‘ ’ “ ” • … −,
box drawing and block elements (U+2500–U+259F) and U+FFFD. A program that
prints anything else lists device 10, full
text (SPEC §terminal.text): `--optional 10:0` if a placeholder box will do on
a host without it (`terse_has_full_text()` tells it which it got), `--device
10:0` if it cannot do without. A machine that does not list it is shown with
the basic repertoire on every host, the reference host included, so what you
see is what every host shows. Of the bundled programs the reader requires full
text (titles and names come in any script); hello (other scripts), fetch (the
message it fetches), sparks and the shell list it as optional; the rest print
only basic text. Sparks and the shell draw basic stand-ins for their own
symbols without it (sparks' hearts as `<3`; the shell's ⌂ ‖ ■ ✗ as ~ ║ ▄ ×),
so they look whole on every host; the shell still shows titles in any script.

`crt0.S` must come first. There is no C library beyond `terse.c` and no
compiler runtime: avoid 64-bit division and floating point, or bring the routines
(`crates/terse-core/tests/softfloat/softfloat.c` is a tested IEEE double add,
subtract, multiply and divide in integer C). The build is reproducible: the same
sources and the same clang give the same image, and each program's test fails if
its checked-in `.terse` is not what its sources build to (and is skipped when
clang lacks the riscv32 target).

Costs to keep in mind (SPEC §charges): an instruction from ROM costs 1 unit, one from
RAM 4; a call costs 4 on top of its `ecall` (so 5 in all from ROM, 8 from RAM) plus 1 per
4 bytes it copies, or 64 if it is answered (its result depends on the device's state:
`get_cursor`, `get_colours`, `cells_read`, content `submit`, page `locate`); `write`
costs 2 per byte and takes at most 4,096, and a scroll or clear command 150. Writing a whole row with one `write`
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
    10 home                                     the home key (with --shell only)
    10 pad <n> <buttons> <lx> <ly> [<rx> <ry>]  controller n (0-3) from this frame on
    10 pad <n> off                              controller n disconnected

`<key>` is a USB HID usage ID (`40` or `0x28`) or a name: `a`-`z`, `0`-`9`,
`f1`-`f12`, `enter`, `esc`, `backspace`, `tab`, `space`, `up`, `down`, `left`,
`right`, `home`, `end`, `pageup`, `pagedown`, `insert`, `delete`. The events of a
frame are delivered in the order written. A cell (column, row) starts at screen
unit (column × 8, row × 16). With `--shell`, pointer positions are in units of the
whole screen (the shell's), and are mapped into a machine's units when they fall on
its picture.

A `pad` line connects controller `n` and sets its whole state (SPEC §controllers) until the
next `pad` line for it; controllers are disconnected until their first one.
`<buttons>` is hexadecimal, byte 1 in bits 0-7 and byte 2 in bits 8-15 (`1` up,
`2` down, `4` left, `8` right, `10` A, `20` B, `2000` Start, `14` A and left); the
sticks are -127 to 127, x then y, and the right stick defaults to `0 0`.

## Writing an article

An article is UTF-8 text that reads well as it is (`ARTICLE.md` is exact). Blank
lines separate blocks; the markers count only at the start of a line:

- a paragraph: lines joined with spaces and wrapped by the reader; bare
  `terse:<article ID>` and `terse:feed/<key>` are links, and so is
  `[text](target)` with a `terse:` link as the target, shown as its text (on one
  line, the text 1–200 bytes without brackets). Articles do not link to the web:
  `http://` and `https://` addresses are ordinary text, bare or as the target of
  `[text](target)` (which is then not a link and is shown as written);
- `# ` a heading, `## ` a subheading (one line each);
- `- ` or `* ` a list item (one level; the lines after it continue it);
- four spaces or a tab: preformatted lines, shown as written;
- a line of three backquotes, then lines up to the next such line: a fence,
  shown as written (anything after the opening backquotes is ignored, except
  `path`);
- `` ```path W H `` (W × H screen units, 1–1024): a figure, drawn from the path
  string in it, up to the closing backquotes;
- lines that start with `|`: a table, cells separated by `|` (`\|` for a bar in
  a cell), at most 8 columns; a second line such as `|---|:-:|--:|` makes the
  first row a header and sets each column's alignment (start, centre, end);
- inline, a run between two backquotes on one line (`` `like this` ``) is code,
  in a monospaced face; there is no other inline markup.

    | Key | What it does |
    |-----|--------------|
    | `M` | the manual   |

The path string is SVG path data (`M L H V C S Q T Z`, lowercase relative, numbers
with an optional sign and fraction, no exponents, coordinates in screen units within
±2047) plus two words that set how what follows is drawn: `fill <colour>` and
`stroke <colour> [<width>]`, where a colour is `#rgb`, `#rrggbb`, `#rrggbbaa`,
`none` or `fg` (the text colour). Until the first word: no fill, stroke `fg`, width
1. Arcs (`A`) are not supported yet; a figure with one, or with any other error, is
shown as its text. For example, a filled triangle with a light outline on a dark
panel:

    ```path 160 90
    fill #20304a stroke none M0 0 H160 V90 H0 Z
    fill #e06c75 stroke fg 2 M30 75 L80 15 130 75 Z
    ```

The reader shows a table as a grid laid out by the host (the page's Table
command, SPEC §page.document and §page.layout: columns as wide as their widest cell, the widest
narrowed and wrapped when the table is wider than the column); a reader that does
not know tables shows its lines as a paragraph.

`crates/terse-host/demo/pictures.txt` (the "Pictures in articles" sample) has a
chart, a badge and a diagram. A reader that does not draw figures shows their text,
so an article still reads as plain text.

## Manual

`docs/manual` is the Terse manual: the articles listed in `docs/manual/manifest`, written in the article format,
that describe the whole system (the machine, every device call, event, record and
limit, the network, the tools, the shell and the reader). Every host carries it:
`terse-host` and `terse run` put its articles into the store at start (for an image with the content device, or a shell), and M on
the reader's timeline, or the shell launcher's "Manual" entry, opens its index, from
which Tab and Enter follow its links.

Articles link to each other with placeholders, `[the page](terse:{device-page})`,
which `terse manual build` replaces with the blob IDs, in dependency order (the
links must form no cycle: an article links only to those after it in
`docs/manual/manifest`); the result is checked in as `docs/manual/build`, which
`terse-net` embeds, and `programs/reader/build.sh` compiles the index's ID into
the reader. After editing the manual:

    cargo run -p terse-cli --bin terse -- manual build
    programs/reader/build.sh

`terse manual check` tells whether the build is up to date. `terse manual publish`
(with the usual `--relay`, `--store` and `--config`) publishes the manual as article
entries of your own feed, each with its title, the index last, so that others can
read it. `cargo test -p terse-cli --test manual` checks the build, that every link
resolves to a blob every host carries, that the manual names every call, event,
content operation and status of the machine specification (SPEC.md and spec/), and publishing to a relay;
`docs/manual/README` has the rules for writing it.

### The website

`site/` is the manual as a static website, so that it can be read before
installing Terse: one HTML page per article (the overview, "What Terse is", is
`index.html`; the manual's index is `contents.html`), made from the same sources by
the reference article parser, with figures as inline SVG. It is plain static files:
no JavaScript, nothing loaded from anywhere else (no fonts, no images, no
analytics), one small `style.css`, light and dark following the system. Any file
host can serve the directory as it is, and it reads straight from disk too. Links
between articles of the manual lead to their pages; other `terse:` links (feeds,
programs, articles outside the manual) cannot be followed on the web and are shown
as their address. After changing the manual, regenerate it (the output is
deterministic, and it is checked in):

    cargo run -p terse-cli --bin terse -- manual site --out site

`cargo test -p terse-cli --test manual` checks that `site/` is up to date, that
every link resolves to a page of it, and that no page holds a script or a web
address.

#### Any feed as a website

The same renderer makes a website of any feed, so that an author's readers need
install nothing:

    terse feed site [<address>] --out <dir> [--title <t>] [--base-url https://...] [--force]

(default: your own feed; with the usual `--relay`, `--store`, `--config`). It first
fetches the feed's entries from the relays (and the address's hints), then writes:
a page per article entry (`<first 16 hex digits of the blob ID>.html`, the whole
ID if that name is taken), dated by the entry's time; `index.html` with the newest
20 and links to `page-1.html`, `page-2.html`, ... (20 articles each, numbered
from the oldest, shown newest first); `programs.html` listing the feed's programs
by `terse:program/` address (they do not run on the web); the manual's
`style.css` with a few rules more; and with `--base-url` (the site's own address)
`feed.xml`, an Atom feed of the newest 20 articles with their whole HTML, figures
inline. The profile gives the title (unless `--title`) and the description. Every
page ends with the feed's address, `terse:feed/<key>[@hints]`, for following it
in a Terse reader. No scripts, nothing loaded from elsewhere, and no web address
except the site's own (and the Atom namespace name) in `feed.xml`.

It is incremental: `.terse-site`, a state file in the output directory, records
each article's entry, page, the articles its links lead to and the hash of the
inputs of its page, and each other file's hash. A run writes only the files whose
inputs changed and reads only the blobs of the pages it writes: one new article
writes its page, the last index page, `index.html` and `feed.xml`, and reads one
blob. No page lists other articles or links to its neighbours. A link to another
article of the feed leads to its page if the site has it (whenever it was
published), else it is shown as its address; an old article that links to one
published later is written again once, when that one arrives. Another `--title`,
`--base-url` or feed address, or a new generator version, rewrites every file (it
says so), as does `--force`. Nothing is ever removed: the network has no way to
withdraw an entry, and an article relays no longer keep stays on the site. The
output depends only on its inputs (no run times). `cargo test -p terse-cli --test
feed_site` checks all this.

## Named records

A program can keep small data on the machine it runs on: named records (SPEC §content.records),
each up to 64 KiB, at most 256 records and 1 MiB per program. They belong to the
program's identity: the feed and name it was published under (or, for a built-in, its
title), so a new version of a program reads the old version's records; a program run
from a file or started by image ID has the records of that exact image. They never
leave the machine and no other program can read them. Hosts keep them in
`<data>/records` (`--data`, default `$XDG_DATA_HOME/terse`, else
`~/.local/share/terse`); `terse run` and `terse-host` take `--data`. Sparks keeps its
high score there, the shell its layout and its session.

The shell's snapshots (SPEC §machines.snapshots, below) are kept beside them, in
`<data>/snapshots/<owner>/00` … `15` (the owner as for records): private, never sent
anywhere, one file per slot in the format of STATE.md 4, each written to a temporary
file, synced and renamed.

- `records list [<owner>] [--data <dir>]`: every owner with its count and bytes, or one
  owner's records (size, name). An owner is `builtin:<title>`,
  `program:<feed key>:<name>` or `image:<image id>`.
- `records show <owner> <name>` writes the record's bytes to stdout; `records delete
  <owner> <name>` deletes it.

## The shell

`programs/shell` is the program `terse-host` runs when no image is named: a small C
program (19 KB of ROM, 3 RAM pages) with device 8 (SPEC §machines), which starts the host's
built-in programs as machines of their own (at most 8) and shows them as tiles.
Everything about the layout is the shell's; the host only runs the machines, draws
each one's screen where the shell placed it, and routes input.

- **The bar** (row 0): the home segment `⌂`, then one segment per machine: its handle,
  a mark (`…` loading, `‖` paused, `■` exited, `!` faulted, `✗` failed), its title and
  the units it used in its last frame as a share of the budget; the machine with the
  keyboard in blue, the one selected in the launcher in amber; the clock (UTC) at the
  right. A click on a segment gives that machine the keyboard (in the gallery it
  turns the ring to it instead, back in the launcher); one on `⌂` opens the launcher.
- **Tiles**: below the bar, each machine's picture (its whole screen, scaled) at the
  top of its tile, a blue frame around the one with the keyboard. Layouts: master and
  stack (the first machine on the left half, the others stacked on the right; the
  default), grid, zoom (one machine over the whole area, the others hidden but
  still running), and the gallery.
- **The gallery**: every machine's screen is a panel (a Screen on device 9, SPEC §vector3d.list)
  on a ring behind the glass, in a wireframe monitor case on a stand over a faint
  ground grid, the selected machine at the front on the glass and the others turned
  half way towards the viewer. Left and Right (and the bar's segments) turn the ring
  to the next machine, 20 frames eased; the shell only changes the ring's angles.
  Giving a machine the keyboard (Enter, Esc, a click on its panel, starting a
  program, which joins the ring) `face`s it (SPEC §machines.calls): over 20 frames the host
  moves the whole scene until its panel lies flat over the area below the bar, then
  shows it there as a flat picture, as in zoom; F12 `unface`s it, back to the ring.
  With the launcher open the ring stands right of it.
- **The launcher** (on the page, while the shell has the keyboard: at start, and after
  F12, the home key): "Manual" (Enter opens the reader on the manual's index, see
  [Manual](#manual)), the built-in programs by title, then "From your feeds": the
  current version of each program of the feeds you follow, with its publisher's name
  and short key and `↓` while its image is not yet downloaded (read again on every
  feeds-updated event), then the machines with their state (a faulted one with the
  fault kind and `pc`, a failed one with the reason: not found, not a valid image,
  needs a device this host does not have, no room). Programs are started by their
  identity (SPEC §machines.calls), built-ins too, so their named records follow them. The tiles move to the right of
  it. Up/Down move the cursor; Enter starts the program under it (the new machine gets
  the keyboard) or gives the keyboard to the machine under it; Esc gives it back to
  the selected machine; Tab or Right / Shift+Tab or Left select the next / previous
  machine; X stops the selected machine; P pauses or resumes it; Z zooms it (Z again
  goes back); L cycles the layouts; 1–8 select a machine by handle. A click on a
  program starts it, one on a machine selects and focuses it. The layout chosen with
  L and Z is kept in the shell's named record `layout` for the next run.
- **Sessions** (SPEC §machines.snapshots): S in the launcher saves the session: every running or
  paused machine as a snapshot (its whole state; slots 0, 1, … in handle order) and
  the named record `session` (layout, which machine was selected, how many), with
  "Session saved" or "Could not save the session" in the bar. When the
  shell starts and a session was saved, the launcher's first entry is "Restore last
  session (N programs)": Enter restores the machines in their order, where they
  were (sparks mid-game, the reader on its article at the same place, a drawing), with
  the layout and the keyboard on the machine that was selected; D, with the cursor on
  that entry (and only there), discards the session.
  A machine that cannot be restored (its image is gone from the store and no relay has
  it, or the snapshot is unreadable) shows why, like a failed start. A snapshot holds
  one exact version of a program; named records are not in it. Nothing is saved
  without S yet.
- A click on a machine's picture gives it the keyboard (the click also reaches the
  machine). A machine that exits or faults keeps its tile, with its last picture (a
  fault shown on its bottom row), until X stops it; the keyboard goes back to the
  shell as soon as it exits or faults.
- **Open requests** (SPEC §system `open`): a machine may ask the shell to open a program
  (by its identity) or a blob (by its ID); the reader does this when you follow a
  link to a program, an image or a document. The shell takes them one at a time, only
  from the machine that has the keyboard, at most one a second from each, and drops a
  machine's requests for ten seconds after you say no. A built-in program starts at
  once in a new tile. Any other program is fetched first (the host looks it up on its
  relays if it holds no entry for it), then the bar asks
  `reader asks to start Tool by Alice 3fa9c2…c21e07. Y/N` (the keyboard goes to the
  shell until you answer): Y starts it in a new tile with the keyboard, N or Esc does
  not. For a blob the shell reads its first bytes: an image (`TERS`) is asked about as
  an unpublished program (its title and short ID); a document (`TDOC`, SPEC §content.documents) starts
  its format's program with the document as its argument (a published format is asked
  about the first time in a run); anything else opens in a new reader tile. A failure
  (not found, no free handle) is a brief message in the bar.

## The reader

`programs/reader` is a feed reader written in C, the first of the built-in programs
the shell offers. Every view is typeset on the [page](#page) in the sans face;
the terminal shows only the status row (row 29: key hints, the position such as
`2/3`, or the outcome of the last action) and, while a feed key is typed, row 28.

- **Timeline** (newest first): one entry per paragraph, the title, then a dimmer line
  with the feed (name and short key), the date and time (UTC), the
  size, and `↳ reply` or `not downloaded` when they apply. The selected entry is in
  amber. Up/Down, PgUp/PgDn, Home/End and the wheel (3 entries a step) move the
  selection; Enter, or a click on an entry, opens it; / search; F feeds; ? or F1
  help; T publishes a test article; Esc quits. With no entries, a page says so and
  which keys to press. New entries appear without a key press.
- **Feeds** (F): the subscribed feeds, each with its name and short key (`you` marks
  your own, `your nickname` a name you gave it), the highest entry number and how
  many entries are held. Enter opens the feed's page. N types a nickname for the
  selected feed, which only you see (empty removes it; content op 0Ch); P types your
  public name, then asks on the status row whether search may find it (Y/N), and
  publishes your profile (op 0Bh). Both names are checked first (1 to 48 bytes, no
  control characters, no space at either end) and the host asks you before either
  is set. S opens the address entry on
  row 28: type or paste a feed key, an address (`key@relay`) or a `terse:feed/` link
  (whitespace is ignored, so the grouped form from the help page pastes as is); Enter
  subscribes with the address's hints (the host asks you, naming the relays), Esc
  cancels, Backspace deletes; a malformed key or address is reported on the status
  row. Feeds followed through a relay their address names are marked `@`. U unsubscribes
  from the selected feed. Esc, Backspace or Left go back.
- **Search** (/ from the timeline or feeds): type words and press Enter. The
  reader searches two places at once and lists both, under a heading each:
  - **In your articles**: the articles on this host, which the reader searches
    itself, newest first, by title and by text, in bounded memory: it walks the
    timeline (op 04h, 8 KiB of records at a time), tests each title, then reads
    the first 48 KiB of each article in the host's store with ranged gets of up
    to 16 KiB (several at once, leaving two of the eight request slots free),
    scans each chunk as it arrives and drops it, carrying the last bytes of a
    chunk to the next so that a match across the boundary is found. Articles
    not downloaded are searched by title only (nothing is fetched for a
    search) and counted as skipped. Results appear as they are found: the
    title, then smaller the feed and date, and a line of the raw text around
    the first match (line breaks as spaces, the match in colour, cut between
    characters), under a progress line, `searched 340 of 1,204 articles ·
    12 skipped (not downloaded)`; at most 64 (`more than 64 matches; refine
    the search`). Typing, opening a result or leaving the view stops it at
    once. Enter on a result whose text matched opens the article scrolled to
    the match (an anchor on it, found with the page's `locate`), in colour.
    It uses only operations of the reading profile (timeline and get), so it
    works on `terse run --profile reading` too.
  - **On your relays**: listed feeds by name and their articles by title
    (op 0Ah, twice): the feeds (name, your nickname for it if you gave one,
    also to a feed you do not follow; short key, `you`, `subscribed`) and the
    articles (title; feed, date, size), or why the relays could not be
    searched (no relay could be reached; the relays refused, which a relay
    does when one connection searches more than 4 times a second, and which a
    reading-profile host always does).

  Both match as NETWORK.md 4.7 says: the query's bytes in a row, A–Z and
  a–z alike, every other byte only itself (`köln` finds `Köln`, `KÖLN` does
  not). Up/Down, the wheel or a click select (results of this host arriving
  above the selection do not move it); Enter opens a feed's page or an
  article; Esc goes back. In the reference fixture the relays' results are
  on the page 2 frames after Enter (the frame of the key counted as 1) with a
  relay on this machine; for this host's articles see the measurements below.
- **Feed page** (Enter on a feed, or a `terse:feed/<key>` link): the feed's name
  (and whether it is your nickname or the feed's own), its key in full and whether
  you follow it; S subscribes, U unsubscribes, Esc goes back.
- **The manual** (M on the timeline, also named on the empty timeline's page and
  in help): opens the index of [the manual](#manual), which every host carries;
  Esc goes back to the timeline.
- **Help** (? or F1): the keys of every view and your own feed address in the
  monospaced face: the key in groups of eight hex digits, then `@` and the hints
  the host gives (op 08h with a buffer), or a note that without a public relay
  only people on the same relay find your feed. T publishes a test article
  ("Test article N", a few paragraphs with the frame and time in them): the reader
  puts it (op 02h), publishes it (op 07h), and if you do not yet follow your own
  feed, subscribes to it (op 05h) so that the article shows in the timeline; each
  step's outcome (published, refused, failed) appears on the status row. T also
  works from the timeline and feeds views.

The timeline and feeds lists are virtual: the page holds a heading and only the
entries from the top of the visible window on (at most 24), every one laid out the
same way (one title line of 16 units and one detail line of 12, neither wrapped, and
cut with an ellipsis when too long), so
the height of the whole document, which the page's height event reports, divided by
the number of entries plus one gives the height of an entry with the host's fonts.
From that the reader knows how many entries are fully visible and scrolls the window
to keep the selection in it; each move submits a new, small document.

**The article view.** The status bar (row 29) stays on the terminal; the page fills
the rows above it, as a centred column 512 units wide (about 68 characters of 15-unit
body text in the reference layout, a comfortable measure), on an opaque dark
background. It shows the title (26 units, bold), a grey line
(feed key prefix, entry number, date, size, and `↳ reply` for a reply), a rule, then
the article, formatted as [Writing an article](#writing-an-article) says:
paragraphs wrapped, headings in bold (21 and 17 units), list items with a bullet,
preformatted blocks and fences in monospace without wrapping, figures centred (one
wider than the column scaled down to its 512 units), tables in 14-unit text (the
header bold, under a rule), code spans in the monospaced face a unit smaller, in
green. An article that has no entry (one opened by a link, or the manual) and
whose first line is a heading (`# `) is titled by that heading, which is then
not shown twice; so is one whose entry has that title. Links (only `terse:` forms;
web addresses are ordinary text) get an anchor and a link colour: `terse:<id>` links
(an article, by its blob ID) and `terse:feed/<key>` links (a feed), shown shortened
(`terse:360793…dec189`), and links with text (`[text](target)`), shown as their text: clicking one, or making it
current with Tab or Shift+Tab (it turns amber, the status row shows its target and
the page scrolls just enough to show its line) and pressing Enter, opens the article
or the feed's page. After you scroll, Tab starts again from the first link at the
top of the view (Shift+Tab from the last one above its bottom): the reader finds it
by asking the page where links are (`locate`, SPEC §page.calls), a binary search of one
question a frame. The article opened by a link has the title of the
timeline's entry for that blob if your host has one, else its ID's short form; one
that is missing, larger than the 48 KiB the reader shows, or cannot be fetched gets
a page saying so. Esc goes back through the articles followed (up to 8, each at the
place it was scrolled to), then to where the first was opened from. On opening an article the reader also gets its first 8 article links, one byte
each, at most two at a time, so the host holds them by the time you follow one (the
host itself does the same for articles of subscribed feeds as they arrive,
NETWORK 4.4). Up/Down scroll by 20 units, PageUp/PageDown/Space by the rectangle less one
line, the wheel by 60 a step, Home/End to the ends; the reader clamps its scroll
value with the height from the page's height event and shows in the status bar how
far it is scrolled, 0% at the top and 100% at the bottom (nothing until the first
height arrives, and nothing when the article fits; nothing waits for it).

The document is built over several frames (at most about 80,000 units of work a
frame) and submitted after the first frame's share, then whole when it is complete,
so a long article appears at once and its percentage follows. It holds one
timeline request's worth of entries (16 KiB of records: 70 to 147 entries) and
requests another range as you scroll, and up to 48 KiB of an article (read in 16
KiB gets). An article opened from the timeline, from search, from the manual or
passed by the shell that is longer, or one whose formatting would pass the 64 KiB
document, ends with a note saying how much is shown; one opened by a link that is
longer than 48 KiB is not shown at all, and a page says so (above). Rebuild it with
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
    $T run programs/reader/reader.terse $R --events /tmp/terse-demo/keys.txt --frames 30 --dump --dump-page

    # Publish while terse-host runs: the entry appears at the top.
    $T feed publish /tmp/terse-demo/a.txt --title "Live" $A

Measured with the golden-test fixture (`cargo test --release -p terse-cli --test
reader -- --nocapture`), headless with the reference layout: a Down key submits the
document with the new selection in the same frame and its height arrives in the
next; the list is drawn 3 frames after reset; an already-local article is submitted
2 frames after Enter (a stand-in page with its title in the Enter frame) and its
height arrives in the next; the whole height of a 48 KiB article arrives 16 frames
after Enter. The busiest frame uses 76,183 units (61% of 125,000, building an
article's document; 77,383 (62%) for a 48 KiB article of nothing but tables);
a typical frame 183; no frame runs out of budget.
ROM 60,000 bytes, 42 RAM pages (the 64 KiB document, the 48 KiB article, the
timeline records, the names, the search results, a table's 1,024 cells, and about
100 KiB for searching this host's articles: a 64 KiB ring of chunks, an 8 KiB
timeline page, 16 entries in hand and 64 results). Following a `terse:` link to
a local article: the article is on the page 2 frames after Enter, busiest frame
41,635 units; a relay search: busiest frame 24,490 units.

Searching this host's articles (`cargo test --release -p terse-cli --test reader
local_search -- --include-ignored --nocapture`; generated libraries of prose, one
word in every 97th article, headless, frames of 1/60 s):

| Library | Query | First result | Done | Text per frame | Busiest frame |
|---|---|---|---|---|---|
| 1,000 × 3 KB | in 10 articles | 17 frames (0.28 s) | 174 frames (2.9 s) | 17.3 KB | 62,737 (50%) |
| 1,000 × 3 KB | in none | — | 173 frames (2.9 s) | 17.4 KB | 49,073 (39%) |
| 5,000 × 3 KB | in 51 articles | 4 frames (0.07 s) | 851 frames (14.2 s) | 17.6 KB | 63,554 (51%) |
| 5,000 × 3 KB | in none | — | 850 frames (14.2 s) | 17.7 KB | 50,100 (40%) |
| 100 × 3 KB, 6 of them 48 KiB | in 1 article | 17 frames (0.28 s) | 29 frames (0.5 s) | 19.9 KB (41.8 KB at most) | 61,423 (49%) |

Small articles go at about six a frame: the search keeps six requests pending
(the content device allows eight), so it is bound by requests, not by scanning.
The scan (Horspool over case-folded bytes, 7 instructions a window) makes 0.13 to
0.22 windows a byte for these queries, about 1 to 2 instructions a byte; with the
gets, the delivery charge and the list, 2.0 to 2.9 units a byte in all. A key
during a search is handled in its own frame (Down submits the new list in the frame
of the key; typed text is on row 28 in that frame), and no frame runs out of budget.

In the window (release build, `TERSE_PAGE_TRACE=1 terse-host --selftest` with 28
entries, 13 Down then 13 Up presses 8 frames apart, the list window scrolling): every
list document was laid out in 0.6–0.7 ms and its height queued at the start of the
frame after the key's frame, so the new highlight is on screen in the next presented
frame, one frame (about 17 ms at 60 Hz) after the key; 290 of 290 frames presented.

In the window (`TERSE_PAGE_TRACE=1 terse-host --selftest …` with a script that
presses Enter on the 2.6 KB fixture article in frame 12, release build): submitted
in frame 13, laid out on the worker in 33 ms (the first layout of the run, with
cold shaping caches; 7 ms for the next article), and shown with its height event
in frame 16, four frames after Enter; a later article showed two frames after
Enter. `cargo test --release -p terse-host layout_time -- --ignored --nocapture`
times the layout of reader-like documents with warm caches: 3.6 ms for 6 KB, 26 ms
for 48 KiB, 34 ms for 64 KiB. Drawing the page while scrolling every other frame
added about 0.3 ms per presented frame (render-submit p50 1.26 ms against 0.98 ms
with the page still); the first frame that brings a new script into view (CJK, at
first use of its font) took up to 23 ms once, rasterising its glyphs.

Figures: `cargo test --release -p terse-host figure_raster_time -- --ignored
--nocapture` rasterises hostile 64 KiB documents, each figure as costly as the caps
of SPEC §page.document allow in one way (full-height curves, 5,000 tiny cubics, 256 small
paths, strokes 4,096 units wide, narrow full-height strips, many figures). One
figure takes at most about 11 ms at 2 pixels per unit and 15 ms at 3 (outlines up
to 14 ms); the figures of the first screen and its margin, the most a scroll asks
for at once, 30 ms at 2 pixels per unit (six 640 × 128 figures of 256 paths each),
all on the figure thread; layout of these documents takes under 0.5 ms. Before the
caps, one figure of 5,000 big cubics took 0.5–0.9 s. In the window (`--selftest`,
880 × 660, a test program scrolling 8 units a frame): render-submit p50 0.42 ms,
p99 1.3 ms, max 2.6 ms through those six figures; through 2,340 figures 2 units
high (some 480 in view, each its own texture and draw call), p50 1.2 ms, p99 3.7 ms,
max 8.7 ms, 3 MiB of figure textures; no frame dropped in either.

## Vector display

Device 3 (SPEC §vector) draws a display list of lines, circle outlines, discs,
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
draw or print from these. **Fill cap** (SPEC §vector.fill-cap): the filled shapes of one list
may cover at most four screens of area (each counted by its bounding box, clipped
to the screen); the fill that would go past that, and every fill after it, is not
drawn, while lines and circles go on. **Fill mode** (`vec_fill(VEC_FILL_OFF)`)
draws every filled shape as its outline in the clean style.

`terse-host` draws lines, outlines and discs as instanced quads shaded by distance
(round ends, antialiased), rectangles and triangles as plain triangles, blended in
list order, clipped to the screen, and re-uploads them only when the list changes.
The screen is the terminal grid's rectangle, which is 4:3, so a circle is round.
Layers in the clean style (SPEC §vector.layers): cell backgrounds, then shapes, then glyphs
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

`vec_style(VEC_STYLE_PHOSPHOR)` (SPEC §vector.phosphor) asks for the look of a vector
monitor tube. Every host must draw the background black (cell and page backgrounds
are not drawn), shapes and the page's figures as outlines, and the text in its
colours, legible; the look beyond that (light that adds up, glow, afterglow) is the
host's choice, and a host that draws plain lines conforms. In `terse-host` the shapes
add light instead of covering, and the terminal's text is light on the same tube.
What the program shows now is drawn at full brightness each frame; an afterglow of
earlier frames fades behind it, fast at first and then with a dim tail multiplied by
persistence / 256 each frame (`vec_persistence(n)`, 160 after reset, a hint other
hosts may ignore). The style, persistence and fill mode are machine state (in the
state hash, STATE.md 1.7); the afterglow is not, and the program cannot see it.

How `terse-host` draws it (all constants in `crates/terse-host/src/phosphor.rs`): the
aim is a real vector monitor or oscilloscope, a thin, sharp, nearly white-hot line
on true black with a very faint, very wide glow, not a neon blur. Each line is the
light of a Gaussian beam spot moved along it at constant speed: a Gaussian across the
line (σ = 0.5 pixel at the default width, so the core is about 1.2 pixels wide at half
its peak at any resolution, and grows with wider lines) and the difference of two
error functions along it, so segments that meet end to end join seamlessly and a lone
end is a soft round cap. The core's peak is 0.85 (linear, times colour and opacity).
Every distinct end point gets a dot of extra light where the beam dwells, shared
between the lines that meet there; a line of length 0 is a dot. Light adds up
linearly, so a line drawn twice is twice the light, and crossings are brighter.

Text (the terminal's glyphs and cursor, the page's text and figures) is light on the
same tube in its own antialiasing and exactly its colours; it gets no narrow glow and
no motion smear, so it stays as readable as in the clean style.

The glow is the faceplate's halation: four Gaussians of 0.4%, 1.2%, 3% and 8% of the
screen's height, weighted 0.06, 0.08, 0.07 and 0.05, computed on a pyramid of the
light at a quarter, an eighth, a sixteenth and a thirty-second of the resolution and
added on top of the untouched core (text feeds it at a quarter of its light). Each is
faint where it lands, so the gaps inside letters and between close lines stay dark,
but together they put about a sixth of a line's light more than three pixels away.

The afterglow is two floating-point textures, a fast component (time constant 20 ms
for the default tube, so about 8% after three frames) and a slow tail (× persistence /
256 per frame), fed with 0.2 of the beam and text (0.6 of that to the fast component)
taken as a maximum. What is shown is the brighter of what is lit now and its
afterglow, so something that stays put shows exactly its own light while a trail
fades behind a moving one; each component loses a small epsilon per frame so trails
really reach black. When a list keeps its sequence of opcodes from one frame to the
next, the afterglow also takes half that deposit at 3 to 15 positions interpolated
between the old and new coordinates, so a moving shape leaves a continuous smear, not
separate copies (not for jumps of more than 40 units). The tone curve leaves light
below 0.8 untouched and rolls the brightest channel off above it, scaling the others
with it so the hue is kept; light above 2.1 (a line drawn three times, bright
crossings) first spills into the other channels and turns white-hot. The afterglow
advances once per machine frame, not per presented frame or by the clock, so it does
not depend on the display's refresh rate and screenshots are the same on every run.

`--tube` chooses the tube: `xy` (the default, a colour XY monitor: each line in its
colour), `p31` (a green oscilloscope tube with a very short fast afterglow, 3 ms),
`amber` (50 ms) or `vectrex` (white, 15 ms); the monochrome tubes show colours as
brightness. `--vignette` darkens the glass towards the corners (by up to 30%). Both
renderers draw all of this the same way, within rounding. (`--bloom`, which chose the
resolution of the earlier bloom, is gone: the halo is always computed on its pyramid.)

Measured by `cargo test -p terse-host soft::look` (both renderers, 1280 × 960 and
2560 × 1920; a slanted white line of width 1 unit, and grey "8 corners, 12 edges"):
the core is 1.1 pixels wide at half maximum at both sizes; the frequency at which a
line's modulation falls to 50% is 0.92 of the clean style's at 1280 × 960 (the clean
line is 2 pixels wide there, the phosphor core thinner); 16–18% of a line's light lies
more than 3 pixels from it; a line drawn twice gives 1.5 times the light within
1.5 pixels of it; the contrast between a letter's strokes and the gaps inside it is
0.91–0.92 (the earlier, narrow bloom left 0.29–0.45), and the strokes' peak is the
clean style's within 1/255.

A program can therefore brighten a line on `terse-host` by drawing it more than once (another
host need not add light up, SPEC §vector.phosphor); Sparks does that with its arena border and craft.

`--no-trails` turns the afterglow off, for anyone who finds it distracting;
`--force-phosphor` shows any program this way. In the clean style none of this
runs.

Try it in Sparks (P switches style):

    cargo run --release -p terse-host -- programs/sparks/sparks.terse
    cargo run --release -p terse-host -- programs/sparks/sparks.terse --no-trails
    cargo run --release -p terse-host -- programs/sparks/sparks.terse --events programs/sparks/sessions/phosphor.txt --frames 900 --screenshot /tmp/phosphor.png
    cargo run --release -p terse-host -- programs/reader/reader.terse --force-phosphor

## 3D vector display

Device 9 (SPEC §vector3d) draws lines, points and circles placed in depth: the program
gives geometry, the host projects it. Coordinates are sixteenths of a screen unit,
x right and y down as on the screen and z away from the viewer, with 0 at the glass
(the screen's plane). The reference eye is at (320, 240, −960); a point is drawn at
x′ = ex + (x − ex)·960 / (960 + z) (y′ likewise), so a point on the glass is where
the 2D layers put it, from any eye. The host moves the eye up to 160 units each way
(parallax): in a window it follows the pointer (the picture's centre is the
reference eye, an edge the full 160 units, smoothed over a few frames; in the shell
every tile has the same eye); `--parallax off` keeps it still, and `--eye <dx>,<dy>`
places it for `--screenshot` and `--bench-render`. The program never learns where the
eye is.

The list (at most 16,384 bytes, submitted like device 3's) has a matrix stack:
Translate, Rotate (about x, y or z, by 1/65,536 of a turn; a positive angle about z
turns x towards y, clockwise on the screen) and Scale (8.8) multiply the transform
on the right, so the last given applies first to later coordinates; Push and Pop
save and restore it, 8 deep. Move and Line keep the current point where they put it
(transformed), so a later transform does not move it; Circle draws about the current
point in the transform's xy plane (Move to (0, 0, 0) after a Translate to centre a
circle there); Point draws a dot. Lines are cut at z = −800 and at the screen, and
dim with depth: opacity × 960 / (960 + z) at each end (at most 1), interpolated
along the line. One list draws at most 65,536 units of length on the glass (seen
from the reference eye, so the same lines are drawn from every eye) and 16,384
segments; a circle is 64 chords. The style and persistence are device 3's (calls
`902h`, `903h` set the same setting), so a program with only device 9 can choose
the phosphor style. Device 9 is drawn after device 3's shapes, under the terminal's
characters, so text on the glass labels the models.

**Screens** (Screen, opcode `0Dh`: a handle and a width) work only in the shell's
list: each places that machine's whole screen as a flat 4:3 panel centred on the
transform's origin, in its xy plane (with the identity it lies like the glass),
drawn in perspective from the eye and hiding the shell's lines behind it like a face.
The shell's screen is then drawn first without its 3D lines, the panels far to near
over it, the lines over them, and flat pictures over everything (SPEC §machines.screens). Each
panel's machine is drawn into a picture of its own at about the panel's size on the
screen (a whole glyph cell per 80th, at most the shell's own), mipmapped (both
renderers: trilinear, and anisotropic on the GPU), so text on a turning panel does
not shimmer; seen from behind a panel is a dark plate. The pointer reaches the
nearest panel the ray from the eye through it meets. Device 8's `face` and `unface`
move the whole scene (a rigid motion and scale, eased) until a panel lies flat on
the glass over a rectangle and then show that machine flat there, pixel for pixel
as `place` would; the view's progress counts the shell's frames and is in its
state. `--panels flat` makes the software renderer draw each panel's picture
upright in the rectangle the panel covers instead (the fallback SPEC §machines.screens allows a
host that cannot draw in perspective).

**Faces** (Face, opcode `0Ch`: 3 or 4 corners) draw nothing but hide the lines,
points and circles behind them, as the eye in use sees them: a point is hidden when
the straight line from the eye to it passes through a face's triangle (a 4-corner
face is triangles 0-1-2 and 0-2-3) and the point lies more than 2 sixteenths beyond
the triangle's plane, so a solid's own edges, given with the same corners as its
faces, are never hidden by those faces. Faces hide wherever they are in the list,
have two sides, and are cut at z = −800 like lines. A hidden stretch is cut out of
the line: the pieces keep the depth cue of the whole line, and the cut ends are
butt ends without the phosphor's vertex dwell (pieces shorter than half a unit with
a cut end are dropped, so no specks show at silhouettes). A list has at most 256
faces, and faces × segments at most 131,072 (16,384 segments with up to 8 faces, 512
with 256); the caps count as if nothing were hidden, so the same commands are drawn
from every eye, and at most 16,384 pieces are drawn after cutting. A list without
faces draws exactly as before.

**Surfaces** (Surface, opcode `0Eh`: a source rectangle x, y, w, h of the program's
own screen, clamped as the page's rectangle is, and a panel width) let any program
with device 9 put parts of its own 2D layers (terminal, page, 2D shapes; never its 3D
list) on flat panels in its scene: a panel as wide as given and as high as the
source's shape, centred on the transform's origin like a Screen. At most 4 take
effect in a list; each counts as a face and hides the lines behind it as a Face of
its corners does. The machine is then drawn as its glass (its 2D layers with the
sources black: what a Surface shows is not also on the glass), the Surfaces far to
near in perspective, then its 3D lines over them; with no Surface nothing changes
by a pixel. Wherever the machine is shown (alone, in a tile, as a panel in the
shell's gallery) it is this complete picture; the shell's own Surfaces join its
panels. A pointer over a Surface's front (the nearest one the ray from the eye
meets) is delivered as a pointer at the matching point of its source
(`vector3::surface_pointer`, applied by `System`'s routing to every machine), so text
and page anchors on a console can be clicked; anywhere else, as on the glass. The
GPU draws the machine's 2D layers a second time into a texture as large as the glass
(larger, up to twice, if a Surface magnifies its source) when they change, and maps
it like a panel; the software renderer samples a pyramid of the part of the screen's
own image the sources cover. In the phosphor style the sources give no light on the
glass (nothing of them glows there), the 2D layers on a Surface glow with their
afterglow (it belongs to the screen, so it moves with the panel), and the 3D lines
over the Surfaces are drawn as over the shell's panels, as clean beams with the
depth cue. `v3_surface(x, y, w, h, width)` builds one.

`terse_core::vector3::project` turns a list and an eye into the segments to draw (on
the glass, in sixteenths, with the depth cue at each end and, where a circle's
chords join or a face cut a line, butt ends instead of round caps and vertex dwell),
with what the faces hide already cut away. It cuts each segment against the faces'
triangles nearest first: the triangles are sorted by their nearest z, so only those
nearer than a segment's far end are tried, each first by its box on the glass and
by whether both ends of the segment lie outside one of its edges, and a segment
stops being tried once one triangle hides all of it. A typical scene (20 boxes, 100
faces, 300 lines) projects in about 0.2 ms on an i7-10750H; the worst lists the caps
allow (16 large faces that hide nothing in front of 8,192 chords, or 256 slivers
over 512) in about 2.2 ms, against 1.3 ms for 16,384 chords without faces. Both renderers draw
them through the 2D display's line path, in the same instanced quads and the same
software spans, so the clean style, the phosphor beam, afterglow and smear all apply.
They project again only when the list or the eye changes. From C:

    static uint8_t list[1024];
    v3_begin(list);
    v3_colour(80, 220, 255, 255);
    v3_width(20);
    v3_push();
    v3_translate(320 * V3_UNIT, 240 * V3_UNIT, 0);  /* the centre, on the glass */
    v3_rotate(V3_AXIS_Y, angle);                     /* angle: 0-65535, a turn */
    v3_move(-50 * V3_UNIT, 0, -50 * V3_UNIT);
    v3_line(50 * V3_UNIT, 0, 50 * V3_UNIT);
    v3_move(0, 0, 0);
    v3_circle(60 * V3_UNIT);
    v3_pop();
    v3_submit_list(list);                            /* VEC_DONE or VEC_REJECTED */

`v3_point`, `v3_scale`, `v3_clear()`, `v3_style()` and `v3_persistence()` are the rest
(`programs/lib/terse.h`), with `v3_face3(a, b, c)` and `v3_face4(a, b, c, d)`, whose
arguments point at x, y, z triples (a solid's corner table serves its edges and
its faces).

## Holo

`programs/holo` shows the 3D display: Space (or Tab) goes to the next scene, 1–6
choose one: a turning cube and icosahedron labelled on the glass, six nested rings
each turning in the one outside it, 384 points receding from the glass, an orrery
of four planets on tilted orbits (two with moons), a city: ten solid blocks
round a tower (eleven in all) on a ground grid, turning, so that blocks pass in front of one another
and hide each other and the ground (55 faces), and a cockpit: stars streaming past
and three wire octahedra receding through a wire-frame canopy, over a solid
dashboard with three angled consoles, each a Surface showing a region of holo's
terminal (instruments that change every frame, a log that scrolls, a list of
targets). A click on a row of the list on its tilted console selects it (holo gets
an ordinary pointer event inside the list's region); a status line stays on the
glass, and in the cockpit the arrows bank and pitch the craft, consoles and canopy
together. H makes the cube and icosahedron
solid (6 and 20 faces) and wire again. The arrows turn the model, A stops and
starts the turning, R resets the view, P switches between the clean and the
phosphor style. All the rotation is the host's (Rotate); the program only
changes angles, so it does no trigonometry. Move the pointer over the window to look
around the models: near parts move against far ones, while the labels and the dim
frame on the glass stay put.

    cargo run --release -p terse-host -- programs/holo/holo.terse
    cargo run --release -p terse-host -- programs/holo/holo.terse --frames 90 --eye 160,0 --screenshot /tmp/holo.png

ROM 10,284 bytes, 2 RAM pages; devices 2 and 9; the busiest frame of every scene
uses at most 23% of the budget (the city in the phosphor style; the cockpit's 23.1%,
its typical frame 15.7%). Tests: `crates/terse-cli/tests/holo.rs`
(golden screens of each scene in `programs/holo/expected/`, the keys, the solids,
the first four scenes' segments unchanged by faces, the frame budget, the cockpit's
Surfaces and a click through its tilted console from three eyes).

## Page

Device 7 (SPEC §page) shows typeset prose: proportional fonts, shaped (ligatures,
Arabic joining), bidirectional, broken into lines at the width of a rectangle the
program chooses. The program builds a **document** (a short binary list like a
display list: Text, Font, Size, Colour, Paragraph, Rule, Anchor, Figure, Table, Cell, End table; at most
65,536 bytes) in ROM or RAM and submits it (1 unit per 4 bytes); it sets the rectangle,
the scroll value (screen units from the top of the document) and the background.
The host lays the document out with its own fonts and draws the part in view,
between the cell backgrounds and the vector shapes (SPEC §vector.layers). The program learns
two facts about the layout, each from input events: the document's height (`08h`),
which arrives some frames after a submit, a clear or a width change, and, when it
calls `locate` with an anchor id, the y and height of the first line or figure with
that anchor (`0Dh` and `0Eh`, a pair; `page_place_event` in `terse.h` puts them
together), which arrive with the height or in the next frame. A press over text
with an anchor gives an anchor event (`09h`). All are recorded inputs, so a run
replays exactly although layouts differ between hosts. A paragraph flag cuts a
no-wrap line that does not fit with an ellipsis. An image gets the device
with `terse pack … --device 7:0`.

From C (`programs/lib/terse.h`), a builder appends commands to a buffer you own,
much like the display-list builder (see the header for the exact names).

**Reference layout.** `terse run` has no fonts; it lays documents out with the
reference layout of SPEC §page.reference (every character half its size wide, a wide one its
size, lines 1.25 × size high, greedy wrapping at spaces), which is deterministic
and what the golden tests and the conformance hashes use. It is not what users see.
`terse run … --dump --dump-page` prints the lines in view:

    -- page: height 1076, scroll 0 (stored 0), region 40 16 560 448 --
        0  33     0   sans26b |The Quiet Craft of Typesetting
       39  14     0   sans11 |a1b2c3d4 · 2026-11-17 18:10 · 6.1 KB
       61   8     0  -- rule --
       79  19     0   sans15 |Good typography is invisible. A reader who notices the type has been

(y, line height and x in screen units, the font of the line's first span, then the
text; anchored text is shown as `[text]{id}`, a figure as `-- figure WxH --`).

**Figures.** A Figure command is a block of its own in the flow, width × height
screen units (up to 1024 each), aligned start, centre or end and cut off (not
scaled) if wider than the rectangle. It holds path commands much like an SVG path
in binary: Fill and Stroke (colour and opacity; stroke width), Move, Line,
Quadratic, Cubic and Close, with coordinates in sixteenths of a unit from the
figure's corner. Each run of subpaths between Fill/Stroke changes is filled
(non-zero) and then stroked with round joins. Three caps per figure (256 paths, an
area of 8 × the figure's, a total path length) keep a hostile document cheap to
draw (SPEC §page.document). With the vector display in the phosphor style or fill mode 1,
figures are outlines, and in the phosphor style they glow like the text.

**In `terse-host`.** [cosmic-text](https://crates.io/crates/cosmic-text) lays the
document out (HarfRust shaping, the Unicode bidirectional algorithm and line
breaking, fallback to other installed fonts for scripts the face lacks) on a worker
thread, in screen units and without hinting, so a window of any size uses the same
layout; the machine keeps running at 60 frames/s and shows the previous layout (or
only the background) until the new one is ready, then the height event goes in at
the next frame. Glyphs are rasterised with [swash](https://crates.io/crates/swash)
at the window's own pixel size (not at 640 × 480 and scaled), cached in a 2048 ×
2048 atlas, and drawn as quads clipped to the rectangle; only the lines in view are
drawn, each line's baseline on a whole pixel. Scrolling moves the lines only (no new
layout) and eases towards the program's scroll value over a few frames, so on a
large window it moves in steps finer than a screen unit. The three faces are the
system's `sans-serif`, `serif` and `monospace` families as `fc-match` resolves them
(Noto Sans, Noto Serif and Noto Sans Mono on the development machine), or the
families or font files given with `--page-sans`, `--page-serif` and `--page-mono`;
the line height is the face's own (ascent + descent + gap), at least 1.3 × the size.
`terse-host` prints the faces it uses. In the phosphor style the page's background
is not drawn and its text is light on the tube, drawn as terminal text is. Figures are rasterised with [tiny-skia](https://crates.io/crates/tiny-skia)
(antialiased, at the window's pixel size) on their own thread, only those within
half a rectangle height of the view; each gets a texture (at most 4 Mpixels; a
bigger one is drawn at a lower resolution), and those that leave that band are
dropped, so the memory held stays bounded whatever the document holds. A figure
that has just come into view may appear a frame or two late. `--screenshot` lays
the page out with the same fonts between frames, and rasterises figures at once,
so the picture shows the finished page.

    fc-list : family | sort -u
    cargo run --release -p terse-host -- programs/reader/reader.terse --page-serif "DejaVu Serif"
    cargo run --release -p terse-host -- programs/reader/reader.terse --page-sans /usr/share/fonts/TTF/SomeFont.ttf

Setting `TERSE_PAGE_TRACE=1` makes `terse-host` print, per frame, when a layout is
pending and when its height event is queued (with the layout's time), and each
figure as it is rasterised (with the time and the figure memory held).

## Fonts

`terse-host` draws the terminal grid with the system's monospace font (the
`monospace` family as [fontdb](https://crates.io/crates/fontdb) resolves it, which
need not be what `fc-match monospace` says), falling back to other installed fonts
for characters it lacks (CJK, symbols). `--font` picks another one, by family name
as `fc-list` shows it or by the path of a `.ttf`, `.otf` or `.ttc` file; in a window
`terse-host` prints the font it uses on stderr:

    fc-list : family | sort -u
    cargo run --release -p terse-host -- programs/reader/reader.terse --font "DejaVu Sans Mono"
    cargo run --release -p terse-host -- programs/reader/reader.terse --font "Noto Sans"
    cargo run --release -p terse-host -- programs/reader/reader.terse --font /usr/share/fonts/liberation/LiberationSerif-Regular.ttf
    cargo run --release -p terse-host -- programs/hello/hello.terse --font "DejaVu Serif" --screenshot /tmp/serif.png --frames 30

A monospace font sets the cell size from its own advance, as the default does.
A proportional font is only a look test: the machine's terminal is still a grid, so
the cell stays the size the default monospace font gives, the font is sized so its
text body (ascent to descent) is as tall as that font's, and each glyph is centred in
its cell (a wide character in its two cells), squeezed horizontally only if it would
not fit (`m` and `W` usually are). Characters the font lacks come from the fallbacks
as usual.

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
- The high score (`HIGH` in the HUD) is the named record `high-score`, four bytes
  little-endian: asked for in frame 0 and shown when it arrives (the first frame does
  not wait), saved when a game beats it ("New high score!").
- Content is optional (SPEC §image): sparks requires devices 2 and 3 only, so it runs on a
  host of the program profile (`--profile program`); there the high score lasts for
  the session, and the game is otherwise the same.

In the phosphor style Sparks draws the arena border and the craft twice, so they
shine brighter than the sparks (overdraw adds light on `terse-host`, SPEC §vector.phosphor); the list is
otherwise the same in both styles.

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
the busiest frame is frame 0 (setting up, 8,953 units, 7.2% of the budget); after
that at most 5,400 (4.3%), a typical frame 2,077 (1.7%), about 12 µs of host time.
ROM 6,464 bytes, 1 RAM page; devices 2, 3 and 4.

## Sketch

`programs/sketch` is a small drawing program for the vector display, in C, built in
so that documents (SPEC §content.documents) have a program to open them. Draw strokes with the
pointer (press and drag); 1–6 choose the colour (white, red, yellow, green, blue,
magenta, shown by the disc at the bottom right), U undoes the last stroke, C clears,
F switches between the clean and the [phosphor](#phosphor-style) style.

- S saves the drawing to the named record `drawing`, which is loaded at start.
- P publishes it: the drawing is put as a document of the built-in format (zero key,
  `sketch`) and published as an entry of kind 4 titled `Sketch N` (N counts this
  host's publishes, in the record `published`); the host asks first, as for any
  publish.
- Started with an argument (a shell opening a sketch document, for example from a
  reader link or a timeline entry), it shows that document read-only; the first
  stroke starts editing a copy, which S saves as the `drawing`.

The data is a list of strokes (colour, point count, points in screen units), at most
8,192 bytes, so a published sketch stays under the automatic download limit; the
exact layout is in the header comment of `sketch.c`. ROM 4,988 bytes, 4 RAM pages;
devices 2 and 3, and 4 (content) as optional: without content it says "Drawings are
not kept", draws as ever and cannot save or publish. Tests:
`crates/terse-cli/tests/sketch.rs`.

## Sound

Device 6 (SPEC §sound) is a 16-voice wavetable synthesizer in the spirit of the MSX SCC,
with pairing (one voice moves another's phase), feedback and ring modulation, noise
and four sample slots; 48,000 Hz stereo, 800 sample pairs a frame, every step integer
and specified to the bit. It is optional: the reading profile does not have it.
`programs/tune` plays a short piece through each kind of voice in turn (about 18 s,
then it exits):

    terse run programs/tune/tune.terse --audio-out tune.wav     # write it, play nothing

`terse-bus --audio-out` does the same with the device as a driver process
(`terse-dev-sound`). `terse-host` plays the sound only when built with the `audio`
feature (cpal, so ALSA, which PipeWire serves):

    scripts/cargo build --release -p terse-host --features audio

Without the feature, and in `--selftest` or with `TERSE_AUDIO=off`, the device is
there and runs as usual but nothing is heard; the audio output is opened only when a
machine first gives samples. Tests: `crates/terse-core/src/sound.rs` (the
synthesis, bit for bit), `crates/terse-cli/tests/tune.rs` (the golden samples, a
snapshot in the middle of a note, `--audio-out`, the reading profile),
`crates/terse-devices/tests/sound.rs` (the driver and the compiled-in device give the
same samples).

## Conformance image

`programs/conform/conform.terse` (C and assembly: `conform.c`, `cpu.S`, with its own
`crt0.S` and `conform.ld`, so changes to `programs/lib` do not touch it) is for
whoever writes a new host: run it with its scripted input `conform.events` and
compare. It needs devices 2, 3, 4, 7 and 9 and 16 RAM pages. Two uses:

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
  predicts from the constants of SPEC §frames and §charges (a loop of period p whose call is
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
  split across `write` calls), cells and palette calls with their wrap and bounds,
  and that basic and other characters take the same cells whether or not the
  machine has full text (device 10, which it does not list: absent);
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
- the page (frames 110–145, after the stress phase): submits from ROM and RAM, a
  65,536-byte document ending at the last byte of RAM, every rejection (length 0
  and 65,537, null, straddling the end of RAM or ROM), region clamping, a negative
  and a huge scroll value, background, clear, unknown page calls; height events
  (one the frame after a submit, again for the same bytes, a narrower rectangle
  gives a height at least as large, none while the width is 0, 0 after a clear),
  and anchor events for scripted presses over large anchored text, each right
  after its pointer event with its buttons; `locate` rejections and answers (a
  figure at y 0, 100 high; the text after it at y 100; an absent anchor). Its own checks hold for any host's
  layout (they pass in `terse-host` with Noto Sans and DejaVu Sans as well); the
  exact heights and frames of the reference layout are in RAM, so the hashes
  cover them;
- frames 49–108, the stress phase: every frame uses its whole budget, rewrites all
  2,400 cells (600 CJK, Extension A and Hangul characters never shown before, Latin
  Extended-A and Cyrillic rows, colours changing) and submits a new 16,384-byte
  display list: frames 49–78 mixed shapes (about 1,900 circles and many lines; the
  fill cap stops the fills), frames 79–108 the worst legal case: fills totalling
  just under the cap (three screen-covering discs and a 639 × 480 rectangle), then
  3,268 translucent lines of width 255 across the screen, about 109 screens of line
  area (lines are not capped, SPEC §unspecified).

**The reading profile.** `programs/conform-reading/conform-reading.terse` is the
same source built with `-DREADING` (`programs/conform-reading/build.sh`), for a host
that offers only the machine's reading profile: devices 2, 4 and 7, 16 RAM pages.
The vector displays and the optional content operation (put) are left out, calls
to devices 3 and 9 are checked as calls to absent devices, the stress phase
rewrites the terminal only, and the gets read two blobs the host must hold before
it starts (`programs/conform-reading/blobs/`, the bytes conform puts). Run it with
those blobs in an otherwise empty store and no subscriptions, and compare with its
`expected-hashes.txt` and `expected-screen.txt`; it gives the same on a full host
and under `--profile reading`:

    mkdir -p /tmp/conform-reading/config
    for f in programs/conform-reading/blobs/*; do cargo run -p terse-cli -- put $f --store /tmp/conform-reading/store --relay none; end
    cargo run -p terse-cli -- run programs/conform-reading/conform-reading.terse --events programs/conform-reading/conform-reading.events --store /tmp/conform-reading/store --config /tmp/conform-reading/config --frames 200 --hash-every 10 --profile reading

`cargo test -p terse-cli --test conform_reading` does this, on both hosts.

**The program profile.** `programs/conform-program/conform-program.terse` is the
same source built with `-DPROGRAM` (`programs/conform-program/build.sh`), for a host
that offers only the machine's program profile: devices 2 and 3, 16 RAM pages. The
3D display, content and the page are left out and their calls checked as calls to
absent devices, and a section checks the `device` call. It needs no store: run it
with `programs/conform-program/conform-program.events` and compare with its
`expected-hashes.txt` and `expected-screen.txt` (`--dump --dump-vectors`); it gives
the same on a full host and under `--profile program`:

    cargo run -p terse-cli -- run programs/conform-program/conform-program.terse --events programs/conform-program/conform-program.events --frames 200 --hash-every 10 --profile program

`cargo test -p terse-cli --test conform_program` does this, on both hosts.

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
at 60 Hz. With the page device added (and its two sections after the stress
phase), 200 frames in the window: emulation p50 0.13 ms, p99 0.88 ms; render-submit
p50 1.05 ms, p99 11.9 ms, the same one-time 1.06 s for the CJK fonts (before the
page device: p50 1.02 ms, p99 9.2 ms); 199 of 200 frames presented. Nothing passes
or fails on these numbers.

## Software renderer

`terse-host --renderer software` (or `auto` on a machine without a hardware
graphics adapter) draws the same picture as the GPU renderer, on the CPU, into a
buffer of XRGB8888 pixels (`0x00RRGGBB` words, what softbuffer and DRM dumb buffers
take): the shell's screen, each machine's picture scaled into its tile, the
consent prompt and the fault row. It is `crates/terse-host/src/soft/`, behind the
`Display` trait both renderers implement (`display.rs`).

- **What it draws.** Every layer of SPEC §vector.layers with the GPU shaders' arithmetic:
  cell backgrounds; the page (background, glyphs rendered with swash at whole-pixel
  positions, rules, figures rasterised with tiny-skia, scrolled and clipped); the
  vector display (lines, circle outlines and discs by their distance from the pixel
  centre with a one-pixel edge, rectangles and triangles by pixel centres; fill mode
  1 as outlines); the glyphs (wide and colour ones) and the cursor; the host's
  overlay over everything. Cells, glyph pixels and screen pixels map through tables
  computed with the shader's formula, so a glyph lands on the pixels it does on the
  GPU.
- **The phosphor style** follows `phosphor.rs` and the GPU's passes in linear float
  RGB: the text's light, the shapes' Gaussian beam with its vertex dwell, the
  afterglow's two components faded and deposited with `max` (with the smear of
  moving shapes), the halo's pyramid (the light averaged over 4 × 4 pixels, each
  level from the one before, the blurs, the sums back up), then tint, spill, tone
  curve, glass and sRGB. Differences from the GPU are of rounding: the beam's
  Gaussian and error function come from 4,096-step tables, sRGB from a 4,096-step
  table, and floats are 32-bit (the GPU's are 16-bit).
- **Only what changed is drawn.** Each screen keeps an image of its pixels and
  compares what it shows with what it drew: cells whose glyph or colours changed,
  the cursor's old and new cells, the page's rectangle when it scrolled or its
  layout, figures or background changed, the boxes of shapes that changed; the
  damage is kept in 32-pixel tiles and drawn again from scratch there, cell by cell
  (a blank cell is a fill). The frame is then put together where anything changed
  (shell, pictures in handle order, prompt), and the damaged rectangles go to the
  presenter. A phosphor screen keeps, per row and buffer, a mask of the 16-pixel
  blocks where light may be, so dark parts of the tube cost nothing.
- **Threads.** Tile rows (and bands of 16 rows of the tube) are drawn on a rayon
  pool; `--threads n` sets its size (default one per core; 1 draws on the calling
  thread). There is no `unsafe` code: the inner loops are written to vectorise.
- **Presenting.** In a window the frame is copied into a softbuffer surface, only
  the damage when the buffer handed back still holds a recent frame. This path has
  not been tried in a live window yet. `soft::FrameSink` is where the bootable
  image's KMS/DRM output is to plug in.
- **Without the GPU and the window.** `cargo build --release -p terse-host
  --no-default-features` leaves out wgpu and winit (features `gpu` and `window`):
  the result has `--screenshot` and `--bench-render` with the software renderer and
  no window yet.
- **Tests.** `cargo test -p terse-host` checks that a screen drawn bit by bit
  equals one drawn from scratch, that the phosphor afterglow fades to black, the
  shapes' row spans against brute force, butt-ended segments against a whole line,
  the phosphor look's measurements (`soft::look`, by both renderers), and ten
  scripted scenes against pictures in
  `crates/terse-host/tests/reference/` (400 × 300, made with the DejaVu fonts; at
  most 0.5% of pixels may differ by more than 8/255).

Drawing time per frame, p50 in ms (`--bench-render`, i7-10750H, 600 frames after
the warm-up; software with one thread and with all twelve; GPU: RTX 2060, timed
until the GPU is done). A single machine is shown at a whole multiple of its
native 880 × 660 grid, so at 1320 × 990 it is drawn at 880 × 660 and at
1920 × 1440 at 1760 × 1320; the shell fills the window.

| Scene | 880 × 660: 1 / 12 threads / GPU | 1320 × 990 | 1920 × 1440 |
|---|---|---|---|
| hello, idle but for its frame counter | 0.13 / 0.20 / 0.21 | 0.13 / 0.17 / 0.25 | 0.14 / 0.17 / 0.53 |
| reader, an article scrolling all the time | 1.29 / 0.58 / 0.36 | 1.23 / 0.47 / 0.42 | 3.79 / 1.27 / 0.56 |
| sparks | 0.18 / 0.14 / 0.20 | 0.18 / 0.11 / 0.26 | 0.36 / 0.17 / 0.36 |
| sparks, phosphor | 6.84 / 1.62 / 0.82 | 6.68 / 1.84 / 0.84 | 20.9 / 6.41 / 3.72 |
| shell: reader scrolling, sparks phosphor, hello, sparks | 2.00 / 0.83 / 0.74 | 3.00 / 1.18 / 0.84 | 5.62 / 1.98 / 1.06 |
| reader scrolling with `--force-phosphor` (a screen of glowing text) | 16.5 / 4.58 / 0.99 | 16.1 / 4.77 / 1.02 | 54.3 / 15.2 / 4.29 |
| holo, rings (six circles of 64 chords turning; its busiest scene) | 1.12 / 0.36 / 0.26 | 1.09 / 0.35 / 0.25 | — |
| holo, rings, phosphor (the turning rings' smear: about 5,000 deposits a frame) | 19.6 / 3.95 / 0.99 | 19.9 / 4.35 / 0.98 | — |
| holo standing still (A): nothing to draw again | 0.00 / — / 0.22 | — | — |
| holo, city (55 faces cut out of 160 lines each frame; `--sweep-eye`) | 1.02 / 0.49 / 0.41 | — | — |
| holo, city, phosphor (`--sweep-eye`) | 16.3 / 3.62 / 1.05 | — | — |

The phosphor rows predate the reworked phosphor look (Gaussian beam, wide halo
pyramid). Measured again at 1280 × 960 with twelve threads, before → after: sparks
2.0 → 2.45 ms, holo's wireframes 3.8 → 3.6, rings 4.5 → 4.2, city 4.3 → 3.8, the
shell with four machines forced to phosphor 4.8 → 4.8; on the GPU sparks 1.12 → 0.88,
holo 0.98 → 0.92, the shell 3.2 → 3.5.

## Measurements

Release build on the development machine (i7-10750H). Budget 125,000 units per
frame; "busiest" and "typical" (median) are units used per frame in the tests'
recorded sessions.

| Program | ROM | RAM | Busiest frame | Typical frame | Host time per frame |
|---|---|---|---|---|---|
| hello | 881 bytes | 1 page | 1,649 (1.3%, frame 0) | 152 (0.1%) | about 1 µs |
| fetch | 1,544 bytes | 1 page | 2,239 (1.8%) | 36 | about 1 µs |
| reader | 60,000 bytes | 42 pages | 77,383 (61.9%, building a 48 KiB article of tables) | 183 (0.1%) | — |
| holo | 5,460 bytes | 2 pages | 24,898 (19.9%) in the city (11 solid blocks: 55 faces, 3,195 list bytes); 20,753 (16.6%) in the starfield | 10,698 (8.6%) | the city's projection 0.14 ms |
| sparks | 6,464 bytes | 1 page | 8,953 (7.2%, frame 0); 5,400 (4.3%) after | 2,077 (1.7%) | 12 µs |
| conform | 39,224 bytes | 16 pages | 125,000 (stress) | — | 0.57 ms (stress) |
| shell | 19,140 bytes | 3 pages | 12,989 (10.4%, the launcher rebuilt with 4 machines); 24,184 (19.3%) in the gallery; saving a session of 3 machines 7,834 (6.3%) | 537 (0.4%) | — |

A system in a 1320 × 990 window (`terse-host --selftest --shell --size 1320x990`,
1,200 frames, RTX 2060; the machines started by script in the first 300 frames;
steady state is frames 400 on): emulation is all the machines' frames,
render-submit the upload of every screen and the encoding and submission of the
picture.

| Machines | Emulation p50 / p99 / max | Render-submit p50 / p99 / max | Steady state, emulation + render p50 / p99 |
|---|---|---|---|
| 1 (reader on an article) | 0.26 / 0.32 / 6.3 ms | 1.5 / 1.7 / 18 ms | 1.7 / 1.9 ms |
| 4 (2 readers, sparks in phosphor, hello) | 0.75 / 0.92 / 6.4 ms | 3.3 / 3.6 / 628 ms | 4.2 / 4.4 ms |
| 8 (3 readers, 3 sparks in phosphor, 2 hellos, grid) | 1.37 / 1.65 / 7.0 ms | 5.5 / 8.1 / 642 ms | 6.9 / 7.3 ms |

The frames over 16.7 ms are the ones in which a machine first appears (a renderer
of its own, its glyphs: 15–19 ms), in which a new machine makes the others' tiles
change size (glyphs and textures for the new size: 13–15 ms), and the first frame
showing hello's CJK and Greek text (600–640 ms looking through the system's fonts for
the characters the monospace font lacks; `terse-host hello.terse` alone pays the
same at its first frame).

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

## Licence

Apache License 2.0; see `LICENSE`. That covers the code, the specifications
(`NETWORK.md`, `ARTICLE.md`, `SPEC.md`, `STATE.md`, `DEVICES.md`), the manual and
the programs in this repository. Anyone may write their own implementation
of the specifications.
