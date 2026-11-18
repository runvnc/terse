# Terse machine specification — v0 (draft)

Status: draft for the first prototype. Everything here may change until v1; after
v1 a published machine profile and published device versions never change.

Terse is a small virtual computer: an exact Zilog Z80, a program of at most
128 KB, at most 1 MB of RAM, a fixed clock, and a set of host-side devices
reached through I/O ports. One program runs in one machine. Programs are
identified by the hash of their image and have no dependencies.

Design rules that every part of this spec follows:

1. **Fixed speed.** A program receives the same number of CPU cycles per frame
   on every host.
2. **Deterministic.** Machine state is a pure function of the image and the
   recorded inputs (events, clock readings, random bytes).
3. **Devices present and transport; they do not compute for the program.**
   Device commands have bounded cost, are not programmable, and nothing the
   host renders can be read back by the program.
4. **Small enough to reimplement.** An independent implementation of this
   document should be a few thousand lines.

All multi-byte values are little-endian. `h` suffix means hexadecimal.

---

## 1. Machine profile 1

| Property | Value |
|---|---|
| CPU | Zilog Z80, including undocumented instructions and flags (must pass ZEXALL) |
| Clock | 300,000 T-states per frame |
| Frame rate | 60 frames per second, nominal |
| ROM | 1 byte to 131,072 bytes (16 pages of 8 KB), read-only |
| RAM | 4 to 128 pages of 8 KB (32 KB to 1 MB), zero-filled at reset |
| I/O | 256 ports; the upper address byte during I/O is ignored |

### 1.1 Memory map

The 64 KB address space is eight **slots** of 8 KB. Slot `n` covers addresses
`n × 2000h` to `n × 2000h + 1FFFh`. Each slot shows one **page**, selected by
that slot's register.

| Page number | Meaning |
|---|---|
| `00h`–`0Fh` | ROM page. Reads beyond the end of the image return `FFh`. Writes are ignored. |
| `10h`–`7Fh` | Unmapped. Reads return `FFh`. Writes are ignored. |
| `80h`–`FFh` | RAM page `n − 80h`. Pages at or above the image's declared RAM size are unmapped. |

Code may execute from any mapped page, ROM or RAM.

### 1.2 Reset state

- Slots 0–3 show ROM pages `00h`–`03h`. Slots 4–7 show RAM pages `80h`–`83h`.
- `PC = 0000h`, `SP = 0000h`. All other registers, including the alternate
  set, `I` and `R`, are zero. `IFF1 = IFF2 = 0`. Interrupt mode 0.
- All RAM is zero. All devices are in their reset state.

Execution starts at address `0000h`, which is the first byte of the ROM. The
restart vectors (`08h`–`38h`) are ordinary ROM locations.

### 1.3 Frames, the clock and the interrupt

Time advances in frames. For each frame the host does the following, in order:

1. Appends this frame's input events to the event queue (section 4.1) and
   latches the clock (section 4.3).
2. Sets the frame interrupt request.
3. Adds 300,000 to the T-state budget and runs the CPU while the budget is
   greater than zero.
4. Commits device state for presentation (the display shows what the
   terminal grid contains at this moment).

Rules:

- An instruction that starts with a positive budget runs to completion. A
  negative remainder carries into the next frame.
- If the CPU is halted (`HALT`) and no interrupt can wake it, the rest of the
  frame's budget is discarded. Unused budget never carries forward.
- The frame interrupt is the only interrupt source. It is maskable. The
  request stays set until the CPU accepts it; it is not counted twice if a
  whole frame passes with interrupts disabled. There is no NMI.
- During interrupt acknowledge the data bus reads `FFh`. So mode 0 and mode 1
  both execute `RST 38h`, and mode 2 reads its vector from `(I × 100h) + FFh`.
- Accepting the interrupt is a step of its own and costs 13 T-states in
  modes 0 and 1, and 19 in mode 2.
- While the CPU is halted, `PC` holds the address of the `HALT` instruction,
  and `R` does not advance during discarded time.
