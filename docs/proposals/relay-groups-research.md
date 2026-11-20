# Prior art for Terse relay groups (research, 2026-11-19)

A research report for `docs/proposals/relay-groups.md`; claims were checked only as far as section 5 says.

I read `NETWORK.md` and `docs/proposals/relay-groups.md`. No repository files were changed. Every non-obvious claim has a URL. "Recent" means published since about November 2025. Section 5 lists what I could not check.

## 1. One-page summary

**P1. Membership governance.** The most applicable existing answers:
- **Tor:** about nine always-online authorities vote, and clients accept a consensus only if more than half of the authorities signed it.
- **CT/C2SP witnesses:** a witness co-signs a log head only if it is a consistent extension of the last head it signed, which stops a log showing different versions to different clients.
- **ERA (2026):** a "finality node" closes epochs, so a late or backdated change cannot reorder what came before.

**Change I recommend:** keep signers for proposing and vetoing, but let a lapse take effect only when more than half of the relays in list L have each signed a **seal** for it. A seal says (list ID, the set of veto IDs this relay holds, "effective"). A relay seals at most one list per number. A relay also returns a signed **receipt** when it accepts a veto.
- Relays are always online and signers are not. So signers who are absent cannot block a change, and the decision moves to parties that can agree in time.
- Any two majorities of relays share a member, so a fork needs some relay to sign two seals. Those two signatures prove that relay misbehaved.
- A seal that leaves out a veto the same relay gave a receipt for proves a hidden veto. This turns open questions 15.1 and 15.2 from undetectable into provable.

