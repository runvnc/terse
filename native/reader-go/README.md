# tersereader: a native Tersenet reader in Go

`tersereader` is a second, independent implementation of the Tersenet core: a
terminal reader that speaks the Tersenet protocol (`NETWORK.md`) and shows Tersenet
articles (`ARTICLE.md`) with **no virtual machine**. It is a single static
binary with no third-party dependencies.

It was written **from the specifications alone**, without reading the
reference implementation (no Rust in `crates/`, no C in `programs/`, no
`SPEC.md`, `STATE.md` or `DEVICES.md`). The documents used:

- `NETWORK.md`, `ARTICLE.md`
- `vectors/README` and every file in `vectors/`
- `docs/manual/native-reader.txt`, `docs/manual/writing-articles.txt`,
  `docs/manual/network.txt`

The reference binaries (`terse`, `terse-relay`) were used only as black boxes
in the interoperability test. Where the documents left a question open, the
question and the assumption made are in `SPEC-FINDINGS.md`.

## Build

    cd native/reader-go
    CGO_ENABLED=0 go build -o tersereader .

Go 1.22 or later. Linux (and the BSDs and macOS, untested) for the
interactive mode, which puts the terminal in raw mode with `ioctl`.

## Use

    tersereader relay add ws://127.0.0.1:7878     # your relay (none is assumed)
    tersereader subscribe <key>[@host[:port][,host[:port]]]
    tersereader sync                              # new entries and small articles
    tersereader list                              # the timeline, newest first
    tersereader show <blob-id>                    # an article as text
    tersereader --dump <blob-id|file> --plain     # for screen readers and pipes
    tersereader                                   # the interactive reader

Commands: `subscribe`, `unsubscribe`, `sync`, `list`, `feeds`, `feed`,
`show`, `relay add|remove|list`, `nickname`, and `--dump`. `--help` lists
the options. Everything is kept in one directory, `--dir` (default
`$XDG_DATA_HOME/tersereader`, or `~/.local/share/tersereader`): the blobs,
every entry kept in arrival order, the subscriptions, relays and nicknames.

Interactive keys: Up/Down (or j/k) move, Enter opens, Tab and Shift-Tab
select a link, a number then Enter follows link n, Esc or Backspace goes
back, `s` subscribes (to the selected feed link in an article), `u`
unsubscribes, `F` shows an entry's feed, `f` lists the feeds, `?` helps,
`q` quits. Left/Right scroll wide tables and preformatted text sideways.

### Output

- `show` and `--dump` write the title, the article laid out to `--width`
  columns, and a numbered list of the links' targets, with ANSI styles
  when the output is a terminal (not with `--plain`, `NO_COLOR` or
  `TERM=dumb`); `--color` and `--no-color` choose.
- Code in a fence whose info string names a language (`c`, `rust`, `go`,
  `sh`, ...: a choice of this reader, not of the format) is coloured with
  the terminal's palette, in the interactive reader too (not with
  `NO_COLOR`); `internal/render/highlight.go`.
- `--plain` is for screen readers: no box drawing, no braille, lists with
  `-`, tables as space-aligned columns, figures as their source text (the
  fallback ARTICLE.md allows).
- `--tokens` writes the parsed article in the dump format of
  `vectors/README`.

### Figures

Figures are drawn with Unicode braille characters (2 x 4 dots a cell): a
small rasteriser fills paths with the non-zero rule and strokes them with
round joins and ends. Scale follows ARTICLE.md 6.2: 68 columns are the
512-unit column, narrower columns scale figures down, wider ones do not
enlarge them. In colour (the interactive reader, `--color`) each cell shows
its most common colour as background and the other dots over it in 24-bit
colour; `fg` is the terminal's text colour. In monochrome, a colour covering
most of the figure is taken as a panel and left blank so that what is drawn
on it shows. Strokes are thickened to at least one dot.

## What it does

- Blobs: BLAKE3-256 (implemented here), verified on every receipt; a store
  of one file per ID, hashed again when read.
- Entries: every rule of NETWORK.md 4.2, with strict Ed25519 (small-order
  keys and R refused, S < L, canonical encodings); the first valid entry of
  a feed and seq is kept; arrival order kept across restarts.
