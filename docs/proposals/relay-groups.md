# Relay groups — proposal, version 3 (draft, not adopted)

Status: a proposal for the project owner to read and correct. Nothing here is
built or changes `NETWORK.md`, `ARTICLE.md` or `vectors/` until adopted
(section 14). Known holes are in section 15. Sources of borrowed rules are in
`relay-groups-research.md` (R, with its section).

**Changes from version 2.**
- A lapse change takes effect only when more than half of the relays **seal** it; vetoes get signed receipts; double seals and hidden vetoes become proofs.
- Lapse is capped (⌊`k`/4⌋ joins or removals per quorum list); URL changes and listing need the relay's signed acceptance; vetoed proposers wait.
- Relays renew the list daily, so clients can see a frozen list.
- A group mirrors only feeds a relay has **admitted**, within quotas, tagged by group; signed drop lists are advisory.
- Reconciliation is Negentropy (NIP-77) over entry IDs; author forks become fork proofs.

It extends `NETWORK.md` (N) and `ARTICLE.md` (A). "Must", "should" and "may"
are as in N; integers are little-endian; `‖` is concatenation; **now** is the
clock of whoever applies a rule; division rounds down. A **small** blob is at
most 16,384 bytes (N section 4.4); a **large** blob is any other. The **ID** of
an entry or record is BLAKE3-256 of its bytes.

## 1. Extended messages

| Direction | Message | Layout |
|---|---|---|
| any → relay | Extended request | `0Fh`, `req` (4), `ext` (2), body |
| relay → any | Extended reply | `8Fh`, `req` (4), `ext` (2), `status` (1), body |
| relay → any | Extended push | `9Fh`, `ext` (2), body |

- A relay that knows `0Fh` but not the `ext` replies status `4` (not
  supported). A sender with no reply to its first extended request in 10 s
  sends that relay no more. Clients ignore what they do not use.
- Requests: `0001h` Challenge (2); `0002h`–`0005h` Group get, pending, submit,
  follow (4.8); `0006h` Have (5.3); `0007h` Reconcile, `0008h` Item get (5.4);
  `0009h` Group forward (5.2); `000Ah` Head (8.2, optional). Pushes: `0001h`
  Group record, `0002h` Group change.
- N's Publish reply gains status `5`: **stored, not admitted** (kept by this
  relay only, 5.1). Older clients read it as refused.

## 2. Relay identity

Each relay has an Ed25519 **relay key** (valid, N section 4.1) and **relay
URLs** in canonical form: scheme and host in lowercase, explicit port, a path
(`/` if none), nothing else (`wss://a.example:443/`).

| Message | Body |
|---|---|
| Challenge | `flags` (1), challenge (32), `n` (1), URL (`n`) |
| Challenge reply | relay key (32), signature (64) |

