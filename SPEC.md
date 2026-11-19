# Terse machine specification — v0 (draft)

Status: draft for the first prototype. Everything here may change until v1;
after v1 a published machine profile and published device versions never
change. Section 15 lists the numbers that are placeholders until hosts have
been measured.

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
   spent executing instructions (section 13).
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
   (section 9.2).
2. Adds 125,000 to the budget, subtracts the charge for content delivered
   in step 1 (section 13), and runs the CPU while the budget is greater
   than zero.
3. Ends the frame: the frame counter increases by one, and the host presents
   the terminal, the vector display and the page as they are at this moment.

Rules:

- An instruction fetched from ROM costs 1 unit; one fetched from RAM costs
  4. An `ecall` costs 1 (or 4 from RAM) plus the charge of the call it makes
  (section 13).
- An instruction that starts with a positive budget runs to completion, so
  the budget can end below zero. A negative remainder carries into the next
  frame; it may be larger than a frame's budget, in which case that frame
  runs nothing and the rest carries on. Unused budget never carries forward.
- When the budget runs out the machine simply pauses: the next frame
  continues with the next instruction, and the program is not told.
- A program that has finished its work for the frame calls `wait_frame`
  (section 4): the rest of the budget is discarded and the call returns at
  the start of the next frame.
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
| 10 | 1 | `n_dev`: number of device requirements, 0 to 16 |
| 11 | 5 | Reserved, must be zero |
| 16 | 16 | Title, UTF-8, padded with `00h` |
| 32 | 2 × `n_dev` | Device requirements: (device slot, device version) pairs |
| 32 + 2 × `n_dev` | `rom_len` | ROM contents; byte 0 is at address `0001_0000h` |

- The file length must equal `32 + 2 × n_dev + rom_len` exactly.
- The **image ID** is the BLAKE3-256 hash of the whole file.
- A device slot may be listed at most once.
- A host must refuse an image whose profile or required device versions it
  does not implement. This holds for an image a shell starts too: the start
  fails (section 12.2, reason 3) and the machine never runs. A device not
  listed in the header behaves as absent.
- Device 1 (system) is always present and need not be listed.

| Slot | Device | Version | Status in v0 |
|---|---|---|---|
| 1 | System | 0 | specified (sections 4–6) |
| 2 | Terminal | 0 | specified (section 8) |
| 3 | 2D vector display | 0 | specified (section 10) |
| 4 | Content | 0 | specified (section 9) |
| 5 | Messages | — | reserved |
| 6 | Sound | — | reserved |
| 7 | Page | 0 | specified (section 11) |
| 8 | Machines | 0 | specified (section 12); only the shell of a system has it |
| 10 | 3D vector display | — | reserved |

Other slots are free; 0, 9 and 11–255 are not devices in v0.

---

## 3. Calls

### 3.1 Convention

A program reaches a device with `ecall`. Register `a7` (`x17`) holds the
call number, `(device slot << 8) | function`; arguments are in `a0`–`a5`;
the result is in `a0`, and a second result in `a1` where listed. A call
changes no register but `a0` (and `a1` where listed) and no memory but the
buffers it is said to write. `pc` continues after the `ecall`.

- An unknown call, or a call to a device the image does not list, returns
  `FFFF_FFFFh` and does nothing else.
- A **buffer** is a pointer and a length in bytes. A buffer the device
  reads must lie wholly in ROM or wholly in RAM; one it writes must lie
  wholly in RAM. An empty buffer (length 0) may also start just past the
  end of ROM or RAM. Otherwise the call returns
  `FFFF_FFFEh` and does nothing else; this is not a fault. Two calls have
  their own rule for this: a display list not in RAM is rejected
  (section 10.2), and a content request block not in RAM is rejected with
  handle 0 (section 9.2).
- Buffers need no alignment.
- Every call is charged, also when it fails (section 13).

### 3.2 Call table

