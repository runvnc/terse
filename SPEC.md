# Terse machine specification — v0 (draft)

Status: draft for the first prototype. Everything here may change until v1;
after v1 a published machine profile and published device versions never
change. Section 16 lists the numbers that are placeholders until hosts have
been measured.

**Terse in three layers.** (a) The Terse network, `NETWORK.md`: blobs,
relays, feeds, entries, profiles and names, search. (b) Terse articles,
`ARTICLE.md`: the article format and what a reader must do with it. (a)
and (b) are **the core**: a native reader that implements them, with no
virtual machine, is a complete, conforming Terse client. (c) The Terse
machine, this document and `STATE.md`: an optional layer that reuses the
core (programs are blobs published in feeds, and the reader program
shows articles on the page device). Nothing in the core depends on this
document.

Terse is a small virtual computer: a 32-bit RISC-V CPU (RV32IMC), a program
of at most 128 KiB, at most 1 MiB of RAM, a fixed budget of work per frame,
and a set of host-side devices reached through system calls. One program runs
in one machine. Programs are identified by the hash of their image and have
no dependencies; a program published in a feed also has an identity, its
publisher and name, that stays the same from one version to the next
(section 12.1). A host may run several machines as one system, in which one
machine, the shell, starts the others and lays out their screens, and
which a machine may ask to open a program or a blob (section 12). A shell
can keep a machine as a snapshot and restore it later (section 12.7).

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
   spent executing instructions (section 14).
5. **Small enough to reimplement.** An independent implementation of this
   document should be a few thousand lines.

All multi-byte values are little-endian. `h` suffix means hexadecimal.
"KiB" is 1,024 bytes.

---

## 1. Machine profile 1

| Property | Value |
|---|---|
| CPU | RV32IMC, little-endian, user mode only (section 1.1) |
| Budget | 125,000 units per frame (section 1.4) |
| Frame rate | 60 frames per second, nominal |
| ROM | 1 to 131,072 bytes at `0001_0000h`, read and execute |
| RAM | 1 to 128 pages of 8 KiB (8 KiB to 1 MiB) at `0010_0000h`, read, write and execute; zero at reset |
| Devices | reached with `ecall` (section 3) |

### 1.1 CPU

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

Everything else faults (section 1.5): `ebreak` and `c.ebreak`; every
reserved or illegal encoding, including the all-zero parcel (`c.unimp`),
`C.ADDI4SPN`, `C.ADDI16SP` and `C.LUI` with a zero immediate, `C.LWSP` and
`C.JR` with register 0, RV32 C shifts with shamt[5] set, `C.SUBW`, `C.ADDW`
and the reserved MISC-ALU codes, every compressed floating-point load or
store, and all 48-bit and longer encodings; CSR instructions, `FENCE.I`,
`MRET`, `WFI` and other privileged encodings.

### 1.2 Memory map

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
once, and it costs more (section 1.4).

### 1.3 Reset state

- `pc = 0001_0000h`, the first byte of ROM.
- `x2` (`sp`) = `0010_0000h + 8,192 × ram_pages`, the end of RAM. Every
  other register is zero.
- All RAM is zero. All devices are in their reset state. The budget is
  zero and the frame counter is 0.

### 1.4 Frames and the budget

Time advances in frames. There are no interrupts. For each frame the host
does the following, in order:

1. Appends this frame's input events to the event queue (section 5),
   latches the clock, the random bytes and the controllers (sections 4 and
   6), and applies the content completions it has for this frame
   (section 9.2). The content-done events of the completions are queued
   after the frame's input events.
2. Adds 125,000 to the budget, subtracts the charge for content delivered
   in step 1 (section 14), and runs the CPU while the budget is greater
   than zero.
3. Ends the frame: the frame counter increases by one, the sound device
   renders the frame's samples (section 18.5), and the host presents the
   terminal, the vector displays and the page as they are at this moment,
   and plays the samples.

Rules:

- An instruction fetched from ROM costs 1 unit; one fetched from RAM costs
  4. An `ecall` costs 1 (or 4 from RAM) plus the charge of the call it makes
  (section 14).
- An instruction that starts with a positive budget runs to completion, so
  the budget can end below zero. A negative remainder carries into the next
  frame; it may be larger than a frame's budget, in which case that frame
  runs nothing and the rest carries on. Unused budget never carries forward.
- When the budget runs out the machine simply pauses: the next frame
  continues with the next instruction, and the program is not told.
- A program that has finished its work for the frame calls `wait_frame`
  (section 4): the rest of the budget is discarded and the call returns at
  the start of the next frame. That frame always runs: after `wait_frame`
  the carry is at most the 8 units of the `ecall` and its call charge,
  and the content delivered at the start of a frame costs at most 32,768
  units (8 requests of 16,384 bytes, section 14).
- `exit` and a fault stop the machine at once (section 1.5). The rest of
  that frame's budget is dropped; the frame still counts. Nothing runs in
  later frames.

A host that falls behind real time lets virtual time lag. It must not run
extra frames to catch up.

In a system of several machines (section 12) each machine has its own
budget, carry and frame counter; section 12.5 says in what order their
frames run.

### 1.5 Stopping and faults

A machine is running, or stopped in one of two ways:

| Stopped state | Cause | Recorded |
|---|---|---|
| Exit | the `exit` call (section 4) | the exit code, 0–255 |
| Fault | one of the faults below | the kind, `pc` and an address |

A fault and an exit are never confused: a fault has no exit code. The
faulting instruction is not executed and is not charged, and `pc` is left
pointing at it.

| Fault kind | Cause | Address recorded |
|---|---|---|
| Illegal instruction | a reserved, illegal or unsupported encoding (section 1.1); the encoding (16 or 32 bits) is recorded too | `pc` |
| Breakpoint | `ebreak`, `c.ebreak` | `pc` |
| Fetch | an instruction, or its second half, is not in ROM or RAM | the address that could not be fetched |
| Load | a load from outside ROM and RAM | the load address |
| Store | a store outside RAM (ROM included) | the store address |
| Misaligned | a load or store whose address is not a multiple of its size | the access address |

A host must show the user that the program faulted, with the kind and the
addresses, and must not present a fault as an exit code.

---

## 2. Image format

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
  section 12.1, and absent elsewhere whatever the header says).
- The **title** is the 16 bytes without their trailing `00h` bytes
  (possibly empty). It should be UTF-8, but a host does not check it: it
  shows what it cannot decode as it can (for example as U+FFFD). A
  built-in program whose title is not a valid display name (section 9.5)
  has no identity `start_program` can name (section 12.2); it can still
  be started by its image ID.
- A host must refuse an image whose profile or required device versions it
  does not implement. This holds for an image a shell starts too: the start
  fails (section 12.2, reason 3) and the machine never runs. A device not
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
  `device` (section 4).
- Device 1 (system) is always present and need not be listed.

| Slot | Device | Version | Status in v0 |
|---|---|---|---|
| 1 | System | 0 | specified (sections 4–6); **reading** and **program profiles** |
| 2 | Terminal | 0 | specified (section 8); **reading** and **program profiles** |
| 3 | 2D vector display | 0 | specified (section 10); **program profile** |
| 4 | Content | 0 | specified (section 9); **reading profile** |
| 5 | Messages | — | reserved |
| 6 | Sound | 0 | specified (section 18) |
| 7 | Page | 0 | specified (section 11); **reading profile** |
| 8 | Machines | 0 | specified (section 12); only the shell of a system has it |
| 9 | 3D vector display | 0 | specified (section 13) |

Other slots are free; 0 and 10–255 are not devices in v0.

### 2.1 Machine hosts and profiles

A **machine host** implements this document: the machine of section 1,
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

- the machine, profile 1 (section 1), and devices 1 System (sections
  4–6), 2 Terminal (section 8) and 3 2D vector display (section 10),
  each complete;
- every other device is optional for it: content, sound, the page, the
  3D display. A program-profile host refuses an image that requires one
  of them and runs, with the device absent, an image that lists it as
  optional.

A program meant to run on every machine host requires only devices 1, 2
and 3 and lists anything else it can do without as optional (for
example, content to keep a high score, sound for a game's effects), and
asks `device` (section 4) at start which of them it has.

A machine host that only wants to run the reader (the built-in feed
reader, `programs/reader`, or any program written for the same profile)
provides the machine's **reading profile**:

- the machine, profile 1 (section 1), and devices 1 System (sections
  4–6), 2 Terminal (section 8), 4 Content (section 9) and 7 Page
  (section 11), each complete except as said next for device 4;
- within Content it must carry out get (`01h`, section 9.2),
  subscriptions (`03h`), timeline (`04h`), names (`09h`), identity
  (`08h`, with the own address's hints), subscribe (`05h`, with the hints
  of a feed address) and unsubscribe (`06h`) (section 9.5), and
  may complete any other operation (put, publish, find, set profile,
  set nickname, programs, and the named records of section 9.6) with
  status `05h` (Refused), without asking and without carrying it out (the
  submit itself is as section 9.2 says: an invalid request is still
  rejected with `04h`, a valid one accepted); a program cannot tell that
  from a user or host that refuses;
- within Page, everything;
- devices 3, 6, 8 and 9 are optional: a reading-profile host need not
  provide them; it refuses an image that requires one and runs one that
  lists them as optional without them (the reader lists none of them).
  Device 8 exists only for the shell of a system (section 12) in any
  case. Nothing a reader shows depends on sound (device 6, section 18).

Sections 9.5 and 9.6 say, for each optional group of operations, that a
host may answer it with `05h`. A program meant for the reading profile
must treat `05h` from those operations as a normal outcome and keep
working. The core of Terse (the network and articles, `NETWORK.md` and
`ARTICLE.md`) does not need a machine at all; the reading profile is
only for hosts that run the reader as a program. `terse run --profile
reading` runs one machine with exactly this profile, and
`programs/conform-reading` is the conformance image for it (section 15);
`terse run --profile program` and `programs/conform-program` do the same
for the program profile. `terse run --without <slot>` withholds an
optional device on any host.

---

## 3. Calls

### 3.1 Convention

A program reaches a device with `ecall`. Register `a7` (`x17`) holds the
call number, `(device slot << 8) | function`; arguments are in `a0`–`a5`;
the result is in `a0`, and a second result in `a1` where listed. A call
changes no register but `a0` (and `a1` where listed) and no memory but the
buffers it is said to write. `pc` continues after the `ecall`.

- An unknown call, or a call to a device the machine does not have (one
  the image does not list, or lists as optional and was not granted,
  section 2), returns `FFFF_FFFFh` and does nothing else.
- A **buffer** is a pointer and a length in bytes. A buffer the device
  reads must lie wholly in ROM or wholly in RAM; one it writes must lie
  wholly in RAM. A buffer of length 0 is valid at any address, for every
  call and for every data range of a content request block (section
  9.1). Otherwise the call returns `FFFF_FFFEh` and does nothing else;
  this is not a fault.
- Five calls answer a buffer that is not where it must be in their own
  way instead: the `submit` of each vector display (sections 10.2, 13.2)
  rejects a list not wholly in RAM with 1, as it rejects a bad length, and
  so does the sound device's `submit` a list not wholly in ROM or RAM
  (section 18.2);
  content `submit` rejects a request block not wholly in RAM with handle
  0 (section 9.2), and a data range in the block that is not where
  section 9.1 requires makes the request invalid (status `04h`); page
  `submit` returns 1 for a bad length before it looks at the buffer
  (section 11.2).
- A call that takes a buffer and other arguments checks the buffer
  first: a bad buffer returns `FFFF_FFFEh` whatever the other arguments
  are (for example `info` of a handle out of range, section 12.2). Page
  `submit` is the one exception, above.
- Buffers need no alignment.
- Every call is charged, also when it fails (section 14).

### 3.2 Call table

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

Sections 4 to 13 and 18 define each call. The class (section 14) sets a call's
base charge: 4 for a **deferred** call, whose result and charge follow from
its arguments, 64 for an **answered** one, whose result or charge depends
on the device's state; the calls of devices 1 and 8 (**core**) are the
machine's own and cost 4.

---

## 4. Device 1 — System

- **`wait_frame`** ends the frame (section 1.4) and returns one more
  than the frame counter at the call. The value is fixed at the call;
  since the next frame always runs (section 1.4), it is the frame counter
  of the frame the call returns in.
- **`next_event`** removes the event at the head of the queue (section 5)
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
  (section 6) and returns byte 0. A controller number above 3 reads as a
  controller that is not connected.
- **`exit`** stops the machine with exit code `a0 & FFh` (section 1.5).
- **`open`** asks the shell to open something. `a0` is the address of an
  **open request**, which must lie wholly in ROM or wholly in RAM:

  | Offset | Size | Field |
  |---|---|---|
  | 0 | 1 | Kind: `1` a blob, `2` a program |
  | 1 | 32 | Kind 1: the blob ID. Kind 2: the publisher's feed key (all zero for a built-in program) |
  | 33 | 1 | Kind 2: the name's length n, 1 to 48 |
  | 34 | n | Kind 2: the name, a valid display name (section 9.5) |

  A request of another kind, or one that does not lie wholly in ROM or
  RAM, or whose name is not valid, is a bad buffer (`FFFF_FFFEh`). If the
  machine is not one a shell started (it runs on its own, or it is the
  shell itself), or a request of this machine still waits (the shell has
  not read it with `request`, section 12.2), the call returns 0 and does
  nothing else. Otherwise the request waits for the shell, which is told
  of it at the start of the next system frame (section 12.5), and the call
  returns 1. What the shell does with it is the shell's business; the
  machine is not told.
- **`argument`** writes the machine's **argument**, the blob ID the shell
  gave it when it started it (section 12.2), to the buffer, 32 bytes wholly
  in RAM (else `FFFF_FFFEh`), and returns 1; a machine that has none (the
  shell gave none, it runs on its own, or it is the shell) gets 0 and
  nothing is written. The argument never changes and is part of the
  machine's state (`STATE.md` 1.13).
- **`device`** returns the version of device `a0` if the machine has it:
  device 1, every device its image requires, and every optional device
  it was granted (section 2); 8 only for a shell (section 12.1). For any
  other `a0`, also one above 255, it returns `FFFF_FFFFh`. The answer is
  fixed at reset, so a program asks once at start and branches on it.

Events, the latched time, the random bytes and controller state are the
machine's only inputs from this device (which optional devices were
granted is fixed at reset, part of the state, `STATE.md` 1.4). A host that records them per frame
(with the content completions of section 9.4) can replay a run exactly:
the events as the host appended them (page events, section 11.5, and
feeds updated included; content-done events follow from the completions
and need no recording), and of the random bytes only those the program
took, since it reads zeros beyond them either way. What else a replay
needs is where the run started: reset, or a restored snapshot (section
12.7), which holds the machine's argument. For a machine that runs on its
own, `open` always returns 0 (or `FFFF_FFFEh` for a bad buffer), so it is
no input; for one a shell started it is (section 12.6). `terse run
--record` writes such a recording and `--replay` replays it (`STATE.md`
5).