- The challenge is 32 random bytes; the URL is the one dialled (in a reverse
  challenge, the receiver's URL in the list). The relay refuses (`3`) `n` 0 or
  a URL not its own, else signs `terse-relay-auth-0` (18) ‖ challenge ‖ `n` ‖
  URL. `flags` bit 0 asks for a challenge back.
- The sender checks key and signature (strict); on failure or no reply in
  10 s the connection has failed (N section 5.1).

## 3. Addresses with hints

| Form | Names |
|---|---|
| `terse:feed/` key [`@` hint [`,` hint]] | A feed, with up to two relays that should hold it |
| `terse:group/` ID [`/` number `/` list ID] `@` hint [`,` hint …] | A group, with an optional checkpoint (4.9) and 1 to 4 hints |

- Keys and IDs are 64 lowercase hex digits; the number is decimal, no leading
  zero, at most 4,294,967,295. A **hint** is `host:port`: a DNS name (labels of
  1 to 63 letters, digits and `-`, not starting or ending with `-`, at most 253
  bytes, a letter in the last label), a dotted IPv4 address without leading
  zeros, or `[`RFC 5952 IPv6`]`; a port 1 to 65535, no leading zero. It means
  `wss://host:port/`, then `ws://` if TLS fails.
- **In articles** the hint list is the longest matching run of `@` and hints
  separated by `,`. If a letter or digit follows it, or it has too many hints,
  there are none and the text from `@` is text: a feed link stays A's feed
  link; a group link is not a link. Group links are never prefetched.
- Hints give reachability only, checked as `relay` lines are (N section 5.1).
  A client subscribing from a hinted feed link uses the hints (at most two) if
  every `highest` from its relays is 0, until the feed's profile has `relay`
  lines. This is how a reader follows a feed no group of theirs admits (5.1).

## 4. Groups

A **group** is a set of relays that mirror each other (5), named by a **group
ID** and described by a hash chain of **member lists**.

### 4.1 Member list

A blob of at most 16,384 bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | `TGRP` (ASCII) |
| 4 | 1 | Version: `1` |
| 5 | 32 | Group ID; zeros in genesis |
| 37 | 4 | Number: 0 for genesis, then the previous list's plus 1 |
| 41 | 32 | ID of the previous list; zeros in genesis |
| 73 | 4 | `T`: Unix seconds, as claimed by its writer |
| 77 | 1 | `lapses`: counted lapse changes since the last quorum list (4.4) |
| 78 | 1 | `q`: quorum, 1 to `s` |
| 79 | 1 | `v`: veto count, 1 to `q` |
| 80 | 4 | `D`: delay in seconds, 86,400 to 31,536,000 |
| 84 | 32 | Successor group ID, or zeros |
| 116 | 2 | `qf`: admitted feeds per relay, 1 to 65,535 |
| 118 | 4 | `qb`: mirrored bytes per admitted feed per day, at least 4,096 |
| 122 | 1 | `s`: signers, 1 to 16 |
| 123 | 32`s` | Signer keys, increasing, distinct |
| 123 + 32`s` | 1 | `k`: relays, 1 to 32 |
| then | | `k` relays, by increasing key, distinct: key (32), state (1: `1` joining, `2` active), operator feed key or zeros (32), `n` (1, 1 to 255), URL (`n`) |

Nothing follows the last relay; keys are valid; at least one relay is active;
genesis has number 0, zero group, previous and successor IDs, and `lapses` 0.
The **group ID** is the genesis list's ID. A blob starting `TGRP` is not shown
as an article. Below, `k` is the number of **active** relays of the list being
amended, a **majority** is ⌊`k`/2⌋ + 1 of them, and `W` = `D`/4 is the **seal
window**.

### 4.2 Defaults and small signer sets

Tooling writes `D` = 1,209,600 (14 days), `q` = ⌊`s`/2⌋ + 1, `v` = ⌈`s`/3⌉,
`qf` = 1,000, `qb` = 131,072 (appendix A).

| `s` | `q` | `v` | Withstands | Does not withstand |
|---|---|---|---|---|
| 1 | 1 | 1 | nothing: the signer is the group | loss of the key (no quorum change: exit); theft |
| 2 | 2 | 1 | one absent, for lapse changes | one key lost: signers and parameters frozen (exit); one hostile: vetoes every lapse, withholds every quorum |
| 3 | 2 | 1 | one absent or hostile | two hostile: a quorum |
| 4 | 3 | 2 | one absent or hostile, who cannot block a lapse alone | two absent: quorum frozen, lapse works; two hostile: frozen (exit) |
| 5 | 3 | 2 | two absent or hostile; one cannot block a lapse | three hostile: a quorum |

Version 2 capped `v` at `s` − `q`, forcing `v` = 1 at `s` = 4; but two hostile
signers of four already block every quorum, so the cap only let one signer
block every lapse. Seals make every veto count as cast, so `v` is a count of
signers, not a bet on relays. With 2`q` ≤ `s`, disjoint quorums can fork (7).

### 4.3 Records

Each record starts with its type byte and is signed with Ed25519 over
`terse-group-<name>-0` ‖ every byte before the signature (no domain is a
prefix of another or of `terse-entry-0`). IDs, keys and reasons are 32 bytes;
times are Unix seconds (4). Sizes include the type byte.

| Type | Name | After the type byte | Size |
|---|---|---|---|
| 1 | `sign` | group ID, list ID, signer key, signature (64) | 161 |
| 2 | `propose` | group ID, list ID, `E` (4), reason, signer key, signature | 197 |
| 3 | `veto` | group ID, list ID, reason, signer key, signature | 193 |
| 4 | `accept` | group ID, list ID, relay key, signature | 161 |
| 5 | `seal` | group ID, number (4), list ID, `E` (4), time, relay key, `c` (1), `c` vetoing signer keys, signature | 174 + 32`c` |
| 6 | `receipt` | group ID, record ID, time, relay key, signature | 165 |
| 7 | `renew` | group ID, number (4), list ID, time, relay key, signature | 169 |
| 8 | `admit` | group ID, feed key, time, `on` (1), relay key, signature | 166 |
| 9 | `drop` | group ID, time, key, `count` (2, ≤ 400), `count` × (`what` (1: 1 feed, 2 blob), ID), signature | 135 + 33`count` |
| 10 | proof | group ID, `form` (1), `count` (1), `count` × (`length` (2), record) | varies |
| 11 | fork proof | `length` (2), entry, `length` (2), entry | ≤ 591 |
| 12 | `head` | feed key, `seq` (4), entry ID, time, relay key, signature | 169 |

Signers of the list the named list amends sign 1–3; relays sign 4–8 and 12; 9
is signed by a signer or relay of the list in effect; 10 and 11 carry signed
records. Reasons name an article, or are zeros, and are not checked. Other
type bytes are malformed.

### 4.4 How a list takes effect

Let `L` be the list in effect, number `n`, signers `S`. A **candidate** is a
well-formed list `C` of the group, number `n` + 1, previous ID `L`'s, `T` not
before `L`'s and at most now + 600 s, with an **acceptance** (type 4 naming
`C`) by each relay new in `C` or with a changed URL: a relay that never agreed
is never listed (R P1, FidoNet). When one candidate takes effect the others
and their records are **void**. A list in effect is never replaced (**final**).

