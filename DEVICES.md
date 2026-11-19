# Terse device bus — v0 (draft)

Status: draft, written with the prototype in `crates/terse-bus` (the core
host and the protocol) and `crates/terse-devices` (the drivers). Nothing in
`SPEC.md` depends on this document: it describes one way of building a
machine host, not the machine. A host that compiles its devices in, as
`terse run` and `terse-host` do, is just as conforming. In this
repository both are one machine: terse-core's `Machine` runs the frame
loop and the System and Machines devices over a `Devices` trait, whose
implementations are the devices compiled in (`terse run`, `terse-host`, a
system's machines) and the bus (`terse-bus`).

A **device driver** is a separate program that implements one device of
`SPEC.md` (one slot) and speaks the protocol below to a **core host**. The
core runs the machine: the CPU, ROM and RAM, the budget and the frame loop,
the System device (slot 1), the event queue, and the composition of the
screen. It knows nothing of what any other device means: every call to
slots 2–255 goes to the slot's driver, checked first against a **call
table** the driver gives at the handshake. So a device is written once, in
any language, and used by any core that speaks this protocol; a driver can
be replaced by another for some hardware, or left out (an image that lists
a device no driver provides is refused, SPEC 2).

Design rules:

1. **One message format**, independent of the transport. Two transports
   carry it: a Unix-domain socket (section 3) and shared memory (section
   4). A core supports both at once; each driver chooses one at the
   handshake.
2. **The machine's semantics are kept exactly.** A run with drivers gives
   the same state hash after every frame as the same run with the devices
   built in (`STATE.md`). Charges, results and the order of events are the
   drivers' to compute; the core applies them.
3. **Calls are synchronous; inputs arrive at frame boundaries.** A call's
   reply carries its result, its charge and the bytes it writes, and
   nothing else. A driver delivers events, content completions and
   anything else SPEC lists as an input when the core begins a frame.
4. **The core can check what it cannot interpret.** The call table says
   where each call's buffer is and how long it is, so the core applies
   SPEC 3.1's rules for buffers itself, and for the calls SPEC 14 calls
   deferred, whose result and charge need no device state, it computes
   the result and the charge itself and sends the call later, in a batch
   (section 6).

All integers are little-endian. `h` suffix means hexadecimal. "Core" and
"driver" name the two ends.

---

## 1. Messages

Every message is a 4-byte length `n` (u32: the length of the body, not
counting these 4 bytes; at most 16,777,216) followed by `n` bytes of body.
The body's first byte is the message's type.

| Type | Name | From | Reply |
|---|---|---|---|
| `01h` | HELLO | core | `81h` HELLO |
| `02h` | RESET | core | `82h` RESET |
| `10h` | CALL | core | `90h` RESULT |
| `11h` | BATCH | core | none |
| `20h` | READ | driver, while it owes a reply | `A0h` DATA |
| `30h` | BEGIN | core | `B0h` INPUTS |
| `31h` | END | core | none |
| `40h` | STATE | core | `C0h` STATE |
| `41h` | RESTORE | core | `C1h` RESTORE |
| `50h` | PRESENT | core | `D0h` PRESENT |
| `60h` | SHUTDOWN | core | none |
| `7Fh` | ERROR | either | none |

- The core sends one request at a time and waits for its reply before it
  sends another. BATCH, END and SHUTDOWN have no reply: on the socket they
  are written at once; on shared memory they wait and go in front of the
  next request, in the same packet (section 4.2), since nothing tells the
  core when the driver has read them. Between a request
  and its reply the driver may send READ messages and must then wait for
  each one's DATA. Nothing else is ever in flight.
- A driver that receives a message it cannot handle replies ERROR (UTF-8
  text after the type byte) and the core stops the machine with that
  message. A core may also send ERROR before it closes the connection.
- Reserved bytes are written as zero and ignored when read.
- Strings (paths, names) are UTF-8, given as a u16 length then the bytes.

### 1.1 HELLO and the handshake

The core starts the driver (section 2) and sends HELLO:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `01h` |
| 1 | 1 | Protocol version: `0` |
| 2 | 1 | Slot |
| 3 | 1 | The device version the image lists for the slot |
| 4 | 4 | Flags: bit 0 shared memory is offered (the paths below are valid); bit 1 the core presents (visual drivers will be asked for layers, section 7); bit 2 replay (the run's inputs come from a recording, section 8: the driver must not use its live source, for example the network) |
| 8 | 32 | Image ID |
| 40 | 4 | `R`: ROM length |
| 44 | 4 | `L`: RAM length |
| 48 | 2 + … | The memory file's path (section 4.1); empty if not offered |
| … | 2 + … | The driver's bus file's path (section 4.2); empty if not offered |
| … | 2 + … | The session directory: where a driver creates its layer files (section 7) |
| … | `R` | The ROM, so that a driver can read ROM without asking |

The driver answers HELLO (`81h`):

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `81h` |
| 1 | 1 | Protocol version: `0` |
| 2 | 1 | Slot |
| 3 | 1 | The device version it implements |
| 4 | 1 | Transport chosen: `0` the socket, `1` shared memory |
| 5 | 2 + … | The driver's name (for messages and measurements) |
| … | 2 | Native width in pixels (0: no preference) |
| … | 2 | Native height in pixels |
| … | 1 | `k`: number of layers (section 7) |
| … | 2 × `k` | Each: layer ID, z (u8 each) |
| … | 1 | `m`: number of call table entries |
| … | 64 × `m` | The call table (section 5) |

- The core refuses a driver whose slot or version differs from what it
  asked, or which chooses shared memory when it was not offered.
- After the reply, every message travels on the chosen transport. A driver
  that chose shared memory keeps the socket open and sends nothing on it:
  it is how each end learns that the other has gone.
- The native size is a picture size at which the driver's drawing is
  sharpest (the terminal's: its font's cell times 80 by the 4:3 shape).
  The core uses the first one given, in slot order, as the size of a
  picture when none is asked for, and fits the 640 × 480 screen into any
  picture as the largest whole multiple of it that fits, else scaled down
  (section 7).

### 1.2 RESET

`02h` and nothing else: the device returns to its reset state (SPEC 1.3)
and drops its host bookkeeping (pending layouts, requests in flight). The
reply is `82h`. A core sends it to reuse drivers for a new run of the same
image.

### 1.3 CALL and RESULT

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `10h` |
| 1 | 1 | Function (the low byte of `a7`) |
| 2 | 2 | Reserved |
| 4 | 24 | `a0`–`a5` (u32 each) |
| 28 | 8 | Budget (i64): what is left after the `ecall` instruction and the call's 4 (SPEC 14); no reference driver needs it |
| 36 | 4 | `n`: the bytes of the call's read buffer that follow |
| 40 | `n` | Those bytes, from the buffer's start |

On the socket transport the core sends the buffer the device reads (the
whole buffer). On shared memory it sends none (`n` = 0) and the driver
reads guest memory itself, except in a batch (section 6).

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
  write buffer (section 5); otherwise the core stops the machine with an
  error (a driver bug, not a fault of the program).
- On shared memory a driver may instead write its buffer in place, in the
  memory file, before it replies.
- A driver that needs other guest memory during the call (the content
  device's data ranges, SPEC 9.1) sends READ (`20h`, 3 reserved bytes,
  address u32, length u32). The core answers DATA: `A0h`, then `01h` and
  the bytes if the range lies wholly in ROM or wholly in RAM, else `00h`
  and nothing; then 2 reserved bytes before the bytes. (Shared-memory
  drivers can read the memory file instead.)

### 1.4 BEGIN and INPUTS

At step 1 of each frame (SPEC 1.4) the core sends BEGIN to every driver:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `30h` |
| 1 | 1 | Flags: bit 0 a recorded input follows (replay) |
| 2 | 2 | Reserved |
| 4 | 4 | The frame counter of the frame that begins |
| 8 | 4 | The Unix time latched for it |
| 12 | 4 | `e`: the host's input events for this frame (keys, text, pointer, wheel, …) |
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
input events). Among events of the same place, drivers in increasing slot
order, each driver's in the order it gave them. These are queued as SPEC 5
says (at most 64; one feeds-updated event a frame). Then the core latches
the time, random bytes and controllers, copies every driver's writes into
RAM, and queues the events at position 3 (completions: SPEC 9.2's
content-done events, queued after the frame's input), counting those
dropped. Then it runs the frame with 125,000 units less every driver's
charge.

**Flags.** Bit 0: this event and the next one of the same driver are a
pair, queued together or not at all (SPEC 11.5's page located events).

The core may send BEGIN to all drivers at once and collect the replies in
any order. A driver may READ before it replies.

### 1.5 END

`31h`, stopped flag (u8: the machine has exited or faulted), 2 reserved,
the frame counter after the frame (u32). No reply. The frame has ended
(SPEC 1.4 step 3); what the device does between frames (lay out a page,
send content requests to the network) it does now, while the core goes on
(it hashes the state, composes the screen, waits for the next frame). Any
calls of the frame still in a batch arrive just before END.

### 1.6 STATE and RESTORE

STATE is `40h` alone. The reply is `C0h`, 3 reserved bytes, a length (u32)
and the device's section of the state serialization (`STATE.md` 1.6–1.9:
the bytes after the present flag; for a device with no section there, its
own bytes, which the core leaves out of the hash). The core writes the
present flag and puts the sections in `STATE.md`'s order. Defined only
between frames.

RESTORE is `41h`, 3 reserved bytes, a length (u32) and such a section; the
driver replies `C1h`, then `01h` if it took it or `00h` if it was not a
valid section (then nothing changed), and 2 reserved bytes. Host
bookkeeping restarts as `STATE.md` 4 says (content requests that were
pending fail in the next frame; a document is laid out as new).

### 1.7 PRESENT

Section 7.

### 1.8 SHUTDOWN

`60h` alone: the driver releases what it holds (its layer files) and exits.
A driver also exits when its socket is closed.

---

## 2. Starting a driver

The core starts the driver's command line with these environment
variables:

| Variable | Value |
|---|---|
| `TERSE_BUS_SOCKET` | Path of a Unix-domain stream socket the core listens on |
| `TERSE_BUS_SLOT` | The slot, in decimal |
| `TERSE_BUS_CPU` | Optional: a CPU number the driver should run on |

The driver connects to the socket and waits for HELLO. The core accepts
one connection per driver, waits at most 10 seconds for it and for the
HELLO reply, and refuses the run if either does not come. The core reaps
every driver it started: it sends SHUTDOWN, closes the socket, and kills
a driver that has not exited 2 seconds later.

`terse-bus` starts, by default, `terse-dev-terminal` for slot 2,
`terse-dev-vector` for 3, `terse-dev-content` for 4 and `terse-dev-page`
for 7, from its own directory; `--driver <slot>=<command line>` replaces
one.

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

### 4.1 The memory file

One file for the machine, shared by every driver that chose shared memory:
the ROM at offset 0 (its `R` bytes; the rest of the first 131,072 bytes is
zero) and the RAM at offset 131,072 (`L` bytes). The core's CPU runs on
this mapping, so it is the guest's memory itself, not a copy. A driver maps
it shared, for reading and writing. It reads a call's buffers here, writes
its write buffer here (or returns writes), and must not touch it while the
core is not waiting for one of its replies.

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
The core writes a request packet: in practice a BATCH (if any) then one
request. The driver handles them in order and writes its reply (or a
READ) as a packet of one message.

All control words are aligned 32-bit words, written and read only as
single atomic operations. Each side starts from the sequence numbers it
finds when it maps the file; a driver maps it (and reads `req_seq`)
before it sends its HELLO reply, since the core may send its first
request as soon as it has the reply. (The prototype's Rust drivers mapped
it after replying, and could miss the first BEGIN and wait for ever.)

**Hand-over.** The writer writes the packet, then its length, then
increments the sequence number with release ordering; if the other side's
sleeping word is 1 it wakes it. The reader spins on the sequence number
(acquire) for a while; if it has not changed, it stores 1 in its sleeping
word, checks the sequence number again, and sleeps in the kernel until the
number changes (or a timeout passes, after which it checks that the other
end is alive: the socket is not closed), then stores 0.

**What is OS-specific.** The files live in `/dev/shm` (any tmpfs; the
prototype falls back to the system's temporary directory) and are mapped
with `mmap(MAP_SHARED)`. The sleep and wake are Linux futexes on the
sequence words (`FUTEX_WAIT` with a timeout, `FUTEX_WAKE`, not private,
because the words are shared between processes). Elsewhere they would be
`WaitOnAddress`/`WakeByAddressAll` (Windows), `__ulock_wait`/`os_sync_wait_on_address`
(macOS), or a pipe written after the sequence number (anywhere, at the
cost of the system calls the transport is meant to avoid). The spin times
are the prototype's: the core spins up to 100 µs for a reply; a driver up
to 2 ms for the next request after it replied, so the calls of a busy
frame need no system call at all, while a driver idle between frames
sleeps (`--spin-us`, `TERSE_BUS_SPIN_US`; 0 sleeps at once). The spinning is
where the speed comes from: with no spinning, a futex hand-over costs about
what the socket does (measured: 4.1 µs against 4.7 µs a round trip), and
the spinning costs a CPU: about 12 % of one per driver at 60 frames a
second with the 2 ms default, against 1 % without.

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
answered call) and does not reach the driver (SPEC 14: nothing is copied). This is the whole of SPEC 3.1's rules,
including its four exceptions, which are only different results in the
table.

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

Content (slot 4): `submit` (`a0`, fixed 52 bytes, read and written, RAM
only; the place result is 0); answered.

Page (slot 7): `submit` (`a0`, `a1` × 1, 1 … 65,536, read, ROM; length
result 1, place result `FFFF_FFFEh`; deferred: 0, 1 per 4 bytes),
`clear`, `region`, `scroll`, `background` (deferred, 0), `locate`
(answered).

## 6. Batching

A deferred call (entry flag bit 1) changes the device's state but returns
nothing that depends on it: its result and charge follow from its
arguments and its buffer's length by the entry's rules. The core does not
wait for it. After its checks it computes the result and the charge,
applies them, and appends the call to the driver's **batch**: a CALL
message with the buffer's bytes inline (on both transports: the program
may change the buffer before the batch is sent). The batch is sent as one
BATCH message:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `11h` |
| 1 | 3 | Reserved |
| 4 | 4 | Count |
| 8 | … | The CALL messages, each framed (length, body) |

before the driver's next request of any kind (a call that needs a reply,
END, STATE, PRESENT), in the same packet. The driver carries the calls out
in order and replies to none of them. A driver must give the same result
and charge as the rule says; the prototype's drivers check this in debug
builds.

