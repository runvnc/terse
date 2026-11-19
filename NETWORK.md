# Terse network — v0 (draft)

Status: the first, deliberately minimal step. It moves immutable blobs between
hosts through relays, carries signed feeds of small articles with
self-declared names, lets relays mirror each other and answers simple
searches. There are no messages or peer-to-peer links between hosts yet.

Programs never see this protocol. They use the content device (SPEC.md
section 9); the host speaks the protocol on their behalf.

## 1. Blobs

A blob is 1 to 1,048,576 bytes. Its ID is the BLAKE3-256 hash of its bytes
(32 bytes; written as 64 lowercase hexadecimal digits). A program image is a
blob, so an image ID is a blob ID.

## 2. Host behaviour

- A host keeps a local blob store (a cache). A get is answered from the store
  when possible.
- Otherwise the host asks its relays (section 5). In v0 it fetches the whole blob, checks
  that its BLAKE3-256 hash equals the requested ID, stores it, and only then
  delivers the requested range to the program. A blob that fails the check is
  discarded and treated as not found on that relay. A get reply carries the
  whole blob, so a host learns a blob's size only from an entry that points
  at it or by downloading it.
- A put stores the blob locally and sends it to every relay the host is
  connected to (section 5).
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

Message types `01h`–`08h` are requests, `81h`–`86h` and `88h` replies and
`90h` a pushed entry (sections 4.3, 4.7, 4.8, 5).

- The relay computes the ID of a put itself; the client does not send it.
- A relay must refuse blobs larger than 1,048,576 bytes and may impose a
  total store limit.
- A relay closes the connection on a message of a known type that it cannot
  parse, or on a text message. It ignores a message whose type it does not
  know, and a reply or pushed entry from a connection that is not a peer
  (section 5). (Relays before this revision closed the connection on an
  unknown type.)

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
| 40 | 1 | Kind: `1` = article, `2` = profile (section 4.6), `3` = program, `4` = document |
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
and non-canonical signatures are rejected), `seq` is at least 1, the kind is
known, the size is 1 to 1,048,576 (1 to 4,096 for a profile), the title is
valid UTF-8, and, for a profile or a program, the title is a valid display
name (section 4.6). Hosts and relays discard invalid entries.

A **program** entry (kind 3) publishes a program: its title is the
program's name, its blob a Terse image (SPEC.md section 2). The entry is
valid whether or not the blob is a valid image; that is found out when the
program is started. A feed's **current version** of a program is its valid
program entry with that name and the highest `seq` (of those a host or
relay holds). A program's identity is (feed key, name) (SPEC.md section
12.1).

A **document** entry (kind 4) publishes a document (SPEC.md section 9.7):
its title is the document's name (any title, as for an article), its blob
a document. The entry is valid whether or not the blob starts with a
document header; a host that publishes one for a program checks it first
(SPEC.md section 9.5).

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
- A relay may keep only the most recent entries of a feed (at least 256),
  but always keeps the feed's latest profile entry (the one with the highest
  `seq`, section 4.6), however old.

### 4.4 Host behaviour

- The host keeps a list of subscribed feeds and every valid entry it has
  received for them.
- When connected to a relay it subscribes to each feed with `after` set to
  the highest `seq` it holds (with several relays, on each: section 5).
- **Subscribing** (by the user, or by a program with the user's consent:
  SPEC.md section 9.5) adds the feed to the list and, when connected,
  subscribes to it at once the same way; for a feed the host holds nothing
  of, `after` is 0, so the relay sends every entry it holds (a relay keeps
  at least the 256 most recent of each feed, section 4.3). Unsubscribing
  removes the feed from the list and unsubscribes on the connection.
- **Automatic download:** when the host receives an entry for a subscribed
  feed whose size is at most 16,384 bytes (the user may change this limit),
  it gets the blob without being asked. This applies to profile and
  document entries too. If the blob's length differs from the entry's size, the blob is not
  associated with the entry. Larger blobs are fetched only on request.
