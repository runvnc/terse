# §content Device 4 — Content (version 0)

Part of the Terse machine specification, v0 (draft): `SPEC.md` is the core
machine and says how these documents fit together; this one is complete
for its device, given the core sections it builds on (below).

Fetches and publishes **blobs**. A blob is 1 to 1,048,576 bytes and is named
by its BLAKE3-256 hash. The host does all hashing, verification, caching and
transport; the program only ever receives bytes that match the hash it asked
for. How the host obtains blobs is described in `NETWORK.md` and cannot be
observed by the program except through timing of completions. The host is
a client of the network in `NETWORK.md`'s terms, speaking it for its
programs; section 7 there lists what the network means for a machine host
(programs never see the protocol or a secret key, and the operations below
that change the user's feeds are asked for by programs with the user's
consent).

**Builds on** `SPEC.md`: the call convention (§calls.convention), the
frame loop, whose step 1 applies completions (§frames), events (§events),
charges (§charges) and optional devices and profiles (§image,
§profiles); display names are defined in §display-names. Named records
belong to a program's identity, which `spec/machines.md` defines
(§machines.shell). The reading profile requires this device, with the
operations §profiles lists.

## §content.request Request block

A request is described by a 52-byte block that must lie wholly in RAM, at
any alignment.

| Offset | Size | Field | Written by |
|---|---|---|---|
| 0 | 1 | `op`: `01h` get, `02h` put (`03h`–`0Dh` and `12h`: §content.feeds; `0Eh`–`11h`: §content.records) | program |
| 1 | 1 | `status` (§content.status) | device |
| 2 | 2 | Reserved; ignored | — |
| 4 | 32 | `id`: get and publish: blob hash; subscribe, unsubscribe and set nickname: feed key; save, load and delete: the record's name (§content.records). Put: the blob hash, written on completion; identity: the host's feed key, written on completion; find and find more: on completion, byte 0 is 1 if a find more has results, else 0, and the other bytes 0 (§content.feeds). | both |
| 36 | 4 | `offset`: get: byte offset into the blob; load: byte offset into the record; records (subscriptions, timeline, names, programs, records): index of the first record; find and find more: the scope, 1 to 3; set profile: bit 0 = listed, other bits ignored; publish: the entry's kind, 0 or 1 (article) or 4 (document) | program |
| 40 | 4 | `length`: get, load, records, find and find more: bytes wanted, 1 to 16,384. On completion: bytes delivered, which is fewer if the blob or record ends first. Publish: the title's length, 0 to 120, read at submit. Set profile: the name's length, 1 to 48; set nickname: 0 to 48; subscribe: the hints' length, 0 to 161; read at submit. Identity: the size of a buffer for the own address's hints, 0 (none) to 16,384; on completion, if not 0, the bytes written. | both |
| 44 | 4 | `addr`: address of the data. Get, load, records, find and find more: the `length` bytes there must lie in RAM. Put and save: the `size` bytes there must lie wholly in ROM or wholly in RAM. Publish: the title, `length` bytes of UTF-8, which must lie wholly in ROM or wholly in RAM. Find: also the query, the `size` bytes there, which must lie in RAM. Set profile and set nickname: the name, `length` bytes, which must lie wholly in ROM or wholly in RAM. Subscribe: the hints, `length` bytes, wholly in ROM or wholly in RAM. Identity: the buffer, `length` bytes, which must lie in RAM. | program |
| 48 | 4 | `size`: get: blob size, written on completion; load: the record's size, written on completion. Put: 1 to 1,048,576, read at submit; save: 0 to 65,536, read at submit. Records: the number of records that exist, written on completion. Find: the query's length, 1 to 48, read at submit; find and find more: the number of results, written on completion (find more does not read it). Publish and set profile: the new entry's `seq`, written on completion. | both |

## §content.submit Submit and completion

The device has one call:

| `a7` | Call | Arguments | Result | Class |
|---|---|---|---|---|
| `400h` | `submit` | `a0` = address of a request block | a handle 1–8, or 0 if rejected | answered |

`submit` (`400h`) with `a0` = the block's address:

1. If the 52 bytes at `a0` do not lie wholly in RAM, the submit is rejected
   (it returns 0) and nothing is written.
