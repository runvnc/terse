# Terse machine specification — v0 (draft)

Status: draft for the first prototype. Everything here may change until v1;
after v1 a published machine profile and published device versions never
change. §provisional lists the numbers that are placeholders until hosts have
been measured.

**Terse in three layers.** (a) The Terse network, `NETWORK.md`: blobs,
relays, feeds, entries, profiles and names, search. (b) Terse articles,
`ARTICLE.md`: the article format and what a reader must do with it. (a)
and (b) are **the core**: a native reader that implements them, with no
virtual machine, is a complete, conforming Terse client. (c) The Terse
machine, this document with the device documents in `spec/` and
`STATE.md`: an optional layer that reuses the core (programs are blobs
published in feeds, and the reader program shows articles on the page
device). Nothing in the core depends on these documents.

**How the machine is specified.** This document is the **core machine**:
the CPU, memory and frames, the image format, the calls, device 1
System with its events and controllers, device 2 Terminal, device 3 2D
vector display, their charges and limits, and conformance. That is the
whole of the machine's program profile. Each other device has a document
of its own, which says which sections of this one it builds on:

| Document | Device |
|---|---|
| `spec/content.md` | 4 Content: blobs, feeds, names, search, named records, documents |
| `spec/sound.md` | 6 Sound |
| `spec/page.md` | 7 Page: typeset prose and figures |
| `spec/machines.md` | 8 Machines: a system of machines, the shell, snapshots |
| `spec/vector3d.md` | 9 3D vector display |

`STATE.md` defines the state serialization and its hash, which
conformance compares (§conformance). Sections are named by stable ids,
written with a section sign and the id (§terminal.put): an id that starts with `content.`, `sound.`, `page.`,
`machines.` or `vector3d.` (or is one of those names) is in that
document of `spec/`, every other id is in this one. The rest of the
repository cites them the same way (`SPEC §terminal.put`).
`spec/RENUMBERING.md` maps the section numbers of earlier drafts to the
ids.

## §minimal What a minimal host implements

The split exists so that a host that only runs programs reads only this
document. A **program-profile host** (§profiles) implements:

- **The machine** (§profile): RV32IMC in user mode, ROM and RAM, the frame
  loop and its budget, stopping and faults; the image format (§image).
- **25 calls** (§calls.table): 10 of System (`wait_frame`,
  `next_event`, `frame_count`, `unix_time`, `random`, `controller`,
  `exit`, `open`, `argument`, `device`), 10 of the Terminal (`write`,
  `command`, `set_cursor`, `get_cursor`, `set_colours`, `get_colours`,
  `cells_read`, `cells_write`, `palette_set`, `size`) and 5 of the 2D
  vector display (`submit`, `clear`, `style`, `persistence`, `fill`).
  Every other call returns `FFFF_FFFFh`.
- **5 event types** it sends (§events): key down, key up, text, pointer
  and wheel. The other types come only from the optional devices.
- **Up to four controllers** (§controllers); a host with none reports
  them all as not connected.
- `open` and `argument` are trivial on a host without a shell: `open`
  checks its request and returns 0 (or `FFFF_FFFEh` for a bad one), and
  `argument` checks its buffer and returns 0.
- The data tables the machine's state depends on: UTF-8 decoding and the
  East Asian Width property for the terminal's wide characters
  (§terminal.cells), the xterm palette (§terminal.reset), and the
  White_Space property for display names (§display-names).
