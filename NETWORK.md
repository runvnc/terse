# Terse network — v0 (draft)

**Terse in three layers.** (a) The Terse network, this document: blobs,
relays, feeds, entries, profiles and names, search. (b) Terse articles,
`ARTICLE.md`: the article format and what a reader must show for it.
(a) and (b) are **the core**: a native reader that implements them, with
no virtual machine, is a complete, conforming Terse client (section 6
says what it must do). (c) The Terse machine, `SPEC.md` (with its device documents, `spec/`) and `STATE.md`:
an optional layer that reuses the core. Nothing in this document depends
on (c); what the network means for a machine host is collected in
section 7, which a reader implementer can skip.

Status: the first, deliberately minimal step. It moves immutable blobs
between clients through relays, carries signed feeds of small articles
with self-declared names, lets relays mirror each other and answers
simple searches. There are no messages or peer-to-peer links between
clients yet.

Test vectors for this document (blob IDs, entries signed with a public
test key and invalid ones with the rule each breaks, profile documents
and names, feed addresses, and the messages of a relay session) are in
`vectors/`, described in `vectors/README`.

A **client** is a program that speaks this protocol to relays for a
person: a native reader, a machine host (which speaks it for the
programs it runs), a command line tool. A **relay** is a server that
stores blobs and entries and passes them on (section 3).

## 1. Blobs

A blob is 1 to 1,048,576 bytes. Its ID is the BLAKE3-256 hash of its bytes
(32 bytes; written as 64 lowercase hexadecimal digits).

### 1.1 Kinds of blob

The network carries any bytes; what a blob holds is told by the entry
that points at it (section 4.2) and, for a blob reached without one (by
an article link, `ARTICLE.md` section 4), by its first bytes:

- a blob that starts with the four bytes `TERS` is a **program image**
  for the Terse machine (`SPEC.md` §image);
- a blob that starts with the four bytes `TDOC` is a **document**, data
  for a program (`spec/content.md` §content.documents);
- any other blob reached as an article is an **article**, UTF-8 text
  (`ARTICLE.md`).

A client that does not run programs needs only these four bytes to know
that it cannot open the first two kinds (section 6).

## 2. Client behaviour

- A client keeps a local blob store (a cache). It answers a want for a
  blob, or a part of one, from the store when possible, without asking
  any relay and without waiting for one to connect or answer (section
  6.1).
- Otherwise it asks its relays (section 5). In v0 it fetches the whole
  blob, checks that its BLAKE3-256 hash equals the requested ID, stores
  it, and only then uses it (or the part wanted). A blob that fails the
  check is discarded and treated as not found on that relay. A get reply
  carries the whole blob, so a client learns a blob's size only from an
  entry that points at it or by downloading it.
- A put stores the blob locally and sends it to every relay the client is
  connected to (section 5).

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

Status: `1` done, `2` not found, `3` and above refused (section 3.1 says
why).

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

**The WebSocket itself** (RFC 6455):

- A relay URL is `wss://` (encrypted: WebSocket over TLS) or `ws://`
  (plain), a host, an optional port (the scheme's default when left
  out, as for any WebSocket URL) and an optional path. The client opens
  the WebSocket with the URL's path and query as written, or `/` when it
  has none. A relay accepts any path.
- **Encrypted by default.** A relay address written without a scheme
  (typed by a person, given on a command line, in a configuration, or a
  hint, section 4.9) names an encrypted relay: `wss://` and the address,
  on the default port of `wss://` when it gives none. A port, when
  written, is only a port: it says nothing about encryption. A plain
  relay is written with its scheme, `ws://host:port`; it is meant for
  the local machine, private networks and tests. The one exception: an
  address without a scheme whose host is `localhost` or a loopback IP
  address (127.0.0.0/8, `::1`) names a plain relay (`ws://`) on that
  host, since its traffic never leaves the machine; written `wss://`, it
  is encrypted. A client never takes `localhost` or a loopback address
  from the network (section 5.1), so the exception cannot turn a relay
  named by someone else into a plain one.
- **No downgrade.** A client connects to a relay without encryption
  only when its own user wrote that relay as a `ws://` relay (or as a
  loopback address without a scheme): in its relay list, on its command
  line, or in its settings. A relay named by the network (a `relay` line
  of a profile, section 4.6, or a hint, section 4.9) that is plain is
  not used unless the user wrote that same relay so; one that is
  encrypted is never reached by a plain connection instead, and a
  failed TLS handshake is never retried without TLS.
- Over `wss://` the client checks the relay's certificate as a web
  browser does for `https://`: it must chain to a certificate authority
  the client trusts and be for the URL's host (RFC 6125). A client does
  not connect when the check fails, and offers no way to skip it: no
  setting, option or variable makes it accept any certificate. TLS hides
  from the network which feeds and blobs a client asks for; it adds
  nothing to their integrity, which the signatures and hashes ensure
  (sections 1, 4.2).