- Relays: WebSocket (RFC 6455, implemented here; `ws://` and `wss://`),
  get, subscribe with `after` the highest seq held, unsubscribe, pushed
  entries; other reply types ignored. Several relays, reconnecting after
  1 s doubling to 30 s, at most 16 connections. A relay written without a
  scheme is `wss://` (encrypted), but `localhost` or a loopback address is
  `ws://` (NETWORK.md 3). Private authorities and pinned keys from your own
  settings: `<dir>/authorities/*.pem`, options on a relay line
  (`authority=`, `only-authority=`, `pin=sha256:...`), `TERSE_RELAY_CA`,
  `--relay-authority` (the main README, "Private authorities and pins").
- Feed relays: the `relay` lines of a feed's profile (up to two), and the
  hints of a feed address when none of your relays has the feed. Local
  and private addresses are refused, names resolved once; a plain (`ws://`)
  relay named by the network is never used.
- Automatic download up to 16,384 bytes (articles, profiles, documents;
  not programs or undefined kinds) and prefetch of the first 8 linked
  articles. Larger articles are fetched when opened.
- Profiles and display names (always with the short key), nicknames.
- Articles: every construct of ARTICLE.md, links of all three kinds, feed
  addresses with hints in links.

## What it does not do

- Run programs or open documents: program links, program and document
  entries, and blobs starting with `TERS` or `TDOC` are shown as things this
  reader cannot open.
- Publish, keep an identity, set a profile, search, or look programs up
  (all optional for a reading client, NETWORK.md 6).
- Bidirectional reordering or shaping of complex scripts (left to the
  terminal); character widths are approximate.

## A node and a reader (LOCAL.md)

The network half can run on its own and serve readers over a local socket
(`LOCAL.md`):

    tersereader node                  # the node: relays, verifying, the store
    tersereader list                  # a reader: uses the node if its socket answers

`tersereader` with `--node auto` (the default) uses a running node if
`$XDG_RUNTIME_DIR/terse/node.sock` (or `--socket`, or `$TERSE_NODE_SOCKET`)
answers, else runs the node in its own process as before; `--node socket`
and `--node in-process` force either. Relay changes made through a running
node take effect when it next starts.

A **thin reader** has no network half at all:

    CGO_ENABLED=0 go build -tags thin -o tersereader-thin .

It links no cryptography, BLAKE3, store, WebSocket or relay client
(`TestThinReader` checks the import graph): about 5,300 lines of Go and
2.7 MB stripped, against 8,300 lines and 5.8 MB for the full program.

## Layout

The network half and the reading half are separate packages that meet only
in `internal/nodeapi`, so that they also run as two programs:

    internal/nodeapi   the seam: the Node interface and plain data types
    internal/node      the network half, implementing nodeapi.Node
    internal/client    relay connections, subscriptions, downloads, prefetch
    internal/store     blobs, entries, subscriptions, relays, nicknames
    internal/ws        a minimal WebSocket client (and server, for tests)
    internal/blake3    BLAKE3-256
    internal/terse     data formats used by both halves: entries, keys,
                       display names, profile documents, feed addresses
    internal/article   the article parser and the vectors' dump format
    internal/render    layout for a terminal: wrapping, tables, figures
    internal/local     the local node protocol: messages, framing, the socket
    internal/localnode a node served over the socket
    internal/localclient  nodeapi.Node over the socket (all a thin reader has)
    main.go, ui.go     the commands and the interactive reader
    wire.go            binds the reader to a running node or package node;
                       wire_thin.go, with -tags thin, to a running node only

`TestSeam` checks that the reading half imports nothing of the network half.

## Tests

    GOMAXPROCS=4 go test -p 2 ./...

- every file of `vectors/`: blobs, entries (and re-signing them), display
  names, profile documents, addresses, the relay session (messages encoded
  byte for byte, replies decoded), and all article cases (dumps compared);
- the client against an in-process relay that sends an invalid entry and a
  blob that does not match its ID;
- every article of `docs/manual/build/` parsed and laid out in every mode;
- the interactive reader driven in a pseudo-terminal;
- interoperability: the reference `terse-relay` on ephemeral ports and
  `terse` publishing with temporary identities (articles, a large one, a
  prefetched link, a feed reachable only through a hint, the whole manual).
  It is skipped if the reference binaries are not built; `TERSE_BIN_DIR`
  names their directory. The test switch `TERSEREADER_ALLOW_PRIVATE=1`
  lets hints point at loopback relays.