| `a7` | Call | Arguments | Result |
|---|---|---|---|
| `100h` | `wait_frame` | — | the frame counter in the frame it returns in |
| `101h` | `next_event` | `a0` = buffer (4 bytes, written) | 1 and the event, or 0 if the queue is empty |
| `102h` | `frame_count` | — | the frame counter |
| `103h` | `unix_time` | — | the Unix time latched at the start of the frame |
| `104h` | `random` | `a0` = buffer (written), `a1` = length, at most 256 | the length |
| `105h` | `controller` | `a0` = controller number, `a1` = buffer (8 bytes, written) | byte 0 of the state |
| `106h` | `exit` | `a0` = exit code (low 8 bits) | does not return |
| `107h` | `open` | `a0` = address of an open request (read) | 1 passed to the shell, 0 not |
| `108h` | `argument` | `a0` = buffer (32 bytes, written) | 1, or 0 if the machine has none |
| `200h` | `write` | `a0` = buffer (read), `a1` = length | bytes consumed |
| `201h` | `command` | `a0` = command | 0 |
| `202h` | `set_cursor` | `a0` = column, `a1` = row | 0 |
| `203h` | `get_cursor` | — | `a0` = column, `a1` = row |
| `204h` | `set_colours` | `a0` = foreground, `a1` = background | 0 |
| `205h` | `get_colours` | — | `a0` = foreground, `a1` = background |
| `206h` | `cells_read` | `a0` = first cell, `a1` = buffer (written), `a2` = count | the count |
| `207h` | `cells_write` | `a0` = first cell, `a1` = buffer (read), `a2` = count | the count |
| `208h` | `palette_set` | `a0` = first entry, `a1` = buffer (read), `a2` = count | the count |
| `209h` | `size` | — | `a0` = 80 columns, `a1` = 30 rows |
| `300h` | `submit` | `a0` = list address, `a1` = length | 0 done, 1 rejected |
| `301h` | `clear` | — | 0 |
| `302h` | `style` | `a0` = style | 0 done, 1 rejected |
| `303h` | `persistence` | `a0` = persistence | 0 done, 1 rejected |
| `304h` | `fill` | `a0` = fill mode | 0 done, 1 rejected |
| `400h` | `submit` | `a0` = address of a request block | a handle 1–8, or 0 if rejected |
| `700h` | `submit` | `a0` = document address, `a1` = length | 0 done, 1 rejected |
| `701h` | `clear` | — | 0 |
| `702h` | `region` | `a0` = x, `a1` = y, `a2` = width, `a3` = height | 0 |
| `703h` | `scroll` | `a0` = y | 0 |
| `704h` | `background` | `a0` = colour | 0 |
| `705h` | `locate` | `a0` = anchor id | 0 done, 1 rejected |
| `800h` | `start` | `a0` = address of an image ID (32 bytes, read) | a handle 1–8, or 0 if none is free |
| `801h` | `stop` | `a0` = handle | 1, or 0 if the handle is not in use |
| `802h` | `place` | `a0` = handle, `a1` = x, `a2` = y, `a3` = width, `a4` = height | 1, or 0 if the handle is not in use |
| `803h` | `run` | `a0` = handle, `a1` = 1 run, 0 pause | 1, or 0 if refused |
| `804h` | `focus` | `a0` = handle, or 0 | 1, or 0 if refused |
| `805h` | `info` | `a0` = handle, `a1` = buffer (48 bytes, written) | 1, or 0 if the handle is not 1–8 |
| `806h` | `builtin` | `a0` = index, `a1` = buffer (32 bytes, written) | 1, or 0 past the last |
| `807h` | `start_program` | `a0` = address of a program record (read) | a handle 1–8, or 0 if none is free |
| `808h` | `identity` | `a0` = handle, `a1` = buffer (81 bytes, written) | 1, or 0 if it has none |
| `809h` | `request` | `a0` = handle, `a1` = buffer (82 bytes, written) | 1, or 0 if no request waits |
| `80Ah` | `argument` | `a0` = handle, `a1` = address of a blob ID (32 bytes, read) | 1, or 0 if the handle is not loading |
| `80Bh` | `snapshot` | `a0` = handle, `a1` = slot | 1 accepted, 0 refused |
| `80Ch` | `restore` | `a0` = slot | a handle 1–8, or 0 if none is free or the slot is out of range |
| `80Dh` | `snapshot_info` | `a0` = slot, `a1` = buffer (122 bytes, written) | 1 if the slot holds a snapshot, else 0 |
| `80Eh` | `snapshot_delete` | `a0` = slot | 1, or 0 if the slot is out of range |