## 5. Events

The queue holds at most 64 events. An event that arrives when the queue is
full is dropped.

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
| `0Ah` | Machine changed | handle | new state | 0 |
| `0Bh` | Home | 0 | 0 | 0 |
| `0Ch` | Machine pressed | handle | buttons | 0 |
| `0Dh` | Page located | anchor id bits 0–7 | bits 8–15 | line height |
| `0Eh` | Page located y | y bits 0–7 | bits 8–15 | bits 16–23 |
| `0Fh` | Open requested | handle | 0 | 0 |
| `10h` | Snapshot done | slot | status | 0 |

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
- **Keys a host keeps.** Besides the home key of a system (section 12.4)
  and the keyboard while a consent prompt is shown (section 9.5), a host
  may keep a few keys for its own functions, such as a paste key; such a
  key's press and its release reach no program (the modifier keys held
  with it are still delivered). A host should keep as few as it can and
  say which.
- **Pointer** positions are in screen units (section 7): x is 0–639 and y
  is 0–479. It is sent when the position or a button changes, at most once
  per frame for movement; a button change is sent at once, with the current
  position. Positions outside the screen are clamped to it, and nothing is
  sent when the pointer leaves the host's window. Buttons: primary,
  secondary, middle, from the lowest bit.
- **Wheel:** a positive vertical step means scroll down (towards later
  content); a positive horizontal step means scroll right. At most one wheel
  event is sent per frame, with each value from −127 to 127; the host adds
  up smaller movements until they make a whole step.
- **Content done** is queued when a content request completes (section 9.2).
- **Feeds updated** is sent when the host has stored one or more new feed
  entries since the previous frame (section 9.5). At most one is queued per
  frame; a second in the same frame's input is dropped. It is sent after the
  automatic download of those entries' articles has finished or failed, so
  the timeline's "local" flags are already correct. The download of a
  program's image (`NETWORK.md` 4.4), which starts once its feed is caught
  up, is announced by a feeds-updated event of its own when it ends.
- **Page laid out**, **page anchor**, **page located** and **page located
  y** come from the page device (section 11.5).
- **Machine changed**, **home**, **machine pressed**, **open
  requested** and **snapshot done** are sent only to the shell of a
  system (section 12.4).

## 6. Controllers

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

## 7. Screen units

The screen is 640 units wide and 480 high, with (0, 0) at the top left. A
terminal cell is 8 units wide and 16 high, so the cell at (column, row) covers
x from `column × 8` and y from `row × 16`. Pointer positions, the vector
display and the page use these units. Units are square: the host shows the screen at a
4:3 shape, so a circle is round.

---

## 8. Device 2 — Terminal (version 0)

A grid of 80 columns by 30 rows. The grid is held by the device, not in the
program's memory.

### 8.1 Cells

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

### 8.2 Calls

- **`write`** puts characters (section 8.3) from all the bytes of the
  buffer (`a0` = address, `a1` = length, 0 to 4,096 bytes, in ROM or RAM),
  in order, and returns the length. A length above 4,096 is a bad buffer:
  the call returns `FFFF_FFFEh` and does nothing (section 3.1). The device
  takes the whole buffer whatever the budget; the charge is 2 per byte and
  nothing for the scrolls and clears the bytes cause (section 14), so a call
  that starts with little budget left can take the budget below zero, by
  at most 8,199 units (an `ecall` from RAM, 4, the call's 4 and 8,192 for
  4,096 bytes, started with 1 unit left), which carries into the next
  frame (section 1.4). An empty buffer returns 0. A UTF-8 sequence may be
  split across calls.
- **`command`** carries out a command (section 8.4); a value above 255 or
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

### 8.3 Put character

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

### 8.4 Commands

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

### 8.5 Reset state and palette

Every cell is a space with foreground 7 and background 0. The cursor is at
(0, 0) and shown; pending wrap is clear and no UTF-8 sequence is pending.
The palette is the xterm 256-colour palette: entries 0–15 the standard and
bright colours, 16–231 the 6 × 6 × 6 colour cube, 232–255 the grey ramp.

### 8.6 Presentation

How the host draws the grid (font, size, cursor shape, smoothing) is not part
of this spec and cannot be observed by the program. Section 10.5 says how the
grid is combined with the vector display. Since the scrolls and clears inside
a `write` are not charged, a host should make a scroll or a clear cost time
proportional to one row, not to the whole grid (for example, rows kept as a
ring and a cleared row filled only when it is written to).

---

## 9. Device 4 — Content (version 0)

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

### 9.1 Request block

A request is described by a 52-byte block that must lie wholly in RAM, at
any alignment.

| Offset | Size | Field | Written by |
|---|---|---|---|
| 0 | 1 | `op`: `01h` get, `02h` put (`03h`–`0Dh`: section 9.5; `0Eh`–`11h`: section 9.6) | program |
| 1 | 1 | `status` (section 9.3) | device |
| 2 | 2 | Reserved; ignored | — |
| 4 | 32 | `id`: get and publish: blob hash; subscribe, unsubscribe and set nickname: feed key; save, load and delete: the record's name (section 9.6). Put: the blob hash, written on completion; identity: the host's feed key, written on completion. | both |
| 36 | 4 | `offset`: get: byte offset into the blob; load: byte offset into the record; records (subscriptions, timeline, names, programs, records): index of the first record; find: the scope, 1 to 3; set profile: bit 0 = listed, other bits ignored; publish: the entry's kind, 0 or 1 (article) or 4 (document) | program |
| 40 | 4 | `length`: get, load, records and find: bytes wanted, 1 to 16,384. On completion: bytes delivered, which is fewer if the blob or record ends first. Publish: the title's length, 0 to 120, read at submit. Set profile: the name's length, 1 to 48; set nickname: 0 to 48; subscribe: the hints' length, 0 to 161; read at submit. Identity: the size of a buffer for the own address's hints, 0 (none) to 16,384; on completion, if not 0, the bytes written. | both |
| 44 | 4 | `addr`: address of the data. Get, load, records and find: the `length` bytes there must lie in RAM. Put and save: the `size` bytes there must lie wholly in ROM or wholly in RAM. Publish: the title, `length` bytes of UTF-8, which must lie wholly in ROM or wholly in RAM. Find: also the query, the `size` bytes there, which must lie in RAM. Set profile and set nickname: the name, `length` bytes, which must lie wholly in ROM or wholly in RAM. Subscribe: the hints, `length` bytes, wholly in ROM or wholly in RAM. Identity: the buffer, `length` bytes, which must lie in RAM. | program |
| 48 | 4 | `size`: get: blob size, written on completion; load: the record's size, written on completion. Put: 1 to 1,048,576, read at submit; save: 0 to 65,536, read at submit. Records: the number of records that exist, written on completion. Find: the query's length, 1 to 48, read at submit; the number of results, written on completion. Publish and set profile: the new entry's `seq`, written on completion. | both |

### 9.2 Submit and completion

`submit` (`400h`) with `a0` = the block's address:

1. If the 52 bytes at `a0` do not lie wholly in RAM, the submit is rejected
   (it returns 0) and nothing is written.
2. If the request is invalid (unknown `op`, a length, size, scope or
   publish kind out of range, a data range not wholly where section 9.1 requires, a publish
   title or find query that is not valid UTF-8, a name that is not a valid
   display name, section 9.5, subscribe hints that are not a valid hint
   list, section 9.5, a record name that is not valid, section 9.6),
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
section 1.4, the host may complete any pending requests. Completing a
request:

- get and load, on success: copies the delivered bytes to `addr`, writes
  `length` and `size`;
- subscriptions, timeline, names, programs, records and find, on success:
  copies the records to `addr`, writes `length` and `size` (sections 9.5,
  9.6);
- put, on success: writes `id`;
- subscribe, unsubscribe, set nickname, save and delete, on success: nothing
  more;
- publish and set profile, on success: writes `size`;
- identity, on success: writes `id`, and, if `length` was not 0, the own
  address's hints to `addr` and `length` (section 9.5);
- in every case writes `status`, frees the handle, and queues a content-done
  event (section 5). If the event queue is full the event is dropped; the
  block is still updated.

The device writes to the block at the address given at submit, whatever the
program has since done with that memory. Completions in one frame are
applied in increasing handle order. A get whose `offset` is at or beyond the
end of the blob completes with status `01h`, `length` 0 and `size` the
blob's size; so does a load at or beyond the end of its record, with the
record's size.

**After a restore.** A machine restored from a snapshot (section 12.7)
has the requests that were pending when the snapshot was taken, and no
host will carry them out. Each of them completes with status `03h`
(failed) in the machine's first frame after the restore, in increasing
handle order, before any other completion of that frame; a request that
was waiting for the user's consent completes the same way. Nothing else
of the block is written.

### 9.3 Status values

| Value | Meaning |
|---|---|
| `00h` | Pending |
| `01h` | Done |
| `02h` | Not found |
| `03h` | Failed (network error or timeout; the host decides when to give up) |
| `04h` | Bad request |
| `05h` | Refused: the user said no, the host does not permit it, the host does not provide the operation (a host with only the machine's reading profile, section 2.1: put, publish, find, set profile, set nickname, programs, save, load, delete and records), or the relays refused a find (section 9.5) |
| `06h` | No room: a save would exceed the owner's limits (section 9.6) |

### 9.4 Determinism

Completions (which handle, in which frame, with what status, size and bytes)
are inputs to the machine, alongside those of section 4. A request waiting
for the user's consent (section 9.5) is simply pending; the user's answer
reaches the machine only as its completion. The failed completions of a
restore (section 9.2) follow from the snapshot, not from the host: a
replay that starts from the same snapshot gets them again.

### 9.5 Feeds, names and search

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
section 9.7) of all subscribed feeds, newest first by the order in which
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
arrive, the program is told by a feeds-updated event (section 5) and can
read the timeline again.

**Programs** (`0Dh`) delivers the current version of each program
(`NETWORK.md` section 4.2) of the subscribed feeds, in increasing order of
feed key, then of name (byte order), one record per program in the
timeline's record format; flag bit 0 is set when the image is in the host's
store. The name is the entry's title. A program is started by its identity
with device 8's `start_program` (section 12.2). A host that offers only the machine's reading profile (section 2.1) may complete every programs
request with status `05h`.

**Subscribe** (`05h`) adds the feed whose key is `id` to the host's
subscriptions, and **unsubscribe** (`06h`) removes it (`NETWORK.md` section
4.4). For subscribe, `length` and `addr` may give the **hints** of the
feed's address (`NETWORK.md` section 4.9): the text after its `@`
exactly, one or two hints separated by `,` (`relay.example:9000` or
`a.example,[2001:db8::7]`), 1 to 161 bytes wholly in ROM or wholly in
RAM, copied at submit; `length` 0 gives none. Hints that are not a
valid hint list, or longer than 161 bytes, make the request invalid
(section 9.2: status `04h`). The host keeps them with the subscription
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
blob must start with a valid document header (section 9.7); if it does
not, the request completes with status `04h`, without asking (the host
checks this at completion, since the blob is not the program's to read at
submit). The host signs the entry with its own key, creating
its identity first if it has none. On success `size` is the new entry's
`seq`. A host that offers only the machine's reading profile (section 2.1) may complete every put and publish with status `05h`.

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
to set nickname, must be a valid display name: valid UTF-8, 1 to 48 bytes,
no control characters (U+0000–U+001F, U+007F–U+009F), and no whitespace
(the Unicode White_Space property) as its first or last character.
Otherwise the request is invalid (section 9.2).

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
`03h`. A host that offers only the machine's reading profile (section 2.1) may complete every find with status `05h`.

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
already is as asked. A host that offers only the machine's reading profile (section 2.1) may complete every set profile and set nickname with status
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

### 9.6 Named records

Four operations keep small data on this host for the program: its **named
records**. They use the same request block, call, handles and completion
rules as get and put. A host that offers only the machine's reading profile (section 2.1) may complete every save, load, delete and records request
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
invalid (section 9.2). For records, `id` is not used.

**Owner.** A machine's records belong to its program's identity (section
12.1): a machine started with `start_program`, or a shell the host started
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

Records are inputs like any completion (section 9.4): what a load delivers
and the status of a save are recorded, so a run replays without the
records directory.

### 9.7 Documents

A **document** is a blob that says which program reads it. It starts with
a header:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic: `54h 44h 4Fh 43h` (`TDOC`) |
| 4 | 1 | Version: `0` |
| 5 | 32 | The format owner's feed key; all zero for a built-in program's format |
| 37 | 1 | n: the format name's length, 1 to 48 |
| 38 | n | The format name, a valid display name (section 9.5) |
| 38 + n | — | The data, in the format's own layout (possibly empty) |

A header is valid if all of it is there and every field is as the table
says. The format (owner key, name) is the identity of the program that
defines it (section 12.1): a shell opens a document by starting that
program with the document as its argument (section 12.2). A blob that
starts with `TERS` is an image (section 2); a blob with neither is an
article (`ARTICLE.md`). A document is published as an entry of kind
4 (publish, section 9.5; `NETWORK.md` 4.2).

---

## 10. Device 3 — 2D vector display (version 0)

Draws lines and simple filled shapes from a **display list**: a short
sequence of drawing commands that the program builds in RAM and hands to the
device. The device keeps its own copy and the host draws it every frame until
the program submits another. The device does no geometry for the program: it
does not rotate, scale or clip against anything but the screen, and nothing
it draws can be read back.

### 10.1 Coordinates

Coordinates are signed 16-bit numbers in **sixteenths of a screen unit**
(section 7), little-endian. The screen's top left is (0, 0) and its bottom
right is (10240, 7680). Anything outside the screen is not shown.

### 10.2 Calls

- **`submit`** copies `a1` bytes from address `a0` into the device,
  replacing the previous list, and returns 0. It is rejected, returning 1
  and keeping the previous list, if the length is 0 or more than 16,384 or
  any part of the range is outside RAM.
- **`clear`** empties the list.
- **`style`** sets the display style: 0 clean, 1 phosphor (section 10.6).
- **`persistence`** sets the persistence, 0–255 (section 10.6).
- **`fill`** sets the fill mode: 0 filled shapes are filled, 1 they are
  drawn as outlines (section 10.3).

