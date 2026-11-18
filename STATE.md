# Terse machine state hash — v0 (draft)

Status: part of SPEC section 13 (Conformance). It lets two implementations of
machine profile 1 be compared frame by frame: run the same image with the same
recorded inputs on both, and compare the state hash after every frame (or every
Nth). The first frame whose hashes differ is where the implementations diverge.

The **state hash** is the BLAKE3-256 hash (unkeyed, 32 bytes) of the **state
serialization** defined below, written as 64 lowercase hexadecimal digits. The
serialization covers everything that can influence later frames or what is
presented, and nothing else. It is defined only **between frames**: after step 3
of a frame (SPEC 1.4) and before step 1 of the next. A machine that has just
been reset (before frame 0) is also between frames.

All integers are little-endian. `u8`, `u32` are unsigned; `i32` is two's
complement. Flags are one byte, `00h` (false) or `01h` (true). There is no
padding and no alignment. Every field is always present unless its section
says otherwise, so the layout has fixed offsets up to the RAM.

## 1. Layout

### 1.1 Header (45 bytes)

| Offset | Size | Field |
|---|---|---|
| 0 | 13 | The ASCII bytes `terse-state-1` (no terminator) |
| 13 | 32 | Image ID (SPEC 2: BLAKE3-256 of the image file) |

The image ID stands for the ROM, the declared RAM size and the device list,
which never change.

### 1.2 CPU (128 bytes)

| Offset | Size | Field |
|---|---|---|
| 45 | 4 | `pc` (u32). After a fault, the address of the faulting instruction. |
| 49 | 124 | `x1` to `x31`, u32 each, in register order (`x0` is always zero and is not included) |

### 1.3 Frame loop (23 bytes)

| Offset | Size | Field |
|---|---|---|
| 173 | 4 | Frame counter (u32): the value `frame_count` returns in the next frame |
| 177 | 1 | Stopped: `00h` running, `01h` exited, `02h` faulted (SPEC 1.5) |
| 178 | 1 | Exit code, or `00h` if not exited |
| 179 | 1 | Fault kind, or `00h` if not faulted: `01h` illegal instruction, `02h` breakpoint, `03h` fetch, `04h` load, `05h` store, `06h` misaligned |
| 180 | 4 | Illegal instruction: its encoding (u32; a 16-bit encoding in the low half). Otherwise zero. |
| 184 | 4 | Fault `pc` (u32), or zero |
| 188 | 4 | Fault address (u32, SPEC 1.5), or zero |
| 192 | 4 | Budget (i32): the budget carried into the next frame; zero or negative (SPEC 1.4) |

### 1.4 RAM (4 + L bytes)

| Offset | Size | Field |
|---|---|---|
| 196 | 4 | L (u32): RAM size in bytes, `ram_pages × 8192` |
| 200 | L | All of RAM, from address `0010_0000h` |

Offsets below are relative to `S = 200 + L`.

### 1.5 System (37 + 4 × n bytes)

| Offset | Size | Field |
|---|---|---|
| S | 4 | Time latch (u32): the Unix time latched for the frame just run |
| S + 4 | 1 | n: events in the queue, 0–64 |
| S + 5 | 4 × n | The queued events from the head: type, a, b, c |
| S + 5 + 4n | 32 | Controllers 0–3, eight bytes each, exactly as `controller` returned them in the frame just run (all zero for a controller that is not connected) |

Let `T = S + 37 + 4n`.

### 1.6 Terminal (1 or 10,379 bytes)

| Offset | Size | Field |
|---|---|---|
| T | 1 | Present (flag): the image lists device 2. If `00h`, the section ends here. |
| T + 1 | 9,600 | The grid: cells 0 to 2,399, four bytes each (SPEC 8.1) |
| T + 9,601 | 1 | Cursor column |
| T + 9,602 | 1 | Cursor row |
| T + 9,603 | 1 | Foreground index |
| T + 9,604 | 1 | Background index |
| T + 9,605 | 1 | Cursor shown (flag) |
| T + 9,606 | 1 | Pending wrap (flag) |
| T + 9,607 | 768 | Palette entries 0–255: red, green, blue |
| T + 10,375 | 1 | k: bytes of an incomplete UTF-8 sequence given to `write`, 0–3 |
| T + 10,376 | 3 | Those k bytes, in the order written, then `00h` up to 3 bytes |

The incomplete sequence is a valid lead byte (`C2h`–`F4h`) and the valid
continuation bytes received after it, while the character is not yet complete;
it may have been started by an earlier `write` call. It is empty at reset and
after a character completes. A byte that is not a valid continuation ends the
sequence (one `FFFDh`) and is then decoded afresh (SPEC 8.3), so the sequence
becomes that byte if it is a lead byte, and empty otherwise.

Let `V` be the offset after this section.

### 1.7 Vector display (1 or 8 + m bytes)

