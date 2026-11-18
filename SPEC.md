# Terse machine specification — v0 (draft)

Status: draft for the first prototype. Everything here may change until v1;
after v1 a published machine profile and published device versions never
change. Section 13 lists the numbers that are placeholders until hosts have
been measured.

Terse is a small virtual computer: a 32-bit RISC-V CPU (RV32IMC), a program
of at most 128 KiB, at most 1 MiB of RAM, a fixed budget of work per frame,
and a set of host-side devices reached through system calls. One program runs
in one machine. Programs are identified by the hash of their image and have
no dependencies.

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
   spent executing instructions (section 11).
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
   in step 1 (section 11), and runs the CPU while the budget is greater
   than zero.
3. Ends the frame: the frame counter increases by one, and the host presents
   the terminal and the vector display as they are at this moment.

Rules:

- An instruction fetched from ROM costs 1 unit; one fetched from RAM costs
  4. An `ecall` costs 1 (or 4 from RAM) plus the charge of the call it makes
  (section 11).
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
  does not implement. A device not listed in the header behaves as absent.
- Device 1 (system) is always present and need not be listed.

| Slot | Device | Version | Status in v0 |
|---|---|---|---|
| 1 | System | 0 | specified (sections 4–6) |
| 2 | Terminal | 0 | specified (section 8) |
| 3 | 2D vector display | 0 | specified (section 10) |
| 4 | Content | 0 | specified (section 9) |
| 5 | Messages | — | reserved |
| 6 | Sound | — | reserved |
| 10 | 3D vector display | — | reserved |

Other slots are free; 0 and 7–9, 11–255 are not devices in v0.

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
- Every call is charged, also when it fails (section 11).

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

Sections 4 to 10 define each call.

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

- **Key codes** are USB HID keyboard usage IDs (for example `04h` = A,
  `28h` = Enter, `29h` = Escape, `4Fh`–`52h` = Right, Left, Down, Up).
- **Modifiers:** bit 0 Shift, bit 1 Control, bit 2 Alt, bit 3 Meta.
- **Text** carries one Unicode scalar value produced by the host's text
  input, after keyboard layout and input-method composition.
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
  the timeline's "local" flags are already correct.

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
x from `column × 8` and y from `row × 16`. Pointer positions and the vector
display use these units. Units are square: the host shows the screen at a
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
  it is consumed (section 11), and the call stops after the byte whose charge
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
| 0 | 1 | `op`: `01h` get, `02h` put (`03h`, `04h`: section 9.5) | program |
| 1 | 1 | `status` (section 9.3) | device |
| 2 | 2 | Reserved; ignored | — |
| 4 | 32 | `id`: blob hash | program for get; device for put |
| 36 | 4 | `offset`: get: byte offset into the blob; records: index of the first record | program |
| 40 | 4 | `length`: get and records: bytes wanted, 1 to 16,384. On completion: bytes delivered, which is fewer if the blob ends first. | both |
| 44 | 4 | `addr`: address of the data. Get and records: the `length` bytes there must lie in RAM. Put: the `size` bytes there must lie wholly in ROM or wholly in RAM. | program |
| 48 | 4 | `size`: get: blob size, written on completion. Put: 1 to 1,048,576, read at submit. Records: the number of records that exist, written on completion. | both |

### 9.2 Submit and completion

`submit` (`400h`) with `a0` = the block's address:

1. If the 52 bytes at `a0` do not lie wholly in RAM, the submit is rejected
   (it returns 0) and nothing is written.
2. If the request is invalid (unknown `op`, a length or size out of range, a
   data range not wholly where section 9.1 requires), the device writes
   status `04h` to the block and rejects the submit.
3. If eight requests are already pending, the submit is rejected and the
   block is not changed.
4. Otherwise the device writes status `00h`, assigns the lowest free handle
   (1–8), which the call returns, and the request becomes pending. For put,
   the device copies the `size` bytes at `addr` at this moment.

At the start of a later frame (never the frame of the submit), in step 1 of
section 1.4, the host may complete any pending requests. Completing a
request:

- get, on success: copies the delivered bytes to `addr`, writes `length`
  and `size`;
- subscriptions and timeline, on success: copies the records to `addr`,
  writes `length` and `size` (section 9.5);
- put, on success: writes `id`;
- in every case writes `status`, frees the handle, and queues a content-done
  event (section 5). If the event queue is full the event is dropped; the
  block is still updated.

The device writes to the block at the address given at submit, whatever the
program has since done with that memory. Completions in one frame are
applied in increasing handle order. A get whose `offset` is at or beyond the
end of the blob completes with status `01h` and `length` 0.

### 9.3 Status values