**(a) Quorum.** `C` with `lapses` 0 takes effect when the relay holds
signatures (type 1) for it from `q` signers of `S`. A signer signs at most one
candidate per number; a proposal is not a signature.

**(b) Lapse**, only for a **small change**: bytes 78 to 122 + 32`s` are equal
in `L` and `C`, and the relays, matched by key and compared as bytes, differ in
exactly one way: (1) one relay more, state `1`; (2) one relay's state `1` to
`2`; (3) one relay fewer; (4) one relay's `n` and URL. `C`'s `lapses` is `L`'s
plus 1 for changes 1 and 3, else `L`'s, and at most max(1, ⌊`k`/4⌋); beyond
that a quorum is needed (R P1, slow erosion). A URL change so needs the
relay's acceptance plus one signer: a wrong URL for a known key only denies
service, and a quorum is too costly for a move.

A proposal for `C` by a signer of `S` has `T` + `D` ≤ `E` ≤ `T` + 2`D`. The
**winner** for `n` + 1 is, of accepted proposals vetoed by fewer than `v`
signers, the one with the earliest `E`, then the lowest list ID. Each active
relay of `L` **seals** the winner (type 5) once, as soon as `E` ≤ now ≤ `E` +
`W`, if (1) no list `n` + 1 is in effect there, (2) it holds vetoes for `C`
from fewer than `v` signers of `S`, and (3) it has sealed no other list `n` + 1
whose window [`E`, `E` + `W`] overlaps `C`'s. The seal names every signer whose
veto for `C` the relay holds, and is persisted before it is sent (R P1, C2SP).

`C` takes effect at a relay when now ≥ `E` − 600 and it holds valid seals for
`C` from a majority of `L`'s active relays: each names `C`, `n` + 1 and the
proposal's `E`, is dated in [`E`, `E` + `W`], and names fewer than `v`
signers. Without that by `E` + `W` + 600, `C` is void there. A window, not an
instant, because fixed instants made Tor fragile (R P1).

**(c) Quorum against lapse.** While a winner for `n` + 1 exists, a relay
accepts signatures for other candidates `n` + 1 from clients only until the
winner's veto deadline and from links only until its `E`, so a quorum list
takes effect before `E` (and no honest relay seals) or after the winner is
void. A quorum list never supersedes a sealed one; signers who disagree make
list `n` + 2 by quorum, effective at once. An unwanted list may stand until a
quorum acts, but no relay ever undoes a list.

**Guarantees.** Two majorities of `L`'s active relays share a relay, so two
lists of one number both take effect only if a relay sealed both with
overlapping windows: a proof (4.6). A **relay majority acting together** can
force through a small change (by refusing to accept or receipt vetoes) and
block any lapse (by not sealing); it cannot change signers, parameters or the
successor, and a quorum overrides it both ways (vetoes sent to honest relays;
list `n` + 2).

### 4.5 Accepting records; receipts

A relay **accepts** a record (stores, counts and forwards it) only as follows,
refusing (`3`) the rest; it first gets the named list from the sender or the
group.

- **Signatures, acceptances:** for a candidate, until list `n` + 1 takes
  effect, within 4.4 (c).
- **Proposals:** for a small-change candidate, `E` in bounds, if the signer
  has no other pending proposal for `n` + 1 here, and no proposal that reached
  `v` vetoes with a veto deadline after now − `D` (**cooldown**, R P1). From
  clients until the **midpoint** `T` + (`E` − `T`)/2; from links until `E`.
- **Vetoes:** for a candidate, one per signer and list. From clients until the
  **veto deadline** `E` − (`E` − `T`)/4 of every accepted proposal for that
  list (with none yet, kept); from links until the list takes effect or is void.
- **Seals** as 4.4, one per relay and list; the newest **renewal** per relay,
  **admission** per relay and feed, **drop list** per key; **proofs** that
  verify, one per accused key and group.