Sections 4 to 12 define each call.

---

## 4. Device 1 — System

- **`wait_frame`** ends the frame (section 1.4) and returns the frame
  counter as it reads in the frame the call returns in (one more than
  before the call).
- **`next_event`** removes the event at the head of the queue (section 5)
  and writes its four bytes, type, `a`, `b`, `c`, to the buffer; it returns
  1. If the queue is empty it writes nothing and returns 0.
- **`frame_count`** returns the frame counter: N throughout frame N. Frame 0
  is the first frame after reset. It increases at the end of every frame
  and wraps from `FFFF_FFFFh` to 0.
- **`unix_time`** returns the Unix time in seconds (low 32 bits) that the
  host latched at the start of the frame.
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
  machine's state (`STATE.md` 1.11).

Events, the latched time, the random bytes and controller state are the
machine's only inputs from this device. A host that records them per frame
(with the content completions of section 9.4) can replay a run exactly.

## 5. Events

The queue holds at most 64 events. An event that arrives when the queue is
full is dropped.

| Type | Name | `a` | `b` | `c` |
|---|---|---|---|---|
| `01h` | Key down | key code | modifiers | 0 |
| `02h` | Key up | key code | modifiers | 0 |
| `03h` | Text | code point bits 0–7 | bits 8–15 | bits 16–20 |
| `04h` | Pointer | x bits 0–7 | y bits 0–7 | bits 0–1: x bits 8–9; bit 2: y bit 8; bits 3–5: buttons |
| `05h` | Content done | request handle | status | 0 |
| `06h` | Wheel | vertical steps, signed | horizontal steps, signed | 0 |
| `07h` | Feeds updated | 0 | 0 | 0 |
| `08h` | Page laid out | height bits 0–7 | bits 8–15 | bits 16–23 |
| `09h` | Page anchor | anchor id bits 0–7 | bits 8–15 | buttons |
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
- **Text** carries one Unicode scalar value produced by the host's text
  input, after keyboard layout and input-method composition. Text the user
  pastes arrives the same way, one event per character (U+000A for a line
  break; other control characters are dropped), no faster than the queue
  has room for.
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
F; every other character is narrow. A wide character occupies two cells in a
row: the left cell holds its code and the right cell holds the continuation
code `FFFFh`. Combining marks are not treated specially in v0; each takes a
cell.

### 8.2 Calls

- **`write`** puts characters (section 8.3) from the bytes of the buffer,
  in order, and returns how many bytes it consumed. Each byte is charged as
  it is consumed (section 13), and the call stops after the byte whose charge
  brings the budget to zero or below, so it may consume fewer bytes than
  asked; it always consumes at least one byte of a non-empty buffer. The
  program calls `write` again with the rest (the machine pauses first if the
  budget is used up). A UTF-8 sequence may be split across calls.
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
  (0–255 each). `first` must be below 256 and `count` at most 256.
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
   column 79 is first set to a space with the current colours.
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

"Cleared" means space (`0020h`) with the current colours.

### 8.5 Reset state and palette