A call that writes guest memory, reads device state, or whose charge or
result depends on the device's state cannot be batched: it is answered.

## 7. Layers and presentation

A visual driver declares its **layers** at the handshake: an ID (its own
name for it) and a z, the place in the stack, from the bottom. SPEC 10.5's
order gives:

| z | Layer |
|---|---|
| 10 | Terminal: cell backgrounds |
| 20 | Page: background, text, figures |
| 30 | 2D vector display |
| 35 | 3D vector display |
| 40 | Terminal: characters and cursor |

So one driver may have several layers, above and below other drivers'.
The core composes the layers in increasing z (equal z: increasing slot,
then the order declared) over opaque black.

When it presents (HELLO flag bit 1), the core sends PRESENT to every
driver, in increasing slot order: a driver with layers brings them up to
date for the picture, and any driver may report properties (a driver with
no layers replies with none):

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `50h` |
| 1 | 3 | Reserved |
| 4 | 2 | Picture width in pixels |
| 6 | 2 | Picture height |
| 8 | 16 | The 640 × 480 screen in the picture: x, y, width, height (f32 each) |
| 24 | 4 | `p`: properties |
| 28 | 8 × `p` | Each: slot (u8), key (u8), reserved (2), value (u32) |

The properties are those that the drivers before it in this round
reported (below): presentation facts one device's drawing needs from
another, which the core passes on without reading them.

