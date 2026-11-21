# Terse device bus — v1 (draft)

Status: draft, written with the core host in `crates/terse-bus` (the
protocol, the core and its compositor) and the drivers in
`crates/terse-devices`. Nothing in `SPEC.md` depends on this document: it
describes one way of building a machine host, not the machine. A host that
compiles its devices in, as `terse run` and `terse-host` do, is just as
conforming. In this repository both are one machine: terse-core's
`Machine` runs the frame loop and the System and Machines devices over a
`Devices` trait, and terse-core's `System` runs a shell and the machines it
starts; the devices compiled in (`terse run`, `terse-host`) and the bus
(`terse-bus`) are two implementations of `Devices`.

A **device driver** is a separate program that implements one or more
devices of `SPEC.md` (slots) and speaks the protocol below to a **core
host**. The core runs the machines: the CPU, ROM and RAM, the budget and the
frame loop, the System device (slot 1), the Machines device (slot 8) of a
shell, the event queue, and the composition of the picture. It knows
nothing of what any other device means (but where its section goes in the
state serialization, section 1.7): every call to a slot other than 1
and 8 goes to the slot's driver, checked first against a **call table** the
driver gives at the handshake. So a device is written once, in any language,
and used by any core that speaks this protocol; a driver can be replaced by
another for some hardware, or left out (an image that lists a device no
driver provides is refused, SPEC §image).

One driver process serves **every machine** of a run (section 1.2): the
shell and each machine it starts are **instances**, named in every
message. So a shell with eight machines and the five reference drivers is
six processes, not forty-five.

Design rules:

1. **One message format**, independent of the transport. Two transports
   carry it: a Unix-domain socket (section 3; `AF_UNIX` on Linux, macOS
   and Windows alike, section 4.3) and shared memory (section 4). A core
   supports both at once; each driver chooses one at the handshake.
2. **The machine's semantics are kept exactly.** A run with drivers gives
   the same state hash after every frame as the same run with the devices
   built in (`STATE.md`), for every machine of a system. Charges, results
   and the order of events are the drivers' to compute; the core applies
   them.
3. **Calls are synchronous; inputs arrive at frame boundaries.** A call's
   reply carries its result, its charge and the bytes it writes, and
   nothing else. A driver delivers events, content completions and
   anything else SPEC lists as an input when the core begins a machine's
   frame.
4. **The core can check what it cannot interpret.** The call table says
   where each call's buffer is and how long it is, so the core applies
   SPEC §calls.convention's rules for buffers itself, and for the calls SPEC §charges calls
   deferred, whose result and charge need no device state, it computes
   the result and the charge itself and sends the call later, in a batch
   (section 6).
5. **What one end needs of another's state is named.** A driver that
   needs a fact of another device's (the page needs the vector display's
   fill mode), or the core that needs a device's (device 8 needs device
   9's list), gets it as a **property** the other publishes, carried by
   the core (section 7.2), never by reading the other's state.

All integers are little-endian. `h` suffix means hexadecimal. "Core" and
"driver" name the two ends.

---

## 1. Messages

Every message is a 4-byte length `n` (u32: the length of what follows, at
most 16,777,216), then an 8-byte **address**, then the body:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | `n` |
| 4 | 4 | Instance: the machine the message is about (section 1.2) |
| 8 | 1 | Slot: one of the driver's slots, or 0 for the instance as a whole |
| 9 | 3 | Reserved |
| 12 | `n` − 8 | The body; its first byte is the message's type |

HELLO, BATCH and SHUTDOWN are the process's: instance 0, slot 0. A reply
has the address of its request. Field offsets below are offsets in the
body.

| Type | Name | From | Address | Reply |
|---|---|---|---|---|
| `01h` | HELLO | core | process | `81h` HELLO |
| `02h` | RESET | core | instance | `82h` RESET |
| `03h` | OPEN | core | instance | `83h` OPEN |
| `04h` | CLOSE | core | instance | none |
| `10h` | CALL | core | instance, slot | `90h` RESULT |
| `11h` | BATCH | core | process | none |
| `20h` | READ | driver, while it owes a reply | instance | `A0h` DATA |
| `30h` | BEGIN | core | instance | `B0h` INPUTS |
| `31h` | END | core | instance | none |
| `40h` | STATE | core | instance, slot | `C0h` STATE |
| `41h` | RESTORE | core | instance, slot | `C1h` RESTORE |
| `42h` | GET | core | instance | `C2h` GET |
| `43h` | SET | core | instance | none |
| `44h` | SERVICE | core | instance | `C4h` SERVICE |
| `50h` | PRESENT | core | instance | `D0h` PRESENT |
| `60h` | SHUTDOWN | core | process | none |
| `7Fh` | ERROR | either | any | none |

- The core sends one request at a time to a driver and waits for its reply
  before it sends that driver another. CLOSE, BATCH, END, SET and SHUTDOWN
  have no reply: on the socket they are written at once; on shared memory
  they wait and go in front of the next request, in the same packet
  (section 4.2), since nothing tells the core when the driver has read
  them. Between a request and its reply the driver may send READ messages
  and must then wait for each one's DATA. Nothing else is ever in flight.
- A driver that receives a message it cannot handle replies ERROR (UTF-8
  text after the type byte); the core gives the driver up (section 10). A
  core may also send ERROR before it closes the connection.
- Reserved bytes are written as zero and ignored when read.
- Strings (paths, names) are UTF-8, given as a u16 length then the bytes;
  a blob is a u32 length then the bytes.

### 1.1 HELLO and the handshake

The core starts the driver (section 2) and sends HELLO:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `01h` |
| 1 | 1 | Protocol version: `1` |
| 2 | 2 | Reserved |
| 4 | 4 | Flags: bit 0 shared memory is offered; bit 1 the core presents (visual drivers will be asked for layers, section 7); bit 2 replay (the run's inputs come from a recording, section 8: the driver must not use its live source, for example the network) |
| 8 | 1 | `s`: the number of slots the core asks this driver for |
| 9 | `s` | The slots (which ones: below) |
| … | 2 + … | The session directory: where a driver creates its layer files (section 7). Always a directory that exists, which the core created for the run and removes after it; given also when the core neither presents nor offers shared memory (a driver that never draws need not use it) |
| … | 2 + … | The driver's bus file (section 4.2); empty if shared memory is not offered |

The driver answers HELLO (`81h`):

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `81h` |
| 1 | 1 | Protocol version: `1` |
| 2 | 1 | Transport chosen: `0` the socket, `1` shared memory |
| 3 | 1 | Reserved |
| 4 | 2 + … | The driver's name (for messages and measurements) |
| … | 2 | Native width in pixels (0: no preference) |
| … | 2 | Native height in pixels |
| … | 1 | `s`: slots, each: slot (u8), the device version it implements (u8), `k` layers (u8) and `k` × (layer ID, z) (u8 each, section 7), `m` call table entries (u8) and `m` × 64 bytes (section 5) |
| … | 1 + … | Properties it publishes: a count (u8), then names (section 7.2) |
| … | 1 + … | Properties it subscribes to: a count, then names |
| … | 1 + … | Host services it gives: a count, then names (section 9) |
| … | 2 + … | Optional: the driver's version (printable ASCII without spaces, at most 32 bytes) |
| … | 2 + … | Optional, with the version: its build identifier (the same characters): a tag naming a hash, a colon and the first 8 bytes of that hash of the driver's program file in hex (`b3:` BLAKE3, `fnv:` FNV-1a 64, `b2:` BLAKE2b, `s2:` SHA-256), or `unknown` |

- **Which slots the core asks for.** The slots of the driver (section 2)
  that a machine of the run may have: for a single machine, those its
  image lists (required, or optional and not withheld), never 1, 8 or a
  slot that is no device (SPEC §image); for a shell, all of them. Slots
  that share state are asked for together (next item).
- A driver answers for **exactly** the slots it was asked for, or for all
  of them but slot 10 (below). The core refuses a driver that answers for
  any other set (a slot it did not ask for, or one asked for left out),
  or that chooses shared memory when it was not offered: the run does not
  start (`STATE.md` 6.4, status 2).
- **A driver may serve several slots.** It does when the devices share
  state: device 9's `style` and `persistence` are device 3's (SPEC §vector3d.calls),
  one section of the state (`STATE.md` 1.7), so `terse-dev-vector` serves
  slots 3 and 9, and a core starts one driver for both. The calls of both
  slots go to the one process, in order (a batch may hold both), so the
  shared setting is never in two places. Each slot keeps its own call
  table, layers and state section.
- **Slots that share state are asked for together**, whichever of them a
  machine lists: a core that asks the vector driver for 3 or 9 asks it
  for both, in HELLO, whether or not an image lists the other, since the
  driver needs slot 3's state to carry out device 9's calls. (`terse-dev-vector`
  refuses OPEN of a machine that has device 9 when it was not asked for
  slot 3.) OPEN then lists, per machine, only the slots the machine has:
  for `programs/holo`, which lists only 9, HELLO asks for 3 and 9 and
  OPEN lists 9. STATE for that machine asks for slot 3's section
  (section 1.7). In general, the drivers' documentation (section 2's
  table) names each group; 3 and 9 are the only one in v0.
- **Full text (slot 10) goes with the terminal.** Device 10 (SPEC
  §terminal.text) is a capability: no calls, no layers, no state. When a
  machine's image lists it, the core asks the terminal's driver (slot 2)
  for slot 10 as well. A driver that can draw every character answers for
  it (version 0, no layers, an empty call table) and is told in OPEN
  whether each machine has it; one that answers only for slot 2 does not
  give it, which the core accepts: an optional device 10 is then not
  granted, and an image that requires it is refused. `terse-dev-terminal`
  answers for it; so does the C terminal driver, which draws nothing and
  so lacks no character.
- **The driver's identity** (`PERFORMANCE.md` R13) is the name with the
  version and the build identifier. A reply may end after the services
  (an older driver: version and build empty), and a core ignores anything
  after the build (room for later fields), so neither end needs a new
  protocol version. It is self-reported, for accountability among honest
  drivers (the record, `RECORD.md`, names the driver that made a frame
  late), not a security measure; a core never passes it to a machine.
  Every driver prints the same three words with `--version`, without
  connecting.
- After the reply, every message travels on the chosen transport. A driver
  that chose shared memory keeps the socket open and sends nothing on it:
  it is how each end learns that the other has gone.
- The native size is a picture size at which the driver's drawing is
  sharpest (the terminal's: its font's cell times 80 by the 4:3 shape).
  The core takes the first one given as the native size of a machine's
  screen (section 7).