| Offset | Size | Field |
|---|---|---|
| V | 1 | Present (flag): the image lists device 3. If `00h`, the section ends here. |
| V + 1 | 1 | Style: `00h` clean, `01h` phosphor |
| V + 2 | 1 | Persistence; 160 after reset |
| V + 3 | 1 | Fill mode: `00h` fill, `01h` outlines |
| V + 4 | 4 | m (u32): length of the list the device holds, 0–16,384 |
| V + 8 | m | The list (the bytes of the last accepted submit; empty after reset or clear) |

The afterglow (SPEC 10.6) is presentation and is not included.

Let `C` be the offset after this section.

### 1.8 Content (1 or 425 bytes)

| Offset | Size | Field |
|---|---|---|
| C | 1 | Present (flag): the image lists device 4. If `00h`, the section ends here. |
| C + 1 | 8 × 53 | The request with handle 1, then 2, …, 8 |

Each request record is 53 bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `op` of the pending request (`01h`–`08h`), or `00h` if the handle is free (then every other field is zero) |
| 1 | 4 | Address of its request block (u32) |
| 5 | 32 | Get, subscribe, unsubscribe, publish: the `id` read at submit. Put: the BLAKE3-256 ID of the bytes copied at submit. Otherwise zero. |
| 37 | 4 | `offset` read at submit (get, subscriptions, timeline), else zero |
| 41 | 4 | `length` read at submit (get, subscriptions, timeline, publish), else zero |
| 45 | 4 | `addr` read at submit (get, subscriptions, timeline), else zero |
| 49 | 4 | `size` read at submit (put), else zero |

Let `P` be the offset after this section.

### 1.9 Page (1 or 21 + d bytes)

| Offset | Size | Field |
|---|---|---|
| P | 1 | Present (flag): the image lists device 7. If `00h`, the section ends here (and the serialization). |
| P + 1 | 2 | Rectangle x (u16), as clamped by `region` (SPEC 11.2) |
| P + 3 | 2 | Rectangle y (u16) |
| P + 5 | 2 | Rectangle width (u16) |
| P + 7 | 2 | Rectangle height (u16) |
| P + 9 | 4 | Scroll value (u32), as stored, not clamped |
| P + 13 | 4 | Background (u32), as given to `background`; `FF00_0000h` after reset |
| P + 17 | 4 | d (u32): length of the document, 1–65,536, or 0 for no document |
| P + 21 | d | The document (the bytes of the last accepted submit) |

Nothing about the layout is included: the height a host computed, the lines,
the anchors' positions. They reach the machine only as events (SPEC 11.5),
which are in the queue and RAM like any other input.

That is the end of the serialization. Its length is
`45 + 128 + 23 + 4 + L + 37 + 4n` plus 1 or 10,379 (terminal), 1 or 8 + m
(vector), 1 or 425 (content) and 1 or 21 + d (page).

## 2. What is left out, and why

- **ROM, title and device list**: fixed for the image, covered by the image ID.
- **This frame's random bytes**: an input consumed within the frame (SPEC 4);
  a new set is latched at step 1 of every frame, so nothing of them outlives
  the frame.
- **Requests not yet handed to the host**, the bytes a put copied and the
  title a publish copied: the pending request records hold everything about
  them that can affect the machine (a put's bytes only matter through their
  ID, which the completion writes; a publish's title only through the entry
  the host publishes, which reaches the machine as later inputs). Whether a
  request waits for the user's consent (SPEC 9.5) is host state; to the
  machine it is pending.
- **Host bookkeeping**: counters that tell a host the display list changed,
  per-frame statistics (units used, events dropped), the decoded form of ROM,
  caches.
- **Presentation**: fonts, pixels, blending, the afterglow (SPEC 8.6, 10.5,
  10.6). The vector list bytes are included; how they are drawn is not.
- **The page's layout** (SPEC 11.4): it depends on the host's fonts. The
  document bytes (with its figures' path commands), rectangle, scroll value
  and background are included; how figures are drawn is not; the height a
  host reports is an input event.

## 3. Test vectors

From this repository's implementation (the reference):

- `programs/hello/hello.terse` at reset (no frame run): serialization
  18,811 bytes, hash
  `0ce433a89182bc2f737e02bff89d21d4e8e7c462a40c262cdc8b15240d1321be`.
- The same after 1 frame with no input:
  `d91a4c7f56a8183d0c0a2a5520e219b1628a3661751460a0eea996fac8761ebe`;
  after 30 frames:
  `066e293e97fd6d5908db87cc110ebc46846d73b310a7047e1f888c7b010e2c83`.
- `programs/conform/expected-hashes.txt`: the conformance image with its
  scripted input, every 10 frames to frame 200 (README, "Conformance image").

"No input" means: no events, Unix time 0, no random bytes (so `random` gives
zeros), all controllers disconnected, no completions — what `terse run` uses.

`terse run <image> --hash` prints the hash after the last frame, and
`--hash-every N` prints `<frame counter> <hash>` after every frame whose
counter is a multiple of N.
