# Spec findings from a clean-room reader

`tersereader` was written from `NETWORK.md`, `ARTICLE.md`, `vectors/` (with
its README) and the manual's `native-reader.txt`, `writing-articles.txt` and
`network.txt`, without reading the reference implementation. This file lists
every place where those documents did not settle a question: the section,
the question, and what this reader assumed. At the end, after the reader
was finished, a few assumptions were checked against the reference binaries
as black boxes; the outcome is noted under each.

Overall: **nothing was blocking.** The article format, with its vectors,
was implemented from the text with no guessing that the vectors did not
then confirm. All 56 article vectors matched exactly on the first run, and
so did the entry, profile, address, blob and relay-session vectors. The
reader synced, verified and showed what the reference CLI published, through
the reference relay, also on the first run. The questions below are about
details at the edges.

## Resolved

Every finding below has since been settled in the documents, and both
implementations (the Rust reference and this reader) follow the written
rule:

| # | Decision | Where | Vectors | Code changed |
|---|---|---|---|---|
| 1 | Strict verification is four listed checks: canonical, non-small-order A; S < L; R decodes and is not of small order; the canonical encoding of [S]B − [k]A equals R's bytes (cofactorless) | NETWORK.md 4.2 | entries.txt: `small-order-key-signed`, `small-order-key-order-8-signed`, `non-canonical-key`, `small-order-r`, `non-canonical-r`, `mixed-order-r` | Rust: a key must be canonical (was accepted with y ≥ p). Go: tests only (it already followed the rule) |
| 2 | RFC 8032 decoding: y < p, no sign bit on x = 0; a small-order component is allowed | NETWORK.md 4.1 | addresses.txt: `key-not-canonical`, `key-mixed-order` | Rust (as 1) |
| 3 | Hints: no own relay, all failed, or every connected own relay answered `highest` 0 or refused; relays still connecting do not hold it up; an own relay later answering `highest` > 0 stops the hints | NETWORK.md 4.9 | — | Rust: a refusal counts as holding none. Go: all three parts |
| 4 | Not associated: not held for the entry, not scanned, not fetched again; may be kept; opens by its ID | NETWORK.md 4.4 | — | none (both conform) |
| 5 | `ws://`/`wss://`, then non-empty, not starting with `/`, no control or whitespace character | NETWORK.md 4.6 | profiles.txt: `relay-values` | Rust: `/` and control characters. Go: Unicode whitespace, `/` |
| 6 | The first 8 links not held and not being fetched; skipped ones (over the limit, program blobs) count | NETWORK.md 4.4 | — | Go |
| 7 | At most two outstanding (sliding); 10 s per relay; wait until none connecting, or one connected and 200 ms since the request began to wait; fail at once with none connecting | NETWORK.md 5.1 | — | Go |
| 8 | Path as written or `/`; no subprotocol; one binary message each; fragments reassembled; at least 1,048,582 bytes accepted | NETWORK.md 3 | — | none |
| 9 | Feed relays recomputed from the profile in force; relays no longer named stop serving the feed; a profile whose document is not held names none | NETWORK.md 5.1 | — | Go |
| 10 | Coarse surfaces may thicken, threshold, reduce colours, drop a monochrome panel; must keep size, positions, order, clipping; text fallback always conforms | ARTICLE.md 6.3 | — | none |
| 11 | Monospaced body text: 68 cells are 512 units | ARTICLE.md 6.2 | — | none |
| 12 | A width is a number: `+` allowed, a command may follow directly | ARTICLE.md 6.1 | articles: `figures-width-forms` | none |
| 13 | A lone `M` moves, draws nothing, and is dumped | ARTICLE.md 6.3 | articles: `figures-lone-moves` | none |
| 14 | The first block, if it is a heading | ARTICLE.md 4.4, 9 | — | none |
| 15 | Plain text: an empty line or a line of `-` after the header row | ARTICLE.md 2.5 | — | none |
| 16 | Titles and names by the text rules of section 1, on one line | ARTICLE.md 9 | — | Go: titles were printed raw |
| 17 | The timeline keeps documents (programs read it); a reader may leave them out of what it shows | NETWORK.md 4.4 | — | none |
| 18 | Capital hex digits are an optional typing aid | NETWORK.md 4.9 | — | none |
| 19 | A host ends at the first `:` outside brackets; reason names are one order of checking | NETWORK.md 4.9 | — | none |
| 20 | A pushed valid entry of any subscribed feed is kept, whichever relay sent it | NETWORK.md 5.1 | — | Go |

