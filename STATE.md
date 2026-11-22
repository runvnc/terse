# Terse machine state hash — v0 (draft)

Terse has three layers: (a) the network (`NETWORK.md`) and (b) articles
(`ARTICLE.md`), which together are the core, so that a native reader with
no virtual machine that implements them is a complete, conforming Terse
client; and (c) the machine (`SPEC.md`, the device documents in `spec/`, and this document), an optional
layer that reuses them. This document belongs to layer (c).

Status: part of SPEC §conformance (Conformance). It lets two implementations of
machine profile 1 be compared frame by frame: run the same image with the same
recorded inputs on both, and compare the state hash after every frame (or every
Nth). The first frame whose hashes differ is where the implementations diverge.
The serialization is also a machine's snapshot (SPEC §machines.snapshots): section 4 says how
a machine is restored from it.

The **state hash** is the BLAKE3-256 hash (unkeyed, 32 bytes) of the **state
serialization** defined below, written as 64 lowercase hexadecimal digits. The
serialization covers everything that can influence later frames or what is
presented, and nothing else. It is defined only **between frames**: after step 3
of a frame (SPEC §frames) and before step 1 of the next. A machine that has just
been reset (before frame 0) is also between frames.

All integers are little-endian. `u8`, `u32` are unsigned; `i32` is two's
complement. Flags are one byte, `00h` (false) or `01h` (true). There is no
padding and no alignment. Every field is always present unless its section
says otherwise, so the layout has fixed offsets up to the RAM.

## 1. Layout

Sections 1.1–1.5 and 1.14 are the core machine's (SPEC.md); 1.6–1.12
belong to devices, and 1.13 to experimental devices, in a fixed order, each with its own rule for when it
is there. A host whose devices are separate drivers (`DEVICES.md`) asks
each driver for its section and must still know that order and those
rules: `DEVICES.md` 1.7 gives them as one table.

### 1.1 Header (45 bytes)

| Offset | Size | Field |
|---|---|---|
| 0 | 13 | The ASCII bytes `terse-state-1` (no terminator) |
| 13 | 32 | Image ID (SPEC §image: BLAKE3-256 of the image file) |

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
| 177 | 1 | Stopped: `00h` running, `01h` exited, `02h` faulted, `03h` halted: stopped by the host because a device did not keep up (SPEC §stopping, `PERFORMANCE.md` R15) |
| 178 | 1 | Exit code if exited; the device's slot if halted; else `00h` |
| 179 | 1 | Fault kind, or `00h` if not faulted: `01h` illegal instruction, `02h` breakpoint, `03h` fetch, `04h` load, `05h` store, `06h` misaligned |
| 180 | 4 | Illegal instruction: its encoding (u32; a 16-bit encoding in the low half). Otherwise zero. |
| 184 | 4 | Fault `pc` (u32), or zero |
| 188 | 4 | Fault address (u32, SPEC §stopping), or zero |
| 192 | 4 | Budget (i32): the budget carried into the next frame; zero or negative (SPEC §frames) |

