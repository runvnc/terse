# Relay groups — proposal (draft, not adopted)

Status: a proposal for the project owner to read and correct. Nothing
here is built, and nothing here changes `NETWORK.md`, `ARTICLE.md` or
`vectors/` until it is adopted (section 15 lists what would change).
Where a choice was the author's, it is stated as the proposal; holes and
alternatives found while writing it are in section 16, not folded into
the design.

It extends the Terse network, `NETWORK.md` (cited as N), and the article
format, `ARTICLE.md` (cited as A). It fills two items of N section 8:
"Sharding content among relays by hash" and, in part, "Relay discovery
beyond configuration".

**What it adds.** Relays prove a key (section 1). Feed and group
addresses carry relay hints (section 2). A **group** is a set of relays
named by a signed, hash-chained series of member lists; the list changes
by a quorum of its signers, or by one signer and a delay that nobody
vetoes (section 3). Each item has **responsible** relays chosen by hash
(section 4); relays copy, repair and audit by that rule (sections 5,
6). Clients follow a group from its ID (section 7). Signers decide with
information section 8 says they must be shown. Search (section 9) and an
author's own backups (section 10) need no group.

"Must", "should" and "may" are as in N and A. Multi-byte integers are
little-endian, as in N section 3. `‖` is concatenation. **Now** is the
clock of the relay or client applying the rule.

## 1. Relay identity

Each relay has an Ed25519 key pair, the **relay key** (a valid key by N
section 4.1). The relay keeps the secret key and never sends it. A relay
also has one or more **relay URLs**, the `ws://` or `wss://` URLs it is
configured to be reached at, in **canonical form**: the scheme and host
in lowercase, an explicit port, a path (`/` if none), no query, no
fragment, no user information; for example `wss://a.example:443/`.

### 1.1 Messages

| Direction | Message | Layout |
|---|---|---|
| client or relay → relay | Challenge | `09h`, `req` (4), `flags` (1), challenge (32), `n` (1), URL (`n` bytes) |
| relay → client or relay | Challenge reply | `89h`, `req` (4), `status` (1), then if `status` is 1: relay key (32), signature (64) |