Every cell is a space with foreground 7 and background 0. The cursor is at
(0, 0) and shown; pending wrap is clear and no UTF-8 sequence is pending.
The palette is the xterm 256-colour palette: entries 0–15 the standard and
bright colours, 16–231 the 6 × 6 × 6 colour cube, 232–255 the grey ramp.

### 8.6 Presentation

How the host draws the grid (font, size, cursor shape, smoothing) is not part
of this spec and cannot be observed by the program. Section 10.5 says how the
grid is combined with the vector display.

---

## 9. Device 4 — Content (version 0)

Fetches and publishes **blobs**. A blob is 1 to 1,048,576 bytes and is named
by its BLAKE3-256 hash. The host does all hashing, verification, caching and
transport; the program only ever receives bytes that match the hash it asked
for. How the host obtains blobs is described in `NETWORK.md` and cannot be
observed by the program except through timing of completions.

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
| 40 | 4 | `length`: get, load, records and find: bytes wanted, 1 to 16,384. On completion: bytes delivered, which is fewer if the blob or record ends first. Publish: the title's length, 0 to 120, read at submit. Set profile: the name's length, 1 to 48; set nickname: 0 to 48; read at submit. | both |
| 44 | 4 | `addr`: address of the data. Get, load, records and find: the `length` bytes there must lie in RAM. Put and save: the `size` bytes there must lie wholly in ROM or wholly in RAM. Publish: the title, `length` bytes of UTF-8, which must lie wholly in ROM or wholly in RAM. Find: also the query, the `size` bytes there, which must lie in RAM. Set profile and set nickname: the name, `length` bytes, which must lie wholly in ROM or wholly in RAM. | program |
| 48 | 4 | `size`: get: blob size, written on completion; load: the record's size, written on completion. Put: 1 to 1,048,576, read at submit; save: 0 to 65,536, read at submit. Records: the number of records that exist, written on completion. Find: the query's length, 1 to 48, read at submit; the number of results, written on completion. Publish and set profile: the new entry's `seq`, written on completion. | both |

### 9.2 Submit and completion

`submit` (`400h`) with `a0` = the block's address:

1. If the 52 bytes at `a0` do not lie wholly in RAM, the submit is rejected
   (it returns 0) and nothing is written.
2. If the request is invalid (unknown `op`, a length, size, scope or
   publish kind out of range, a data range not wholly where section 9.1 requires, a publish
   title or find query that is not valid UTF-8, a name that is not a valid
   display name, section 9.5, a record name that is not valid, section 9.6),
   the device writes status `04h` to the block and rejects the submit.
3. If eight requests are already pending, the submit is rejected and the
   block is not changed.
4. Otherwise the device writes status `00h`, assigns the lowest free handle
   (1–8), which the call returns, and the request becomes pending. For put
   and save, the device copies the `size` bytes at `addr` at this moment;
   for publish,
   the title; for find, the query; for set profile and set nickname, the
   name.

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
- identity, on success: writes `id`;
- in every case writes `status`, frees the handle, and queues a content-done
  event (section 5). If the event queue is full the event is dropped; the
  block is still updated.

The device writes to the block at the address given at submit, whatever the
program has since done with that memory. Completions in one frame are
applied in increasing handle order. A get whose `offset` is at or beyond the
end of the blob completes with status `01h` and `length` 0; so does a load
at or beyond the end of its record.

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
| `05h` | Refused: the user said no, the host does not permit it, or the relays refused a find (section 9.5) |
| `06h` | No room: a save would exceed the owner's limits (section 9.6) |

### 9.4 Determinism

Completions (which handle, in which frame, with what status, size and bytes)
are inputs to the machine, alongside those of section 4. A request waiting
for the user's consent (section 9.5) is simply pending; the user's answer
reaches the machine only as its completion. The failed completions of a
restore (section 9.2) are inputs like any other.

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
with device 8's `start_program` (section 12.2).