- **Overrides come only from the user.** So that a person or an
  organisation can run relays that the public authorities do not vouch
  for, a client may let its user:
  - trust more certificate authorities, for every relay or for named
    hosts only;
  - pin a host to one authority, or to one certificate or public key
    (by its fingerprint): then nothing else is accepted for that host, a
    certificate from a public authority included, and a client that
    refuses one says that it does not match what is pinned;
  - write a relay as plain (`ws://`), as above.

  An override names the host it applies to, or, for an added authority,
  applies to every relay; none turns the check off, and none applies to a
  host it does not name. Overrides come only from the user's own
  settings and command line. Nothing received from the network (a hint,
  a `relay` line of a profile, a link, an entry, an article) carries an
  override, adds a trusted authority or a plain connection, or makes a
  client use one; a relay named by the network is checked with whatever
  the user set for its host, like any other. How a client keeps these
  settings is its own (README "Private authorities and pins" has the
  reference's).
- No subprotocol is used: a client sends no `Sec-WebSocket-Protocol`
  header, and a relay chooses none.
- Every message of this protocol is one binary WebSocket message. A
  sender should send each in one frame; a receiver must accept one split
  into fragments and reassemble it. Each side must accept messages of up
  to 1,048,582 bytes (the largest in this protocol: a get reply with the
  largest blob) and may close the connection on a larger one.
- Each side answers pings as RFC 6455 says. A relay may ping a
  connection that has been silent for a while (one reached through a
  proxy that closes idle connections should, every 30 s or more often);
  a client need not send any.
- A relay may refuse a connection at the handshake with HTTP status 429
  (too many connections from the client's address) or 503 (too many
  connections in all): the client treats it as a failed connection and
  tries again as section 5.1 says.
- A client should support both. One that supports only `ws://` treats a
  `wss://` relay as a relay that cannot be reached (section 5.1).

### 3.1 Refusals

A relay that refuses a request says why by the status of its reply, in
every reply that has a status. The layout of a refused reply is the same
whatever the status: nothing follows the status (no blob, ID, `highest`
or entry; a find reply has `count` 0).

| Status | The relay refused because | A client tells its user, for example |
|---|---|---|
| `3` | of a reason it does not give: too large, empty, malformed, an invalid entry, a different entry with that `seq` (section 4.3), or any other | "refused it" |
| `4` | the feed has reached this relay's limit and the entry would be dropped at once (section 5.3) | "this feed is over the relay's limit" |
| `5` | the relay is full | "the relay is full" |
| `6` | the feed is not on this relay's list of writers (section 5.3) | "this feed is not on the relay's list of writers" |
| `7` to `63` | not defined by this revision: a reason the client cannot name | "refused it" |
| `64` to `183` | too many requests from this address: try again in *s* − 63 seconds (1 to 120) | "too many requests from this address; try again in 30 s" |
| `184` to `255` | the same, in *s* − 181 minutes (3 to 74) | "... try again in 10 min" |

- A relay rounds a wait up to what the status can say (a wait of 121 s
  is status `184`, three minutes; a longer one than 74 minutes is
  `255`). A client does not repeat the request before the wait has
  passed; it need not repeat it at all.
- Relays before this revision sent only `3`, and clients before it read
  every status but `1` and `2` as a refusal: a client reads a status it
  does not know as `3`. Gets and subscribes are not refused for the
  reasons of statuses `4` to `255`.

## 4. Feeds

A feed is a numbered series of small signed entries from one author, each
pointing to a blob (an article). Clients subscribe to feeds, receive new
entries as they are published, and download small articles ahead of time
so that reading never waits on the network.

### 4.1 Identity

A feed is identified by an Ed25519 public key (32 bytes; written as 64
lowercase hexadecimal digits). A **valid feed key** is 32 bytes that
are the canonical encoding of an Ed25519 point that is not of small
order:

- **Decoding** (RFC 8032 section 5.1.3): bit 255 (the top bit of the
  last byte) is the sign of x; the other 255 bits, little-endian, are y.
  The bytes decode only if y < p (p = 2^255 − 19), the curve equation
  gives an x for that y, and x is not 0 while the sign bit is 1. So an
  encoding with y ≥ p is refused even where y − p is the y of a point:
  each point has exactly one encoding. (Equivalently: the bytes decode,
  and encoding the point again gives the same 32 bytes.)
- **Small order**: P is of small order if [8]P is the neutral point
  (there are eight such points). A point that is not of small order is
  accepted whether or not it lies in the prime-order subgroup (a key
  with a small-order component is valid).

`vectors/addresses.txt` has a non-canonical key (`key-not-canonical`)
and a valid key with a small-order component (`key-mixed-order`). The
author's client holds the secret key and never sends it anywhere.

### 4.2 Entry

| Offset | Size | Field |
|---|---|---|
| 0 | 32 | Feed key |
| 32 | 4 | `seq`: 1 for the first entry, then increasing by 1 |
| 36 | 4 | Time: Unix seconds, as claimed by the author |
| 40 | 1 | Kind: `1` = article, `2` = profile (section 4.6), `3` = program, `4` = document; `0` and `255` are reserved; `5` to `254` are not defined by this revision |
| 41 | 3 | Size of the blob in bytes |
| 44 | 32 | Blob ID |
| 76 | 32 | Reply-to: the blob ID this entry responds to, or all zeros |
| 108 | 1 | `n`: title length in bytes, 0 to 120 |
| 109 | `n` | Title, UTF-8 |
| 109 + `n` | 64 | Signature |

The signature is Ed25519 over the 13 ASCII bytes `terse-entry-0` followed by
bytes 0 to 108 + `n` of the entry. An entry is at most 293 bytes.

**Strict verification.** The signature is R (its first 32 bytes) and S
(its last 32, a little-endian integer of all 256 bits: no bit is
ignored); A is the feed key's 32 bytes, M the signed message. B is the
base point of RFC 8032 (section 5.1), [n]P the point P added to itself n
times, and a point is **of small order** if [8]P is the neutral point
(x = 0, y = 1). It verifies if and only if all of these hold:

1. A is a valid feed key (section 4.1): canonical, and not of small
   order.
2. S < L, where L = 2^252 + 27742317777372353535851937790883648493 (the
   order of B).
3. R decodes to a point that is not of small order. Here R may be
   decoded as A is (section 4.1) or more loosely (y taken modulo p, x = 0
   accepted with the sign bit set): the verdict is the same, since every
   R that only the loose decoding accepts either is of small order or
   fails step 4.
4. With k = SHA-512(R ‖ A ‖ M), its 64 bytes taken as a little-endian
   integer modulo L (R and A as the bytes received), the canonical
   encoding of the point [S]B − [k]A is exactly the 32 bytes R. This is
   the cofactorless equation: it is not multiplied by 8. A non-canonical
   R never verifies, since the encoding computed is canonical.

The order of the checks does not change the verdict. There are eight
points of small order; their canonical encodings, in hexadecimal, are

    0100000000000000000000000000000000000000000000000000000000000000  order 1 (neutral)
    ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f  order 2
    0000000000000000000000000000000000000000000000000000000000000000  order 4
    0000000000000000000000000000000000000000000000000000000000000080  order 4
    26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05  order 8
    26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85  order 8
    c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a  order 8
    c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa  order 8

and a loose decoder also takes these six to points of small order:
`eeff…ff7f` and `eeff…ffff` (y = p + 1), `edff…ff7f` and `edff…ffff`
(y = p), `0100…0080` and `ecff…ffff` (x = 0 with the sign bit set).
Comparing with these fourteen byte strings is an alternative to
computing [8]P.

**Why so strict.** Ed25519 as RFC 8032 and the common libraries define
it leaves room: a library may check [8][S]B = [8]R + [8][k]A (the
cofactored equation) or the equation above, may or may not refuse S ≥ L,
non-canonical encodings and points of small order, and the choices
change the verdict on signatures that an attacker can make. For a key of
small order anyone can make signatures that satisfy the equation; with
non-canonical encodings or S ≥ L one valid entry has several encodings;
and where two implementations disagree on one entry, relays and clients
disagree on which entries a feed holds. With these four rules every
signature is valid for every conforming implementation or for none, no
one but the holder of the secret key can sign for a feed, and each valid
entry has exactly one valid encoding.

An ordinary Ed25519 verification function is not enough: many accept a
small-order R or A, a non-canonical A, an S ≥ L that passes a check of
its top bits only, or check the cofactored equation. An implementation
adds the checks it lacks (1 to 3 can be done before calling it; 4 by
comparing bytes), or computes step 4 itself. `vectors/entries.txt` has
an entry for each check (`small-order-key-signed`,
`small-order-key-order-8-signed`, `non-canonical-key`,
`non-canonical-signature`, `small-order-r`, `non-canonical-r`,
`mixed-order-r`): an implementation that accepts any of them is wrong.
`sdk/testdata/strict.txt` has 64 more cases (every point of small order
as A and as R, S + L and S = L, mixed-order R, and keys with a
small-order component, whose signatures are valid exactly when [k]
times that component is the neutral point). The Tersenet SDK, `sdk/go`
and `sdk/rust`, implements all of section 4.2 in one call.

An entry is valid if `n` is at most 120, its length matches `n`, the
signature verifies under the feed key by strict verification (above),
`seq` is at least 1, the kind is
not `0` or `255`, the size is 1 to 1,048,576 (1 to 4,096 for a profile),
the title is valid UTF-8 (RFC 3629: no overlong forms, no surrogates,
nothing above U+10FFFF), and, for a profile or a program, the title is a
valid display name (section 4.6). Clients and relays discard invalid
entries.

**Kinds not defined.** An entry whose kind is not one this revision
defines (`5` to `254`) is not invalid for that reason: it is valid if
it meets every rule above, and the kind-specific rules (a profile's or a
program's title is a display name, a profile's size is at most 4,096)
apply only to the kinds that have them. Relays store it and pass it on
like any other entry (sections 4.3, 5.2). Clients keep it or skip it
without error, do not download its blob automatically, and do not show
it as something they can open (section 6). Later revisions define new
kinds this way, so that relays and clients of this one carry them.

An **article** entry (kind 1) publishes an article (`ARTICLE.md`); its
title is the article's title.

A **program** entry (kind 3) publishes a program for the Terse machine:
its title is the program's name, its blob a program image (section 1.1).
The entry is valid whether or not the blob is a valid image; that is
found out when the program is started. A feed's **current version** of a
program is its valid program entry with that name and the highest `seq`
(of those a client or relay holds). A program's identity is (feed key,
name).