The driver replies `D0h`:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `D0h` |
| 1 | 1 | `k`: layers |
| 2 | 2 | Reserved |
| … | | `k` times: layer ID (u8), flags (u8: bit 0 a new buffer file, whose path follows), path (u16 length and bytes, if bit 0), damage: count (u32) and that many rectangles, x0, y0, x1, y1 (i32 each, half open, in picture pixels) |
| … | 4 | `p`: properties |
| … | 8 × `p` | Each: key (u8), reserved (3), value (u32) |

A **layer file** is the driver's: a file it creates in the session
directory, picture width × height × 4 bytes, one u32 a pixel, rows from
the top, **premultiplied** colour: alpha in bits 24–31, red 16–23, green
8–15, blue 0–7 (sRGB values, as the reference renderer blends them). The
core maps it read-only. A driver gives a new file when the picture size
changes, and its first damage then covers the whole picture. The damage is
what changed in the file since the driver's last PRESENT; the core
composes only there.

Properties of the reference drivers: the vector display (slot 3) reports
key 1, its figures-as-outlines flag (fill mode 1, or the phosphor style),
which the page (slot 7) needs for its figures (SPEC 11.4).

Only the clean style is composed this way. The phosphor style (SPEC 10.6)
is a whole-screen effect across the layers: the light of the terminal's
characters, the page's text and the shapes adds up on one tube with one
afterglow. It would need the drivers to give their light (linear, additive)
instead of covering layers, and the core to own the tube and its
persistence (or one driver to own it and the others to give it their
primitives); the prototype leaves it out.

