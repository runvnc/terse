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
nothing of what any other device means: every call to a slot other than 1
and 8 goes to the slot's driver, checked first against a **call table** the
driver gives at the handshake. So a device is written once, in any language,
and used by any core that speaks this protocol; a driver can be replaced by
another for some hardware, or left out (an image that lists a device no
driver provides is refused, SPEC 2).

One driver process serves **every machine** of a run (section 1.2): the
shell and each machine it starts are **instances**, named in every
message. So a shell with eight machines and the five reference drivers is
six processes, not forty-five.

Design rules:

1. **One message format**, independent of the transport. Two transports
   carry it: a Unix-domain socket (section 3) and shared memory (section
   4). A core supports both at once; each driver chooses one at the
   handshake.
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
   SPEC 3.1's rules for buffers itself, and for the calls SPEC 14 calls
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
| 9 | `s` | The slots |
| … | 2 + … | The session directory: where a driver creates its layer files (section 7) |
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

- The core refuses a driver that answers for other slots than it asked
  for, or which chooses shared memory when it was not offered.
- **A driver may serve several slots.** It does when the devices share
  state: device 9's `style` and `persistence` are device 3's (SPEC 13.2),
  one section of the state (`STATE.md` 1.7), so `terse-dev-vector` serves
  slots 3 and 9, and a core starts one driver for both. The calls of both
  slots go to the one process, in order (a batch may hold both), so the
  shared setting is never in two places. Each slot keeps its own call
  table, layers and state section.
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
serial among the machines of its system (SPEC 12: the shell, or a single
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
| … | 1 + … | The driver's slots the image lists: a count (u8), then (slot, version) pairs |
| … | 1 + … | The program's identity (SPEC 12.2), if it was started as one: `0`, or `1`, the publisher (32 bytes) and the name (u16 length and bytes) |
| … | `R` | The ROM, so that a driver can read ROM without asking |

The reply is `83h`, then `01h` if the driver took the instance or `00h` if
not, 2 reserved bytes and a reason (a string). The core refuses to start a
machine (SPEC 12.5: `start` fails with "needs a device") that a driver will
not take, or whose image lists a device no driver serves, or a version a
driver does not implement.

An instance's devices start at reset (SPEC 1.3). Each instance has its
own memory file on shared memory, and its own state in the driver; a
driver keeps nothing of one instance in another's. What a driver may share
between instances is what makes no difference to the machines: fonts,
glyph caches, the content device's network node (whose named records are
each program's own, SPEC 9.6, by the identity given in OPEN, else the
image ID).

CLOSE (`04h`) ends an instance (the shell stopped the machine, or the run
ends): the driver drops its state and unmaps its memory file. RESET
(`02h`, reply `82h`) returns an instance's devices to their reset state and
drops their host bookkeeping (pending layouts, requests in flight).

### 1.3 Ordering

A system's frame (SPEC 12.5) runs the shell's frame, then each running
machine's in increasing handle order. For each machine's frame the core
sends BEGIN, the calls, then END, to every driver of that machine; drivers
see instance 0's BEGIN first in every system frame (the content driver
announces feeds updated since the last system frame then, to every
machine, as SPEC 5 says). Each driver sees the messages of all machines in
one order, which is the system's.

### 1.4 CALL and RESULT

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `10h` |
| 1 | 1 | Function (the low byte of `a7`) |
| 2 | 2 | Reserved |
| 4 | 24 | `a0`–`a5` (u32 each) |
| 28 | 8 | Budget (i64): what is left after the `ecall` instruction and the call's 4 (SPEC 14); no reference driver needs it |
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
  device's data ranges, SPEC 9.1) sends READ (`20h`, 3 reserved bytes,
  address u32, length u32). The core answers DATA: `A0h`, then `01h` and
  the bytes if the range lies wholly in ROM or wholly in RAM, else `00h`
  and nothing; then 2 reserved bytes before the bytes. (Shared-memory
  drivers can read the memory file instead.)