A **document** entry (kind 4) publishes a document, data that a program
reads (section 1.1): its title is the document's name (any title, as for
an article), its blob a document. The entry is valid whether or not the
blob starts with a document header.

Clients that do not run programs still handle program and document
entries as the network requires (they are valid entries, stored and
passed on like any other) and need not open their blobs (section 6).

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
- Publish, subscribe and unsubscribe replies use status `1` (done) or a
  refusal (`3` or another of section 3.1). Unsubscribing from a feed that was not subscribed succeeds.
- `seq` values need not be consecutive, and the time is not checked.
- A relay may keep only the most recent entries of a feed (at least 256,
  or fewer to keep within a limit in bytes, section 5.3), but always keeps
  the feed's latest profile entry (the one with the highest `seq`, section
  4.6), however old.

### 4.4 Client behaviour

- The client keeps a list of subscribed feeds and every valid entry it
  has received for them.
- When connected to a relay it subscribes to each feed with `after` set to
  the highest `seq` it holds (with several relays, on each: section 5).
- **Subscribing** (when the user asks, by key or by feed address, section
  4.9; a machine host also lets programs ask, section 7) adds the feed to
  the list and, when connected,
  subscribes to it at once the same way; for a feed the client holds
  nothing of, `after` is 0, so the relay sends every entry it holds (a
  relay keeps at least the 256 most recent of each feed, section 4.3).
  Unsubscribing removes the feed from the list and unsubscribes on the
  connection. A client subscribes only to valid feed keys (section 4.1).
  The list is the one in force now: a client whose list is changed by
  another program while it runs (the reference implementation's
  `terse feed subscribe` beside a running `terse-host`) follows the change
  within about a second, as if it had been asked, and keeps that change
  when it saves its own.
