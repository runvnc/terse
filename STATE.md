# Terse machine state hash — v0 (draft)

Status: proposed for SPEC section 9 (Conformance). It lets two implementations
of profile 1 be compared frame by frame: run the same image with the same
recorded inputs on both, and compare the state hash after every frame (or every
Nth). The first frame whose hashes differ is where the implementations diverge.

The **state hash** is the BLAKE3-256 hash (unkeyed, 32 bytes) of the **state
serialization** defined below, written as 64 lowercase hexadecimal digits. The
serialization covers everything that can influence later frames or what is
presented, and nothing else. It is defined only **between frames**: after step 4
of a frame (SPEC 1.3) and before step 1 of the next. A machine that has just
been reset (before frame 0) is also between frames.

All integers are little-endian. `u8`, `u16`, `u32` are unsigned; `i32` is
two's complement. Flags are one byte, `00h` (false) or `01h` (true). There is
no padding and no alignment. Every field is always present unless its
section says otherwise, so the layout has fixed offsets up to the RAM.

## 1. Layout

### 1.1 Header (45 bytes)

| Offset | Size | Field |
|---|---|---|
| 0 | 13 | The ASCII bytes `terse-state-0` (no terminator) |
| 13 | 32 | Image ID (SPEC 2: BLAKE3-256 of the image file) |

The image ID stands for the ROM, the declared RAM size and the device list,
which never change.

### 1.2 CPU (35 bytes)

| Offset | Size | Field |
|---|---|---|
| 45 | 2 | AF (u16: A × 100h + F, so F is the first byte) |
| 47 | 2 | BC (B × 100h + C) |
| 49 | 2 | DE |
| 51 | 2 | HL |
| 53 | 2 | AF′ |
| 55 | 2 | BC′ |
| 57 | 2 | DE′ |
| 59 | 2 | HL′ |
| 61 | 2 | IX |
| 63 | 2 | IY |
| 65 | 2 | SP |
| 67 | 2 | PC. While halted, the address of the HALT instruction (SPEC 1.3). |
| 69 | 2 | MEMPTR (also called WZ): the Z80's internal address register, as on a Zilog Z80. It is visible through flags 3 and 5 of `BIT n,(HL)`. |
| 71 | 1 | I |
| 72 | 1 | R, all eight bits (bit 7 is only changed by `LD R,A`) |
| 73 | 1 | IFF1 (flag) |
| 74 | 1 | IFF2 (flag) |
| 75 | 1 | Interrupt mode: 0, 1 or 2 |
| 76 | 1 | Halted (flag): the CPU has executed HALT and not yet accepted an interrupt |
| 77 | 1 | Interrupt shadow (flag), see below |
| 78 | 1 | Pending prefix: `00h`, or `DDh`, `EDh` or `FDh`, see below |
| 79 | 1 | Q, see below |

**Interrupt shadow** is `01h` when IFF1 is set and the CPU will not accept a
maskable interrupt before executing its next instruction: right after `EI`,
and while a prefix is pending. It is `00h` whenever IFF1 is clear, so an
implementation that also blocks interrupts for one instruction after `DI` (it
makes no difference there) writes `00h`.

**Pending prefix.** The reference CPU core executes a run of `DDh`/`FDh`
prefixes in steps: a `DDh` or `FDh` followed by another prefix byte (`DDh`,
`EDh` or `FDh`) is one step of 8 T-states that fetches both bytes, ignores the
first and leaves the second pending; the next step executes the instruction
that the pending prefix starts, without fetching the prefix again. A frame can
end between those steps. This field is that pending prefix byte, `00h` between
whole instructions. (SPEC 1.3 does not yet say whether such a run is one
instruction or several; see the conformance notes in README.)

**Q** is the Z80's flag-write latch, which decides flags 3 and 5 of `SCF` and
`CCF` on a Zilog Z80: the value of F that the most recently executed
instruction wrote, or `00h` if that instruction did not write F. Accepting an
interrupt sets it to `00h`. A prefix-only step (above) leaves it unchanged.

### 1.3 Frame loop (11 bytes)

| Offset | Size | Field |
|---|---|---|
| 80 | 4 | Frame counter (u32, the value port 14h returns in the next frame) |
| 84 | 1 | Exited (flag): the program has written port 1Fh |
| 85 | 1 | Exit code, or `00h` if not exited |
| 86 | 4 | Budget (i32): the T-state budget carried into the next frame; zero or negative (SPEC 1.3) |
| 90 | 1 | Interrupt request (flag): the frame interrupt request latch |

### 1.4 Memory (12 + L bytes)

| Offset | Size | Field |
|---|---|---|
| 91 | 8 | Slot registers 0–7 (the values ports 00h–07h return) |
| 99 | 4 | L (u32): RAM size in bytes, `ram_pages × 8192` |
| 103 | L | All declared RAM, from linear address 0 (SPEC 7.1) |

Offsets below are relative to `S = 103 + L`.

### 1.5 System (39 + 4 × n bytes)

| Offset | Size | Field |
|---|---|---|
| S | 4 | Time latch (u32): the Unix time latched for the frame just run |
| S + 4 | 1 | n: events in the queue, 0–64 |
| S + 5 | 4 × n | The queued events from the head: type, a, b, c |
| S + 5 + 4n | 32 | Controllers 0–3, eight bytes each, exactly as port 1Eh returns them for the frame just run (all zero for a controller that is not connected) |
| S + 37 + 4n | 1 | Selected controller: 0–3, or `FFh` for none |
| S + 38 + 4n | 1 | Read position: the byte (0–7) the next read of port 1Eh returns; `00h` when no controller is selected (it cannot be observed then, since selecting a controller restarts it) |