`style`, `persistence` and `fill` return 1 and change nothing for a value
out of range, else 0. After reset the list is empty, the style is clean, the
persistence is 160 and the fill mode is 0. The list that the device holds at
the end of a frame is the one the host shows for that frame.

### 10.3 Display list

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
  back. The page's figures are outlined too (section 11.4).

### 10.4 Fill cap

When filled shapes are filled (the clean style with fill mode 0), the area
they cover in one list is limited. Each disc, rectangle and triangle, in list
order, counts the area of its bounding box: for a disc the square from
(x − r, y − r) to (x + r, y + r), for a rectangle the rectangle itself, for a
triangle the smallest axis-aligned box holding its three points, with
coordinates as resolved in section 10.3. A disc's square is computed from
its resolved centre without wrapping (x − r and x + r may lie beyond the
16-bit range). The box is clipped to the screen
(0 to 10240 by 0 to 7680), and its area in square screen units is its width
times its height in sixteenths, divided by 256 and rounded up.

The running total starts at 0 for each list. A filled shape whose area would
take the total above 4 × 640 × 480 = 1,228,800 is not drawn, and neither is
any filled shape after it in the list; lines and circle outlines are still
drawn. A shape that is not drawn does not count. Outlines (section 10.3) are
not limited.

### 10.5 Layers

In the clean style the host combines the displays in a fixed order, from the
bottom:

1. the terminal's cell backgrounds;
2. the page (section 11): its background, then its text;
3. the vector displays: device 3's shapes, then device 9's (section 13.5);
4. the terminal's characters and cursor.

So terminal text is always drawn over vector shapes, and a cell's background
colour is always behind them. The page lies between: shapes are drawn over
it, and terminal characters over both (a program that shows the page usually
leaves the terminal cells under its rectangle blank). A program that wants a panel of text to hide the
shapes behind it draws a filled rectangle at the end of its list; the text
still appears above it. Smoothing of edges and the exact colour of blended
pixels are up to the host and cannot be observed by the program.

In a system (section 12) each machine's screen is combined in this order on
its own, with its own style; section 12.3 says how the screens are combined.
A machine whose 3D list has Surfaces in effect (section 13.3) is drawn as
section 13.5 says: these layers, less the Surfaces' sources, then the
Surfaces, then its 3D lines.

### 10.6 Phosphor style

In the phosphor style the whole screen imitates a vector monitor or
oscilloscope tube:

- The screen is black: cell background colours are not drawn.
- Shapes are drawn as outlines (section 10.3) and add light to what is
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

### 10.7 Determinism

The device's state is the style, the persistence, the fill mode and the copy
of the list. It takes no input from the host. The style and the persistence
are also set by device 9 (section 13.2): one setting for both displays.

---

## 11. Device 7 — Page (version 0)

Shows typeset prose: proportional fonts, shaped, bidirectional and broken
into lines at the width of a rectangle on the screen, with figures drawn
from paths between the paragraphs. The program hands the device a
**document**, a short binary list of styled text and figures much like a
display list, and a rectangle; the host lays the text out with its own fonts
and draws the part that is scrolled into view. The device parses no markup
language and does no layout for the program.

Layout is the host's business and differs between hosts, because fonts
differ. The only facts about it a program learns are the laid-out height of
the document and, when it asks, where an anchor is, and it learns them
through input events (section 11.5), so a recorded run replays exactly
whatever the fonts.

### 11.1 Rectangle and scrolling

The page is drawn inside its **rectangle** (x, y, width, height in screen
units, section 7) and nothing of it outside. The document is laid out as one
column as wide as the rectangle; the **scroll value** is the distance in
screen units from the top of the document to the top of the rectangle. A
rectangle with a width or height of 0 hides the page.

### 11.2 Calls

- **`submit`** copies `a1` bytes from address `a0` into the device,
  replacing the document, sets the scroll value to 0, and returns 0. If the
  length is 0 or more than 65,536 it returns 1 (rejected) and changes
  nothing. Otherwise the bytes must lie wholly in ROM or wholly in RAM; if
  they do not, the call returns `FFFF_FFFEh` and changes nothing (section
  3.1). A submit of the same bytes again is still a new document
  (section 11.5).
- **`clear`** removes the document: the page shows only its background.
  The scroll value is kept.
- **`region`** sets the rectangle to x = `a0`, y = `a1`, width = `a2`,
  height = `a3`, clamped so it lies on the screen: x to at most 640, y to at
  most 480, the width to at most 640 − x and the height to at most 480 − y.
- **`scroll`** sets the scroll value to `a0`, stored as given (32 bits).
  When it presents the page, the host takes the value as a signed number and
  clamps it to 0 … max(0, height − rectangle height), where height is the
  document's laid-out height; the stored value does not change.
- **`background`** sets the colour behind the text in the rectangle:
  `a0` = r | g << 8 | b << 16 | a << 24, where `a` is opacity (255 opaque,
  0 transparent, so the cell backgrounds show through).
- **`locate`** asks where anchor `a0` is laid out in the current document;
  the host answers later with the two page located events (section 11.5).
  It returns 0, or 1 (rejected, and nothing is asked) if `a0` is 0 or above
  65,535, if there is no document, or if the program has already made 8
  locates in this frame.

After reset there is no document, the rectangle is (0, 0, 0, 0) (hidden),
the scroll value is 0 and the background is `FF00_0000h`, opaque black.

### 11.3 Document

A document is 1 to 65,536 bytes: a sequence of commands, each an opcode byte
followed by its operands, little-endian. It ends at an End command, at the
end of the bytes, at an unknown opcode, at a command whose operands are
cut off or at a Table command whose column count is 0 or above 8; the
commands before that are used.

| Opcode | Command | Operands | Effect |
|---|---|---|---|
| `00h` | End | — | Ends the document |
| `01h` | Text | n (2 bytes), then n bytes of UTF-8 | Adds the text to the current paragraph in the current font, size, colour and anchor |
| `02h` | Font | face (1 byte: 0 sans, 1 serif, 2 monospace), flags (1 byte: bit 0 bold, bit 1 italic) | Sets the font for later text; an unknown face is 0; other flag bits are ignored |
| `03h` | Size | size (1 byte) | Sets the font size in screen units, clamped to 6–96 |
| `04h` | Colour | r, g, b | Sets the text colour |
| `05h` | Paragraph | align (0 start, 1 centre, 2 end; anything else is 0), indent (1 byte), space (1 byte), flags (1 byte) | Starts a new paragraph with these properties |
| `06h` | Rule | — | Starts a new paragraph that is a horizontal line across the rectangle, in the current colour |
| `07h` | Anchor | id (2 bytes) | Later text and figures belong to anchor `id`; 0 is no anchor |
| `08h` | Figure | width (2 bytes), height (2 bytes), align (1 byte), n (2 bytes), then n bytes of path commands | A figure: a block of its own, width × height screen units |
| `09h` | Table | columns (1 byte, 1–8), flags (1 byte: bit 0 header), then one align byte per column (0 start, 1 centre, 2 end; anything else is 0) | Starts a table: a block of its own, a grid of cells |
| `0Ah` | Cell | — | Starts the table's next cell |
| `0Bh` | End table | — | Ends the table |

State at the start of a document: sans, no flags, size 14, colour
(229, 229, 229), anchor 0, and an implicit first paragraph that is
start-aligned with indent 0, space 0 and wrapping. Font, size, colour and
anchor carry on across paragraphs.

- **Text.** Each Text command is decoded on its own: malformed UTF-8 becomes
  U+FFFD as in the terminal (section 8.3; a sequence split across two Text
  commands is malformed in both), but code points above U+FFFF are kept.
  U+000A breaks the line inside the paragraph; U+0009 is a space; other
  control characters (U+0000–U+001F, U+007F–U+009F) are ignored. Consecutive
  spaces are kept.
- **Paragraph properties.** `indent` is in screen units from the start edge
  and applies to every line of the paragraph. `space` is the gap in screen
  units above the paragraph, also for the first paragraph and also for a
  paragraph with no text (which takes no other room), so an empty paragraph
  is a vertical gap. Flags bit 0 is **no wrap**: lines break only at U+000A,
  and a line wider than the rectangle is cut off at the rectangle's edge.
  Flags bit 1 is **ellipsis**, used only with no wrap: a line wider than
  the rectangle less the indent ends in an ellipsis (U+2026) where it is
  cut, so that the ellipsis is wholly shown. Other flag bits are ignored.
- **Start and end** follow each paragraph's text direction, which the host
  takes from its text (the first strong character, as in the Unicode
  bidirectional algorithm): left for left-to-right text, right for
  right-to-left text. Indent is measured from that start edge.
- **Figure.** Width and height are clamped to 1–1024; align is 0 start
  (left), 1 centre, 2 end (right), anything else 0, whatever the
  direction of the text around it. A figure ends the
  current paragraph and is a block of its own, as high as its height, with
  the space of the latest Paragraph command (or of the implicit first
  paragraph) above it; if the current paragraph has no text yet, the figure
  takes its place instead and that paragraph's space is the space above the
  figure. Text after a figure starts a new paragraph as after a rule. The
  figure is placed in the rectangle's whole width, without indent: centre
  puts half the leftover width before it, end all of it. A figure wider than
  the rectangle starts at its left edge and is cut off at its right edge,
  not scaled. Drawing is clipped to the figure's box. The colour and anchor
  in effect when the Figure command appears belong to the figure.
- **Rule.** A rule paragraph is 8 screen units high with a line about one
  unit thick across the whole width of the rectangle at its middle; it has
  no space above it (put an empty Paragraph before it for a gap). Text that
  follows a rule starts a new paragraph with the properties of the latest
  Paragraph command (or those of the implicit first one).
- **Table.** A table is a block of its own, a grid of cells laid out as
  section 11.4 says. It ends the current paragraph and takes its space as
  a figure does: the space of the latest Paragraph command (or of the
  implicit first paragraph) above it, or, if the current paragraph has no
  text yet, it takes that paragraph's place and its space. Other flag bits
  are ignored. The cells come in rows of `columns` cells: each Cell
  command starts the next cell, in row order (the first Cell starts the
  first cell of the first row), and the last row's missing cells are
  empty. In a table, Text adds to the current cell in the current font,
  size, colour and anchor, and Font, Size, Colour and Anchor work as
  anywhere; text before the table's first Cell command is ignored. U+000A
  breaks the line in a cell. A table ends at an End table command, and
  also at a Paragraph, Rule, Figure or Table command, which then does what
  it does, and at the end of the document. Text after a table starts a new
  paragraph as after a rule. A table with no cells takes its space and
  nothing else. The colour in effect at the Table command is the colour of
  the table's rules. With flag bit 0 the first row is a **header**: its
  text is drawn bold, whatever the bold flag of its font, and a rule is
  drawn under it. A Cell command or End table outside a table does
  nothing.
- **Table limits.** A table holds at most 1,024 cells and a document at
  most 4,096 cells in all its tables. A Cell command that would pass
  either limit, and the text after it up to the end of its table, are
  ignored (Font, Size, Colour and Anchor commands still change the state).
- **Line height** is the host's natural line height for the font and size;
  a line's height is that of the largest text on it. A line break at the end
  of a paragraph's text gives an empty last line.
- **Anchors** mark text and figures a pointer press reports, and that
  `locate` finds (section 11.5); the host draws anchored text like any
  other text, in its colour.

**Path commands.** A figure's n bytes are a sequence of path commands, each
an opcode byte followed by its operands. Coordinates are signed 16-bit
numbers in sixteenths of a screen unit from the figure's top left corner;
colours are r, g, b, a (1 byte each, `a` the opacity).

| Opcode | Command | Operands | Effect |
|---|---|---|---|
| `01h` | Fill | r, g, b, a | Ends the path; later paths are filled in this colour, not filled if `a` is 0 |
| `02h` | Stroke | r, g, b, a, width (2 bytes, unsigned, sixteenths) | Ends the path; later paths are stroked in this colour and width, not stroked if `a` or the width is 0 |
| `03h` | Move | x, y | Starts a subpath at (x, y) |
| `04h` | Line | x, y | A straight segment to (x, y) |
| `05h` | Quadratic | x1, y1, x, y | A quadratic Bézier segment to (x, y) with control point (x1, y1) |
| `06h` | Cubic | x1, y1, x2, y2, x, y | A cubic Bézier segment to (x, y) with control points (x1, y1), (x2, y2) |
| `07h` | Close | — | A straight segment back to the subpath's start, which ends the subpath; nothing if no subpath is open |

- At the start of the commands: no fill; stroke in the document's text
  colour at the Figure command, opaque, 16 (one unit) wide; current point
  (0, 0). The path commands end at the end of the n bytes, at an unknown
  opcode or at a command whose operands are cut off; the commands before
  that are used.
- Each segment starts at the current point and ends at its end point, which
  becomes the current point. A segment with no subpath open (at the start,
  after a Close, a Fill or a Stroke) starts a subpath at the current point.
  After a Close the current point is the start of the subpath it closed.
- A **path** is the subpaths between two Fill or Stroke commands (or the
  start or end of the commands). Each path is filled with the non-zero rule,
  then stroked, centred on its outline, with round joins and round ends;
  later paths are drawn over earlier ones, blended by opacity. A path with no
  segment, or with neither a fill nor a stroke, draws nothing.
- **Caps.** Paths are drawn in order and counted against three caps per
  figure; the first path that would take a total above its cap is not drawn,
  and neither is any path after it. A path that draws nothing (no segment,
  or neither a fill nor a stroke) is skipped and counts against no cap.
  (1) At most 256 paths. (2) Area: a path counts the box of all its points
  (each Move's point, each segment's start, control and end points),
  clipped to the figure's box, once if it is filled, and that box grown
  on every side by half the stroke width in sixteenths, rounded up, and
  clipped, once more if it is stroked; each box's area in square screen
  units is its width times its height in sixteenths, divided by 256 and
  rounded up. The total is at most 8 × width × height of the figure and at
  most 4 × 640 × 480 = 1,228,800. (3) Length: a path's length L is, over
  its Line, Quadratic and Cubic commands, the sum of |dx| + |dy| of each
  step from the segment's start through its control points to its end, in
  sixteenths; s is the number of those commands. A Close counts in
  neither, though it draws a segment. A path's count is 2 × L + 4 × 16 × s
  if it is filled, plus 2 × L + 4 × w × s with w its stroke width if it is
  stroked; that sum is divided by 16 and rounded up once, and the total is
  at most 65,536. The caps are counted from the path commands alone, the
  same in every style and fill mode: a figure drawn as outlines (section
  11.4) still counts its fills.

### 11.4 Layout and presentation