- Device charges (see each device) are subtracted from the budget exactly as
  if the I/O instruction had taken that many more T-states.

A host that falls behind real time lets virtual time lag. It must not run
extra frames to catch up.

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
| 9 | 1 | `ram_pages`: number of 8 KB RAM pages, 4 to 128 |
| 10 | 1 | `n_dev`: number of device requirements, 0 to 16 |
| 11 | 5 | Reserved, must be zero |
| 16 | 16 | Title, UTF-8, padded with `00h` |
| 32 | 2 × `n_dev` | Device requirements: (device slot, device version) pairs |
| 32 + 2 × `n_dev` | `rom_len` | ROM contents; byte 0 is loaded at ROM page 0, offset 0 |

- The file length must equal `32 + 2 × n_dev + rom_len` exactly.
- The **image ID** is the BLAKE3-256 hash of the whole file.
- A device slot may be listed at most once.
- A host must refuse an image whose profile or required device versions it
  does not implement. A device not listed in the header behaves as absent.
- Device 0 (memory) and device 1 (system) are always present and need not be
  listed.

---

## 3. Port map

The high four bits of the port number select a device slot; the low four bits
select a register in that device.

| Ports | Slot | Device | Status in v0 |
|---|---|---|---|
| `00h`–`0Fh` | 0 | Memory | specified |
| `10h`–`1Fh` | 1 | System | specified |
| `20h`–`2Fh` | 2 | Terminal | specified |
| `30h`–`3Fh` | 3 | 2D vector display | specified |
| `40h`–`4Fh` | 4 | Content | specified |
| `50h`–`5Fh` | 5 | Messages | reserved |
| `60h`–`6Fh` | 6 | Sound | reserved |
| `70h`–`8Fh` | 7, 8 | — | free |
| `90h`–`9Fh` | 9 | V9938 (`98h`–`9Bh`) | reserved |
| `A0h`–`AFh` | A | 3D vector display | reserved |
| `B0h`–`FFh` | B–F | — | free |

Reading a port that is absent, reserved or undefined returns `FFh`. Writing to
one has no effect.

---

## 4. Device 0 — Memory (version 0)

| Port | Access | Function |
|---|---|---|
| `00h`–`07h` | read/write | Page register for slots 0–7 |
| `08h`–`0Fh` | — | Reserved (block copy) |

A write takes effect before the next instruction is fetched. Reads return the
last value written (or the reset value).

## 5. Device 1 — System (version 0)

| Port | Access | Function |
|---|---|---|
| `10h` | read | Type of the event at the head of the queue; `00h` if the queue is empty |
| `10h` | write | Remove the head event (any value) |
| `11h`–`13h` | read | Parameters `a`, `b`, `c` of the head event; `00h` if the queue is empty |
| `14h`–`17h` | read | Frame counter, 32 bits. Frame 0 is the first frame after reset. |
| `18h`–`1Bh` | read | Unix time in seconds, 32 bits, latched at the start of the frame |
| `1Ch` | read | One random byte from the host |
| `1Fh` | write | Exit: the machine stops. The value is the exit code. |

- The frame counter reads N throughout frame N and increases at the end of
  the frame.
- Exit stops the machine immediately after the `OUT`. The rest of that frame's
  budget is dropped; the frame still counts.
- Each read of `1Ch` consumes the next byte of the random bytes the host
  supplied for this frame. The host must supply enough for any frame
  (27,273 bytes covers the worst case).

### 5.1 Events

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
- **Pointer** positions are in screen units (section 5.4): x is 0–639 and y
  is 0–479. It is sent when the position or a button changes, at most once
  per frame for movement; a button change is sent at once, with the current
  position. Positions outside the screen are clamped to it, and nothing is
  sent when the pointer leaves the host's window. Buttons: primary,
  secondary, middle, from the lowest bit.
- **Wheel:** a positive vertical step means scroll down (towards later
  content); a positive horizontal step means scroll right. At most one wheel
  event is sent per frame, with each value from −127 to 127; the host adds
  up smaller movements until they make a whole step.