- The challenge is 32 random bytes chosen by the sender. The URL is the
  canonical form of the URL the sender dialled (for the reverse
  challenge below, the receiver's URL as the member list names it).
- The relay refuses (status `3`) a challenge whose URL is not one of its
  own relay URLs, or whose `n` is 0. Otherwise it answers status `1`
  with its key and an Ed25519 signature over:

  `terse-relay-auth-0` (18 ASCII bytes) ‖ challenge (32) ‖ `n` (1) ‖ URL (`n`)

- `flags` bit 0 set means "I am a relay; challenge me back". A relay
  that receives a Challenge with bit 0 set sends its own Challenge on
  the same connection (bit 0 clear) once it has replied. Other bits are
  0; a challenge with another bit set is refused.
- A relay answers every Challenge, at any time, on any connection.

### 1.2 Verifying

The sender checks that the status is `1`, that the key is the one it
expected (from a member list, section 3), and that the signature verifies
(strict verification, as in N section 4.2) over the message above with
its own challenge and URL.

**On failure** (status not `1`, a different key, a bad signature, or no
reply within 10 s) the sender closes the connection and treats it as a
failed connection (N section 5.1: try again after 1 s, doubling up to
30 s). A client then uses another relay of the group (section 7.2); a
relay does not count the connection as a group link (section 3.8). A
client needs no challenge for a relay it uses only as a plain relay (one
it is configured with, or a feed relay): content is verified by hash and
signature whatever the relay.

What the challenge proves: whoever answers holds the secret relay key,
or is passing messages to and from someone who does. On `ws://` a
machine on the path can do the latter; on `wss://` the TLS certificate
binds the connection to the host name, and the URL in the signed message
binds the key to that host name. Section 16, item 14.

## 2. Addresses with hints

### 2.1 Forms

| Form | Names |
|---|---|
| `terse:feed/` key | A feed (unchanged, A section 4.1) |
| `terse:feed/` key `@` hint | A feed, with one relay that should hold it |
| `terse:feed/` key `@` hint `,` hint | A feed, with two |
| `terse:group/` ID `@` hint (`,` hint) … | A group (section 3) by its ID, with 1 to 4 hints |

Key and ID are 64 lowercase hexadecimal digits. A **hint** is
`host:port`:

- `host` is a DNS name (labels of 1 to 63 letters, digits and `-`, not
  starting or ending with `-`, separated by `.`, at most 253 bytes in
  all, at least one letter in the last label), an IPv4 address in dotted
  decimal (four numbers 0 to 255 without leading zeros), or `[`, an IPv6
  address in the text form of RFC 5952, `]`.
- `port` is 1 to 5 decimal digits with no leading zero, a value from 1
  to 65535. The port is required.
- A hint means the relay URL `wss://host:port/`; a client that fails to
  open a TLS connection there tries `ws://host:port/` (section 16, item
  12). A relay reached by hint must serve at path `/`.

### 2.2 Recognising them in articles

- The hint list is `@` and the hints separated by `,`, with no blanks.
  It is the longest run after the 64 digits that matches the forms
  above. If that run is followed by a letter or a digit, or holds more
  hints than the form allows (three for a feed, five for a group), the
  link has **no hints** and the text from `@` on is ordinary text.
- A feed link without a well-formed hint list is the feed link of A
  section 4.1, followed by ordinary text. A group link must have at
  least one hint; without a well-formed hint list it is not a link.
- The 64 digits are not followed by a letter or a digit (as now, A
  section 4.1); `@` is neither, so a feed link with hints already reads,
  in a reader of today, as a feed link followed by the text `@host:port`.
- Example: `see terse:feed/` key `@a.example:443,[2001:db8::7]:7878.`
  is a feed link with two hints; the final `.` is text.
- A link with text (A section 4.2) may have either form as its target:
  hints hold no blank and no `)`.
- Group links, like feed links, are not article links and are never
  prefetched (N section 4.4).

### 2.3 What a client does with hints

- Hints are for **reachability only**. Content is verified as always
  (blobs by hash, entries by the feed key, lists by the group chain,
  section 3.9); a hint never makes anything trusted.
- The checks of N section 5.1 apply to hint hosts: loopback, private,
  link-local and unspecified addresses are ignored.
- **Feed hints.** When the person follows a feed link with hints, or
  subscribes from one, the client keeps the hints with the subscription.
  If none of its connected relays (its own and the feed relays of N
  section 5.1) holds anything of the feed (every subscribe reply gives
  `highest` 0), it connects to the hints, at most two, within the limit
  of 16 connections, and subscribes there. Once it holds a profile in
  force with `relay` lines, those replace the hints, which it forgets.
- **Group hints.** Section 7.1.
- A client that writes a feed address for its own user (for example to
  share it) adds as hints the host and port of up to two of the relays
  its profile names.

## 3. Groups

A **group** is a set of relays that share out content (section 4). It is
named by a **group ID** and described by a chain of **member lists**,
each a blob. Lists change only by signed records (section 3.3) under the
rules of section 3.4.

### 3.1 Member list

A member list is a blob of at most 16,384 bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | `TGRP` (ASCII) |
| 4 | 1 | Version: `0` |
| 5 | 32 | Group ID; all zeros in the genesis list |
| 37 | 4 | Number: 0 for the genesis list, then the previous list's plus 1 |
| 41 | 32 | ID of the previous list; all zeros in the genesis list |
| 73 | 4 | Time: Unix seconds, as claimed by its writer |
| 77 | 1 | `q`: quorum, 1 to `s` |
| 78 | 1 | `v`: veto count, 1 to `s` |
| 79 | 4 | `D`: delay in seconds, 86,400 to 31,536,000 |
| 83 | 1 | `R`: replicas, 1 to 8 |
| 84 | 32 | Successor group ID, or all zeros |
| 116 | 1 | `s`: number of signers, 1 to 16 |
| 117 | 32 × `s` | Signer keys, in increasing byte order, distinct |
| 117 + 32`s` | 2 | `k`: number of relays, 1 to 64 |
| then | `k` × … | Relays, in increasing order of relay key, keys distinct |

Each relay:

| Size | Field |
|---|---|
| 32 | Relay key |
| 1 | State: `1` = joining, `2` = active |
| 32 | Operator: the feed key of the person who runs it, or all zeros (none) |
| 1 | `n`: URL length, 1 to 255 |
| `n` | Relay URL, canonical form (section 1) |

- Nothing follows the last relay. Every key is a valid key (N section
  4.1). At least one relay is active.
- **Defaults** (what tooling writes unless told otherwise): `q` = ⌊`s`/2⌋
  + 1 (a majority), `v` = max(1, ⌈`s`/3⌉), `D` = 1,209,600 (14 days),
  `R` = 2. For `s` = 1 to 7: `q` = 1, 2, 2, 3, 3, 4, 4; `v` = 1, 1, 1,
  2, 2, 2, 3.
- The genesis list has number 0, a zero group ID, a zero previous ID and
  a zero successor.
- A member list starts with `TGRP`, so it is not an article (N section
  1.1 would list it). A client does not show it as text.

### 3.2 Group ID

The group ID is the BLAKE3-256 ID of the genesis list (N section 1). The
ID of list 0 is the group ID; every later list names it and the ID of
the list before it. Changing any list changes the IDs of every list
after it, and changing list 0 changes the group ID.

### 3.3 Records

All records are signed with domain-separated messages. No domain string
is a prefix of another, nor of `terse-entry-0`.

**Signature** (type 1, 161 bytes): a signer endorses a list.

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `1` |
| 1 | 32 | Group ID |
| 33 | 32 | List ID |
| 65 | 32 | Signer key |
| 97 | 64 | Ed25519 over `terse-group-sign-0` (18) ‖ bytes 1 to 64 |

**Proposal** (type 2, 197 bytes): a signer proposes a list to take
effect by lapse. A proposal also counts as its signer's signature for
section 3.4 (a).

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `2` |
| 1 | 32 | Group ID |
| 33 | 32 | List ID |
| 65 | 4 | Effective time `E`, Unix seconds |
| 69 | 32 | Reason: the blob ID of an article explaining it, or all zeros |
| 101 | 32 | Signer key |
| 133 | 64 | Ed25519 over `terse-group-propose-0` (21) ‖ bytes 1 to 132 |

**Veto** (type 3, 193 bytes): a signer objects to a proposed list.

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `3` |
| 1 | 32 | Group ID |
| 33 | 32 | List ID |
| 65 | 32 | Reason: blob ID of an article, or all zeros |
| 97 | 32 | Signer key |
| 129 | 64 | Ed25519 over `terse-group-veto-0` (18) ‖ bytes 1 to 128 |

**Observation** (type 4, 188 bytes): one relay's claim about another
(section 8.4). It is never counted by any rule of this section.

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `4` |
| 1 | 32 | Group ID |
| 33 | 32 | Observer: relay key |
| 65 | 32 | Subject: relay key |
| 97 | 4 | From: Unix seconds |
| 101 | 4 | To: Unix seconds |
| 105 | 2 | Spot checks made (section 6) |
| 107 | 2 | Spot checks served and verified |
| 109 | 2 | Median time to serve, in milliseconds (`FFFFh`: none served) |
| 111 | 2 | Connection attempts |
| 113 | 2 | Connections that opened |
| 115 | 1 | Challenge: `0` not tried, `1` succeeded, `2` failed |
| 116 | 4 | Items the subject is or will be responsible for, as the observer counts them among its own (section 4) |
| 120 | 4 | Of those, items the subject was found to hold |
| 124 | 64 | Ed25519, by the observer's relay key, over `terse-group-observe-0` (21) ‖ bytes 1 to 123 |

The signer key of types 1 to 3 must be a signer of the list in effect
that the named list amends (section 3.4); otherwise the record is
discarded. The observer of type 4 must be a relay of the list in effect.
The reason is not checked: it need not exist, and may be any blob.

### 3.4 How a list takes effect

Let `L` be the list in effect, number `n`, with signers `S`, quorum `q`,
veto count `v`, delay `D`. A **candidate** is a list `C` that is
well-formed (section 3.1) with group ID the group's, number `n` + 1,
previous ID the ID of `L`, a time not before `L`'s and not more than 600
s after now. Only candidates of `L` can take effect next.

**(a) By quorum.** `C` takes effect as soon as the relay holds signature
or proposal records for `C` from at least `q` distinct signers of `S`.

**(b) By lapse.** A proposal for `C` by a signer of `S`, with
`T` = `C`'s time and `T` + `D` ≤ `E` ≤ `T` + 2`D`, makes `C` take
effect at `E` if, when now reaches `E`:

1. no list of number `n` + 1 has taken effect;
2. the relay holds veto records for `C` from fewer than `v` distinct
   signers of `S`; and
3. this proposal **wins**: of the accepted proposals of number `n` + 1
   whose lists are not vetoed (fewer than `v` vetoes held), it has the
   earliest `E`, and of those with that `E`, the lowest list ID.

A candidate whose successor field is not zero takes effect only by (a);
a proposal for it is refused.

**Once a list of number `n` + 1 takes effect it is final**: it is never
replaced by another list of that number, whatever records arrive later.
Every other candidate of `L`, and every proposal and veto for one, is
then **void**. If two candidates reach quorum at the same moment (the
records that complete both are processed together), the one with the
lower list ID takes effect.

The **list in effect** of a group is the genesis list, or the last list
to take effect by these rules.

### 3.5 Accepting records

A relay **accepts** a record (stores it, counts it, and forwards it to
the other relays of the group, section 3.8) only by these rules; it
refuses (status `3`) the rest. It must hold the list blob a record names
before accepting the record; if it does not, it gets the blob from the
connection that sent the record, then from the relays of the group.

- **Signatures** for a candidate: accepted until a list of number `n` +
  1 takes effect.
- **Proposals:** accepted if the list is a candidate, `E` is within the
  bounds of section 3.4 (b), the successor is zero, and the signer has
  no other **pending** proposal of number `n` + 1 at this relay (one
  that is accepted, not void and not vetoed by `v` signers). From a
  connection that is not a group link, the proposal is refused if now is
  after `T` + (`E` − `T`)/2 (the **midpoint**). From a group link it is
  accepted until `E`.
- **Vetoes:** accepted if the list is a candidate. From a connection
  that is not a group link, a veto is refused if every accepted proposal
  of that list has `E` − (`E` − `T`)/4 (its **veto deadline**) before
  now; a veto for a list with no accepted proposal is accepted (it waits
  for one). From a group link it is accepted until the list takes effect
  or is void.
- **Observations:** accepted if the observer is a relay of the list in
  effect, `From` < `To` ≤ now + 600, and the relay holds no observation
  of the same observer and subject with a later `To` whose `From` is
  within the last 86,400 s. A relay keeps at most one per observer and
  subject per day, and drops them when the list they were made under is
  no longer in effect.

Divisions are of integers, rounded down. The honest-publishing bound:
a proposal accepted anywhere was first accepted no later than its
midpoint, so every signer has at least (`E` − `T`)/4 ≥ `D`/4 of real
time (3.5 days by default) between the last moment a proposal can appear
and its veto deadline, and the relays have at least another `D`/4 to
pass a veto on before `E`.

### 3.6 Messages

| Direction | Message | Layout |
|---|---|---|
| client → relay | Group get | `0Ah`, `req` (4), group ID (32), number (4) |
| relay → client | Group get reply | `8Ah`, `req` (4), `status` (1), then if `status` is 1: number (4), `how` (1), `count` (1), `count` records, `length` (2), the list (`length` bytes) |
| client → relay | Group pending | `0Bh`, `req` (4), group ID (32) |
| relay → client | Group pending reply | `8Bh`, `req` (4), `status` (1), then if `status` is 1: `count` (2), `count` records |
| client → relay | Group submit | `0Ch`, `req` (4), record |
| relay → client | Group submit reply | `8Ch`, `req` (4), `status` (1) |
| client → relay | Group follow | `0Dh`, `req` (4), group ID (32), `on` (1), `after` (4) |
| relay → client | Group follow reply | `8Dh`, `req` (4), `status` (1), then if `status` is 1: number of the list in effect (4) |
| relay → client | Group record | `91h`, record |
| relay → client | Group change | `92h`, group ID (32), number (4), list ID (32) |
| relay → relay | Summary | `0Eh`, `req` (4), group ID (32), `what` (1), `bits` (1), `bucket` (2) |
| relay → relay | Summary reply | `8Eh`, `req` (4), `status` (1), then as section 5.4 says |

A record's length follows from its type byte (161, 197, 193 or 188
bytes); a record of another type is malformed.

