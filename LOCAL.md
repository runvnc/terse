# Terse local node protocol — v0 (draft)

A Terse client does two different jobs: it follows feeds on relays,
verifies and stores what it receives (the network side, `NETWORK.md`), and
it shows articles to a person (the reading side, `ARTICLE.md`). This
document lets the two run as two programs on one machine:

- a **node** speaks `NETWORK.md` to relays, verifies every blob and entry,
  keeps the store, the subscriptions, the timeline and the user's
  nicknames, and serves this protocol on a local socket;
- a **reader** connects to the node and shows what it holds. A reader
  needs no cryptography, no hash function, no WebSocket and no store: the
  node has done all of that.

A program that does both in one process, as before, is still a
conforming client; nothing here is required of it. The test vectors are
in `vectors/local-session.txt` (described in `vectors/README`).

**Implementations.** Two nodes speak this protocol: the reference's
`terse node` (`crates/terse-cli/src/node.rs`; it uses the store,
subscriptions, nicknames and relay list of `terse` and `terse-host`, and
answers Find and Lookup) and the independent Go reader's `tersereader
node` (`native/reader-go`, its own store; no Find or Lookup). The Go
reader is also a client, and its thin build (`-tags thin`) is a
reader-only program. `crates/terse-cli/tests/node.rs` runs a client
written from this document against both nodes, and the thin reader
against `terse node`. Both serve Unix domain sockets only; the loopback
TCP transport below is specified but not implemented yet.

## 1. Why not the relay protocol itself

A node is close to "a relay on your own machine that you trust", and the
messages of `NETWORK.md` section 3 are reused where they mean the same
(section 4.2). The relay protocol alone is not enough for a reader:

| A reader needs | The relay protocol has |
|---|---|
| A blob, verified, fetched from the relays if not held | Get: the same (reused) |
| A blob only if held (no network) | nothing |
| The timeline: the user's entries in arrival order, whether each blob is held | Subscribe, which sends one feed's entries in `seq` order, with no arrival order and nothing of what is held |
| The subscription list, kept across restarts, with each feed's address | Subscribe and unsubscribe are per connection: nothing is kept |
| Names: the profile in force's display name, the user's nicknames | nothing (a relay holds the profile entry, not its meaning) |
| The relay list, and which relays are connected | nothing |
| "Something changed", and catching up | pushed entries only |

So the local protocol is the relay protocol's framing ideas and message
style, two reused messages (Get, and optionally Find and Lookup), and the
calls for the user's own state.

## 2. Transport

**The socket.** On a system with Unix domain sockets the node listens on
a stream socket at `$XDG_RUNTIME_DIR/terse/node.sock`; a client may be
given another path (the reference command line takes `--socket`, and
the environment variable `TERSE_NODE_SOCKET` overrides the default for
both sides). Without `XDG_RUNTIME_DIR` the directory is
`/tmp/terse-<uid>/` (`<uid>` the user's numeric ID). Required:

- the directory is created with mode 0700 and must be owned by the user
  and not readable or writable by others (a node or reader that finds it
  otherwise refuses to use it); the socket file has mode 0600;
- a node that finds a socket file nobody listens on removes it and
  listens there; one that finds a live node there does not start (one
  node per user and path);
- a node removes its socket file when it stops.

Suggested: a node also checks the peer's user ID (for example
`SO_PEERCRED`) and closes connections from another user.

**Windows** 10 version 1803 and later has Unix domain sockets (`AF_UNIX`
through Winsock, as the device bus uses them, `DEVICES.md` 4.3), so the
above applies, with: the directory is `terse` in the user's local
application data directory (`%LOCALAPPDATA%\terse\`, else
`TERSE_NODE_SOCKET`), whose access the user's profile already limits to
the user, so there are no modes to set or check. The path is UTF-8 and at
most 103 bytes. Windows before 10 version 1803 uses the TCP port below.
(Not implemented yet: `terse node` says it is not available on Windows,
and `native/reader-go` has no Windows build. `crates/terse-bus/src/sys`
has the Winsock socket a node there could use.)

**Other systems** (no Unix domain sockets): the node listens on a TCP
port of the loopback address 127.0.0.1, chosen by the system, and writes
the port and a random token of 32 bytes (64 lowercase hexadecimal
digits) as one line `<port> <token>` to a file only the user can read
(`terse/node` in the user's local application data directory). A reader
sends the token in Hello (section 4.1); a node closes a connection whose
Hello does not carry it. A node never listens on any address but
loopback.

**Framing.** The socket is a byte stream. Every message is its length as
4 bytes (little-endian, not counting these 4) followed by the message,
at most 2,097,152 bytes. A side that receives a longer length closes the
connection. Multi-byte integers are little-endian, as in `NETWORK.md`.

**Requests and replies.** As on a relay: `req` is a 32-bit number the
reader chooses and the node echoes; replies may come in any order (a
slow Get does not hold up a Timeline). A node also **pushes** messages
(section 4.4) that answer no request. Each connection is independent:
several readers may use one node at once.

## 3. Trust

- The node verifies everything (blobs against their IDs, `NETWORK.md`
  section 2; entries by section 4.2, strict verification included)
  before it stores it or gives it to a reader. A reader does not verify
  again: the socket is the security boundary.
- A reader must still parse what it receives defensively (lengths, UTF-8,
  `ARTICLE.md` section 1 for text it shows): a bug in a node must not
  become a crash or a terminal escape in a reader.
- A node closes the connection on a first message that is not Hello, a
  second Hello, a message of a known type it cannot parse (too short, or
  with bytes after its last field), and a message longer than the limit;
  it ignores a message whose type it does not know. It answers with
  status 3 (refused) requests that would break `NETWORK.md`: subscribing
  to an invalid key or a malformed address, more than eight relays or a
  relay that is not a relay URL, a nickname that is not a display name
  or for an invalid key.
- Secret keys never cross the socket (a node that publishes keeps its
  identity, section 6).

## 4. Messages

`n` (1) is a length byte followed by that many bytes, `n2` (2) a 2-byte
length; text is UTF-8. **Status** is as on a relay: `1` done, `2` not
found, `3` refused. In the replies `A0h` to `ABh`, status `3` is
followed by `n` (1) and a reason for people (any text; readers show it,
tests do not compare it) and nothing else.

### 4.1 Session

| Direction | Message | Layout |
|---|---|---|
| reader → node | Hello | `20h`, `req` (4), `version` (1) = 0, `n` (1), token (`n` bytes; none on a Unix socket) |
| node → reader | Hello reply | `A0h`, `req` (4), `status` (1), then if 1: `version` (1) = 0, `flags` (1) |

Hello is the first message on a connection, and the only Hello. `flags`
bit 0: the node answers Find and Lookup (section 4.2); other bits are 0.
A node answers a version it does not speak with status 3 and waits for
another Hello.

### 4.2 Reused from the relay protocol

| Direction | Message | Layout |
|---|---|---|
| reader → node | Get | `01h`, `req` (4), `id` (32) |
| node → reader | Get reply | `81h`, `req` (4), `status` (1), then the blob if 1 |
| reader → node | Find | `06h`, as `NETWORK.md` 4.7 |
| node → reader | Find reply | `86h`, as `NETWORK.md` 4.7 |
| reader → node | Lookup | `08h`, as `NETWORK.md` 4.8 |
| node → reader | Lookup reply | `88h`, as `NETWORK.md` 4.8 |

Get answers from the store, else asks the relays (`NETWORK.md` section
5.1) and stores the blob; status 2 when no relay has it (at once when the
node has no relay). Find and Lookup, if the node offers them (Hello
flags), are the client's merged answers of `NETWORK.md` 4.7 and 4.8:
every entry in them is verified. The cursor of a node's Find reply is
the node's own (it stands for the relays' cursors, which the node keeps
or puts in it), and a page is the next 16 of the merged results
(`NETWORK.md` 4.7, "More results"). A node keeps a bounded number of
searches (the reference 64, the one used least recently dropped first)
and refuses (status 3, `count` 0, `c` 0) a cursor for a search it no
longer keeps; the reader starts again from the first page. A Find that no relay answered (or
that the node had no relay to ask) is refused: status 3, `count` 0; a
Lookup that no relay answered has status 2. The relay protocol's other
requests (Put, Publish, Subscribe, Unsubscribe, Peer) are not local
messages: a node ignores them like any unknown type.

### 4.3 The user's state

| Direction | Message | Layout |
|---|---|---|
| reader → node | Held | `21h`, `req` (4), `id` (32) |
| node → reader | Held reply | `A1h`, `req` (4), `status` (1), then the blob if 1 |
| reader → node | Timeline | `22h`, `req` (4), `before` (4), `max` (2) |
| node → reader | Timeline reply | `A2h`, `req` (4), `status` (1), then if 1: `count` (2), `count` records |
| reader → node | Subscriptions | `23h`, `req` (4) |
| node → reader | Subscriptions reply | `A3h`, `req` (4), `status` (1), then if 1: `count` (2), `count` feed records |
| reader → node | Feed | `24h`, `req` (4), `flags` (1), `n` (1), address |
| node → reader | Feed reply | `A4h`, `req` (4), `status` (1), then if 1: a feed record, and if `flags` bit 0: `count` (2), `count` records |
| reader → node | Subscribe | `25h`, `req` (4), `n` (1), address |
| node → reader | Subscribe reply | `A5h`, `req` (4), `status` (1), then if 1: `new` (1) |
| reader → node | Unsubscribe | `26h`, `req` (4), feed key (32) |
| node → reader | Unsubscribe reply | `A6h`, `req` (4), `status` (1) |
| reader → node | Nickname | `27h`, `req` (4), feed key (32), `n` (1), name |
| node → reader | Nickname reply | `A7h`, `req` (4), `status` (1) |
| reader → node | Relays | `28h`, `req` (4) |
| node → reader | Relays reply | `A8h`, `req` (4), `status` (1), then if 1: `count` (1), `count` times: `flags` (1), `n` (1), URL |
| reader → node | Set relays | `29h`, `req` (4), `count` (1), `count` times: `n` (1), URL |
| node → reader | Set relays reply | `A9h`, `req` (4), `status` (1) |
| reader → node | Sync | `2Ah`, `req` (4), `timeout` (4, milliseconds) |
| node → reader | Sync reply | `AAh`, `req` (4), `status` (1) |
| reader → node | Title | `2Bh`, `req` (4), `id` (32), `size` (4) |
| node → reader | Title reply | `ABh`, `req` (4), `status` (1), then if 1: `n` (1), title |

- **Held** answers from the store only, never from the network: status 2
  if the blob is not held.
- **A record** is one entry the node holds: `arrival` (4), `flags` (1),
  `n` (1) and the feed's **name**, `m` (2) and the entry (`m` bytes, the
  complete signed entry of `NETWORK.md` 4.2, which the reader reads by
  its layout and does not verify). `arrival` numbers the entries in the
  order the node first stored them (`NETWORK.md` 4.4), from 1, and never
  changes. `flags` bit 0: **held**, the node holds the blob and its
  length is the entry's size; bit 1: **needs full text**, the entry's
  title, or its blob if it is held and the entry is an article, is not
  in basic text (`ARTICLE.md` section 1.1); other bits are 0. The name
  is the user's nickname for the feed, else the display name of its
  profile in force, else empty; a reader shows it with the short key
  (`NETWORK.md` 4.6), which it takes from the entry.
- **A reply's size**: a Subscriptions or Feed reply that would be longer
  than the limit of section 2 carries as many records as fit (`count`
  says how many): Subscriptions the first, Feed the latest (the
  highest `seq`, still in increasing order).
- **Timeline**: the records of the timeline (`NETWORK.md` 4.4: article
  and document entries of subscribed feeds), newest first, those with
  `arrival` below `before` (`before` 0: from the newest), at most `max`
  of them (and at most 1,024). A reader pages back with `before` set to
  the last `arrival` it received.
- **A feed record**: feed key (32), `flags` (1: bit 0 subscribed, bit 1
  the name is a nickname), `n` (1) name (as in a record), `n` (1) the
  hint list it was subscribed with (the text after `@` in its address,
  empty if none), `count` (1) and that many `n` (1) URLs: the hints'
  relay URLs; `count` (1) and URLs: the hinted relays the node follows
  the feed on now; `count` (1) and URLs: the `relay` lines of the profile
  in force; `n2` (2) the profile document's description.
- **Feed** describes the feed of an address (a key, maybe with hints,
  `NETWORK.md` 4.9; refused if malformed or the key invalid), whether or
  not it is subscribed. With `flags` bit 0 it adds every entry the node
  holds of the feed, in increasing `seq`, as records.
- **Subscribe** takes an address (`NETWORK.md` 4.4, 4.9; status 3 if it
  is malformed or the key invalid); `new` is 1 if the feed was not
  subscribed. **Unsubscribe** succeeds for a feed not subscribed.
- **Nickname** sets the user's nickname for a feed (a display name,
  `NETWORK.md` 4.6), or removes it when `n` is 0.
- **Relays** lists the node's relay connections: `flags` bit 0
  configured (one of the user's own relays), bit 1 a hinted relay, bit 2
  connected; the configured ones first, in their order. A relay that a
  feed's profile names (`NETWORK.md` section 5) has neither bit 0 nor
  bit 1. **Set relays** replaces the configured list (at most eight relay
  URLs, `NETWORK.md` 4.6's test; status 3 otherwise). The node keeps the
  list as the user's relay list and uses it from then on: it disconnects
  from relays no longer listed and connects to the new ones.
- **Sync** connects (if the node is not connected), waits until every
  subscribed feed has caught up on its relays (`NETWORK.md` 4.3) and
  the automatic downloads it caused are done, or the timeout: status 1
  if everything arrived, 2 if not, 3 (with a reason) if it cannot try,
  for example because it has no relay.
- **Title** is the title of an article entry the node holds for that
  blob ID and size (status 2 if none; of several, the one the node
  stored first; an empty title is status 1 with `n` 0): what a reader
  shows for an article reached by a link (`ARTICLE.md` 4.4).

### 4.4 Pushed by the node

| Direction | Message | Layout |
|---|---|---|
| node → reader | Changed | `C0h` |
| node → reader | Log | `C1h`, `n2` (2), text |

- **Changed**: the node holds something new or its state changed (an
  entry, a blob, a subscription, a nickname, a relay connecting or
  dropping). It carries nothing: a reader asks again for what it shows.
  A node may send one Changed for several changes, but sends one after
  every change (within about a second).
- **Log**: a message for people (a relay that cannot be reached, a hint
  in use). A reader may show it or not.

Pushes go to every connection that has said Hello, between replies,
never inside one. A reader ignores pushed types it does not know.

### 4.5 Answering at once

Held, Timeline, Subscriptions, Feed, Relays and Title, and Get of a blob
the node holds, are answered from the node's own state: the node asks no
relay for them, does not wait for one to connect or answer, and does not
hold their replies behind a request that waits on the network (a Get of
a blob not held, Find, Lookup, Sync; replies may come in any order,
section 2). So a reader on a node sees what is held in one round trip on
the local socket, whatever the network is doing. `PERFORMANCE.md` says
how this is tested and gives the reference's times as targets
(information), with ceilings that a node on any reasonable computer must
stay under.

## 5. Builds

- A **node-only** program speaks `NETWORK.md` and this protocol and shows
  nothing. With a reader it makes a reading client: the pair meets
  `NETWORK.md` section 6 (the node does the verifying, the reader the
  showing).
- A **reader-only** program speaks only this protocol and `ARTICLE.md`.
  It holds no keys, computes no hashes and keeps no store. On its own it
  is not a client of the network; it needs a node.
- A **combined** program does both in one process and need not offer the
  socket; it conforms as before. A reader that finds no node may start
  one, in its own process or as another.

## 6. Not in this revision

Publishing, the user's identity and profile, and the machine's
consent prompts. A node that publishes would take the article and title
and sign the entry itself, so that secret keys never reach a reader.

**Machine hosts.** A machine host's content driver (`spec/content.md` §content,
the Content device) speaks the network on behalf of programs; it is the
same kind of client of a node as a reader is, and could use this
protocol instead of a network stack of its own. That is not specified
here.