The host lays the document out with proportional fonts it chooses for the
three faces: shaping (ligatures, joining scripts such as Arabic), the
Unicode bidirectional algorithm within each line, Unicode line breaking at
the rectangle's width less the paragraph's indent, and fallback to other
fonts for characters the chosen font lacks. It draws the background over the
rectangle, then the lines and figures that intersect the rectangle after
scrolling, clipped to it. How the text looks (fonts, hinting, smoothing, how
a line that does not fit is cut) and how figures are smoothed are up to the
host and cannot be observed by the program.

**Tables.** A table is laid out in these steps, with lengths in screen
units (a host measures text with its fonts; the reference layout,
section 11.6, says how it measures):

1. **Padding.** Each cell has 6 units of padding at its left and right
   and 3 at its top and bottom.
2. **Natural widths.** A cell's natural width is the width of its widest
   line when its text is broken only at U+000A, plus 12; a cell with no
   text has 12. A column's natural width is the largest of its cells'.
3. **Column widths.** If the columns' natural widths add up to at most
   the rectangle's width W, each column is its natural width. Otherwise
   the widest columns are narrowed first, all to the same width: each
   column's width is the smaller of its natural width and a cap C, where
   C is the largest width for which these widths add up to at most W; but
   C is never less than 48. (When C is 48 the table may be wider than the
   rectangle: it is cut off at the rectangle's right edge, like a
   figure.)
4. **Rows.** Each cell's text is laid out as a paragraph (wrapped, no
   indent, aligned as its column says, start and end following the
   cell's text direction as a paragraph's do) at its column's width less 12, its
   first line 3 units below the top of its row. A row is as high as the
   tallest text of its cells plus 6 (a cell with no text counts 0). The
   table is as high as its rows.
5. **Placing.** The columns are placed side by side from the rectangle's
   left edge, the first column leftmost, whatever the direction of the
   text in the cells; the rows one below another.
6. **Rules.** Rules take no room: each is a line about one unit thick
   across the table's width, in the table's rule colour, centred on the
   bottom edge of a row. A table with a header (flag bit 0) has a rule
   under its first row, also when that is its only row. Every other row
   but the last has a faint rule under it: drawn at about a third of the
   colour's opacity, so the page's background shows through. There is no
   rule above the first row or under the last (but for a header that is
   the only row).

A line of a cell is a line for the page's events (section 11.5): a press
on anchored text in a cell reports its anchor, and `locate` gives the
first line that carries the anchor, lines being taken in order of their
tops (for a table row, its cells' lines from the left to the right).

Figures are drawn as outlines when the vector display is in the phosphor
style or in fill mode 1 (section 10.3; an image without the vector display
has the clean style and fill mode 0): nothing is filled, and a path that is
filled but not stroked is stroked one unit wide in its fill colour.

Cost stays bounded: a document is at most 65,536 bytes and holds at most
4,096 table cells (section 11.3); the host lays it out
outside the machine's frames (a host may take several frames, on another
thread, while the machine runs on and the previous layout or nothing is
shown); the machine never waits for a layout and is not charged for it; the
host draws only the lines and figures in view, so scrolling needs no new
layout; and the caps of section 11.3 bound the work of drawing each figure,
which a host may also do outside the frames and only for the figures near
the view (showing a figure some frames late).

### 11.5 Events

- **Page laid out** (`08h`): `a`, `b`, `c` are bits 0–7, 8–15 and 16–23 of
  the document's height in screen units at the rectangle's current width,
  FFFFFFh if it is larger. The host sends it at the start of a frame after
  the one in which the program submitted a document, after a clear (height
  0), and again whenever the height changes because the rectangle's width
  changed. While the rectangle's width is 0 a document is not laid out, and
  the event waits until there is a width. A host may skip intermediate
  layouts when several changes come quickly, and then reports only the
  latest, but it must eventually report the height for the latest document
  and width once they stay unchanged. At most one is sent per frame.
- **Page anchor** (`09h`): sent when a pointer button is pressed while the
  pointer is over text whose anchor is not 0, as the page was last shown:
  `a`, `b` = the anchor id, low then high byte; `c` = the buttons in bits
  3–5, the same bits as in a pointer event, and its other bits 0; also when it is pressed over a figure
  whose anchor is not 0. It follows the pointer event of that press, which
  is sent as usual. "Over text" means within the extent of the text's
  glyphs along the line and within the line's height; "over a figure",
  within its box as shown (cut off at the rectangle's edge).
- **Page located** (`0Dh`) and **page located y** (`0Eh`) answer a
  `locate`, always as this pair, one after the other: `0Dh` has `a`, `b` =
  the anchor id, low then high byte, and `c` = the height in screen units
  of the first line, or figure, of the document that carries the anchor
  (255 if it is larger); `0Eh` has `a`, `b`, `c` = bits 0–7, 8–15 and 16–23
  of y, the distance in screen units from the top of the document to the
  top of that line or figure (FFFFFFh if it is larger). If no text or
  figure carries the anchor, the height is 0 and y is FFFFFFh. Lines are
  in document order, so the first line is the topmost one. A host gives
  whole units: the top rounded down, and a height that reaches the bottom
  rounded up. The two are queued together: if the queue has room for only
  one, neither is queued.
- A locate is answered from the layout of the document current at the call,
  the same layout as the latest height event (at the rectangle's width of
  that layout): at the start of a frame after the one of the call, in the
  same frame as that document's height event (after it) if that is still to
  come, else as soon as the host has the layout. Answers come in the order
  of the calls. A locate whose document is replaced (by a submit or a
  clear, also later in the frame of the call) before it is answered is
  dropped and never answered. While the rectangle's width is 0 the answer
  waits, like the height event; a host keeps at least the 8 latest locates
  that wait and may drop older ones.

A height event describes the document and width as they were at the end of
an earlier frame. A program that submits again before reading it gets
another one later, so the last height event it has read is the height of
the last document only once no newer change is pending; a program should
use it for clamping and showing progress, never for anything that must be
exact. A located anchor, like the height, is a fact about one layout that
the host may show differently later (after a change of width). All these
events are ordinary inputs (section 4): recorded with the rest, they make
replay exact although layout differs between hosts.

### 11.6 Reference layout

A host without fonts (such as a headless test runner) lays out the
document with this **reference layout**, which is a pure function of the
document and the width; it is what `terse run` uses, and what makes its
height events reproducible. It is a stand-in for testing and is not what
users see.

- Every character advances 0.5 × size; a wide character (section 8.1; every
  code point above U+FFFF counts as wide) 1.0 × size. There is no
  shaping, no kerning and no bidirectional reordering: all text runs left to
  right, and start is left.
- A line is ceil(1.25 × size) high for the largest size on it; an empty line
  has the size in effect at its line break.
- Wrapping is greedy at spaces (U+0020): a word goes on the line if the line
  so far, the spaces before the word and the word fit in the width less the
  indent; otherwise the line ends (the spaces are dropped) and the word
  starts the next one. A word wider than a whole line is split between
  characters, at least one character per line. Spaces at the start of a
  paragraph or after a line break are kept.
- Centre puts a line's leftover width, halved and rounded down, before it;
  end puts all of it there. Paragraph space and indent are as in section
  11.3; a rule is 8 units high.
- A no-wrap line with ellipsis that is wider than the width less the
  indent loses characters from its end until the rest and an ellipsis
  (U+2026, in the style of the last character dropped) fit, or none is
  left; then the ellipsis is added.
- A figure is a line as high as the figure, placed as in section 11.3 (the
  leftover width halved and rounded down for centre); a press within its
  box hits its anchor.
- A table is laid out as section 11.4 says, with widths in half units:
  a line's width is the sum of its characters' advances, C is the largest
  whole number of half units that fits, and a cell's lines are wrapped,
  aligned as a paragraph's are (greedy wrapping at spaces). Its rules
  are lines of height 0 at the rows' bottom edges.
- A locate gives the y and height of the first line (a figure is a line)
  with a character or figure of that anchor; a table's lines are taken in
  order of y, and lines of the same y from left to right.
- The height is the sum of the spaces, line heights, rules, figures and
  tables.

### 11.7 Determinism

The device's state is the document bytes (or no document), the rectangle,
the scroll value and the background. Nothing about layout is machine state,
nor are the locates the program made: what the layout means for the
program arrives only as the events of section 11.5.

---

## 12. Device 8 — Machines (version 0)

A host may run several machines as one **system**. One of them, the
**shell**, starts the others, decides where their screens are shown and
which of them receives the keyboard, through device 8. The host provides
the mechanism (running the machines, drawing their screens, routing input);
how screens are laid out and what the user is offered is the shell's
business.

### 12.1 The shell and its machines

A host starts a system with one image, the shell. The shell has device 8 if
its image lists (8, 0). Every other machine has device 8 absent (its calls
return `FFFF_FFFFh`, section 3.1), also if its image lists it; so does an
image a host runs on its own, outside a system. How the user names the shell
is up to the host.

The shell starts machines under **handles** 1 to 8. Each is a complete,
separate machine: its own ROM, RAM, devices, event queue, content request
handles (section 9), budget and frame counter. Machines share nothing but
what the host keeps behind the content device (its store, feeds, identity).
Each machine's screen is the whole 640 × 480 units (section 7).

A handle is **in use** from the `start` that returns it until a `stop` of it,
and is then in one of these states:

| Value | State | Meaning |
|---|---|---|
| 0 | none | not in use |
| 1 | loading | started; the host does not have the image yet |
| 2 | running | runs a frame in every system frame |
| 3 | paused | runs no frames and receives no input |
| 4 | exited | stopped by `exit` (section 1.5) |
| 5 | faulted | stopped by a fault (section 1.5) |
| 6 | failed | the image could not be had, is not an image the host can run, the host has no room for another machine, or a snapshot could not be restored; `info` gives the reason |

Each handle also has a **run flag**, a **rectangle** (x, y, width,
height in the shell's screen units) and possibly an **identity**. The
system has a **focus**: a handle, or 0 for the shell; it is 0 at reset;
and a **view** of the shell's 3D scene (`face`, below): none at reset.

A program's **identity** is its publisher's feed key and its name: the
(feed key, title) of a program entry (`NETWORK.md` section 4.2). The
host's built-in programs have the identity (32 zero bytes, image title). A
machine started with `start_program` has that identity for its whole life,
whatever version of the program it runs; one started with `start` (by blob
ID), and an image a host runs from a file, have none; a restored machine
has the identity of the machine its snapshot was taken of. A shell the host
starts as its own built-in shell has the identity of a built-in. Section
9.6 says what the identity is for.

### 12.2 Calls

- **`start`** reads the 32-byte image ID at `a0`, which must lie wholly in
  ROM or wholly in RAM (else `FFFF_FFFEh`). If every handle is in use it
  returns 0. Otherwise the lowest handle not in use becomes loading, with the
  run flag set, the rectangle (0, 0, 0, 0) and that image ID, and the call
  returns it. The host then looks for the blob with that ID, in its store
  first and then as section 9 does for a get, and checks it as an image
  (section 2); a later frame takes the result into account (section 12.5):
  the handle becomes running (paused if the run flag is clear) with a
  machine reset with that image (section 1.3), or failed. The host may take
  any number of frames to get the image.
- **`start_program`** reads a program record at `a0`: the publisher's feed
  key (32 bytes; all zero for a built-in program), the name's length n (1
  byte) and the name (n bytes, a valid display name, section 9.5, so n is
  1 to 48). The 33 +
  n bytes must lie wholly in ROM or wholly in RAM and the name must be
  valid, else the call returns `FFFF_FFFEh` and does nothing else. If every
  handle is in use it returns 0. Otherwise the lowest handle not in use
  becomes loading, with the run flag set, the rectangle (0, 0, 0, 0), image
  ID zero and that identity, and the call returns it. The host then finds
  the current version of the program that it holds (`NETWORK.md` section
  4.2; for the zero key, the first built-in program whose image title is
  the name), gets its image as for `start` and checks it; a later frame
  takes the result into account: the handle becomes running or paused,
  with the record's image ID that of the version started, or failed, with
  reason 1 if the host holds no such program or cannot get its image. The
  host may take any number of frames.
- **`identity`** writes the identity of handle `a0` to the buffer, 81
  bytes wholly in RAM (else `FFFF_FFFEh`): the feed key (32), the name's
  length (1) and the name, padded with `00h` to 48 bytes; it returns 1. For
  a handle not in use or with no identity it writes nothing and returns 0.
- **`stop`** with a handle in use: the machine ceases to exist and the
  handle is not in use (state none, run flag clear, rectangle and record
  zero); if it had the focus, the focus becomes 0. Its pending content
  requests are never completed, and the host withdraws its consent prompts
  for them. Returns 1, or 0 for any other `a0`.
- **`place`** sets the rectangle of a handle in use, clamped to the screen as
  the page's `region` is (section 11.2). Returns 1, or 0 (and nothing
  changes) for a handle not in use.
- **`run`** with a handle that is loading, running or paused: `a1` = 1 sets
  the run flag, 0 clears it; a running handle becomes paused, a paused one
  running, at once; a loading one keeps its state. Returns 1; returns 0 and
  changes nothing for any other handle or any other `a1`.
- **`focus`** with `a0` = 0 gives the focus to the shell; with a handle that
  is loading, running or paused, to that handle. Returns 1; returns 0 and
  changes nothing for any other `a0`.
- **`info`** writes a 48-byte record of handle `a0` to the buffer, which must
  lie wholly in RAM (else `FFFF_FFFEh`), and returns 1; for `a0` outside 1–8
  it writes nothing and returns 0.

  | Offset | Size | Field |
  |---|---|---|
  | 0 | 1 | State |
  | 1 | 1 | Exit code if exited, else 0 |
  | 2 | 1 | Fault kind if faulted, numbered as in `STATE.md` 1.3 (1 illegal instruction … 6 misaligned), else 0 |
  | 3 | 1 | Why the start failed, if failed: 1 not found, 2 not a valid image, 3 the image needs a device (slot, version) the host does not provide, 4 no room for another machine, 5 no snapshot in the slot, or one that cannot be read or does not fit its image (`restore`); else 0 |
  | 4 | 4 | Units the machine used in its last frame (section 1.4, delivered content included), 0 before its first |
  | 8 | 4 | The machine's frame counter |
  | 12 | 4 | Fault `pc` if faulted, else 0 |
  | 16 | 32 | The image ID given to `start`; for `start_program` and `restore`, zeros while loading, then the ID of the image the host got (also when it failed as not an image the host can run, or as not fitting the snapshot); zeros for a handle not in use |

  The record is as the host latched it at the start of the frame (section
  12.5), with the changes the shell's own calls made since.