### 1.5 BEGIN and INPUTS

At step 1 of each of a machine's frames (SPEC 1.4) the core sends BEGIN to
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
| … | 4 + … | Replay: the driver's recorded input for this frame (u32 length, bytes) |

and the driver replies INPUTS:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `B0h` |
| 1 | 3 | Reserved |
| 4 | 4 | Charge: units subtracted from this frame's budget (SPEC 14, content delivered) |
| 8 | 4 | `e`: events |
| 12 | 8 × `e` | Each: type, `a`, `b`, `c`, position (u8), flags (u8), index (u16) |
| … | 4 + … | Writes: count, then address, length, bytes, as in RESULT; anywhere in RAM |
| … | 4 + … | This frame's input record (u32 length, bytes; section 8) |

**Positions.** The core builds the frame's input from the host's events
and the drivers' events: first every event at position 0 (before the
host's events); then the host's events, each followed by the events at
position 1 whose index is its index; then those at position 2 (after the
input events). Among events of the same place, drivers in increasing order
of their lowest slot, each driver's in the order it gave them. These are
queued as SPEC 5 says (at most 64; one feeds-updated event a frame). Then
the core latches the time, random bytes and controllers, copies every
driver's writes into RAM, and queues the events at position 3
(completions: SPEC 9.2's content-done events, queued after the frame's
input), counting those dropped. Then it runs the frame with 125,000 units
less every driver's charge.

