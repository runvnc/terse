# Terse network — v0 (draft)

Status: the first, deliberately minimal step. It moves immutable blobs between
hosts through a relay and carries signed feeds of small articles. There are
no names, messages or peer-to-peer links yet.

Programs never see this protocol. They use the content device (SPEC.md
section 9); the host speaks the protocol on their behalf.

## 1. Blobs

A blob is 1 to 1,048,576 bytes. Its ID is the BLAKE3-256 hash of its bytes
(32 bytes; written as 64 lowercase hexadecimal digits). A program image is a
blob, so an image ID is a blob ID.

## 2. Host behaviour

- A host keeps a local blob store (a cache). A get is answered from the store
  when possible.
- Otherwise the host asks its relays. In v0 it fetches the whole blob, checks
  that its BLAKE3-256 hash equals the requested ID, stores it, and only then
  delivers the requested range to the program. A blob that fails the check is
  discarded and treated as not found on that relay.
- A put stores the blob locally and sends it to the host's relays.
- A host may start a program by image ID: it gets the blob, validates it as an
  image (SPEC.md section 2) and runs it.

## 3. Relay protocol

A relay is a blob store reachable over WebSocket. All messages are binary
WebSocket messages. Multi-byte integers are little-endian. `req` is a 32-bit
number chosen by the client and echoed in the reply; replies may arrive in any
order.

| Direction | Message | Layout |
|---|---|---|
| client → relay | Get | `01h`, `req` (4), `id` (32) |
| relay → client | Get reply | `81h`, `req` (4), `status` (1), then the blob bytes if `status` is 1 |
| client → relay | Put | `02h`, `req` (4), blob bytes |
| relay → client | Put reply | `82h`, `req` (4), `status` (1), `id` (32) if `status` is 1 |

Status: `1` done, `2` not found, `3` refused (too large, empty, store full,
or malformed request).

- The relay computes the ID of a put itself; the client does not send it.
- A relay must refuse blobs larger than 1,048,576 bytes and may impose a
  total store limit.
- A relay closes the connection on a message it cannot parse.

Clients do not trust relays: every blob is verified against its ID on
receipt.

## 4. Feeds

A feed is a numbered series of small signed entries from one author, each
pointing to a blob (an article). Hosts subscribe to feeds, receive new entries
as they are published, and download small articles ahead of time so that
reading never waits on the network.

### 4.1 Identity

A feed is identified by an Ed25519 public key (32 bytes; written as 64
lowercase hexadecimal digits). The host holds the secret key. Programs never
see secret keys.

### 4.2 Entry

| Offset | Size | Field |
|---|---|---|
| 0 | 32 | Feed key |
| 32 | 4 | `seq`: 1 for the first entry, then increasing by 1 |
| 36 | 4 | Time: Unix seconds, as claimed by the author |
| 40 | 1 | Kind: `1` = article |
| 41 | 3 | Size of the blob in bytes |
| 44 | 32 | Blob ID |
| 76 | 32 | Reply-to: the blob ID this entry responds to, or all zeros |
| 108 | 1 | `n`: title length in bytes, 0 to 120 |
| 109 | `n` | Title, UTF-8 |
| 109 + `n` | 64 | Signature |

The signature is Ed25519 over the 13 ASCII bytes `terse-entry-0` followed by
bytes 0 to 108 + `n` of the entry. An entry is at most 293 bytes.

An entry is valid if `n` is at most 120, its length matches `n`, the
signature verifies under the feed key (strict verification: small-order keys
and non-canonical signatures are rejected), `seq` is at least 1, the kind is known, the size is 1 to 1,048,576
and the title is valid UTF-8. Hosts and relays discard invalid entries.

If two different valid entries have the same feed key and `seq`, the one
received first is kept.

### 4.3 Relay messages

| Direction | Message | Layout |
|---|---|---|
| client → relay | Publish | `03h`, `req` (4), entry |
| relay → client | Publish reply | `83h`, `req` (4), `status` (1) |
| client → relay | Subscribe | `04h`, `req` (4), feed key (32), `after` (4) |
| relay → client | Subscribe reply | `84h`, `req` (4), `status` (1), then if `status` is 1: `highest` (4), the highest `seq` the relay holds for the feed, or 0 |
| client → relay | Unsubscribe | `05h`, `req` (4), feed key (32) |
| relay → client | Unsubscribe reply | `85h`, `req` (4), `status` (1) |
| relay → client | Entry | `90h`, entry |