- **Automatic download:** when the client receives an entry for a
  subscribed feed whose size is at most 16,384 bytes (the user may change
  this limit), it gets the blob without being asked. This applies to
  profile and document entries too, not to programs (below) or to kinds
  this revision does not define (section 4.2). Larger blobs are fetched
  only on request.
- **A blob whose length differs from the entry's size** is not
  associated with the entry: the client treats the entry as not having
  its blob (it is not shown as downloaded, is not scanned for prefetch
  and is not downloaded again automatically); opening the entry should
  say that its blob does not match. The blob is still valid by its ID:
  the client may keep it or discard it, and following an article link
  to its ID shows it like any other blob.
- **Programs** (for a client that runs them) are downloaded differently:
  once a subscribed feed is caught up (section 4.3), and after that
  whenever one of its program entries arrives, the client gets the image
  of the current version of each of the feed's programs that it does not
  hold, whatever the article limit, if its size is at most 131,136 bytes
  (the largest image); not older versions. The user may turn this off. A
  client that does not run programs does not download them.
- **Prefetch:** when the client stores an article blob through automatic
  download for an article entry (kind 1) of a subscribed feed, it scans
  the text for article links (`terse:` and 64 lowercase hexadecimal
  digits, not preceded by an ASCII letter or digit and not followed by
  one, `ARTICLE.md` section 4.1; also the targets of links with text),
  takes the first 8 distinct ones, in the order of the text, whose blobs
  it does not hold and is not already fetching, and gets each of those 8
  that the two rules below do not skip (a link so skipped still counts
  as one of the 8; one held or being fetched does not). The scan may be a plain scan of the
  bytes that does not parse the article's blocks (so that it also finds
  the link forms in code spans, which are not links when the article is
  shown): prefetch only fetches ahead of time. It skips a link when an
  entry it holds gives the blob a size over the automatic download limit,
  and discards a fetched blob over that limit. Program links
  (`terse:program/`) are not article links and are never prefetched. A
  client does not prefetch a blob that a program entry (kind 3) it holds
  names, since that blob is an image; it cannot tell that any other
  linked blob is an image (or a document) before it has fetched it, so
  such a blob is fetched like an article, within the limit. Documents are
  not scanned for links. Prefetched blobs are not scanned. The user may
  turn prefetch off.
- **Arrival order:** the client records the order in which it first
  stored each entry, and keeps that record across restarts. This is the
  order of its **timeline**, newest first, which holds only article and
  document entries (kinds 1 and 4), not profiles, programs or kinds this
  revision does not define.
  Unsubscribing removes a feed's entries from the timeline but not from
  the record; subscribing again puts them back where they were. The
  timeline holds documents because a machine host's programs read it
  (section 7); a client that does not open documents may leave them out
  of the timeline it shows, or show them as something it cannot open.
