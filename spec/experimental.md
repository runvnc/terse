# §experimental Experimental devices

Part of the Terse machine specification, v0 (draft): `SPEC.md` is the core
machine and says how these documents fit together. This one says how a
device that nobody has specified yet can be tried out on Terse32 without
a SPEC change, and without making any machine that does not use it
different in any way.

**Builds on** `SPEC.md`: the image header and optional devices (§image),
`device` (§system), the call convention (§calls.convention), charges
(§charges), profiles (§profiles), conformance (§conformance) and the
Machines device (§machines: a shell and the machines it starts);
`STATE.md` 1.13 (the experimental sections of the state); the device bus,
`DEVICES.md` 15 (how a driver declares such a device), 2.2 (installing
one) and 10 (a driver that fails).

**Why.** A numbered device (2 Terminal, 6 Sound, …) is specified exactly
by a document, and every core knows where its state goes. Writing that
document first is a lot to ask of someone who only wants to see whether
an idea works. An **experimental device** is the idea with the document
not written yet: a driver says what its calls are, what they cost and
how big its state is, and the core checks and charges the calls and
hashes the state without knowing what the device means. It runs only on
the device bus, only where the user has installed its driver, and only
for programs that can do without it.

**What it is not.** It is not a way to extend a numbered device, to add
an event type or a picture, or to reach anything outside the machine (the
network, files, the clock). It is not covered by §conformance. No profile
includes one, and nothing about one is ever fetched from the network.

## §experimental.slots Slots and names

Slots **240 to 255** (`F0h`–`FFh`) are the **experimental slots**. They
are never given to a numbered device. Every other slot keeps its meaning
(§image's table: 1–10 devices, 5 reserved, 0 and 11–239 no device in v0).
An image has at most 16 device entries, so 16 experimental slots are as
many as one image can list; the calls of slot `s` are `a7` = `s × 256 +
function`, `F000h` to `FFFFh`, all below the `FFFFh` limit of
§calls.convention.

A slot number cannot tell two unrelated experiments apart: two people
will both pick 240. So an experimental device is identified by a
**name**, which the image states and the driver announces (`DEVICES.md`
15.1), and the slot is only the handle by which this one image's
program calls it. Another image may call the same device on another
slot, and the two may run at once in one shell (§experimental.grants).

A **name** is 1 to 30 bytes of `a`–`z`, `0`–`9`, `-` and `.`, starting
with a letter. By convention it is `<who>.<what>`, the author's own word
first (`example.dice`, `ada.plotter`), which is what keeps two
experiments apart; nothing registers names. The device also has a
**version**, 0 to 127, the low seven bits of its entry's version byte as
for any device; a driver that changes what a call does, its charge or its
state's layout gives it a new version.

## §experimental.image The image

An experimental device is listed in the header like any device (§image):
`(slot, version byte)`. It must be **optional**: the version byte's top
bit is set. Its name is in the **experimental device table**, the last
bytes of the ROM:

| ROM offset | Size | Field |
|---|---|---|
| `rom_len` − 4 − 32k | 32 × k | k entries, each: the slot (1), the name's length n (1, 1–30), the name (n), then `00h` up to 32 bytes |
| `rom_len` − 4 | 1 | k: the number of entries, 1 to 16 |
| `rom_len` − 3 | 3 | `54h 58h 44h` (`TXD`) |

The table is ROM: it is covered by the image ID, it is loaded at
`0001_0000h` with the rest of the ROM, and a program may read it. A host
that knows nothing of experimental devices parses the header exactly as
before and runs the image with the device absent: for it an optional
entry of slot 240–255 is "a slot that is no device", never granted
(§image), and the table is just the end of the ROM. So nothing is added
to the header and no existing host's parsing changes.

The table is **valid** if the ROM is at least 4 + 32k bytes long, ends
with the three bytes `TXD`, k is the number of experimental slots the
header lists, the entries' slots are exactly those slots (each once, in
any order), every name is a valid name, no two names are the same, and
every padding byte is `00h`. A host looks for the table only when the
header lists an experimental slot; with the table invalid or missing it
grants none of the image's experimental devices.

**Refusals.** An image that lists an experimental slot as required is
refused by every host (it is a required device the host does not
provide: §image; a host that has the driver refuses it too, so that no
program can depend on an experiment). `terse pack` and `terse build`
write the table and refuse to make such an image, or one with an
invalid table: an experimental slot is given only with `--optional
<slot>:<version>=<name>`, for example `--optional 240:0=example.dice`.