### 6.1 What the reference devices allow

Deferred, so batchable: terminal `write`, `command`, `set_cursor`,
`set_colours`, `cells_write`, `palette_set`, `size`; every call of the
vector display; page `submit`, `clear`, `region`, `scroll`, `background`.
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

Replay needs the driver to accept its record: a driver's record is only
as good as the driver's own definition of its inputs. The page driver's
record is its events, because what the layout means for the machine
reaches it only as events (SPEC 11.5); the content driver's must also
update the device's state (which handles are free), so it records
completions, not their effects.

The bus recording file is the core's own (`terse-bus --record`): `TBRC`,
version 0, the image ID, then per frame: the frame counter, the Unix time,
the 32 controller bytes, the host's events (count and events), the random
bytes taken (count and bytes), the drivers' records (count; then slot,
length, bytes for each) and the state hash. It is not the format of
`STATE.md` 5, whose completions are a content device's: a core that knows
nothing of content cannot write that one.

## 9. Measurements

`programs/hostile` spends every frame's budget on the cheapest calls of
one kind, 60 frames a phase; `terse-bus --bench --frame-times` gives each
frame's emulation time, the bus included. On an i7-10750H, release build,
the Rust drivers, batching on (default spin times); milliseconds per
frame, median / worst of each phase:

| Phase (calls per frame) | In-process | Shared memory | Socket |
|---|---|---|---|
| terminal `get_cursor`, answered (1,923) | 0.05 / 0.05 | 0.71 / 0.83 | 9.98 / 11.20 |
| vector `clear`, deferred (25,000) | 0.58 / 0.61 | 2.06 / 3.11 | 2.02 / 2.34 |
| page `locate`, answered (1,923) | 0.05 / 0.05 | 0.73 / 0.83 | 8.13 / 10.78 |
| terminal `write` of 1 byte, deferred (12,500) | 0.56 / 0.56 | 1.42 / 2.01 | 1.39 / 1.63 |
| terminal `cells_read` of 2,400 cells, answered (51) | 0.02 / 0.02 | 0.05 / 0.06 | 0.50 / 0.56 |
| vector `submit` of 16,384 bytes, deferred (30) | 0.01 / 0.01 | 0.15 / 0.18 | 0.11 / 0.28 |
| terminal `write` of 4,096 line feeds, deferred (15; 61,000 scrolls) | 0.35 / 0.39 | 0.41 / 0.44 | 0.39 / 0.41 |

Every phase fits a 16.7 ms frame on every transport. Before answered calls
cost 64 and `write` took its whole buffer (every call cost 4, and `write`
was answered), the same program on the socket took 118 ms a frame for `get_cursor` and `locate`
and 58 ms for one-byte writes (each a round trip of about 5 µs), and 9 ms
on shared memory. Without batching the deferred calls are round trips
again: on the socket 120 ms a frame for `clear` and 64 ms for one-byte
writes (shared memory: 9 and 5 ms), so a core that offers the socket
should batch. The C terminal driver gives the same figures within 10 %
(1.9 ms for `cells_read`, which it copies byte by byte); the Python vector
driver needs 21 ms a frame to carry out 25,000 batched `clear` calls, its
own speed rather than the protocol's.

## 10. Open questions

- **Devices that share state.** SPEC 13.2 has device 9's `style` and
  `persistence` set device 3's (one section of the state, `STATE.md` 1.7).
  Under the bus two drivers cannot share it: either one driver provides both
  slots (a "device group" in HELLO), or the shared setting moves to one
  device. The prototype has no device 9.
- **Devices that depend on another's state for their look.** The page's
  figures are outlines when device 3 is in the phosphor style or fill mode
  1 (SPEC 11.4). The prototype passes this as a presentation property from
  the vector driver to the page driver through the core.
- **The phosphor style** (section 7).
- **Trust.** A driver on shared memory can write any of RAM at any time;
  the core trusts its drivers as it trusts its own code. A core that does
  not would offer only the socket, check every write, and pay for it.
- **System device.** The core keeps slot 1: `wait_frame` and `exit` are the
  frame loop, `next_event` is the queue every device feeds, and the time,
  random bytes and controllers are host inputs the core records. It could
  be a driver only by moving the frame loop with it.