- **Publishing** an article or a document (when the user asks; a machine
  host also lets programs ask, section 7): the client puts the blob, then
  signs and publishes an entry with the next `seq` (one more than the
  highest it holds or has used for its own feed). The entry is stored
  locally like a received one; if the client subscribes to its own feed
  it is in the timeline at once.

### 4.5 Articles

The article format, and what a reader must show for it, is defined in
`ARTICLE.md`.

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
| `relay` | A `ws://` or `wss://` URL of a relay where the author publishes. Zero to four lines; readers take the first four that are URLs and skip other values. A value is a URL here if it starts with `ws://` or `wss://` (lowercase), what follows is not empty and does not start with `/`, and it holds no control character (U+0000–U+001F, U+007F–U+009F) and no whitespace (the White_Space characters listed above). Whether its host may be used is section 5.1's question. |

Unknown fields are ignored. A client that writes a profile for its user
puts a `relay` line for each of its relays that is not a loopback
address.

Names are self-declared and **not unique**: anyone can declare any name.
Anything shown to a user that names a feed shows the feed key (or a short
form of it, for example its first and last six hex digits) beside the name.

A **nickname** is a name the user gives a feed (the same rules as a display
name). The client keeps nicknames in its own configuration and never
sends them anywhere. When showing a feed, a nickname takes precedence over the
declared name.

### 4.7 Search

| Direction | Message | Layout |
|---|---|---|
| client → relay | Find | `06h`, `req` (4), `scope` (1), `n` (1), query (`n` bytes) |
| relay → client | Find reply | `86h`, `req` (4), `status` (1), `count` (1), then `count` times: `length` (2), entry (`length` bytes, complete with its signature) |

- `scope` `1` searches feeds by display name; `2` searches article and
  document titles; `3` searches program names. The query is 1 to 48 bytes
  of UTF-8. A find with another scope or a bad query is refused (status
  `3`, `count` 0).
- A find the relay carries out is answered with status `1` (done) and
  the results, `count` of them, 0 when nothing matches. Status `2` is not
  used in a find reply.
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
- A relay may refuse a find (status `3`, or a "try again" status of
  section 3.1; `count` 0) from a connection or an address that searches
  too often.