- **Feeds updated** is sent when the host has stored one or more new feed
  entries since the previous frame (section 7.7). At most one per frame. It
  is sent after the automatic download of those entries' articles has
  finished or failed, so the timeline's "local" flags are already correct.

### 5.2 Determinism

Events, the latched time, random bytes and controller state are the machine's
only inputs from this device. A host that records them per frame can replay a
run exactly.

### 5.3 Controllers

Up to four game controllers, numbered 0–3. Their state is latched at the
start of each frame.

| Port | Access | Function |
|---|---|---|
| `1Dh` | write | Select controller 0–3 (other values select none) and restart reading at byte 0 |
| `1Eh` | read | Next byte of the selected controller's state; after byte 7 it returns to byte 0 |

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

A controller that is not connected, or no selection, reads as all zeros.
After reset, controller 0 is selected. The read position is kept from one
frame to the next; only a write to `1Dh` restarts it. Axis values are never
−128.

### 5.4 Screen units

The screen is 640 units wide and 480 high, with (0, 0) at the top left. A
terminal cell is 8 units wide and 16 high, so the cell at (column, row) covers
x from `column × 8` and y from `row × 16`. Pointer positions and the graphics
devices use these units. Units are square: the host shows the screen at a
4:3 shape, so a circle is round.

## 6. Device 2 — Terminal (version 0)

A grid of 80 columns by 30 rows. The grid is held by the device, not in the
program's memory.

### 6.1 Cells

Each cell is four bytes:

| Byte | Field |
|---|---|
| 0 | Character code, low byte |
| 1 | Character code, high byte |
| 2 | Foreground palette index |
| 3 | Background palette index |

The character code is a Unicode code point in the range `0000h`–`FFFFh`. Cell
memory is 9,600 bytes; the cell at (column, row) starts at byte
`(row × 80 + column) × 4`.

A character is **wide** if its Unicode 16.0 East Asian Width property is W or
F; every other character is narrow. A wide character occupies two cells in a
row: the left cell holds its code and the right cell holds the continuation
code `FFFFh`. Combining marks are not treated specially in v0; each takes a
cell.

### 6.2 Ports

| Port | Access | Function |
|---|---|---|
| `20h` | write | Put character: one byte of a UTF-8 stream (section 6.3) |
| `21h` | read/write | Cursor column, 0–79 (writes are clamped) |
| `22h` | read/write | Cursor row, 0–29 (writes are clamped) |
| `23h` | read/write | Current foreground index |
| `24h` | read/write | Current background index |
| `25h` | write | Command (section 6.4) |
| `26h` | read/write | Cell pointer, low byte |
| `27h` | read/write | Cell pointer, high byte |
| `28h` | read/write | Byte of cell memory at the cell pointer; the pointer then increases by one and wraps from 9,599 to 0 |
| `29h` | read/write | Palette index |
| `2Ah` | write | Palette data: three writes set red, green, blue (0–255) of the indexed entry, then the palette index increases by one |
| `2Bh` | read | Number of columns (80) |
| `2Ch` | read | Number of rows (30) |

- The cell pointer is stored as written (16 bits) and reduced modulo 9,600
  when port `28h` uses it.
- Writing port `29h` restarts palette data at the red component. The palette
  index wraps from 255 to 0.

### 6.3 Put character

Bytes written to port `20h` are decoded as UTF-8. When a character is
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
`0Ah`, `0Ch` and `0Dh`, by a write to port `21h` or `22h`, and by command
`01h`. Ignored control characters leave it set.

The code point `FFFFh` sent through port `20h` is written as `FFFDh`, so put
character never creates a continuation cell on its own.

Writes through the cell pointer are not checked. The host draws a wide
character across two cells when the next cell in the row holds `FFFFh`; how
it draws any other arrangement is not specified.

Malformed UTF-8 and code points above `FFFFh` are written as `FFFDh`. Each
maximal invalid prefix of a sequence produces one `FFFDh` (the WHATWG rule),
and the byte that interrupted the sequence is decoded afresh. Overlong forms
and surrogates are malformed.