For each veto or proposal it accepts, from a client or a link, a relay
returns a **receipt** (type 6: record ID, time received) after persisting the
record; a resubmission gets the first receipt. A relay that receipted a veto by
`X` for `C` at `t` must list `X` in every seal for `C` dated after `t`. Client
vetoes close before `E`, so every veto a relay receipted from a client is in
its seal, or the seal is a proof.

### 4.6 Proofs

A **misbehaviour proof** (type 10) carries the records of one `form`:

| `form` | Records | Shows |
|---|---|---|
| 1 | two seals by one relay, one group and number, different lists, overlapping windows | double seal |
| 2 | two renewals by one relay naming different lists of one number, or a higher number dated earlier | double or rolled-back renewal |
| 3 | a seal; a receipt by that relay dated not after it, for a veto of the sealed list; the veto; the amended list | hidden veto |
| 4 | two signatures by one signer for different lists of one number; the amended list | double signature |
| 5 | two heads by one relay for one feed, the earlier-dated with a higher `seq`, or one `seq` with two entry IDs | rolled-back head |

- Anyone holding the records can build one. Relays store and forward proofs
  like other records; clients show them on the accused relay or signer;
  signers' nodes show them (9). A proof is evidence; removal needs a list.
- Whoever holds a proof against a relay (signer) does not count its seals
  (signatures) toward **any list not yet in effect there**. That is safe: it
  only removes votes while the threshold stays a majority of `k`. Applied to a
  list already in effect it would be unsafe (a relay could double-seal on
  purpose to un-make a list it sealed), so it never is; holders and
  non-holders can then briefly disagree (7).

### 4.7 Renewals

Each active relay signs a **renewal** (type 7) of the list in effect at it
every 24 hours and on each new list. A client treats its newest verified list
`n` as **stale** if it cannot get renewals dated in the last 72 hours, naming
`n` or later, from a majority of list `n`'s active relays; it says "group list
not confirmed since …" and keeps working. A higher number makes it fetch the
chain. This is TUF's freeze defence without signers having to act (R P1).

### 4.8 Group messages

| Message | Body |
|---|---|
| Group get | group ID (32), number (4); `FFFFFFFFh`: the list in effect |
| Group get reply | number (4), `how` (1), `count` (1), `count` × (`length` (2), record), `length` (2), the list |
| Group pending | group ID (32) |
| Group pending reply | `count` (2), `count` × (`length` (2), record) |
| Group submit | a record |
| Group submit reply | a receipt (veto, proposal) or nothing |
| Group follow | group ID (32), `on` (1), `after` (4) |
| Group follow reply | number of the list in effect (4) |
| Group record (push) | a record |
| Group change (push) | group ID (32), number (4), list ID (32) |

- `how`: `0` genesis (its acceptances); `1` quorum (signatures, acceptances);
  `2` lapse (proposal, seals, acceptances). Status `2`: no such group or list.
- Pending returns every accepted, not void record for candidates of the list
  in effect, the newest renewal of each relay, and every proof.
- Follow, `on` 1: the reply, a Group change for each list after `after`, the
  pending records, then each new list and record as it comes; `on` 0 stops.

### 4.9 Serving, links, verifying, exit

- A relay **serves** a group if configured to (group ID and a hint). It keeps
  every list with the records that made it take effect, pending records,
  renewals and proofs, and evaluates 4.4 on each accepted record and at each
  pending `E` and `E` + `W`.
- Every relay of the list in effect keeps a **group link** to every other:
  the lower key dials and sends Challenge with bit 0; both check keys against
  the list; both send requests. One link serves every group both are in; it is
  not a peer link (N 5.2). A record accepted from outside the group's links
  goes with Group submit on every other link; records from links are not sent
  on.
- **Verifying** list `i` ≥ 1: well-formed; group ID, number, previous ID; `T`
  not before list `i` − 1's; the acceptances; then by `how`: `1`, `lapses` 0
  and signatures by `q` signers of list `i` − 1; `2`, a small change with the
  right `lapses`, a proposal by a signer with `E` in bounds and not after now
  + 600, and majority seals as 4.4. If a held proof leaves too few, the step
  is a dispute (7). A verifier need not trust that no veto was hidden.
- A client keeps per group a **checkpoint**, the newest list it verified
  (number, ID), verifies only from it, and never accepts a chain without it.
  A group address may carry one. With none, a client trusts its hints; leaked
  keys of signers retired after a checkpoint can still build a branch.
- **Exit.** Content belongs to no group; any relays may start a new group and
  copy what they hold. A list may name a **successor** (quorum only); a client
  follows it only if the person agrees.

## 5. Mirroring

### 5.1 Admission and quotas

