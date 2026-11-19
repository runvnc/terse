# Relay groups — proposal, version 2 (draft, not adopted)

Status: a proposal for the project owner to read and correct. Nothing
here is built or changes `NETWORK.md`, `ARTICLE.md` or `vectors/` until
adopted (section 14). Known holes are in section 15.

**Changes from version 1.**
- Every relay of a group mirrors every entry and every blob up to 16,384 bytes; responsibility by hash is gone (appendix A).
- Repair is one set comparison between mirrors; joining is "copy everything, then become active"; leaving moves nothing.
- Lapse may make only one small, mechanically checked change; signers, parameters and successors need a quorum. Defaults are given for 1 to 5 signers.
- Clients detect stale or withholding relays by comparing two relays of a group, and keep checkpoints, which group addresses may carry.
- The observation record is dropped; new messages use one extended message type.

It extends `NETWORK.md` (N) and `ARTICLE.md` (A), and fills in part the
N section 8 item "Relay discovery beyond configuration". "Must",
"should" and "may" are as in N; integers are little-endian; `‖` is
concatenation; **now** is the clock of whoever applies a rule. A
**small** blob is at most 16,384 bytes (the automatic download limit, N
section 4.4); a **large** blob is any other.

## 1. Extended messages

N section 3 leaves only `09h`–`0Fh` for requests (replies are request
+ `80h`, pushes from `90h`). This proposal takes `0Fh` as an escape.

| Direction | Message | Layout |
|---|---|---|
| any → relay | Extended request | `0Fh`, `req` (4), `ext` (2), body |
| relay → any | Extended reply | `8Fh`, `req` (4), `ext` (2), `status` (1), body |
| relay → any | Extended push | `9Fh`, `ext` (2), body |

- A relay that knows `0Fh` but not the `ext` replies status `4` (not
  supported). Relays of the current N ignore unknown types (N section
  3) and so never answer: a sender with no reply to its first extended
  request in 10 s sends that relay no more. (Older relays close the
  connection.) Clients ignore replies and pushes they do not use.
- `ext` of requests: `0001h` Challenge (2); `0002h`–`0005h` Group get,
  pending, submit, follow (4.6); `0006h` Have (5.2); `0007h` Summary,
  `0008h` Listing (5.3). Of pushes: `0001h` Group record, `0002h` Group
  change. Tables below give bodies; a reply's follows `status`.

## 2. Relay identity

Each relay has an Ed25519 **relay key** (valid, N section 4.1) and
**relay URLs** in canonical form: scheme and host in lowercase, explicit
port, a path (`/` if none), nothing else (`wss://a.example:443/`).

| Message | Body |
|---|---|
| Challenge | `flags` (1), challenge (32), `n` (1), URL (`n`) |
| Challenge reply | relay key (32), signature (64) |