- **Group get**: number `FFFFFFFFh` asks for the list in effect. `how`
  is `0` (genesis), `1` (by quorum: the records are the signature and
  proposal records that made the quorum, at most 16) or `2` (by lapse:
  the first record is the winning proposal, then every veto the relay
  holds for that list). Status `2`: the relay serves no such group or
  holds no such list.
- **Group pending** returns every accepted, not void record of the
  candidates of the list in effect (proposals, signatures, vetoes), and
  the observations held under the list in effect. The candidate lists
  themselves are got by ID with Get (N section 3).
- **Group submit**: the relay accepts or refuses the record (section
  3.5): status `1` or `3`; status `2` if it serves no such group. A
  record it already holds succeeds.
- **Group follow** with `on` 1: the relay replies, then sends a Group
  change for each list of number greater than `after` up to the one in
  effect, in order, then every pending record (as Group pending would
  return), each as a Group record, then each new list in effect and each
  newly accepted record as it comes, until `on` 0 or disconnection.
  `on` 0 stops it (status `1` even if not followed).
- These are requests and replies like any other (N section 3); on a
  group link (section 3.8) both sides may send them.

Message types are then `01h`–`0Eh` requests, `81h`–`86h` and `88h`–`8Eh`
replies, `90h`–`92h` pushed. `0Fh` is reserved as an escape for a longer
type field, since the next request type, `10h`, would make a reply of
type `90h` (section 16, item 11).

### 3.7 Storing and serving the chain

- A relay **serves** a group if its operator configured it to (by group
  ID and at least one hint). It gets the chain from the hints and
  follows it from then on, whether or not it is in the list. It serves
  at most a number of groups set by its operator.
- A relay that serves a group keeps every list from 0 to the one in
  effect, the records that made each take effect (for a lapse, also the
  vetoes held for it), and every pending record. It drops void records
  once the list after them is in effect.
- It evaluates section 3.4 on every accepted record and when its clock
  reaches each pending `E`. When a list takes effect it sends Group
  change to every connection following the group.

### 3.8 Group links

- Every relay of the list in effect (joining or active) that serves the
  group keeps a **group link** to every other one: of each pair, the
  relay whose key is lower (in byte order) dials the URL the list gives.
  It sends Challenge with bit 0 set; each side checks the other's key
  against the list. A connection on which both keys have been proven,
  both in the list, is a group link for that group, and for every other
  group both serve and both are in.
- Each accepted record is sent with Group submit on every group link
  except the one it came from. A list blob a record names is sent with
  Put first.
- A connection that is a group link is not also a peer link (N section
  5.2) for the same pair.

### 3.9 Verifying a chain (clients and relays)

From a group ID and a source of lists (a relay), a verifier checks, for
list 0: its ID is the group ID, and it is well-formed. For each list `i`
≥ 1, with list `i` − 1 already checked: it is well-formed, its group ID
and number are right, its previous ID is the ID of list `i` − 1, its time
is not before that of list `i` − 1; and by `how`:

- `1`: the records include signature or proposal records for list `i`
  from at least `q` distinct signers of list `i` − 1 (its `q`), all
  verifying;
- `2`: the proposal verifies, is by a signer of list `i` − 1, its `E` is
  within the bounds of section 3.4 (b) under list `i` − 1's `D`, `E` is
  not after now + 600, list `i`'s successor is zero, and fewer than `v`
  distinct signers of list `i` − 1 have a verifying veto among the
  records. The verifier also asks a second relay for list `i` (Group get
  by number) when it has one, and rejects the list if that relay's
  records hold `v` or more vetoes. It cannot check the acceptance
  deadlines of section 3.5 (section 16, item 2).

A verifier that has already checked list `c` of the group (a
**checkpoint**: group ID, number, list ID) starts from `c`. A client
keeps its latest checkpoint per group. A list `i` ≤ `c` that a relay
serves with another ID is a **fork**: section 7.4.

### 3.10 Exit and successors

- Content is signed by its authors and named by hash, so it belongs to no
  group. Any relays may at any time start another group (a new genesis,
  a new ID) and copy what they hold; users point at the new ID.
- A list may name a **successor** group ID, only by quorum (section 3.4).
  A client that sees a list in effect with a successor asks the relays
  of the old group for the successor's chain (relays that served one
  should serve the other), verifies it from its ID (section 3.9), tells
  the person, and with their agreement follows the successor in place of
  the old group. Nothing moves automatically.

### 3.11 Single signer

With `s` = 1, `q` = `v` = 1. The signer changes the list by signing a
signature record (quorum, at once). A proposal by the signer can be
vetoed by the same signer. No other key has any say. This is the case of
one person running a few relays, and needs no delay at all.

## 4. Responsibility

### 4.1 Score

For an item with key `K` (32 bytes) and a relay with key `r`:

    score(r, K) = BLAKE3-256(domain ‖ r ‖ K)

| Item | `K` | Domain (18 ASCII bytes) |
|---|---|---|
| A blob | its ID | `terse-place-blob-0` |
| A feed (its entries, of every kind, profiles included) | its key | `terse-place-feed-0` |

Scores are compared as 32-byte unsigned numbers, byte 0 most
significant. The **responsible relays** of `K` under a list are the `R`
active relays of that list with the highest scores (all of them if there
are `R` or fewer). Two relays with equal scores (only with equal keys,
which a list forbids) are ordered by lower key first.

The **forward set** of `K` is the responsible relays under the list in
effect, together with the `R` highest-scoring relays among its active
and joining relays (the responsible relays once the joining ones become
active).

### 4.2 Puts and publishes

- A relay that accepts a Put or a Publish **from a connection that is not
  a group link** (a client or a peer) stores it and sends it to every
  relay of the forward set other than itself, for every group it is in
  (joining or active), with the ordinary Put and Publish on the group
  links. It keeps its own copy at least until each of those has
  accepted, or 7 days, whichever is longer; it retries a failed send
  after 1 s, doubling up to 30 s.
- A put or publish **from a group link** is stored and not sent on to
  any group relay. It is passed on to peers (N section 5.2) and pushed to
  subscribers (N section 4.3) as usual.
- A blob is forwarded whatever its size (up to the limit of N section
  1), unlike peer links (which push only up to 16,384 bytes).

### 4.3 Gets

A relay asked for a blob it does not hold asks the responsible relays
under the list in effect, in order of score, one at a time, each for at
most 3 s; then, during the grace period (section 5.2), those under the
previous list; then its peers (N section 5.2). It verifies the blob
against its ID, serves it, and may keep it (a cache, which it may evict).
If all fail it answers not found. A get from a group link is answered
from the relay's store only.

### 4.4 Feeds

- The entries of feed `K` are held by the responsible relays of `K`, and
  cached by any relay that has subscribed onward.
- A client may subscribe at any relay of the group. A relay that is not
  responsible for `K` and has a client subscribed to `K` subscribes
  (once per group link) on each responsible relay of `K`, with `after` its
  own highest `seq`, and passes on what arrives as N section 5.2 does
  for peers. It re-subscribes when the responsible relays change.
- A responsible relay keeps every entry of a feed it is responsible for
  (not only the latest 256 of N section 4.3); entries are at most 293
  bytes.

### 4.5 With peering

N section 5.2 stays as it is: a relay may have peers and groups. A
peer link carries what the relay newly stores, whatever its source,
group links included; a group link carries only the forwarding of
section 4.2. A group with `R` at least its number of active relays is a
full mirror, which is what peering gives without keys or lists; peering
remains for that simple case (section 16, item 10).

## 5. Joining, leaving and repair

### 5.1 Joining

A relay is added in the `joining` state. It is not responsible for
anything (section 4.1) but is in every forward set it would be in once
active, so new content reaches it, and it copies the backlog by repair
(section 5.4) from the relays of the group, as if it were active. A
later list makes it `active`. Signers decide when (section 8.4 says what
they can see).

### 5.2 When the list changes

When a list takes effect, each relay of it recomputes, for every item it
holds or is responsible for, the responsible relays and forward sets.

- It gets what it has become responsible for, by repair (section 5.4)
  with the relays that were responsible under the previous list.
- It keeps serving everything it held under the previous list for the
  **grace period**: at least 7 days after the change, and until repair
  has shown that each item is held by its responsible relays under the
  new list. After that it may drop what it is not responsible for (it
  keeps it as a cache if it likes).
- Gets ask the previous list's responsible relays during the grace
  period (section 4.3), so a lookup does not fail while copies move.

### 5.3 Leaving

A relay is removed by a list without it. The others re-replicate by
section 5.2. A removed relay should keep serving for the grace period,
but nothing depends on it.

### 5.4 Repair

Each relay, on each group link, every hour and after each change of the
list in effect, compares with the other relay the **shared set**: the
items for which both are responsible, or for which the other is in the
forward set and this relay is responsible. It does so by Summary.

- `what` is `1` (blobs) or `2` (feeds). An item's **element** is its ID
  for a blob, and BLAKE3-256(feed key ‖ highest `seq` held (4)) for a
  feed.
- Items are put in 2^`bits` buckets (`bits` 0 to 12) by the first `bits`
  bits of `K` (byte 0 most significant).
- `bucket` `FFFFh`: the reply is, for each bucket in order, `count` (4)
  and the XOR of the elements in it (32): 36 × 2^`bits` bytes.
- Another `bucket`: the reply is `count` (4), then `count` items of that
  bucket in increasing order, each `K` (32) for blobs, `K` (32) and
  highest `seq` (4) for feeds; at most 4,096 items (status `3` if more:
  ask again with more bits).