- **`builtin`** writes the image ID of the host's built-in program number
  `a0` to the buffer, 32 bytes wholly in RAM (else `FFFF_FFFEh`), and
  returns 1; past the last program it writes nothing and returns 0. The
  built-in programs are images the host has in its store, in an order it
  chooses, the same for the whole run. A shell learns a program's title by
  getting the first 32 bytes of its image with the content device.
- **`request`** reads the open request (section 4, `open`) machine `a0`
  made: if one waits and the shell has been told of it (an open requested
  event, section 12.4), it writes 82 bytes to the buffer, which must lie
  wholly in RAM (else `FFFF_FFFEh`): the kind (1), then for kind 1 the
  blob ID (32) and zeros, for kind 2 the program record as `identity`
  writes it (feed key, name length, name padded with `00h` to 48 bytes).
  The request no longer waits (the machine may make another) and the call
  returns 1. Otherwise, and for `a0` outside 1–8, it writes nothing and
  returns 0. A `stop` of the handle, and a `start` that reuses it, drop
  its request. What the shell does next is its own policy.
- **`argument`** reads a blob ID, 32 bytes wholly in ROM or wholly in RAM
  at `a1` (else `FFFF_FFFEh`). If handle `a0` is loading (so the machine
  has not run its first frame), the ID becomes the machine's argument
  (section 4, `argument`), replacing one given before, and the call
  returns 1; otherwise it returns 0 and changes nothing. The argument is
  given to the machine when it becomes running or paused (section 12.5).
  A handle being restored (`restore`) takes no argument: the call returns
  0 and changes nothing, and the machine keeps the argument in its
  snapshot. The device tells such a handle from its record: loading, with
  image ID zero and no identity; a `start` of the all-zero image ID, which
  can never be found, looks the same and is treated the same.
- **`snapshot`** with a handle that is running or paused and a slot 0–15:
  the host keeps the machine's snapshot (section 12.7) in that slot,
  replacing what was there, taken at the end of this system frame (step
  6 of section 12.5, after the machine's own frame if it runs one). Returns
  1; returns 0 and does nothing for any other handle or slot. The result
  arrives as a snapshot done event (section 12.4): status 1 stored, 3
  failed (the machine was stopped in the same frame, or the host could not
  store it); the values are content's done and failed (section 9.3). A
  snapshot the shell deletes with `snapshot_delete` later in the same
  frame is stored and then deleted: its event says 1, and the slot is
  empty.
- **`restore`** with a slot 0–15: if every handle is in use it returns 0.
  Otherwise the lowest handle not in use becomes loading, with the run
  flag set, the rectangle (0, 0, 0, 0), image ID zero and no identity,
  and the call returns it. The host reads the slot's snapshot and gets the
  image it names, in its store first and then as section 9 does for a get;
  a later frame takes the result into account (section 12.5): the handle
  becomes running (paused if the run flag is clear) with the machine as
  it was when the snapshot was taken, its record's image ID that of the
  snapshot and its identity that of the snapshot, or failed: reason 5 if
  the slot holds no snapshot, one the host cannot read, or one that does
  not fit its image (`STATE.md` 4), reasons 1–3 as for `start` if the
  image cannot be had or run, reason 3 also if the snapshot's machine was
  granted an optional device (section 2) this host cannot give it. A
  machine restored where the host has an optional device the snapshot's
  machine was not granted runs without it. The host may take any number
  of frames. A slot out of range returns 0.
- **`snapshot_info`** writes a 122-byte record of slot `a0`, as the host's
  snapshots are at the start of the frame with the changes the shell's
  own `snapshot_delete` calls made since, to the buffer, which must lie
  wholly in RAM (else `FFFF_FFFEh`), and returns 1 if the slot holds a
  snapshot; for an empty slot the record is all zero and it returns 0;
  for a slot out of range it writes nothing and returns 0.

  | Offset | Size | Field |
  |---|---|---|
  | 0 | 1 | Present: 1, or 0 for an empty slot |
  | 1 | 32 | The image ID |
  | 33 | 81 | The identity, as `identity` writes it; all zero for none |
  | 114 | 4 | The machine's frame counter when it was taken |
  | 118 | 4 | When it was taken: the Unix time latched for the machine's last frame, as its state holds it (`STATE.md` 1.5: 0 if it never ran; for a restored machine that has not run since, the time of the snapshot it was restored from) |

- **`snapshot_delete`** with a slot 0–15: the slot is empty from now on
  (for `snapshot_info` at once); the host removes its snapshot at the end
  of the frame, in order with the snapshots asked for in the same frame.
  Returns 1, or 0 for a slot out of range.
- **`face`** moves the host's view of the shell's 3D scene (section 12.3)
  so that machine `a0`'s panel comes to lie flat on the glass, over `a1`
  frames (0–600), and shows it there as a flat picture in the rectangle
  `a2`–`a5` (x, y, width, height, clamped as `place` clamps). It is
  refused (returns 0, nothing changes) unless `a0` is 1–8, its machine
  has a picture (running, paused, exited or faulted), its own rectangle
  shows no picture (section 12.3), the shell's 3D list as it holds it now
  has a Screen of `a0` (section 13.3; decoded only, the caps not counted),
  `a1` is at most 600 and the rectangle shows a picture. Otherwise the view
  becomes: handle `a0`, towards the glass, that rectangle, start frame
  the shell's frame counter in this frame, `a1` frames, and as its start
  progress the view's progress now if it already moved `a0`'s panel,
  else 0. Returns 1.
- **`unface`** moves the view back to the scene as listed over `a0`
  frames (0–600), from its progress now: the direction becomes back, the
  start progress the progress now, the start frame this frame's, the
  duration `a0`. Refused (0) if `a0` is above 600 or there is no view.
  Returns 1.

  The view's **progress** is a number from 0 (the scene as listed) to
  4,096 (the panel flat on the glass). Shown after the shell's frame with
  counter f, with start progress p0, start frame s and duration d, it is
  p0 + (e − p0) × k ÷ d, rounded towards zero, for k = f − s < d (as
  32-bit unsigned numbers), and e from k = d on (also for d = 0), where e
  is 4,096 towards the glass and 0 back. The view **moves** while the
  progress is not e. At the end of each of the shell's frames (section
  12.5, step 3), if the view's machine is not in use, has no picture, has
  a rectangle that shows a picture, or has no Screen in the shell's 3D
  list, the view becomes none: the scene is shown as listed at once.

### 12.3 Screens

A machine's **picture** is its screen scaled uniformly into its rectangle
(x, y, w, h) and centred. If w × 480 ≤ h × 640, the picture is
pw = w wide and ph = ⌊w × 480 / 640⌋ high, otherwise pw = ⌊h × 640 / 480⌋
and ph = h; it starts at px = x + ⌊(w − pw) / 2⌋, py = y + ⌊(h − ph) / 2⌋.
If pw or ph is 0 nothing is shown. A machine that is running, paused,
exited or faulted has a picture; one that is loading or failed has none.
Exited and faulted machines show the screen as it was at their last frame.
A machine's screen here is its complete picture, its own Surfaces (section
13.5) included.

A machine is shown **flat** in its picture: the picture of its own
rectangle, or, once the view (section 12.2 `face`) of it has finished
moving towards the glass, the picture of the view's rectangle. A machine
is shown as a **panel** where the shell's 3D list has a Screen of it in
effect (section 13.3), unless it is shown flat by the view.

If the shell's 3D list has no Screen in effect, the host shows, from the
bottom:

1. the shell's screen, over the whole of the host's screen as a single
   machine's would be, combined as section 10.5 says;
2. each flat picture, in increasing handle order, each a complete
   screen (its own layers, style, afterglow and page, sections 10.5, 10.6,
   11) scaled into its picture and covering what is below it; outside the
   picture, the rest of the rectangle shows what is below;
3. the host's own prompts (section 9.5), over everything; a consent prompt
   names the image title of the machine whose request it is.

If it has at least one, the host shows the shell's **scene**, from the
bottom:

1. the shell's screen as above, without the lines, points and circles of
   its 3D list (its glass, section 13.5, if it has Surfaces in effect);
2. the panels, and the shell's own Surfaces (section 13.3), from the
   farthest to the nearest by the distance from the eye in use (section
   13.1) to the panel's centre (the mean of its corners), the lower handle
   first where two are as far (a Surface before a Screen, two Surfaces in
   list order); each covers what is below it;
3. the lines, points and circles of the shell's 3D list as section 13
   draws them, each Screen and Surface in effect hiding what is behind it
   as a face of its four corners does;
4. each flat picture, in increasing handle order, as step 2 above;
5. the host's own prompts, as step 3 above.

A **panel** shows its machine's complete screen (as a flat picture does)
on its front, seen in perspective from the eye in use: the point of the
panel a fraction u along its top edge from its top left corner and v
down its left edge shows the point (640u, 480v) of the machine's screen.
Seen from behind (section 13.3) it is an opaque plate of a dark colour
the host chooses. Panels are cut at the near plane (z = −800) and at the
edges of the shell's screen. A host that cannot draw a picture in
perspective may draw each panel's picture flat instead: scaled uniformly
into the bounding rectangle of the panel's projection (its part in front
of the near plane), as a picture into a rectangle, and centred; the panel
still hides what it would hide.

**The view.** While there is a view whose machine has a Screen in
effect, everything in steps 2 and 3 of the scene (panels, lines, points,
circles and the faces that hide them) is moved by the view's transform
before it is seen from the eye; the caps of section 13.4 are counted on
the list as given. Let the view's machine's panel as listed have the
centre c₀, the width w₀ and the orientation R₀ (the rotation whose
columns are the unit vectors along its top edge, down its left edge, and
their cross product), and let the picture of the view's rectangle be
(px, py, pw, ph), with centre c₁ = (px + pw/2, py + ph/2, 0). At
progress p (section 12.2) let t = p/4,096 and e = t² (3 − 2t). Then
c = c₀ + (c₁ − c₀) e, w = w₀ (pw/w₀)^e, R the rotation the spherical
interpolation of the quaternions of R₀ and the identity by e gives, by
the shorter arc, and the transform takes a point q to
c + (w/w₀) R R₀ᵀ (q − c₀). So at progress 0 the scene is as listed, and at
4,096 the view's panel lies on the glass over the picture, upright,
centred on it and as wide; the eye stays where it is, so what is not on
the glass still moves with it. When the view has finished moving towards
the glass its machine is shown flat (step 4) and not as a panel. A host
that cannot animate may show every view as at its end (progress 0 or
4,096) at once; the input is routed by the view as section 12.4 says.

How the host scales a screen (fonts rasterised for the picture's size,
smoothing) is presentation (section 8.6), and so are the resolution a
panel's picture is drawn at, how it is filtered, and the smoothing of a
panel's edges.

### 12.4 Input

The host's input events (section 5) are in the shell's screen units. In each
frame, before the shell runs, they are routed in order by the focus, the
flat pictures and the panels as they are at that moment (section 12.3, as
shown after the shell's last frame, seen from the eye the host draws
from):

| Input | Goes to |
|---|---|
| Key down, key up, text, wheel | the machine with the focus if it is running; the shell if the focus is 0; otherwise nobody |
| Pointer, over a flat picture | the machine whose picture it is (the highest handle where pictures overlap), mapped into its screen units, if it is running; otherwise nobody |
| Pointer, over the front of a panel and no flat picture | while the view moves (section 12.2), nobody; otherwise the machine whose panel it is, mapped into its screen units, if it is running; otherwise nobody |
| Pointer, over nothing else | the shell, unchanged (but for the shell's own Surfaces, below) |
| Controllers (section 6) | the machine with the focus (the shell if the focus is 0); every other machine reads every controller as not connected, so nobody reads them while the machine with the focus does not run |
| Content done, feeds updated, page events | each machine its own (sections 9, 11.5) |
| Any other event in the host's input | the shell, unchanged |

The focus that routes the input of a frame, the controllers included, is
the one at this step: the home key changes it here (below), for the
events after it; a `focus` call takes effect at the next frame's routing
(section 12.5).

A position (x, y) over a picture (px, py, pw, ph) is mapped to
x′ = ⌊(2(x − px) + 1) × 640 / (2pw)⌋ and y′ = ⌊(2(y − py) + 1) × 480 / (2ph)⌋.
The pointer at (x, y) is over a panel if the ray from the eye in use
(section 13.1) through the point (x + 1/2, y + 1/2, 0) meets the panel in
front of the near plane (z > −800), at a fraction u along its top edge
(0 ≤ u < 1) and v down its left edge (0 ≤ v < 1); of several, the one it
meets nearest the eye counts. If that panel is seen from behind (section
13.3), the pointer is over nothing; otherwise it is mapped to x′ = ⌊640u⌋,
y′ = ⌊480v⌋. A pointer event that presses a button (sets a button bit that
was clear in the previous pointer event, wherever that went) over a flat
picture, or over a panel while the view does not move, also gives the
shell a machine pressed event, in the place among the shell's events that
the pointer event had in the input, whatever the machine's state (also
when the pointer event itself goes to nobody).

**Surfaces.** A machine's own Surfaces (section 13.3) map the pointer once
more: a pointer event going to a machine, at (x′, y′) in its screen units
as above, is delivered at the point of the Surface it is over, if any, as
section 13.3 says, seen from the same eye. The shell's own Surfaces, when
its scene has panels, are among the panels (section 12.3): a pointer over
the front of one goes to the shell at that point (while the view moves, to
nobody), and gives no machine pressed event; when its scene has no
panels, the pointer events that go to the shell are mapped through its
Surfaces as any machine's are. A host running a single machine maps its
pointer events through its Surfaces in the same way.

**The home key.** A host reserves one key (or other input) as its home key;
which one is up to the host. It is never delivered to any machine as a key
event. When it is pressed, the focus becomes 0 and a home event is queued
for the shell. A host running a single machine, not a system, has no
shell to tell: the home key does nothing there (the reference host then
treats that key as an ordinary key).

**Events for the shell** (section 5):

| Type | Name | `a` | `b` | `c` |
|---|---|---|---|---|
| `0Ah` | Machine changed | handle | the new state | 0 |
| `0Bh` | Home | 0 | 0 | 0 |
| `0Ch` | Machine pressed | handle | buttons | 0 |
| `0Fh` | Open requested | handle | 0 | 0 |
| `10h` | Snapshot done | slot | 1 stored, 3 failed | 0 |

Open requested says that machine `a` made an open request the shell can
read with `request`; there is one such event per request. Snapshot done
answers a `snapshot` call; there is one per accepted call, in the order of
the calls.

Machine changed is sent for every change of state the shell did not make
with a call: loading to running, paused or failed; running to exited or
faulted. When the machine with the focus becomes exited, faulted or failed,
the focus becomes 0 (and no home event is sent).

### 12.5 Frames

For each system frame the host does the following, in order:

1. **Latch.** For each handle in use, in increasing order: a loading handle
   whose image the host now has, or has failed to get, becomes running
   (paused if the run flag is clear) or failed; a running handle whose
   machine stopped in an earlier frame becomes exited or faulted, with its
   exit code or fault kind and `pc`. Each of these changes queues a machine
   changed event for the shell, ahead of the frame's input. Each record then
   takes the machine's units and frame counter (section 12.2). Then, in
   increasing handle order, each machine that made an open request in its
   last frame (section 4) has it waiting for `request`, and an open
   requested event is queued for the shell, after the machine changed
   events and ahead of the frame's input. A machine that becomes running
   or paused here is reset with the argument its handle was given, if
   any, or, for `restore`, is the machine of the snapshot. Then each
   snapshot the host has stored, or failed to store, since the last
   frame queues a snapshot done event, in the order of the `snapshot`
   calls, and a slot whose snapshot was stored holds it from now on for
   `snapshot_info` (unless the shell deleted it after asking for it).
2. **Route** the frame's input (section 12.4).
3. **The shell** runs its frame (section 1.4). Then, if the view's
   machine is no longer shown as a panel, the view becomes none (section
   12.2 `face`).
4. **Starts and stops** the shell made in its frame are carried out in the
   order it made them, with the arguments it gave. A machine started in
   this frame is loading: its first frame is at the earliest in the next
   system frame, the one in whose step 1 it becomes running. A request
   the shell read with `request` in its frame no longer waits, so the
   machine's next `open` (in step 5) can make another.
5. **The machines** that are running now, in increasing handle order, each
   run one frame (section 1.4) with a full budget and their own carry. So a
   `run`, `stop` or `place` made by the shell takes effect in the same
   frame; a `focus` takes effect at the next frame's routing, for every
   kind of input, the controllers included (section 12.4).
6. **Snapshots and deletes** the shell asked for in its frame are carried
   out in the order it asked for them: each snapshot is taken of the
   machine as it is now (failed if it no longer exists), each delete
   removes a slot's snapshot.

Paused, loading, exited, faulted and failed machines run no frames, and
their frame counters do not advance. When the shell stops (section 1.5), the
system stops: no machine runs any more frames.

### 12.6 Determinism

Each machine is deterministic given its own inputs (sections 4 and 9.4). For
the shell they include the machine changed, home, machine pressed, open
requested and snapshot done events and what device 8 returns that the
shell did not set itself: the latched records (states, exit codes, faults,
failure reasons, the image IDs and identities of programs started by
identity or restored, units, frame counters), the open requests `request`
delivers, the built-in programs and what `snapshot_info` tells; the view
is the shell's own state, and the eye the host routes the pointer by is
part of the input. For a machine
the shell started, what `open` returns (whether a request of it still
waits for the shell) is an input like a completion. The identities the shell gave are its own state. Device 8's state is part of the shell's state (`STATE.md`
1.10); a started machine's state is its own and holds nothing of the
system.

### 12.7 Snapshots

A **snapshot** is a machine's complete state, its state serialization
(`STATE.md` 1), with what is needed to recreate it: the image ID and the
argument (both in the serialization) and the program's identity. A machine
restored from a snapshot has the state hash the machine had when the
snapshot was taken and, given the same later inputs, runs exactly as that
machine would have. Its frame counter goes on from the snapshot's; the
time, random bytes and every other input are what the host supplies after
the restore.

- The shell has 16 **slots**, 0 to 15. They are the host's local data,
  kept per shell as named records are (by the shell's identity, else its
  image ID, section 9.6), durably (written to a temporary file, synced,
  renamed into place), and never sent anywhere. `STATE.md` 4 gives a
  host's file format; a snapshot may be over 1 MiB.
- A snapshot belongs to its exact image. After a program is updated, a
  snapshot of the old version still restores the old version, by its
  image ID, as long as the host can get that image; it never restores
  into another image.
- Named records (section 9.6) are not part of a snapshot: a restored
  machine reads its owner's records as they are now.
- Requests the machine had pending complete as failed in its first frame
  (section 9.2). An open request that waited for the shell (section 4
  `open`) is dropped. Locates the machine made in its last frame before
  the snapshot (section 11.2) are not answered. A host lays the page out
  and reports its height (section 11.5) as for a new document, and draws
  the vector display without afterglow (section 10.6).
- The shell itself is never snapshotted: a state with section 1.10
  present cannot be restored.

---

## 13. Device 9 — 3D vector display (version 0)

Draws lines, points and circles placed in a box around the screen, from a
display list like device 3's (section 10); faces, which draw nothing, hide
what is behind them; Surfaces show parts of the machine's own 2D layers
as panels in the box; in the shell's list, Screens place the screens of the
machines it started as panels in the box (section 12.3). The program gives geometry, not
a picture: the host projects it onto the screen from an eye it chooses
(section 13.1), so it may let the viewer look around the models, and the
program cannot observe that choice. As with device 3, nothing drawn can be
read back.

### 13.1 Space and view

Coordinates are signed 16-bit numbers in **sixteenths of a screen unit**
(section 7), little-endian. x and y are as on the screen (x right, y down);
z is depth in the same units: 0 is the **glass** (the plane of the screen),
positive is away from the viewer. The axes are right-handed.

The **reference eye** is at (320, 240, −960) in screen units. Seen from an
eye at (ex, ey, −960), a point (x, y, z) with z > −800 is drawn on the glass
at

  x′ = ex + (x − ex) × 960 / (960 + z),  y′ = ey + (y − ey) × 960 / (960 + z).

So a point on the glass is drawn exactly where the 2D layers would put it,
from any eye; a point behind the glass moves with the eye, one in front of
it against the eye. A host may move the eye sideways and up or down by up
to 160 units on each axis from the reference eye, at its own discretion
(for example following the viewer's pointer or head); the eye's distance
from the glass, and the glass, stay fixed. The program is not told where
the eye is.

### 13.2 Calls

- **`submit`** copies `a1` bytes from address `a0` into the device,
  replacing the previous list, and returns 0. It is rejected, returning 1
  and keeping the previous list, if the length is 0 or more than 16,384 or
  any part of the range is outside RAM.
- **`clear`** empties the list.
- **`style`** and **`persistence`** set the display style and the
  persistence of section 10.6, with the results of device 3's calls of the
  same names. They are one setting with device 3's: whichever device sets
  them, both displays (and the whole screen) are shown that way. A machine
  whose image lists device 9 and not device 3 has the setting too (device
  3's calls stay absent).

After reset the list is empty (and the style and persistence are as section
10.2 says). The list the device holds at the end of a frame is the one the
host shows for that frame. Nothing of device 9 is filled, so the fill mode
does not apply.

### 13.3 Display list

A list is a sequence of commands, each one opcode byte followed by its
operands. Drawing state at the start of a list: colour opaque white (255,
255, 255, 255), width 16 (one screen unit), the identity transform, no
saved transforms, current point (0, 0, 0).

| Opcode | Command | Operands | Effect |
|---|---|---|---|
| `00h` | End | — | Ends the list |
| `01h` | Colour | r, g, b, a (1 byte each) | Sets the colour; `a` is opacity, 255 opaque |
| `02h` | Width | w (2 bytes, unsigned) | Sets the line width in sixteenths, the same on the screen at any depth; 0 counts as 1, more than 255 as 255 |
| `03h` | Move | x, y, z | Sets the current point |
| `04h` | Line | x, y, z | Draws a line from the current point to (x, y, z), which becomes the current point |
| `05h` | Point | — | Draws a dot at the current point |
| `06h` | Circle | radius (2 bytes, unsigned) | Draws a circle about the current point in the transform's xy plane |
| `07h` | Push | — | Saves the transform |
| `08h` | Pop | — | Restores the transform saved last |
| `09h` | Translate | x, y, z | Moves the transform's origin to (x, y, z) |
| `0Ah` | Rotate | axis (1 byte: 0 x, 1 y, 2 z), angle (2 bytes, unsigned, in 1/65,536 of a turn) | Turns the transform about its axis |
| `0Bh` | Scale | s (2 bytes, unsigned, 8.8 fixed point: 256 is 1) | Scales the transform uniformly; s = 0 puts every point given after it at the transform's origin |
| `0Ch` | Face | n (1 byte: 3 or 4), then n corners x, y, z | Hides what is behind it (below); draws nothing |
| `0Dh` | Screen | handle (1 byte, 1–8), width w (2 bytes, unsigned) | Shows machine `handle`'s screen as a panel (below and section 12.3) |
| `0Eh` | Surface | x, y, width, height (2 bytes each, unsigned, screen units), panel width w (2 bytes, unsigned) | Shows that rectangle of the machine's own 2D layers as a panel (below and section 13.5) |

- **The transform.** A point p given to Move or Line is drawn at M(p),
  where M is the transform in force, with p and every translation in
  screen units (sixteenths divided by 16). Translate t, Rotate R and Scale
  s replace M by M ∘ T(t), M ∘ R and M ∘ S(s): each applies to coordinates
  given after it, before the transforms given earlier, as in ordinary
  matrix-stack graphics. So "Translate (100, 0, 0), Rotate z a quarter
  turn, Move (10, 0, 0)" puts the current point at (100, 10, 0).
- **Rotation.** A positive angle θ about x turns y towards z, about y turns
  z towards x, and about z turns x towards y (clockwise as seen on the
  screen): about x, (x, y, z) goes to (x, y cos θ − z sin θ, y sin θ +
  z cos θ); about y, to (x cos θ + z sin θ, y, −x sin θ + z cos θ); about z,
  to (x cos θ − y sin θ, x sin θ + y cos θ, z).
- **The current point** is kept as Move or Line placed it, in the box (M
  applied), so a later transform does not move it, as a later Origin does
  not move the 2D display's (section 10.3). A Line after a Push, a
  Translate and a Pop starts where the Move put it. To draw a circle about
  a transform's origin, Move to (0, 0, 0) first.
- **A circle** of radius r (in sixteenths) about the current point c is
  the closed curve c + L(r cos φ, r sin φ, 0) / 16, where L is M without its
  translation (so it lies in the plane of M's x and y axes and is scaled
  with it). It is drawn as the 64 chords between the points at φ = k/64 of
  a turn, k = 0 to 63, which join without ends of their own: one curve.
  A circle of radius 0 is 64 chords of length 0: it counts 64 segments
  and no length (section 13.4) and draws nothing.
- Lines and points use the current width and have round ends, as the 2D
  display's lines do (a Point is the dot a line of length 0 draws). Circle,
  Point and the transform commands leave the current point unchanged.
- **The stack** holds up to 8 transforms. Push saves only the transform
  (not the colour, the width or the current point). A Push with 8 saved,
  and a Pop with none saved, end the list.
- The list ends at an End command, at the end of the submitted bytes, at
  an unknown opcode, at a command whose operands are cut off by the end of
  the list, at a Rotate whose axis is above 2, at a Face whose n is not 3
  or 4, at a Screen whose handle is 0 or above 8, or as the stack rule
  says. Commands before that point are drawn.
- Lines, points and circles are drawn in list order, each over the earlier
  ones, blended by opacity as in section 10.3. Only faces hide: a near line
  does not cover a far one any more than a later one covers an earlier one.
- **A face** is a flat polygon that gives off no light but hides the
  lines, points and circles behind it. Its corners are transformed as
  Move's point is (M applied); the current point is not changed. A face of
  3 corners is the triangle they make; one of 4 corners is the two
  triangles of corners 0, 1, 2 and 0, 2, 3 (so it need not be flat or
  convex). Both sides of a face hide alike. A list without faces draws
  exactly as it would if faces did not exist.
- **What a face hides.** Seen from the eye in use (section 13.1), a point
  of a line, point or circle is hidden if the straight segment from the eye
  to it passes through one of a face's triangles and the point lies more
  than **2 sixteenths** (1/8 screen unit) beyond the triangle's plane,
  measured perpendicular to the plane, on the side away from the eye. So a
  line in a face's plane, along its edge or ending on it is not hidden by
  that face, and a solid's edges given with the same corners as its faces
  are drawn. Only a face's part with z > −800 hides (a face is cut at that
  plane as lines are). A face hides whatever is behind it wherever the
  face is in the list, before or after it. A triangle with no area, or
  seen edge on, hides nothing.
- **What is drawn of a hidden line.** A line, and each chord of a circle,
  is drawn as its pieces that are not hidden. Each piece has, along it,
  the depth cue that part of the whole line has (section 13.5). An end
  where a piece was cut is a butt end without vertex dwell, as where a
  circle's chords join; the line's own ends keep their own. A piece with a
  cut end that is shorter than 1/2 screen unit on the glass is not drawn,
  unless the whole line is that short. The whole line's length is that of
  its part in front of the near plane, on the glass, seen from the eye in
  use, before it is clipped to the screen. A point is drawn or hidden whole.
- **Faces that cross** need no rule of their own: each point of a line is
  hidden or not by each face. Faces do not hide one another and draw
  nothing, so no line is drawn where two faces cross unless the list draws
  one.
- **A Screen** is a flat panel w/16 units wide and 3/4 of that high,
  centred on the transform's origin in its xy plane: its corners are
  M(−w/32, −3w/128, 0) (top left), M(w/32, −3w/128, 0) (top right),
  M(w/32, 3w/128, 0) (bottom right) and M(−w/32, 3w/128, 0) (bottom left),
  with w in sixteenths and M the transform in force (so it is scaled,
  turned and moved with it). With the identity transform it lies like the
  glass: its top edge along x, its left edge down along y, its front
  towards the viewer. It does not change the current point. With a = top
  right − top left and b = bottom left − top left, the panel is **seen
  from behind** from an eye on the side of its plane that a × b points to
  (or in its plane), else its front is seen.
- **Which Screens are in effect.** A Screen is in effect only in the list
  of the shell of a system (the machine that has device 8, section 12.1),
  only if it is the first Screen of its handle in the list, the handle's
  machine has a picture (section 12.3) and the handle's rectangle shows no
  picture. Any other Screen is decoded and does nothing: it draws nothing,
  hides nothing and is not counted. A Screen in effect is shown as section
  12.3 says, and hides lines, points and circles exactly as a Face of its
  four corners does; one of width 0 has no area and hides nothing.
- **A Surface** shows a rectangle of the machine's own screen: its
  **source**, x, y, width and height in screen units, clamped so it lies
  on the screen as the page's rectangle is (section 11.2: x to at most
  640, y to at most 480, the width to at most 640 − x, the height to at
  most 480 − y). What it shows is the machine's 2D layers there (section
  10.5: the terminal's cell backgrounds, the page, device 3's shapes, the
  terminal's characters and cursor, in that order and in the machine's
  style), never its 3D list. It is a flat panel w/16 units wide and as
  high as the source's shape makes it, h′ = w × height / width (in
  sixteenths), centred on the transform's origin in its xy plane: its
  corners are M(−w/32, −h′/32, 0) (top left), M(w/32, −h′/32, 0) (top
  right), M(w/32, h′/32, 0) (bottom right) and M(−w/32, h′/32, 0) (bottom
  left), lying as a Screen's do; the point of the panel a fraction u along
  its top edge and v down its left edge shows the point (x + u × width,
  y + v × height) of the screen. It does not change the current point. It
  is seen from behind as a Screen is.