### 6.4 Commands

| Value | Command |
|---|---|
| `01h` | Clear: every cell becomes a space with the current colours; cursor to (0, 0) |
| `02h` | Scroll up one row; the bottom row is cleared |
| `03h` | Scroll down one row; the top row is cleared |
| `04h` | Clear from the cursor to the end of the row |
| `05h` | Show the cursor |
| `06h` | Hide the cursor |

"Cleared" means space (`0020h`) with the current colours.

### 6.5 Charges

Clear, scroll up and scroll down each cost 600 T-states, including when a
scroll or a clear (`0Ch`) is caused by put character. All other terminal operations are free.

### 6.6 Reset state and palette

Every cell is a space with foreground 7 and background 0. The cursor is at
(0, 0) and shown. The cell pointer and palette index are 0. The palette is the
xterm 256-colour palette: entries 0–15 the standard and bright colours,
16–231 the 6 × 6 × 6 colour cube, 232–255 the grey ramp.

### 6.7 Presentation

How the host draws the grid (font, size, cursor shape, smoothing) is not part
of this spec and cannot be observed by the program. Section 8.6 says how the
grid is combined with the vector display.

## 7. Device 4 — Content (version 0)

Fetches and publishes **blobs**. A blob is 1 to 1,048,576 bytes and is named
by its BLAKE3-256 hash. The host does all hashing, verification, caching and
transport; the program only ever receives bytes that match the hash it asked
for. How the host obtains blobs is described in `NETWORK.md` and cannot be
observed by the program except through timing of completions.

### 7.1 Linear RAM addresses

This device addresses RAM directly, independent of the slot registers. A
**linear RAM address** is a 24-bit value: RAM page `p` (page number
`80h + p`) occupies linear addresses `p × 2000h` to `p × 2000h + 1FFFh`. An
address at or beyond the declared RAM size is invalid.

### 7.2 Request block

A request is described by a 45-byte block that must lie entirely in RAM.

| Offset | Size | Field | Written by |
|---|---|---|---|
| 0 | 1 | `op`: `01h` get, `02h` put (`03h`, `04h`: section 7.7) | program |
| 1 | 1 | `status` (section 7.4) | device |
| 2 | 32 | `id`: blob hash | program for get; device for put |
| 34 | 3 | `offset`: byte offset into the blob (get) | program |
| 37 | 2 | `length`: bytes wanted, 1 to 16,384 (get). On completion: bytes delivered, which is fewer if the blob ends first. | both |
| 39 | 3 | `addr`: linear RAM address of the data | program |
| 42 | 3 | `size`: blob size in bytes. Get: written on completion. Put: 1 to 1,048,576, read at submit. | both |

### 7.3 Ports

| Port | Access | Function |
|---|---|---|
| `40h` | write | Block address, low byte |
| `41h` | write | Block address, high byte |
| `42h` | write | Submit (any value) |
| `42h` | read | Result of the last submit: a handle 1–8, or 0 if it was rejected |

Submit:

1. The block address is a 16-bit address in the current slot mapping. The
   device translates it to a linear RAM address; the block is the 45
   consecutive bytes of linear RAM from there. If the address is not in RAM
   or the block runs past the declared RAM, the submit is rejected and
   nothing is written.
2. If the request is invalid (unknown `op`, a length or size out of range, a
   data range not wholly inside declared RAM), the device writes status
   `04h` to the block and rejects the submit.
3. If eight requests are already pending, the submit is rejected and the
   block is not changed.
4. Otherwise the device writes status `00h`, assigns the lowest free handle
   and the request becomes pending. For put, the device copies the `size`
   bytes at `addr` at this moment.

At the start of a later frame (never the frame of the submit), before the CPU
runs, the host may complete any pending requests. Completing a request:

- get, on success: copies the delivered bytes to `addr`, writes `length`
  and `size`;