## Had to guess (behaviour could differ between implementations)

1. **NETWORK.md 4.2, "strict verification".** The text says small-order
   keys and non-canonical signatures are rejected, but does not define
   strict verification fully. Open questions: is a signature whose R is
   of small order refused? Is a key or R whose y coordinate is not
   canonical (y >= p) refused? Is the check cofactored or cofactorless?
   The vectors cover only a small-order key (the neutral point) and S >= L.
   *Assumed:* the rules of ed25519-dalek's `verify_strict`: refuse a
   small-order A, a small-order R, S >= L, and non-canonical point
   encodings, and check the cofactorless equation. *Checked at the end:*
   the reference relay **refuses** an entry whose signature has R = the
   neutral point and otherwise satisfies the cofactorless equation. The
   guess was right. This one matters: Go's `crypto/ed25519.Verify`
   **accepts** that signature, so an implementer who uses a standard
   library as it is would keep entries that relays and other clients
   refuse. *Suggestion:* list the checks of strict verification in 4.2,
   and add vectors for a small-order R and a non-canonical A.

2. **NETWORK.md 4.1, "a valid feed key ... decode to an Ed25519 point".**
   Is a non-canonical encoding (y >= p, or x = 0 with the sign bit set)
   a point that decodes? *Assumed:* no (RFC 8032 decoding). There are no
   vectors for it.

3. **NETWORK.md 4.9 and 5.1, when hints are used.** "If none of its own
   relays holds an entry of the feed (every connected one answers the
   subscribe with highest 0, ...)". Questions: how long does a client wait
   for relays that are still connecting before it decides? Does a refused
   subscribe (status 3) count as "holds none"? If an own relay later gets
   entries, does the client stop using the hints? *Assumed:* decide once
   every own relay has either failed to connect or answered the subscribe;
   a refusal counts as holding none; the hints stay in use until the
   profile in force has `relay` lines. *Not checked against the reference.*

4. **NETWORK.md 4.4, a blob "not associated with the entry".** When a
   blob's length differs from the entry's size, should the client keep the
   blob (its hash is right), and what does it show for the entry? *Assumed:*
   the blob is stored, since it is valid by its ID, but the entry is
   treated as not having it (not "downloaded" in listings), and opening a
   link to the blob still shows it.

5. **NETWORK.md 4.6, "readers take the first four that are URLs".** What
   is a URL here? The vectors show only that `http://` is skipped.
   *Assumed:* `ws://` or `wss://`, a non-empty host, no blanks or control
   characters.

6. **NETWORK.md 4.4, prefetch "takes the first 8 distinct ones".** Do the
   links it skips (held, being fetched, over the limit by an entry, named
   by a program entry) count towards the 8? *Assumed:* no, the 8 are the
   blobs actually requested.

7. **NETWORK.md 5.1, get "two at a time".** Two relays asked at once, then
   the next two after both answer, or after either? And how long is "briefly
   for the others" when no relay is connected yet? *Assumed:* ask two
   together, wait for both, then the next two; wait 300 ms for the others
   after the first connection.

8. **NETWORK.md 3, the WebSocket itself.** The path of a relay URL given as
   `ws://host:port`, a subprotocol, and whether relays fragment messages
   are not stated. *Assumed:* path `/`, no subprotocol, fragmented messages
   reassembled. This worked with the reference relay.