A group **mirrors** only (a) entries of feeds that an active relay of the list
in effect has **admitted** (type 8, `on` 1, newest by time for that relay and
feed), (b) small blobs those entries reference, (c) the group's records.
Anything else a relay accepts stays local (R P2: a server answers for whom it
admits).

- Only a relay's first `qf` admissions in force (by time) count. `on` 0
  revokes. A feed with no admission in force is no longer mirrored; what is
  held is kept.
- The **entry relay** (the first to store an item from outside the group)
  forwards a feed's entries and referenced small blobs only while the feed's
  bytes received there this UTC day are within `qb`; the rest stays local.
  Other relays refuse (status `5`) a feed's items beyond 2`qb` a day by their
  own count.
- **Asking for admission.** A profile whose `relay` lines name a relay is an
  admission request to it; its operator decides (invitation, payment, review).
  Until then the relay answers Publish with status `5`. A feed no group of a
  reader admits is still read through hints and `relay` lines (3; N 5.1).
- **Drop lists.** A relay or signer of the list in effect may publish a drop
  list (type 9) of feeds or blobs it considers abusive; a newer one by the same
  key replaces it, an empty one withdraws it. Each operator chooses in local
  configuration whose lists to honour (Usenet `control.ctl`, R P2): not
  mirroring, forwarding or serving those items.

### 5.2 Forwarding, tagged by group

| Message | Body |
|---|---|
| Group forward | group ID (32), `what` (1: 1 entry, 2 small blob), bytes |
| Group forward reply | status only: `1` mirrored, `5` stored not mirrored, `3` refused |

- On group links content goes only with Group forward. A relay that newly
  stores an item mirrored for G from outside G's links sends it to every other
  relay of G (an entry before its blob), and never sends on what came from
  G's links.
- An item arriving for G is mirrored for G only; a relay in G and H mirrors it
  for H only if it qualifies there. A feed admitted in G alone does not leak
  into H.
- Newly stored items still go to peers and subscribers (N 4.3, 5.2).
- **Joining and leaving.** A `joining` relay has links, receives forwards,
  reconciles until both sets match, and does not seal, renew or get picked; a
  later list makes it `active`. A removed relay takes nothing small with it;
  large blobs only it held are lost unless put elsewhere first.

### 5.3 Large blobs

| Message | Body |
|---|---|
| Have | `count` (1), 1 to 16, then `count` blob IDs (32) |
| Have reply | `count` bytes, `1` held or `0` not |

A relay asked by a client or peer for a blob it lacks sends Have on its group
links (at most 31, in parallel), waits at most 2 s, Gets from those that said
`1`, one at a time, 10 s each, then asks its peers; it verifies, serves and may
cache the blob. Have and Get from a link are answered from the store only.
Large blobs are not forwarded (appendix A).

### 5.4 Reconciliation

Two relays compare two sets per group with **Negentropy** protocol version 1
(`61h`), message bytes exactly as NIP-77: items ordered by (timestamp, ID);
modes Skip, Fingerprint, IdList; fingerprint = first 16 bytes of SHA-256(sum of
the IDs mod 2^256, as 32-byte little-endian integers ‖ varint count). Messages
are at most 65,536 bytes.

| Set | Item | Timestamp | ID |
|---|---|---|---|
| 1 | mirrored entry (per feed and `seq`, the two lowest IDs, 6) | its time | entry ID |
| 1 | seal, renewal, admission, drop list | its time | record ID |
| 1 | signature, proposal, veto, acceptance | `T` of the list named | record ID |
| 1 | proof, fork proof | 0 | record ID |
| 2 | mirrored small blob | least time of the mirrored entries here naming it | blob ID |

| Message | Body |
|---|---|
| Reconcile | group ID (32), `set` (1), Negentropy message |
| Reconcile reply | Negentropy message (empty when done) |
| Item get | `count` (1), 1 to 64, `count` IDs (32) |
| Item get reply | `count` × (`length` (2), the entry or record; `length` 0 if not held) |

- On each link, when it opens and hourly, each relay runs both sets as the
  Negentropy client and fetches what it lacks (Item get; Get for blobs) as
  forwarded for that group; the other side runs its own. Comparing entry IDs
  shows two entries for one `seq` (6).
- **Why not version 2's buckets with this fingerprint.** Both resist the XOR
  collision (R P4) in 2–3 round trips at 10^6 items, but hash-keyed buckets
  spread new items everywhere, so after a day's outage repair lists every key
  (32 MB); time-ordered ranges confine it. Negentropy is a few hundred lines
  needing only SHA-256, with libraries in seven languages.

## 6. Author forks