- **Programs** are downloaded differently: once a subscribed feed is caught
  up (section 4.3), and after that whenever one of its program entries
  arrives, the host gets the image of the current version of each of the
  feed's programs that it does not hold, whatever the article limit, if
  its size is at most 131,136 bytes (the largest image); not older
  versions. The user may turn this off.
- **Prefetch:** when the host stores an article blob through automatic
  download for an article entry (kind 1) of a subscribed feed, it scans the
  text for `terse:` article links (section 4.5, also the targets of links
  with text), takes the first 8 distinct
  ones whose blobs it does not hold and is not already fetching, and gets
  them. It skips a link when an entry it holds gives the blob a size over
  the automatic download limit, and discards a fetched blob over that
  limit. Program links (`terse:program/`, section 4.5) are not article
  links and are never prefetched. A host does not prefetch a blob that a
  program entry (kind 3) it holds names, since that blob is an image; it
  cannot tell that any other linked blob is an image (or a document)
  before it has fetched it, so such a blob is fetched like an article,
  within the limit. Documents are not scanned for links. Prefetched blobs
  are not scanned. The user may turn prefetch off.
- **Arrival order:** the host records the order in which it first stored
  each entry, and keeps that record across restarts. This is the order of
  the timeline that programs read (SPEC.md section 9.5), which holds only
  article and document entries (kinds 1 and 4), not profiles or
  programs. Unsubscribing
  removes a feed's entries from the timeline but not from the record;
  subscribing again puts them back where they were.