| Value | Meaning |
|---|---|
| `00h` | Pending |
| `01h` | Done |
| `02h` | Not found |
| `03h` | Failed (network error or timeout; the host decides when to give up) |
| `04h` | Bad request |

### 9.4 Determinism

Completions (which handle, in which frame, with what status, size and bytes)
are inputs to the machine, alongside those of section 4.

### 9.5 Feeds

Two further operations read the feeds the host has subscribed to
(`NETWORK.md` section 4). They use the same request block, call, handles and
completion rules as get. `id` is not used. Managing subscriptions and
publishing are done by the host, not by programs, in v0.

| `op` | Operation |
|---|---|
| `03h` | Subscriptions |
| `04h` | Timeline |

For both, `length` is the size of the buffer at `addr` (1 to 16,384) and
`offset` is the index of the first record wanted. The device delivers as many
whole records as fit, writes the number of bytes delivered to `length` and the
total number of records that exist to `size`.

**Subscriptions** delivers one 36-byte record per subscribed feed, in
increasing order of feed key:

| Offset | Size | Field |
|---|---|---|
| 0 | 32 | Feed key |
| 32 | 4 | Highest `seq` held for that feed, or 0 |

**Timeline** delivers the entries of all subscribed feeds, newest first by
the order in which the host received them. Index 0 is the most recently
received entry (`NETWORK.md` section 4.4 defines the order). Entries of a
feed the host has unsubscribed from are not included. Each record is:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `L`: number of bytes that follow in this record |
| 1 | 1 | Flags. Bit 0: the article blob is in the host's store, so a get will complete without the network. Other bits are zero. |
| 2 | `L − 1` | The entry without its signature (`NETWORK.md` section 4.2, bytes 0 to 108 + `n`) |

The host has already verified every entry it delivers. When new entries
arrive, the program is told by a feeds-updated event (section 5) and can
read the timeline again.

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
  command.

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
2. the vector display;
3. the terminal's characters and cursor.

So text is always drawn over vector shapes, and a cell's background colour
is always behind them. A program that wants a panel of text to hide the
shapes behind it draws a filled rectangle at the end of its list; the text
still appears above it. Smoothing of edges and the exact colour of blended
pixels are up to the host and cannot be observed by the program.

### 10.6 Phosphor style

In the phosphor style the whole screen imitates a vector monitor or
oscilloscope tube:

- The screen is black: cell background colours are not drawn.
- Shapes are drawn as outlines (section 10.3) and add light to what is
  beneath them instead of covering it; where lines cross, the crossing is
  brighter. Opacity acts as brightness.
- The terminal's characters and cursor are light on the same tube, in their
  foreground colour, drawn in the same way as the shapes.
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

## 11. Charges

Every call is charged in budget units on top of the `ecall` instruction
itself (1 from ROM, 4 from RAM). Charges are subtracted from the budget as
the call runs and may take it below zero (section 1.4).

| What | Charge |
|---|---|
| Any call, including one that fails | 4 |
| `write`: each byte consumed | 1 |
| A clear, scroll up or scroll down, including one caused by `write` (`0Ah` on the last row, `0Ch`, a wrap) | 150 |
| Bytes copied between memory and a device: `next_event` (4 when an event is written), `random`, `controller` (8), `cells_read`, `cells_write`, `palette_set`, an accepted vector `submit`, an accepted content put (its `size` bytes) | 1 per 4 bytes, rounded up, per call |
| Content delivered at completion: the bytes a get or records request writes to `addr` | 1 per 4 bytes, rounded up, per frame, subtracted from that frame's budget in step 2 of section 1.4 |

Nothing else is charged: a rejected submit or put copies nothing, and the
fields a completion writes to a request block are free.

The charges are chosen so that a frame spent entirely in any one kind of call
costs a host no more per unit than a frame of instructions from ROM;
`crates/terse-core/tests/charges.rs` measures this.

---

## 12. Conformance

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

## 13. Provisional values

These numbers are placeholders until weak hosts have been measured:

- the budget, 125,000 units per frame (section 1.4);
- the cost of an instruction fetched from RAM, 4 (section 1.4);
- the charges of section 11 (4 per call, 1 per `write` byte, 1 per 4 bytes
  copied, 150 per clear or scroll);
- the fill cap, 4 × 640 × 480 square units (section 10.4).

## 14. Not yet specified

- Font selection, combining marks and complex scripts in the terminal.
- Names (signed pointers to blobs), ranges verified without fetching the
  whole blob, and cancelling a content request.
- Devices 5, 6 and 10.
- A limit on the area covered by lines and circle outlines. The fill cap
  does not apply to them, so a legal list of about 3,000 maximum-width lines
  across the whole screen still costs a host far more fill than four
  screens.
- Starting other machines, and messages between machines.
- Snapshots.