- **Basic text** (§terminal.text): the terminal must draw only 361
  characters (printable ASCII, the Latin-1 Supplement, dashes, quotation
  marks, bullet, ellipsis, minus, box drawing and block elements, U+FFFD;
  the articles' basic text repertoire), one fixed-width font; every other
  character is drawn as a placeholder box. Full text, every character, is device 10, which
  a program-profile host may leave out: no shaping, bidirectional text or
  font fallback is needed.

What a host draws (fonts, smoothing, the phosphor style's glow) is up to
it and is never machine state.

**A core on the device bus** (`DEVICES.md`) keeps less. Devices 2 and 3
(and every other device but 1 and 8) are then separate driver programs,
which may be the reference drivers, so the core implements only the
machine of §profile, the image format, device 1 System with §events and
§controllers, §calls.convention's rules for buffers (from each driver's
call table, `DEVICES.md` 5), §charges, and the state serialization's
layout (`STATE.md` 1, whose device sections it asks of the drivers). It
needs none of the terminal's Unicode tables or the palette: only UTF-8
validation and the White_Space property, for `open`'s display names
(§display-names). `DEVICES.md` 12 lists the messages such a core must
speak; `native/core-c` is one, in C.

**Headless, for tests**, a host needs no clock, randomness or
controllers: the reference's headless runs latch Unix time 0, supply no
random bytes (`random` gives zeros) and have controllers only from the
scripted input (`STATE.md` 6), and every expected hash in this repository
is made that way.

**Running is one tier, verifying is another.** Everything above is enough
to run every program written for the program profile. Showing that a host
runs them exactly as the reference does needs `STATE.md` as well:
conformance (§conformance) is defined as equal machine state after every
frame, compared by state hash. Short of that, a host can check itself
without computing any hash: `programs/conform-program` run with its
scripted input prints one PASS or FAIL line per section it can check (the
CPU, the terminal, the vector display's calls, the system calls, events,
controllers, the budget) and is expected to show
`programs/conform-program/expected-screen.txt`. Passing it is strong
evidence but not conformance: it cannot see its own drawings, the palette
it set or every unit of budget, which the hashes cover.

**An order to build it in.**

1. The image, the CPU, memory, the frame loop and `wait_frame`, `exit`
   and the Terminal's `write`: `programs/hello` runs and prints its
   text.
2. The rest of the Terminal (§terminal), then events and the other
   System calls: `hello` echoes what is typed.
3. The 2D vector display (§vector).
4. `programs/conform-program` with its scripted input: every line PASS.
5. For conformance, the state serialization of `STATE.md`: the hashes of
   `programs/conform-program/expected-hashes.txt`.

A host that wants more adds device documents one at a time: the reading
profile adds `spec/content.md`, `spec/page.md` and full text
(§terminal.text); a game may want
`spec/sound.md`.

## §introduction Introduction

Terse is a small virtual computer: a 32-bit RISC-V CPU (RV32IMC), a program
of at most 128 KiB, at most 1 MiB of RAM, a fixed budget of work per frame,
and a set of host-side devices reached through system calls. One program runs
in one machine. Programs are identified by the hash of their image and have
no dependencies; a program published in a feed also has an identity, its
publisher and name, that stays the same from one version to the next
(§machines.shell). A host may run several machines as one system, in which one
machine, the shell, starts the others and lays out their screens, and
which a machine may ask to open a program or a blob (§machines). A shell
can keep a machine as a snapshot and restore it later (§machines.snapshots).

Design rules that every part of this spec follows:

1. **Fixed speed.** A program receives the same budget of work per frame on
   every host.
2. **Deterministic.** Machine state is a pure function of the image and the
   recorded inputs (events, clock readings, random bytes, controller state,
   content completions).
3. **Devices present and transport; they do not compute for the program.**
   Device calls have bounded cost, are not programmable, and nothing the
   host renders can be read back by the program.
4. **Every unit of budget costs a host about the same.** Device calls are
   charged so that a frame spent in calls costs a host no more than a frame
   spent executing instructions (§charges).
5. **Small enough to reimplement.** An independent implementation of this
   document should be a few thousand lines.

All multi-byte values are little-endian. `h` suffix means hexadecimal.
"KiB" is 1,024 bytes.

---

## §profile Machine profile 1

| Property | Value |
|---|---|
| CPU | RV32IMC, little-endian, user mode only (§cpu) |
| Budget | 125,000 units per frame (§frames) |
| Frame rate | 60 frames per second, nominal |
| ROM | 1 to 131,072 bytes at `0001_0000h`, read and execute |
| RAM | 1 to 128 pages of 8 KiB (8 KiB to 1 MiB) at `0010_0000h`, read, write and execute; zero at reset |
| Devices | reached with `ecall` (§calls) |

### §cpu CPU

Exactly the RV32I base integer instruction set with the M (multiply and
divide) and C (compressed) extensions, as ratified by RISC-V International.
There is no A, F, D or B extension, no Zicsr or Zifencei, and no privileged
architecture: the program runs in user mode and cannot see control and
status registers, counters or traps.

- `FENCE` (any form with funct3 = 0, including `FENCE.TSO`, `PAUSE` and the
  hint forms) does nothing.
- HINT encodings (for example `c.addi x0, 5`, `c.slli x0, 1`,
  `c.srli x8, 0`, `slli x0, x0, 3`) execute as the instructions they are;
  writes to `x0` are discarded.
- Integer division follows the RISC-V rules and never traps: division by
  zero gives all ones (`div`, `divu`) and the dividend (`rem`, `remu`);
  `div` of −2³¹ by −1 gives −2³¹ and `rem` gives 0.
- Jump and branch targets need only be even (C is present); `jalr` clears
  bit 0 of the target as usual. A jump to an address outside ROM and RAM
  faults when the instruction there is fetched, not at the jump.
- Loads and stores must be aligned to their size (2 for halfwords, 4 for
  words); a misaligned access faults.

Everything else faults (§stopping): `ebreak` and `c.ebreak`; every
reserved or illegal encoding, including the all-zero parcel (`c.unimp`),
`C.ADDI4SPN`, `C.ADDI16SP` and `C.LUI` with a zero immediate, `C.LWSP` and
`C.JR` with register 0, RV32 C shifts with shamt[5] set, `C.SUBW`, `C.ADDW`
and the reserved MISC-ALU codes, every compressed floating-point load or
store, and all 48-bit and longer encodings; CSR instructions, `FENCE.I`,
`MRET`, `WFI` and other privileged encodings.

An instruction is fetched by its first 16-bit parcel: if the parcel's
low two bits are not `11` it is a 16-bit instruction, else the next
parcel is fetched too and the 32 bits are decoded. So a 48-bit or longer
encoding is fetched as 32 bits and faults as an illegal instruction with
those 32 bits recorded (§stopping); if its second parcel lies outside ROM
and RAM, it is a fetch fault at the second parcel's address instead.

### §memory Memory map

| Address | Contents | Access |
|---|---|---|
| `0001_0000h` to `0001_0000h + rom_len − 1` | ROM: the image's bytes | read, execute |
| `0010_0000h` to `0010_0000h + 8,192 × ram_pages − 1` | RAM | read, write, execute |
| everything else | nothing | any access faults |

So address 0 (a null pointer) faults, a store to ROM faults, and so does
fetching an instruction any part of which lies outside ROM and RAM (for
example a 32-bit instruction whose second half would be past the end of
ROM).

Code may run from RAM as well as ROM. An instruction fetched from RAM is
decoded when it is executed, so code a program writes to RAM takes effect at
once, and it costs more (§frames).

### §reset Reset state

- `pc = 0001_0000h`, the first byte of ROM.
- `x2` (`sp`) = `0010_0000h + 8,192 × ram_pages`, the end of RAM. Every
  other register is zero.
- All RAM is zero. All devices are in their reset state. The budget is
  zero and the frame counter is 0.

### §frames Frames and the budget

Time advances in frames. There are no interrupts. For each frame the host
does the following, in order:

1. Appends this frame's input events to the event queue (§events),
   latches the clock, the random bytes and the controllers (§system and
   §controllers), and applies the content completions it has for this frame
   (§content.submit). The content-done events of the completions are queued
   after the frame's input events.
2. Adds 125,000 to the budget, subtracts the charge for content delivered
   in step 1 (§charges), and runs the CPU while the budget is greater
   than zero.
3. Ends the frame: the frame counter increases by one, the sound device
   renders the frame's samples (§sound.render), and the host presents the
   terminal, the vector displays and the page as they are at this moment,
   and plays the samples.

Rules:

- An instruction fetched from ROM costs 1 unit; one fetched from RAM costs
  4. An `ecall` costs 1 (or 4 from RAM) plus the charge of the call it makes
  (§charges).
- An instruction that starts with a positive budget runs to completion, so
  the budget can end below zero. A negative remainder carries into the next
  frame; it may be larger than a frame's budget, in which case that frame
  runs nothing and the rest carries on. Unused budget never carries forward.
- When the budget runs out the machine simply pauses: the next frame
  continues with the next instruction, and the program is not told.
- A program that has finished its work for the frame calls `wait_frame`
  (§system): the rest of the budget is discarded and the call returns at
  the start of the next frame. That frame always runs: after `wait_frame`
  the carry is at most the 8 units of the `ecall` and its call charge,
  and the content delivered at the start of a frame costs at most 32,768
  units (8 requests of 16,384 bytes, §charges).
- `exit` and a fault stop the machine at once (§stopping). The rest of
  that frame's budget is dropped; the frame still counts. Nothing runs in
  later frames. The budget left in the state is that of any other frame's
  end: a positive remainder becomes 0, and a negative one (an `exit`
  whose `ecall` and charge took the budget below zero) is kept.

A host that falls behind real time lets virtual time lag. It must not run
extra frames to catch up.

In a system of several machines (§machines) each machine has its own
budget, carry and frame counter; §machines.frames says in what order their
frames run.

### §stopping Stopping and faults

A machine is running, or stopped in one of two ways:

| Stopped state | Cause | Recorded |
|---|---|---|
| Exit | the `exit` call (§system) | the exit code, 0–255 |
| Fault | one of the faults below | the kind, `pc` and an address |

A fault and an exit are never confused: a fault has no exit code. The
faulting instruction is not executed and is not charged, and `pc` is left
pointing at it.

| Fault kind | Cause | Address recorded |
|---|---|---|
| Illegal instruction | a reserved, illegal or unsupported encoding (§cpu); the encoding (16 or 32 bits) is recorded too | `pc` |
| Breakpoint | `ebreak`, `c.ebreak` | `pc` |
| Fetch | an instruction, or its second half, is not in ROM or RAM | the address that could not be fetched |
| Load | a load from outside ROM and RAM | the load address |
| Store | a store outside RAM (ROM included) | the store address |
| Misaligned | a load or store whose address is not a multiple of its size | the access address |

Alignment is checked first: a load or store that is both misaligned and
outside memory is a misaligned fault.

A host must show the user that the program faulted, with the kind and the
addresses, and must not present a fault as an exit code.

## §image Image format

A program is a single file (suggested extension `.terse`): a header followed
by the ROM contents.

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic: `54h 45h 52h 53h` (`TERS`) |
| 4 | 1 | Format version: `0` |
| 5 | 1 | Machine profile: `1` |
| 6 | 3 | `rom_len`: ROM length in bytes, 1 to 131,072 |
| 9 | 1 | `ram_pages`: number of 8 KiB RAM pages, 1 to 128 |
| 10 | 1 | `n_dev`: number of device entries, 0 to 16 |
| 11 | 5 | Reserved, must be zero |
| 16 | 16 | Title, UTF-8, padded with `00h` |
| 32 | 2 × `n_dev` | Device entries: (device slot, version byte) pairs |
| 32 + 2 × `n_dev` | `rom_len` | ROM contents; byte 0 is at address `0001_0000h` |

- The file length must equal `32 + 2 × n_dev + rom_len` exactly.
- The **image ID** is the BLAKE3-256 hash of the whole file.
- A device entry's version byte holds the device version in its low
  seven bits (0 to 127) and, in its top bit (`80h`), whether the device
  is **optional**: clear, the image **requires** the device; set, the
  program can run without it. So `(4, 00h)` requires content version 0,
  and `(4, 80h)` lists it as optional.
- A device slot may be listed at most once (slot 1 included), required or
  optional; a host refuses an image that lists one twice. Devices 1 and 8
  are never optional: a host refuses an image that lists either with the
  top bit set (device 1 is always there; device 8 is the shell's,
  §machines.shell, and absent elsewhere whatever the header says). So an
  image that lists device 8 and runs alone, or is started by a shell, is
  not refused and does not have device 8: `device` and every call to slot
  8 return `FFFF_FFFFh`, and its state's section for device 8 is the
  single byte `00h` (`STATE.md` 1.10; an image that does not list 8 has
  no such section at all).
- The **title** is the 16 bytes without their trailing `00h` bytes
  (possibly empty). It should be UTF-8, but a host does not check it: it
  shows what it cannot decode as it can (for example as U+FFFD). A
  built-in program whose title is not a valid display name (§display-names)
  has no identity `start_program` can name (§machines.calls); it can still
  be started by its image ID.
- A host must refuse an image whose profile or required device versions it
  does not implement. This holds for an image a shell starts too: the start
  fails (§machines.calls, reason 3) and the machine never runs. A device not
  listed in the header behaves as absent.
- An optional device the host provides in the version listed is
  **granted** when the machine is reset; one it lacks (or lacks in that
  version, or a slot that is no device) is not, and the image runs anyway
  with that device absent, exactly as if it were not listed. A host may
  also withhold an optional device it has (a user's choice, or a test).
  Which optional devices were granted is fixed at reset, never changes
  while the machine runs, and is part of the machine's state (`STATE.md`
  1.4), of its snapshots and of its recordings, so a replay or another
  implementation has the same devices. A program learns what it has with
  `device` (§system).
- Device 1 (system) is always present and need not be listed.

| Slot | Device | Version | Status in v0 |
|---|---|---|---|
| 1 | System | 0 | specified (§system, §events and §controllers); **reading** and **program profiles** |
| 2 | Terminal | 0 | specified (§terminal); **reading** and **program profiles** |
| 3 | 2D vector display | 0 | specified (§vector); **program profile** |
| 4 | Content | 0 | specified (§content, `spec/content.md`); **reading profile** |
| 5 | Messages | — | reserved |
| 6 | Sound | 0 | specified (§sound, `spec/sound.md`) |
| 7 | Page | 0 | specified (§page, `spec/page.md`); **reading profile** |
| 8 | Machines | 0 | specified (§machines, `spec/machines.md`); only the shell of a system has it |
| 9 | 3D vector display | 0 | specified (§vector3d, `spec/vector3d.md`) |
| 10 | Full text | 0 | specified (§terminal.text): a capability, no calls; **reading profile** |

Other slots are free; 0 and 11–255 are not devices in v0.

### §profiles Machine hosts and profiles

A **machine host** implements this document: the machine of §profile,
device 1 (always present) and any set of the other devices it chooses. A
host needs neither content nor the page to be a machine host. It states
which devices (slot and version) it provides and, for the content device,
which operations it carries out; an image that requires a device the host
lacks is refused (above), so a program never runs on a host that cannot
give it the devices its header requires, and an optional device the host
lacks is simply absent.

Two profiles name the sets of devices most hosts will have.

A machine host that runs programs, games and tools but not
necessarily the reader provides the machine's **program profile**:

- the machine, profile 1 (§profile), and devices 1 System (§system, §events and §controllers), 2 Terminal (§terminal) and 3 2D vector display (§vector),
  each complete, the terminal drawing at least the basic repertoire
  (§terminal.text);
- every other device is optional for it: content, sound, the page, the
  3D display, full text (device 10). A program-profile host refuses an
  image that requires one of them and runs, with the device absent, an
  image that lists it as optional.

A program meant to run on every machine host requires only devices 1, 2
and 3 and lists anything else it can do without as optional (for
example, content to keep a high score, sound for a game's effects), and
asks `device` (§system) at start which of them it has.

A machine host that only wants to run the reader (the built-in feed
reader, `programs/reader`, or any program written for the same profile)
provides the machine's **reading profile**:

- the machine, profile 1 (§profile), and devices 1 System (§system, §events and §controllers), 2 Terminal (§terminal), 4 Content (§content), 7 Page
  (§page) and 10 Full text (§terminal.text: the reader shows feeds' titles
  and names, in any script, in the terminal; the page draws every
  character in any case, §page.layout), each complete except as said next for device 4;
- within Content it must carry out get (`01h`, §content.submit),
  subscriptions (`03h`), timeline (`04h`), names (`09h`), identity
  (`08h`, with the own address's hints), subscribe (`05h`, with the hints
  of a feed address) and unsubscribe (`06h`) (§content.feeds), and
  may complete any other operation (put, publish, find, set profile,
  set nickname, programs, and the named records of §content.records) with
  status `05h` (Refused), without asking and without carrying it out (the
  submit itself is as §content.submit says: an invalid request is still
  rejected with `04h`, a valid one accepted); a program cannot tell that
  from a user or host that refuses;
- within Page, everything;
- devices 3, 6, 8 and 9 are optional: a reading-profile host need not
  provide them; it refuses an image that requires one and runs one that
  lists them as optional without them (the reader lists none of them).
  Device 8 exists only for the shell of a system (§machines) in any
  case. Nothing a reader shows depends on sound (device 6, §sound).

§content.feeds and §content.records say, for each optional group of operations, that a
host may answer it with `05h`. A program meant for the reading profile
must treat `05h` from those operations as a normal outcome and keep
working. The core of Terse (the network and articles, `NETWORK.md` and
`ARTICLE.md`) does not need a machine at all; the reading profile is
only for hosts that run the reader as a program. `terse run --profile
reading` runs one machine with exactly this profile, and
`programs/conform-reading` is the conformance image for it (§conformance);
`terse run --profile program` and `programs/conform-program` do the same
for the program profile. `terse run --without <slot>` withholds an
optional device on any host, and `--with <slot>` adds one to a profile
(`--profile program --with 10`: a program host with full text).

## §calls Calls

### §calls.convention Convention

A program reaches a device with `ecall`. Register `a7` (`x17`) holds the
call number, `(device slot << 8) | function`; arguments are in `a0`–`a5`;
the result is in `a0`, and a second result in `a1` where listed. A call
changes no register but `a0` (and `a1` where listed) and no memory but the
buffers it is said to write. `pc` continues after the `ecall`.

- An unknown call, or a call to a device the machine does not have (one
  the image does not list, or lists as optional and was not granted,
  §image), returns `FFFF_FFFFh` and does nothing else. That includes
  every call to slot 8 on a machine that is not a shell (§image), to slot
  10 (a device with no calls, §terminal.text), to a slot that is no device
  (0, 5, 11–255) and with `a7` above `FFFFh`. Its charge is 4 (§charges).
- A **buffer** is a pointer and a length in bytes. A buffer the device
  reads must lie wholly in ROM or wholly in RAM; one it writes must lie
  wholly in RAM. A buffer of length 0 is valid at any address, for every
  call and for every data range of a content request block (§content.request). Otherwise the call returns `FFFF_FFFEh` and does nothing else;
  this is not a fault.
- Five calls answer a buffer that is not where it must be in their own
  way instead: the `submit` of each vector display (§vector.calls, §vector3d.calls)
  rejects a list not wholly in RAM with 1, as it rejects a bad length, and
  so does the sound device's `submit` a list not wholly in ROM or RAM
  (§sound.calls);
  content `submit` rejects a request block not wholly in RAM with handle
  0 (§content.submit), and a data range in the block that is not where
  §content.request requires makes the request invalid (status `04h`); page
  `submit` returns 1 for a bad length before it looks at the buffer
  (§page.calls).
- A call that takes a buffer and other arguments checks the buffer
  first: a bad buffer returns `FFFF_FFFEh` whatever the other arguments
  are (for example `info` of a handle out of range, §machines.calls). Page
  `submit` is the one exception, above.
- Buffers need no alignment.
- Every call is charged, also when it fails (§charges).

### §calls.table Call table

| `a7` | Call | Arguments | Result | Class |
|---|---|---|---|---|
| `100h` | `wait_frame` | — | the frame counter in the frame it returns in | core |
| `101h` | `next_event` | `a0` = buffer (4 bytes, written) | 1 and the event, or 0 if the queue is empty | core |
| `102h` | `frame_count` | — | the frame counter | core |
| `103h` | `unix_time` | — | the Unix time latched at the start of the frame | core |
| `104h` | `random` | `a0` = buffer (written), `a1` = length, at most 256 | the length | core |
| `105h` | `controller` | `a0` = controller number, `a1` = buffer (8 bytes, written) | byte 0 of the state | core |
| `106h` | `exit` | `a0` = exit code (low 8 bits) | does not return | core |
| `107h` | `open` | `a0` = address of an open request (read) | 1 passed to the shell, 0 not | core |
| `108h` | `argument` | `a0` = buffer (32 bytes, written) | 1, or 0 if the machine has none | core |
| `109h` | `device` | `a0` = device slot | the device's version, or `FFFF_FFFFh` if the machine does not have it | core |
| `200h` | `write` | `a0` = buffer (read), `a1` = length, at most 4,096 | the length | deferred |
| `201h` | `command` | `a0` = command | 0 | deferred |
| `202h` | `set_cursor` | `a0` = column, `a1` = row | 0 | deferred |
| `203h` | `get_cursor` | — | `a0` = column, `a1` = row | answered |
| `204h` | `set_colours` | `a0` = foreground, `a1` = background | 0 | deferred |
| `205h` | `get_colours` | — | `a0` = foreground, `a1` = background | answered |
| `206h` | `cells_read` | `a0` = first cell, `a1` = buffer (written), `a2` = count | the count | answered |
| `207h` | `cells_write` | `a0` = first cell, `a1` = buffer (read), `a2` = count | the count | deferred |
| `208h` | `palette_set` | `a0` = first entry, `a1` = buffer (read), `a2` = count | the count | deferred |
| `209h` | `size` | — | `a0` = 80 columns, `a1` = 30 rows | deferred |
| `300h` | `submit` | `a0` = list address, `a1` = length | 0 done, 1 rejected | deferred |
| `301h` | `clear` | — | 0 | deferred |
| `302h` | `style` | `a0` = style | 0 done, 1 rejected | deferred |
| `303h` | `persistence` | `a0` = persistence | 0 done, 1 rejected | deferred |
| `304h` | `fill` | `a0` = fill mode | 0 done, 1 rejected | deferred |

§system, §terminal.calls and §vector.calls define these calls; each other
device's document defines its own, and §index lists every call of every
device. The class (§charges) sets a call's
base charge: 4 for a **deferred** call, whose result and charge follow from
its arguments, 64 for an **answered** one, whose result or charge depends
on the device's state; the calls of devices 1 and 8 (**core**) are the
machine's own and cost 4.

## §system Device 1 — System

- **`wait_frame`** ends the frame (§frames) and returns one more
  than the frame counter at the call. The value is fixed at the call;
  since the next frame always runs (§frames), it is the frame counter
  of the frame the call returns in.
- **`next_event`** removes the event at the head of the queue (§events)
  and writes its four bytes, type, `a`, `b`, `c`, to the buffer; it returns
  1. If the queue is empty it writes nothing and returns 0.
- **`frame_count`** returns the frame counter: N throughout frame N. Frame 0
  is the first frame after reset. It increases at the end of every frame
  and wraps from `FFFF_FFFFh` to 0.
- **`unix_time`** returns the Unix time in seconds (low 32 bits) that the
  host latched at the start of the frame. (At reset the latched time is
  0 and every controller reads as not connected. No call can observe
  that, because every call runs in a frame, after step 1 has latched new
  values. It shows only in the state of a machine just reset, `STATE.md`
  1.5.)
- **`random`** fills the buffer with the next bytes of the random bytes the
  host supplied for this frame, in order. A length above 256 is a bad
  buffer. The host must supply enough bytes for any frame: 463,872 cover the
  worst case (every call that can start in a frame taking 256). A host that
  supplies fewer (a headless run supplies none) gives zeros once its bytes
  are used up.
- **`controller`** writes the eight bytes of controller `a0`'s state
  (§controllers) and returns byte 0. A controller number above 3 reads as a
  controller that is not connected.
- **`exit`** stops the machine with exit code `a0 & FFh` (§stopping).
  Like any call it then writes its result, 0, to `a0`, and `pc` is left
  after the `ecall`; the call is charged as any of device 1 (4, plus the
  `ecall`'s own 1 or 4), and then the budget is settled as §frames says.
  The state after an `exit` (`STATE.md` 1.2, 1.3) therefore has `a0` = 0
  whatever code was given (`STATE.md` 3 has a test vector).
- **`open`** asks the shell to open something. `a0` is the address of an
  **open request**, which must lie wholly in ROM or wholly in RAM:

  | Offset | Size | Field |
  |---|---|---|
  | 0 | 1 | Kind: `1` a blob, `2` a program |
  | 1 | 32 | Kind 1: the blob ID. Kind 2: the publisher's feed key (all zero for a built-in program) |
  | 33 | 1 | Kind 2: the name's length n, 1 to 48 |
  | 34 | n | Kind 2: the name, a valid display name (§display-names) |

  A request of another kind, or one that does not lie wholly in ROM or
  RAM, or whose name is not valid, is a bad buffer (`FFFF_FFFEh`). Since
  the request's length depends on its bytes, it is checked in this order,
  each step reading only what the earlier ones showed to be there: byte 0
  is in ROM or RAM; the kind is 1 or 2; for kind 1, bytes 0–32 lie wholly
  in ROM or wholly in RAM; for kind 2, bytes 0–33 do, n is 1–48, bytes
  0 to 33 + n do, and the name is a valid display name. If the
  machine is not one a shell started (it runs on its own, or it is the
  shell itself), or a request of this machine still waits (the shell has
  not read it with `request`, §machines.calls), the call returns 0 and does
  nothing else. Otherwise the request waits for the shell, which is told
  of it at the start of the next system frame (§machines.frames), and the call
  returns 1. What the shell does with it is the shell's business; the
  machine is not told.
- **`argument`** writes the machine's **argument**, the blob ID the shell
  gave it when it started it (§machines.calls), to the buffer, 32 bytes wholly
  in RAM (else `FFFF_FFFEh`), and returns 1; a machine that has none (the
  shell gave none, it runs on its own, or it is the shell) gets 0 and
  nothing is written. The argument never changes and is part of the
  machine's state (`STATE.md` 1.13).
- **`device`** returns the version of device `a0` if the machine has it:
  device 1, every device its image requires, and every optional device
  it was granted (§image); 8 only for a shell (§machines.shell). For any
  other `a0`, also one above 255, it returns `FFFF_FFFFh`. The answer is
  fixed at reset, so a program asks once at start and branches on it.

Events, the latched time, the random bytes and controller state are the
machine's only inputs from this device (which optional devices were
granted is fixed at reset, part of the state, `STATE.md` 1.4). A host that records them per frame
(with the content completions of §content.determinism) can replay a run exactly:
the events as the host appended them (page events, §page.events, and
feeds updated included; content-done events follow from the completions
and need no recording), and of the random bytes only those the program
took, since it reads zeros beyond them either way. What else a replay
needs is where the run started: reset, or a restored snapshot (§machines.snapshots), which holds the machine's argument. For a machine that runs on its
own, `open` always returns 0 (or `FFFF_FFFEh` for a bad buffer), so it is
no input; for one a shell started it is (§machines.determinism). `terse run
--record` writes such a recording and `--replay` replays it (`STATE.md`
5).

## §events Events

The queue holds at most 64 events. An event that arrives when the queue is
full is dropped.

| Type | Name | `a` | `b` | `c` |
|---|---|---|---|---|
| `01h` | Key down | key code | modifiers | 0 |
| `02h` | Key up | key code | modifiers | 0 |
| `03h` | Text | code point bits 0–7 | bits 8–15 | bits 16–20 |
| `04h` | Pointer | x bits 0–7 | y bits 0–7 | bits 0–1: x bits 8–9; bit 2: y bit 8; bits 3–5: buttons; bits 6–7: 0 |
| `06h` | Wheel | vertical steps, signed | horizontal steps, signed | 0 |

- **Key codes** are USB HID keyboard usage IDs (for example `04h` = A,
  `28h` = Enter, `29h` = Escape, `4Fh`–`52h` = Right, Left, Down, Up).
- **Modifiers:** bit 0 Shift, bit 1 Control, bit 2 Alt, bit 3 Meta.
- **Key repeat.** While a key is held, a host may send further key-down
  events for it (its auto-repeat), each followed by the text it types,
  with no key-up between them; one key-up ends the key. So a key-down for
  a key already down is a repeat. A program that wants a key's state
  tracks the downs and ups; one that wants its typing reads text events.
- **Text** carries one Unicode scalar value produced by the host's text
  input, after keyboard layout and input-method composition. A host
  sends no text event for a control character (U+0000–U+001F,
  U+007F–U+009F): Enter, Tab, Backspace, Escape and Control-letter
  combinations arrive only as key events. Text the user pastes arrives the
  same way, one event per character, with no key events: a CR LF pair or
  a lone CR becomes U+000A, U+000A is kept, and every other control
  character is dropped; it comes no faster than the queue has room for,
  and goes to whichever machine has the focus as each part is sent. How
  the user pastes is up to the host (the reference host: Control+V, or
  Shift+Insert).
- **Keys a host keeps.** Besides the home key of a system (§machines.input)
  and the keyboard while a consent prompt is shown (§content.feeds), a host
  may keep a few keys for its own functions, such as a paste key; such a
  key's press and its release reach no program (the modifier keys held
  with it are still delivered). A host should keep as few as it can and
  say which.
- **Pointer** positions are in screen units (§screen): x is 0–639 and y
  is 0–479. It is sent when the position or a button changes, at most once
  per frame for movement; a button change is sent at once, with the current
  position. Positions outside the screen are clamped to it, and nothing is
  sent when the pointer leaves the host's window. Buttons: primary,
  secondary, middle, from the lowest bit.
- **Wheel:** a positive vertical step means scroll down (towards later
  content); a positive horizontal step means scroll right. At most one wheel
  event is sent per frame, with each value from −127 to 127; the host adds
  up smaller movements until they make a whole step.
- **Content done** and **feeds updated** come from the content device
  (§content.events).
- **At most one feeds-updated event is queued per frame**: a second one
  in the same frame's input (from the host or a device) is dropped, as
  is any event that finds the queue full. This holds for every machine,
  also one without device 4: a host that is given such an event (a
  scripted input, `STATE.md` 6) queues it like any other, and a program
  that does not use content ignores it.
- **Page laid out**, **page anchor**, **page located** and **page located
  y** come from the page device (§page.events).
- **Machine changed**, **home**, **machine pressed**, **open
  requested** and **snapshot done** are sent only to the shell of a
  system (§machines.input).

## §controllers Controllers

Up to four game controllers, numbered 0–3. Their state is latched at the
start of each frame; `controller` returns eight bytes:

| Byte | Contents |
|---|---|
| 0 | Bit 0: connected. Other bits zero. |
| 1 | Bits 0–7: up, down, left, right, A (bottom), B (right), X (left), Y (top) |
| 2 | Bits 0–7: left shoulder, right shoulder, left trigger, right trigger, select, start, left stick press, right stick press |
| 3 | Left stick x, signed: −127 left to 127 right |
| 4 | Left stick y, signed: −127 up to 127 down |
| 5 | Right stick x |
| 6 | Right stick y |
| 7 | Zero |

A controller that is not connected reads as all zeros. Axis values are never
−128.

## §screen Screen units

The screen is 640 units wide and 480 high, with (0, 0) at the top left. A
terminal cell is 8 units wide and 16 high, so the cell at (column, row) covers
x from `column × 8` and y from `row × 16`. Pointer positions, the vector
display and the page use these units. Units are square: the host shows the screen at a
4:3 shape, so a circle is round.

## §display-names Display names

A **valid display name** is valid UTF-8, 1 to 48 bytes, with no control
characters (U+0000–U+001F, U+007F–U+009F) and no whitespace (the Unicode
White_Space property) as its first or last character. Program names in
an open request (§system) and the names of the content device
(`spec/content.md`) must be valid display names, and a built-in program
whose image title is not one cannot be named by its title (§image).

## §terminal Device 2 — Terminal (version 0)

A grid of 80 columns by 30 rows. The grid is held by the device, not in the
program's memory.

### §terminal.cells Cells

Each cell is four bytes:

| Byte | Field |
|---|---|
| 0 | Character code, low byte |
| 1 | Character code, high byte |
| 2 | Foreground palette index |
| 3 | Background palette index |

The character code is a Unicode code point in the range `0000h`–`FFFFh`. The
grid is 2,400 cells (9,600 bytes); cell number `row × 80 + column` is the
cell at (column, row), and `cells_read` and `cells_write` use these bytes in
this order.

A character is **wide** if its Unicode 16.0 East Asian Width property is W or
F, including the code points that are unassigned in 16.0 and that
`EastAsianWidth.txt` gives W by default (for example U+FA6E, in the CJK
Compatibility Ideographs block); every other character is narrow. A wide character occupies two cells in a
row: the left cell holds its code and the right cell holds the continuation
code `FFFFh`. Combining marks are not treated specially in v0; each takes a
cell.

### §terminal.calls Calls

- **`write`** puts characters (§terminal.put) from all the bytes of the
  buffer (`a0` = address, `a1` = length, 0 to 4,096 bytes, in ROM or RAM),
  in order, and returns the length. A length above 4,096 is a bad buffer:
  the call returns `FFFF_FFFEh` and does nothing (§calls.convention). The device
  takes the whole buffer whatever the budget; the charge is 2 per byte and
  nothing for the scrolls and clears the bytes cause (§charges), so a call
  that starts with little budget left can take the budget below zero, by
  at most 8,199 units (an `ecall` from RAM, 4, the call's 4 and 8,192 for
  4,096 bytes, started with 1 unit left), which carries into the next
  frame (§frames). An empty buffer returns 0. A UTF-8 sequence may be
  split across calls.
- **`command`** carries out a command (§terminal.commands); a value above 255 or
  not in the table does nothing.
- **`set_cursor`** moves the cursor to (`a0`, `a1`), each clamped to 79 and
  29, and clears pending wrap.
- **`set_colours`** sets the current foreground and background indexes; a
  value above 255 leaves that colour as it is. `get_cursor` and
  `get_colours` return the current values.
- **`cells_read`** copies `count` cells (`4 × count` bytes) of the grid,
  from cell `first` on and wrapping from cell 2,399 to cell 0, into the
  buffer. **`cells_write`** copies the buffer into the grid in the same way.
  `first` must be below 2,400 and `count` at most 2,400; otherwise the call
  returns `FFFF_FFFEh` and does nothing. Cells written are not checked: the
  host draws a wide character across two cells when the next cell in the row
  holds `FFFFh`; how it draws any other arrangement is not specified.
- **`palette_set`** sets `count` palette entries from entry `first` on,
  wrapping from 255 to 0, from `3 × count` bytes of red, green, blue
  (0–255 each). `first` must be below 256 and `count` at most 256;
  otherwise the call returns `FFFF_FFFEh` and does nothing.
- **`size`** returns 80 and 30.

### §terminal.put Put character

The bytes given to `write` are decoded as UTF-8. When a character is
complete:

| Character | Action |
|---|---|
| `08h` | Cursor left one column, stopping at column 0 |
| `09h` | Cursor to the next column that is a multiple of 8, stopping at column 79 |
| `0Ah` | Cursor to column 0 of the next row; scrolls up if on the last row |
| `0Ch` | Clear the grid and move the cursor to (0, 0) |
| `0Dh` | Cursor to column 0 |
| other below `20h`, and `7Fh` | Ignored |
| anything else | Printed (see below) |

Printing a character:

1. If the **pending wrap** flag is set, or the character is wide and the
   cursor is at column 79, the cursor first moves to column 0 of the next
   row, scrolling up if on the last row. In the second case the cell at
   column 79 is first set to a space with the current colours; if it was
   the right half of a wide pair, the left half at column 78 becomes a
   space and keeps its colours, as in step 3.
2. The character is written at the cursor with the current colours. A wide
   character also writes `FFFFh` with the current colours to the next cell.
3. If a cell written in step 2 was half of a wide pair, the other half of
   that pair becomes a space and keeps its colours.
4. The cursor moves right by the character's width. If that would pass
   column 79, the cursor stays at column 79 and pending wrap is set.

Pending wrap is cleared by step 1, by the control characters `08h`, `09h`,
`0Ah`, `0Ch` and `0Dh`, by `set_cursor`, and by command `01h`. Ignored
control characters leave it set.

The code point `FFFFh` is written as `FFFDh`, so put character never creates
a continuation cell on its own.

Malformed UTF-8 and code points above `FFFFh` are written as `FFFDh`. Each
maximal invalid prefix of a sequence produces one `FFFDh` (the WHATWG rule),
and the byte that interrupted the sequence is decoded afresh. Overlong forms
and surrogates are malformed.

### §terminal.commands Commands

| Value | Command |
|---|---|
| `01h` | Clear: every cell becomes a space with the current colours; cursor to (0, 0) |
| `02h` | Scroll up one row; the bottom row is cleared |
| `03h` | Scroll down one row; the top row is cleared |
| `04h` | Clear from the cursor to the end of the row; if the cursor is on the right half of a wide pair, the left half becomes a space and keeps its colours |
| `05h` | Show the cursor |
| `06h` | Hide the cursor |

"Cleared" means space (`0020h`) with the current colours. Commands
`02h`–`06h` leave the cursor and pending wrap as they are.

### §terminal.reset Reset state and palette

Every cell is a space with foreground 7 and background 0. The cursor is at
(0, 0) and shown; pending wrap is clear and no UTF-8 sequence is pending.
The palette is the xterm 256-colour palette: entries 0–15 the standard and
bright colours, 16–231 the 6 × 6 × 6 colour cube, 232–255 the grey ramp.

### §terminal.presentation Presentation

How the host draws the grid (font, size, cursor shape, smoothing) is not part
of this spec and cannot be observed by the program, except which
characters it must be able to draw (§terminal.text). §vector.layers says how the
grid is combined with the vector display. Since the scrolls and clears inside
a `write` are not charged, a host should make a scroll or a clear cost time
proportional to one row, not to the whole grid (for example, rows kept as a
ring and a cleared row filled only when it is written to).

### §terminal.text Basic text and full text

What the terminal holds never depends on which characters a host can
draw: the cells, the widths of §terminal.cells, put character, the
commands and the cursor are the same on every host, and so is the
machine's state. Only what is drawn differs:

- The **basic repertoire** is what every host that has the terminal
  draws. It is the basic text repertoire of articles (`ARTICLE.md`
  section 1.1 lists it in the same words), 361 characters:

      U+0020-U+007E  printable ASCII                       95
      U+00A0-U+00FF  Latin-1 Supplement: no-break space,   96
                     accented letters, £ ° × ÷ and more
      U+2013-U+2014  – — en dash, em dash                   2
      U+2018-U+2019  ‘ ’ single quotation marks             2
      U+201C-U+201D  “ ” double quotation marks             2
      U+2022         • bullet                               1
      U+2026         … horizontal ellipsis                  1
      U+2212         − minus sign                           1
      U+2500-U+259F  box drawing and block elements       160
      U+FFFD         � replacement character                1

  All are narrow (§terminal.cells: none is W or F; those whose East
  Asian Width is A, such as × and …, are narrow like every other), so
  one fixed-width font of 8 × 16 cells covers them.
  `vectors/basic-text.txt` gives the list as ranges.
- **Full text** is every other character as well, drawn with fonts and
  fallback the host chooses, a wide character across its two cells
  (§terminal.cells).

A machine has full text if it has **device 10, Full text** (version 0),
a capability: a device with no calls, no events and no state. An image
lists (10, 00h) if it cannot do without it, so that a host without full
text refuses it (§image), or (10, 80h) if it can, and then asks `device`
(§system) whether it was granted and may choose what it prints. A call
to slot 10 returns `FFFF_FFFFh`. A host provides device 10 if it can
draw full text; device 10 without device 2 changes nothing.

A machine that does not have device 10 is drawn with the basic
repertoire on every host, also on one that could draw more, so that it
looks the same everywhere and its author sees what other hosts show.
Each cell is drawn on its own: a cell whose code is in the basic
repertoire as that character (U+0020 and U+00A0 as its background
only), a cell holding a control code (`0000h`–`001Fh`, `007Fh`–`009Fh`)
as its background only, and every other cell as the **placeholder**: an
outlined box inside the cell, in the cell's foreground colour. So a wide
character is two placeholders, its own cell and its continuation cell;
U+FFFD, which put character writes for malformed UTF-8, is basic and
drawn as itself. A machine that has device 10 is drawn with full text.

Text input is not affected: a text event (§events) carries any Unicode
scalar value, on every host and whether or not the machine has full
text.

## §vector Device 3 — 2D vector display (version 0)

Draws lines and simple filled shapes from a **display list**: a short
sequence of drawing commands that the program builds in RAM and hands to the
device. The device keeps its own copy and the host draws it every frame until
the program submits another. The device does no geometry for the program: it
does not rotate, scale or clip against anything but the screen, and nothing
it draws can be read back.

### §vector.coordinates Coordinates

Coordinates are signed 16-bit numbers in **sixteenths of a screen unit**
(§screen), little-endian. The screen's top left is (0, 0) and its bottom
right is (10240, 7680). Anything outside the screen is not shown.

### §vector.calls Calls

- **`submit`** copies `a1` bytes from address `a0` into the device,
  replacing the previous list, and returns 0. It is rejected, returning 1
  and keeping the previous list, if the length is 0 or more than 16,384 or
  any part of the range is outside RAM.
- **`clear`** empties the list.
- **`style`** sets the display style: 0 clean, 1 phosphor (§vector.phosphor).
- **`persistence`** sets the persistence, 0–255 (§vector.phosphor).
- **`fill`** sets the fill mode: 0 filled shapes are filled, 1 they are
  drawn as outlines (§vector.list).

`style`, `persistence` and `fill` return 1 and change nothing for a value
out of range, else 0. After reset the list is empty, the style is clean, the
persistence is 160 and the fill mode is 0. The list that the device holds at
the end of a frame is the one the host shows for that frame.

### §vector.list Display list

A list is a sequence of commands, each one opcode byte followed by its
operands. Drawing state at the start of a list: colour opaque white
(255, 255, 255, 255), width 16 (one screen unit), origin (0, 0), current
point (0, 0).

| Opcode | Command | Operands | Effect |
|---|---|---|---|
| `00h` | End | — | Ends the list |
| `01h` | Colour | r, g, b, a (1 byte each) | Sets the colour; `a` is opacity, 255 opaque |
| `02h` | Width | w (1 byte) | Sets the line width in sixteenths; 0 counts as 1 |
| `03h` | Origin | x, y | Sets the origin, which is added to every later coordinate (but not to a later origin) |
| `04h` | Move | x, y | Sets the current point |
| `05h` | Line | x, y | Draws a line from the current point to (x, y), which becomes the current point |
| `06h` | Circle | radius (2 bytes, unsigned) | Draws a circle outline centred on the current point |
| `07h` | Disc | radius (2 bytes, unsigned) | Draws a filled circle centred on the current point |
| `08h` | Rectangle | x, y | Fills the rectangle with opposite corners at the current point and (x, y) |
| `09h` | Triangle | x1, y1, x2, y2 | Fills the triangle formed by the current point and the two given points |

- Lines and circle outlines use the current width and have round ends, so a
  line to the current point draws a dot.
- The sum of a coordinate and the origin wraps to 16 bits. The current
  point is stored after the origin has been added, so a later Origin command
  does not move it.
- A circle outline is centred on its radius: half the width falls inside
  and half outside.
- Circle, disc, rectangle and triangle leave the current point unchanged.
- Shapes are drawn in list order, each over the earlier ones, blended by
  opacity.
- The list ends at an End command, at the end of the submitted bytes, at an
  unknown opcode, or at a command whose operands are cut off by the end of
  the list. Commands before that point are drawn.
- **Outlines.** In the phosphor style, and with fill mode 1, nothing is
  filled: a disc is drawn as a circle outline of the same radius, a
  rectangle as the four lines between its corners and a triangle as the
  three lines between its points, each with the width in effect at that
  command. With (x0, y0) the current point and (x1, y1) the rectangle's
  given corner, its lines are drawn in the order (x0, y0)–(x1, y0),
  (x1, y0)–(x1, y1), (x1, y1)–(x0, y1), (x0, y1)–(x0, y0); a triangle's
  from the current point to the first given point, to the second, and
  back. The page's figures are outlined too (§page.layout).

### §vector.fill-cap Fill cap

When filled shapes are filled (the clean style with fill mode 0), the area
they cover in one list is limited. Each disc, rectangle and triangle, in list
order, counts the area of its bounding box: for a disc the square from
(x − r, y − r) to (x + r, y + r), for a rectangle the rectangle itself, for a
triangle the smallest axis-aligned box holding its three points, with
coordinates as resolved in §vector.list. A disc's square is computed from
its resolved centre without wrapping (x − r and x + r may lie beyond the
16-bit range). The box is clipped to the screen
(0 to 10240 by 0 to 7680), and its area in square screen units is its width
times its height in sixteenths, divided by 256 and rounded up.

The running total starts at 0 for each list. A filled shape whose area would
take the total above 4 × 640 × 480 = 1,228,800 is not drawn, and neither is
any filled shape after it in the list; lines and circle outlines are still
drawn. A shape that is not drawn does not count. Outlines (§vector.list) are
not limited.

### §vector.layers Layers

In the clean style the host combines the displays in a fixed order, from the
bottom:

1. the terminal's cell backgrounds;
2. the page (§page): its background, then its text;
3. the vector displays: device 3's shapes, then device 9's (§vector3d.presentation);
4. the terminal's characters and cursor.

So terminal text is always drawn over vector shapes, and a cell's background
colour is always behind them. The page lies between: shapes are drawn over
it, and terminal characters over both (a program that shows the page usually
leaves the terminal cells under its rectangle blank). A program that wants a panel of text to hide the
shapes behind it draws a filled rectangle at the end of its list; the text
still appears above it. Smoothing of edges and the exact colour of blended
pixels are up to the host and cannot be observed by the program.

In a system (§machines) each machine's screen is combined in this order on
its own, with its own style; §machines.screens says how the screens are combined.
A machine whose 3D list has Surfaces in effect (§vector3d.list) is drawn as
§vector3d.presentation says: these layers, less the Surfaces' sources, then the
Surfaces, then its 3D lines.

### §vector.phosphor Phosphor style

In the phosphor style the whole screen imitates a vector monitor or
oscilloscope tube:

- The screen is black: cell background colours are not drawn.
- Shapes are drawn as outlines (§vector.list) and add light to what is
  beneath them instead of covering it; where lines cross, the crossing is
  brighter. Opacity acts as brightness. Hosts should make overlapping and
  repeated lines add up to visibly brighter light, so a program can brighten
  a line by drawing it more than once.
- The terminal's characters and cursor are light on the same tube, in their
  foreground colour, drawn in the same way as the shapes. So are the page's
  text and figures (as outlines), in their colours; the page's background
  is not drawn.
- The beam is a sharp line with a faint bloom: its bright core is narrower
  than the stated width, and the glow around it is dim and short.
- Light lingers. What the current list and text show is drawn at full
  brightness each frame; in addition, an afterglow of what was shown before
  fades away: quickly at first, then with a dim tail that is multiplied by
  persistence ÷ 256 each frame, down to black. With persistence 0 there is
  no tail. Something that stays put does not grow brighter than about its
  own light, and when a shape keeps its structure from one frame to the next
  and only moves, its afterglow is a continuous smear rather than separate
  copies.

The exact brightness, core width, glow, decay of the fast part, tone curve
and where an outline sits on a shape's edge are up to the host. The
afterglow is part of how the host presents the display, not machine state,
and the program cannot observe it. A host should let the user turn the
afterglow off. In the clean style the persistence value has no effect.

### §vector.determinism Determinism

The device's state is the style, the persistence, the fill mode and the copy
of the list. It takes no input from the host. The style and the persistence
are also set by device 9 (§vector3d.calls): one setting for both displays.

## §charges Charges

Every call is charged in budget units on top of the `ecall` instruction
itself (1 from ROM, 4 from RAM). Charges are subtracted from the budget as
the call runs and may take it below zero (§frames).

| What | Charge |
|---|---|
| Any call, including one that fails: its base charge | 4 for a deferred call and for every call of devices 1 and 8; 64 for an answered call |
| An unknown call, a call to a device the machine does not have, and a call whose `a7` is above `FFFFh` (§calls.convention) | 4 |
| `write`: each byte of the buffer | 2 |
| A clear, scroll up or scroll down done by `command` (`01h`–`03h`) | 150 |
| Bytes copied between memory and a device: `next_event` (4 when an event is written), `random`, `controller` (8), `cells_read`, `cells_write`, `palette_set` and an accepted vector `submit`, each when it is carried out; `open` (33, or 34 + n, when the request is read: whenever it is valid, also when the call returns 0) and the system's `argument` (32, when written) | 1 per 4 bytes, rounded up, per call |

The other devices are charged in the same terms; each device's document
lists its charges (§content.charges, §sound.charges, §page.charges,
§machines.charges, §vector3d.charges).

Nothing else is charged: a rejected submit copies nothing, and the
scrolls and clears a `write` causes are covered by its 2 per byte
(§terminal.presentation).

**Deferred and answered calls.** A device call is **deferred** if its
result is one the program could work out without the device (a constant,
one of its arguments, or a simple function of its arguments and of where
its buffer lies) and its charge depends only on its arguments; it is
**answered** if its result or its charge depends on the device's state.
§calls.table gives every call's class. The answered calls are the
terminal's `get_cursor`, `get_colours` and `cells_read`, content `submit`
and page `locate`; every other call of devices 2, 3, 4, 6, 7 and 9 is
deferred. A host that keeps a device elsewhere (another process, a socket)
can carry out its deferred calls later, together, without waiting for
them, while an answered call needs a round trip, which its base charge of
64 pays for. Devices 1 (System) and 8 (Machines) are the machine's own:
the frame loop, the event queue, the host's inputs and the machines of a
system are never carried elsewhere, so their calls cost 4 whatever they
return, although several (`next_event`, `frame_count`, device 8's `info`,
`start`, …) answer from state.

The charges are chosen so that a frame spent entirely in any one kind of call
costs a host no more per unit than a frame of instructions from ROM;
`crates/terse-core/tests/charges.rs` measures this.

## §conformance Conformance

An implementation conforms to profile 1 if:

- its CPU executes RV32IMC exactly as §cpu says, including which
  encodings fault;
- for any image and recorded input sequence, the machine state after every
  frame equals that of the reference implementation. The state, and a hash
  of it for comparing implementations, are defined byte for byte in
  `STATE.md`.

`programs/conform` is a test image with a recorded input script and the
expected state hash every 10 frames. It checks itself where it can (one PASS
or FAIL line per section) and its last phase is a deliberate worst case for a
host (full-budget frames, every cell rewritten, maximum-size display lists at
the fill cap), for measuring how a host holds its frame rate.

A host that offers only the reading profile (§profiles) cannot run
`programs/conform`, which requires devices 3 and 9. `programs/conform-reading`
is built from the same source with the sections for other devices and for
the optional content operations left out, and has its own input script
and expected hashes; `terse run --profile reading` runs it (and any image)
on exactly the reading profile. Likewise `programs/conform-program`, for
the program profile, requires only devices 2 and 3, leaves out the
sections of content, the page and the 3D display, and checks `device`;
`terse run --profile program` runs it on exactly that profile.

## §provisional Provisional values

These numbers are placeholders until weak hosts have been measured. Every
numeric limit of the core machine is listed here, with the section that
defines it; each device document lists its own (§content.charges,
§sound.charges, §page.charges, §machines.charges, §vector3d.charges).

The budget and the charges:

- the budget, 125,000 units per frame (§frames);
- the cost of an instruction fetched from RAM, 4 (§frames);
- the charges of §charges (4 per deferred or core call, 64 per answered
  call, 2 per `write` byte and per sound list byte, 1 per 4 bytes copied,
  150 per clear or scroll command), and the longest `write`, 4,096 bytes
  (§terminal.calls).

What one frame or one list may make a host draw:

- the 2D display's fill cap, 4 × 640 × 480 square units (§vector.fill-cap).

The sizes of what a program hands a device:

- display lists of 1 to 16,384 bytes for the 2D vector display
  (§vector.calls);
- 256 random bytes per call (§system);
- the event queue, 64 events (§events).

The sizes that define machine profile 1 (ROM of 128 KiB, RAM of 1 to 128
pages of 8 KiB, 16 devices in an image, the 640 × 480 screen, the
terminal's 80 × 30 cells, 4 controllers) and the layouts of records, blocks
and events are not placeholders: they are the profile and its device
versions.

## §unspecified Not yet specified

- Font selection, combining marks and complex scripts in the terminal
  (with full text, §terminal.text).
- Device 5.
- A limit on the area covered by lines and circle outlines, including the
  outlines that filled shapes become in the phosphor style and fill mode 1
  (§vector.list). The fill cap does not apply to them, so a legal list of about 3,000 maximum-width lines
  across the whole screen still costs a host far more fill than four
  screens.

Each device document lists what is not yet specified for its device.

---

## §index Index of calls and events

Every call of every device, and every event type, in one place. Each
document has the rows of its own devices (this one those of devices 1, 2
and 3, §calls.table and §events); a test
(`crates/terse-cli/tests/spec.rs`) checks that the rows there and here are
the same.

### §index.calls Calls

| `a7` | Call | Arguments | Result | Class |
|---|---|---|---|---|
| `100h` | `wait_frame` | — | the frame counter in the frame it returns in | core |
| `101h` | `next_event` | `a0` = buffer (4 bytes, written) | 1 and the event, or 0 if the queue is empty | core |
| `102h` | `frame_count` | — | the frame counter | core |
| `103h` | `unix_time` | — | the Unix time latched at the start of the frame | core |
| `104h` | `random` | `a0` = buffer (written), `a1` = length, at most 256 | the length | core |
| `105h` | `controller` | `a0` = controller number, `a1` = buffer (8 bytes, written) | byte 0 of the state | core |
| `106h` | `exit` | `a0` = exit code (low 8 bits) | does not return | core |
| `107h` | `open` | `a0` = address of an open request (read) | 1 passed to the shell, 0 not | core |
| `108h` | `argument` | `a0` = buffer (32 bytes, written) | 1, or 0 if the machine has none | core |
| `109h` | `device` | `a0` = device slot | the device's version, or `FFFF_FFFFh` if the machine does not have it | core |
| `200h` | `write` | `a0` = buffer (read), `a1` = length, at most 4,096 | the length | deferred |
| `201h` | `command` | `a0` = command | 0 | deferred |
| `202h` | `set_cursor` | `a0` = column, `a1` = row | 0 | deferred |
| `203h` | `get_cursor` | — | `a0` = column, `a1` = row | answered |
| `204h` | `set_colours` | `a0` = foreground, `a1` = background | 0 | deferred |
| `205h` | `get_colours` | — | `a0` = foreground, `a1` = background | answered |
| `206h` | `cells_read` | `a0` = first cell, `a1` = buffer (written), `a2` = count | the count | answered |
| `207h` | `cells_write` | `a0` = first cell, `a1` = buffer (read), `a2` = count | the count | deferred |
| `208h` | `palette_set` | `a0` = first entry, `a1` = buffer (read), `a2` = count | the count | deferred |
| `209h` | `size` | — | `a0` = 80 columns, `a1` = 30 rows | deferred |
| `300h` | `submit` | `a0` = list address, `a1` = length | 0 done, 1 rejected | deferred |
| `301h` | `clear` | — | 0 | deferred |
| `302h` | `style` | `a0` = style | 0 done, 1 rejected | deferred |
| `303h` | `persistence` | `a0` = persistence | 0 done, 1 rejected | deferred |
| `304h` | `fill` | `a0` = fill mode | 0 done, 1 rejected | deferred |
| `400h` | `submit` | `a0` = address of a request block | a handle 1–8, or 0 if rejected | answered |
| `600h` | `submit` | `a0` = list address, `a1` = length | 0 done, 1 rejected | deferred |
| `601h` | `wave` | `a0` = slot, `a1` = buffer (64 bytes, read) | 0 | deferred |
| `602h` | `sample` | `a0` = slot, `a1` = buffer (read), `a2` = length, `a3` = loop start | 0 | deferred |
| `603h` | `info` | — | `a0` = 16 voices, `a1` = capabilities | deferred |
| `700h` | `submit` | `a0` = document address, `a1` = length | 0 done, 1 rejected | deferred |
| `701h` | `clear` | — | 0 | deferred |
| `702h` | `region` | `a0` = x, `a1` = y, `a2` = width, `a3` = height | 0 | deferred |
| `703h` | `scroll` | `a0` = y | 0 | deferred |
| `704h` | `background` | `a0` = colour | 0 | deferred |
| `705h` | `locate` | `a0` = anchor id | 0 done, 1 rejected | answered |
| `800h` | `start` | `a0` = address of an image ID (32 bytes, read) | a handle 1–8, or 0 if none is free | core |
| `801h` | `stop` | `a0` = handle | 1, or 0 if the handle is not in use | core |
| `802h` | `place` | `a0` = handle, `a1` = x, `a2` = y, `a3` = width, `a4` = height | 1, or 0 if the handle is not in use | core |
| `803h` | `run` | `a0` = handle, `a1` = 1 run, 0 pause | 1, or 0 if refused | core |
| `804h` | `focus` | `a0` = handle, or 0 | 1, or 0 if refused | core |
| `805h` | `info` | `a0` = handle, `a1` = buffer (48 bytes, written) | 1, or 0 if the handle is not 1–8 | core |
| `806h` | `builtin` | `a0` = index, `a1` = buffer (32 bytes, written) | 1, or 0 past the last | core |
| `807h` | `start_program` | `a0` = address of a program record (read) | a handle 1–8, or 0 if none is free | core |
| `808h` | `identity` | `a0` = handle, `a1` = buffer (81 bytes, written) | 1, or 0 if it has none | core |
| `809h` | `request` | `a0` = handle, `a1` = buffer (82 bytes, written) | 1, or 0 if no request waits | core |
| `80Ah` | `argument` | `a0` = handle, `a1` = address of a blob ID (32 bytes, read) | 1, or 0 if the handle is not loading | core |
| `80Bh` | `snapshot` | `a0` = handle, `a1` = slot | 1 accepted, 0 refused | core |
| `80Ch` | `restore` | `a0` = slot | a handle 1–8, or 0 if none is free or the slot is out of range | core |
| `80Dh` | `snapshot_info` | `a0` = slot, `a1` = buffer (122 bytes, written) | 1 if the slot holds a snapshot, else 0 | core |
| `80Eh` | `snapshot_delete` | `a0` = slot | 1, or 0 if the slot is out of range | core |
| `80Fh` | `face` | `a0` = handle, `a1` = frames, `a2` = x, `a3` = y, `a4` = width, `a5` = height | 1, or 0 if refused | core |
| `810h` | `unface` | `a0` = frames | 1, or 0 if refused | core |
| `900h` | `submit` | `a0` = list address, `a1` = length | 0 done, 1 rejected | deferred |
| `901h` | `clear` | — | 0 | deferred |
| `902h` | `style` | `a0` = style | 0 done, 1 rejected | deferred |
| `903h` | `persistence` | `a0` = persistence | 0 done, 1 rejected | deferred |

### §index.events Events

| Type | Name | `a` | `b` | `c` |
|---|---|---|---|---|
| `01h` | Key down | key code | modifiers | 0 |
| `02h` | Key up | key code | modifiers | 0 |
| `03h` | Text | code point bits 0–7 | bits 8–15 | bits 16–20 |
| `04h` | Pointer | x bits 0–7 | y bits 0–7 | bits 0–1: x bits 8–9; bit 2: y bit 8; bits 3–5: buttons; bits 6–7: 0 |
| `05h` | Content done | request handle | status | 0 |
| `06h` | Wheel | vertical steps, signed | horizontal steps, signed | 0 |
| `07h` | Feeds updated | 0 | 0 | 0 |
| `08h` | Page laid out | height bits 0–7 | bits 8–15 | bits 16–23 |
| `09h` | Page anchor | anchor id bits 0–7 | bits 8–15 | bits 3–5: buttons, as in a pointer event; other bits 0 |
| `0Ah` | Machine changed | handle | the new state | 0 |
| `0Bh` | Home | 0 | 0 | 0 |
| `0Ch` | Machine pressed | handle | buttons | 0 |
| `0Dh` | Page located | anchor id bits 0–7 | bits 8–15 | line height |
| `0Eh` | Page located y | y bits 0–7 | bits 8–15 | bits 16–23 |
| `0Fh` | Open requested | handle | 0 | 0 |
| `10h` | Snapshot done | slot | 1 stored, 3 failed | 0 |