- The challenge is 32 random bytes; the URL is the one dialled (in a
  reverse challenge, the receiver's URL in the list). The relay refuses
  (`3`) `n` 0 or a URL not its own, else signs `terse-relay-auth-0` (18)
  ‖ challenge ‖ `n` ‖ URL. `flags` bit 0 asks the receiver to challenge
  back; other bits are 0.
- The sender checks key and signature (strict); on failure or no reply
  in 10 s the connection has failed (N section 5.1). On `wss://` the
  certificate and signed URL bind key and host. A relay used only as a
  plain relay needs no challenge: content is verified anyway.

## 3. Addresses with hints

| Form | Names |
|---|---|
| `terse:feed/` key [`@` hint [`,` hint]] | A feed, with up to two relays that should hold it |
| `terse:group/` ID [`/` number `/` list ID] `@` hint [`,` hint …] | A group, with an optional checkpoint (4.8) and 1 to 4 hints |

- Keys and IDs are 64 lowercase hex digits; the number is decimal, no
  leading zero, at most 4,294,967,295. A **hint** is `host:port`: a DNS
  name (labels of 1 to 63 letters, digits and `-`, not starting or
  ending with `-`, at most 253 bytes, a letter in the last label), a
  dotted IPv4 address without leading zeros, or `[`RFC 5952 IPv6`]`; a
  port 1 to 65535, no leading zero. It means `wss://host:port/`, then
  `ws://` if TLS fails.
- **In articles** the hint list is the longest matching run of `@` and
  hints separated by `,`. If a letter or digit follows it, or it has too
  many hints, there are none and the text from `@` is text: a feed link
  stays A's feed link (as readers of today read it); a group link is not
  a link. Group links are never prefetched.
- Hints give reachability only, checked as `relay` lines are (N section
  5.1). A client subscribing from a hinted feed link uses the hints (at
  most two) if every `highest` from its relays is 0, until the feed's
  profile has `relay` lines.

## 4. Groups

A **group** is a set of relays that mirror each other (section 5),
named by a **group ID** and described by a hash chain of **member
lists**, which change only under section 4.4.

### 4.1 Member list

A blob of at most 16,384 bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | `TGRP` (ASCII) |
| 4 | 1 | Version: `0` |
| 5 | 32 | Group ID; zeros in the genesis list |
| 37 | 4 | Number: 0 for genesis, then the previous list's plus 1 |
| 41 | 32 | ID of the previous list; zeros in genesis |
| 73 | 4 | Time: Unix seconds, as claimed by its writer |
| 77 | 1 | `q`: quorum, 1 to `s` |
| 78 | 1 | `v`: veto count, 1 to `s` |
| 79 | 4 | `D`: delay in seconds, 86,400 to 31,536,000 |
| 83 | 32 | Successor group ID, or zeros |
| 115 | 1 | `s`: signers, 1 to 16 |
| 116 | 32`s` | Signer keys, increasing, distinct |
| 116 + 32`s` | 1 | `k`: relays, 1 to 32 |
| then | | `k` relays, by increasing key, distinct: key (32), state (1: `1` joining, `2` active), operator feed key or zeros (32), `n` (1, 1 to 255), URL (`n`) |

Nothing follows the last relay; keys are valid; at least one relay is
active;
genesis has number 0 and zero group, previous and successor IDs. The
**group ID** is the genesis list's ID, so changing any list changes
every later ID. A blob starting `TGRP` is not shown as an article.

### 4.2 Defaults and small signer sets

Tooling writes `D` = 1,209,600 (14 days), `q` = ⌊`s`/2⌋ + 1 and `v` = max(1,
min(⌈`s`/3⌉, `s` − `q`)): where possible, signers able to block a lapse
leave enough others to overrule them by quorum.

| `s` | `q` | `v` | Withstands | Does not withstand |
|---|---|---|---|---|
| 1 | 1 | 1 | nothing: the signer is the group | loss of the key (no change ever: exit); theft (the thief is the group) |
| 2 | 2 | 1 | one absent, for small changes by lapse | one key lost: signers and parameters frozen (exit); one hostile: vetoes every lapse, withholds every quorum |
| 3 | 2 | 1 | one absent or hostile | two hostile: a quorum |
| 4 | 3 | 1 | one absent or hostile | two absent: small changes only; two hostile: frozen (exit) |
| 5 | 3 | 2 | two absent or hostile; one cannot block a lapse alone | three hostile: a quorum |

A list with 2`q` ≤ `s` is allowed, but two disjoint quorums can then
fork the group (section 6).

### 4.3 Records

| Record | Layout | Signed over |
|---|---|---|
| Signature, 161 bytes | `1`, group ID, list ID, signer key, signature (64) | `terse-group-sign-0` (18) ‖ bytes 1–96 |
| Proposal, 197 bytes | `2`, group ID, list ID, `E` (4), reason, signer key, signature | `terse-group-propose-0` (21) ‖ bytes 1–132 |
| Veto, 193 bytes | `3`, group ID, list ID, reason, signer key, signature | `terse-group-veto-0` (18) ‖ bytes 1–128 |

IDs, keys and reasons are 32 bytes; `E` is the effective time; the
reason is the ID of an article, or zeros, and is not checked. The signer
must be a signer of the list the named list amends. A proposal also
counts as a signature. Other type bytes are malformed. No domain string
is a prefix of another or of `terse-entry-0`.

### 4.4 How a list takes effect

Let `L` be the list in effect, number `n`, signers `S`. A **candidate**
is a well-formed list `C` of the group, number `n` + 1, previous ID
`L`'s, time `T` not before `L`'s and at most now + 600 s. When one takes
effect, the other candidates and their records are **void**.

**(a) Quorum.** `C` takes effect when the relay holds signatures or
proposals for it from `q` signers of `S`; if one record completes two
candidates, the lower list ID wins.

**(b) Lapse**, only for a **small change**: bytes 77 to 115 + 32`s`
(`q` to the last signer key) are equal in `L` and `C`, and the relays,
matched by key and compared as bytes, differ in exactly one way:

1. `C` has every relay of `L` and one more, in state `1`;
2. same keys; one relay differs only in state, `1` to `2`;
3. `C` has every relay of `L` but one, and no other;
4. same keys; one relay differs only in `n` and URL.

Anything else (signers, `q`, `v`, `D`, successor, an operator, two
relays) needs a quorum. A proposal for a small change `C` by a signer of
`S`, with `T` + `D` ≤ `E` ≤ `T` + 2`D`, makes `C` take effect when now
reaches `E` if then (1) no list `n` + 1 is in effect, (2) the relay
holds vetoes for `C` from fewer than `v` signers of `S`, and (3) it
**wins**: of accepted proposals for `n` + 1 whose lists have fewer than
`v` vetoes, it has the earliest `E`, then the lowest list ID.

**Final.** A list in effect at a relay is never replaced there.

### 4.5 Accepting records

A relay **accepts** a record (stores, counts and forwards it) only as
follows, refusing (`3`) the rest; it first gets the named list, from the
sender, then from the group.

- **Signatures:** for a candidate, until list `n` + 1 takes effect.
- **Proposals:** for a small-change candidate, `E` in bounds, if the
  signer has no other pending (accepted, not void, fewer than `v`
  vetoes) proposal for `n` + 1 here. Not from a group link: only until
  the **midpoint** `T` + (`E` − `T`)/2; from a group link: until `E`.
- **Vetoes:** for a candidate. Not from a group link: refused once now
  is past the **veto deadline** `E` − (`E` − `T`)/4 of every accepted
  proposal for that list (with none yet, kept); from a group link: until
  the list takes effect or is void.

Division rounds down. Signers thus have at least `D`/4 (3.5 days by
default) to veto, and relays another `D`/4 to pass vetoes on.

### 4.6 Group messages

| Message | Body |
|---|---|
| Group get | group ID (32), number (4); `FFFFFFFFh`: the list in effect |
| Group get reply | number (4), `how` (1), `count` (1), records, `length` (2), the list |
| Group pending | group ID (32) |
| Group pending reply | `count` (2), records |
| Group submit | a record |
| Group follow | group ID (32), `on` (1), `after` (4) |
| Group follow reply | number of the list in effect (4) |
| Group record (push) | a record |
| Group change (push) | group ID (32), number (4), list ID (32) |

- `how`: `0` genesis; `1` quorum (its records, at most 16); `2` lapse
  (the winning proposal, then every veto held). Status `2`: no such
  group or list here. Pending returns every accepted, not void record
  for candidates of the list in effect. Submit: `1` accepted or held.
- Follow, `on` 1: the reply, a Group change for each list after `after`,
  every pending record, then each new list and record as it comes; `on`
  0 stops.

### 4.7 Serving the chain, group links

- A relay **serves** a group if configured to (group ID and a hint),
  whether or not it is listed. It keeps every list, the records that
  made each take effect and the pending records, and evaluates 4.4 on
  each accepted record and at each pending `E`.
- Every relay of the list in effect keeps a **group link** to every
  other: the lower key dials and sends Challenge with bit 0; both check
  keys against the list. Both sides send requests on it, as on a peer
  link (N section 5.2), and follow the group on each other. It serves
  every group both are in, and is not also a peer link.
- Accepted records go out with Group submit on every other group link,
  the list first with Put.

### 4.8 Verifying a chain; checkpoints

A verifier checks that list 0's ID is the group ID, and that each list
`i` ≥ 1 is well-formed with the right group ID, number and previous ID
and a time not before list `i` − 1's, and by `how`: `1`, verifying
signatures or proposals by `q` signers of list `i` − 1; `2`, a small
change, a verifying proposal by a signer, `E` in bounds and not after
now + 600, and verifying vetoes from fewer than `v` signers. It cannot
check deadlines or hidden vetoes.

A client keeps per group a **checkpoint**, the newest list it has
verified (number, ID), verifies only from it, and never accepts a chain
without it. A group address may carry one, used (after checking its ID)
in place of genesis. **Residual risk:** with no checkpoint a client
trusts its hints; a link's checkpoint is as good as its author; leaked
keys of signers retired after a client's checkpoint can build a branch
that client accepts.

### 4.9 Exit and successors

Content belongs to no group; any relays may start a new group and copy
what they hold. A list may name a **successor** (quorum only); a client
verifies its chain, tells the person, and follows it only if they agree.

## 5. Mirroring

### 5.1 Holding and forwarding

- Every relay of the list in effect, joining or active, holds every
  valid entry and small blob that any relay of the group holds, and
  keeps every entry (not only the latest 256, N section 4.3).
- A relay that newly stores an entry or small blob from a connection
  that is not a group link of group G (a client, a peer, another
  group's link) sends it with Publish or Put to every other relay of G
  on its link. It never sends on what came from G's links, so an item
  crosses each link at most once each way. A link that is down is not
  waited for; repair covers it. Large blobs are not forwarded.
- Newly stored items still go to peers and subscribers (N 4.3, 5.2).

### 5.2 Large blobs

A client puts to every relay it is connected to (N section 5.1), so a
large blob is on its two picked relays (7.1).

| Message | Body |
|---|---|
| Have | `count` (1), 1 to 16, then `count` blob IDs (32) |
| Have reply | `count` bytes, `1` held or `0` not |

A relay asked by a client or peer for a blob it lacks sends Have to the
other relays of the group it has links to, in parallel (at most 31),
waits at most 2 s, Gets from those that said `1`, one at a time, 10 s
each, then asks its peers. It verifies, serves and may cache the blob.
Have and Get from a group link are answered from the store only.

### 5.3 Repair

Two relays of a group compare two sets: **blobs**, every small blob
(key and element: its ID); **feeds**, every feed with an entry (key:
the feed key; element: BLAKE3-256(feed key ‖ highest `seq` (4) ‖
entries held (4))).

| Message | Body |
|---|---|
| Summary | group ID (32), `what` (1: blobs, 2: feeds), `p` (1: 0 or 1), prefix (`p`) |
| Summary reply | 256 × (`count` (4), XOR of the elements (32)): 9,216 bytes |
| Listing | group ID (32), `what` (1), `p` (1: 0 to 2), prefix (`p`) |
| Listing reply | `count` (4), the items in key order: key (32), for feeds also highest `seq` (4) and entries held (4); at most 4,096, else status `3` |

- A Summary splits the items whose key starts with the prefix into 256
  buckets by the next byte; a Listing lists them.
- On each group link, when it opens and hourly, each relay asks both
  Summaries at `p` 0. A differing bucket is listed if it holds at most
  64 items here, else summarised one level down (listing at `p` 2).
- It Gets each missing blob. For a feed where the other's highest is
  higher it subscribes on the link with `after` its own highest; where
  only the count differs, with `after` 0; then unsubscribes. What the
  other lacks is left to the other's repair. Repair trusts the other
  side; audit (section 9) checks it.

### 5.4 Joining and leaving

A relay is added `joining`: it has links, receives forwards, repairs
until its summaries match, and is not picked by clients. A later list
makes it `active`. A removed relay takes nothing small with it; large
blobs only it held are lost unless it puts them elsewhere first.

## 6. Forks

Lapse is not consensus: whether a veto or signature reached a relay
before its clock reached `E` can differ (a partition, a slow link, a
relay timing what it forwards), and relays can then hold different
lists of one number, each final. With mirroring only these depend on
the list: whom a relay links and forwards to; whom clients pick and
which key a challenge expects; who is `active`; and the base of the
next change.

A relay holding two lists of one number that each verify against the
previous one is in **dispute**. It keeps its own list and forwards to
its relays, but accepts group links with, takes forwarded content from
and repairs with any relay of **either** list, so content reaches every
relay of both. Its operator ends the dispute. Forks are visible (7.3),
final, and resolved by people: exit, or a successor by quorum.

## 7. Clients

### 7.1 Following and choosing relays

From a group address, a client connects to the hints in turn and gets
and verifies the lists from its checkpoint (or the address's, or
genesis). It keeps a **client seed**, 32 random bytes never sent, ranks
the active relays by BLAKE3-256(`terse-pick-0` (12) ‖ relay key ‖ seed),
and uses the first two that connect and pass the challenge, following
the group on both. When a list removes or demotes one, or it fails for
60 s, it takes the next, keeping the old until the new has caught up.
It uses them as any relays (N section 5.1).

### 7.2 Freshness

A client subscribed to a feed at its two relays of a group records, per
relay, the highest `seq` that relay has **shown**: `highest` in the
subscribe reply, and each entry pushed. No extra message is needed.

- A relay is **behind** on a feed while the other has shown a valid
  entry with a higher `seq` more than 120 s ago and it has not; likewise
  for the list number in follow replies and Group changes.
- The client already has the entry from the other relay, and gets
  blobs from either. It notes the event (relay, feed, `seq`, time) and
  shows the counts with the relay's status. It may publish the missing
  entries to the behind relay, which repairs an honest one, and may move
  to its next relay if one stays behind for an hour.
- It accuses no one: nothing is published or sent to signers. An author
  following their own feed sees the same way whether a publish spread.

### 7.3 Forks and several groups

If its relays serve different lists of one number, or one without its
checkpoint, the client stays at its checkpoint, keeps reading, tells the
person, and follows neither branch until they choose. A client may
follow several groups, two relays each, within 16 connections. A relay
may serve and belong to several groups, keeping chains and links apart;
it mirrors the union of their content (5.1 forwards between them).

## 8. Signers

Silence is consent, so a signer must be told.

- **Being told.** A signer's node follows every group whose list names
  its key, on two relays, which push pending records at once. It shows
  each pending proposal prominently (time to the veto deadline and to
  `E`, the difference, proposer, reason, vetoes against `v`), and again
  when a quarter of the time to the veto deadline is left.
- **The difference**, computed mechanically: per relay key `relay added
  <key> <state> <url> operator <feed|none>`, `relay removed <key>` or
  `relay <key> state|url|operator <old> → <new>`; `signer added|removed
  <key>`; changed `q`, `v`, `D`, `successor`; whether it is a small
  change.
- **Reasons** name an article in the signer's feed; discussion is
  replies to it. **Operators** are shown with profile and feed, as the
  list writer's claim (the operator may confirm it in an article), or as
  "anonymous operator".
- **Local evidence**, from the signer's own relay or node against each
  listed relay: connections and challenges; audits (section 9); a
  joining relay's summaries against its own (counts show progress); its
  freshness notes (7.2). Reports by other relays are left out: a hostile
  relay can fake them, and a signer can ask any relay itself.

## 9. Audit

A relay cannot serve wrong bytes, only fail to serve or serve old ones.
Every active relay should hold every entry and small blob, so anyone
can audit any relay: ask for random held entries and small blobs older
than an hour (Get; Subscribe with `after` one less than the highest
held). Not found, no reply in 10 s, a bad blob or a lower `highest`
fails. The result is the auditor's log, not a proof; anyone can repeat
it. Removal is a new list, never automatic.

## 10. Search, authors

- **Search** stays per relay (N section 4.7). Every relay of a group
  indexes the same entries and profiles, so one relay answers for the
  whole group: version 1's partial indexes and Find fan-out are gone.
  Two relays answering differently is a freshness signal.
- **Authors** should publish to at least two relays (two of a group
  count). Moving: publish a new profile naming the new relays to old and
  new, then re-publish the feed to the new. If an author's only relay
  vanishes first, readers who reach the feed only through it lose it;
  two relays, hints or a group avoid this. Proposal: a profile field
  `group`, a group address, up to two.

## 11. Worked example

Signers S1–S5: `s` = 5, `q` = 3, `v` = 2, `D` = 1,209,600. List 3:
A, B, C, D active, E `e5e5…e5e5` joining. Address (shortened):
`terse:group/9f3c…01d7/3/3e81…77c0@b.example:443,c.example:443`.

**An article.** The author uses A; a reader's client picked D and B.

1. The client puts a 2,310-byte article to A, which puts it to B, C, D
   and E; they forward it to no group relay. The client publishes entry
   `seq` 12; A forwards it the same way.
2. D and B both push entry 12 to the reader, who stores it once, notes
   both have shown 12, and gets the blob from D. Had C been down, its
   next repair would have brought both.

**A change by lapse.** E's summaries now equal S2's relay's. S2 writes
list 4, list 3 but for E's state (`relay e5e5…e5e5 state joining →
active`, small change), `T` = 1,795,046,400 (2026-11-19 00:00 UTC), and
submits a proposal with `E` = `T` + `D` = 1,796,256,000 (2026-12-03) to
B at 1,795,047,000, before the midpoint 1,795,651,200. B accepts and
forwards it. Veto deadline: 1,795,953,600 (2026-11-29 12:00).

- S1 signs: 2 of 3 for quorum. On 2026-11-24 S4 vetoes (E's operator is
  anonymous): 1 veto, `v` 2. S3 and S5 are silent.
- At `E` every relay finds no list 4, 1 veto, one proposal: list 4
  takes effect, and clients may pick E.

Had list 4 also removed A, the proposal would be refused: it would need
three signatures, or two lists.

## 12. Threats

| Who | Can | Cannot | Mitigation |
|---|---|---|---|
| A listed relay | Fail to store or serve; withhold new entries or lists; treat clients differently; see requests; time records | Serve wrong bytes; forge anything | Every relay holds the same; clients compare two (7.2); audit; removal; exit |
| A signer | Sign, veto, propose small changes; with `v` = 1, block every lapse | Change signers, parameters or successor alone (`q` > 1); beat `v` vetoes; act faster than `D` | Vetoes; removal by quorum |
| ≥ `v`, < `q` signers | Veto every lapse | Change the list | Quorum of the rest (4.2), else exit |
| ≥ `q` signers | Anything, at once | Alter content or past lists; stop exit | Exit; differences shown |
| Partition, timing | A fork | Stop content (6) | Forks visible; people resolve |
| Leaked retired signer keys | A branch for clients with an older checkpoint or none | Fool clients past it | Checkpoints; comparing relays |
| Wrong relay clock | Apply a lapse early or late | Affect correct relays | 600 s tolerance; NTP |
| Anyone | Put small blobs every relay keeps | — | Limits (15) |

## 13. Costs

- **Sizes.** A list is 117 + 32`s` + Σ(66 + URL length) bytes (five
  signers and five relays with 20-byte URLs: 707); 16 signers and 32
  relays fit. Records: 161, 197, 193 bytes.
- **Content.** With `N` relays a small item is stored `N` times and
  crosses `N` − 1 links: for 10,000 authors writing two 4 KiB articles a
  day, 80 MB a day per relay (29 GB a year) plus 6 MB of entries. A
  large get miss: up to 31 Haves of 40 bytes, one Get.
- **Repair**, 8 relays, 1,000,000 items: in sync, 14 Summaries an hour
  received and 14 sent per relay, about 260 KB; a missing item, one
  more Summary and a Listing, under 10 KB; a joining relay, 32 MB of
  keys plus the items.
- **Changes** move no content. **Growth:** a full mesh, 496 links at 32
  relays (appendix A).

## 14. Changes if adopted

- **NETWORK.md:** `TGRP` blobs (1.1); extended types and status `4`
  (3); new sections for relay identity, groups, mirroring, forks,
  freshness and signers; group relays keep every entry (4.3); the
  `group` field (4.6); hints, groups and group links (5); following
  groups is a "may" (6); section 8 amended.
- **ARTICLE.md:** hinted feed links, group links, host and port grammar
  (4.1); examples (4.3); following a group link (4.4); hints optional
  (8).
- **vectors/:** `groups.txt` (lists, chains by quorum and by lapse with
  vetoes below and at `v`, each small change and near misses, a fork,
  difference text); `group-session.txt` (extended messages with an
  unknown `ext`, Challenge both ways, group messages, Have, Summary,
  Listing); `articles/` (hinted and group links; dump lines `link feed
  <key> via <hint> …`, `link group <id> at <number> <list id> via <hint>
  …`); `vectors/README`.

## 15. Open questions and weaknesses

1. **Lapse is not consensus** (6): branches no longer split content but
   govern separately, and only people rejoin them.
2. **Hidden vetoes.** A client cannot tell a relay hid vetoes; the
   small-change rule limits the damage to one relay change.
3. **Slow erosion.** Unless `v` signers veto each time, one signer can
   remove an honest relay, add a hostile one, or point a URL elsewhere
   once per `D`. Operator changes need a quorum, perhaps too strictly.
4. **Proposal spam.** A signer may propose again as soon as vetoed.
5. **Groups merge.** A relay in two groups forwards between them, so
   both hold the union; avoiding it needs per-group tags on items.
6. **Large blobs** are not replicated by the group.
7. **Abuse.** Anyone can put small blobs that every relay must keep;
   store and abuse limits (N section 8) become urgent.
8. **Equivocation.** Two entries with one feed and `seq` can differ
   between relays; repair compares counts and highest `seq` only.
9. **Clocks.** 600 s tolerance is a guess.
10. **Hints and identity.** `ws` fallback invites a downgrade; on
    `ws://` a forwarding relay is undetectable; hints outlive relays.
11. **Vetting.** A list may name a relay that never agreed; one operator
    may run many keys; operators are claims.
12. **Privacy.** Relays see requests; a large get miss tells the group
    which blob was wanted (not by whom).
13. **The decision is social.** Logging, jurisdiction and who really
    runs a relay are invisible; tooling should say so.

## Appendix A. If groups outgrow mirroring (not normative)

10,000 active authors writing two 4 KiB articles a day is 80 MB a day,
29 GB a year per relay: any server holds that for years. At 100,000
authors (290 GB a year, 3 TB a decade), or on very small machines,
mirroring stops being reasonable; traffic (75 kbit/s per relay) is not
the limit. The likely next step is rendezvous hashing: each item held
by the `R` relays with the highest BLAKE3-256(domain ‖ relay key ‖ item
key), with repair of shared items and a grace period on list changes,
as version 1 described.