**Subscribe** (`05h`) adds the feed whose key is `id` to the host's
subscriptions, and **unsubscribe** (`06h`) removes it (`NETWORK.md` section
4.4). `offset`, `length`, `addr` and `size` are not used. The host completes
with status `01h` once the subscription list has changed (the entries
arrive later, each announced as usual by a feeds-updated event); at once,
without asking, if the list already is as asked (subscribing to a feed
already subscribed, unsubscribing from one that is not); and with `05h`
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
`seq`.

**Identity** (`08h`): on completion `id` is the host's own feed key, or the
status is `02h` if the host has no identity yet. `offset`, `length`, `addr`
and `size` are not used.

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
| 1 | 1 | Flags. Bit 0: the name is the user's local nickname for the feed. Bit 1: the feed has a profile in force (`NETWORK.md` section 4.6). Bit 2: the feed is not subscribed (it is listed for its nickname). Other bits are zero. |
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
`03h`.

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
already is as asked.

**Consent.** Subscribe, unsubscribe, publish, set profile and set nickname
change what the user follows, speak in the user's name or change what the
user sees, so the host asks the user before it carries one out, and the
request stays pending until the user answers. The host draws its own prompt
over the bottom of the screen, on top of everything the program shows; the
program cannot draw it, read it or answer it. The prompt names the image's
title (in a system, that of the machine whose request it is) and what is
asked: the feed key (for example as its first and last
six hex digits, `3fa9c2…c21e07`); whether an article or a document is
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
rules as get and put.

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
`start`, or from a file) has the records of its exact image ID. A program
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
article (`NETWORK.md` 4.5). A document is published as an entry of kind
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
  command. The page's figures are outlined too (section 11.4).

### 10.4 Fill cap

When filled shapes are filled (the clean style with fill mode 0), the area
they cover in one list is limited. Each disc, rectangle and triangle, in list
order, counts the area of its bounding box: for a disc the square from
(x − r, y − r) to (x + r, y + r), for a rectangle the rectangle itself, for a
triangle the smallest axis-aligned box holding its three points, with
coordinates as resolved in section 10.3. The box is clipped to the screen
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
3. the vector display;
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
of the list. It takes no input from the host.

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
end of the bytes, at an unknown opcode or at a command whose operands are
cut off; the commands before that are used.

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
  (left), 1 centre, 2 end (right), anything else 0. A figure ends the
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
  and neither is any path after it. (1) At most 256 paths. (2) Area: a path
  counts the box of all its points (start, control and end points), clipped
  to the figure's box, once if it is filled, and that box grown by half the
  stroke width on every side, clipped, once more if it is stroked; each box's
  area in square screen units is its width times its height in sixteenths,
  divided by 256 and rounded up. The total is at most 8 × width × height of
  the figure and at most 4 × 640 × 480 = 1,228,800. (3) Length: a path's
  length L is, over its Line, Quadratic and Cubic segments, the sum of
  |dx| + |dy| of each step from the segment's start through its control
  points to its end, in sixteenths. A path counts 2 × L + 4 × w × s if it is
  filled, with w = 16, and again with w its stroke width if it is stroked,
  where s is its number of segments; the counts are divided by 16 and
  rounded up, and the total is at most 65,536.

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

Figures are drawn as outlines when the vector display is in the phosphor
style or in fill mode 1 (section 10.3; an image without the vector display
has the clean style and fill mode 0): nothing is filled, and a path that is
filled but not stroked is stroked one unit wide in its fill colour.

Cost stays bounded: a document is at most 65,536 bytes; the host lays it out
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
  `a`, `b` = the anchor id, low then high byte; `c` = the buttons, with the
  same bits as in a pointer event; also when it is pressed over a figure
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
- A locate gives the y and height of the first line (a figure is a line)
  with a character or figure of that anchor.
- The height is the sum of the spaces, line heights, rules and figures.

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
system has a **focus**: a handle, or 0 for the shell; it is 0 at reset.

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
  byte) and the name (n bytes, a valid display name, section 9.5). The 33 +
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
  A machine being restored keeps the argument in its snapshot; one given
  to it is ignored.