- **Clients** ask all their relays and merge the replies: they verify every
  returned entry and drop invalid ones, ones of the wrong kind and ones that
  do not match the query, and keep one entry per feed and `seq`. Scope 1:
  only the highest `seq` of each feed, ordered by the rules above. Scopes 2
  and 3: by rank, the first result of each relay (in the order of the
  client's relays), then the second of each, and so on; for scope 3, a
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
- A relay may refuse a lookup (status `3`, or a "try again" status of
  section 3.1) from a connection or an address that asks too often;
  lookups and finds may share one allowance.
- **Clients** ask all their connected relays, verify every entry returned
  and drop one that is invalid or not of the feed, kind and name asked
  for, and take the one with the highest `seq`. The entry is stored like
  a received one (it is not added to the subscriptions). A client looks
  a program up when it is to start a program, by its identity, that it
  holds no entry for (on a machine host, section 7), and then gets the
  image.

### 4.9 Feed addresses

A feed key alone says which feed, not where to find it: a client that
asks only its own relays finds nothing of a feed whose author publishes
elsewhere, and cannot read the author's profile (section 4.6), which
names their relays, before it has found the feed. A **feed address** is a
feed key with up to two **hints**, relays that should hold the feed:

    address   = key [ "@" hint [ "," hint ] ]
    key       = 64 lowercase hexadecimal digits
    hint      = [ "ws:" ] host [ ":" port ]            at most 80 bytes
    host      = name / IPv4 / "[" IPv6 "]"
    name      = label *( "." label )                   the last label holds a letter
    label     = 1 to 63 of A-Z a-z 0-9 "-", not starting or ending with "-"
    IPv4      = four decimal numbers 0 to 255 separated by ".", without leading zeros
    IPv6      = 8 groups of 1 to 4 hexadecimal digits (either case)
                separated by ":", or fewer and one "::" standing for one
                or more groups of zeros; no dotted IPv4 part, no zone
    port      = 1 to 65535, decimal, without leading zeros

For example `3fa9…c21e07@relay.example.org` or
`3fa9…c21e07@relay.example.org:9000,[2001:db8::7]` (the key written in
full). There are no blanks, no scheme and no path; a hint list is at
most 161 bytes. A hint names an encrypted relay, as an address without
a scheme does (section 3): `relay.example.org` is
`wss://relay.example.org` and `relay.example.org:9000` is
`wss://relay.example.org:9000`. A hint that starts with `ws:` names a
plain relay, `ws://host` with its port (the default port of `ws://` if
it gives none): `ws:relay.example.org:7878` is
`ws://relay.example.org:7878`. A hint whose host is `localhost` or a
loopback address without `ws:` names a plain relay too (section 3);
clients ignore such hints anyway (section 5.1). Whether a client may
use a plain hint is section 3's question (no downgrade): only if its
user wrote that relay as a plain one. Hint names compare without regard
to case; two hints name the same relay when their hosts, ports (the
default port of the scheme when left out) and encryption are the same.
The address of an article link is `terse:feed/<address>` (`ARTICLE.md`
section 4.1).

Earlier revisions read a hint without a port as port 7878, and every
hint as plain except one with port 443, which named `wss://host`. A
reader of an earlier revision takes an encrypted hint of this revision
as a plain relay, which fails to connect unless the host also serves a
plain relay there; one written with port 443 means the same in both. A
`ws:` hint is malformed for such a reader (its host would be `ws`): it
refuses a typed address that holds one, and in an article it reads the
link as the key alone.

**A malformed address.** A client refuses an address that does not match
this grammar when it is typed or given to it (the whole address is
invalid, hints included: it does not subscribe to the key alone). As
typing aids it may ignore blanks around it, take the link form
`terse:feed/<address>`, and take a key written with capital hexadecimal
digits (it then uses the key in lowercase); it need not. A hint's host
ends at the first `:` that is not inside brackets, so an IPv6 address
without brackets is malformed. The reasons named in
`vectors/addresses.txt` show one order of checking; an implementation
that refuses the same addresses for other reasons conforms.
In an article a malformed hint list never makes the link invalid: the
link is then the key alone and the rest is text (`ARTICLE.md` section
4.1), so an article reads the same in a reader of this revision as in
one that knows no hints.

**What a client does with hints.** Hints only say where to look: every
entry received through one is verified like any other (section 4.2).

- Subscribing with an address, the client keeps the hints with the
  subscription (a newer address for a feed already subscribed replaces
  them) and subscribes on its own relays as section 4.4 says.
- If none of its own relays holds an entry of the feed, it also follows
  the feed on the hinted relays, at most two: it connects to them as to
  feed relays (section 5.1: the same refusal of loopback, private and
  link-local addresses, checked when connecting, a name resolved once;
  they count towards the 16 connections; turning feed relays off turns
  hints off too), and subscribes to that feed there. None of its own
  relays holds an entry of the feed when it has no own relay; or the
  last attempt to connect to each own relay failed; or at least one own
  relay is connected and every connected one has answered the feed's
  subscribe with `highest` 0 (section 4.3) or refused it. An own relay
  that is still connecting does not hold this up, and the client does
  not wait for one.
- Once it follows a feed on its hints, it goes on doing so, with one
  exception: when an own relay answers a subscribe for the feed with
  `highest` above 0 (after a reconnection, for example), the client
  stops following the feed on the hinted relays (it unsubscribes there,
  or closes a connection that serves nothing else) and does not use the
  hints again until the feed is unsubscribed and subscribed again.
- While the feed's profile in force names usable relays (sections 4.6,
  5.1), the hints are not used (the client keeps them): the feed relays
  of section 5.1 take their place. If a later profile names none, the
  hints are used again as above.
- A client tells its user which feeds it follows through a hinted relay
  and, before subscribing with hints on someone's behalf (a link, a
  program), that it will connect to the relays they name.

**A client's own address.** A client shows its user their feed address
with hints taken, in order, from the `relay` lines of their profile in
force, then from its configured relays: those that can be written as a
hint (`wss://`, with no path but `/`; the port is left out when it is
the default) and whose host is not refused by section 5.1, without
repeats, at most two. Plain relays are not given as hints. If none is left, the address is the
bare key, and the client says that only people who use one of the same
relays will find the feed.

## 5. Several relays

### 5.1 Clients

A client's relays are those it is configured with (at most eight) and,
for each subscribed feed whose profile in force has `relay` lines, up to
two of those (the first two that are not already among the client's own
relays), or else, for a feed subscribed with a feed address that none of
the client's own relays holds, up to two of its hints (section 4.9);
**feed relays** may be turned off. A client keeps at most 16
relay connections. It ignores a `relay` line or hint whose host is `localhost`, a name
under `.localhost`, or an address that is loopback, unspecified, private
(10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10, fc00::/7) or
link-local (169.254.0.0/16, fe80::/10), and does not connect if a host
name resolves to any such address; it connects only to the addresses it
checked, without resolving the name again. It also ignores a `relay`
line or hint that names a plain relay (`ws://`, or a `ws:` hint) unless
its user wrote that same relay as a plain one (section 3: no
downgrade). A client connects to all its
relays and, when a connection fails or drops, tries again after 1 s,
doubling the wait up to 30 s. A relay that is still connecting does not
hold up requests: they go to the relays that are connected. Only while
none is connected does a request wait: if none is connecting either, it
fails at once (a get: not found); otherwise it waits until none is
still connecting, or until at least one is connected and 200 ms have
passed since the request began to wait, whichever comes first, and then
goes to the relays connected then.

Own relays come first within the 16 connections; which feed relays get
the rest is the client's choice. A feed's relays are worked out again
whenever its profile in force, or that profile's document, changes, and
when the feed is subscribed or unsubscribed: a relay no longer named
for a feed stops serving it (the client unsubscribes from the feed
there, or closes the connection if it serves no other feed), and newly
named ones are added. A profile in force whose document the client does
not hold (yet) names no relays.