- The asker compares with its own, lists each bucket that differs, then
  gets each missing blob by Get, and for each feed where the other has a
  higher `seq`, subscribes with `after` its own. It chooses `bits` so
  that its shared set holds about 32 items per bucket.

Cost, for a relay holding `H` items of which `P` are shared with a given
link: an hourly round is two fingerprint messages of 36 × 2^`bits` bytes
(about `P` bytes each, at 32 items per bucket) plus one listing per
differing bucket. In a group of 8 relays with `R` = 2 and 1,000,000
items in all, each pair shares about 2 × 1,000,000 / 28 ≈ 71,000 items
(both kinds), 2^`bits` = 2,048 buckets, 74 KiB a message: about 1 MiB
per relay per hour for its 7 links when nothing is missing.

## 6. Audit

A relay cannot serve wrong bytes, only fail to serve, or serve old ones
(an older entry set without the newest entries).

**Spot check.** Anyone holding entries can check a relay `x` of a group:

1. From entries the auditor holds, pick at random blob IDs and feed keys
   for which `x` is responsible under the list in effect, leaving out
   items whose entries are less than an hour old.
2. Ask `x` directly: Get for a blob, Subscribe with `after` one less than
   the highest `seq` the auditor holds for a feed.
3. Verify. A blob or entry served and verified passes. Not found, no
   reply within 10 s, a blob that fails its ID, or a feed whose highest
   `seq` is below the auditor's fails. Note the time taken.