**Flags.** Bit 0: this event and the next one of the same driver are a
pair, queued together or not at all (SPEC 11.5's page located events).

The core may send BEGIN to all of a machine's drivers at once and collect
the replies in any order. A driver may READ before it replies.

### 1.6 END

`31h`, stopped flag (u8: the machine has exited or faulted), 2 reserved,
the frame counter after the frame (u32). No reply. The machine's frame has
ended (SPEC 1.4 step 3); what the device does between frames (lay out a
page, send content requests to the network) it does now, while the core
goes on. Any calls of the frame still in a batch arrive just before END.

### 1.7 STATE and RESTORE

STATE is `40h` alone, for an instance and slot. The reply is `C0h`, 3
reserved bytes, a length (u32) and the slot's section of the state
serialization (`STATE.md` 1.6–1.12: the bytes after the present flag). The
core writes the present flags and puts the sections in `STATE.md`'s order
(slot 3's section for an image that lists only device 9 too: the style
lives there, SPEC 13.2). Defined only between frames.

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
| `TERSE_BUS_SOCKET` | Path of a Unix-domain stream socket the core listens on |
| `TERSE_BUS_CPU` | Optional: a CPU number the driver should run on |

The driver connects to the socket and waits for HELLO. The core accepts
one connection per driver, waits at most 10 seconds (`--start-timeout`)
for it and for the HELLO reply, and refuses the run if either does not
come or the driver exits first; a slow driver is waited for that long, and
the core gives up on it without leaving it running. The core reaps every
driver it started: it sends SHUTDOWN, closes the socket, and kills a
driver that has not exited 2 seconds later.

`terse-bus` starts, by default, `terse-dev-terminal` for slot 2,
`terse-dev-vector` for slots 3 and 9, `terse-dev-content` for 4,
`terse-dev-sound` for 6 and `terse-dev-page` for 7, from its own directory, each only if a machine of
the run may list one of its slots (for a shell, all of them); `--driver
<slots>=<command line>` replaces the driver of those slots (`--driver
2=...`, `--driver 3,9=...`).

---

## 3. Socket transport

Messages are written to the stream one after another, each as section 1
says. That is all: any language that can open a Unix socket can be a
driver.

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
| 72 | 4 | `driver_sleeping`: 1 while the driver waits in the kernel |
| 128 | 4 | `rep_seq`: incremented by the driver after it writes a packet |
| 132 | 4 | `rep_len` |
| 136 | 4 | `core_sleeping` |

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
increments the sequence number with release ordering; if the other side's
sleeping word is 1 it wakes it. The reader spins on the sequence number
(acquire) for a while; if it has not changed, it stores 1 in its sleeping
word, checks the sequence number again, and sleeps in the kernel until the
number changes (or a timeout passes, after which it checks that the other
end is alive: the socket is not closed), then stores 0.

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

**What is OS-specific.** The files live in `/dev/shm` (any tmpfs; the
reference core falls back to the system's temporary directory) and are
mapped with `mmap(MAP_SHARED)`. The sleep and wake are Linux futexes on
the sequence words (`FUTEX_WAIT` with a timeout, `FUTEX_WAKE`, not
private, because the words are shared between processes). Elsewhere they
would be `WaitOnAddress`/`WakeByAddressAll` (Windows),
`__ulock_wait`/`os_sync_wait_on_address` (macOS), or a pipe written after
the sequence number (anywhere, at the cost of the system calls the
transport is meant to avoid).

---

## 5. The call table

Each entry is 64 bytes and describes one function of the device. A
function not in the table is an unknown call: the core returns
`FFFF_FFFFh` and charges 4, without the driver (SPEC 3.1).

The table also gives each call's class (SPEC 14). A call with flag bit 1
is **deferred**: its result and charge follow from its arguments and its
buffer's length by the entry's rules (section 6), so the core can compute
them; its base charge is 4. Every other call is **answered**: only the
driver knows its result or charge, and its base charge is 64, which the
core charges, also when one of its checks fails; the driver's charge is
on top. So "deferred" is exactly "computable by the core from the table",
and a driver that marks a call batchable states that its class is
deferred. The System device's calls and device 8's are the machine's own
(SPEC 14: core, 4) and never reach a driver.

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

Fields an entry does not use are zero, but argument numbers, which are
`FFh` ("none"); a call that is not batchable has rule 0 and charge zero.

**The core's checks.** For a call with a buffer, in this order:

1. The length is the length argument × the scale (computed without
   overflow), or the fixed length. Outside least … greatest: the result is
   the length result.
2. A buffer of length 0 is valid at any address. Otherwise it must lie
   wholly in RAM, or (if bit 2 and the device does not write it) wholly in
   ROM or wholly in RAM: if not, the place result.
3. The checked argument, if any, must be at most its greatest value: if
   not, the place result (SPEC 3.1: a bad buffer and a bad argument give
   the same `FFFF_FFFEh` in every call that has both).

A call that fails a check is charged its base charge (4, or 64 for an
answered call) and does not reach the driver (SPEC 14: nothing is copied).
This is the whole of SPEC 3.1's rules, including its five exceptions,
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
style: SPEC 13.2); all deferred.

Content (slot 4): `submit` (`a0`, fixed 52 bytes, read and written, RAM
only; the place result is 0); answered.

Page (slot 7): `submit` (`a0`, `a1` × 1, 1 … 65,536, read, ROM; length
result 1, place result `FFFF_FFFEh`; deferred: 0, 1 per 4 bytes),
`clear`, `region`, `scroll`, `background` (deferred, 0), `locate`
(answered).

Sound (slot 6, SPEC 18): `submit` (`a0`, `a1` × 1, 1 … 4,096, read, ROM;
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
state). These are SPEC 14's classes exactly.

`write` takes its whole buffer, at most 4,096 bytes, and costs 2 per byte
however many scrolls it causes (SPEC 8.2), so its entry says "`a0` = `a1`;
2 per byte" and a frame of one-byte writes goes to the driver as a few
batches instead of one round trip a call. A batched call with a buffer
carries a copy of it: on shared memory a big one costs more batched than
sent (the vector display's 16 KiB list).

## 7. Layers and presentation

A visual driver declares the **layers** of each of its slots at the
handshake: an ID (its own name for it) and a z, the place in the stack,
from the bottom. SPEC 10.5's order gives:

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
8–15, blue 0–7 (sRGB values, as the reference renderer blends them). The
core maps it read-only. A driver gives a new file when the picture size
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
- a machine in a tile (SPEC 12.3): a picture the size of its rectangle,
  glyph cells `⌊width / 80⌋`;
- a machine on a panel of the shell's scene: a picture as large as the
  panel looks (whole cells of 80 columns, kept while the panel moves a
  little), glyph cells of that width.

The picture is then put together as terse-host's software renderer puts it
together, and with its code (terse-host's `soft::scene` and `soft::panel`,
used by both): the shell's screen; with Screens in effect (SPEC 12.3), the
shell's **scene**: its screen with its Surfaces' sources blacked out, the
panels far to near (each machine's picture, from a pyramid of it; a
Surface from the shell's own screen), then the shell's 3D layer over them;
each machine shown flat in its rectangle, in increasing handle order (a
machine with Surfaces of its own as its own scene, SPEC 13.5); the
machine a finished `face` shows flat in the view's rectangle; the fault
line (below); the host's consent prompt over everything.

**Panels are the core's.** A Screen shows another machine, and a Surface
shows a part of the machine's own picture: the pixels are the core's, put
together from several drivers' layers (and, for a Screen, of another
instance). So the core draws the panels, from geometry it projects itself
from device 9's list (the `vector3.list` property: terse-core's
projection, the one SPEC 12.4's pointer routing over panels already
needs), and the 3D driver draws only lines, into its layer (z 35), which
the core composes among the others when no scene is in effect and over
the panels when one is. The alternative, handing the 3D driver the other
layers' pixels to draw panels with, would make one driver read every
other driver's output and every other machine's, and draw a picture that
is not its device's; this way each driver draws only its device, and what
spans machines (tiles, panels, the prompt) is the compositor's.

The 3D driver needs two facts to draw: the eye (`core.eye`: where the
window's pointer puts it, `--eye` headless; SPEC 13.1) and, for the shell,
the view of its scene (`core.view`: which Screens are in effect and the
`face` motion, SPEC 12.2), which the core publishes (section 7.2): its
lines are then cut by the panels' faces and move with the view.

**The fault line** is the terminal's: when a machine faults (SPEC 1.5)
the core publishes `core.status`, "fault: …", and the terminal shows it
over its bottom row as terse-host does (colours 15 on 1).

**The consent prompt** (SPEC 9.5) is the core's: the content driver
publishes the question it waits on (`content.prompt`: the asking machine,
the request, what is asked, how many wait); the core draws it over
everything, with terse-host's grid renderer in the overlay's colours, takes
the keyboard while it is shown (Y allows, N or Esc refuses; no key or text
reaches a machine, nor the release of a key pressed then) and gives the
answer back as `core.answer`.

Only the clean style is composed this way. The phosphor style (SPEC 10.6)
is a whole-screen effect across the layers: the light of the terminal's
characters, the page's text and the shapes adds up on one tube with one
afterglow. It would need the drivers to give their light (linear, additive)
instead of covering layers, and the core to own the tube and its
persistence (or one driver to own it and the others to give it their
primitives); the reference core leaves it out.

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
| `vector.outlines` | vector display | page | 1 if figures and filled shapes are drawn as outlines (fill mode 1, or the phosphor style: SPEC 11.4), else 0 |
| `vector3.list` | vector display | core | device 9's list (SPEC 13), which device 8 (`face`), the shell's scene and the pointer over panels (SPEC 12.2–12.4) look at |
| `core.eye` | core | vector display | the eye: two f32, screen units from the reference eye (SPEC 13.1) |
| `core.view` | core | vector display | the shell's scene: which Screens are in effect (u8) and the view (12 f64: the linear part by rows, then the translation); empty: none |
| `core.status` | core | terminal | the line shown over the bottom row (UTF-8): a fault; empty: none |
| `content.prompt` | content | core | the consent question waiting: the asking machine (u32), the request's handle (u8), how many wait (u32), what is asked (kind and fields); empty: none |
| `core.answer` | core | content | the answer: the question's machine (u32) and handle (u8), then 1 allow or 0 refuse |
| `sound.frame` | sound | core | the frame counter after the frame the driver rendered last (u32), then its 800 sample pairs (i16, left and right interleaved; SPEC 18.7); empty before the first |

`vector3.list` is read in the middle of a frame when device 8's `face`
needs it: the core sends GET, after the batch, as it would a call.

**Sound.** The sound driver renders a machine's samples at END (SPEC
18.5: the end of the frame) and plays nothing itself: the core reads
`sound.frame` after the machine's frame (GET, in front of which the END
goes on shared memory), mixes the machines that ran in the system frame
(SPEC 18.7) and plays or writes the result. The frame counter in the value
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
machine): `TBRC`, version 0, the image ID, then per frame: the frame
counter, the Unix time, the 32 controller bytes, the host's events (count
and events), the random bytes taken (count and bytes), the drivers' records
(count; then the driver's lowest slot, length, bytes for each) and the
state hash. It is not the format of `STATE.md` 5, whose completions are a
content device's: a core that knows nothing of content cannot write that
one.

## 9. Host services

A shell's host does more than run machines: it fetches the images the
shell starts (SPEC 12.2 `start`, `start_program`), and stores its
snapshots (SPEC 12.7). A core that knows nothing of the network or the
store asks a driver that does: a driver names the **services** it gives at
the handshake, and the core calls them with SERVICE, for the instance on
whose behalf it asks (a snapshot's owner is the shell's program, SPEC
12.7). The reference content driver, which holds the run's network node,
gives:

| Service | Request | Answer |
|---|---|---|
| `image.fetch` | the machine (u32), the image ID | `1` and the image's bytes, `2` not found, `3` failed |
| `program.fetch` | the machine, the program (publisher, name as a string), the built-ins (count, IDs) | as `image.fetch`: the current version the host holds |
| `snapshot.save` | the slot (u8), the snapshot file (STATE.md 4) | `1` stored, `0` not |
| `snapshot.load` | the machine (u32), the slot | `1`, the image file and the snapshot file (blobs); or `0`, the reason (SPEC 12.2's failure code), whether its image ID follows and it, the message |
| `snapshot.delete` | the slot | nothing |
| `snapshot.table` | nothing | the 16 slots' `snapshot_info` records (SPEC 12.2) |

The core calls them after the system frame in which the shell asked, and
the answers take effect at the start of the next (SPEC 12.5), as the
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
`FFFF_FFFFh`): a device's absence is a property of the image (SPEC 2), and
a program that has used a device would see it vanish. And a machine fault
(SPEC 1.5) is the program's, recorded in the state with a kind SPEC
defines; a driver's failure is the host's. So:

- **The run stops.** The core stops running frames, kills every driver it
  started (none is left behind), removes the session's files, prints which
  driver failed, in which frame, and why, and exits with status 3 (a
  window shows the last picture until it is closed, with the reason in its
  title and on standard error). With one driver process per device for all
  machines, a failed driver is a failed device of every machine, so this
  is also what a per-machine rule would come to.
- Nothing is written to a machine's state: hashes up to the failure are
  those of the run so far.

A SPEC change would be needed for anything finer: a stop reason "the host
lost a device" (neither exit nor fault) that a shell could show for a
machine whose own driver failed, if drivers were ever per machine.

`terse-bus` also ends cleanly on SIGINT, SIGTERM and SIGHUP and when its
window is closed: it finishes the frame, sends SHUTDOWN to every driver,
reaps them (killing one that does not exit within 2 s) and removes the
session directory.

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

## 12. Open questions

- **The phosphor style** (section 7.1).
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