## §experimental.grants Grants and `device`

An experimental device is **granted** to a machine, at reset, exactly
when the host runs it on a device bus, the image's table is valid, and
the user has given the host a driver for that name in that version: one
installed in the drivers folder (`DEVICES.md` 2.2) or named for the run
(`terse-bus --driver <name>=<command line>`). Otherwise it is not granted,
and the program runs without it, as for any optional device (§image):
`device` returns `FFFF_FFFFh` for its slot and every call to the slot
returns `FFFF_FFFFh` at a charge of 4. The rule is the same for a machine
run alone and for every machine of a system: the shell itself, and each
machine it starts or restores (§machines).

Like any grant, it is fixed at reset, is one byte of the state's grants
(`STATE.md` 1.4: there for an experimental entry whether granted or not,
exactly as on a host that has never heard of experimental devices), and
is part of snapshots and recordings.

For a granted experimental device, `device` returns its version (the
entry's low seven bits).

**Where they are granted.** Only on a core with a device bus:

- `terse run`, `terse-host` and `terse-bus --in-process` (the devices
  compiled in) never grant one: they have no drivers. They need no code
  for it: an experimental slot is no device of theirs. A shell there
  starts such a program as any other, and it runs without the device.
- `terse-bus` with one machine grants it as above, starting a driver for
  each experimental device the image lists.
- `terse-bus --shell` grants it as above to the shell and to every
  machine the shell starts. The run starts every experimental driver it
  has (each `--driver <name>=...` and each installed one, at most 16) when
  it starts, since the set of drivers is fixed then and the machines are
  not known (`DEVICES.md` 15.1); a machine is granted a device when one of
  them serves its name in the version its image lists.
- `native/core-c`'s `tcore` does not grant them: a minimal core need not
  (`DEVICES.md` 12).

**Several machines, one driver.** In a system, one driver process serves
every machine that has its device, each as an instance of its own
(`DEVICES.md` 1.2), and each machine calls the device on the slot its own
image gives it: one program may list `example.dice` on slot 240 and
another on 241, and both run at once, each with its own state. The slot
travels with each machine (in OPEN, and in the address of each of its
calls: `DEVICES.md` 15.3); the driver is chosen by the name alone. Two
machines never share a device's state, whichever slots they use, so a
machine's results are the same as if it ran alone (§experimental.determinism).

**Limits in a system.** At most 16 experimental drivers per run. A driver
serves at most as many machines at once as a system has (the shell and 8,
§machines); a ninth machine the shell starts is refused by the Machines
device as always ("no room"), never because of an experimental device.
A restored machine is granted what its snapshot's grants say, not by
this rule (§experimental.conformance).

## §experimental.calls Calls and charges

A call to a granted experimental device goes to its driver, checked
first by the core against the call table the driver declared
(`DEVICES.md` 5, with the limits of 15.2), exactly as for a numbered
device on the bus: §calls.convention's rules for buffers, the length and
place results, the checked argument, `a1` where the table says.

**Charges are the table's, always.** For a numbered device an answered
call's charge is whatever its driver says, because SPEC says what the
right charge is. For an experimental device nothing does, so the charge
of **every** call, deferred or answered, is computed by the core from
the table: the base charge (4 for a deferred call, 64 for an answered
one, §charges) plus the entry's charge rule (`DEVICES.md` 5: a constant,
1 per 4 bytes of the buffer, so many units per byte, and an amount when
an argument is in a range). So a charge is a pure function of the call's
arguments, as declared at the handshake: it cannot depend on the
driver's timing, its host or its state. A driver that replies to an
answered call with another charge has broken the protocol and fails
(`DEVICES.md` 10).

**What a table may declare** (`DEVICES.md` 15.2): 1 to 16 calls; a
buffer of at most 4,096 bytes per call; a constant charge of at most
4,096, at most 2 units per byte of the buffer, an amount in a range of at
most 4,096; and a buffer must be charged at least 1 per 4 bytes (by the
flag, or 1 or 2 per byte), as every copy between memory and a device is
(§charges). The core refuses a driver whose table breaks any of these:
the run does not start.