**Evidence** is the auditor's own log: the items, times and results. It
is not a cryptographic proof (a relay's replies are not signed, and "not
found" proves nothing to a third party); anyone can repeat the same
checks, which is what makes it worth something. Removing a relay is a
governance act, a new list (section 3.4), not an automatic consequence.

## 7. Clients

### 7.1 Following a group

Given a group address (section 2), a client:

1. connects to the hints, in order, until one answers, and gets lists 0
   to the one in effect with Group get, verifying the chain (section
   3.9), from its checkpoint if it has one;
2. picks relays (section 7.2), connects, challenges each (section 1) and
   compares its key with the list;
3. follows the group on each picked relay (Group follow, `after` its
   checkpoint) and from then on applies each Group change after
   verifying it.

It never needs to compute responsibility: it puts, publishes, gets and
subscribes at its picked relays, which do the rest. It may compute it to
send a Get straight to a responsible relay.

### 7.2 Choosing relays

A client keeps a **client seed**, 32 random bytes made once and never
sent. It ranks the active relays of the list in effect by BLAKE3-256(
`terse-pick-0` (12) ‖ relay key ‖ client seed), highest first, and uses
the first two that connect and pass the challenge. The rule spreads
clients evenly and changes a client's relays only when one of them
leaves or fails.

### 7.3 When its relays leave

When a list removes a relay a client uses, or makes it `joining`, the
client connects to the next in its ranking, and keeps the old
connection until the new one has passed its challenge and caught up.
A relay that fails is retried as N section 5.1 says; after 60 s of
failure the next is used meanwhile.

### 7.4 Forks

If two relays serve different lists of the same number, or a relay
serves a list that disagrees with the client's checkpoint, the client
stays at its checkpoint, tells the person, and does not follow either
branch further until the person chooses.

### 7.5 Several groups

A client may follow several groups, each with up to two picked relays,
within its 16 connections (N section 5.1); one connection to a relay
serves every group it uses that relay for. A relay may serve and belong
to several groups; it keeps their chains, links and forwarding apart.

## 8. What signers decide with

Because silence is consent, a signer who is not told is a signer who
consented. These rules make sure each signer is told, and told enough.

### 8.1 The difference

A proposal (and any candidate) is presented as the **difference** from
the list in effect, computed mechanically, never as "a new list". The
difference is these lines, in this order:

1. for each relay key in either list, in increasing key order:
   `relay added <key> <state> <url> operator <feed key|none>`;
   `relay removed <key>`; or, for one in both, any of
   `relay <key> state joining → active` (or back),
   `relay <key> url <old> → <new>`,
   `relay <key> operator <old> → <new>`;
2. `signer added <key>`, `signer removed <key>`, in key order;
3. each parameter that changed, in the order of section 3.1:
   `q 3 → 4`, `v`, `D` (in days and seconds), `R`, `successor`.

Tooling shows with it the proposer, `E` and the time left, the signers
who have signed, proposed or vetoed it (with their reasons, section
8.2), and what the change does to the counts: how many signers remain,
and how many signers (by the new `q`) a later change needs.

### 8.2 Reasons and discussion

- A proposal or veto may name, in its reason field, an ordinary Terse
  article (kind 1, in the signer's own feed) explaining it. Discussion
  happens in Terse: replies are articles whose entries have the reason's
  blob ID as reply-to (N section 4.2).
- A client shows a proposal or veto with no reason as "no reason given",
  and still shows it.

### 8.3 Who runs a relay

Each relay in a list may name its operator's feed key. A client shows
the operator's profile in force (name with key, N section 4.6) beside
the relay, and lets the signer open its articles and follow the feed. A
relay with no operator is allowed and is shown as "anonymous operator".
The operator field is the list writer's claim: nothing proves that the
key's owner runs the relay. An operator may confirm it with an article
in their feed naming the relay key; tooling may show whether such an
article is held.

### 8.4 Evidence a signer can gather

A signer's own relay (or node), automatically, against every relay of
the list in effect, joining ones included:

- runs the spot check of section 6 a few times an hour, and records
  passes, failures and time to serve;
- records how often it can connect, and whether the challenge succeeds;
- for a joining relay, counts the items it holds that the joining relay
  will be responsible for, and how many of them the joining relay has
  (by Summary, section 5.4, or by spot checks), so progress of copying
  is known.

It may publish a summary of this as an observation record (section
3.3), at most one per subject per day. **Proposal:** observations are
optional. Local observation is what a signer should trust; observations
from other relays add views from other networks and catch a relay that
serves some relays and not others. They are claims of the observer
relay, shown as such ("relay X reports"), never counted, never proof,
and a malicious relay can publish false ones about anyone. If the owner
prefers fewer moving parts, drop type 4: nothing else depends on it.

### 8.5 Being told in time

- A signer's node must follow (Group follow) every group whose list in
  effect names its key, on at least two relays of the group, so pending
  records reach it without being asked (section 3.6).
- It must surface pending proposals prominently, not in a log: how many
  are pending, for each the time left before its veto deadline and before
  `E`, the difference (section 8.1), the proposer and the reason, and
  the vetoes so far against `v`. It reminds again when a quarter of the
  time to the veto deadline is left.
- A relay sends pending records to every connection following the group,
  at once on follow and as they are accepted.

## 9. Search

Search (N section 4.7) stays per relay: a substring of a name cannot be
assigned to a relay by hash. A relay may forward a Find from a client
(not from a group link or peer) to at most 7 other active relays of its
group, wait at most 2 s, and merge their replies with its own by the
client's rule of N section 4.7 (at most 16 results). A forwarded Find is
answered from the receiver's own store only, and counts against the
forwarding relay's allowance there. A relay knows `listed` only for the
profiles it holds; with `R` small, its own answers cover a fraction of
the group, which forwarding makes up for groups of up to 8 relays.

## 10. Moving and backups for an author

This needs no group.

- An author's client should publish to at least two relays, and say so
  when it has only one.
- The profile in force's `relay` lines are the pointer to where a feed
  lives (N section 4.6). Readers follow the latest profile (N section
  5.1).
- **Moving** is: connect to the new relays, publish a new profile whose
  `relay` lines name them, publish it to the old and new relays, then
  re-publish every entry and re-put every blob the author's node holds of
  its own feed to the new relays. Readers connected to any of the old
  relays receive the profile and connect to the new ones.
- **Failure case.** If the author's only relay disappears before a new
  profile is published there, readers who reach the feed only through it
  can no longer find it: they hold its key, and nothing says where it
  went. The author still holds everything and can publish it anywhere,
  but the readers are not told. Two relays avoid this. Feed addresses
  with hints (section 2) let links name relays other than the profile's.
  A group removes it for the members' content: any relay of the group
  holds or can fetch the feed, and the group outlives a relay.
  Proposal: a profile field `group`, a group address (section 2.1), zero
  to two lines, tells readers which groups carry the feed.

## 11. Worked example

Keys are shown as their first and last four hex digits; scores as their
first byte. All values are made up but consistent.

**The group.** Signers S1 `5a11…11a5`, S2 `5b22…22b5`, S3 `5c33…33c5`,
S4 `5d44…44d5`, S5 `5e55…55e5`: `s` = 5, `q` = 3, `v` = 2, `D` =
1,209,600, `R` = 2. List 3 (ID `3e81…77c0`, time 1,793,000,000) has
relays A `a1a1…a1a1` `wss://a.example:443/`, B `b2b2…b2b2`, C
`c3c3…c3c3`, D `d4d4…d4d4` active, and E `e5e5…e5e5` joining. The group
ID is `9f3c…01d7`; the group address is
`terse:group/9f3c…01d7@b.example:443,c.example:443` (in full, 64 digits).

**Scores.**

| Relay | blob `7d3e…a902` | feed `f00d…beef` |
|---|---|---|
| A | `41` | `88` |
| B | `0c` | `f1` |
| C | `e7` | `9a` |
| D | `b9` | `17` |
| E (joining) | `52` | `c4` |

Blob: responsible C, D; forward set C, D (E is not in the top two of
A–E). Feed: responsible B, C; forward set B, C and E (B, E are the top
two of A–E).

**Publish.** The author (feed `f00d…beef`) uses relay A and a relay of
their own outside the group.

1. The client puts the article, 2,310 bytes, ID `7d3e…a902`, to both. A
   stores it and, not being responsible, puts it to C and D on its group
   links. C and D store it and send it nowhere else in the group.
2. The client publishes entry `seq` 12 pointing to it. A stores it and
   publishes it to B, C and E.
3. A reader, whose client ranked D and B first (section 7.2), follows the
   feed. D is not responsible for `f00d…beef`, so when the reader
   subscribed, D subscribed on B and C. B pushes entry 12 to D first; D
   stores it and pushes it to the reader. (C's copy arrives a moment
   later and is ignored: same entry.)
4. The reader's client downloads the 2,310-byte blob automatically (N
   section 4.4): Get at D. D is responsible and holds it.
5. Had the reader been on B instead: B does not hold `7d3e…a902`, asks C
   (score `e7`, the highest), verifies, serves and caches it.

**A change by lapse.** S2 writes list 4: E `active`, A removed, nothing
else. Its time `T` is 1,795,046,400 (2026-11-19 00:00 UTC), ID
`4c90…1be2`. The difference, as every signer's node shows it:

    relay removed a1a1…a1a1
    relay e5e5…e5e5 state joining → active

S2 puts the list and submits a proposal with `E` = 1,796,256,000
(2026-12-03 00:00 UTC, `T` + `D`) and a reason, an article in S2's feed
saying that A's operator is shutting it down and that E has copied
nearly everything (S2's relay reports E holding 70,912 of 70,940 items it will
be responsible for). S2 submits it to B at 1,795,047,000. B accepts it
(before the midpoint, 1,795,651,200, 2026-11-26) and passes it to the
other relays within seconds. Every signer's node shows: one proposal, 14
days left, 10.5 days to the veto deadline (1,795,953,600, 2026-11-29
12:00 UTC).

- S1 signs it (signature record). With S2's proposal, that is 2 of the 3
  needed for quorum: not yet in effect.
- On 2026-11-24 (1,795,478,400) S4 vetoes at D, with a reason: E is run
  by an anonymous operator. 1 veto; `v` is 2.
- Nobody else acts. S3 and S5 are silent: that is consent.
- At 1,796,256,000 each relay finds: no list 4 in effect, 1 veto (< 2),
  this the only proposal. List 4 takes effect. Each sends Group change.

**After.** Active relays B, C, D, E. Blob `7d3e…a902`: C (`e7`), D
(`b9`), unchanged. Feed `f00d…beef`: B (`f1`), E (`c4`); C no longer
responsible keeps serving for the 7-day grace period, D re-subscribes on
B and E. Items for which A was responsible get a new holder from the
other responsible relay by repair. Clients using A move to the next
relay in their ranking. The author, who was using A, sees in a Group
change that A left and moves to B (section 7.3).

## 12. Threats

| Who | Can | Cannot | Mitigation |
|---|---|---|---|
| A malicious relay in the list | Fail to serve or store what it is responsible for; serve old feed states (omit newest entries); serve some clients and not others; see who asks for what; delay or selectively forward records (section 16, item 1); publish false observations | Serve wrong bytes; forge entries, lists, proposals or vetoes; change the list | `R` ≥ 2; clients use two relays; spot checks (section 6); removal by a new list; exit |
| A compromised signer | Sign, propose (one pending proposal per number), veto; with `v` = 1, block every change by lapse, including its own removal | Change the list alone when `q` > 1; make a proposal take effect against `v` vetoes; act faster than `D` | Vetoes within `D`/4; removal by quorum; for `s` = 1, nothing: the signer is the group |
| A minority of signers (fewer than `q`, at least `v`) | Veto every lapse proposal; propose | Change the list | Quorum of the rest overrides by (a); if the rest are fewer than `q`, the group is stuck: exit (section 16, item 4) |
| A majority of signers (at least `q`) | Anything: remove relays and signers, change parameters, set a successor, at once | Alter content or earlier lists; stop relays and users from leaving | Exit to a new group; clients show every change (section 8) |
| A network partition | Make relays on either side reach different results for a lapse proposal (vetoes or a quorum seen on one side only), forking the chain | Make either side accept an invalid record | Forks are final and visible (section 7.4); signers publish early; resolution is social (exit or a successor) |
| A relay with a wrong clock | Apply a lapse early (clock fast) or late (slow); refuse honest proposals or vetoes as late, or accept late ones | Change the result at relays with right clocks | Lists more than 600 s in the future are refused; observations and clients compare; operators run NTP |

## 13. Costs

**Sizes.** A list is 117 + 32`s` + 2 + Σ(66 + URL length) bytes. Five
signers and five relays with 20-byte URLs: 117 + 160 + 2 + 430 = 709
bytes. The 16,384-byte limit allows 16 signers and 64 relays with URLs
up to 180 bytes. Records: signature 161, proposal 197, veto 193,
observation 188 bytes.

**A put** of a blob of `b` bytes by a client to one relay of a group:
2 messages to the first relay, then up to `R` + `R` puts and replies to
the forward set (up to 4 for `R` = 2 during a join, 2 or 4 at other
times): 4 to 10 messages, `b` × 3 bytes typically. A publish is the
same with an entry of at most 293 bytes, then one push per relay that
subscribed onward, from each of the `R` responsible relays.

**A get miss** at a relay that holds nothing: 2 messages client–relay,
2 per responsible relay asked (usually one): 4 messages, the blob twice.

**A membership change** in a group of `N` relays holding `M` items:
records: the proposal plus up to 16 signatures or vetoes, each sent over
`N` − 1 links by its first relay, and over every link once (about
`N`(`N` − 1)/2 × 200 bytes each). Data: adding one active relay moves
about `R`·`M`/(`N` + 1) items to it (already copied while joining);
removing one copies about `R`·`M`/`N` items, one Get and reply each.
With `N` = 8, `M` = 1,000,000 items of 4 KiB, `R` = 2: about 250,000
items, 1 GiB, across the group.

**Growth.** Every relay of a group keeps a link to every other
(`N`(`N` − 1)/2 links), repairs with each, forwards every record to
each, and a Find covers at most 8. Every list holds every relay and
every client verifies every list. This is reasonable up to about 32
relays: 496 links, 31 per relay, a list of about 3 KiB. Beyond that,
the full mesh, the per-pair repair and the 64-relay list limit break
down, and a design with partial views (relays knowing only some others)
would be needed. Signers past 16 are not allowed at all.

## 14. Summary of new constants

| Name | Value |
|---|---|
| Domain strings | `terse-relay-auth-0`, `terse-group-sign-0`, `terse-group-propose-0`, `terse-group-veto-0`, `terse-group-observe-0`, `terse-place-blob-0`, `terse-place-feed-0`, `terse-pick-0` |
| Challenge timeout | 10 s |
| Clock tolerance for a list's time | 600 s |
| Proposal first-hop deadline | `T` + (`E` − `T`)/2 |
| Veto first-hop deadline | `E` − (`E` − `T`)/4 |
| `E` bounds | `T` + `D` to `T` + 2`D` |
| `D` bounds, default | 1 to 365 days, 14 days |
| `R` bounds, default | 1 to 8, 2 |
| Signers, relays | 1–16, 1–64 |
| Grace period | 7 days and until repaired |
| Repair interval | 1 hour, and after each change |
| Find fan-out | 7 relays, 2 s |
| Picked relays per group per client | 2 |

## 15. What would change if adopted

**NETWORK.md**

- Section 1.1: a blob starting with `TGRP` is a member list, not an
  article.
- Section 3: the message table and the ranges of types (requests
  `01h`–`0Eh`, replies `81h`–`86h` and `88h`–`8Eh`, pushes `90h`–`92h`;
  `0Fh` reserved).
- A new section for relay identity (section 1 here).
- A new section for groups (sections 3 to 9 here), with responsibility,
  repair, audit and the signer's node duties.
- Section 4.3: responsible relays keep every entry of their feeds.
- Section 4.6: optionally the `group` profile field (section 10 here).
- Section 5.1: feed hints and groups as sources of relays; connection
  limits counting them. Section 5.2: coexistence with group links.
- Section 6: whether a reading client may or must follow groups (this
  proposal: may).
- Section 8: remove sharding by hash; amend relay discovery.

**ARTICLE.md**

- Section 4.1: the hint list on feed links; the group link form and its
  rule that a group link without hints is not a link; the host and port
  grammar.
- Section 4.3: examples (a feed link with hints, a trailing `.`, a
  malformed hint list, a third hint).
- Section 4.4: following a group link (show the group: its relays and
  operators, its signers, and offer to follow it); feed links with hints.
- Section 8: a reader need not use hints.

**vectors/**

- `groups.txt`: member lists (genesis and later, valid and each kind of
  invalid), group IDs, records of each type with signed messages,
  chains to verify (by quorum, by lapse with vetoes below and at `v`, a
  fork, a broken link), and the difference of section 8.1 for each pair.
- `placement.txt`: scores for blobs and feeds and the responsible and
  forward sets for a sample list; client ranking for a sample seed.
- `relay-session.txt` or a new `group-session.txt`: Challenge (both
  directions), Group get, pending, submit, follow, Summary.
- `articles/`: feed links with and without hints, group links, near
  misses; the dump format gains `link feed <key> via <hint> …` and
  `link group <id> via <hint> …`.
- `vectors/README`: the new files.

## 16. Open questions and weaknesses

1. **Lapse is not consensus.** The result of a lapse depends on which
   records each relay holds when its clock reaches `E`. A partition, or
   a malicious relay that forwards a veto or a quorum signature to some
   relays just before `E` and to others just after, splits the group
   into two chains. The two-tier deadlines (section 3.5) make honest
   records arrive early, so only an adversary or a long partition hits
   the edge, but no finite deadline removes it. The design makes forks
   final and visible rather than resolving them; resolution is exit.
   Alternatives: a lapse list could also need a later confirming
   signature by any one signer who saw no `v` vetoes (moves the problem,
   does not remove it); or one designated relay could order records (a
   central point, which this design avoids); or a real consensus
   protocol (far more complex than Terse wants).
2. **Clients cannot verify a lapse fully.** They can check the proposal
   and `E`, but not that vetoes did not exist or that deadlines were
   respected: a relay can hide vetoes. Asking two relays helps only if
   one is honest. Quorum lists do not have this problem; groups that
   care could require quorum for changes that remove signers.
3. **Clocks.** Lapse uses each relay's clock; `T` is the proposer's
   claim. A proposer can backdate `T` only by shortening its own window
   to propose (the midpoint moves with it); it cannot shorten the
   vetoers' window below (`E` − `T`)/4. A relay with a clock far off
   breaks the guarantee for proposals submitted to it. The 600 s
   tolerance is a guess. Leap seconds and 32-bit time (to 2106) as in N.
4. **Stalemate and veto abuse.** Absent signers cannot block, but present
   ones can: if at least `v` signers veto everything and the rest are
   fewer than `q`, no change is possible (with `s` = 4: 2 hostile
   signers of 4). A compromised signer with `v` = 1 can veto its own
   removal forever unless a quorum exists. With the defaults, `v`
   hostile signers still leave `q` others (`s` − `v` ≥ `q`) for `s` = 3,
   5, 6, 7, but not for `s` = 1, 2, 4. A group that has lost its quorum
   (keys lost, people gone) and has `v` hostile signers is stuck. Exit always works; the cost is that
   every reader must be told the new ID.
5. **Proposal spam.** One pending proposal per signer per number bounds
   it to `s` at a time, but a hostile signer can propose again as soon
   as the last is vetoed, forcing `v` signers to act every few days.
   Options: a signer whose proposal was vetoed may not propose again for
   that number; or each veto also counts against the proposer. Neither
   is in the design.
6. **What lapse may change.** A single signer can propose anything a
   quorum can (but a successor): replace every signer, set `q` = 1,
   shrink `D` to one day. Only vetoes stop it. A safer rule: by lapse, a
   list may not remove more than one signer, add signers, lower `q`,
   `v` or `D`; such changes need quorum. Recommended, not adopted here.
7. **Tiny signer sets.** "A third" is ⌈`s`/3⌉, at least 1: for `s` = 1
   to 3, one veto suffices, so one signer can block every lapse and
   lapse protects only against absence. For `s` = 2, `q` = 2: every
   quick change needs both, and either alone can block a lapse. Groups
   of 2 signers are therefore as rigid as unanimity; 3 is the smallest
   that tolerates one absent and one hostile signer by quorum.
8. **Long-range attacks.** Keys of former signers, if later leaked, can
   sign a valid alternative branch from the list where they were a
   quorum. A client verifying from genesis cannot tell which branch is
   real; relays and checkpointed clients can (first in effect is final).
   Mitigations: checkpoints shipped with group addresses (a number and
   list ID in the link?), clients comparing relays, signers destroying
   retired keys. A newcomer with no checkpoint trusts its hints.
9. **Who may submit, and relay load.** Any connection can submit
   records; only signed ones by current signers are accepted, so the
   cost is signature checks. Candidate lists are ordinary blobs anyone
   can put. Observations from 64 relays about 64 relays daily are 4,096
   records a day: bounded but not small; another reason to drop them.
10. **Is hash-assigned storage needed at all?** Terse articles are a few
    KiB; entries at most 293 bytes. 10,000 active authors writing two
    4 KiB articles a day is 80 MB a day, 29 GB a year, plus 6 MB a day
    of entries. A full mirror of that fits on any server for decades
    and costs each relay 80 MB a day of inbound traffic. Sharding pays
    only for large blobs (documents up to 1 MiB, program images up to
    128 KiB, a future of images) or for relays run on very small
    machines. Simpler alternative: mirror every entry and every blob up
    to 16 KiB on every relay of the group (no responsibility, repair by
    whole-set summaries, Find covers all, no forwarding fan-in), and
    shard only larger blobs by section 4. The present design already
    becomes a mirror when `R` ≥ the number of active relays. I would
    consider the hybrid before building section 4 as written.
11. **Message type space.** With replies at request + 80h and pushes at
    90h, request types end at `0Eh`. This proposal reserves `0Fh` as an
    escape; it should be decided before anything else takes it.
12. **Hints.** `wss` then `ws` invites a downgrade (harmless for content,
    which is verified, but it exposes what is read). Hints cannot carry
    a path or a scheme. Hints put relay host names and addresses into
    articles, where they outlive the relays. A reader of today shows the
    hint list as text after the link, which is acceptable.
13. **Vetting joining relays.** Nothing in the protocol vets a relay; a
    list may even name a relay that never agreed to serve the group (it
    then simply fails its spot checks). A relay could be required to
    sign a record accepting membership before it may be `active`. Sybil
    relays (one operator, many keys) are visible only through the
    operator field, which is a claim.
14. **Relay identity on `ws://`.** The challenge cannot detect a relay of
    the path forwarding to the real one. Group relays should use
    `wss://`; the current prototype supports only `ws://` (README).
15. **Freshness and withholding.** Hash naming detects wrong bytes, not
    missing ones. A responsible relay can hide a feed's newest entries,
    or a newer list. Two relays per client and audits by highest `seq`
    reduce this; nothing prevents it.
16. **Privacy.** A relay sees its clients' addresses and what they ask
    for; a relay that fetches onward reveals to the responsible relays
    which items its clients want (not who). Rendezvous assignment means
    a feed's responsible relays see all interest in it from the group.
    Subscriptions are visible to the relays they are made at.
17. **Abuse and storage.** Anyone can put to a group relay; each put is
    stored `R` times (more during joins) in every group the relay is in.
    Responsible relays may not evict (section 4.4), so a store limit
    becomes a group-level problem. N section 8 already leaves abuse
    limits open; groups make them more urgent.
18. **Author equivocation.** Two different entries with the same feed
    and `seq`: N keeps the first received, which can now differ between
    responsible relays, and repair (which compares highest `seq` only)
    will not notice. Relays could keep both as evidence.
19. **Relay key rotation** needs a new list; a relay that loses its key
    must be removed and re-added.
20. **The decision is social.** Everything section 8 provides is
    evidence about behaviour so far, seen from a few places. In the end
    a signer decides whether to trust an operator. Not available to a
    signer, by anything in this design: what a relay logs and for how
    long; who else it shares data with (other operators, its hosting
    provider, authorities); whether one person runs several relays
    under different keys and operator feeds; where it is hosted and
    under which jurisdiction; whether it will keep running; whether
    the operator field names the real operator (unless they confirm
    it); and how it behaves toward clients other than the observers.
    Tooling should say so plainly beside the evidence it does show.