- The relay verifies an entry before storing it. Publishing an entry the relay
  already holds succeeds. Publishing a different entry with a `seq` it already
  holds is refused.
- After a successful subscribe, the relay sends the reply, then every entry it
  holds for that feed with `seq` greater than `after`, in increasing order,
  and then each new entry as it is published, until the client unsubscribes
  or disconnects. Each entry is sent once. The client has caught up with the
  feed when it has received an entry whose `seq` is `highest` or greater
  (whether or not that entry is valid), or immediately if `highest` is not
  greater than `after`.
- Publish, subscribe and unsubscribe replies use status `1` (done) or `3`
  (refused). Unsubscribing from a feed that was not subscribed succeeds.
- `seq` values need not be consecutive, and the time is not checked.
- A relay may keep only the most recent entries of a feed (at least 256).

### 4.4 Host behaviour

- The host keeps a list of subscribed feeds and every valid entry it has
  received for them.
- When connected to a relay it subscribes to each feed with `after` set to
  the highest `seq` it holds.
- **Subscribing** (by the user, or by a program with the user's consent:
  SPEC.md section 9.5) adds the feed to the list and, when connected,
  subscribes to it at once the same way; for a feed the host holds nothing
  of, `after` is 0, so the relay sends every entry it holds (a relay keeps
  at least the 256 most recent of each feed, section 4.3). Unsubscribing
  removes the feed from the list and unsubscribes on the connection.
- **Automatic download:** when the host receives an entry for a subscribed
  feed whose size is at most 16,384 bytes (the user may change this limit),
  it gets the blob without being asked. If the blob's length differs from the
  entry's size, the blob is not associated with the entry. Larger blobs are
  fetched only on request.
- **Arrival order:** the host records the order in which it first stored
  each entry, and keeps that record across restarts. This is the order of
  the timeline that programs read (SPEC.md section 9.5). Unsubscribing
  removes a feed's entries from the timeline but not from the record;
  subscribing again puts them back where they were.
- Publishing an article (by the user, or by a program with the user's
  consent: SPEC.md section 9.5): the host puts the blob, then signs and
  publishes an entry with the next `seq` (one more than the highest it holds
  or has used for its own feed). The entry is stored locally like a
  received one; if the host subscribes to its own feed it is in the
  timeline at once.

### 4.5 Articles

An article is a blob of UTF-8 text that reads well as it is. Authors should
keep articles to a few kilobytes; the automatic download limit is what
keeps reading instant. A reader shows the text by these rules; text that
matches no rule is ordinary text.

**Lines.** Lines end at LF; a CR before the LF is ignored by these rules.
Blanks are spaces, tabs and CRs. A line of only blanks is a blank line.
The markers below count only at the very start of a line.

**Blocks.** The text is a sequence of blocks. Blank lines separate them,
and a block also ends before any line that it does not hold (by the table
below), which then starts the next block.