9. **NETWORK.md 5.1, feed relays when a profile changes.** If a newer
   profile names other relays, does the client stop following the feed on
   the old ones? *Assumed:* new ones are added; hint relays are dropped;
   old profile relays are kept until restart.

10. **ARTICLE.md 6.2 and 6.3, drawing on a coarse grid.** The figure rules
    assume a surface that can draw 1-unit strokes. A terminal has about
    0.27 braille dots per unit at the recommended width, so a 1-unit stroke
    falls between dots. *Assumed:* a reader may approximate ("may smooth
    edges as it likes"): strokes are thickened to stay visible, a dot is set
    where the colour is at least a quarter opaque, a cell shows one colour
    over one background colour, and in monochrome output a colour covering
    most of the box is taken as a panel and left blank. Strictly, "every
    other colour is as written: a reader does not change it" is not met
    by a 2-colour-per-cell grid. *Suggestion:* say what a reader whose
    resolution is far below a unit may do, or that the fallback (the
    figure's text) is then preferred.

11. **ARTICLE.md 6.2, "about 68 characters of body text".** For a monospaced
    terminal, are 68 cells 512 units? The manual also says the reference
    reader's column holds about 64 monospaced characters. *Assumed:* 68
    cells are 512 units.

12. **ARTICLE.md 6.1, a stroke width followed directly by a command letter**
    (`stroke fg 2M0 0`), and a width with a `+` sign. The text says a word
    and its colour are followed by a separator or the end, but says nothing
    of the width. *Assumed:* both are allowed, like any number.

13. **ARTICLE.md 6.3 and the dump format, an `M` with no segment after it**
    (`M1 1 M2 2 L3 3`). Is the first `M` part of the path's commands as
    dumped? *Assumed:* yes, each `M` is written out. No vector covers it.

## Minor wording

14. **ARTICLE.md 9 and 4.4, the title of an article without an entry.**
    Section 9 says "the article's first line ... if it is a heading", while
    4.4 says "the article's first heading". *Assumed:* the first block, if
    it is a heading (section 9's reading).

15. **ARTICLE.md 2.5, showing the header "apart".** For plain text (screen
    readers) there is no bold and no rule that is not noise. *Assumed:*
    the header row followed by an empty line.

16. **Titles and names as text.** An article's title may hold control
    characters (any UTF-8). ARTICLE.md 1 says how to show an article's
    control characters, but not a title's. *Assumed:* the same rules.

17. **NETWORK.md 4.4, the timeline holds documents (kind 4).** A reader
    without the machine cannot open them, so it lists things it cannot
    open (as "document, cannot open"). Consistent with section 6, but it
    reads oddly that the core timeline holds a machine-only kind.

18. **vectors/README, typing aids.** It allows "a key with capital digits"
    as a typing aid, which NETWORK.md 4.9 does not mention. This reader
    does not accept capitals.

19. **NETWORK.md 4.9 grammar and the vector reasons.** The order in which a
    malformed address is diagnosed (a host checked before its port, so
    `2001:db8::1` without brackets is `bad-ipv4`; a final dot is `bad-name`
    although the last label then "holds no letter") comes only from the
    vectors. Harmless, since only the reason names differ, but the grammar
    alone does not say that a host ends at the first `:`.

20. **Entries for a feed not subscribed on that connection.** Not stated;
    this reader ignores them.

## What the documents got right

- `native-reader.txt` is a good map: it names the four messages a reader
  needs and points at section 6 for the rest.
- The article vectors are complete enough that a reader could be written
  against them alone; every near miss in them is a real edge (hint lists
  ending in punctuation, `\|` counted as two bytes, a `]` that closes an
  IPv6 address, reflected control points after `Z`).
- The relay-session transcript made the message encodings checkable without
  a relay.