- **Put and publish** go to every connected relay. They succeed when at
  least one relay accepts. When publishing an article, the entry is
  published once a relay has accepted the blob. A client that tells its
  user which relays took a put or an entry tells, for each that refused,
  the reason (section 3.1), and for a relay that took the entry but
  refused its blob, that.
- **Subscribe** goes to every connected relay that serves the feed (all of
  the client's own relays, and the feed relays named in the feed's
  profile or by its address's hints), each with `after` set to the
  highest `seq` the client holds. An entry
  received from several relays is stored once (the first received of a
  `seq` wins, section 4.2). A pushed entry is kept if it is a valid
  entry of a feed the client subscribes to, whichever of its relays sent
  it; entries of other feeds are ignored.
- **Get** asks the connected relays in turn: first the feed relays named in
  the profile in force of a feed with an entry pointing at the blob, then
  the rest in the client's order, with at most two requests outstanding:
  as soon as one relay answers without the blob (not found, a blob that
  fails the check of section 2, or no answer within 10 s, which counts as
  a failure), the next is asked. The first blob that passes the check is
  the answer. The blob is not found only when every connected relay has
  said so or failed.
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

Each relay keeps to its own limits (section 5.3) for what a peer sends it
as for what a client sends: a relay with a list of writers refuses a
peer's entries of other feeds, and one with a limit per feed drops a
feed's oldest entries whatever its peers hold. Nothing tells a relay that
a peer dropped an entry or a blob: a relay keeps what it holds until its
own limits remove it. A relay may exempt the peers it connected to itself
from its limits per address; a connection that announced itself with Peer
is a client's for those limits (anyone can send Peer).

### 5.3 What a relay may limit

A relay's limits are its operator's choice. This section says what a
relay may do, so that clients can rely on everything else and tell their
users what happened (section 3.1).

- **A feed's bytes.** A relay may limit the bytes of the blobs a feed's
  entries name: each entry counts its size (section 4.2), so a blob
  named by entries of two feeds counts for each. When a new entry would
  put a feed over the limit, the relay drops the feed's oldest entries
  until it fits, never the latest profile (section 4.3). It refuses the
  entry (status `4`) if the entry would itself be dropped: it is larger
  than the limit, or it does not fit beside the latest profile, or it is
  older than every entry kept. The entry limit of section 4.3 is applied
  the same way.
- **Blobs no entry names.** A blob is *named* when an entry the relay
  holds points at it with the blob's length as its size (section 4.4).
  A relay may keep a blob that is not named for a limited time only, at
  least 10 minutes after it was put (a publisher puts the blob, then
  publishes the entry, section 4.4); a put of the same blob again starts
  the time again. A relay that removes such blobs answers "not found"
  (status `2`) to a get of one while it is not named, so that what it
  serves does not depend on when it collects them. A blob whose last
  entry is dropped is no longer named and is treated the same way. It
  may refuse a put (with a "try again" status) when the blobs waiting to
  be named, in all or from the client's address, take up its allowance.
  So on such a relay, an article that is reached only by a link from
  another article (no entry names it) is not kept: a publisher who wants
  it kept publishes an entry for it too.
- **Rates per address.** A relay may limit, per client address, the
  puts and publishes, the bytes put, the entries of feeds it holds
  nothing of, the finds and lookups, and the connections (statuses `64`
  to `255`, or HTTP 429 at the handshake). An IPv6 client is counted per
  /64 or a larger prefix, not per address. A relay behind a proxy takes
  the client's address from the proxy's header (such as
  `CF-Connecting-IP` or `X-Forwarded-For`) only for connections from a
  proxy it was told to trust; from anyone else, the header is ignored.
- **A list of writers.** A relay may take entries only of the feeds on
  its list (status `6` for others). Reading is not limited by the list:
  gets of named blobs, subscribes, finds and lookups are answered for
  every feed it holds. Since a put carries no key, such a relay takes
  puts from anyone under the rules for blobs no entry names, and may give
  an address that none of its listed feeds has published from lately a
  smaller allowance.
- **The whole store** (status `5`), as section 3 says.


## 6. What a reading client must and may do

A **reading client** shows people the articles of the feeds they follow.
A native reader with no machine is one; so is a machine host running the
reader program. This section is normative for any client that shows
articles.

A reading client **must**:

- hold blobs by their BLAKE3-256 ID and verify every blob it receives
  against its ID before using it (sections 1, 2);
- talk to at least one relay by the protocol of section 3: at least get
  (section 3), subscribe, unsubscribe and the pushed entries (section
  4.3), and ignore reply types it does not use;
- verify every entry it receives by the rules of section 4.2 and discard
  invalid ones, keeping the first valid entry received for a feed and
  `seq`;
- let the user subscribe to and unsubscribe from feeds by key (only valid
  keys, section 4.1), subscribe with `after` as section 4.4 says, and
  keep its timeline in arrival order (section 4.4);
- accept a feed address (section 4.9) wherever it lets the user give a
  key, subscribing to its key (it need not use the hints), and refuse a
  malformed one;
- show articles by `ARTICLE.md`;
- show a feed's name only with its key or a short form of it beside it
  (section 4.6), and show the profile in force's display name, or the
  user's nickname, as section 4.6 says.

A reading client may be one program or two: a node that does the
network side and a reader that shows articles, talking over a local
socket (`LOCAL.md`); the pair is then the reading client.

A reading client **may**:

- publish articles (and keep an identity, section 4.1), set its user's
  profile and nicknames (section 4.6);
- search (section 4.7) and look programs up (section 4.8);
- use several relays and feed relays (section 5), and the hints of feed
  addresses (section 4.9);
- download automatically and prefetch (section 4.4), within the limits
  there;
- keep a smaller automatic download limit, or none (fetching on
  request), and cap what it shows of a large article (`ARTICLE.md`
  section 1).

**Kinds it does not handle.** Entries of kinds a client does not handle
(program entries, kind 3, and documents, kind 4, for a client without
the machine; kinds this revision does not define, `5` to `254`, for
every client) are tolerated: a client keeps them or skips them without
error, never treats them as a failure of the feed or the connection,
and shows them, if at all, as something it cannot open (for example in
a list of the feed's entries, with their title and kind). An entry of a
kind this revision does not define is valid if it meets the rules of
section 4.2 (only kinds `0` and `255` are invalid as such), so a client
that verifies entries keeps the first one received for its feed and
`seq` like any other; it is not in the timeline (section 4.4) and its
blob is not downloaded automatically. A link to a program or to a blob
that turns out to be an image or a document (section 1.1, `ARTICLE.md`
section 4.4) is handled the same way: the client says that it cannot
open it.

### 6.1 Responsiveness

Reading should never wait on the network for what the client already
holds. A reading client **must**:

- show an article it holds (opened from the timeline, by following a
  link to it, or by its ID) from its store: it sends no request to any
  relay for it and does not wait for a relay to connect or to answer;
- get a blob it does not hold as section 5.1 says, so that when the
  first connected relay asked holds it, the client has it after one
  round trip (one Get and its reply) with no other request before it;
- show its timeline and subscription list from what it holds, at once,
  never after waiting for a relay to connect or for feeds to catch up;
  entries that arrive later are added as they come;
- catch a feed up on a relay with one Subscribe (section 4.3), the
  entries pushed after it, not asked for one by one.

These are stated in round trips and in what may wait on what, so that
any client can be tested against them on any network (a relay that
counts the messages it receives, and one that accepts connections and
never answers). Times in milliseconds depend on the machine and the
network; `PERFORMANCE.md` gives the reference implementation's on a
named machine as targets (information), ceilings far above them that a
client on any reasonable computer must stay under, and how to run the
same tests.

## 7. For machine hosts

This section concerns only clients that run the Terse machine
(`SPEC.md`); a reader implementer can skip it.

- Programs never see this protocol. They use the content device
  (`spec/content.md` §content); the host speaks the protocol on their behalf.
  Programs never see secret keys.
- Subscribing, unsubscribing, publishing, setting the profile and
  setting a nickname may be asked for by a program; the host asks the
  user's consent first (`spec/content.md` §content.feeds). The timeline of section
  4.4 is what a program reads with the timeline operation (`spec/content.md` §content.feeds).
- A host may start a program by image ID: it gets the blob, validates it
  as an image (`SPEC.md` §image) and runs it. A program's identity,
  (feed key, name) of section 4.2, is the one of `spec/machines.md` §machines.shell;
  a host looks a program up (section 4.8) when a shell starts a program
  it holds no entry for (`spec/machines.md` §machines.calls, `start_program`), and
  downloads programs as section 4.4 says.
- A host that publishes a document for a program checks first that the
  blob starts with a document header (`spec/content.md` §content.feeds, §content.documents).
- The built-in reader shows articles by `ARTICLE.md` on the page device
  (`ARTICLE.md` section 11).

## 8. Not yet specified

- Verified range requests (so a range can be delivered without fetching the
  whole blob).
- Finding replies to an article, ranking, moderation and spam limits.
- Key rotation and several feeds per identity.
- What `ARTICLE.md` section 10 lists for articles (more inline markup,
  images not drawn from paths, arcs in figure paths, numbered and nested
  lists).
- Named pointers: signed, versioned pointers from a public key and a name
  to a blob (not the display names of section 4.6).
- Messages and topics.
- A relay keeping the current version of every program of a feed however
  old (it keeps only the latest profile so), so a lookup can miss an old
  program of a busy feed. Lookups of kinds other than programs.
- Sharding content among relays by hash, so that each relay holds only part
  of it.
- Relay discovery beyond configuration, the local relay, the `relay`
  lines of profiles and the hints of feed addresses (section 4.9).
- Direct connections between clients.
- Abuse limits beyond those of section 5.3; a list of writers that its
  writers extend (vouching) instead of the relay's operator.