2. If the request is invalid (unknown `op`, a length, size, scope or
   publish kind out of range, a data range not wholly where §content.request requires, a publish
   title or find query that is not valid UTF-8, a name that is not a valid
   display name, §display-names, subscribe hints that are not a valid hint
   list, §content.feeds, a record name that is not valid, §content.records),
   the device writes status `04h` to the block and rejects the submit.
3. If eight requests are already pending, the submit is rejected and the
   block is not changed. (The result is 0 as in step 1, and the block is
   unchanged in both; a program tells them apart by knowing how many of
   its requests are pending.)
4. Otherwise the device writes status `00h`, assigns the lowest free handle
   (1–8), which the call returns, and the request becomes pending. For put
   and save, the device copies the `size` bytes at `addr` at this moment;
   for publish,
   the title; for find, the query; for set profile and set nickname, the
   name; for subscribe, the hints.

The device does not look at the other pending requests: a block that is
already pending may be submitted again, and then two requests write the
same block when they complete, the higher handle last within a frame.

At the start of a later frame (never the frame of the submit), in step 1 of
§frames, the host may complete any pending requests. Completing a
request:

- get and load, on success: copies the delivered bytes to `addr`, writes
  `length` and `size`;
- subscriptions, timeline, names, programs, records, find and find more, on
  success: copies the records to `addr`, writes `length` and `size`
  (§content.feeds, §content.records), and for find and find more `id`;
- put, on success: writes `id`;
- subscribe, unsubscribe, set nickname, save and delete, on success: nothing
  more;
- publish and set profile, on success: writes `size`;
- identity, on success: writes `id`, and, if `length` was not 0, the own
  address's hints to `addr` and `length` (§content.feeds);
- in every case writes `status`, frees the handle, and queues a content-done
  event (§events). If the event queue is full the event is dropped; the
  block is still updated.

**Without waiting on the network.** A get for a blob the host holds
completes from its store; the network is consulted only for a blob that
is not held (`NETWORK.md` section 2). Likewise subscriptions, timeline,
names, identity, programs and the named-record operations are answered
from the host's own state, and a get of a blob not held when the host
has no relay connected or connecting completes as not found. None of
these waits for a relay to connect or to answer, or for any other
request: each completes at the start of the first or the second frame
after the frame of its submit. (The frame is still an input to the
machine, §content.determinism; a headless host that completes every
request in the next frame meets this.) `programs/conform-perf` checks a
host's own frame loop against it, and `PERFORMANCE.md` says how the
reference is tested.

The device writes to the block at the address given at submit, whatever the
program has since done with that memory. Completions in one frame are
applied in increasing handle order. A get whose `offset` is at or beyond the
end of the blob completes with status `01h`, `length` 0 and `size` the
blob's size; so does a load at or beyond the end of its record, with the
record's size.

**After a restore.** A machine restored from a snapshot (§machines.snapshots)
has the requests that were pending when the snapshot was taken, and no
host will carry them out. Each of them completes with status `03h`
(failed) in the machine's first frame after the restore, in increasing
handle order, before any other completion of that frame; a request that
was waiting for the user's consent completes the same way. Nothing else
of the block is written.

## §content.status Status values

