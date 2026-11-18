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
- Publishing an article: the host puts the blob, then signs and publishes an
  entry with the next `seq`.

### 4.5 Articles

An article is a blob. In v0 it is plain UTF-8 text. Authors should keep
articles to a few kilobytes; the automatic download limit is what keeps
reading instant.

## 5. Not yet specified

- Verified range requests (so a range can be delivered without fetching the
  whole blob).
- Letting programs subscribe and publish, with the user's consent. (Reading
  feeds is specified: SPEC.md section 9.5.)
- Finding replies to an article, ranking, moderation and spam limits.
- Key rotation and several feeds per identity.
- An article markup format.
- Names: signed, versioned pointers from a public key and a name to a blob.
- Messages and topics.
- Relay discovery, relay-to-relay exchange and direct links between hosts.
- Abuse limits beyond blob size.