**Why that keeps R10.** §charges promises that a unit of budget costs a
host about the same in any call. The core cannot measure a driver's
work, so the promise is the driver's: by declaring a charge it states
that the call costs it no more than that many instructions' time. The
floor on buffers keeps the copying honest (the core's own share of the
cost), and the ceilings keep a single call from eating a frame. A
driver that breaks its promise is late, and the host treats it like any
late component (`PERFORMANCE.md` R13–R15: warned at the 3rd late frame
in 60, its machines saved and stopped at the 10th in 300). With no
layers (§experimental.presentation) an experimental device adds no
drawing to R10's bound at all.

## §experimental.state State

An experimental device's state is a **section** of at most 16,384 bytes
that only its driver understands: the core asks for it (STATE), puts it
in the serialization with its length, and gives it back on restore
(RESTORE), without reading it. Where it goes is `STATE.md` 1.13: after
the sound device's section, before the argument, one section for each
experimental device the machine has, in header order, each a u32 length
and the bytes. A machine that has none (every machine of an image that
lists none, and every machine that was not granted one) has nothing
there, so its state, its hash and its snapshots are byte for byte what
they were before experimental devices existed.

A driver declares the most its section can be (at most 16,384) at the
handshake, and fails if a section it gives is longer.

A machine of a system has its own sections, from its own instance in the
driver, in the order of its own image's header, whatever slot another
machine gives the same device. So the state of a machine in a shell, its
hash and its snapshots follow exactly these rules, as for a machine run
alone.

## §experimental.determinism Determinism

A Terse32 machine's state is a pure function of its image and its
recorded inputs (§introduction, rule 2). An experimental driver must
keep that so: every result, every byte it writes and its state section
must be a pure function of

- the device's reset state (OPEN),
- the calls the machine made, in order: function, arguments and the
  bytes of the buffer the call reads, and
- the section it was restored from (RESTORE).

So it reads no clock, no random source, no file, no network and no
environment for anything a machine can see, keeps nothing of one machine
in another's, and does not depend on the budget field of CALL or on how
calls are batched. In a system one driver sees the calls of several
machines interleaved (in the system's order, `DEVICES.md` 1.3); since
each machine's results depend only on its own calls, the order in which
the other machines' calls come cannot change them, and a machine's state
is the same in a shell as alone. Nor may a result depend on the slot the
machine calls the device on: that is the image's handle, not an input. It has **no inputs**: unlike the content and page
drivers it gives no events, no writes and no charge at BEGIN, and its
input record (`DEVICES.md` 8) is empty, so a recording needs nothing
from it. A program that wants chance seeds the device from `random`
(§system), which is recorded; one that wants the time passes it
`unix_time`.

The core checks what it can: a driver that gives an input at BEGIN, or
asks to READ guest memory outside its call's buffer, fails (`DEVICES.md`
15.3). What it cannot check, that the results are a function of the
calls, shows as a state hash that differs between two runs with the same
inputs: run it twice, or replay a recording.

## §experimental.presentation Presentation

An experimental device draws nothing and plays nothing: its driver
declares no layers, publishes and subscribes to no properties and gives
no host services (`DEVICES.md` 15.2). A program shows what the device
tells it with the terminal or the vector display, as with any other
answer.