| Value | Meaning |
|---|---|
| `00h` | Pending |
| `01h` | Done |
| `02h` | Not found |
| `03h` | Failed (network error or timeout; the host decides when to give up) |
| `04h` | Bad request |
| `05h` | Refused: the user said no, the host does not permit it, the host does not provide the operation (a host with only the machine's reading profile, §profiles: put, publish, find, find more, set profile, set nickname, programs, save, load, delete and records), or the relays refused a find (§content.feeds) |
| `06h` | No room: a save would exceed the owner's limits (§content.records) |

## §content.events Events

The device's events (§events):

| Type | Name | `a` | `b` | `c` |
|---|---|---|---|---|
| `05h` | Content done | request handle | status | 0 |
| `07h` | Feeds updated | 0 | 0 | 0 |

- **Content done** is queued when a content request completes (§content.submit).
- **Feeds updated** is sent when the host has stored one or more new feed
  entries since the previous frame (§content.feeds). At most one is queued per
  frame; a second in the same frame's input is dropped. It is sent after the
  automatic download of those entries' articles has finished or failed, so
  the timeline's "local" flags are already correct. The download of a
  program's image (`NETWORK.md` 4.4), which starts once its feed is caught
  up, is announced by a feeds-updated event of its own when it ends.
  It is also sent when the subscription list changed (a machine's
  subscribe or unsubscribe, or another program of the host changing the
  list it keeps, such as a command run while the host runs): the
  timeline is that of the list as it is now,
  and the host follows the list as it is now (`NETWORK.md` 4.4), within
  about a second of the change, without being restarted.

## §content.determinism Determinism

Completions (which handle, in which frame, with what status, size and bytes)
are inputs to the machine, alongside those of §system. A request waiting
for the user's consent (§content.feeds) is simply pending; the user's answer
reaches the machine only as its completion. The failed completions of a
restore (§content.submit) follow from the snapshot, not from the host: a
replay that starts from the same snapshot gets them again.

## §content.feeds Feeds, names and search

Eleven further operations use the feeds of `NETWORK.md` section 4, with the
same request block, call, handles and completion rules as get and put.
Four read what the host holds about the feeds it has subscribed to (and,
for names, about the feeds the user gave a nickname); one
searches the relays; five change the user's subscriptions, feed, profile or
nicknames, with the user's consent; one tells the program the host's own
feed key.

| `op` | Operation |
|---|---|
| `03h` | Subscriptions |
| `04h` | Timeline |
| `05h` | Subscribe |
| `06h` | Unsubscribe |
| `07h` | Publish |
| `08h` | Identity |
| `09h` | Names |
| `0Ah` | Find |
| `0Bh` | Set profile |
| `0Ch` | Set nickname |
| `0Dh` | Programs |
| `12h` | Find more |

**Reading.** For subscriptions, timeline, names and programs, `id` is not
used;
`length` is the size of the buffer at `addr` (1 to 16,384) and `offset` is
the index of the first record wanted. The device delivers as many whole
records as fit, writes the number of bytes delivered to `length` and the
total number of records that exist to `size`.

**Subscriptions** delivers one 36-byte record per subscribed feed, in
increasing order of feed key:

| Offset | Size | Field |
|---|---|---|
| 0 | 32 | Feed key |
| 32 | 4 | Highest `seq` held for that feed, or 0 |

**Timeline** delivers the article and document entries (kinds 1 and 4,
§content.documents) of all subscribed feeds, newest first by the order in which
the host received them; profile and program entries (kinds 2 and 3) are
not included; a program tells articles and documents apart by the kind
byte of the entry (byte 40). Index 0 is the most recently received entry
(`NETWORK.md` section 4.4 defines the order). Entries of a feed the host
has unsubscribed from are not included. Each record is:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `L`: number of bytes that follow in this record |
| 1 | 1 | Flags. Bit 0: the entry's blob is in the host's store, so a get will complete without the network. Other bits are zero. |
| 2 | `L − 1` | The entry without its signature (`NETWORK.md` section 4.2, bytes 0 to 108 + `n`) |

The host has already verified every entry it delivers. When new entries
arrive, the program is told by a feeds-updated event (§events) and can
read the timeline again.

**Programs** (`0Dh`) delivers the current version of each program
(`NETWORK.md` section 4.2) of the subscribed feeds, in increasing order of
feed key, then of name (byte order), one record per program in the
timeline's record format; flag bit 0 is set when the image is in the host's
store. The name is the entry's title. A program is started by its identity
with device 8's `start_program` (§machines.calls). A host that offers only the machine's reading profile (§profiles) may complete every programs
request with status `05h`.

**Subscribe** (`05h`) adds the feed whose key is `id` to the host's
subscriptions, and **unsubscribe** (`06h`) removes it (`NETWORK.md` section
4.4). For subscribe, `length` and `addr` may give the **hints** of the
feed's address (`NETWORK.md` section 4.9): the text after its `@`
exactly, one or two hints separated by `,` (`relay.example:9000` or
`a.example,[2001:db8::7]`), 1 to 161 bytes wholly in ROM or wholly in
RAM, copied at submit; `length` 0 gives none. Hints that are not a
valid hint list, or longer than 161 bytes, make the request invalid
(§content.submit: status `04h`). The host keeps them with the subscription
and uses them as `NETWORK.md` section 4.9 says. Unsubscribe does not use
`length` and `addr`, and neither uses `offset` and `size`. The host
completes with status `01h` once the subscription list has changed (the entries
arrive later, each announced as usual by a feeds-updated event); at once,
without asking, if the list already is as asked (subscribing to a feed
already subscribed, unsubscribing from one that is not; the hints of a
feed already subscribed are not changed); and with `05h`
without asking if `id` is not a valid feed key (`NETWORK.md` section 4.1).

**Publish** (`07h`) publishes the blob whose hash is `id` as the next entry
of the host's own feed (`NETWORK.md` section 4.4): the kind given in
`offset` (0 or 1: an article, 4: a document; any other value makes the
request invalid), the size of the blob, no reply-to, the title copied at
submit and the time from the host's clock. The blob must already be in
the host's store (a program puts it first with op `02h`); if it is not,
the request completes with status `02h`, without asking. A document's
blob must start with a valid document header (§content.documents); if it does
not, the request completes with status `04h`, without asking (the host
checks this at completion, since the blob is not the program's to read at
submit). The host signs the entry with its own key, creating
its identity first if it has none. On success `size` is the new entry's
`seq`. A host that offers only the machine's reading profile (§profiles) may complete every put and publish with status `05h`.

**Identity** (`08h`): on completion `id` is the host's own feed key, or the
status is `02h` if the host has no identity yet. If `length` is not 0, the
`length` bytes at `addr` (1 to 16,384, in RAM) are a buffer for the hints
of the host's own feed address (`NETWORK.md` section 4.9): on success the
host writes there the hint list (the text after the `@`, at most 161
bytes), or as many of its hints as fit whole (the first, or none), and
sets `length` to the bytes written; 0 means the host has no hints to give
(no public relay), so its address is the bare key. A program shows the
address as the key, `@` and those bytes. `offset` and `size` are not used,
nor `addr` when `length` is 0. (A program written before hints existed
and that leaves another request's `length` and `addr` in the block may
now get the hints written there; one that leaves a `length` over 16,384,
or a range not in RAM, gets `04h`. Programs set `length` to 0, as the
library's `content_identity` does.)

**Display names.** A name given to set profile, and a non-empty name given
to set nickname, must be a valid display name (§display-names).
Otherwise the request is invalid (§content.submit).

**Names** (`09h`) delivers one record per feed that is subscribed or has a
nickname, in increasing order of feed key:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `L`: number of bytes that follow in this record, 33 to 81 |
| 1 | 1 | Flags. Bit 0: the name is the user's local nickname for the feed. Bit 1: the feed has a profile in force (`NETWORK.md` section 4.6). Bit 2: the feed is not subscribed (it is listed for its nickname). Bit 3: the host follows the feed through a relay a hint of its address names (`NETWORK.md` section 4.9), since none of its own relays holds it. Other bits are zero. |
| 2 | 32 | Feed key |
| 34 | `L − 33` | The name, 0 to 48 bytes of UTF-8 |

The name is the user's nickname for the feed if there is one (bit 0 set),
else the display name of the feed's profile in force, else empty. Bit 1 is
set whenever a profile is in force, whichever name is given (for a feed
that is not subscribed, a profile the host still holds). Names are
declared by the feeds themselves (or chosen by the user) and are not
unique: a program that shows a name should show the short key beside it.

**Find** (`0Ah`) searches the relays (`NETWORK.md` section 4.7). `id` is not
used. `offset` is the scope: 1 searches feeds by their profiles, 2 searches
articles and documents by their titles, 3 searches programs by their
names (current versions only); any other value is invalid. The query is the
`size` bytes at `addr`, 1 to 48 bytes of valid UTF-8, which must lie in RAM
and are copied at submit. `length` is the size of the buffer at `addr` for
the results (1 to 16,384, in RAM), so the results overwrite the query. On
completion the buffer holds the results as timeline records (only flag bit
0 may be set): profile entries (kind 2) for scope 1, article and document
entries (kinds 1 and 4) for scope 2, program entries (kind 3) for scope 3. As many whole results as fit are delivered, in order;
`length` is the number of bytes delivered and `size` the number of results
the host has, at most 16. The host asks all its relays, verifies every
entry they return and discards invalid ones and duplicates, and orders and
caps the merged results as `NETWORK.md` section 4.7 says. If no relay
answered with results and at least one refused the find (a relay may refuse
one from a client that searches too often), the request completes with
status `05h`; otherwise, if no relay could be asked or none answered, with
`03h`. A host that offers only the machine's reading profile (§profiles) may complete every find with status `05h`.

**Find more** (`12h`) shows the next results of the machine's last find
of the scope `offset` (1 to 3; any other value is invalid): `NETWORK.md`
section 4.7, "More results". `id` and `size` are not read; `length` is
the size of the buffer at `addr` (1 to 16,384, in RAM). It completes
like a find, with up to 16 results that the find and the find mores of
that scope since it did not deliver, in the order of `NETWORK.md` 4.7
(the host keeps each relay's cursor and the results it has delivered).
On the completion of a find or a find more with status `01h`, the first
byte of `id` is 1 if the host has more results for a find more of that
scope, else 0; the other 31 bytes are 0. A find replaces the machine's
last find of its scope. A find more completes with status `02h` if the
host holds no find of that scope from the machine that has more results
(none was made, the last had no more, or the host no longer holds it, as
after it restarted); otherwise with `05h` or `03h` as a find does. A host
keeps at least the last find of each scope of each machine that runs.

**Set profile** (`0Bh`) publishes a new profile for the host's own feed.
`id` is not used. The display name is the `length` bytes at `addr` (1 to
48, a valid display name, copied at submit); bit 0 of `offset` is set if
the name may be found by search (listed). The host builds the profile
document (`NETWORK.md` section 4.6) with that name and listing, keeping the
description of the profile in force if there is one, and with a relay line
for each of its configured relays that is not a loopback address. It puts
the document and publishes a profile entry (kind 2) for it as the next
entry of its own feed, creating its identity first if it has none. On
success `size` is the new entry's `seq`.

**Set nickname** (`0Ch`) sets the user's nickname for the feed whose key is
`id` to the `length` bytes at `addr` (0 to 48, copied at submit; a
non-empty one must be a valid display name), or removes it if `length` is
0. `offset` and `size` are not used. A nickname is local to the host and is
never sent anywhere. The host completes with status `05h` without asking if
`id` is not a valid feed key, and with `01h` without asking if the nickname
already is as asked. A host that offers only the machine's reading profile (§profiles) may complete every set profile and set nickname with status
`05h`, without asking.

**Consent.** Subscribe, unsubscribe, publish, set profile and set nickname
change what the user follows, speak in the user's name or change what the
user sees, so the host asks the user before it carries one out, and the
request stays pending until the user answers. The host draws its own prompt
over the bottom of the screen, on top of everything the program shows; the
program cannot draw it, read it or answer it. The prompt names the image's
title (in a system, that of the machine whose request it is) and what is
asked: the feed key (for example as its first and last
six hex digits, `3fa9c2…c21e07`) and, for a subscribe with hints, the
relays they name, since the host may connect to them ("… asks to
subscribe to 3fa9c2…c21e07 and look for it on relay.example:9000");
whether an article or a document is
published, its title and size; the display
name and whether it will be searchable; or the feed key and the nickname.
While a prompt is shown the keyboard's input goes to the host, not to the
program; Y allows, N or Escape refuses, and other keys (Enter included) do
nothing. One request is asked about at a time; others wait, in increasing
handle order. A refusal completes the request with status `05h`. A host
where nobody can answer (a headless run) completes such requests as its
user configured it to (by default with `05h`). Identity, names, programs
and find need no consent.

## §content.records Named records

Four operations keep small data on this host for the program: its **named
records**. They use the same request block, call, handles and completion
rules as get and put. A host that offers only the machine's reading profile (§profiles) may complete every save, load, delete and records request
with status `05h`; a program then has no named records on that host.

| `op` | Operation |
|---|---|
| `0Eh` | Save |
| `0Fh` | Load |
| `10h` | Delete |
| `11h` | Records |

**Names.** For save, load and delete, `id` holds the record's name: 1 to 32
bytes of UTF-8 with no control characters (U+0000–U+001F, U+007F–U+009F),
then `00h` to the end of the 32 bytes. Any other `id` makes the request
invalid (§content.submit). For records, `id` is not used.

**Owner.** A machine's records belong to its program's identity (§machines.shell): a machine started with `start_program`, or a shell the host started
as its built-in shell, reads and writes those of that identity, whichever
version of the program it runs. A machine with no identity (started with
`start`, or from a file) has the records of its exact image ID. So the
same built-in image has two owners: its identity when it is started with
`start_program` (or as the built-in shell), its image ID when it is
started with `start`. A program
cannot reach another owner's records. Records are kept on this host only:
the host never sends them to a relay or to any other program.

| Operation | Fields | Completion |
|---|---|---|
| Save | `addr` and `size`: the data, 0 to 65,536 bytes, wholly in ROM or wholly in RAM, copied at submit | `01h` once the record is replaced and durably stored (written to a temporary file, synced, renamed into place); `06h` if it would exceed the limits, and nothing changes |
| Load | `offset`, `length` (1 to 16,384) and `addr` (in RAM) as for get | as a get: the bytes from `offset` to `addr`, `length` = bytes delivered, `size` = the record's size; `02h` if there is no such record |
| Delete | — | `01h`, whether or not the record existed |
| Records | `offset`, `length` and `addr` as for subscriptions | records of 36 bytes, in increasing order of name (byte order): the name, padded with `00h` to 32 bytes, and the size (4) |

**Limits** per owner: 256 records and 1,048,576 bytes of data in all. A
save that would exceed either (counting the record it replaces as gone)
completes with `06h`. A host that cannot keep records (no data directory,
an I/O error) completes the request with `03h`. No consent is asked.

Records are inputs like any completion (§content.determinism): what a load delivers
and the status of a save are recorded, so a run replays without the
records directory.

## §content.documents Documents

A **document** is a blob that says which program reads it. It starts with
a header:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic: `54h 44h 4Fh 43h` (`TDOC`) |
| 4 | 1 | Version: `0` |
| 5 | 32 | The format owner's feed key; all zero for a built-in program's format |
| 37 | 1 | n: the format name's length, 1 to 48 |
| 38 | n | The format name, a valid display name (§display-names) |
| 38 + n | — | The data, in the format's own layout (possibly empty) |

A header is valid if all of it is there and every field is as the table
says. The format (owner key, name) is the identity of the program that
defines it (§machines.shell): a shell opens a document by starting that
program with the document as its argument (§machines.calls). A blob that
starts with `TERS` is an image (§image); a blob with neither is an
article (`ARTICLE.md`). A document is published as an entry of kind
4 (publish, §content.feeds; `NETWORK.md` 4.2).

## §content.charges Charges and limits

The device is charged as §charges says:

| What | Charge |
|---|---|
| `submit`, including one that fails: its base charge (an answered call) | 64 |
| Bytes copied between memory and the device at submit: an accepted put or save (its `size` bytes), publish (its title), find (its query), set profile or set nickname (its name) | 1 per 4 bytes, rounded up, per call |
| Content delivered at completion: the bytes a get, load, records (subscriptions, timeline, names, programs, records) or find request writes to `addr` | 1 per 4 bytes: the bytes of all the frame's completions added up, then divided by 4 and rounded up once; subtracted from that frame's budget in step 2 of §frames |

Nothing else is charged: a rejected put copies nothing, and the fields a
completion writes to a request block are free.

Provisional values (§provisional):

- blobs of 1 to 1,048,576 bytes; 8 requests pending; 16,384 bytes
  delivered by one get, load, records or find request;
- display names, nicknames and queries of 48 bytes, titles of 120, and 16
  results to a find or find more (§content.feeds);
- named records (§content.records): names of 32 bytes, records of 65,536 bytes,
  and per owner 256 records and 1,048,576 bytes in all.

## §content.unspecified Not yet specified

- Named pointers (a signed, versioned pointer from a key and a name to a
  blob; not the display names and nicknames of §content.feeds), ranges
  verified without fetching the whole blob, and cancelling a content
  request.
- For the requests that need consent (§content.feeds): telling the program
  that one waits for the user, remembering a user's answer for later
  requests, and publishing replies or kinds of entry other than articles,
  documents and profiles (programs are published by the host, not by
  programs).
- Named records (§content.records): moving records from one owner to another
  (for example when a program run from a file is later published), keeping
  them in step between a user's machines, and sharing a record with
  another program. Starting a version of a program other than the current
  one.