In a group, a relay or client holding two different valid entries with one
feed key and `seq` builds a **fork proof** (type 11, the entries in increasing
entry-ID order), stores it and forwards it as a record of each group that
mirrors the feed. Relays keep, per feed and `seq`, the two entries with the
lowest IDs, so they converge; subscribers still get one per `seq` (N 4.3). A
client marks the feed **forked at `seq`**, shows the first entry it received
with a warning that names the other and lets the person open it, and accepts
later entries as before: the feed is not destroyed (SSB's lesson, R P5). This
amends N 4.2: the first received is still the one shown, but the other is
kept as evidence, not discarded.

## 7. Disputes

A relay holding two lists of one number that each verify is in **dispute**:
it keeps its own list and forwards to its relays, but links with, takes content
from and reconciles with any relay of either list; its operator ends it, and
clients stay at their checkpoint and tell the person. With seals it arises only
when (1) a relay seals two lists with overlapping windows (proof form 1); (2) a
signer signs two candidates and both reach `q` (form 4; with 2`q` ≤ `s` no
double signature is needed); (3) a relay holds back seals until others have
voided a list, then releases them; (4) a proof removes a seal some relays
counted before it arrived (4.6); (5) relays already disagree about an earlier
list. 1 and 2 are provable; 3 and 4 need a relay to time what it forwards.

## 8. Clients

### 8.1 Following and choosing relays

From a group address, a client connects to the hints in turn, gets and
verifies the lists from its checkpoint (or the address's, or genesis), and
gets renewals (4.7). It keeps a **client seed**, 32 random bytes never sent,
ranks active relays by BLAKE3-256(`terse-pick-0` (12) ‖ relay key ‖ seed),
skips relays it holds a proof against, and uses the first two that connect
and pass the challenge, following the group on both. When a list removes or
demotes one, or it fails for 60 s, it takes the next, keeping the old until
the new has caught up. A client may follow several groups, two relays each,
within 16 connections.

### 8.2 Freshness of content

Subscribed to a feed at its two relays, a client records per relay the highest
`seq` it has **shown** (`highest` in the subscribe reply, each entry pushed). A
relay is **behind** while the other has shown a valid higher `seq` more than
120 s ago and it has not; likewise for the list number. The client has the
entry from the other relay; it notes the event, shows counts with the relay's
status, may publish the missing entries to the behind relay, and may move to
its next relay after an hour. It accuses no one.

**Feed heads (optional).** Head (feed key (32)) is answered with a head (type
12: highest `seq` held, its entry ID, now). Only two contradicting heads of one
relay are a proof (form 5); a relay signs nothing on receiving an entry, so
heads catch rollback, not withholding. Not worth building before the rest.

### 8.3 Forks, proofs, search

- If its relays serve different lists of one number, the client stays at its
  checkpoint, keeps reading, shows the proofs it holds, and follows neither
  branch until the person chooses.
- **Search** is per group: any relay answers for its group. A client may send
  Find (N 4.7) to one relay of each group it follows and merge as N says. No
  global search is promised (R P7).

## 9. Signers

Silence is consent, so a signer must be told. A signer's node follows every
group whose list names its key, on two relays, which push pending records at
once. It shows each pending proposal prominently (time to the veto deadline and
to `E`, difference, proposer, reason, vetoes against `v`), and again when a
quarter of the time to the veto deadline is left. It sends a veto to at least
two relays and keeps both receipts.

- **The difference**, computed mechanically: `relay added <key> <state> <url>
  operator <feed|none>`, `relay removed <key>`, `relay <key> state|url|operator
  <old> → <new>`; `signer added|removed <key>`; changed `q`, `v`, `D`, `qf`,
  `qb`, `successor`; small change or not, and `lapses` against its limit.
  Operators are shown as the list writer's claim, or "anonymous operator".
- **Evidence**, from the signer's own node: connections and challenges;
  audits (10); renewals; proofs; a joining relay's reconciliation progress;
  freshness notes (8.2); admissions against `qf`. Unsigned reports by other
  relays are left out.

## 10. Audit and authors

- **Audit.** A relay cannot serve wrong bytes, only fail to serve or serve old
  ones. Anyone can ask any active relay for random mirrored entries and small
  blobs older than an hour (Get; Item get; Subscribe with `after` one less
  than the highest held). Not found, no reply in 10 s, a bad blob or a lower
  `highest` fails. The result is the auditor's log, not a proof; removal is a
  new list, never automatic.
- **Authors** should publish to two relays (two of a group count) and name
  them in `relay` lines, which also asks for admission. Moving: publish a new
  profile naming the new relays to old and new, then re-publish the feed to
  the new. Proposal: a profile field `group`, a group address, up to two.

## 11. Worked example

Signers S1–S5: `s` 5, `q` 3, `v` 2, `D` 1,209,600, `W` 302,400. List 3 (quorum,
`lapses` 0): A, B, C, D active (`k` 4, majority 3, lapse limit 1), E joining.
Address: `terse:group/9f3c…01d7/3/3e81…77c0@b.example:443,c.example:443`.

**An article.** Author F's profile has `relay: wss://a.example:443/`. F's entry
11 gets status `5` from A. A's operator admits F; A sends the admission (166
bytes) to B–E. F puts a 2,310-byte article and publishes entry 12 (about 2,600
bytes today, under `qb`); A sends Group forward (G, entry) and (G, blob) to
B–E; entry 11 follows at the next reconciliation. A reader using D and B gets
entry 12 from both, stores it once, and the blob from D.

**A lapse change; a veto falls short.** S2 writes list 4: list 3 with E
`joining → active` (uncounted), `T` 1,795,046,400 (2026-11-19 00:00 UTC), and
proposes it with `E` 1,796,256,000 (2026-12-03) through B before the midpoint
1,795,651,200; B returns a receipt and forwards it. Veto deadline
1,795,953,600; window end 1,796,558,400. S1 signs (2 of 3). On 2026-11-24 S4
vetoes through B and keeps the receipt: 1 signer, short of `v`. At `E`, A–D
each seal list 4 naming S4 (206 bytes); with three held, list 4 is in effect,
clients may pick E, and every relay renews it.

**A double seal.** S1's list 4′ (remove A, `E′` a day later) lost. C, hostile,
seals 4′ too and sends that seal only to D. D holds C's seals for 4 and 4′,
windows overlapping, builds proof form 1 and forwards it; clients skip C. List
4 stays final with three other seals; 4′ is void at `E′` + `W` + 600. S2
proposes list 5 removing C (`lapses` 1, the limit for `k` 5).

## 12. Threats

| Who | Can | Cannot | Mitigation |
|---|---|---|---|
| A listed relay | Fail to store or serve; withhold; treat clients differently; see requests; refuse receipts; admit abuse within `qf`, `qb` | Serve wrong bytes; forge; hide a receipted veto or double-seal unproven | Two-relay comparison; renewals; proofs; audit; drop lists; removal |
| A relay majority | Force or block any small change; withhold renewals ("stale") | Change signers, parameters, quotas; undo a quorum list | Quorum: vetoes to honest relays, list `n` + 2; exit |
| A signer | Sign, veto, propose (cooldown after a veto) | Block a lapse alone when `v` ≥ 2 | Vetoes; removal by quorum |
| ≥ `v`, < `q` signers | Veto every lapse | Change the list | Quorum of the rest, else exit |
| ≥ `q` signers | Anything, at once | Alter content or past lists; stop exit | Exit; differences shown |
| An author | Equivocate; publish up to quota | Hide a fork in a group | Fork proofs |
| Anyone | Fill one relay's local store | Make the group keep it | Admission |
| Leaked retired signer keys | A branch for clients with an older checkpoint | Fool clients past it | Checkpoints; renewals |
| Wrong relay clock | Seal late; void early | Seal validly before `E` | 600 s tolerance; `W`; NTP |

## 13. Costs

- **Sizes.** A list is 124 + 32`s` + Σ(66 + URL length) bytes (five signers,
  five relays, 20-byte URLs: 714). Records: 4.3.
- **Governance**, 8 relays / 32 relays: a lapse change, `k` seals each sent on
  `k` − 1 links, 56 / 992 messages, 11 / 200 KB in all, and a client checks 5
  / 17 seals of 206 bytes; receipts, 1.3 / 5.3 KB per veto; renewals, 56 / 992
  messages of 169 bytes a day (9.5 / 168 KB); admissions, 166 bytes sent `k` −
  1 times, 1.7 MB per relay for 10,000 feeds.
- **Content**, 10,000 authors writing two 4 KiB articles a day: 80 MB a day
  per relay plus 6 MB of entries. A large get miss: up to 31 Haves of 40 bytes.
- **Reconciliation** in sync: one round trip per set and link an hour, under
  1 KB; after a day's outage at 10^6 items with 20,000 new, about 3 round
  trips and 640 KB of IDs.

## 14. Changes if adopted

- **NETWORK.md:** `TGRP` blobs (1.1); extended types, status `4`, Publish
  status `5` (3, 4.3); in groups, fork proofs and both entries kept (4.2, 4.3);
  group relays keep every mirrored entry (4.3); `relay` lines as admission
  requests, the `group` field (4.6); new sections for this document's sections
  2 to 9; group links are not peer links (5); following groups is a "may", and
  a client that follows one must show forks and proofs (6); section 8 amended.
- **ARTICLE.md:** hinted feed links, group links, host and port grammar (4.1);
  examples (4.3); following a group link (4.4).
- **vectors/:** `groups.txt` (chains by quorum and by lapse, seals at and below
  a majority, each small change, `lapses` at and over the limit, vetoes below
  and at `v`, every proof form and near misses, fork proofs, renewals,
  difference text); `group-session.txt` (unknown `ext`, Challenge, group
  messages with receipts, Group forward in two groups, Have, a Negentropy
  exchange, Item get); `articles/` (hinted and group links); `vectors/README`.

## 15. Open questions and weaknesses

**Version 2's list, re-evaluated.** 1 *Lapse is not consensus:* reduced;
conflicting lists need a provable double seal; timing cases remain (7: 3, 4).
2 *Hidden vetoes:* fixed for receipted vetoes; a relay may refuse a veto, which
then goes elsewhere and fails only against a relay majority. 3 *Slow erosion:*
reduced (⌊`k`/4⌋ cap, acceptances). 4 *Proposal spam:* reduced (cooldown). 5
*Groups merge:* fixed (tags). 6 *Large blobs:* left (appendix A). 7 *Abuse:*
reduced (admission, quotas, drop lists); an admitted feed can still fill `qb`
daily; local stores remain N's problem. 8 *Equivocation:* fixed for detection.
9 *Clocks:* left, with more surfaces (18). 10 *Hints and identity:* left. 11
*Vetting:* reduced (acceptance); one operator may still run many keys. 12
*Privacy:* left. 13 *The decision is social:* left.

**New in version 3.**

14. **Relays now govern.** A relay majority decides every lapse change and, by
    withholding renewals, can make every client see "stale". With `k` = 2 one
    relay blocks every lapse; with `k` = 1 it is the lapse authority. Groups
    should keep at least 3 active relays.
15. **Seal hold-back** (7, case 3) allows an unprovable fork, if a relay sits
    on seals for days while the others are online.
16. **Proof exclusion** makes holders and non-holders briefly disagree about
    lists not yet in effect (4.6): a new, if rare, source of disputes.
17. **Denial of service.** Receipts and seals are bounded (one per signer or
    relay and list), proofs by one per accused key. Negentropy costs CPU; a
    hostile relay can flap its link to force reruns (limit runs per link).
18. **Clock skew.** The seal window, the void moment, renewal age and the
    quota day read clocks; 600 s is tolerated; an hour's error makes a relay's
    seals invalid (not counted, not a proof).
19. **Admission is gatekeeping.** Relays decide what the group carries; a feed
    admitted nowhere is absent from group search. Hints and `relay` lines keep
    it readable and any relay may admit it, but exclusion shows only as absence.
20. **Quota counts are local** (arrival day per relay): relays can disagree on
    over-quota items and reconcile them repeatedly.
21. **Complexity.** One list format, 12 record types (5 proof forms), 10
    extended requests, 2 pushes, plus Negentropy. A volunteer could build a
    client that verifies chains, renewals and freshness from this in about a
    week; a relay takes weeks, and only with the vectors. To halve it: drop
    lapse (quorum only: no proposals, vetoes, seals, receipts, windows,
    `lapses`, proof forms 1 and 3), heads and drop lists; keep renewals,
    admission, tagged forwarding, Negentropy and fork proofs.

## Appendix A. Quotas, and outgrowing mirroring (not normative)

An average author writes about 8 KiB a day; `qb` = 131,072 is 16 times that,
or 8 full small blobs. A relay admitting its full `qf` = 1,000 feeds to quota
adds at most 125 MiB a day (45 GB a year) to every relay: survivable and
removable. A group's ceiling is `k` × `qf` × `qb`: 1 GiB a day at 8 relays, 4
GiB at 32. Mirroring is cheap to about 10,000 authors (29 GB a year per relay).
A group should turn on the following when mirrored small blobs exceed about 500
GB per relay (some 50,000 authors for three years), or to keep large blobs:

- Every relay keeps every mirrored entry forever, so freshness, search and
  fork proofs stay complete.
- Small blobs are mirrored by all for 90 days from their set-2 timestamp, then
  kept only by their **placement**: the `R` = 3 active relays with the highest
  BLAKE3-256(`terse-place-0` (13) ‖ group ID ‖ relay key ‖ blob ID), compared
  as big-endian numbers. Large blobs of admitted feeds go to their placement
  from the start (Group forward from the entry relay), within `qb`.
- A relay lacking a blob asks its placement first, then the rest (5.3). Set 2
  holds blobs younger than 90 days plus those placed on both relays of the
  link. After a list change a relay drops a blob only once the three relays now
  placed have each answered Have with `1`.

At 100,000 authors and 16 relays: about 15 GB of entries and 54 GB of old
blobs a year per relay, plus 72 GB of recent blobs.