### 1.2 Instances: OPEN, CLOSE and RESET

A driver serves any number of machines at once. Each is an **instance**,
named by a u32 the core chooses; the reference core uses the machine's
serial among the machines of its system (SPEC §machines: the shell, or a single
machine, is 0; each machine the shell starts gets the next number, never
used again). Before the first message about an instance the core sends
OPEN:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `03h` |
| 1 | 3 | Reserved |
| 4 | 32 | Image ID |
| 36 | 4 | `R`: ROM length |
| 40 | 4 | `L`: RAM length |
| 44 | 2 + … | The instance's memory file (section 4.1); empty unless the driver chose shared memory |
| … | 1 + … | The driver's slots the machine has: a count (u8), then (slot, version) pairs. These are the devices the image requires and the optional ones granted (SPEC §image); a device the machine was not granted is not listed, so the driver treats it as absent |
| … | 1 + … | The program's identity (SPEC §machines.calls), if it was started as one: `0`, or `1`, the publisher (32 bytes) and the name (u16 length and bytes) |
| … | `R` | The ROM, so that a driver can read ROM without asking |

The reply is `83h`, then `01h` if the driver took the instance or `00h` if
not, 2 reserved bytes and a reason (a string). The core refuses to start a
machine (SPEC §machines.frames: `start` fails with "needs a device") that a driver will
not take, or whose image requires a device no driver serves, or a version a
driver does not implement. An optional device no driver serves (in the
version listed) is not granted, and the machine starts without it; a
restored machine (SPEC §machines.snapshots) gets exactly the devices its snapshot's
grants say, and fails with "needs a device" if one of them has no
driver.

An instance's devices start at reset (SPEC §reset). Each instance has its
own memory file on shared memory, and its own state in the driver; a
driver keeps nothing of one instance in another's. What a driver may share
between instances is what makes no difference to the machines: fonts,
glyph caches, the content device's network node (whose named records are
each program's own, SPEC §content.records, by the identity given in OPEN, else the
image ID).

CLOSE (`04h`) ends an instance (the shell stopped the machine, or the run
ends): the driver drops its state and unmaps its memory file. RESET
(`02h`, reply `82h`) returns an instance's devices to their reset state and
drops their host bookkeeping (pending layouts, requests in flight). The
reference core never sends RESET (a machine is reset by a new OPEN of a
new instance); a driver should still answer it.

### 1.3 Ordering

A system's frame (SPEC §machines.frames) runs the shell's frame, then each running
machine's in increasing handle order. For each machine's frame the core
sends BEGIN, the calls, then END, to every driver of that machine; drivers
see instance 0's BEGIN first in every system frame (the content driver
announces feeds updated since the last system frame then, to every
machine, as SPEC §events says). Each driver sees the messages of all machines in
one order, which is the system's.

### 1.4 CALL and RESULT

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `10h` |
| 1 | 1 | Function (the low byte of `a7`) |
| 2 | 2 | Reserved |
| 4 | 24 | `a0`–`a5` (u32 each) |
| 28 | 8 | Budget (i64): the budget after the `ecall` instruction's own cost (1, or 4 from RAM) less 4, for every call: an answered call's further 60 is not taken off (the core charges it with the result). No reference driver reads it |
| 36 | 4 | `n`: the bytes of the call's read buffer that follow |
| 40 | `n` | Those bytes, from the buffer's start |

The address's slot says which device (`a7` / 256). On the socket transport
the core sends the buffer the device reads (the whole buffer). On shared
memory it sends none (`n` = 0) and the driver reads guest memory itself,
except in a batch (section 6).

The driver replies RESULT:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `90h` |
| 1 | 1 | Flags: bit 0 `a1` is set |
| 2 | 2 | Reserved |
| 4 | 4 | `a0` |
| 8 | 4 | `a1` (ignored unless flag bit 0) |
| 12 | 4 | Charge: units on top of the call's base charge (u32) |
| 16 | 4 | `w`: number of writes |
| 20 | … | `w` writes: address (u32), length `n` (u32), `n` bytes |

- The core charges the call's base charge (4, or 64 for an answered call:
  section 5) + the charge, sets `a0` (and `a1`), and copies the writes
  into RAM in order. A write must lie wholly within the call's
  write buffer (section 5); otherwise the core gives the driver up
  (section 10: a driver bug, not a fault of the program).
- On shared memory a driver may instead write its buffer in place, in the
  instance's memory file, before it replies.
- A driver that needs other guest memory during the call (the content
  device's data ranges, SPEC §content.request) sends READ (`20h`, 3 reserved bytes,
  address u32, length u32). The core answers DATA: `A0h`, then `01h` and
  the bytes if the range lies wholly in ROM or wholly in RAM, else `00h`
  and nothing; then 2 reserved bytes before the bytes. (Shared-memory
  drivers can read the memory file instead.)

### 1.5 BEGIN and INPUTS

At step 1 of each of a machine's frames (SPEC §frames) the core sends BEGIN to
every driver of the machine:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `30h` |
| 1 | 1 | Flags: bit 0 a recorded input follows (replay) |
| 2 | 2 | Reserved |
| 4 | 4 | The frame counter of the frame that begins |
| 8 | 4 | The Unix time latched for it |
| 12 | 4 | `e`: the host's input events for this frame, as the system routed them to this machine (keys, text, pointer, wheel, …) |
| 16 | 4 × `e` | The events: type, `a`, `b`, `c` |
| … | 4 + … | Replay: the driver's recorded input for this frame: a u32 length, then that many bytes. **Always present**: the length is 0 unless flag bit 0 is set |

Every driver of the machine gets the same BEGIN, with all of the frame's
host events, whether or not its device uses them.

and the driver replies INPUTS:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `B0h` |
| 1 | 3 | Reserved |
| 4 | 4 | Charge: units subtracted from this frame's budget (SPEC §charges, content delivered) |
| 8 | 4 | `e`: events |
| 12 | 8 × `e` | Each: type, `a`, `b`, `c`, position (u8), flags (u8), index (u16) |
| … | 4 + … | Writes: count, then address, length, bytes, as in RESULT; anywhere in RAM |
| … | 4 + … | This frame's input record (u32 length, bytes; section 8) |

**Positions.** The core builds the frame's input from the host's events
and the drivers' events: first every event at position 0 (before the
host's events); then the host's events, each followed by the events at
position 1 whose index is its index; then those at position 2 (after the
input events). An event at position 1 whose index is not that of one of
the frame's host events (the index is `e` or more) is dropped. Among
events of the same place, drivers in increasing order of their lowest
slot, each driver's in the order it gave them. These are queued as SPEC
§events says (at most 64; at most one feeds-updated event a frame,
whoever sent it; a pair, below, both or neither). Then
the core latches the time, random bytes and controllers, copies every
driver's writes into RAM, and queues the events at position 3
(completions: SPEC §content.submit's content-done events, queued after the frame's
input), counting those dropped. Then it runs the frame with 125,000 units
less every driver's charge.