After an `exit` the CPU section holds `pc` after the `ecall` and `a0` =
0 (SPEC §system `exit`: the call's result), and the budget is settled as
after any frame: 0, or the negative remainder (SPEC §frames). After a
fault `pc` is the faulting instruction's and no register has changed.

### 1.4 RAM and grants (4 + L + k bytes)

| Offset | Size | Field |
|---|---|---|
| 196 | 4 | L (u32): RAM size in bytes, `ram_pages × 8192` |
| 200 | L | All of RAM, from address `0010_0000h` |
| 200 + L | k | The grants: one flag for each device the image lists as optional (SPEC §image), in header order: `01h` granted, `00h` not |

A capability, such as device 10 (full text, SPEC §terminal.text), has a
grant flag like any optional device and no section of its own below.

k is the number of optional devices in the image's header, 0 to 16. An
image with none has no grant bytes, so its state is laid out exactly as
before optional devices existed. Below, the machine **has** a device if
its image requires it, or lists it as optional and it was granted; a
device it does not have is absent, as if the image did not list it.

Offsets below are relative to `S = 200 + L + k`.

### 1.5 System (37 + 4 × n bytes)

| Offset | Size | Field |
|---|---|---|
| S | 4 | Time latch (u32): the Unix time latched for the frame just run |
| S + 4 | 1 | n: events in the queue, 0–64 |
| S + 5 | 4 × n | The queued events from the head: type, a, b, c |
| S + 5 + 4n | 32 | Controllers 0–3, eight bytes each, exactly as `controller` returned them in the frame just run (all zero for a controller that is not connected) |

At reset the time latch is 0, the queue is empty and the controllers are
all zero; no program can see these values (every call runs after a
frame's latch), but they are in the state of a machine just reset.

Let `T = S + 37 + 4n`.

### 1.6 Terminal (1 or 10,379 bytes)

| Offset | Size | Field |
|---|---|---|
| T | 1 | Present (flag): the machine has device 2. If `00h`, the section ends here. |
| T + 1 | 9,600 | The grid: cells 0 to 2,399, four bytes each (SPEC §terminal.cells) |
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
sequence (one `FFFDh`) and is then decoded afresh (SPEC §terminal.put), so the sequence
becomes that byte if it is a lead byte, and empty otherwise.

Let `V` be the offset after this section.

### 1.7 Vector display (1 or 8 + m bytes)

| Offset | Size | Field |
|---|---|---|
| V | 1 | Present (flag): the machine has device 3 or device 9. If `00h`, the section ends here. |
| V + 1 | 1 | Style: `00h` clean, `01h` phosphor |
| V + 2 | 1 | Persistence; 160 after reset |
| V + 3 | 1 | Fill mode: `00h` fill, `01h` outlines |
| V + 4 | 4 | m (u32): length of the list the device holds, 0–16,384 |
| V + 8 | m | The list (the bytes of the last accepted submit; empty after reset or clear) |

The style and the persistence are also those device 9 sets (SPEC §vector3d.calls):
they are held here once. For a machine that has device 9 and not device
3 the fill mode is `00h` and m is 0 (nothing can set them).

The afterglow (SPEC §vector.phosphor) is presentation and is not included.

Let `C` be the offset after this section.

### 1.8 Content (1 or 425 bytes)

| Offset | Size | Field |
|---|---|---|
| C | 1 | Present (flag): the machine has device 4. If `00h`, the section ends here. |
| C + 1 | 8 × 53 | The request with handle 1, then 2, …, 8 |

Each request record is 53 bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `op` of the pending request (`01h`–`11h`), or `00h` if the handle is free (then every other field is zero) |
| 1 | 4 | Address of its request block (u32) |
| 5 | 32 | Get, subscribe, unsubscribe, publish, set nickname: the `id` read at submit; save, load, delete: the record's name as read at submit (`id`, zero padded). Put: the BLAKE3-256 ID of the bytes copied at submit. Otherwise zero. |
| 37 | 4 | `offset` read at submit (get, load, subscriptions, timeline, names, programs, records, find; publish: the kind, 0, 1 or 4); set profile: the listed bit, 0 or 1; else zero |
| 41 | 4 | `length` read at submit (get, load, subscriptions, timeline, names, programs, records, find, publish, set profile, set nickname, subscribe: the hints' length, identity: the hint buffer's, 0 for none), else zero |
| 45 | 4 | `addr` read at submit (get, load, subscriptions, timeline, names, programs, records, find; identity: the hint buffer's, zero if `length` is 0), else zero |
| 49 | 4 | `size` read at submit (put, save; find: the query's length), else zero |

Let `P` be the offset after this section.

### 1.9 Page (1 or 21 + d bytes)

| Offset | Size | Field |
|---|---|---|
| P | 1 | Present (flag): the machine has device 7. If `00h`, the section ends here. |
| P + 1 | 2 | Rectangle x (u16), as clamped by `region` (SPEC §page.calls) |
| P + 3 | 2 | Rectangle y (u16) |
| P + 5 | 2 | Rectangle width (u16) |
| P + 7 | 2 | Rectangle height (u16) |
| P + 9 | 4 | Scroll value (u32), as stored, not clamped |
| P + 13 | 4 | Background (u32), as given to `background`; `FF00_0000h` after reset |
| P + 17 | 4 | d (u32): length of the document, 1–65,536, or 0 for no document |
| P + 21 | d | The document (the bytes of the last accepted submit) |

Nothing about the layout is included: the height a host computed, the lines,
the anchors' positions. They reach the machine only as events (SPEC §page.events),
which are in the queue and RAM like any other input.

Let `M` be the offset after this section.

### 1.10 Machines (0, 1 or 1,126 + 32b + 83r bytes)

This section is there only if the image lists device 8; for any other image
it is empty (0 bytes), so the hashes of images without device 8 are as they
were before device 8 existed.

| Offset | Size | Field |
|---|---|---|
| M | 1 | Present (flag): the machine has device 8 (it is the shell of a system, SPEC §machines.shell). If `00h`, the section ends here. |
| M + 1 | 1 | Focus: a handle 1–8, or 0 for the shell |
| M + 2 | 8 × 138 | Handle 1, then 2, …, 8 |
| M + 1,106 | 1 | b: the number of built-in programs, 0–255 |
| M + 1,107 | 32 × b | Their image IDs, in `builtin` order |
| M + 1,107 + 32b | 1 | r: the number of open requests the shell has been told of and has not read (SPEC §machines.calls `request`), 0–8 |
| M + 1,108 + 32b | 83 × r | Each: the handle (1), then the 82 bytes `request` would write; in increasing handle order |
| M + 1,108 + 32b + 83r | 18 | The view (SPEC §machines.calls `face`), below |

Each handle's record is 138 bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | State (SPEC §machines.shell) |
| 1 | 1 | Run flag |
| 2 | 2 | Rectangle x (u16), as clamped by `place` |
| 4 | 2 | Rectangle y (u16) |
| 6 | 2 | Rectangle width (u16) |
| 8 | 2 | Rectangle height (u16) |
| 10 | 47 | Bytes 1 to 47 of the `info` record (SPEC §machines.calls): exit code, fault kind, failure reason, units, frame counter, fault `pc`, image ID, as latched in the frame just run and changed by the shell's calls since |
| 57 | 81 | The identity given to `start_program`, as `identity` writes it (SPEC §machines.calls), or all zero if the handle has none |

The view is 18 bytes, all zero for none:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | Handle of the machine it moves, 1–8, or 0 for none |
| 1 | 1 | Direction: `01h` towards the glass (`face`), `00h` back (`unface`) |
| 2 | 8 | The rectangle given to `face`, as clamped: x, y, width, height (u16 each) |
| 10 | 2 | Start progress (u16), 0–4,096 |
| 12 | 4 | Start frame (u32): the shell's frame counter in the frame of the call |
| 16 | 2 | Duration in frames (u16), 0–600 |

A handle not in use is all zero. The machines a shell starts are not part of
its state: each has a state of its own, serialized as for any machine (it
never has device 8, so its section 1.10 is `00h` if its image lists device 8,
and absent otherwise).

### 1.11 3D vector display (0 or 4 + m bytes)

This section is there only if the machine has device 9; for any other
image it is empty, so the hashes of images without device 9 are as they
were before device 9 existed.

| Offset | Size | Field |
|---|---|---|
| D | 4 | m (u32): length of the list the device holds, 0–16,384 |
| D + 4 | m | The list (the bytes of the last accepted submit; empty after reset or clear) |

`D` is the offset after section 1.10. The style and persistence are in
section 1.7; the eye a host draws from (SPEC §vector3d.space) is presentation and is
not included.

### 1.12 Sound (0 or 2,688 + s bytes)

This section is there only if the machine has device 6 (SPEC §sound); for any
other image it is empty, so the hashes of images without device 6 are as
they were before device 6 existed. `U` is the offset after section 1.11.

| Offset | Size | Field |
|---|---|---|
| U | 16 × 38 | Voice 0, then 1, …, 15 |
| U + 608 | 32 × 64 | Wavetables 0 to 31, 64 signed bytes each |
| U + 2,656 | … | Sample slots 0 to 3, each: its length len (u32, 0–65,536), its loop start (u32, as given to `sample`), then its len bytes |

s is the sum of the four lengths. Each voice is 38 bytes (SPEC §sound.voices):

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | Source: `00h` wavetable, `01h` noise, `02h` sample |
| 1 | 1 | Wavetable, 0–31 (kept while the source is another) |
| 2 | 1 | Sample slot, 0–3 (likewise) |
| 3 | 4 | Phase (u32) |
| 7 | 4 | Increment (u32) |
| 11 | 1 | Volume |
| 12 | 1 | Pan left |
| 13 | 1 | Pan right |
| 14 | 2 | Attack (u16) |
| 16 | 2 | Decay (u16) |
| 18 | 1 | Sustain |
| 19 | 2 | Release (u16) |
| 21 | 1 | Stage: `00h` off, `01h` attack, `02h` decay, `03h` sustain, `04h` release |
| 22 | 4 | Level (u32), 0 to `FF_FFFFh` |
| 26 | 1 | Modulator: a voice 0–15, or `FFh` none |
| 27 | 1 | Depth |
| 28 | 1 | Feedback |
| 29 | 1 | Ring: a voice 0–15, or `FFh` none |
| 30 | 1 | Mix (flag) |
| 31 | 2 | Noise register (u16), 1 to `7FFFh` |
| 33 | 1 | Ended (flag) |
| 34 | 2 | Output (i16): the voice's output in the last pair rendered |
| 36 | 2 | Previous output (i16): its output in the pair before that |

The samples a frame rendered are presentation (SPEC §sound.presentation) and are not
included; everything they are computed from is.

### 1.13 Experimental devices (0, or 4 + e bytes for each)

This section is there only if the machine has an experimental device
(slots 240–255, `spec/experimental.md`): its image lists one as
optional and it was granted. For any other machine, including one whose
image lists an experimental device that was not granted, it is empty, so
those hashes are as they were before experimental devices existed. `X`
is the offset after section 1.12.

For each experimental device the machine has, in the order of the
image's header:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | e (u32): the section's length, 0 to 16,384 |
| 4 | e | The device's section, as its driver gives it (`DEVICES.md` 15.3) |

Only the driver knows what the bytes mean; this document fixes only
where they go and their length. §conformance does not cover them
(`spec/experimental.md` §experimental.conformance).

### 1.14 Argument (0 or 32 bytes)

| Offset | Size | Field |
|---|---|---|
| A | 32 | The machine's argument (SPEC §system `argument`); there only if the shell gave it one |

`A` is the offset after section 1.13. A machine with no argument (every
machine run on its own, and every shell) has nothing here, so its hash is
as it was before arguments existed.

The section has no flag (a flag would change the hash of every machine
without an argument). Its presence is inferred: a reader that has read
sections 1.1 to 1.13 finds the argument there exactly when 32 bytes
remain, none when 0 remain, and refuses any other remainder. The writer
is the machine's own serialization: it writes the section exactly when
the machine was given an argument (by its shell's `argument` before its
first frame, SPEC §machines.calls, or by the snapshot it was restored from).

That is the end of the serialization. Its length is
`45 + 128 + 23 + 4 + L + k + 37 + 4n` plus 1 or 10,379 (terminal), 1 or 8 + m
(vector), 1 or 425 (content), 1 or 21 + d (page), 0, 1 or 1,126 + 32b +
83r (machines), 0 or 4 + m (3D vector), 0 or 2,688 + s (sound), 4 + e
for each experimental device (none for most machines) and 0 or 32
(argument).

## 2. What is left out, and why

- **ROM, title and device list**: fixed for the image, covered by the image ID.
  Which optional devices were granted is not (the host chooses at reset),
  so the grants are in 1.4.
- **This frame's random bytes**: an input consumed within the frame (SPEC §system);
  a new set is latched at step 1 of every frame, so nothing of them outlives
  the frame.
- **Requests not yet handed to the host**, the bytes a put or a save
  copied, the title a publish copied, the query a find copied and the name
  a set profile or set nickname copied: the pending request records hold
  everything about them that can affect the machine (a put's bytes only
  matter through their ID, which the completion writes; a save's data, a
  title, query or name only through what the host does with it, which
  reaches the machine as later inputs).
- **Named records** (SPEC §content.records) and a machine's identity: the records are
  the host's; what a load delivers is an input. A started machine's
  identity is in the shell's handle record (1.10), not in its own state. Whether a
  request waits for the user's consent (SPEC §content.feeds) is host state; to the
  machine it is pending.
- **Whether a machine's open request waits** (SPEC §system `open`): it stops
  waiting when the shell reads it, which the machine cannot see; what
  `open` returns is an input to the machine (SPEC §machines.determinism). The shell's side,
  the requests it has been told of and not read, is in 1.10.
- **Host bookkeeping**: counters that tell a host the display list changed,
  per-frame statistics (units used, events dropped), the decoded form of ROM,
  caches.
- **Presentation**: fonts, pixels, blending, the afterglow (SPEC §terminal.presentation, §vector.layers,
  §vector.phosphor), the eye the 3D display is seen from (SPEC §vector3d.space), the samples the
  sound device rendered and how a host plays or mixes them (SPEC §sound.presentation). The vector list
  bytes are included; how they are drawn is not.
- **A system's host bookkeeping** (SPEC §machines): which machines exist, their
  images while they load, the order of their frames, what the host shows.
  The shell sees all of it that it can observe through device 8's records
  and events, which are in its state and queue.
- **The page's layout** (SPEC §page.layout): it depends on the host's fonts. The
  document bytes (with its figures' path commands), rectangle, scroll value
  and background are included; how figures are drawn is not; the height a
  host reports is an input event. The locates a program made (SPEC §page.calls)
  are not included either: a host takes them after the frame, and the
  answers are input events.
- **The shell's snapshot slots** (SPEC §machines.snapshots): host data; what
  `snapshot_info` tells and the snapshot done events are the shell's
  inputs.

## 3. Test vectors

From this repository's implementation (the reference):

- `programs/hello/hello.terse` (which lists device 10, full text, as
  optional: one grant byte, granted) at reset (no frame run): serialization
  18,812 bytes, hash
  `f82b7079daea85863f079eaba67e0ad2816b905e9a1a494e7dab587889a4c058`.
- The same after 1 frame with no input:
  `2c8994cf8408449f927b16377cf44750419682c7fcc744d84748459c7ec8f9cf`;
  after 30 frames:
  `819432152b38626f8fc54b43f50d5d4c501dc99e6904315b61dc78b796c611e2`.
- `exit`: the 42-byte image
  `5445525300010a000001000000000000657869740000000000000000000000001d459308601073000000`
  (no devices, one RAM page, title `exit`; ROM `c.li a0, 7`,
  `addi a7, x0, 106h`, `ecall`). At reset: serialization 8,433 bytes,
  hash `9cbfa0da982561dd1c5dbaf1d5fde2121dee78c097d754aa0c2a350584bbeb5a`.
  After 1 frame, in which it exits with code 7 (`pc` = `0001_000Ah`, `a0`
  = 0, `a7` = `106h`, stopped `01h`, exit code `07h`, budget 0):
  `3272a879392d8179997dc398726430bf92bbd413b89653be8c9af4472656fe9b`.
- `programs/conform/expected-hashes.txt`: the conformance image with its
  scripted input, every 10 frames to frame 200 (README, "Conformance image").
  Likewise `programs/conform-program` and `programs/conform-reading`.

"No input" means: no events, Unix time 0, no random bytes (so `random` gives
zeros), all controllers disconnected, no completions — what `terse run` uses.

`terse run <image> --hash` prints the hash after the last frame, and
`--hash-every N` prints `<frame counter> <hash>` after every frame that
leaves the frame counter at a multiple of N: from reset, after the first
N frames, 2N frames, …; a run started from a snapshot goes by the counter,
not by the frames it has run. The frame in which the machine exits or
faults ends the run; it is printed only if its counter is a multiple of
N (with `--hash` the last hash is printed in any case). Section 6 gives
the rest of a headless run's command line, which the other
implementations in this repository share. For a system (`terse run --shell`) these are
the shell's hashes; `--hash-all`, after the last frame only, prints the
shell's hash and then `<handle> <hash>` for each machine the shell has at
that moment, in handle order: one it has stopped is not listed, and `-`
stands for one that has no machine, because its image or snapshot is
still being fetched (a machine started in frame N is loaded at the start
of frame N + 1) or its start failed.

## 4. Restoring, and snapshot files

A machine is **restored** from a serialization S and an image whose ID is
the one in S's header. The restored machine has the devices S's grants
(1.4) say, whatever the restoring host could grant otherwise: an
optional device the host has and S says was not granted stays absent,
and if S says a device was granted that the host cannot provide, the
restore fails (SPEC §machines.calls `restore`, reason 3) and nothing runs. S is
read section by section as section 1 lays it out, with the sections the
image's device list and the grants imply; it is refused
(SPEC §machines.calls `restore`, reason 5) if it is cut short or longer, if its image
ID is another, if section 1.10 is present (a shell), or if serializing the
restored machine would not give S again, byte for byte (so every field is
in range: a flag is `00h` or `01h`, the stopped and fault fields agree, the
budget is zero or negative, L is the image's RAM size, every grant is `00h`
or `01h`, n ≤ 64, the cursor
is on the grid, the incomplete sequence is one, the style, fill mode and
list lengths are valid, an image without device 3 has fill mode `00h` and
an empty 2D list, every sound voice's source, wavetable, sample slot,
stage, level, modulator, ring and noise register are in range and no
sample is longer than 65,536 bytes, the page rectangle is clamped, a document is at most
65,536 bytes, every pending request has a known `op`, its block in RAM and
its fields as a submit leaves them, every experimental section at most
16,384 bytes and taken by its driver). The restored machine's state hash is
S's hash, and it is between frames: its next frame is the one whose number
is S's frame counter.

Everything else a machine needs is in S. Host bookkeeping starts afresh:
the content requests pending in S fail in the next frame (SPEC §content.submit), the
page is laid out and its height reported as for a new document, and the
afterglow is empty.

A host keeps a snapshot (SPEC §machines.snapshots) in a file:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic: `54h 53h 4Eh 50h` (`TSNP`) |
| 4 | 1 | Version: `0`; `1` for a machine that has an experimental device (section 1.13), which is otherwise the same |
| 5 | 81 | The identity, as SPEC §machines.calls `identity` writes it; all zero for none |
| 86 | 4 | The Unix time latched for the machine's last frame (u32) |
| 90 | 4 | N (u32): the length of S |
| 94 | 200 | Bytes 0–199 of S: header, CPU, frame loop and L |
| 294 | 16 | Page map: bit k (byte k / 8, bit k mod 8) is set if RAM page k (8,192 bytes at S offset 200 + 8,192 k) is stored below; bits past the last page are clear |
| 310 | 8,192 × p | The p pages whose bit is set, in increasing order. A page whose bit is clear is all zero. |
| 310 + 8,192p | N − 200 − L | The rest of S, from offset 200 + L |
| … | 32 | The state hash of S |

A writer sets a page's bit exactly when the page is not all zero; a reader
accepts any page stored. Before it reads S, a reader refuses a file whose
N is below 200 or above 1,685,299 (the longest serialization: 1 MiB of
RAM, 16 grants, a full queue, every device at its largest, 16
experimental sections of 16,384 bytes and an argument; 1,423,091 without
the experimental sections), whose L
is 0, not a multiple of 8,192, above 1,048,576 or more than N − 200, or
whose page map has a bit set for a page past the last (bit k for k ≥
L / 8,192): a writer leaves those bits clear. A file that does not end right after the hash, or
whose hash is not S's, is refused like a malformed S.

`terse run <image> --save-state <file>` writes such a file after the last
frame, and `--load-state <file>` starts from one: with `--hash` the hash
after `--frames 0` is the hash printed when the file was saved, and the
hashes of a continued run equal those of the uninterrupted run with the
same inputs.

## 5. Recordings

A **recording** holds every input of every frame of one machine's run
(SPEC §system, §content.determinism, §page.events), so that the run can be replayed exactly without the
host that made it: no network, no blob store, no data directory. It also
holds the state hash after each frame, so a replay can tell the first frame
at which it differs. `terse run <image> --record <file>` writes one for a
single machine (not `--shell`), and `terse run <image> --replay <file>`
replays it from the image file and the recording alone, checking every
frame's hash; it refuses a recording of another image.

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | Magic: `54h 52h 45h 43h` (`TREC`) |
| 4 | 1 | Version: `0` |
| 5 | 32 | The image ID |
| 37 | 4 | M (u32): the length of the start snapshot; 0 if the run starts from reset |
| 41 | M | The snapshot file (section 4) the machine is restored from, whose image ID is the one at offset 5 |
| 41 + M | k | The machine's grants (1.4), one flag per optional device the image lists; none for an image without optional devices. With a snapshot they are the snapshot's |
| 41 + M + k | … | Frame records, one per frame run, in order, to the end of the file |

A frame record:

| Size | Field |
|---|---|
| 4 | The frame counter as the frame starts (u32): reset or the snapshot's counter for the first record, one more for each next |
| 4 | The Unix time latched for the frame (u32) |
| 32 | Controllers 0–3, eight bytes each, as `controller` writes them (SPEC §controllers): all zero for one not connected; else byte 0 is `01h`, bytes 3–6 are never `80h` and byte 7 is `00h` |
| 4 | e (u32): the number of events the host appended to the queue in step 1 of SPEC §frames |
| 4e | The events, four bytes each (type, `a`, `b`, `c`), in the order appended, including those the machine then drops (a full queue, a second feeds-updated event, the second half of a located anchor) |
| 4 | r (u32): the number of random bytes the program took (at most 463,872) |
| r | Those bytes, in order. A replay supplies exactly these; the program read zeros beyond the host's bytes either way (SPEC §system) |
| 4 | c (u32): the number of completions the host gave |
| … | c completions, in the order given (the machine applies them in increasing handle order, SPEC §content.submit) |
| 32 | The state hash after the frame |

A completion: one byte, the handle; one byte, its kind; then the kind's
fields:

| Kind | Outcome | Fields |
|---|---|---|
| `01h` | Get or load succeeded | size (u32), n (u32), the n bytes delivered |
| `02h` | Put succeeded | the blob ID (32) |
| `03h` | Records (subscriptions, timeline, names, programs, records) or find succeeded | the number of records that exist (u32), n (u32), the n bytes delivered |
| `04h` | Done (subscribe, unsubscribe, set nickname, save, delete) | — |
| `05h` | Publish or set profile succeeded | seq (u32) |
| `06h` | Identity succeeded | the feed key (32) |
| `0Ch` | Identity succeeded, with the hints of the host's own address (SPEC §content.feeds) | the feed key (32), n (u32), the n bytes of the hint list (1 to 161) |
| `07h` | Not found | — |
| `08h` | Failed | — |
| `09h` | Refused | — |
| `0Ah` | No room | — |
| `0Bh` | Bad request | — |

These are what the host hands the machine; the machine derives everything
it writes to the request block from them and the pending request (SPEC
§content.submit), and queues the content-done events itself, so neither is recorded. A
completion that matches no pending request is recorded like any other and
ignored again on replay. The requests pending in a start snapshot fail in
the first frame (SPEC §content.submit) without a record of it: they follow from the
snapshot.

A single machine's run in a host that records it never halts it in
`terse run` (its headless frames are not timed); the bus's recordings
(`DEVICES.md` 8) carry a stop when the host made one.

`terse run` never grants an experimental device (`spec/experimental.md`
§experimental.grants), so its recordings never have one; the bus's
recordings of such a machine have version 2 (`DEVICES.md` 8).

A replay makes the machine with the recorded grants (or the snapshot's),
on a host that can provide them, or refuses. Nothing else is an input of
a single machine: `open` always returns 0
(or `FFFF_FFFEh`) for a machine no shell started (SPEC §system), its argument is
in the snapshot (section 1.14; none from reset), and the page's layout
reaches it only as events. A recording is read to its end; a file that ends
inside a header or record, has a kind or controller bytes out of range, or a
frame counter other than the machine's, is refused, and a replay stops with
an error at the first frame whose hash differs. A frame record is only
written for a frame that ran: once the machine stops, `terse run` stops
too, so the last record is that of the frame in which it exited or
faulted.

## 6. Headless runs: scripted input, the screen dump, exit status

The expected files of this repository (`expected-hashes.txt`,
`expected-screen.txt`) are made by headless runs with scripted input. Every
implementation that checks itself against them (`terse run`, `terse-bus`,
`native/core-c`'s `tcore`) reads the same events file and prints the same
dump, as this section says; a new implementation that wants to use them
does the same.

### 6.1 Inputs of a headless run

Each frame's Unix time is 0, no random bytes are supplied (`random`
writes zeros, SPEC §system), there are no inputs but the events file's,
and content requests complete at the start of the next frame (the content
device's rule for a reproducible run, SPEC §content.determinism). A
controller is not connected until a `pad` line connects it.

### 6.2 The events file (`--events <file>`)

UTF-8 text; lines end with LF or CR LF. Each line is blank, a comment (its
first word starts with `#`), or an event line: a frame number (decimal, 0
to 4,294,967,295), whitespace, a command, and the command's arguments
separated by whitespace. Except in `text`, everything from a `#` to the
end of the line is ignored. A line that is not one of these, an unknown
command or key, a missing argument or a value out of range makes the whole
file invalid: the run does not start (exit status 2).

The events of frame N are appended in step 1 of the frame whose counter is
N (SPEC §frames), in the order their lines appear in the file; lines need
not be in frame order. Lines for a frame that is never run are ignored.

| Command | Arguments | Events appended (SPEC §events) |
|---|---|---|
| `down` | key, then any of the modifiers `shift`, `ctrl` (or `control`), `alt`, `meta` | one key down (`01h`) with that key code and the modifier bits |
| `up` | key, modifiers | one key up (`02h`) |
| `press` | key, modifiers | a key down, then a key up, both in this frame; no text event |
| `text` | the rest of the line | one text event (`03h`) per Unicode scalar value of the rest of the line, which starts after the single whitespace character that follows `text` and includes `#`, spaces and everything else up to the line end; none if it is empty |
| `pointer` | x, y, then optionally buttons | one pointer event (`04h`); x and y are decimal 0–65,535, clamped to 639 and 479; buttons decimal 0–7 (bit 0 primary), default 0 |
| `click` | x, y | a pointer event with buttons 1, then one with buttons 0, at the same position (clamped as above) |
| `wheel` | vertical, then optionally horizontal | one wheel event (`06h`); each decimal −127 to 127, default 0 |
| `feeds-updated` | — | one feeds-updated event (`07h`); queued as SPEC §events says (at most one a frame), on any machine |
| `home` | — | the host's home key (SPEC §machines.input), for a system; dropped when a single machine runs |
| `pad` | n, buttons, lx, ly, then optionally rx, ry | no event: controller n (0–3) is connected with this state from this frame on, until the next `pad` line for n. Buttons are hexadecimal (an optional `0x`), 0 to `FFFF`: byte 1 of the state in bits 0–7, byte 2 in bits 8–15 (SPEC §controllers). The sticks are decimal −127 to 127 (−128 is out of range); rx and ry default to 0, and there are exactly four or six arguments |
| `pad` | n, `off` | controller n is not connected from this frame on |

**Keys.** A key is a name or a USB HID usage ID, case-insensitive:

- a single character `a`–`z` is that letter's key (`04h`–`1Dh`), and a
  single digit is that digit's key (`1`–`9`: `1Eh`–`26h`, `0`: `27h`): so
  `5` is the key 5, not usage 5;
- `f1`–`f12` (`3Ah`–`45h`), `enter` (`28h`), `esc` or `escape` (`29h`),
  `backspace` (`2Ah`), `tab` (`2Bh`), `space` (`2Ch`), `insert` (`49h`),
  `home` (`4Ah`), `pageup` (`4Bh`), `delete` (`4Ch`), `end` (`4Dh`),
  `pagedown` (`4Eh`), `right`, `left`, `down`, `up` (`4Fh`–`52h`);
- two or more decimal digits are a usage ID, 0–255 (`40` is `28h`;
  `05` is usage 5), and so is `0x` followed by a hexadecimal number up to
  `FF` (`0x28`);
- `lctrl`, `lshift`, `lalt`, `lmeta` are the modifier keys themselves
  (`E0h`–`E3h`), which a window sends as keys too.

**Sequential lines (an extension).** `terse run`, `terse-bus` and
`terse-host` also read the following; an implementation that reads only
the lines above (`tcore`) refuses a file that uses them, so the expected
files of this repository do not.

- A line may start with its command, without a frame number: it belongs
  to the **current frame**, which is the frame of the line before it (0
  for the first line). A numbered line sets the current frame.
- `wait <n>` (no events) moves the current frame on by n (0 to
  4,294,967,295).
- `type <text>`: the rest of the line typed as a keyboard types it, one
  character a frame from the current frame on: for each character a key
  down with the key a US keyboard types it with (Shift held, modifier bit
  0, for capitals and the shifted symbols), its text event, and the key
  up, all in that frame (a character no key types, its text event alone;
  a newline is Enter and a tab Tab, without text). The current frame
  moves on by the number of characters.
- The commands of 6.5 (`shot`, `dump`, `settle`, `watch`, `expect`,
  `resize`, `pointer-px`, `click-px`, `drag`, `drag-px`, `hold`, `perf`,
  `budget`, `say`, `sleep`, `quit`) are for a window: a headless run reads
  them and does nothing (`terse run` says how many it passed over).

So one script serves a headless run and a window: in each, the events of
a frame arrive in that frame.

### 6.3 The screen dump (`--dump`)

After the last frame, the terminal's grid (device 2's cells, SPEC
§terminal.cells) as text, 30 lines, row 0 first, each ended by LF. A row
is its 80 cells from column 0, each written as follows, then stripped of
trailing U+0020 spaces:

- a cell holding `FFFFh` right after a cell that does not (the right half
  of a wide pair) is skipped: the wide character before it already stands
  for both;
- any other `FFFFh` (in column 0, or after another `FFFFh`) is a space;
- every other code is that character in UTF-8, control codes included; a
  surrogate code (`D800h`–`DFFFh`) is written as U+FFFD.

Nothing else (cursor, colours, palette) is shown. A machine without device
2 dumps nothing to standard output. `terse run --dump` and `terse-bus
--dump` print this; `--dump-vectors` and `--dump-page` (terse run only)
add lines after it.

### 6.4 The command line and exit status

`terse run`, `terse-bus` and `tcore` take, for a single machine: the image
file, `--frames N` (the most frames to run; the run ends earlier when the
machine exits or faults), `--events <file>`, `--hash` and `--hash-every N`
(section 3), `--dump`, and `--without <slot>` (an optional device
withheld, SPEC §image). They end with:

| Status | When |
|---|---|
| the exit code, 0–255 | the program exited; 0 also when it is still running after the last frame |
| ended by signal `SIGTERM` (a shell shows 143) | the machine faulted (SPEC §stopping: a fault is never shown as an exit code); the fault is printed on standard error |
| 2 | the run did not start: a bad command line or events file, an image refused (SPEC §image: malformed, or a required device or version not provided), a driver that could not be started or whose handshake failed (`DEVICES.md` 1.1, 2) |
| 3 | a driver failed while the machine ran (`DEVICES.md` 10) |
| 4 | the host stopped the machine: a device did not keep up (SPEC §stopping, halted; `PERFORMANCE.md` R15); the message names the device and where the machine's snapshot from just before went |

### 6.5 Driving a window (`terse-host --drive`)

`terse-host --drive <script>` runs the image (or the system) in its window,
in real time, with input from the script and reports on what it presents
(README, Driving the window). The script is an events file (6.2, with the
sequential lines); its lines are carried out in order, each in the frame it
belongs to, and these differ from a headless run:

- Input enters where the window system's does: `down`, `up` and `press`
  press the modifier keys named as keys of their own first (`press g
  ctrl` is Control down, G down, G up, Control up, the modifier state
  changing as a window system reports it) and a key that types a character
  on a US keyboard also gives its text, as a window does; `pointer` and
  `click` positions are screen units, turned into window pixels at the
  unit's centre and back through the window's mapping; `text` is text
  events without a key (as an input method gives them); `home` is the F12
  key.
- A line that observes (`shot`, `dump`, `watch`, `settle`, `expect`,
  `perf`, `budget`) looks at the first picture presented after its frame;
  the next line waits for it.

The commands of a window:

| Command | Arguments | What it does |
|---|---|---|
| `pointer-px`, `click-px` | x, y (window pixels)[, buttons] | as `pointer` and `click`, at a pixel of the window |
| `drag`, `drag-px` | x0, y0, x1, y1 | the primary button pressed at the first point, the pointer moved to the second over 8 frames, released in the 9th |
| `hold` | key, frames | the key held down for that many frames, repeating as a keyboard does (after 30 frames, every 2) |
| `resize` | width, height | asks for the window's inner size in pixels; the next line waits 3 frames |
| `sleep` | milliseconds | the next line waits that long in real time |
| `shot` | file | writes the picture presented, as PNG |
| `dump` | `terminal`, `page`, `vectors` or `state`, optionally a machine handle, optionally `> file` | the terminal grid (6.3), the page as this host laid it out (each line in view: its top, height and x in screen units and its text), the vector lists (as `--dump-vectors`), or the frame, window and machines |
| `watch` | frames, optionally `every <n>`, `save <dir>` | reports, after that many pictures, how many changed, where (a box of pixels) and by how much, how many were distinct, and blinks: 32 × 32 pixel tiles that changed and came back to what they were 2 to 4 pictures before |
| `settle` | optionally `max <frames>` (600), `still <n>` (10) | waits until n pictures in a row are the same |
| `expect idle` | frames, optionally `tolerance <pixels>` (0) | as `watch`; fails if a picture differs from the one before by more pixels than the tolerance, or blinks |
| `expect <terminal\|page\|vectors\|state> contains\|lacks` | text (the rest of the line) | fails unless the dump contains (lacks) the text |
| `expect budget` | percent | fails if a machine used more of a frame's budget (SPEC §frames) in any frame so far |
| `expect no-slow` | — | fails if a frame was late (`PERFORMANCE.md` R14) or the rule acted |
| `perf`, `budget` | — | the slow-frame notes so far; each machine's worst and last budget use since the last `budget` |
| `say` | text | prints the text |
| `quit` | — | ends the run (so does the end of a script file) |

The run ends with status 1 if an expectation failed, else as the window's.