- **Which Surfaces are in effect.** Any machine's list may have them (a
  machine needs no device but 9 for them; they show what its other devices
  draw). A Surface is in effect if its source, after clamping, has a width
  and a height, its w is not 0 and fewer than **4** Surfaces before it in
  the list are in effect; any other Surface is decoded and does nothing: it
  draws nothing, hides nothing, is not counted and leaves the glass as it
  is. A Surface in effect counts as one face for the caps (section 13.4);
  if a cap stops it, it is not in effect either. It is shown as section
  13.5 says: it is opaque, hides lines, points and circles exactly as a
  Face of its four corners does (from either side), and its source is not
  drawn on the glass.
- **The pointer over a Surface.** A pointer position (x, y) on the glass
  is over a Surface in effect if the ray from the eye in use (section
  13.1) through (x + 1/2, y + 1/2, 0) meets it in front of the near plane
  (z > −800) at a fraction u along its top edge (0 ≤ u < 1) and v down its
  left edge (0 ≤ v < 1); of several, the one it meets nearest the eye
  counts. If that Surface's front is seen, the machine receives the
  pointer event at x′ = sx + ⌊u × sw⌋, y′ = sy + ⌊v × sh⌋, where (sx, sy,
  sw, sh) is its source, with the same buttons; otherwise (seen from
  behind, or over no Surface) at (x, y) as on the glass. The list and the
  eye are those the machine's picture was last shown with (section 12.4);
  a page anchor press (section 11.5) is found at the position delivered,
  so text and anchors shown on a Surface can be pressed there.

### 13.4 Clipping and caps

A line is cut where it crosses the plane z = −800: only its part with z >
−800 is drawn; a point (or a dot) at z ≤ −800 is not drawn. What is left is
projected (section 13.1) and clipped to the screen grown by 32 units on
every side; what lies beyond that margin is not drawn, and the host clips
what it draws to the screen.

The amount one list draws is limited. In list order, each Line, Point and
Circle counts a **length** and a number of **segments**:

| Command | Length (screen units) | Segments | Faces |
|---|---|---|---|
| Line | its length on the glass, seen from the reference eye, of its part with z > −800 that lies on the screen (0 to 640 by 0 to 480) | 1 | 0 |
| Point | 0 | 1 | 0 |
| Circle | the sum of its 64 chords' lengths, each measured as a line's | 64 | 0 |
| Face | 0 | 0 | 1 |
| Screen in effect | 0 | 0 | 1 |
| Surface in effect | 0 | 0 | 1 |

A command that would take the list's total length above 65,536, its
number of segments above 16,384, its number of faces above 256, or its
number of faces times its number of segments above 131,072 is not drawn,
and neither is any command after it; a face not drawn hides nothing. A
command that is not drawn does not count. The caps are counted from the
reference eye whatever eye the host draws from, and as if faces hid
nothing, so the same commands are drawn from every eye.

| Faces | Segments at most |
|---|---|
| 0 to 8 | 16,384 |
| 16 | 8,192 |
| 64 | 2,048 |
| 256 | 512 |

After the hidden parts are cut out (section 13.3), at most 16,384 pieces
are drawn, in list order; pieces beyond that are not drawn. Only pieces
within the screen grown by 32 units count. Which pieces those are depends
on the eye.

The caps are evaluated by the host, after the floating-point projection
of section 13.1, so hosts may differ at the boundary by rounding: a list
whose totals come within rounding of a cap may be cut one command
earlier on one host than on another. This affects only what is drawn,
never the machine's state, which holds the list as submitted (section
13.6).

### 13.5 Presentation

- **Depth cue.** Light dims with depth. Each end of a line (after the cut
  at z = −800) has the factor min(1, 960 / (960 + z)); along the line on
  the glass the factor goes linearly from one end's to the other's, which
  is its value at every point of the line, as it is the projection's own
  scale. The opacity is multiplied by the factor: 0.5 at z = 960, and never
  more than the colour's own in front of the glass. A point has its own,
  each chord of a circle those of its ends.
- **Layers.** Device 9 is drawn in the vector layer of section 10.5, after
  device 3's shapes: over the page and the 2D shapes, under the terminal's
  characters, so text on the glass labels the models.
- **Phosphor.** In the phosphor style (section 10.6) lines, points and
  circles are beam light like the 2D display's lines: core, halo, bloom,
  afterglow and smear, with the vertex dwell at line ends and points but
  not where a circle's chords join or where a face cut a line.
- **Screens** in effect are drawn as panels, in the shell's scene (section
  12.3), not in the vector layer: the shell's lines, points and circles
  are then drawn over the panels, and in the phosphor style how they are
  lit there (beam, afterglow) is up to the host.
- **Surfaces.** A machine whose list has no Surface in effect is drawn
  exactly as without this rule (section 10.5). One that has Surfaces in
  effect is drawn, from the bottom:
  1. the **glass**: its 2D layers as section 10.5 combines them, without
     the lines, points and circles of its 3D list, and where a pixel lies
     in the source of a Surface in effect, black instead (the 2D layers
     are not drawn there: what a Surface shows is not also on the glass);
  2. its Surfaces, from the farthest to the nearest by the distance from
     the eye in use to their centres (the mean of their corners), the
     earlier in the list first where two are as far; each shows its source
     (section 13.3) in perspective, or from behind the dark plate of a
     Screen, and covers what is below it;
  3. the lines, points and circles of its 3D list as this section draws
     them, each Surface hiding what is behind it as a face of its corners
     does.

  So the program's text on the glass outside the sources stays where it
  is, as a heads-up line, and its 3D lines are drawn over it. The 2D
  layers a Surface shows are those of the same frame, in the machine's
  style; in the phosphor style the afterglow is that of the 2D layers on
  the screen, so it moves with the panel (a moving Surface leaves no
  trail), and how the 3D lines are lit over the Surfaces is up to the host,
  as over a shell's panels. Surfaces are cut at the near plane and at the
  edges of the screen as panels are. A host that cannot draw a picture in
  perspective may draw each Surface's source flat instead, scaled
  uniformly into the bounding rectangle of its projection (its part in
  front of the near plane) and centred, as section 12.3 allows for panels;
  it still hides what it would hide. The resolution the source is drawn at
  for a Surface, its filtering and the smoothing of its edges are
  presentation.

  The shell's own Surfaces, when its scene has panels (section 12.3), are
  among them: step 1 there is the shell's glass, and its Surfaces are
  drawn with the panels, far to near, before the shell's lines. Wherever a
  machine's screen is shown (on its own, as a flat picture or as a panel)
  it is its complete picture, its Surfaces included; a Surface shows only
  2D layers, so no picture shows itself.
- The trigonometry and the projection are the host's, and hosts may differ
  in the last pixel. Smoothing, the exact colour of blended pixels and how
  the eye moves are up to the host.

### 13.6 Determinism

The device's state is the copy of the list (STATE.md 1.11); the style and
the persistence are device 3's (section 10.7, STATE.md 1.7), held once for
both devices. It takes no input from the host: the eye is presentation,
and so is what faces hide from it. Which Surfaces are in effect depends on
the list alone; where a pointer event over a Surface is delivered depends
on the eye, which is part of the host's input (section 12.6). Which
Screens are in effect depends on
the shell's device 8 records, and only for what is shown; a shell's
`face` reads its list as decoded (section 12.2).

---

## 14. Charges

Every call is charged in budget units on top of the `ecall` instruction
itself (1 from ROM, 4 from RAM). Charges are subtracted from the budget as
the call runs and may take it below zero (section 1.4).

| What | Charge |
|---|---|
| Any call, including one that fails: its base charge | 4 for a deferred call and for every call of devices 1 and 8; 64 for an answered call |
| `write`: each byte of the buffer | 2 |
| Sound `submit`: each byte of an accepted list (section 18.2) | 2 |
| A clear, scroll up or scroll down done by `command` (`01h`–`03h`) | 150 |
| Bytes copied between memory and a device: `next_event` (4 when an event is written), `random`, `controller` (8), `cells_read`, `cells_write`, `palette_set`, an accepted vector `submit` (device 3 or 9), sound `wave` (64) and `sample` (its length), each when it is carried out, an accepted content put or save (its `size` bytes), publish (its title), find (its query), set profile or set nickname (its name), a page `submit` whose bytes were copied (its length in range and its buffer valid), device 8's `start` (32, when the ID is read, also when no handle is free), `start_program` (33 + n, when the record is read), `info` (48, when written), `builtin` (32, when written), `identity` (81, when written), `request` (82, when written), `argument` (32, when the ID is read, also when it returns 0) and `snapshot_info` (122, when written); `open` (33, or 34 + n, when the request is read: whenever it is valid, also when the call returns 0) and the system's `argument` (32, when written) | 1 per 4 bytes, rounded up, per call |
| Content delivered at completion: the bytes a get, load, records (subscriptions, timeline, names, programs, records) or find request writes to `addr` | 1 per 4 bytes: the bytes of all the frame's completions added up, then divided by 4 and rounded up once; subtracted from that frame's budget in step 2 of section 1.4 |

Nothing else is charged: a rejected submit or put copies nothing, the
scrolls and clears a `write` causes are covered by its 2 per byte (section
8.6), the fields a completion writes to a request block are free, and so is
laying out the page, which the host does outside the machine's frames
(section 11.4).

**Deferred and answered calls.** A device call is **deferred** if its
result is one the program could work out without the device (a constant,
one of its arguments, or a simple function of its arguments and of where
its buffer lies) and its charge depends only on its arguments; it is
**answered** if its result or its charge depends on the device's state.
Section 3.2 gives every call's class. The answered calls are the
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

---

## 15. Conformance

An implementation conforms to profile 1 if:

- its CPU executes RV32IMC exactly as section 1.1 says, including which
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

A host that offers only the reading profile (section 2.1) cannot run
`programs/conform`, which requires devices 3 and 9. `programs/conform-reading`
is built from the same source with the sections for other devices and for
the optional content operations left out, and has its own input script
and expected hashes; `terse run --profile reading` runs it (and any image)
on exactly the reading profile. Likewise `programs/conform-program`, for
the program profile, requires only devices 2 and 3, leaves out the
sections of content, the page and the 3D display, and checks `device`;
`terse run --profile program` runs it on exactly that profile.

## 16. Provisional values

These numbers are placeholders until weak hosts have been measured. Every
numeric limit of the machine and its devices is listed here, with the
section that defines it.

The budget and the charges:

- the budget, 125,000 units per frame (section 1.4);
- the cost of an instruction fetched from RAM, 4 (section 1.4);
- the charges of section 14 (4 per deferred or core call, 64 per answered
  call, 2 per `write` byte and per sound list byte, 1 per 4 bytes copied,
  150 per clear or scroll command), and the longest `write`, 4,096 bytes
  (section 8.2).

What one frame or one list may make a host draw:

- the 2D display's fill cap, 4 × 640 × 480 square units (section 10.4);
- the 3D display's caps (section 13.4): 65,536 units of length, 16,384
  segments, 256 faces, 131,072 faces × segments and 16,384 pieces drawn
  after hiding; its 64 chords to a circle, its transform stack of 8, its
  widest line, 255 sixteenths, and its 4 Surfaces in effect in a list
  (section 13.3); a face hides only what lies
  more than 2 sixteenths beyond its plane, and a cut piece shorter than
  1/2 unit is dropped (section 13.3); the margin of 32 units around the
  screen within which pieces are drawn (section 13.4); and its eye: 960
  units from the glass, at most 160 from the reference eye on each axis,
  and the near plane at z = −800 (section 13.1);
- the figure caps (section 11.3): 256 paths, the area (8 × the figure's,
  at most 4 × 640 × 480) and the length (65,536); figure sides of 1 to
  1,024 units;
- the page's tables (sections 11.3, 11.4): at most 8 columns, 1,024
  cells in a table and 4,096 in a document; padding of 6 and 3 units; the
  least column cap, 48 units; font sizes 6 to 96.

The sizes of what a program hands a device:

- display lists of 1 to 16,384 bytes, for both vector displays (sections
  10.2, 13.2); page documents of 1 to 65,536 bytes (section 11.2); sound
  command lists of 1 to 4,096 bytes and samples of up to 65,536 bytes
  (section 18.2);
- 8 locates per frame, and at least the 8 latest waiting kept by a host
  (sections 11.2, 11.5);
- 256 random bytes per call (section 4);
- the event queue, 64 events (section 5).

The content device (section 9):

- blobs of 1 to 1,048,576 bytes; 8 requests pending; 16,384 bytes
  delivered by one get, load, records or find request;
- display names, nicknames and queries of 48 bytes, titles of 120, and 16
  results to a find (section 9.5);
- named records (section 9.6): names of 32 bytes, records of 65,536 bytes,
  and per owner 256 records and 1,048,576 bytes in all.

Systems (section 12):

- the number of machines a shell can start, 8, and of its snapshot slots,
  16 (sections 12.1, 12.7); program names of 48 bytes (section 12.2); the
  longest view motion, 600 frames, and the view's progress steps, 4,096
  (section 12.2 `face`).

The sizes that define machine profile 1 (ROM of 128 KiB, RAM of 1 to 128
pages of 8 KiB, 16 devices in an image, the 640 × 480 screen, the
terminal's 80 × 30 cells, 4 controllers) and the layouts of records, blocks
and events are not placeholders: they are the profile and its device
versions. Nor is the sound device's synthesis (section 18: 48,000 Hz, 800
sample pairs a frame, 16 voices, 32 wavetables of 64 samples, 4 sample
slots, every formula): it is the device's version 0.

## 17. Not yet specified

- Font selection, combining marks and complex scripts in the terminal.
- Named pointers (a signed, versioned pointer from a key and a name to a
  blob; not the display names and nicknames of section 9.5), ranges
  verified without fetching the whole blob, and cancelling a content
  request.
- For the requests that need consent (section 9.5): telling the program
  that one waits for the user, remembering a user's answer for later
  requests, and publishing replies or kinds of entry other than articles,
  documents and profiles (programs are published by the host, not by
  programs).