**P2. Storage abuse.** Existing answers:
- Admission is accountable to the server that let the author in: Lobsters' invite tree, Bluesky's per-PDS rate limits that rise as the PDS earns trust, and Usenet peering.
- Signed, opt-in drop lists (Usenet's NoCeM).
- Paid relays and web-of-trust (WoT) relays. In Nostr these displaced proof of work (PoW).

**Change:** a group mirrors only (a) entries of feeds that some member relay has **admitted**, and (b) small blobs referenced by those entries. Each admitting relay gets a quota. Orphan blobs and feeds nobody admitted stay on the relay that received them. The way the proposal is written now, anyone can Put a 16 KiB blob that 32 relays must keep forever, with no entry and no accountability.

**P3. Scaling past full mirroring.** Existing answers: rendezvous or consistent hashing over a known server list (Dynamo); outbox-style interest routing (Nostr NIP-65); DHTs. IPFS's own numbers argue against a DHT for "reading never waits".

**Change:** when the time comes, shard only large and old blobs, using rendezvous hashing with R = 3. Keep every entry (at most 293 bytes) on every relay, so freshness checks, search and equivocation checks stay complete.

**P4. Set reconciliation.** Existing answers: range-based set reconciliation (RBSR) as in Negentropy / NIP-77 and Willow; Minisketch; Merkle search trees.

**Change:** stop using XOR as the summary. Use Negentropy's fingerprint: the IDs added mod 2^256, plus the count, then hashed. XOR collisions can be built in seconds. Anyone can Put blobs, so an attacker can choose items that make two relays' summaries match while their contents differ.

**P5. Withholding and equivocation.** Existing answers: SSB hash-links each message to the previous one, which makes forks detectable but unrecoverable. Witnesses replaced CT "gossip". Clients comparing two sources is the cheap baseline.

**Change:** define the feed repair element over entry IDs, not just (highest `seq`, count). Make a **fork proof** (two valid entries with the same feed and `seq`) a stored, forwarded item that clients display. At present repair cannot see equivocation at all.

**P6. Large blobs.** Existing answers: plain k-copy placement (Dynamo); erasure coding (Tahoe-LAFS 3-of-10).

**Change:** k = 2 or 3 copies by rendezvous hash among active relays, counted against the admitting relay's quota. Not erasure coding.

**P7. Search across servers.** Existing answers: per-server indexes (Mastodon, NIP-50). In practice global search ends up with one big indexer (Bluesky's AppView, nostr.band).

**Change:** none to the protocol beyond letting clients fan out to one relay per group they follow. Group mirroring already gives complete search within a group. Do not promise global search.

## 2. Detailed findings

### P1. Governance without a blockchain

**Tor.** About nine directory authorities vote each hour. Majorities count the votes actually cast, but some parameters need "a majority of all authorities". Clients and caches reject a consensus not signed by more than half of the known authorities (https://spec.torproject.org/dir-spec/computing-consensus.html).
- When no consensus forms, clients keep the old one. In January 2021 consensus building was disrupted for 30.5 hours (https://status.torproject.org/issues/2021-01-28-dir-auth).
- A 2025–26 preprint shows that knocking out a majority of authorities for five minutes, at about $53 a month, brings Tor down, because the protocol assumes synchrony. The authors propose a partially synchronous BFT replacement (https://arxiv.org/abs/2509.10755).
- **Fit:** a majority signing a document fits 3–30 servers very well. Lesson: fixed timing windows are the weak point. Terse's "evaluate at E" is the same kind of synchrony assumption, so a seal should be accepted for some window after E rather than only at E itself.

**TUF.** A new root must be signed by a threshold of both the old and the new root keys. Its version must be exactly N+1, which defeats rollback. An expiry date defeats freeze attacks (spec §5.3.4, §5.3.5, §5.3.10, https://theupdateframework.github.io/specification/latest/).
- **Fit:** Terse's quorum rule already matches TUF's chain. The missing piece is expiry: a relay can serve an old list forever without anyone noticing.
- Expiry conflicts with "non-participation must not block". A relay-sealed renewal (the seal mechanism above) supplies freshness without signers having to act.

**Transparency logs and witnesses.**
- The C2SP witness keeps only the latest checkpoint per log and co-signs only checkpoints that extend it consistently. The consistency check and the state update must be atomic. A witness may keep a rejected checkpoint as evidence (https://github.com/C2SP/C2SP/blob/main/tlog-witness.md).
- CT's gossip draft (SCT feedback, signed-tree-head pollination) is "Expired (IESG: Dead)" (https://datatracker.ietf.org/doc/draft-ietf-trans-gossip/). The ecosystem moved to witness co-signing instead.
- **Fit:** strong. Group relays are natural witnesses of their own list chain: "I sign at most one list n+1" is exactly the witness rule.
- **Failure to learn from:** gossip between clients was never deployed, partly over privacy. Don't depend on clients gossiping.

**Matrix state resolution.** Matrix replays conflicting state events deterministically over an event DAG. Delayed traffic caused "state resets" that undid membership and power levels, and a malicious server could trigger them deliberately (CVE-2025-49090).
- The fix was State Resolution v2.1 in room version 12 (August 2025). It also makes the room ID the hash of the create event (https://matrix.org/blog/2025/08/project-hydra-improving-state-res/).
- The creator still sits at the top of the hierarchy, and backdating remains possible. Matrix is "looking at… 'finality'" (same post).
- **Lesson:** a deterministic merge without finality has been exploited in practice for years. Don't move Terse lists to a CRDT.

**CRDT access control.**
- Keyhive's notebook leaves "two admins revoke each other" open (https://www.inkandswitch.com/keyhive/notebook/01/). Its later entries (October and November 2026) give a notation for authority flow, not a resolution rule (https://www.inkandswitch.com/keyhive/notebook/07/).
- ERA (Dougal, January 2026, revised April 2026) argues that an external arbiter is needed. Finality nodes emit epoch events, and later events go into a pending epoch, which defeats backdating. If the arbiter goes away, finality stalls but nothing is rewritten (https://arxiv.org/abs/2601.22963).
- ERA also summarises Kleppmann's PaPoC 2025 "seniority" rule (the more senior admin wins) and criticises it as centralising.
- p2panda-auth is an operation-based group CRDT with pluggable resolvers (https://docs.rs/crate/p2panda-auth/0.7.1).
- **Fit:** ERA's epoch is close to the relay seal recommended above, with relays as a quorum of finality nodes.

**Fork consistency and accountability.**
- SUNDR: a misbehaving server can only keep clients permanently forked, and the fork shows up once they compare notes (https://www.usenix.org/legacy/event/osdi04/tech/full_papers/li_j/li_j.pdf).
- Depot: clients carry on across forks and merge them, which is close to Terse §6 "dispute" (https://www.usenix.org/legacy/event/osdi10/tech/full_papers/Mahajan.pdf).
- PeerReview: signed logs plus witnesses make faults provable (https://www.mpi-sws.org/~druschel/publications/peerreview-sosp07.pdf).
- CONIKS: the same pattern for key directories (https://www.usenix.org/system/files/conference/usenixsecurity15/sec15-paper-melara.pdf).
- **Lesson:** the goal is not preventing forks but making each fork carry a signed proof of who caused it.

**Federations.**
- **Usenet:** each server's `control.ctl` decides whose PGP-signed newgroup and rmgroup messages it honours. The INN manual says "abuse of control messages is rampant" and that signatures are "the only reliable way" (https://man.archlinux.org/man/control.ctl.5.en). Authority is in local config, and signed messages are advisory. The Big-8 Board still removes groups after public "last call" stages (news dated 2026-10-01, https://www.big-8.org/).
- **IRC:** joining needs link configuration on both sides. Conflicts after a netsplit are settled by a deterministic rule: the older channel timestamp wins, which made "riding the split" useless (https://en.wikipedia.org/wiki/IRC_takeover; TS6 doc https://web.do.metacpan.org/dist/POE-Component-Server-IRC/view/docs/ts6.txt).
- **FidoNet:** a node was listed only after it had delivered netmail to Node 51, a liveness proof before listing. Maintaining one nodelist by hand stopped scaling at about 250 nodes and was devolved to regional hosts (https://en.wikipedia.org/wiki/FidoNet).
- **Fit for Terse:** "joining" should require the joining relay's own signed acceptance, which answers open question 15.11. A relay that never agreed cannot be listed.

**Slow erosion (open question 15.3).** No surveyed system lets one party make unlimited small changes. TUF requires a threshold for every root change. Tor requires a majority for every consensus.

**Recommendation:** count lapse changes since the last quorum-signed list. For example, at most ⌊k/4⌋ removals and URL changes in total before a quorum list is required. Also add a per-signer cooldown after a vetoed proposal, which answers 15.4.

### P2. Storage and publication abuse

**Usenet.**
- Retention expiry kept storage bounded.
- Cancels were unsigned and forgeable, and they became an abuse vector themselves.
- The Breidbart Index (the sum of √n over n cross-posted groups) gave a shared, objective threshold for what counted as spam. NoCeM replaced cancels with signed third-party notices, and each server chose whom to honour (https://en.wikipedia.org/wiki/Breidbart_Index; https://lists.isc.org/pipermail/inn-workers/2001-January/005039.html).

**Nostr.**
- NIP-13 PoW exists (https://nips.nostr.com/13). Relay-selection guides say most clients don't produce PoW, and that the relay types in wide use against spam are WoT and paid relays (search results summarised from community guides; not a primary measurement).
- The measured economics are poor. Of 712 relays, 87 were paid. 64% received no zaps, and an estimated 95% of free relays cannot cover costs from donations. The average post was replicated on 34.6 relays, and 98.2% of download traffic was redundant (Wei & Tyson, https://arxiv.org/html/2402.05709v2).
- Laurie and Clayton's analysis remains the standard reference on why PoW is a poor spam control (https://www.cl.cam.ac.uk/~rnc1/proofwork.pdf).

**Bluesky.** The Relay limits new PDS instances (for example, new-account creation at 5 per second) and says limits rise "as trust and reputation is established" (https://docs.bsky.app/docs/advanced-guides/rate-limits). That is accountability per server, not per user.

**SSB.** Replication followed the follow graph: shown at 2 hops, replicated at 3. Pubs were joined by invite (https://ssbc.github.io/scuttlebutt-protocol-guide/). Spam was naturally limited, but newcomers could not be found without a friend or a pub.

**Lobsters.** A public invitation tree "to combat spam" provides accountability (https://lobste.rs/about).

**Waku RLN.** It needs a stake on a blockchain, so it is out of scope (https://rfc.vac.dev/waku/standards/core/17/rln-relay).

**What worked at small scale:** each server answers for whom it admits, plus opt-in signed blocklists. **What failed:** PoW, unsigned cancels, and free open relays.

### P3. Scaling past full mirroring

**Dynamo.** Preference lists over a known membership, with sloppy quorum and hinted handoff (https://www.allthingsdistributed.com/files/amazon-dynamo-sosp2007.pdf). With a known, signed list of at most 32 relays, rendezvous hashing needs no routing layer at all.

**IPFS** (September 2025). Even after caching improvements, delegated-routing P95 latency is 1.35 s for successful lookups and 7.46 s for misses, with only a 57% success rate. Browsers are expected to use HTTP delegated routing because a full DHT client is "impractical" (https://blog.ipfs.tech/2025-delegated-routing-caching/). That is incompatible with "reading never waits".

**Mainline DHT, BEP 44.** Values are capped at 1,000 bytes and expire in two hours unless re-announced (https://www.bittorrent.org/beps/bep_0044.html). It is fine for pointers, but not for storage.

**AT Protocol.** The "big relay" became cheap only by not archiving. Sync v1.1 relays "no longer mirror full repository data" and run on about 2 vCPU, 12 GB RAM and 30 Mbps, with a 24-hour backfill (https://atproto.com/blog/relay-updates-sync-v1-1). Archival copies live on the users' PDS servers.

**Nostr NIP-65 (outbox model).** Authors list 2–4 write relays, and clients read from those relays (https://nips.nostr.com/65). Limiting clients to an author's top five relays cut wasted retrievals by 93% while keeping 99.4% of posts complete (Wei & Tyson).

**Cheapest design for Terse:** full mirroring of entries and of small blobs younger than some age. Rendezvous-hashed placement (R = 3) for older and larger blobs. Clients ask the hash-chosen relays first, which is still one round trip. Terse's own Appendix A estimate (29 GB a year at 10,000 authors) means this can wait.

### P4. Set reconciliation

**RBSR.** Recursively split ranges, compare fingerprints, and send ID lists when a range is small (Meyer, https://arxiv.org/abs/2212.13567; Willow, https://willowprotocol.org/specs/rbsr/index.html).

**Negentropy / NIP-77** (draft, updated 2026-06-13):
- Fingerprint = SHA-256(sum of IDs mod 2^256 ‖ varint count), first 16 bytes (https://nips.nostr.com/77).
- The author estimates about 3 round trips for 10^6 items and 4 for 10^9. Sets of tens of millions are synced routinely (https://logperiodic.com/rbsr.html).
- The same author found an XOR collision for 256-bit fingerprints in about 2 seconds by Gaussian elimination. A collision for addition took about 28 hours on 8 cores, 60 GB RAM and 1.5 TB of storage. Including the count blocks some attack classes.
- Implementations exist in C++, JavaScript, Rust, Go, C#, Kotlin and Perl (https://github.com/hoytech/negentropy).

**Minisketch.** Elements of at most 64 bits, and capacity must be predicted in advance (https://github.com/sipa/minisketch; logperiodic).

**Merkle search trees and prolly trees.** In the worst case they degrade to transferring the whole set. They are worth it only when membership proofs are needed (logperiodic).

**Verdict:** Terse's two-level 256-bucket scheme is already a fixed-shape RBSR. Swap the XOR for the Negentropy fingerprint, or adopt NIP-77's message layout and keep Terse's framing.

### P5. Withholding and equivocation

**SSB.** Each message includes a `previous` hash (https://ssbc.github.io/scuttlebutt-protocol-guide/). A forked feed "will probably be impossible to fix"; the only remedy is a new identity (Patchwork guidance, found via search). The successor protocol, PPPPP (later PZP), moved to DAG "tangles" partly for this reason (https://hackmd.io/dKmfXuy_T2CSljVCUb5gHg).
- **Lesson:** make equivocation detectable and visible, but don't brick the feed. Terse's current "first received wins" quietly hides it.

**Nostr.** Nostr has no sequence numbers, so withholding cannot be detected. A 2025 EuroS&P attack paper's main advice was "always verify signatures, never trust relays" (https://lists.w3.org/Archives/Public/public-nostr/2025Aug/0002.html).

**Cheap client checks.** Compare two relays, which Terse §7.2 already does. Keep and forward fork proofs. Optionally, relays sign periodic per-feed heads, so that withholding becomes a signed false statement rather than silence. That last idea follows the PeerReview / witness pattern.

### P6. Replicating large blobs

- Tahoe-LAFS defaults to N = 10, k = 3 erasure coding (https://tahoe-lafs.readthedocs.io/en/latest/architecture.html). It requires uploader-side encoding and storage nodes that stay up.
- At 3–30 relays, three plain copies costs only 3× storage, is trivially repairable through the existing Have/Get messages, and needs no new codec in every implementation.
- "Who pays": count copies against the admitting relay's quota (P2).

### P7. Search

- NIP-50 is an optional, per-relay search field (https://nips.nostr.com/50).
- Mastodon 4.2 full-text search is opt-in, covers public posts only, needs Elasticsearch, and searches only what that server has seen. One admin reported about 1% opt-in early on (https://mjtsai.com/blog/2023/08/25/mastodon-full-text-search/; https://blog.joinmastodon.org/2023/09/mastodon-4.2).
- Global search ends up on whoever has crawled everything: Bluesky's AppView, nostr.band.
- Terse's group mirroring already makes one relay complete for its group. That is the right scope.

## 3. What the last year changed

These are recent (since about November 2025) unless marked otherwise.

- **ERA** (January 2026, revised April 2026): concurrent revocations by equal admins need an arbiter, and epochs give finality (https://arxiv.org/abs/2601.22963).
- **Formal verification** of local-first access control with Verus, by Jacob, Stuber and Hartenstein (April 2026, https://arxiv.org/abs/2604.23560).
- **Keyhive notebook entries 06–08** (July–November 2026). Concurrent revocation is still unresolved there.
- **Tor DDoS paper**, revised March 2026 (https://arxiv.org/abs/2509.10755).
- **Range-summarizable stores** (Amparore, March 2026): speeding up fingerprint computation with augmented B+-trees, as in Anti-Entropy LMDB, which reports 4–10× (https://arxiv.org/abs/2603.19820).
- **NIP-77** was updated in June 2026.
- **IETF ATP working group** approved March 2026 (https://datatracker.ietf.org/group/atp/about/). The repository draft is at -03 as of September 2026 (https://www.ietf.org/archive/id/draft-holmgren-at-repository-03.html). The `bsky.network` relay moved to a new Sync 1.1 implementation in January 2026 (https://atproto.com/blog/2026-spring-roadmap).

Slightly older, 13–15 months:
- Rekor v2 tile-based log, first shard valid from 2025-10-06 (https://blog.sigstore.dev/rekor-v2-ga/).
- IPFS Someguy caching (September 2025).
- Matrix room version 12 (August 2025).

## 4. Reading list

1. **Tor dir-spec, consensus computation:** the majority-signed membership document, the closest working model.
2. **arXiv 2509.10755 (Tor DDoS):** why synchronous deadlines like E are fragile.
3. **TUF specification §5.3:** threshold root rotation, rollback and freeze defences for the list chain.
4. **C2SP tlog-witness:** the "co-sign one consistent successor" rule that relay seals should copy.
5. **Matrix Project Hydra post:** what goes wrong when membership is merged without finality.
6. **ERA, arXiv 2601.22963:** epochs and finality nodes, a design pattern for lapse.
7. **SUNDR, OSDI 2004:** fork consistency, the vocabulary for open question 15.1.
8. **PeerReview, SOSP 2007:** turning misbehaviour into signed proof.
9. **logperiodic.com/rbsr.html:** fingerprint choice and the XOR attack; directly fixes §5.3.
10. **NIP-77:** a spec of the right size to borrow from.
11. **Wei & Tyson, arXiv 2402.05709:** real numbers on relay economics and over-replication.
12. **atproto "Relay Updates for Sync v1.1":** the big relay survived by not archiving.
13. **IPFS delegated-routing caching post:** measured evidence against a DHT for fast reads.
14. **INN `control.ctl` and NoCeM:** signed advisory authority honoured by local config, 30 years old.
15. **SSB protocol guide:** hops, pubs and `previous`, and what forks cost.

## 5. Claims I could not verify

- The content of Kleppmann's PaPoC 2025 keynote; I only have it second-hand via the ERA paper.
- Whether Rekor v2 witnessing has shipped.
- Whether nostr.band shut down.
- That changing Tor's authority set requires a software release. I believe it does, but did not confirm it.
- Whether Meyer's paper appeared at SRDS 2023.
- Bluesky's third-party account-creation figures.
- Waku RLN's mainnet status.
- How Big-8 board members are chosen.
- That CT gossip was never deployed; I confirmed only that the draft expired.
- That volunteer Tahoe grids struggled.
- Nostr spam figures: I found no measured study, only operator anecdotes.
- Email anti-abuse consortia (DNSBLs, M3AAWG), DNSSEC root ceremonies, S/Kademlia and CONIKS gossip: I did not research these beyond citing the papers.