| Block | Starts with a line that | Holds |
|---|---|---|
| Heading | starts with `#` and a space | that one line, after the marker |
| Subheading | starts with `##` and a space | that one line, after the marker |
| List item | starts with `-` or `*` and a space | the line after the marker, and the ordinary lines after it |
| Preformatted | starts with four spaces or a tab (an indented line) | that line and the indented lines after it |
| Fence | starts with three backquotes (`` ``` ``) | the lines after it up to a closing line |
| Paragraph | is any other non-blank line | that line and the ordinary lines after it |

- An ordinary line is a non-blank line that starts none of the other
  blocks. After a heading, a preformatted block or a fence it starts a
  paragraph; after a list item it continues the item.
- A paragraph's or list item's lines are joined into one: each line break
  becomes a space and the blanks at the start of each line are dropped,
  also on its first line. So are the blanks after a heading's marker. A
  reader wraps them to its width.
- A preformatted block's lines are shown as written (with their leading
  blanks), in a monospaced face, not wrapped.
- A fence ends at a closing line: three backquotes followed only by
  blanks. Its lines are shown as written, blank lines included, like a
  preformatted block; the opening and closing lines are not shown. A fence
  with no closing line runs to the end of the article; one with no lines
  shows nothing. What follows the backquotes on the opening line (its info
  string) is ignored, except for `path`.
- A link is `http://` or `https://` not preceded by a letter or digit, with
  at least one character after the `//`, running to the first byte that is
  not printable ASCII or is `<`, `>` or `"`; trailing `.` `,` `;` `:` `!`
  `?` `'` are not part of it, nor is a trailing `)` or `]` when the link
  holds more of it than of its opening bracket. Links count in every block
  that shows text.
- There is no inline markup: `*`, `_` and backquotes inside a line are
  ordinary text.

**Figures.** A fence whose info string is the word `path` (after optional
blanks, and followed by a blank or the end of the line) is a figure. Its
opening line goes on with the width W, blanks and the height H, then only
blanks: decimal integers from 1 to 1024, the size of the drawing in screen
units (SPEC.md section 7). Its lines hold a path string. The figure is
drawn if the fence has a closing line and the opening line and the path
string are well formed by the rules below; otherwise its lines are shown
as a fence's. A reader that does not draw figures shows every figure as a
fence, which is why a figure reads as text too.

The path string is SVG path data with two additions:

- Commands `M` (move), `L` (line), `H` and `V` (horizontal and vertical
  line), `C` and `S` (cubic curve, and smooth cubic), `Q` and `T`
  (quadratic curve, and smooth quadratic), `Z` (close the subpath), each
  followed by its numbers, as in SVG: 2, 2, 1, 1, 6, 4, 4, 2 and 0. A
  lowercase command takes its numbers relative to the current point. More
  numbers after a command's last one repeat the command (after `M` or
  `m`, as `L` or `l`). `S` and `T` reflect the last control point of an
  immediately preceding `C` or `S` (for `S`) or `Q` or `T` (for `T`), else
  use the current point. After `Z` the current point is the subpath's
  start. Arcs (`A`) are not supported in v0.
- The first command is `M` or `m` (a first `m` is relative to (0, 0)).
- The words `fill <colour>` and `stroke <colour> [<width>]` may stand
  wherever a command may. They set how the subpaths after them are drawn:
  filled (non-zero winding) in the fill colour, then outlined in the
  stroke colour and width. Until the first such word the fill is `none`
  and the stroke is `fg` with width 1. A colour is `#rgb`, `#rrggbb`,
  `#rrggbbaa` (hexadecimal, either case; `aa` is opacity, `ff` opaque;
  the shorter forms are opaque), `none` (nothing drawn), or `fg` (the
  reader's text colour, opaque). The width is a number from 0 to 2047 in
  screen units; 0 draws no outline. A word ends the path so far: the
  numbers after a word belong to it or to a new command, never repeat the
  command before it.
- Numbers: an optional sign, then digits with an optional decimal point
  (`5`, `-0.5`, `.5`, `5.`), at most 32 characters; no exponents. Numbers
  and commands are separated by any number of spaces, tabs, line breaks
  and commas, or by nothing where that is unambiguous (`M10-5`, `L.5.5`
  is 0.5 and 0.5, `L5 5L6 6`). A word and its colour are each followed by a
  separator or the end.
- Coordinates are screen units from the figure's top left, x right and y
  down. Every number is rounded to the nearest sixteenth of a unit (halves
  away from zero) before it is used, so relative coordinates add up
  exactly. Every point a command reaches, control points included (also
  the reflected ones of `S` and `T`), lies within ±2047 units.
- Anything else (another letter, a number where none is expected, a
  command without all its numbers, a colour or width that is malformed,
  a word with no colour) makes the path string malformed.

Drawing is clipped to the figure's box. A reader may scale a figure that
is wider than its column down to fit, uniformly (widths included).

## 5. Not yet specified

- Verified range requests (so a range can be delivered without fetching the
  whole blob).
- Finding replies to an article, ranking, moderation and spam limits.
- Key rotation and several feeds per identity.
- Inline markup in articles (emphasis, inline code, link text), images
  that are not drawn from paths, and arcs in figure paths.
- Names: signed, versioned pointers from a public key and a name to a blob.
- Messages and topics.
- Relay discovery (a host is told which relay to use, or finds one on its
  own machine), relay-to-relay exchange and direct links between hosts.
- Abuse limits beyond blob size.