- Device 5.
- On the sound device (section 18): a volume per machine in a system,
  filters, and samples longer than 65,536 bytes or of more than 8 bits.
- On the 3D display (section 13): filled or shaded faces; the exact
  intersection of faces (a line drawn where two faces cross); more than 4
  Surfaces in a list, Surfaces that show the 3D layer, and curved or
  non-rectangular Surfaces and screens; panels that cross one another (they are drawn
  far to near by their centres, and one does not cut another); a stereo
  picture or another display of the same geometry (the eye
  rule allows one; how a host shows it is not specified); a limit on the
  area its lines cover (as for the 2D display's lines below).
- On the page: bitmap images, arcs and gradients in figures, links the host
  follows itself, selecting and copying
  text, and fonts beyond the three faces.
- A limit on the area covered by lines and circle outlines, including the
  outlines that filled shapes become in the phosphor style and fill mode 1
  (section 10.3). The fill cap does not apply to them, so a legal list of about 3,000 maximum-width lines
  across the whole screen still costs a host far more fill than four
  screens.
- In a system (section 12): communication between machines (an open
  request carries one blob ID or program identity, and its machine is not
  told what became of it); arguments other than one blob ID; budgets
  other than one full budget per machine per frame; giving a machine less
  than the whole screen; a volume for each machine's sound, and sound placed
  with a panel in the scene; what a host shows for a machine with no
  picture; telling the shell when a view has finished moving.
- Snapshots (section 12.7): saving them without the shell asking (a host
  has no shutdown notification yet), snapshots of the shell itself, moving
  a snapshot to another host, and a machine asking to be snapshotted.
- Named records (section 9.6): moving records from one owner to another
  (for example when a program run from a file is later published), keeping
  them in step between a user's machines, and sharing a record with
  another program. Starting a version of a program other than the current
  one.

---

## 18. Device 6 — Sound (version 0)

A small retro synthesizer in the spirit of the MSX SCC: 16 voices, each
playing a 64-sample wavetable, a noise generator or a sample, with an
envelope, a volume and a pan, and three cheap extras taken from FM chips:
**pairing** (one voice's output moves another's phase), **feedback** (a
voice's own output moves its phase) and **ring modulation** (one voice's
output multiplies another's). It is not a mixer and not a full FM
synthesizer.

Sound is optional (section 2.1): neither profile has it, and nothing a
reader shows depends on it. A program that needs it requires device 6 in
its header like any other device; a game whose sound is an extra lists
it as optional and plays only if `device` (section 4) says it has it.

Every step of the synthesis below is integer arithmetic on the values the
program set, specified to the bit, so two implementations render the same
samples from the same state, and the samples of a run are a function of
the image and its inputs like everything else (section 18.8).

Arithmetic: values are integers; `×` and `+` are exact (the stated ranges
show where they fit in 32 bits); `a >> k` is an arithmetic shift, the
floor of a ÷ 2^k also for a negative a; "mod 2^32" is the low 32 bits, as
an unsigned number.

### 18.1 Output

The device renders **48,000 sample pairs a second**: stereo, each sample
a signed 16-bit number. One frame is **800 pairs** (48,000 ÷ 60). The
device renders a frame's 800 pairs at the end of the frame (section 1.4
step 3), from its state as the frame's calls left it, so what a program
sets in frame N is heard from the first pair of frame N's samples; the
host plays them after the frame. A frame that does not run (a stopped or
paused machine) renders nothing.

### 18.2 Calls

- **`submit`** carries out the command list (section 18.3) of `a1` bytes
  at `a0` at once, and returns 0. It is rejected, returning 1 and doing
  nothing, if the length is 0 or more than 4,096, or if the list does not
  lie wholly in ROM or wholly in RAM. It costs 2 units per byte of an
  accepted list, like `write` (section 8.2), whose bound on how far one
  call can take the budget below zero it shares.
- **`wave`** copies the 64 bytes at `a1` (in ROM or RAM) into wavetable
  `a0` (0–31), each a signed sample (−128 to 127), and returns 0. A slot
  above 31, or a buffer not wholly in ROM or RAM, is a bad buffer
  (`FFFF_FFFEh`, section 3.1).
- **`sample`** copies the `a2` bytes at `a1` (0 to 65,536, in ROM or RAM)
  into sample slot `a0` (0–3), replacing what it held, and returns 0. Each
  byte is a signed sample. `a3` is the **loop start**: a voice that plays
  past the last byte goes on from byte `a3`; if `a3` is at least the
  length, the sample plays once (section 18.6). A length of 0 empties the
  slot. A slot above 3, a length above 65,536 or a buffer not wholly in
  ROM or RAM is a bad buffer.
- **`info`** returns 16 (the voices) in `a0` and the **capabilities** in
  `a1`: bit 0 set, the device plays samples. In version 0 every host sets
  bit 0: a host of version 0 must play samples, so that a program's
  samples are the same on every host (section 18.8). The bit is there for
  a later version that may leave samples out; a program that wants to run
  on one checks it before it uses `sample` or the Sample command.

Every call is deferred (section 14). After reset every voice is as
section 18.4 says, every wavetable is all zero and every sample slot is
empty (length 0, loop start 0).

### 18.3 Command list

A list is a sequence of commands, each an opcode byte followed by its
operands; multi-byte operands are little-endian. The commands set the
registers of the **current voice**, which is voice 0 at the start of each
list. They take effect at once, in order: the next samples rendered use
them (section 18.1).

| Opcode | Command | Operands | Effect on the current voice |
|---|---|---|---|
| `00h` | End | — | Ends the list |
| `01h` | Voice | v (1 byte, 0–15) | Makes voice v the current voice |
| `02h` | Wave | w (1 byte, 0–31) | Source: wavetable w |
| `03h` | Noise | — | Source: the noise generator |
| `04h` | Sample | s (1 byte, 0–3) | Source: sample slot s |
| `05h` | Frequency | increment (4 bytes) | Sets the phase increment (section 18.4) |
| `06h` | Volume | volume (1 byte) | 0–255 |
| `07h` | Pan | left, right (1 byte each) | The levels in the left and right outputs, 0–255 each |
| `08h` | Envelope | attack (2), decay (2), sustain (1), release (2) | Sets the envelope (section 18.5) |
| `09h` | Key on | — | Stage attack; the phase becomes 0 and Ended is cleared (section 18.6). The level is unchanged |
| `0Ah` | Key off | — | Stage release, if the stage is attack, decay or sustain |
| `0Bh` | Modulator | m (1 byte, 0–15 or `FFh`), depth (1 byte) | Pairing: voice m's output moves this voice's phase; `FFh` none |
| `0Ch` | Feedback | depth (1 byte) | This voice's own output moves its phase; 0 none |
| `0Dh` | Ring | q (1 byte, 0–15 or `FFh`) | This voice's output is multiplied by voice q's; `FFh` none |
| `0Eh` | Mix | on (1 byte, 0 or 1) | 1: heard; 0: muted (it is still rendered, so it can modulate or ring another voice) |

- The list ends at an End command, at the end of the submitted bytes, at
  an unknown opcode, at a command whose operands are cut off by the end of
  the list, or at a command with an operand out of range (a voice above
  15, a wavetable above 31, a sample slot above 3, a modulator or ring
  voice above 15 that is not `FFh`, a mix value above 1). The commands
  before it have taken effect; the rest are ignored. `submit` still
  returns 0: like a vector display (section 10.3), the device does not
  report where a list ended, since that would make `submit` an answered
  call (section 14).
- A voice may name itself as its modulator or ring voice; it then reads
  its own output of the previous pair (section 18.5).
- Changing a voice's source does not change its phase, its level or its
  Ended flag.

### 18.4 Voices

Each voice has these registers, with these values after reset:

| Register | Range | After reset |
|---|---|---|
| Source | wavetable w, noise, or sample slot s | wavetable 0 |
| Phase | 32 bits, unsigned | 0 |
| Increment | 32 bits, unsigned | 0 |
| Volume | 0–255 | 255 |
| Pan | left, right: 0–255 each | 255, 255 |
| Attack, decay, release | 0–65,535 each | 0, 0, 0 |
| Sustain | 0–255 | 255 |
| Stage | off, attack, decay, sustain, release | off |
| Level | 0 to `FF_FFFFh` | 0 |
| Modulator, depth | voice 0–15 or none; 0–255 | none, 0 |
| Feedback | 0–255 | 0 |
| Ring | voice 0–15 or none | none |
| Mix | 0 or 1 | 1 |
| Noise register | 15 bits, never 0 | 1 |
| Ended | flag | clear |
| Output | the voice's output in the last pair rendered, −32,768 to 32,767 | 0 |
| Previous output | its output in the pair before that, likewise | 0 |

The **increment** is how far the phase moves each sample. For a
wavetable voice one turn of the phase (2^32) is one pass through the
table, so a tone of f Hz has the increment round(f × 2^32 ÷ 48,000) (440
Hz: 39,370,534). For a noise voice it sets how often the noise changes
(section 18.6). For a sample voice it is the playback rate in samples of
the sample per output sample, with 16 fraction bits: 65,536 plays the
sample at 48,000 samples a second, and a sample recorded at r samples a
second plays at its pitch with round(r × 65,536 ÷ 48,000).

### 18.5 Rendering a pair

For each of the frame's 800 pairs, the device computes each voice's
output in turn, voice 0 to voice 15, then mixes them. Let **cur(q)** be
voice q's output in this pair if q is below the voice being computed (it
was computed already), else voice q's Output register: its output in the
previous pair. So a voice paired with a higher-numbered modulator (or
ringed by one) hears it one pair late, and one paired with itself hears
its own previous output. For voice v, in this order:

1. **Envelope.** With T = sustain × 65,793 (so that 255 gives
   `FF_FFFFh`):
   - attack: if attack is 0 or level + attack ≥ `FF_FFFFh`, the level
     becomes `FF_FFFFh` and the stage decay; else the level increases by
     attack;
   - decay: if decay is 0 or level ≤ T + decay, the level becomes T and the
     stage sustain; else the level decreases by decay;
   - sustain: the level becomes T;
   - release: if release is 0 or level ≤ release, the level becomes 0 and
     the stage off; else the level decreases by release;
   - off: the level becomes 0.

   So the rates are steps per pair on a 24-bit level, and 0 is at once:
   the reset envelope (0, 0, 255, 0) is a gate, full from the first pair
   after a key on and silent from the first pair after a key off.
2. **Modulation.** M = cur(m) × depth if the voice has a modulator m,
   else 0, which is less than 2^23 in size; F = (Output(v) + Previous
   output(v)) × feedback, the sum of its own outputs in the last two
   pairs, which is less than 2^24 in size.
3. **Raw sample** r, −128 to 127, from the source (section 18.6), which
   also advances the phase.
4. **Output.** o = (r × (level >> 8) × volume) >> 16, which lies in
   −32,640 … 32,384. If the voice has a ring voice q, o becomes (o ×
   cur(q)) >> 15.

Then the pair: L is the sum, over the voices whose mix is 1, of o × left;
R likewise with right. The pair's left sample is L >> 10 and its right
sample R >> 10, each clamped to −32,768 … 32,767. (One voice at full
volume and pan reaches a quarter of the range, so four can sound at full
volume before the sum is clamped.) Last, every voice's Previous output
register becomes its Output, and its Output becomes its o.

### 18.6 Sources

**Wavetable w.** The index is ((phase + M × 1,024 + F × 128) mod 2^32)
>> 26, a number 0–63, and r is wavetable w's sample at that index. Then
the phase becomes (phase + increment) mod 2^32. M and F move only the
index, never the phase: with depth 255 and a modulator at full scale the
index moves by about two passes through the table; with feedback 255 and
the voice at full scale, by about half a pass.

Feedback is the average of the last two outputs (F × 128 is that average
× feedback × 256, with no rounding), as on FM chips: a voice moved by its
last output alone settles into an oscillation from one pair to the next
from a swing of about 1 radian (a sixth of a pass) and is chaotic, heard
as static, soon after; the average cancels that oscillation. It goes
unstable in turn, at a 3-pair period, from a swing of about 2 radians
(about a third of a pass): feedback 165 for a voice at full scale, about
210 at volume 200. Up to there a sine grows from pure through brassy to
nearly a sawtooth as the feedback rises; above it the tone gains a shrill
edge, at most half a pass at 255. Since F follows the output, a voice's
tone also mellows as its envelope falls. A voice that names itself as its
own modulator (section 18.3) is moved by its last output alone, at up to
four times the swing: chaotic from small depths.

**Noise.** r is 127 if bit 0 of the noise register is 1, else −127. Then
the phase becomes (phase + increment) mod 2^32, and if that addition
carried (the sum was 2^32 or more), the noise register n becomes (n >> 1)
| (((n XOR (n >> 1)) AND 1) << 14): a 15-bit linear feedback shift
register of period 32,767. So the noise changes increment × 48,000 ÷ 2^32
times a second. Modulation and feedback do not apply.

**Sample slot s**, of length len (0–65,536) and loop start ls. The phase
is a position in the sample with 16 fraction bits. A position p is
**brought into** the sample: if p < len × 65,536 it stays; else, if ls <
len, it becomes ls × 65,536 + ((p − ls × 65,536) mod ((len − ls) ×
65,536)); else the sample has **ended**. Then:

- if the voice's Ended flag is set, r is 0;
- else the phase is brought into the sample. If the sample has ended, the
  flag is set, the phase becomes 0 and r is 0. Otherwise r is the
  sample's byte at (phase >> 16), and the phase becomes phase + increment
  (computed without a 32-bit limit) brought into the sample; if that has
  ended, the flag is set and the phase becomes 0.

Key on clears Ended (section 18.3). Modulation and feedback do not apply.
A slot loaded again while a voice plays it is read as it now is.

### 18.7 Presentation

The 800 pairs of each frame are what the host plays; they are not machine
state and the program cannot read them. In a system (section 12) each
machine has its own sound device; the host adds the pairs of every
machine whose frame ran in a system frame, sample by sample, and clamps
the sums to −32,768 … 32,767. A host may buffer, delay, resample, drop or
not play the samples, as long as nothing in the machines changes: a host
with no audio output has the device all the same (it is present, its
state advances as above, and nothing is heard), as a headless host has a
terminal nobody sees. `terse run --audio-out` and `terse-bus --audio-out`
write the mixed samples to a WAV file instead of playing them.

### 18.8 Determinism

The device's state is every voice's registers, the wavetables and the
sample slots (`STATE.md` 1.12). It takes no input from the host. The
samples a frame renders are a function of that state, so they are the
same on every host and after a restore; a test compares implementations
by hashing them (`programs/tune/expected-audio.txt`).