- put, on success: writes `id`;
- writes `status`, frees the handle, and queues a content-done event
  (section 5.1). If the event queue is full the event is dropped; the block
  is still updated.

Completions in one frame are applied in increasing handle order.

### 7.4 Status values

| Value | Meaning |
|---|---|
| `00h` | Pending |
| `01h` | Done |
| `02h` | Not found |
| `03h` | Failed (network error or timeout; the host decides when to give up) |
| `04h` | Bad request |

A get whose `offset` is at or beyond the end of the blob completes with
status `01h` and `length` 0.

### 7.5 Charges

An accepted put costs 1 T-state per byte of `size`. A rejected submit and
everything else are free.

### 7.6 Determinism

Completions (which handle, in which frame, with what status, size and bytes)
are inputs to the machine, alongside those in section 5.2.

### 7.7 Feeds

Two further operations read the feeds the host has subscribed to
(`NETWORK.md` section 4). They use the same request block, ports, handles
and completion rules as get. `id` is not used. Managing subscriptions and
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
arrive, the program is told by a feeds-updated event (section 5.1) and can
read the timeline again.

---

## 8. Device 3 — 2D vector display (version 0)

Draws lines and simple filled shapes from a **display list**: a short
sequence of drawing commands that the program builds in RAM and hands to the
device. The device keeps its own copy and the host draws it every frame until
the program submits another. The device does no geometry for the program: it
does not rotate, scale or clip against anything but the screen, and nothing
it draws can be read back.

### 8.1 Coordinates

Coordinates are signed 16-bit numbers in **sixteenths of a screen unit**
(section 5.4), little-endian. The screen's top left is (0, 0) and its bottom
right is (10240, 7680). Anything outside the screen is not shown.

### 8.2 Ports

| Port | Access | Function |
|---|---|---|
| `30h` | write | List address, bits 0–7 |
| `31h` | write | List address, bits 8–15 |
| `32h` | write | List address, bits 16–23 |
| `33h` | write | List length, bits 0–7 |
| `34h` | write | List length, bits 8–15 |
| `35h` | write | Command: `01h` submit, `02h` clear |
| `35h` | read | Result of the last command: `00h` done, `01h` rejected |

The address is a linear RAM address (section 7.1). **Submit** copies `length`
bytes from RAM into the device, replacing the previous list. It is rejected,
and the previous list kept, if the length is 0 or more than 16,384 or any
part of the range is outside RAM. **Clear** empties the list. Any other
command value is rejected. Ports `30h`–`34h` cannot be read (they return
`FFh`).

The list that the device holds at the end of a frame is the one the host
shows for that frame. After reset the list is empty, the address and length
are 0 and the result is `00h`.

### 8.3 Display list

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

### 8.4 Charges

A submit costs 1 T-state per byte of its length, wherever the list ends. A clear, or a rejected command,
costs nothing extra.

### 8.5 Determinism

The device's state is the address, the length, the result and the copy of
the list. It takes no input from the host.

### 8.6 Layers

The host combines the displays in a fixed order, from the bottom:

1. the terminal's cell backgrounds;
2. the vector display;
3. the terminal's characters and cursor.

So text is always drawn over vector shapes, and a cell's background colour
is always behind them. A program that wants a panel of text to hide the
shapes behind it draws a filled rectangle at the end of its list; the text
still appears above it. Smoothing of edges and the exact colour of blended
pixels are up to the host and cannot be observed by the program.

---

## 9. Conformance

An implementation conforms to profile 1 if:

- its CPU passes ZEXALL and uses standard Z80 T-state timings;
- for any image and recorded input sequence, the contents of RAM, the CPU
  registers, the slot registers and all device state after every frame equal
  those of the reference implementation.

## 10. Not yet specified

- Block copy (device 0, ports `08h`–`0Fh`) and its charge rate.
- Font selection, combining marks and complex scripts in the terminal.
- Names (signed pointers to blobs), ranges verified without fetching the
  whole blob, and cancelling a content request.
- Devices 5, 6, 9 and A.
- Starting other machines, and messages between machines.
- Snapshots.