- Publishing an article or a document (by the user, or by a program with
  the user's consent: SPEC.md section 9.5): the host puts the blob, then signs and
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
  holds more of it than of its opening bracket. A link is also `terse:`
  followed by 64 lowercase hexadecimal digits (an article, by blob ID) or
  `terse:feed/` followed by 64 lowercase hexadecimal digits (a feed key),
  not preceded by a letter or digit, with the digits not followed by a
  letter or digit. A link is also `terse:program/`, 64 lowercase
  hexadecimal digits (the publisher's feed key, all zero for a built-in
  program), `/` and the program's name, not preceded by a letter or digit:
  the name is the run of bytes `A`–`Z`, `a`–`z`, `0`–`9`, `-`, `.`, `_`,
  `~` and `%` followed by two hexadecimal digits (either case) that
  follows the `/`, less any `.` at its end; each `%XX` stands for the byte XX,
  every other byte for itself, and the bytes so decoded must be a valid
  display name (section 4.6), else the text is not a link. So a name's
  other bytes (a space, non-ASCII letters, a final dot) are written as
  `%XX`. Links count in every block that shows text.
- A **link with text** is `[`, the text, `]`, `(`, the target, `)`, all
  on one line: the text is 1 to 200 bytes holding no `[`, `]` or link,
  and the target is a link by the rules above that is the whole of what is
  between the parentheses (so it holds no blank and no `)`). A reader
  shows the text as the link and does not show the target. Links are found
  from the start of the line: where two would overlap, the one that starts
  first counts, and a link with text starts at its `[`. Brackets that are
  not all of a link with text are ordinary text, and the links in them
  count as bare links.
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

### 4.6 Profiles and nicknames

A **profile** is an entry of kind `2` in a feed. Its title is the feed's
**display name**; its blob is the **profile document**. The profile in
force for a feed is its valid profile entry with the highest `seq`.

A **display name** is 1 to 48 bytes of UTF-8 with no control characters
(U+0000–U+001F, U+007F–U+009F) and no whitespace (the Unicode White_Space
property: U+0009–U+000D, U+0020, U+0085, U+00A0, U+1680, U+2000–U+200A,
U+2028, U+2029, U+202F, U+205F, U+3000) as its first or last character. A
profile entry whose title is not a display name is invalid (section 4.2).

The **profile document** is UTF-8 text of at most 4,096 bytes. Lines end
with `0Ah`. It starts with header lines `field: value` (the field, a colon,
one space, the value), then optionally an empty line and a free-text
description, which is the rest of the document. A header line without `: `
makes the document invalid. Fields in v0:

| Field | Value |
|---|---|
| `listed` | `yes`: the name may be found by search (section 4.7). Any other value, or no `listed` line, means not listed. |
| `relay` | A `ws://` or `wss://` URL of a relay where the author publishes. Zero to four lines; readers take the first four that are URLs and skip other values. |

Unknown fields are ignored. A host that writes a profile for its user puts
a `relay` line for each of its relays that is not a loopback address.

Names are self-declared and **not unique**: anyone can declare any name.
Anything shown to a user that names a feed shows the feed key (or a short
form of it, for example its first and last six hex digits) beside the name.

A **nickname** is a name the user gives a feed (the same rules as a display
name). The host keeps nicknames in its own configuration and never sends
them anywhere. When showing a feed, a nickname takes precedence over the
declared name.

### 4.7 Search

| Direction | Message | Layout |
|---|---|---|
| client → relay | Find | `06h`, `req` (4), `scope` (1), `n` (1), query (`n` bytes) |
| relay → client | Find reply | `86h`, `req` (4), `status` (1), `count` (1), then `count` times: `length` (2), entry (`length` bytes, complete with its signature) |

- `scope` `1` searches feeds by display name; `2` searches article and
  document titles;
  `3` searches program names. The query is 1 to 48 bytes of UTF-8. A find with another scope or a bad
  query is refused (status `3`, `count` 0).
- **Matching:** the query must occur as a contiguous run of bytes in the
  name or title, after the ASCII letters A–Z are changed to a–z on both
  sides. Nothing else is folded.
- **Scope 1** returns the profile in force of each feed whose profile
  document says `listed: yes` and whose display name matches: names that
  start with the query (same folding) first, then the others, each group in
  increasing order of feed key.
- **Scope 2** returns article and document entries (kinds 1 and 4) of
  feeds with a listed
  profile in force whose title matches, newest first by the order in which
  the relay stored them (not by the time the author claims).
- **Scope 3** returns program entries (kind 3) of feeds with a listed
  profile in force whose name matches, current versions only (the relay
  holds no program entry of that feed and name with a higher `seq`), newest
  first by the order in which the relay stored them.
- At most 16 results. A relay answers from what it holds; it needs the
  profile document to know `listed`, so a profile whose document it does
  not hold, or cannot parse, counts as not listed.
- A relay may refuse a find (status `3`, `count` 0) from a connection that
  searches too often.
- **Clients** ask all their relays and merge the replies: they verify every
  returned entry and drop invalid ones, ones of the wrong kind and ones that
  do not match the query, and keep one entry per feed and `seq`. Scope 1:
  only the highest `seq` of each feed, ordered by the rules above. Scopes 2
  and 3: by rank, the first result of each relay (in the order of the
  host's relays), then the second of each, and so on; for scope 3, a
  program is then dropped if another result has the same feed and name and
  a higher `seq`. They keep the first 16. A
  search that no relay answered and at least one refused is refused.

### 4.8 Lookup

| Direction | Message | Layout |
|---|---|---|
| client → relay | Lookup | `08h`, `req` (4), feed key (32), `kind` (1), `n` (1), name (`n` bytes) |
| relay → client | Lookup reply | `88h`, `req` (4), `status` (1), then if `status` is 1: the entry (complete, with its signature) |

- In v0 `kind` must be `3` (a program) and the name a valid display name
  (section 4.6); otherwise the lookup is refused (status `3`).
- The relay answers with the feed's current version of the program of
  that name that it holds (its program entry of that name with the highest
  `seq`, section 4.2), status `1`, or status `2` if it holds none. It
  answers whether or not the feed has a listed profile, or any profile.
- A relay may refuse a lookup (status `3`) from a connection that asks
  too often; lookups and finds may share one allowance.
- **Hosts** ask all their connected relays, verify every entry returned
  and drop one that is invalid or not of the feed, kind and name asked
  for, and take the one with the highest `seq`. The entry is stored like
  a received one (it is not added to the subscriptions). A host asks when
  a shell starts a program it holds no entry for (SPEC.md section 12.2
  `start_program`), and then gets the image.

## 5. Several relays

### 5.1 Hosts

A host's relays are those it is configured with (at most eight) and, for
each subscribed feed whose profile in force has `relay` lines, up to two of
those (the first two that are not already among the host's own relays);
**feed relays** may be turned off. A host keeps at most 16 relay
connections. It ignores a `relay` line whose host is `localhost`, a name
under `.localhost`, or an address that is loopback, unspecified, private
(10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10, fc00::/7) or
link-local (169.254.0.0/16, fe80::/10), and does not connect if a host
name resolves to any such address; it connects only to the addresses it
checked, without resolving the name again. A host connects to all its
relays and, when a connection fails or drops, tries again after 1 s,
doubling the wait up to 30 s. A relay that is still connecting does not
hold up requests: they go to the relays that are connected. Only while
none is connected does a request wait, for the first connection (and
briefly for the others).

- **Put and publish** go to every connected relay. They succeed when at
  least one relay accepts. When publishing an article, the entry is
  published once a relay has accepted the blob.
- **Subscribe** goes to every connected relay that serves the feed (all of
  the host's own relays, and the feed relays named in the feed's profile),
  each with `after` set to the highest `seq` the host holds. An entry
  received from several relays is stored once (the first received of a
  `seq` wins, section 4.2).
- **Get** asks the connected relays in turn: first the feed relays named in
  the profile in force of a feed with an entry pointing at the blob, then
  the rest, two at a time. The blob is not found only when every connected
  relay has said so or failed.
- **Find** and **lookup** ask every connected relay (sections 4.7, 4.8).

### 5.2 Relays that mirror each other

| Direction | Message | Layout |
|---|---|---|
| relay → relay | Peer | `07h` (no body, no reply) |

A relay may be given other relays as **peers**. It connects to each, sends
Peer, and keeps the connection, reconnecting after 1 s, doubling up to
30 s. A relay that receives Peer marks that connection as a peer link. On
a peer link both sides may send requests and replies; requests use the
ordinary messages.

- Every entry the relay newly stores (from a client or from a peer) is
  sent with Publish to every peer except the one it came from. So is every
  blob up to 16,384 bytes it newly stores, with Put. Larger blobs are not
  pushed. Peers are not sent the pushed-entry message (`90h`) for new
  entries.
- When a client (not a peer) subscribes to a feed, the relay subscribes to
  it on each peer, once per peer link, with `after` set to its own highest
  `seq` for the feed, so the peer's backlog flows in; entries a peer pushes
  are stored and passed on like published ones. A new peer link is
  subscribed to every feed clients are subscribed to.
- A get from a client that the relay cannot serve from its store is asked
  of every peer at once; the first reply with a blob that matches the ID
  wins and is stored (if the store has room) and served. If every peer
  says not found or fails, or none answers within 3 s, the reply is "not
  found". A get from a peer is answered from the relay's store only.
- Find and lookup are answered from the relay's own store only.

Because a relay passes on only what it newly stores, an entry crosses each
peer link at most once in each direction.

## 6. Not yet specified

- Verified range requests (so a range can be delivered without fetching the
  whole blob).
- Finding replies to an article, ranking, moderation and spam limits.
- Key rotation and several feeds per identity.
- Inline markup in articles (emphasis, inline code, link text), images
  that are not drawn from paths, and arcs in figure paths.
- Named pointers: signed, versioned pointers from a public key and a name
  to a blob (not the display names of section 4.6).
- Messages and topics.
- A relay keeping the current version of every program of a feed however
  old (it keeps only the latest profile so), so a lookup can miss an old
  program of a busy feed. Lookups of kinds other than programs.
- Sharding content among relays by hash, so that each relay holds only part
  of it.
- Relay discovery beyond configuration, the local relay and the `relay`
  lines of profiles.
- Direct connections between hosts.
- Abuse limits beyond blob size and the search rate: on publishing, on
  peers.