*Why not layers.* A layer has a place in the stack fixed by SPEC
(§vector.layers), a share of the host's drawing bounded by each device's
caps (R10's ink, cells and lines), and a look in the phosphor style
(`DEVICES.md` 7.1); none of these can be declared by a table the core
checks. A device that needs its own picture is a device to specify
(§experimental.graduation), not to try out.

## §experimental.failure When the driver fails or is slow

As for any driver, unchanged. A driver that exits, closes its socket,
does not answer within the reply timeout, answers ERROR or breaks the
protocol (including the rules of `DEVICES.md` 15.3) stops the run with
status 3 (`DEVICES.md` 10): its state is in it, and the device cannot be
made absent after the program has seen it. In a system that is the whole
run, as for a numbered device's driver. A driver that answers too
slowly is timed, warned about and, with its machines saved, stopped
(`PERFORMANCE.md` R14, R15; stopped state Halted with its slot, SPEC
§stopping), and the record names it by the identity it gave in HELLO
(`dev240` to `dev255`, `DEVICES.md` 15.3). Its machines are every machine
that uses it, whatever slot each calls it on: each is saved (the shell's
machines in their automatic slots, §machines.snapshots) and halted with
its own slot for the device.

An installed driver that is missing, cannot be started or fails its
handshake at the start of a run is a different case: no machine has seen
the device yet, so the run goes on without it, not granted, and the host
says why on standard error (`DEVICES.md` 15.1). The program then sees
what it sees on any other host. A driver named for the run with
`--driver` that fails its handshake refuses the run (status 2), as any
`--driver` does: it was chosen by someone watching.

## §experimental.conformance Conformance, snapshots and recordings

**§conformance does not cover experimental devices.** It compares state
hashes, and the hash of a machine that has an experimental device
includes a section that only its driver defines. Two hosts with the same
driver build agree; two hosts with different drivers that claim the same
name and version agree only if their authors made them agree. A machine
that was not granted one (the image lists it, the host did not grant it)
is an ordinary machine, fully covered: its state is defined by
`STATE.md` alone.

So that a snapshot or a recording of a machine with an experimental
device is never taken for a conformance run, its files say so:

- a snapshot file (`STATE.md` 4) of such a machine has version byte `1`
  instead of `0`, and is otherwise the same. A reader refuses version 1
  for an image whose state has no granted experimental device, and
  version 0 for one whose state has one;
- a bus recording (`DEVICES.md` 8) of such a machine has version `2`
  instead of `1`, and is otherwise the same.

A host without the driver refuses to restore either (the grants say the
device was granted, and it cannot provide it: `STATE.md` 4).

**In a system.** A shell's snapshots of its machines (its own slots 0–15,
a saved session, and the automatic slots 16–23 the host fills when a
device is slow) are the same files: version `1` for a machine that has
an experimental device, `0` for one that does not, each machine marked
by its own grants. Restoring one in a shell (`restore`, §machines.calls)
on a host without the driver, or with a driver of another version, fails
for that machine alone with reason 3, "needs a device", as for any
optional device the machine was granted and this host cannot give; the
other machines of a session are restored as usual. A shell whose own
image lists an experimental device is never snapshotted (§machines.snapshots).
There are no recordings of a system: `terse-bus --record` records a single
machine.

## §experimental.graduation From experiment to device

An experiment that works becomes a numbered device by writing down what
its driver already does, exactly, so that a second implementation can
agree with it byte for byte. Its document (`spec/<device>.md`, as
`spec/sound.md` is for sound) must give:

1. **A slot** in 11–239 and version 0. The experimental slot and name
   are not kept: a program built for the device lists the new slot.
2. **Every call**: its number, arguments, result, the buffers it reads
   and writes, what it does to the state, and what it does with every
   argument out of range, to the bit; its class (deferred or answered);
   its row in §index.
3. **Every charge**, in §charges's terms, with why it costs a host about
   what it charges (measured, as `crates/terse-core/tests/charges.rs`
   measures the others), and its numbers in the document's charges and
   limits section.
4. **The state**: the section's layout field by field, where it goes in
   `STATE.md`'s order and when it is there (a new section after the last,
   empty for a machine without the device, so that no hash changes), what
   restoring checks, and what is left out as presentation.
5. **Its inputs**, if it has any (events with new type numbers in
   §events, completions, what a recording holds of them), and how a host
   makes a run reproducible.
6. **Its presentation**, if it draws or plays: what a host must show, its
   layer's place, and the caps that bound its work per frame (R10).
7. **Conformance**: a test program that exercises every call with its
   scripted input and expected state hashes every 10 frames, and test
   vectors for the state section; a reference implementation in the
   repository (compiled in, a driver, or both) that gives them.
8. **The bus**: its row in `DEVICES.md` 1.7's table (where every core puts
   its section) and 5.1's (its call table), and its group in 2.1.

What the experiment gives for free: the call table it declared is item
2's buffers and classes and item 3's charges, and its recordings (made
with its driver, which the reference driver must then reproduce) are
the start of item 7's test.

## §experimental.open Not yet specified

- **Showing the device in a shell.** The Machines device's record of a
  machine (§machines.calls `info`) says nothing of its grants, so a shell
  cannot tell the user that a machine has an experimental device (and is
  outside conformance). A flag in the record, set when the machine's
  grants include one, would let the launcher mark it.
- **Inputs.** An experimental device has none (no events, no
  completions). A device that senses something outside the machine (a
  sensor, a MIDI port) needs event types, which are SPEC's.
- **Pictures and sound** (§experimental.presentation).
- **A minimal core** (`tcore`) does not grant experimental devices.