- **`snapshot`** with a handle that is running or paused and a slot 0–15:
  the host keeps the machine's snapshot (section 12.7) in that slot,
  replacing what was there, taken at the end of this system frame (step
  6 of section 12.5, after the machine's own frame if it runs one). Returns
  1; returns 0 and does nothing for any other handle or slot. The result
  arrives as a snapshot done event (section 12.4): status 1 stored, 3
  failed (the machine was stopped in the same frame, or the host could not
  store it).
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
  image cannot be had or run. The host may take any number of frames. A
  slot out of range returns 0.
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
  | 118 | 4 | When it was taken: the Unix time latched for the machine's last frame |

- **`snapshot_delete`** with a slot 0–15: the slot is empty from now on
  (for `snapshot_info` at once); the host removes its snapshot at the end
  of the frame, in order with the snapshots asked for in the same frame.
  Returns 1, or 0 for a slot out of range.

### 12.3 Screens

A machine's **picture** is its screen scaled uniformly into its rectangle
(x, y, w, h) and centred. If w × 480 ≤ h × 640, the picture is
pw = w wide and ph = ⌊w × 480 / 640⌋ high, otherwise pw = ⌊h × 640 / 480⌋
and ph = h; it starts at px = x + ⌊(w − pw) / 2⌋, py = y + ⌊(h − ph) / 2⌋.
If pw or ph is 0 nothing is shown. A machine that is running, paused,
exited or faulted has a picture; one that is loading or failed has none.
Exited and faulted machines show the screen as it was at their last frame.

The host shows, from the bottom:

1. the shell's screen, over the whole of the host's screen as a single
   machine's would be, combined as section 10.5 says;
2. each machine's picture, in increasing handle order, each a complete
   screen (its own layers, style, afterglow and page, sections 10.5, 10.6,
   11) scaled into its picture and covering what is below it; outside the
   picture, the rest of the rectangle shows what is below;
3. the host's own prompts (section 9.5), over everything; a consent prompt
   names the image title of the machine whose request it is.

How the host scales a screen (fonts rasterised for the picture's size,
smoothing) is presentation (section 8.6).

### 12.4 Input

The host's input events (section 5) are in the shell's screen units. In each
frame, before the shell runs, they are routed in order by the focus and the
pictures as they are at that moment:

| Input | Goes to |
|---|---|
| Key down, key up, text, wheel | the machine with the focus if it is running; the shell if the focus is 0; otherwise nobody |
| Pointer, over a picture | the machine whose picture it is (the highest handle where pictures overlap), mapped into its screen units, if it is running; otherwise nobody |
| Pointer, over no picture | the shell, unchanged |
| Controllers (section 6) | the machine with the focus; every other machine reads every controller as not connected |
| Content done, feeds updated, page events | each machine its own (sections 9, 11.5) |

A position (x, y) over a picture (px, py, pw, ph) is mapped to
x′ = ⌊(2(x − px) + 1) × 640 / (2pw)⌋ and y′ = ⌊(2(y − py) + 1) × 480 / (2ph)⌋.
A pointer event that presses a button (sets a button bit that was clear in
the previous pointer event, wherever that went) over a picture also gives
the shell a machine pressed event, in the place among the shell's events
that the pointer event had in the input.

**The home key.** A host reserves one key (or other input) as its home key;
which one is up to the host. It is never delivered to any machine as a key
event. When it is pressed, the focus becomes 0 and a home event is queued
for the shell.

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
3. **The shell** runs its frame (section 1.4).
4. **Starts and stops** the shell made in its frame are carried out in the
   order it made them, with the arguments it gave. A machine started in
   this frame is loading: its first frame is at the earliest in the next
   system frame, the one in whose step 1 it becomes running. A request
   the shell read with `request` in its frame no longer waits, so the
   machine's next `open` (in step 5) can make another.
5. **The machines** that are running now, in increasing handle order, each
   run one frame (section 1.4) with a full budget and their own carry. So a
   `run`, `stop` or `place` made by the shell takes effect in the same
   frame; a `focus` takes effect for the next frame's routing.
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
delivers, the built-in programs and what `snapshot_info` tells. For a machine
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

## 13. Charges

Every call is charged in budget units on top of the `ecall` instruction
itself (1 from ROM, 4 from RAM). Charges are subtracted from the budget as
the call runs and may take it below zero (section 1.4).

| What | Charge |
|---|---|
| Any call, including one that fails | 4 |
| `write`: each byte consumed | 1 |
| A clear, scroll up or scroll down, including one caused by `write` (`0Ah` on the last row, `0Ch`, a wrap) | 150 |
| Bytes copied between memory and a device: `next_event` (4 when an event is written), `random`, `controller` (8), `cells_read`, `cells_write`, `palette_set`, an accepted vector `submit`, an accepted content put or save (its `size` bytes), publish (its title), find (its query), set profile or set nickname (its name), an accepted page `submit`, device 8's `start` (32, when the ID is read), `start_program` (33 + n, when the record is read), `info` (48, when written), `builtin` (32, when written), `identity` (81, when written), `request` (82, when written), `argument` (32, when the ID is read) and `snapshot_info` (122, when written); `open` (33, or 34 + n, when the request is read) and the system's `argument` (32, when written) | 1 per 4 bytes, rounded up, per call |
| Content delivered at completion: the bytes a get, load, records (subscriptions, timeline, names, programs, records) or find request writes to `addr` | 1 per 4 bytes, rounded up, per frame, subtracted from that frame's budget in step 2 of section 1.4 |

Nothing else is charged: a rejected submit or put copies nothing, the
fields a completion writes to a request block are free, and so is laying out
the page, which the host does outside the machine's frames (section 11.4).

The charges are chosen so that a frame spent entirely in any one kind of call
costs a host no more per unit than a frame of instructions from ROM;
`crates/terse-core/tests/charges.rs` measures this.

---

## 14. Conformance

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

## 15. Provisional values

These numbers are placeholders until weak hosts have been measured:

- the budget, 125,000 units per frame (section 1.4);
- the cost of an instruction fetched from RAM, 4 (section 1.4);
- the charges of section 13 (4 per call, 1 per `write` byte, 1 per 4 bytes
  copied, 150 per clear or scroll);
- the fill cap, 4 × 640 × 480 square units (section 10.4);
- the figure caps: 256 paths, the area (8 × the figure's, at most
  4 × 640 × 480) and the length (65,536) of section 11.3;
- the number of machines a shell can start, 8, and of its snapshot slots,
  16 (section 12).

## 16. Not yet specified

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
- Devices 5, 6 and 10.
- On the page: bitmap images, arcs and gradients in figures, links the host
  follows itself, selecting and copying
  text, and fonts beyond the three faces.
- A limit on the area covered by lines and circle outlines. The fill cap
  does not apply to them, so a legal list of about 3,000 maximum-width lines
  across the whole screen still costs a host far more fill than four
  screens.
- In a system (section 12): communication between machines (an open
  request carries one blob ID or program identity, and its machine is not
  told what became of it); arguments other than one blob ID; budgets
  other than one full budget per machine per frame; giving a machine less
  than the whole screen; sound for several machines; what a host shows for
  a machine with no picture.
- Snapshots (section 12.7): saving them without the shell asking (a host
  has no shutdown notification yet), snapshots of the shell itself, moving
  a snapshot to another host, and a machine asking to be snapshotted.
- Named records (section 9.6): moving records from one owner to another
  (for example when a program run from a file is later published), keeping
  them in step between a user's machines, and sharing a record with
  another program. Starting a version of a program other than the current
  one.