Let `T = S + 39 + 4n`.

### 1.6 Terminal (1 or 10,383 bytes)

| Offset | Size | Field |
|---|---|---|
| T | 1 | Present (flag): the image lists device 2. If `00h`, the section ends here. |
| T + 1 | 9,600 | Cell memory (SPEC 6.1) |
| T + 9,601 | 1 | Cursor column |
| T + 9,602 | 1 | Cursor row |
| T + 9,603 | 1 | Foreground index |
| T + 9,604 | 1 | Background index |
| T + 9,605 | 1 | Cursor shown (flag) |
| T + 9,606 | 1 | Pending wrap (flag) |
| T + 9,607 | 2 | Cell pointer (u16) as written, not reduced |
| T + 9,609 | 768 | Palette entries 0–255: red, green, blue |
| T + 10,377 | 1 | Palette index |
| T + 10,378 | 1 | Palette component: which of red (0), green (1), blue (2) the next write to port 2Ah sets |
| T + 10,379 | 1 | k: bytes of an incomplete UTF-8 sequence written to port 20h, 0–3 |
| T + 10,380 | 3 | Those k bytes, in the order written, then `00h` up to 3 bytes |

The incomplete sequence is a valid lead byte (`C2h`–`F4h`) and the valid
continuation bytes received after it, while the character is not yet complete.
It is empty at reset and after a character completes. A byte that is not a
valid continuation ends the sequence (one `FFFDh`) and is then decoded afresh
(SPEC 6.3), so the sequence becomes that byte if it is a lead byte, and empty
otherwise.

Let `V` be the offset after this section.

### 1.7 Vector display (1 or 14 + m bytes)

| Offset | Size | Field |
|---|---|---|
| V | 1 | Present (flag): the image lists device 3. If `00h`, the section ends here. |
| V + 1 | 4 | List address register (u32, 24-bit value; ports 30h–32h) |
| V + 5 | 2 | List length register (u16; ports 33h–34h) |
| V + 7 | 1 | Result of the last command (port 35h) |
| V + 8 | 1 | Style (port 36h): `00h` clean, `01h` phosphor |
| V + 9 | 1 | Persistence (port 37h); 160 after reset |
| V + 10 | 4 | m (u32): length of the list the device holds, 0–16,384 |
| V + 14 | m | The list (the bytes of the last accepted submit; empty after reset or clear) |

The phosphor trail (SPEC 8.7) is presentation and is not included.

Let `C` be the offset after this section.

### 1.8 Content (1 or 412 bytes)

| Offset | Size | Field |
|---|---|---|
| C | 1 | Present (flag): the image lists device 4. If `00h`, the section ends here (and the serialization). |
| C + 1 | 2 | Block address register (u16; ports 40h–41h) |
| C + 3 | 1 | Result of the last submit (port 42h) |
| C + 4 | 8 × 51 | The request with handle 1, then 2, …, 8 |

Each request record is 51 bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `op` of the pending request (`01h`–`04h`), or `00h` if the handle is free (then every other field is zero) |
| 1 | 4 | Linear RAM address of its request block (u32) |
| 5 | 32 | Get: the `id` read at submit. Put: the BLAKE3-256 ID of the bytes copied at submit. Otherwise zero. |
| 37 | 4 | `offset` read at submit (u32; get, subscriptions, timeline), else zero |
| 41 | 2 | `length` read at submit (u16; get, subscriptions, timeline), else zero |
| 43 | 4 | `addr` read at submit (u32; get, subscriptions, timeline), else zero |
| 47 | 4 | `size` read at submit (u32; put), else zero |

That is the end of the serialization. Its length is
`45 + 35 + 11 + 12 + L + 39 + 4n` plus 1 or 10,383 (terminal), 1 or 14 + m
(vector) and 1 or 412 (content).

## 2. What is left out, and why

- **ROM, title and device list**: fixed for the image, covered by the image ID.
- **This frame's random bytes** (port 1Ch): an input consumed within the frame
  (SPEC 5.2); a new set is latched at step 1 of every frame, so nothing of them
  outlives the frame.
- **Requests not yet handed to the host**, and the bytes a put copied: the
  pending request records hold everything about them that can affect the
  machine (a put's bytes only matter through their ID, which the completion
  writes).
- **Host bookkeeping**: counters that tell a host the display list changed,
  per-frame statistics (T-states used, events dropped), caches.
- **Presentation**: fonts, pixels, blending (SPEC 6.7, 8.6). The vector list
  bytes are included; how they are drawn is not.

## 3. Test vectors

From this repository's implementation (the reference):

- `programs/hello/hello.terse` at reset (no frame run): serialization 43,295
  bytes, hash
  `013759c33d61daa0768a77f6bcb428a88e2273a7e412d35d09496944b3338b53`.
- The same after 1 frame with no input:
  `957d5f4d58c62d97c970d185efc4b1b64567d19a67c86f0495b7f938ac1062be`;
  after 30 frames:
  `7d79f4c1e255bf16c58ecba580f2e4d38ad27f324fde57883b9689ca5a4b538c`.
- `programs/conform/expected-hashes.txt`: the conformance image with its
  scripted input, every 10 frames to frame 200 (README, "Conformance image").

"No input" means: no events, Unix time 0, random bytes `00h` (none supplied),
all controllers disconnected, no completions — what `terse run` uses.

`terse run <image> --hash` prints the hash after the last frame, and
`--hash-every N` prints `<frame counter> <hash>` after every frame whose
counter is a multiple of N.