**Flags.** Bit 0: this event and the next one of the same driver are a
pair, queued together or not at all (SPEC §page.events's page located events).

The core may send BEGIN to all of a machine's drivers at once and collect
the replies in any order. A driver may READ before it replies.

### 1.6 END

`31h`, stopped flag (u8: the machine has exited or faulted), 2 reserved,
the frame counter after the frame (u32). No reply. The machine's frame has
ended (SPEC §frames step 3); what the device does between frames (lay out a
page, send content requests to the network) it does now, while the core
goes on. Any calls of the frame still in a batch arrive just before END.

### 1.7 STATE and RESTORE

STATE is `40h` alone, for an instance and slot. The reply is `C0h`, 3
reserved bytes, a length (u32) and the slot's section of the state
serialization (`STATE.md` 1.6–1.12: the bytes after the present flag). The
core writes the present flags and puts the sections in `STATE.md`'s order
(slot 3's section for an image that lists only device 9 too: the style
lives there, SPEC §vector3d.calls). Defined only between frames.

**This is where the core knows the devices.** The core knows nothing of
what a device means, but to write the serialization, and so to hash, it
must know where each device's section goes and when it is there: that is
the one device-specific table every core holds. After the System section
(`STATE.md` 1.5), in this order:

| Order | `STATE.md` | Section of slot (STATE) | Present flag | What the core writes |
|---|---|---|---|---|
| 1 | 1.6 | 2 | yes | `01h` and slot 2's section if the machine has device 2, else `00h` |
| 2 | 1.7 | 3 | yes | `01h` and slot 3's section if the machine has device 3 **or** device 9 (asked of the vector driver as slot 3 in both cases), else `00h` |
| 3 | 1.8 | 4 | yes | `01h` and slot 4's section if it has device 4, else `00h` |
| 4 | 1.9 | 7 | yes | `01h` and slot 7's section if it has device 7, else `00h` |
| 5 | 1.10 | none: the core's own (device 8) | yes, if there at all | nothing if the image does not list 8; `00h` if it lists 8 and the machine is not a shell (run alone, or started by a shell: SPEC §image); `01h` and the core's own device-8 section for the shell |
| 6 | 1.11 | 9 | no | slot 9's section if the machine has device 9, else nothing |
| 7 | 1.12 | 6 | no | slot 6's section if it has device 6, else nothing |
| 8 | 1.13 | none: the core's | no | the machine's argument, if it has one |

"Has" is SPEC §image's: the image requires the device, or lists it as
optional and it was granted. Device 10 has no section (its grant flag is
in `STATE.md` 1.4); slots that are no device have none. A new device would
need a new row here, in every core, before any core could hash a machine
that has it; section 14 asks whether drivers should declare it instead.

RESTORE is `41h`, 3 reserved bytes, a length (u32) and the rest of a
serialization from the slot's section on (STATE.md 4): the core cannot
tell where a section ends, so the driver reads its section from the start
of these bytes and says how long it was. It replies `C1h`, then `01h` if it
took it or `00h` if it was not a valid section (then nothing changed), 2
reserved bytes, and the section's length (u32). The core goes on with the
next section after it, and in the end checks that the machine writes the
serialization it was given, byte for byte. Host bookkeeping restarts as
`STATE.md` 4 says (content requests that were pending fail in the next
frame; a document is laid out as new). Snapshot files (STATE.md 4) are the
core's: it reads and writes them as `terse run` does, so a snapshot taken
by one host restores in the other.

### 1.8 GET and SET

Properties (section 7.2). GET is `42h`, 1 reserved byte, a count (u8) and
that many names: the reply `C2h` has a count (u8) and, for each property
asked for that the driver publishes, its name and value (a blob; empty:
none). SET is `43h`, 1 reserved byte, a name and a value (blob): a
property the driver subscribes to has changed. No reply.

### 1.9 SERVICE

A host service (section 9): `44h`, 1 reserved byte, the service's name, a
request (blob). The reply `C4h` has `01h` and an empty string then the
answer (blob), or `00h`, why not (a string) and an empty blob.

The instance is the machine on whose behalf the core asks, which need not
have been given to the driver (OPEN): a shell that lists none of the
driver's devices still starts machines (`image.fetch`). A driver answers
a service that needs nothing of the machine whatever the instance, and
refuses (`00h`) one that needs what OPEN gives (a snapshot's owner is the
machine's identity).

### 1.10 PRESENT

Section 7.

### 1.11 SHUTDOWN

`60h` alone: the driver releases what it holds (its layer files) and
exits. A driver also exits when its socket is closed.

---

## 2. Starting a driver

The core starts the driver's command line with these environment
variables:

| Variable | Value |
|---|---|
| `TERSE_BUS_SOCKET` | Path of a Unix-domain (`AF_UNIX`) stream socket the core listens on, on every system (section 4.3) |
| `TERSE_BUS_CPU` | Optional: a CPU number the driver should run on (a hint: a driver on a system that cannot pin a thread, macOS, ignores it) |

Standard input is not used (the reference cores give `/dev/null`, `NUL`
on Windows); a driver writes diagnostics to standard error.

The driver connects to the socket and waits for HELLO. The core accepts
one connection per driver, waits at most 10 seconds (`--start-timeout`)
for it and for the HELLO reply, and refuses the run if either does not
come or the driver exits first; a slow driver is waited for that long, and
the core gives up on it without leaving it running. The core reaps every
driver it started: it sends SHUTDOWN, closes the socket, and kills a
driver that has not exited 2 seconds later.

### 2.1 Which driver serves which slot

A core chooses its drivers. The reference drivers are one program per
group of slots, with fixed names (and `.exe` after them on Windows):

| Slots | Program | Options |
|---|---|---|
| 2, and 10 (full text) | `terse-dev-terminal` | `--config <dir>`: the fonts `<dir>/fonts` names (README "Fonts"), loaded at start; without it the built-in font |
| 3 and 9 (shared state, section 1.1) | `terse-dev-vector` | none |
| 4 | `terse-dev-content` | `--store <dir>`, `--config <dir>`, `--data <dir>`, `--relay <ws://…>` (repeatable; `none` for no relay), `--consent allow\|deny\|ask`, `--profile reading`, `--no-feed-relays`, `--no-program-download`: those of `terse run`, with the same defaults, which are **the user's own** blob store, configuration and data directories. A test must always give `--store`, `--config` and `--data` (and `--relay none` to stay off the network) |
| 6 | `terse-dev-sound` | none |
| 7 | `terse-dev-page` | `--fonts` or `--reference` (lay pages out with real fonts, or with the reference layout; by default real fonts only when the core presents), `--config <dir>` (the fonts `<dir>/fonts` names, loaded at start; without it the built-in fonts) |

Options follow the program name on its command line. The content and
page drivers refuse an option they do not know (status 2); the others
ignore the arguments they do not know. `terse-bus` gives the terminal
and page drivers the `--config` it gives the content driver. Every Rust driver also reads
`TERSE_BUS_TRANSPORT=socket` (never take shared memory),
`TERSE_BUS_SPIN=fixed`, `TERSE_BUS_SPIN_US` and `TERSE_BUS_WAKE=socket`
(section 4.2; `terse-bus` reads the last too). The other
drivers in the repository take no options:
`crates/terse-devices/c/terminal.c` (slots 2 and 10; the socket unless
`TERSE_C_SHM=1`), `crates/terse-devices/python/vector.py` (slots 3 and 9,
socket only), `native/driver-example/terminal.c` (slots 2 and 10, socket
only, section 13) and `terse-dev-crash`, a terminal that fails on purpose
for tests (`TERSE_CRASH`, `TERSE_CRASH_AT`, `TERSE_CRASH_THEN`).

How a core finds its drivers (`terse-bus` does this, and `tcore`):

1. It looks for the programs above in **one directory**: by default the
   directory of its own executable (`tcore` also takes `--drivers <dir>`
   or `TERSE_DRIVERS`). It does not search `PATH` for them.
2. `--driver <slots>=<command line>` replaces the driver of the slots it
   names (`--driver 2=...`, `--driver 3,9=...`). The command line is
   split at spaces; its program is a path, or a name looked up on `PATH`
   as a shell would. A driver that replaces slot 2 is asked for slot 10
   too when a machine lists 10. Name a whole group (`3,9`).
3. It starts a driver only if a machine of the run may have one of its
   slots (section 1.1; for a shell, all of them), and asks it only for
   those.
4. **An optional device without a driver is absent.** For a single
   machine, a default driver whose slots only optional devices of the
   image use is not started if its program is not in the directory, or
   if `--without <slot>` withholds them: the machine runs without those
   devices, which are not granted (SPEC §image).
5. **A required device without a driver refuses the run**, with status 2
   ("needs a device", `STATE.md` 6.4). So does a driver named by
   `--driver` that cannot be started, and any driver whose handshake
   fails (it exits or closes its socket first, does not answer in time,
   answers for the wrong slots, or will not take the machine in OPEN,
   section 1.2), whether its devices are required or optional: a
   `--driver` is a deliberate choice, so the core does not quietly run
   without it.
6. Slots that are no device (0, 5, 11–255) are never asked for, also not
   of a `--driver`: an image that lists one as optional runs without it,
   and one that requires it is refused (SPEC §image).

The set of drivers, and so the devices a machine may be granted, is
fixed when the run starts. Between `--driver` and the core's own
directory comes the user's drivers folder (section 2.2).

### 2.2 Installed drivers: the user's drivers folder

A user **installs** a driver once, and every later run uses it, without
a `--driver` option. The folder is in the user's configuration
directory, beside the fonts (README "Fonts"):

    <config>/drivers/            $XDG_CONFIG_HOME/terse/drivers (else ~/.config/terse/drivers)
      use                        which installed driver serves which slots
      <name>/manifest            one installed driver: what it said it is
      <name>/<program>           a copy of its program file

`terse drivers` writes it (README "Drivers"); a core only reads it, once,
at start. `<config>` is the directory a core's `--config` names, the one
it gives the content driver (section 2.1's table).

**Groups.** A driver is installed for one of the groups of section 2.1's
table, and serves the whole group: slots 2 and 10 (10 if it answers for
it), 3 and 9, 4, 6, or 7. A group is named by its lowest slot.

**`use`** is UTF-8 text, one line per group: the group's lowest slot, a
space and the name of an installed driver, or `default` (the driver
shipped with the core); blank lines and lines starting with `#` are
ignored. A group with no line gets the shipped driver. `terse drivers
add` writes the line for the driver it installs, so the driver installed
last for a group is the one used; `terse drivers use` changes it.

**A name** is 1 to 64 of `a`–`z`, `0`–`9`, `-` and `.`, starting with a
letter or digit, and not `use`.

**The manifest** is UTF-8 text, one field per line, a word, a space and
the value, in this order:

| Line | Value |
|---|---|
| `terse-driver 1` | the format's version |
| `program <file>` | the program's file name in the driver's directory (no `/`) |
| `slot <s> <v>` | a slot the driver answered for when it was installed and its device version; one line per slot, in increasing order |
| `name <text>` | the name it gave in HELLO (section 1.1) |
| `version <text>` | its version, or `-` |
| `build <text>` | its build identifier (`PERFORMANCE.md` R13), or `-` |

A reader ignores lines of words it does not know. The name, version and
build are what `terse drivers list` shows; a run takes the driver's
identity from its HELLO reply as for any driver, so the record
(`RECORD.md` 2, the `device` line) and the rule of `PERFORMANCE.md` R15
name the build that actually ran.

**Installing** (`terse drivers add <file>`) is the user's own act at a
command line, on a file the user names. It starts the program as a core
would (section 2, no options, the socket only) once for each group, sends
HELLO for the group's slots and SHUTDOWN after the reply, and keeps the
group it answered for (`--slot` asks for one group only; a driver that
answers for two groups must be given `--slot`). It refuses a driver that
answers for none, or with a device version this host does not implement
(SPEC §image's table). It shows the driver's name, version, build
identifier and slots and says that a driver is a native program that
runs with the user's full rights. It copies the program file into the
driver's directory (a driver must be one self-contained program file;
installing a name again replaces it), writes the manifest, and sets
`use` for the group.

**A host never installs or chooses a driver by itself.** Nothing a
machine does, nothing in a feed, an article or a program, and nothing
fetched from the network, can add a driver or change `use`: the folder
is written only by `terse drivers`, run by the user. A driver is a
native program with the user's full rights (it can read and change the
user's files and use the network); the core trusts it as it trusts its
own code (section 14, "Trust"), so installing one is the same decision
as installing any program.

**Which driver serves a group**, in this order:

1. a `--driver` on the command line that names any slot of the group
   (section 2.1, rule 2);
2. else the installed driver `use` names for the group;
3. else the driver shipped with the core (section 2.1, rule 1).

An installed driver is started with the options the core gives the
shipped driver of its group (section 2.1's table: `--config` for the
terminal and the page, the content driver's options for content), so
that it finds the same fonts and directories; a driver ignores the
options it does not need (`native/driver-example` ignores them all).

A `use` line of `default`, a group with no line, a name not installed,
or a line that cannot be read gives the shipped driver (rule 3); a core
says so on standard error for the last two. `tcore` (section 12) does
not read the folder: a minimal core need not.

**When an installed driver fails.** The run falls back to the shipped
driver of the group, says so, and goes on, when the installed driver:

- is missing (its directory, manifest or program file), or its manifest
  cannot be read;
- cannot be started, exits or closes its socket before its HELLO reply,
  or does not reply in time (section 2);
- fails its handshake: replies ERROR, answers for a slot outside its
  group or leaves one out (section 1.1), or answers for a slot with a
  device version other than its manifest's (a program changed after it
  was installed).

The message names the installed driver, why it was not used, and the
command that goes back to the shipped one (`terse drivers use <slot>
default`); `terse-bus` writes it to standard error and, in a window,
shows it as a notice (like the slow-device notice of `PERFORMANCE.md`
R15) for 5 seconds. If the core has no shipped driver for the group
either, the run is refused (status 2), as for a `--driver` that fails.
After the handshake an installed driver is like any other: one that
will not take a machine in OPEN refuses the run (section 2.1, rule 5),
one that fails during the run stops it (section 10, status 3), and one
that is slow is warned about and its machines saved and stopped
(`PERFORMANCE.md` R15). None of these falls back, and nothing is
disabled for later runs: the next run tries the installed driver again.

*Why fall back at start, unlike a `--driver`.* A `--driver` is chosen
for one run, by someone watching it; refusing tells them at once. An
installed driver is used by every run from then on, including the
reader, and a user who installed one that is broken, or that a system
update left behind, should still be able to read: R15's principle is that
a component which does not do its job costs the user a notice, not
their work. Before the first frame no machine has seen the device, the
grants are not yet made (SPEC §image), and every driver of a device
version gives the same results (section 13: a device is complete), so
starting the shipped driver instead is just another valid choice, with
the same state hashes. After the handshake the argument of section 10
holds: the device's state is in the driver, so nothing can take over
from it, and the run stops instead.

**Not hot-plugged.** The folder is read only at start: the set of
drivers, like a machine's grants (SPEC §image), is fixed at reset. A
driver installed or chosen while a core runs is used from its next run;
a device that appeared mid-run would contradict `device`'s answer, which
never changes while a machine runs.

---

## 3. Socket transport

Messages are written to the stream one after another, each as section 1
says. That is all: any language that can open a Unix-domain socket can be
a driver (on Windows, an `AF_UNIX` socket through Winsock: section 4.3).

## 4. Shared-memory transport

For a call that moves no buffer, the socket's cost is two system calls on
each side and two context switches. The shared-memory transport removes
the copies and, when both ends are running on their own CPUs, the system
calls too.

### 4.1 Memory files

One file per instance (machine), shared by every driver of it that chose
shared memory: the ROM at offset 0 (its `R` bytes; the rest of the first
131,072 bytes is zero) and the RAM at offset 131,072 (`L` bytes). The
core's CPU runs on this mapping, so it is the guest's memory itself, not a
copy. A driver maps it shared, for reading and writing, at OPEN, and
unmaps it at CLOSE. It reads a call's buffers here, writes its write
buffer here (or returns writes), and must not touch it while the core is
not waiting for one of its replies about that instance.

### 4.2 The bus file

One file per driver. Its first 4,096 bytes are the control block, then the
request area and the reply area:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic `54h 42h 55h 53h` (`TBUS`) |
| 4 | 4 | Version: `0` |
| 8 | 4 | Request area offset |
| 12 | 4 | Request area capacity |
| 16 | 4 | Reply area offset |
| 20 | 4 | Reply area capacity |
| 64 | 4 | `req_seq`: incremented by the core after it writes a packet |
| 68 | 4 | `req_len`: the packet's length |
| 72 | 4 | `driver_sleeping`: how the driver is waiting: `0` awake (spinning or working), `1` asleep on `req_seq`'s address, `2` asleep on the socket (below) |
| 128 | 4 | `rep_seq`: incremented by the driver after it writes a packet |
| 132 | 4 | `rep_len` |
| 136 | 4 | `core_sleeping`: the same for the core, on `rep_seq` |

The core creates the file in the session directory with areas of 4 MiB
each (the file is sparse; pages that are never written cost nothing).
A request area holds one packet at a time: the core writes the next only
after the reply to the last, which is why messages with no reply wait for
the next request (section 1).

A **packet** is one or more messages, framed as in section 1, back to back.
The core writes a request packet: in practice what it holds (CLOSE, END,
SET), a BATCH (if any) then one request. The driver handles them in order
and writes its reply (or a READ) as a packet of one message.

All control words are aligned 32-bit words, written and read only as
single atomic operations. Each side starts from the sequence numbers it
finds when it maps the file; a driver maps it (and reads `req_seq`)
before it sends its HELLO reply, since the core may send its first
request as soon as it has the reply.

**Hand-over.** The writer writes the packet, then its length, then
increments the sequence number (sequentially consistent: at least release
ordering, and nothing after it, the next read included, moves before it),
then reads the other side's sleeping word and wakes it as the word says:

- `0`: nothing;
- `1`: the system's address wake on the sequence word it just
  incremented (section 4.3: a futex on Linux, a shared ulock on macOS;
  never written on Windows);
- `2`: one byte, of any value, written to the socket;
- any other value: as `1` (room for later ways of sleeping).

The reader spins on the sequence number (acquire) for a while; if it has
not changed, it stores how it will sleep (`1` or `2`) in its sleeping word,
checks the sequence number again, and sleeps until the number changes or
at most 100 ms pass (the reference: 50 ms): `1` in the system's address
wait on the sequence word, given the value it last saw; `2` waiting until
the socket can be read, then reading and dropping every byte that has
arrived. Then it stores `0`, and if the number has still not changed it
checks that the other end is alive (the socket has not reached its end:
sleeping on the socket, the end wakes it at once) before it sleeps again.

Each end chooses how it sleeps, every time, and the other wakes it the way
its word says, so a driver that sleeps on the socket works with a core that
sleeps on the address, and the other way round. An end that offers or
takes shared memory must therefore be able to do both wakes of its system,
even if it only ever sleeps one way. The socket carries nothing else after
the handshake, so the bytes are never mistaken for a message. A wake is
only ever an optimisation: the reader looks again at least every 100 ms,
so a wake that is lost (or that an older end does not give for `2`) costs
time, never correctness.

**How long to spin.** A fixed spin costs a CPU whenever the other end is
idle: a driver that spins 2 ms after each reply spends 2 ms of every 16.7
ms frame spinning (12 % of a core per driver), which an idle shell with
four drivers pays for nothing. So the reference drivers spin
**adaptively**: up to twice the longest of their last 32 waits that ended
while spinning (at least 4 µs, at most 2 ms, `TERSE_BUS_SPIN_US`), and
only 4 µs if all of them slept. The calls of a busy frame follow each other
within microseconds, so the driver keeps spinning through them; the wait
between frames is long, so the driver sleeps almost at once.
`TERSE_BUS_SPIN=fixed` restores the fixed spin. The core spins up to
100 µs for a reply (`--spin-us`; `--spin adaptive` makes it adaptive too).

### 4.3 What is particular to each system

Everything above is the same on every system: the messages, the
handshake, the bus file's layout, the sequence and sleeping words and the
memory files. What differs is how a core and a driver reach each other,
how a file is shared and how an end sleeps until the other has written. A
core and a driver interoperate on a system when each does what this
section says for it. (`terse-bus` and the Rust drivers implement all
three systems in `crates/terse-bus/src/sys`; they are run on Linux and,
for now, only checked to compile for macOS and Windows.)

**Finding each other, on every system.** The core makes a session
directory that only the user can open, and in it, for each driver, binds a
Unix-domain stream socket (`AF_UNIX`, `SOCK_STREAM`) to a path, listens on
it and starts the driver with the path in `TERSE_BUS_SOCKET` (section 2).
The driver connects to that path, and the two speak section 1 on the
connection. The path is UTF-8 and at most 103 bytes (`sun_path` holds 104
bytes with its terminating NUL on macOS, 108 on Linux and Windows); a core
whose usual directory would make it longer uses a shorter one, or refuses
the run (status 2). The core removes the socket's file once the driver has
connected, and the session directory at the end of the run. The socket is a
byte stream; each end must be able to read it with a time limit, to tell
without reading whether it can be read, and to see its end (the other
process closed it or exited). Handles and descriptors of the session are
not inherited by the drivers a core starts (a driver holding another's
connection would hide that one's exit).

| | Linux | macOS | Windows |
|---|---|---|---|
| The socket | `AF_UNIX` | `AF_UNIX` | `AF_UNIX` through Winsock 2.2 (`afunix.h`), Windows 10 version 1803 or later, Windows Server 2019 or later |
| The session directory (reference) | in `/dev/shm` (a tmpfs) if there is one, else the temporary directory; mode 0700 | in `/tmp` (`$TMPDIR` makes socket paths too long); mode 0700 | in the user's temporary directory (`%TEMP%`), which only the user may open |
| A shared file is mapped with | `mmap(PROT_READ \| PROT_WRITE, MAP_SHARED)`, the whole file | the same | `CreateFileMappingW(file, NULL, PAGE_READWRITE, 0, 0, NULL)` and `MapViewOfFile(FILE_MAP_READ \| FILE_MAP_WRITE)`, the whole file; every end opens the file with `FILE_SHARE_READ \| FILE_SHARE_WRITE \| FILE_SHARE_DELETE` |
| Sleeping on the address (`1`) | `FUTEX_WAIT` with a timeout and `FUTEX_WAKE`, not the private variants (the word is shared between processes) | `os_sync_wait_on_address_with_timeout` and `os_sync_wake_by_address_all` with their `SHARED` flags (macOS 14.4 or later), or `__ulock_wait` and `__ulock_wake` with `UL_COMPARE_AND_WAIT_SHARED` (and `ULF_WAKE_ALL`; macOS 10.12 or later) | none: an end never writes `1` |
| Sleeping on the socket (`2`) | yes | yes | yes: the only way |
| Pinning to a CPU (`TERSE_BUS_CPU`) | `sched_setaffinity` | not possible: ignored | `SetThreadAffinityMask` (CPUs 0 to 63) |
| What ends a run cleanly (section 10) | `SIGINT`, `SIGTERM`, `SIGHUP` | the same | Ctrl+C, Ctrl+Break, closing the console |
| A driver's CPU time and waits (`PERFORMANCE.md` R14) | all of them | CPU time and run-queue wait | CPU time only |

**Why `AF_UNIX` on Windows, not a named pipe.** Windows has had
Unix-domain stream sockets since Windows 10 version 1803 (2018), with the
semantics the other systems have: a byte stream at a path, its end seen
when the other process closes it or exits, `WSAPoll` to wait for it with a
time limit, `SO_RCVTIMEO` for a read with one. So the protocol, the
handshake (a path in `TERSE_BUS_SOCKET`, one connection accepted), the
liveness check and the socket wake below need no Windows variant, and C
(Winsock), Go (`net.Dial("unix", path)`), .NET (`UnixDomainSocketEndPoint`)
and Rust (Winsock through `windows-sys`: Rust's standard library has Unix
sockets only on Unix, so the reference makes the dozen calls itself) all
reach them. A named pipe is another model: a server instance to create
before each client connects, byte or message mode, no time limit on a read
of a synchronous handle (overlapped I/O for one), the reads and writes of a
synchronous handle serialised against each other, and nothing like `poll`;
two independent implementations would need a second transport section to
agree. The cost of the choice: Windows before 10 version 1803 is not
supported, and a language whose library has no `AF_UNIX` on Windows (the
Python driver, `crates/terse-devices/python`, among them as written) cannot
be a driver there without calling Winsock itself.

**Why the socket wakes on Windows.** `WaitOnAddress` and
`WakeByAddressAll` work only between the threads of one process (they key
on the address in the process), so they cannot wake another process.
Named events or semaphores could, but their names would have to be agreed
and passed, and they would cost what a byte on the socket costs: one system
call to wake, one to sleep, and only when the other end has gone to sleep
(the calls of a busy frame follow each other while the ends spin, with no
system call at all). The socket is already open, needs no name, and
doubles as the liveness check: its end wakes a sleeper at once.

**macOS.** The two address waits are the same kernel wait (`os_sync_*` is
built on the ulock calls), so an end that sleeps in one is woken by the
other's wake; a reference end uses `os_sync_*` where the system has it and
the ulock calls before 14.4, and looks both up when it starts, so that one
program runs on both (with neither, it sleeps on the socket). The ulock
calls are not a public API, though stable since 10.12 and used by the C++
and Rust standard libraries. Neither has been run yet; the wake between
the two kinds is to be confirmed on a Mac. `/tmp` is
on a disk: pages of a shared file stay in memory while mapped, but the
system may write dirty ones back now and then; a core may put its session
directory on a RAM disk (the reference: `TERSE_BUS_DIR`).

**What a conforming implementation may fall back to, on any system:**

1. **The socket alone.** Shared memory is for speed only (section 12): a
   core that cannot share files on a system (or does not want to) does not
   offer it, and a driver may always decline it. The socket transport is
   always enough.
2. **Sleeping on the socket.** On shared memory an end may always sleep on
   the socket (`2`) rather than on the address, on any system (the
   reference: `TERSE_BUS_WAKE=socket`, which is how the tests run that way
   on Linux). It must still wake the other end the way that end's word
   says.
3. **No timing of other processes.** A core that cannot read a driver's
   CPU time or run-queue wait still runs it; `PERFORMANCE.md` R15 and
   `RECORD.md` section 4 say what it may conclude without them (it warns,
   it never stops a machine for it).

---

## 5. The call table

Each entry is 64 bytes and describes one function of the device. A
function not in the table is an unknown call: the core returns
`FFFF_FFFFh` and charges 4, without the driver (SPEC §calls.convention).

The table also gives each call's class (SPEC §charges). A call with flag bit 1
is **deferred**: its result and charge follow from its arguments and its
buffer's length by the entry's rules (section 6), so the core can compute
them; its base charge is 4. Every other call is **answered**: only the
driver knows its result or charge, and its base charge is 64, which the
core charges, also when one of its checks fails; the driver's charge is
on top. So "deferred" is exactly "computable by the core from the table",
and a driver that marks a call batchable states that its class is
deferred. The System device's calls and device 8's are the machine's own
(SPEC §charges: core, 4) and never reach a driver.

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | Function |
| 1 | 1 | Flags: bit 0 the call sets `a1`; bit 1 deferred, so batchable (section 6), else answered; bit 2 the call has a buffer |
| 2 | 1 | Buffer: the argument (0–5) holding its address |
| 3 | 1 | Buffer: the argument holding its length, or `FFh` for a fixed length |
| 4 | 4 | Buffer: bytes per unit of the length argument; for a fixed length, the length |
| 8 | 4 | Buffer: least length in bytes |
| 12 | 4 | Buffer: greatest length in bytes |
| 16 | 1 | Buffer access: bit 0 the device reads it; bit 1 the device writes it; bit 2 it may lie in ROM (only for one not written) |
| 17 | 1 | A checked argument (0–5), or `FFh` |
| 18 | 2 | Reserved |
| 20 | 4 | The checked argument's greatest value |
| 24 | 4 | Result if the length is out of range |
| 28 | 4 | Result if the buffer is not where it must be, or the checked argument is too large |
| 32 | 1 | Result rule (batchable calls): `0` none; `1` constant: `a0` = value, and `a1` = value 2 if flag bit 0; `2` `a0` = the rule argument; `3` `a0` = 0 if the rule argument ≤ value, else 1 (rules 2 and 3 never set `a1`) |
| 33 | 1 | Rule argument |
| 34 | 2 | Reserved |
| 36 | 4 | Value |
| 40 | 4 | Value 2 |
| 44 | 4 | Charge rule (batchable calls): constant units |
| 48 | 1 | Charge rule flags: bit 0 add 1 per 4 bytes of the buffer, rounded up |
| 49 | 1 | Charge rule: an argument, or `FFh` |
| 50 | 2 | Charge rule: units added per byte of the buffer (u16) |
| 52 | 4 | Charge rule: low |
| 56 | 4 | Charge rule: high |
| 60 | 4 | Charge rule: units added when low ≤ the argument ≤ high |

The core reads only the fields an entry uses: the buffer fields
(offsets 2–16) only if flag bit 2 is set, the checked argument's
greatest value only if offset 17 is not `FFh`, the result and charge
rules only if flag bit 1 is set, the charge rule's range only if offset 49
is not `FFh`. Writers set every unused field to zero, except three
argument numbers, which are `FFh` ("none") when unused: the buffer's
length argument (offset 3) of a call without a buffer, the checked
argument (17) and the charge rule's argument (49). (The reference tables
also put `FFFF_FFFEh` in the two failure results of a call without a
buffer; the core never reads them.) A call that is not batchable has rule
0 and charge zero.

**The core's checks.** For every call, in this order (steps 1 and 2 only
for a call with a buffer, step 3 only for one with a checked argument,
whether or not it has a buffer):

1. The length is the length argument × the scale (computed without
   overflow), or the fixed length. Outside least … greatest: the result is
   the length result.
2. A buffer of length 0 is valid at any address. Otherwise it must lie
   wholly in RAM, or (if bit 2 and the device does not write it) wholly in
   ROM or wholly in RAM: if not, the place result.
3. The checked argument must be at most its greatest value: if not, the
   place result (SPEC §calls.convention: a bad buffer and a bad argument
   give the same `FFFF_FFFEh` in every call that has both).

A call that fails a check is charged its base charge (4, or 64 for an
answered call) and does not reach the driver (SPEC §charges: nothing is copied).
This is the whole of SPEC §calls.convention's rules, including its five exceptions,
which are only different results in the table.

### 5.1 The tables of the reference drivers

Terminal (slot 2):

| Fn | Call | Buffer (address, length × scale, range, access) | Checked | Deferred: result, charge (else answered) |
|---|---|---|---|---|
| 0 | `write` | `a0`, `a1` × 1, 0 … 4,096, read, ROM | — | `a0` = `a1`; 2 per byte |
| 1 | `command` | — | — | `a0` = 0; 150 if 1 ≤ `a0` ≤ 3 |
| 2 | `set_cursor` | — | — | 0; 0 |
| 3 | `get_cursor` | — | — | answered (reads state) |
| 4 | `set_colours` | — | — | 0; 0 |
| 5 | `get_colours` | — | — | answered |
| 6 | `cells_read` | `a1`, `a2` × 4, 0 … 9,600, written, RAM | `a0` ≤ 2,399 | answered (writes) |
| 7 | `cells_write` | `a1`, `a2` × 4, 0 … 9,600, read, ROM | `a0` ≤ 2,399 | `a0` = `a2`; 1 per 4 bytes |
| 8 | `palette_set` | `a1`, `a2` × 3, 0 … 768, read, ROM | `a0` ≤ 255 | `a0` = `a2`; 1 per 4 bytes |
| 9 | `size` | — | — | 80 and 30 (`a1`); 0 |

All buffer failures give `FFFF_FFFEh`.

2D vector display (slot 3): `submit` (`a0`, `a1` × 1, 1 … 16,384, read,
RAM only; both failures give 1; deferred: 0, 1 per 4 bytes), `clear`
(0), `style` (0 if `a0` ≤ 1 else 1), `persistence` (0 if `a0` ≤ 255 else
1), `fill` (0 if `a0` ≤ 1 else 1); all deferred, charge 0 but `submit`'s.

3D vector display (slot 9, the same driver): `submit` (as slot 3's),
`clear` (0), `style` and `persistence` (as slot 3's, and they set slot 3's
style: SPEC §vector3d.calls); all deferred.

Content (slot 4): `submit` (`a0`, fixed 52 bytes, read and written, RAM
only; the place result is 0); answered.

Page (slot 7): `submit` (`a0`, `a1` × 1, 1 … 65,536, read, ROM; length
result 1, place result `FFFF_FFFEh`; deferred: 0, 1 per 4 bytes),
`clear`, `region`, `scroll`, `background` (deferred, 0), `locate`
(answered).

Sound (slot 6, SPEC §sound): `submit` (`a0`, `a1` × 1, 1 … 4,096, read, ROM;
both failures give 1; deferred: 0, 2 per byte), `wave` (`a1`, fixed 64
bytes, read, ROM; `a0` ≤ 31; deferred: 0, 1 per 4 bytes), `sample`
(`a1`, `a2` × 1, 0 … 65,536, read, ROM; `a0` ≤ 3; deferred: 0, 1 per 4
bytes), `info` (deferred: 16 and, in `a1`, the capabilities 1); failures
of `wave` and `sample` give `FFFF_FFFEh`.

## 6. Batching

A deferred call (entry flag bit 1) changes the device's state but returns
nothing that depends on it: its result and charge follow from its
arguments and its buffer's length by the entry's rules. The core does not
wait for it. After its checks it computes the result and the charge,
applies them, and appends the call to the driver's **batch**: a CALL
message (with its address: instance and slot) with the buffer's bytes
inline (on both transports: the program may change the buffer before the
batch is sent). The batch is sent as one BATCH message:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `11h` |
| 1 | 3 | Reserved |
| 4 | 4 | Count |
| 8 | … | The CALL messages, each as a blob: u32 length, then the address and body |

before the driver's next request of any kind (a call that needs a reply,
STATE, PRESENT, another machine's BEGIN), in the same packet. A batch may
hold the calls of several slots and instances: they are carried out in
order, which keeps a shared setting (device 9's `style`) in order with the
calls around it. The driver replies to none of them. A driver must give
the same result and charge as the rule says; the reference drivers check
this in debug builds.

A call that writes guest memory, reads device state, or whose charge or
result depends on the device's state cannot be batched: it is answered.

### 6.1 What the reference devices allow

Deferred, so batchable: terminal `write`, `command`, `set_cursor`,
`set_colours`, `cells_write`, `palette_set`, `size`; every call of the
vector displays; every call of the sound device; page `submit`, `clear`,
`region`, `scroll`, `background`.
Answered: terminal `get_cursor`, `get_colours` and `cells_read` (they read
the device), content `submit` (the handle is the device's), page `locate`
(it is refused when there is no document or after 8 in a frame: device
state). These are SPEC §charges's classes exactly.

`write` takes its whole buffer, at most 4,096 bytes, and costs 2 per byte
however many scrolls it causes (SPEC §terminal.calls), so its entry says "`a0` = `a1`;
2 per byte" and a frame of one-byte writes goes to the driver as a few
batches instead of one round trip a call. A batched call with a buffer
carries a copy of it: on shared memory a big one costs more batched than
sent (the vector display's 16 KiB list).

## 7. Layers and presentation

A visual driver declares the **layers** of each of its slots at the
handshake: an ID (its own name for it) and a z, the place in the stack,
from the bottom. SPEC §vector.layers's order gives:

| z | Layer |
|---|---|
| 10 | Terminal: cell backgrounds |
| 20 | Page: background, text, figures |
| 30 | 2D vector display |
| 35 | 3D vector display |
| 40 | Terminal: characters and cursor |

So one driver may have several layers, above and below other drivers'.
A machine's screen is its layers composed in increasing z (equal z:
increasing slot, then the order declared) over opaque black.

When it presents (HELLO flag bit 1), the core sends PRESENT for an
instance to each of its drivers with layers, in order: the driver brings
that machine's layers up to date for a picture of a size the core chooses:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `50h` |
| 1 | 3 | Reserved |
| 4 | 2 | Picture width in pixels |
| 6 | 2 | Picture height |
| 8 | 16 | The 640 × 480 screen in the picture: x, y, width, height (f32 each) |
| 24 | 2 | The terminal's glyph cell width in pixels; 0: its native cell |
| 26 | 2 | Reserved |

The driver replies `D0h`:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `D0h` |
| 1 | 1 | `k`: layers |
| 2 | 2 | Reserved |
| … | | `k` times: slot (u8), layer ID (u8), flags (u8: bit 0 a new buffer file, whose path follows), path (u16 length and bytes, if bit 0), damage: count (u32) and that many rectangles, x0, y0, x1, y1 (i32 each, half open, in picture pixels) |

A **layer file** is the driver's: a file it creates in the session
directory, picture width × height × 4 bytes, one u32 a pixel, rows from
the top, **premultiplied** colour: alpha in bits 24–31, red 16–23, green
8–15, blue 0–7 (sRGB values, as the reference renderer blends them); in
the phosphor style the same words hold light, not premultiplied (section
7.1). The core maps it read-only. A driver gives a new file when the picture size
changes, and its first damage then covers the whole picture. The damage is
what changed in the file since the driver's last PRESENT for this
instance; the core composes only there.

### 7.1 The core's picture

The core draws each machine at the size it shows it (as terse-host's
software renderer does), so a driver never draws more pixels than are seen
and text is rasterised for its size, not scaled:

- a single machine: a picture the size of the window, the screen the
  largest whole multiple of the native size that fits, centred (the
  letterbox is the terminal's background colour 0), glyphs at their native
  cell;
- the shell: the window at 4:3; glyph cells `⌊width / 80⌋` pixels wide
  (its native cell if that is the size);
- a machine in a tile (SPEC §machines.screens): a picture the size of its rectangle,
  glyph cells `⌊width / 80⌋`;
- a machine on a panel of the shell's scene: a picture as large as the
  panel looks (whole cells of 80 columns, kept while the panel moves a
  little), glyph cells of that width.

The picture is then put together as terse-host's software renderer puts it
together, and with its code (terse-host's `soft::scene` and `soft::panel`,
used by both): the shell's screen; with Screens in effect (SPEC §machines.screens), the
shell's **scene**: its screen with its Surfaces' sources blacked out, the
panels far to near (each machine's picture, from a pyramid of it; a
Surface from the shell's own screen), then the shell's 3D layer over them;
each machine shown flat in its rectangle, in increasing handle order (a
machine with Surfaces of its own as its own scene, SPEC §vector3d.presentation); the
machine a finished `face` shows flat in the view's rectangle; the fault
line (below); the host's consent prompt over everything.

**Panels are the core's.** A Screen shows another machine, and a Surface
shows a part of the machine's own picture: the pixels are the core's, put
together from several drivers' layers (and, for a Screen, of another
instance). So the core draws the panels, from geometry it projects itself
from device 9's list (the `vector3.list` property: terse-core's
projection, the one SPEC §machines.input's pointer routing over panels already
needs), and the 3D driver draws only lines, into its layer (z 35), which
the core composes among the others when no scene is in effect and over
the panels when one is. The alternative, handing the 3D driver the other
layers' pixels to draw panels with, would make one driver read every
other driver's output and every other machine's, and draw a picture that
is not its device's; this way each driver draws only its device, and what
spans machines (tiles, panels, the prompt) is the compositor's.

The 3D driver needs two facts to draw: the eye (`core.eye`: where the
window's pointer puts it, `--eye` headless; SPEC §vector3d.space) and, for the shell,
the view of its scene (`core.view`: which Screens are in effect and the
`face` motion, SPEC §machines.calls), which the core publishes (section 7.2): its
lines are then cut by the panels' faces and move with the view.

**The fault line** is the terminal's: when a machine faults (SPEC §stopping)
the core publishes `core.status`, "fault: …", and the terminal shows it
over its bottom row as terse-host does (colours 15 on 1).

**The consent prompt** (SPEC §content.feeds) is the core's: the content driver
publishes the question it waits on (`content.prompt`: the asking machine,
the request, what is asked, how many wait); the core draws it over
everything, with terse-host's grid renderer in the overlay's colours, takes
the keyboard while it is shown (Y allows, N or Esc refuses; no key or text
reaches a machine, nor the release of a key pressed then) and gives the
answer back as `core.answer`.

**The phosphor style** (SPEC §vector.phosphor) is the core's to draw, as
terse-host draws it and with its code: terse-host's tube
(`soft::phosphor`, `soft::screen`: the beam, the afterglow and its smear,
the halo, the tone curve, the tubes and the glass, the shedding of the
look), shared, not copied. A machine is shown in the phosphor style when
its vector display's style says so (the core reads it in `vector.list`)
or the core forces it (`terse-bus --force-phosphor`). Then:

- the core publishes **`core.light`** = 1 to the machine's drivers (0
  otherwise), and sends PRESENT with the screen's top left on whole
  pixels (x and y rounded towards zero), the tube's pixel grid;
- the terminal and page drivers draw their layers as **light**: no cell
  backgrounds and no page background (the screen is black, as the style
  requires), the page's figures and filled shapes as outlines, and each
  pixel **not premultiplied**: the colour (sRGB) in bits 0–23 and the
  coverage in the alpha. A pixel gives linear(colour) × alpha / 255 of
  light: a glyph's coverage times its colour, the reference host's text
  light. Where a driver's own primitives overlap it adds their light
  (linear; the alpha at most 255);
- the vector driver leaves its layers transparent: the core draws the
  beam itself, from device 3's state (`vector.list`) and device 9's list
  (`vector3.list`), within the machine's caps (below);
- the core adds up the light of the layers of the drivers that subscribe
  to `core.light` (but the vector displays'), the beam over it, and
  keeps the tube: the afterglow moves on once for each frame the machine
  ran, and the picture is the tube's. A scene's 3D lines over its panels
  (Screens, Surfaces) are drawn by the core from its own projection, as
  terse-host's are.

A driver that does not subscribe to `core.light` is not told, and its
layers are left out of a machine in the phosphor style (the C terminal
driver, which draws nothing, loses nothing). A core that composes less
often than every frame builds the afterglow up over the frames before a
picture (`terse-bus --screenshot` composes the last 600, as
`terse-host --screenshot` does). With the reference drivers terse-bus's
picture of a machine in the phosphor style is terse-host's software
renderer's, pixel for pixel, in every case its tests compare (sparks'
phosphor session, hello and holo forced into it, a shell with four
machines as tiles and as panels, `crates/terse-devices/tests/pictures.rs`);
only a page whose glyphs overlap can differ, by the rounding of their
added light to 8 bits.

**The system ink** (SPEC §machines.ink) is the core's: it counts what it
shows with terse-core's count, the pictures' areas, then the machines'
2D lists (device 3's state from `vector.list`) and 3D lists
(`vector3.list`) at their scales, as a host with built-in devices does,
frame by frame the same plates, caps and cuts. A picture that is a plate
is drawn by the core alone, a plain rectangle (a blank terminal's
background, black), without PRESENT to its drivers; a panel that is a
plate is not drawn. Each machine's vector driver is given its caps as
**`core.caps`** and draws its lists within them; in the phosphor style the
core's beam is drawn within them. A vector driver that does not publish
`vector.list` gives the core no 2D list: it counts nothing for it (so
cannot cut it), and the core cannot draw its beam.

### 7.2 Properties

A **property** is a named, per-machine value (bytes) that one end
publishes and others subscribe to: a presentation fact one device's
drawing needs from another, or a fact of a device the core needs for the
machine's own devices. Drivers name the properties they publish and
subscribe to at the handshake. The core carries them:

- it reads a driver's published values with GET when it needs them (for
  a subscriber, before it presents the subscriber's machine; for itself,
  when it uses one), and only if the driver may have changed them since
  it last asked (any message about that instance since: a call, BEGIN,
  END, RESTORE, SET);
- it gives a subscriber a value with SET when it changed, before the next
  message that needs it (on shared memory, in the same packet).

So a driver never reads another's state, and the core never reads a
driver's state but through a name it was given. The properties of the
reference drivers and core:

| Name | Published by | Subscribed by | Value |
|---|---|---|---|
| `vector.outlines` | vector display | page | 1 if figures and filled shapes are drawn as outlines (fill mode 1, or the phosphor style: SPEC §page.layout), else 0 |
| `vector3.list` | vector display | core | device 9's list (SPEC §vector3d), which device 8 (`face`), the shell's scene and the pointer over panels (SPEC §machines.calls, §machines.screens and §machines.input) look at |
| `vector.list` | vector display | core | device 3's state as `STATE.md` 1.7's section: style (u8), persistence (u8), fill mode (u8: 1 outlines), the list's length (u32) and the list; the core counts the system ink from it and draws the phosphor style's beam (section 7.1) |
| `core.light` | core | terminal, page, vector display | 1: the machine is shown in the phosphor style and its layers are light (section 7.1); 0 or empty: the clean style |
| `core.caps` | core | vector display | the ink caps the machine's lists are drawn within, its share of the system ink (SPEC §machines.ink): the 2D cap (u64) and the 3D cap (f64), each at most the device's own; empty: the devices' own |
| `core.eye` | core | vector display | the eye: two f32, screen units from the reference eye (SPEC §vector3d.space) |
| `core.view` | core | vector display | the shell's scene: which Screens are in effect (u8) and the view (12 f64: the linear part by rows, then the translation); empty: none |
| `core.status` | core | terminal | the line shown over the bottom row (UTF-8): a fault; empty: none |
| `content.prompt` | content | core | the consent question waiting: the asking machine (u32), the request's handle (u8), how many wait (u32), what is asked (kind and fields); empty: none |
| `core.answer` | core | content | the answer: the question's machine (u32) and handle (u8), then 1 allow or 0 refuse |
| `sound.frame` | sound | core | the frame counter after the frame the driver rendered last (u32), then its 800 sample pairs (i16, left and right interleaved; SPEC §sound.presentation); empty before the first |

`vector3.list` is read in the middle of a frame when device 8's `face`
needs it: the core sends GET, after the batch, as it would a call.

**Sound.** The sound driver renders a machine's samples at END (SPEC
§sound.render: the end of the frame) and plays nothing itself: the core reads
`sound.frame` after the machine's frame (GET, in front of which the END
goes on shared memory), mixes the machines that ran in the system frame
(SPEC §sound.presentation) and plays or writes the result. The frame counter in the value
tells the core whether the driver rendered since it last asked (a machine
that did not run renders nothing). So sound, like the picture, is put
together by the core from what each machine's driver gives, and one
driver serves every machine.

## 8. Recording and replay

INPUTS carries the driver's **input record** for the frame: opaque bytes
from which the driver can produce the same INPUTS again. The core records,
per frame, the host's inputs (time, controllers, events, the random bytes
the program took) and each driver's record, and the state hash after the
frame. To replay, the core sends each driver its record in BEGIN (flag
bit 0) and the HELLO replay flag; the driver must then take its inputs
from the record alone, not from its live source, and must update its own
state from them as it would have (a content driver frees the handles the
recorded completions complete).

The reference drivers' records: the content driver's is its completions
(in `STATE.md` 5's completion format, preceded by a count) and a
feeds-updated flag; the page driver's is the events it reported. A driver
with no inputs gives none.

The bus recording file is the core's own (`terse-bus --record`, a single
machine): `TBRC`, version 1, the image ID, the machine's grants (one byte,
`00h` or `01h`, per optional device the image lists, as `STATE.md` 5;
none for most images), then per frame: the frame
counter, the Unix time, the 32 controller bytes, the host's events (count
and events), the random bytes taken (count and bytes), the drivers' records
(count; then the driver's lowest slot, length, bytes for each), a byte:
0, or the slot of the device for which the host stopped the machine after
this frame (`PERFORMANCE.md` R15; the frame is then the last), and the
state hash (after that stop). A replay stops the machine where the
recording says. Version 0 had no stop byte; a core still reads it. It is not the format of `STATE.md` 5, whose completions are a
content device's: a core that knows nothing of content cannot write that
one.

## 9. Host services

A shell's host does more than run machines: it fetches the images the
shell starts (SPEC §machines.calls `start`, `start_program`), and stores its
snapshots (SPEC §machines.snapshots). A core that knows nothing of the network or the
store asks a driver that does: a driver names the **services** it gives at
the handshake, and the core calls them with SERVICE, for the instance on
whose behalf it asks (a snapshot's owner is the shell's program, SPEC
§machines.snapshots). The reference content driver, which holds the run's network node,
gives:

| Service | Request | Answer |
|---|---|---|
| `image.fetch` | the machine (u32), the image ID | `1` and the image's bytes, `2` not found, `3` failed |
| `program.fetch` | the machine, the program (publisher, name as a string), the built-ins (count, IDs) | as `image.fetch`: the current version the host holds |
| `snapshot.save` | the slot (u8), the snapshot file (STATE.md 4) | `1` stored, `0` not |
| `snapshot.load` | the machine (u32), the slot | `1`, the image file and the snapshot file (blobs); or `0`, the reason (SPEC §machines.calls's failure code), whether its image ID follows and it, the message |
| `snapshot.delete` | the slot | nothing |
| `snapshot.table` | nothing | the 16 slots' `snapshot_info` records (SPEC §machines.calls) |

The core calls them after the system frame in which the shell asked, and
the answers take effect at the start of the next (SPEC §machines.frames), as the
built-in hosts do: the call waits for the fetch, so a headless run is
reproducible. (In a window a fetch from the network holds the frame loop
for its duration; a core that minds would ask in a thread of its own.)

## 10. When a driver fails

A driver **fails** when its process exits or closes its socket, when it
does not answer a request within `--driver-timeout` (30 s by default:
the content driver waits for the network that long at most), when it
answers with ERROR, or when it breaks the protocol (a reply that is not
the one asked for, a write outside its buffer).

The machines that use the driver cannot go on: their devices' state is in
it, and a frame may have stopped half way through, some of its calls
carried out and others not. Restarting the driver would not bring that
state back. Nor can the core make the device absent (calls returning
`FFFF_FFFFh`): a device's absence is a property of the image (SPEC §image), and
a program that has used a device would see it vanish. And a machine fault
(SPEC §stopping) is the program's, recorded in the state with a kind SPEC
defines; a driver's failure is the host's. So:

- **The run stops.** The core stops running frames, kills every driver it
  started (none is left behind), removes the session's files, prints which
  driver failed, in which frame, and why, and exits with status 3
  (`STATE.md` 6.4: status 2 is a run that did not start, a refused image
  or a driver that failed its handshake, section 2.1). A window shows the
  last picture until it is closed, with the reason in its title and on
  standard error. With one driver process per device for all
  machines, a failed driver is a failed device of every machine, so this
  is also what a per-machine rule would come to.
- Nothing is written to a machine's state: hashes up to the failure are
  those of the run so far.

A SPEC change would be needed for anything finer: a stop reason "the host
lost a device" (neither exit nor fault) that a shell could show for a
machine whose own driver failed, if drivers were ever per machine.

**A driver that answers, but too slowly**, has not failed: it is timed.
The core waits for each reply, so the time it waited for each driver in a
frame is the driver's, and the operating system says how much of it the
driver spent waiting for a CPU (`PERFORMANCE.md` R14). One that makes
frames late is warned about, then the machines that use it are saved and
stopped, halted with the device's slot (`SPEC.md` §stopping; `PERFORMANCE.md`
R15), and the record says so (`RECORD.md`). `terse-bus` does this in a
window and with `--realtime` (`--perf-rule on|off` to choose), and writes
the record there to the data directory's `perf/` (`--perf-log <dir>|off`).
`native/driver-example` has a deliberately slow mode for testing it
(`TERSE_EXAMPLE_SLOW`, its README).

`terse-bus` also ends cleanly on SIGINT, SIGTERM and SIGHUP (on Windows:
Ctrl+C, Ctrl+Break and closing its console) and when its window is
closed: it finishes the frame, sends SHUTDOWN to every driver, reaps them
(killing one that does not exit within 2 s) and removes the session
directory. A fault ends it by SIGTERM, as `terse run`; on Windows, which
has no signals, with status 255.

## 11. Measurements

i7-10750H (12 threads), release build, the Rust drivers, while other builds
ran on the machine (figures vary by some tens of percent between runs).

**Calls** (`programs/hostile`, every frame's budget spent on the cheapest
calls of one kind; `terse-bus --bench --frame-times`, ms per frame, median
/ worst of each 60-frame phase):

| Phase (calls per frame) | Shared memory | Socket |
|---|---|---|
| terminal `get_cursor`, answered (1,923) | 1.44 / 1.47 | 12.9 / 16.4 |
| vector `clear`, deferred (25,000) | 3.80 / 3.84 | 3.80 / 4.80 |
| page `locate`, answered (1,923) | 1.40 / 1.44 | 9.41 / 12.6 |
| terminal `write` of 1 byte, deferred (12,500) | 2.35 / 2.42 | 2.36 / 2.49 |
| terminal `cells_read` of 2,400 cells, answered (51) | 0.07 / 0.16 | 0.50 / 0.71 |
| vector `submit` of 16,384 bytes, deferred (30) | 0.14 / 0.15 | 0.14 / 0.15 |
| terminal `write` of 4,096 line feeds, deferred (15) | 0.41 / 0.45 | 0.38 / 0.44 |

Every phase fits a 16.7 ms frame on shared memory; on the socket an
answered call costs about 6 µs, so a program that does nothing but answered
calls comes close to the frame. (Version 0 measured about half these
figures for answered calls on an idle machine; the address and the
per-instance lookups add a little to each message.)

**A shell** (`terse-bus --shell --present-every-frame --size 1280x960
--bench`, 900 frames; the children are holo's city, sparks, sketch with a
stroke, the reader paging up and down, then fetch, hello, holo's rings and
sparks again): emulation of the whole system per frame (the bus included),
then composition (PRESENT to every driver of every machine shown, and the
core's composition), p50 / p99 / max in ms; against terse-host's software
renderer (`--bench-render --renderer software`, 12 threads, drawing only)
and `terse run --shell --bench` (emulation, the devices compiled in):

| Scene | Bus emulation | Bus composition | terse-host drawing | Built-in emulation |
|---|---|---|---|---|
| Shell idle (launcher) | 0.008 / 0.037 / 0.29 | 0.029 / 0.045 / 36 | 0.00 / 0.00 / 0.01 | 0.003 / 0.005 / 0.47 |
| One child | 0.010 / 0.029 / 1.5 | 0.050 / 0.077 / 42 | 0.01 / 0.01 / 0.01 | 0.004 / 0.010 / 1.0 |
| Four children busy | 0.26 / 0.58 / 1.5 | 3.3 / 11.2 / 41 | 0.80 / 1.19 / 2.0 | 0.15 / 0.25 / 1.2 |
| Eight children busy | 0.29 / 0.87 / 1.6 | 3.4 / 17.1 / 592 | 1.18 / 2.09 / 590 | 0.20 / 0.34 / 1.3 |

The worst frames are the first of a machine (its layer files, fonts loaded
in the page and terminal drivers). The processes are the core and four
drivers whatever the number of machines. Running in real time (60 frames
a second, presenting every frame), over 10 s:

| Scene | Processes | CPU (adaptive spin) | CPU (fixed 2 ms spin) | Memory (PSS / RSS, MiB) |
|---|---|---|---|---|
| Shell idle | 5 | 3.0 % | 50 % | 87 / 122 |
| One child | 5 | 4.2 % | 51 % | 106 / 153 |
| Four children busy | 5 | 64 % | 124 % | 102 / 144 |
| Eight children busy | 5 | 77 % | 126 % | 412 / 451 |

(terse-host, one process: 48 MiB with four children, 359 MiB with eight:
both load the CJK fallback font when a machine shows CJK text, there in the
terminal driver.) Start-up to the first frame: 80–90 ms (drivers started
and connected), 130–150 ms to the first picture in a window.

**In a window** (`terse-bus --window --selftest`, 1280 × 960, in a headless
Wayland compositor): present interval p50 16.6–16.7 ms in every run; p99
20–46 ms (the frames in which a machine starts); the scripted input reached
the same machines as headless (the same state hashes).

## 12. A minimal core

What a core must do to run one machine headless with drivers, check it
against the expected hashes, and nothing more (`native/core-c`'s `tcore`
does exactly this, in about 640 lines of C for the bus). The order is
the order of a run:

1. **Start each driver** it needs (section 2): listen on a fresh socket,
   start the program with `TERSE_BUS_SOCKET`, accept one connection
   within 10 seconds.
2. **HELLO** (section 1.1): flags 0 (no shared memory, no presenting, no
   replay), the slots it asks for (section 1.1: listed slots, both of 3
   and 9, 10 with the terminal), a session directory, an empty bus file.
   From the reply it needs the slots, their versions and their call
   tables; it skips the name, native size, layers, properties and
   services. A wrong set of slots, or a transport other than 0, refuses
   the run.
3. **Grants** (SPEC §image): which optional devices the drivers serve in
   the version listed; a required device none serves refuses the run.
4. **OPEN** instance 0 (section 1.2) to every driver: the image ID, the
   ROM and RAM lengths, an empty memory file, the driver's slots the
   machine has, no identity, the ROM. A refusal refuses the run.
5. **Each frame:**
   - step 1: **BEGIN** to every driver (frame counter, Unix time, the
     host's events, an empty replay field), **INPUTS** back; queue the
     events by position, apply the writes, queue the completions
     (section 1.5); the budget gains 125,000 less every driver's charge;
   - step 2: each `ecall` to a driver's slot: the call table's checks
     (section 5), then **CALL** and its **RESULT** (an answered call costs
     64 plus the driver's charge, a deferred one 4 plus the table's
     charge; writes only within the write buffer, section 1.4), answering
     any **READ** with **DATA** meanwhile;
   - step 3: **END** to every driver.
6. **STATE** for each section, between frames, when it hashes: the
   sections in the order and on the rules of section 1.7's table.
7. **SHUTDOWN** to every driver at the end; close the sockets, reap each
   process (kill one that has not exited 2 seconds later).

A message that is not the reply asked for, or ERROR, is a driver failure:
the run stops with status 3 (section 10).

What such a core may leave out, and when it would need it:

| Left out | Needed for |
|---|---|
| BATCH (section 6) | speed only: a deferred call sent as a CALL gets the same result and charge from the driver (`tcore --no-batch` checks this) |
| Shared memory (section 4) | speed only: offer none, and every driver takes the socket |
| PRESENT, layers, native size, section 7.1 | a picture |
| GET and SET (section 7.2) | presenting (subscribers get their properties before PRESENT; the phosphor style and the system ink: `vector.list`, `core.light`, `core.caps`), device 8 (`vector3.list`: a shell), playing sound (`sound.frame`) |
| SERVICE (section 9) | a shell (images to start, snapshots) |
| RESET | nothing: the reference core never sends it |
| CLOSE | a system (a machine the shell stops); at the end of a run SHUTDOWN is enough |
| RESTORE (section 1.7) | snapshots (`--load-state`, a shell's `restore`) |
| HELLO flag bit 2, BEGIN flag bit 0, the input records (section 8) | recording and replay |
| READ and DATA | nothing, for drivers that never READ; the content driver reads its data ranges with it (SPEC §content.request), so a core that starts it must answer |

Besides the protocol, the core keeps four facts of particular devices,
all stated above: slots 3 and 9 go to one driver, asked for together
(1.1); slot 10 goes with slot 2 and has no calls (1.1); the order and
presence of the state's device sections (1.7); and device 8, which is the
core's own and absent on a machine that is not a shell (SPEC §image).

## 13. A minimal driver

What a driver must do to serve one device for any number of machines,
drawing nothing, on the socket. `native/driver-example/terminal.c` does
exactly this for the terminal, with a comment at each step; its README
lists the same steps.

1. **Connect** to `TERSE_BUS_SOCKET`. Read messages as section 1 frames
   them (length, address, body); every reply carries the address of its
   request.
2. **HELLO**: reply `81h`, protocol 1, transport 0 (the socket, whatever
   is offered), a name, native size 0 × 0, then for exactly the slots it
   was asked for (it may leave out 10, and must refuse with ERROR a set it
   cannot serve): the slot, the device version, no layers, the call table
   (section 5). Then no properties published, none subscribed, no
   services.
3. **OPEN**: a new instance, its device at reset (SPEC §reset); reply
   `83h` with `01h` (taken), or `00h` and a reason, for example a device
   version it does not implement. Keep one state per instance; an OPEN
   of an instance already open starts it again.
4. **CLOSE**: drop the instance. No reply.
5. **CALL**: carry the call out on the instance and slot of its address
   and reply RESULT: `a0`, `a1` if the table says the call sets it, the
   charge on top of the base charge, and the bytes it writes (only within
   the call's write buffer). The core has already made the table's checks
   and sent the read buffer's bytes. Every function of the table must be
   handled, deferred ones too: a core that does not batch sends them as
   CALLs, and the reply must then be what the table's rules say.
6. **BATCH**: carry out each CALL in it, in order (they may be for
   several instances and slots), and reply to none.
7. **BEGIN**: reply INPUTS. A device with no inputs replies a charge of
   0, no events, no writes and an empty record. BEGIN's replay field is
   always there.
8. **END**: no reply; do what the device does at the end of a frame (the
   terminal: nothing).
9. **STATE**: reply `C0h` and the slot's section of the serialization
   (`STATE.md`, the bytes after the present flag), for the instance and
   slot of the address.
10. **SHUTDOWN**, or the socket closed: exit. Any other message it does
    not handle: reply ERROR with the reason, and exit.

What it may leave out: shared memory (it never chooses it); PRESENT and
layers (sent only to a driver that declares layers, and only when the
core presents); GET and SET (sent only to a driver that publishes or
subscribes to a property); SERVICE (only to one that names a service);
RESET (never sent by the reference core; trivial anyway); RESTORE, if it
does not mind that a run restoring a snapshot then fails with its ERROR;
the input records of section 8 (a device without inputs has none).

A device must be **complete** to be served: a driver that answers for a
slot implements every call of that device's version exactly (results,
charges, state), since a program asks `device` and relies on all of it,
and the state hash covers it. There is no partial device, and no driver
for a slot that is no device of v0 (section 2.1, rule 6).

## 14. Open questions

- **The phosphor style's light** (section 7.1): text drivers give their
  light in 8-bit layers (straight colour and coverage), exact for the
  terminal's glyphs and a page's single glyphs, rounded where a page's
  glyphs overlap and added up. A float layer (linear light, 12 bytes a
  pixel) would be exact everywhere at three times the memory and the
  bandwidth; nothing measured so far needs it.
- **Services for a machine the driver was not given** (section 1.9): a
  shell that lists no content device can start machines, but its
  snapshots, whose owner is the identity OPEN gives, cannot be stored by
  the content driver. OPEN for the shell with no slots, to every driver
  that gives services, would carry the identity; not done.
- **Trust.** A driver on shared memory can write any of an instance's RAM
  at any time; the core trusts its drivers as it trusts its own code. A
  core that does not would offer only the socket, check every write, and
  pay for it.
- **System device.** The core keeps slot 1: `wait_frame` and `exit` are the
  frame loop, `next_event` is the queue every device feeds, and the time,
  random bytes and controllers are host inputs the core records. It could
  be a driver only by moving the frame loop with it.
- **A driver per machine.** One process per device serves every machine;
  a process set per machine would isolate a crash to one machine but costs
  a process per device per machine (section 11).
- **Drivers declaring their state section.** The core's one
  device-specific table (section 1.7) means a new device needs every core
  changed before it can be hashed. A driver could instead declare, per
  slot in its HELLO reply, where its section goes (its position in
  `STATE.md`'s order) and its presence rule (a present flag always
  written, or a section only when the machine has the device), and which
  slot's section serves another (9 → 3). `STATE.md` would still fix the
  order for every device, so hashes would not change; the core would read
  the order from the drivers instead of knowing it. Not done: v0 has a
  fixed set of devices, and a declaration is one more thing to get wrong.
- **Installed drivers in a host with built-in devices.** The drivers
  folder (section 2.2) is read by `terse-bus` only. `terse-host` runs
  terse-core's machines with the devices compiled in (`InProcess`) and
  draws by reading their state directly (the terminal's cells, the
  display lists, the page's layout). Handing one group to an outside
  driver while keeping the rest would take: a `Devices` that sends that
  group's calls, BEGIN, END and STATE to the driver and the rest to the
  compiled-in devices (the bus's code already does the driver's half);
  that code in a crate `terse-host` can depend on (today `terse-bus`
  depends on `terse-host` for composing) and only on Unix (sockets,
  futexes), so Windows keeps the compiled-in devices; and a picture of
  the device: either the driver draws layers (section 7) that the
  renderer takes as images, or, for a driver that draws nothing like the
  example, the host asks its STATE each presented frame (the terminal's
  is about 10 KB) and draws from a mirror of it. Content and sound also
  carry host services and properties (sections 7.2, 9), so the terminal,
  the vector displays and the page are the natural first cases. The
  record would then blame the driver by name, as `terse-bus` does,
  instead of `core`.
