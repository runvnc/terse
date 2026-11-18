# Terse machine profile 2 (RV32IMC) — experimental

Status: an experiment, implemented in `crates/terse-rv`, to compare a RISC-V
machine with profile 1 (the Z80, SPEC.md). Nothing here is part of SPEC v0.
Profile 1 is unchanged; hosts run either, chosen by the image header.

Everything SPEC.md says about devices 1–4 (events, the terminal and its
UTF-8 rules, the display list, content operations and records, determinism)
holds for profile 2. Only the CPU, the memory map, the way the program reaches
devices, and the frame budget differ; this document describes those.

## 1. Machine

| Property | Value |
|---|---|
| CPU | RV32IMC, little-endian, user mode only |
| Budget | 500,000 instructions per frame (one constant: `INSTRUCTIONS_PER_FRAME`) |
| Frame rate | 60 frames per second, nominal |
| ROM | 1 to 131,072 bytes at `0001_0000h`, read and execute |
| RAM | 4 to 128 pages of 8 KiB at `0010_0000h`, read, write and execute; zero at reset |
| Devices | through `ecall` (section 5) |

### 1.1 CPU

Exactly RV32I, M and C. No A, F, D, B, Zicsr, Zifencei, no privileged
instructions: the program runs in user mode and cannot see CSRs, counters or
traps. `FENCE` (any form with funct3 = 0, including the hint forms) does
nothing. HINT encodings (for example `c.addi x0, 5`, `c.slli x0, 1`,
`c.srli x8, 0`, `slli x0, …`) execute as the instructions they are
(writes to `x0` are discarded).

These stop the machine with a **fault** (section 3): `ebreak` and `c.ebreak`;
every reserved or illegal encoding, including the all-zero parcel
(`c.unimp`), C.ADDI4SPN/C.ADDI16SP/C.LUI with a zero immediate, C.LWSP and
C.JR with register 0, RV32 C shifts with shamt[5] set, C.SUBW/C.ADDW and the
reserved MISC-ALU codes, every compressed floating-point load or store, all
48-bit and longer encodings; CSR instructions, `fence.i`, `mret`, `wfi` and
other privileged encodings; a load from outside ROM and RAM; a store outside
RAM (including ROM); a load or store whose address is not a multiple of its
size; fetching an instruction (either half of it) outside ROM and RAM.
Jump and branch targets need only be even (C is present); `jalr` clears bit 0
as usual. A jump to a bad address faults when the instruction there is
fetched.

Integer division follows the RISC-V rules: division by zero gives all ones
(`div`, `divu`) and the dividend (`rem`, `remu`); `INT_MIN / -1` gives
`INT_MIN` and remainder 0. Nothing traps.

### 1.2 Memory map and reset

| Address | Contents |
|---|---|
| `0001_0000h` to `0001_0000h + rom_len − 1` | ROM (the image's bytes) |
| `0010_0000h` to `0010_0000h + 8192 × ram_pages − 1` | RAM |
| everything else | nothing: any access faults |

So address 0 (a null pointer) faults. At reset: `pc = 0001_0000h` (the
first ROM byte), `sp` (`x2`) = the end of RAM, every other register zero, all
RAM zero, devices in their reset state.

## 2. Frames and the budget

No interrupts. For each frame the host:

1. appends the frame's input events, latches time, random bytes and
   controllers, and applies content completions (as SPEC 1.3 and 7.3);
2. adds 500,000 to the budget and runs instructions while it is positive;
3. ends the frame (the frame counter increases).

Every instruction costs 1. When the budget runs out the machine simply
pauses; the next frame continues with the next instruction, and the program
is not told. A program that is done for the frame calls `wait_frame`, which
discards the rest of the budget and returns at the start of the next frame
(the frame counter then reads one more). Device calls cost their own
instruction plus their charge (section 5.5), which may take the budget below
zero: the deficit carries into the next frame. Unused budget never carries
forward. An exit or a fault stops the machine at once; the frame still counts.

## 3. Faults

A fault stops the machine like an exit. The host records the kind, the
address of the faulting instruction (`pc`, left pointing at it; it does not
execute and is not counted) and the address involved (the load or store
address, the address that could not be fetched, or `pc`). `terse run` and
`terse-host` print it, e.g. `terse: fault: store outside RAM at pc 00010004,
address 00000000`, and the exit code is 255 (`FAULT_EXIT`). A program can
also exit with 255; the fault record tells them apart.

## 4. Image

The SPEC 2 header with machine profile byte `2` (offset 5); every other field
and rule is the same (ROM length, 4–128 RAM pages, devices, the image ID is
the BLAKE3 of the file). `terse pack --profile 2 …` builds one; `terse run`,
`terse info` and `terse-host` accept either profile.

## 5. Devices

`ecall` with `a7 = (device slot << 8) | function`, arguments in `a0`–`a5`,
result in `a0` (and `a1` where listed). A call changes no register but `a0`
and `a1` (`a1` only where listed) and no memory but the buffers it writes.

- An unknown call, or a call to a device the image does not list, returns
  `FFFF_FFFFh` and does nothing else.
- A buffer is a pointer and a length in guest memory. One the device reads
  must lie wholly in ROM or wholly in RAM; one it writes must lie wholly in
  RAM. Otherwise the call returns `FFFF_FFFEh` and does nothing (no fault).
  (Exceptions mirroring profile 1: a display list must be in RAM and a bad
  one is *rejected*, `vec_submit` returns 1; a content block not in RAM is
  rejected with handle 0.)
- Numbers that profile 1 holds in 8 bits (colours, commands, exit code) use
  the low 8 bits unless stated.

### 5.1 System (slot 1)

| a7 | Call | Arguments | Result |
|---|---|---|---|
| `100h` | `wait_frame` | — | the number of the frame it returns in |
| `101h` | `next_event` | a0 = ptr (4 bytes, RAM) | writes type, a, b, c (SPEC 5.1) and removes the event: 1; queue empty: 0 |
| `102h` | `frame_count` | — | the frame counter |
| `103h` | `unix_time` | — | the time latched at the start of the frame |
| `104h` | `random` | a0 = ptr, a1 = len (RAM) | fills len bytes with the frame's random bytes (zeros once the host's are used up, as port 1Ch); returns len |
| `105h` | `controller` | a0 = n, a1 = ptr (8 bytes, RAM) | writes controller n's 8 bytes (SPEC 5.3; all zero for n > 3 or disconnected); returns byte 0 |
| `106h` | `exit` | a0 = code (low 8 bits) | stops the machine |

### 5.2 Terminal (slot 2)

| a7 | Call | Arguments | Result |
|---|---|---|---|
| `200h` | `write` | a0 = ptr, a1 = len | put character (SPEC 6.3) for each byte; len |
| `201h` | `command` | a0 = command (SPEC 6.4; above 255: ignored) | 0 |
| `202h` | `set_cursor` | a0 = column, a1 = row (clamped to 79, 29; clears pending wrap) | 0 |
| `203h` | `get_cursor` | — | a0 = column, a1 = row |
| `204h` | `set_colours` | a0 = foreground, a1 = background; a value above 255 leaves that colour as it is | 0 |
| `205h` | `get_colours` | — | a0 = foreground, a1 = background |
| `206h` | `cells_read` | a0 = first cell (0–2399), a1 = ptr (RAM), a2 = count (≤ 2400) | copies count × 4 bytes of cell memory (SPEC 6.1) from cell a0, wrapping from 2399 to 0; count |
| `207h` | `cells_write` | a0 = first cell, a1 = ptr, a2 = count | the reverse (not checked, as port 28h); count |
| `208h` | `palette_set` | a0 = first entry (0–255), a1 = ptr, a2 = count (≤ 256) | sets count entries from r, g, b triples, wrapping from 255 to 0; count |
| `209h` | `size` | — | a0 = 80, a1 = 30 |

### 5.3 Vector display (slot 3)

| a7 | Call | Arguments | Result |
|---|---|---|---|
| `300h` | `submit` | a0 = ptr, a1 = len | SPEC 8.2's submit: the list must be wholly in RAM, 1 to 16,384 bytes, else rejected (previous list kept); 0 done, 1 rejected |
| `301h` | `clear` | — | 0 |

### 5.4 Content (slot 4)

| a7 | Call | Arguments | Result |
|---|---|---|---|
| `400h` | `submit` | a0 = address of a request block (RAM) | handle 1–8, or 0 if rejected (SPEC 7.3 steps 1–4) |

The operations (`01h` get, `02h` put, `03h` subscriptions, `04h` timeline),
statuses, handles, records and completion rules are SPEC 7's; completion is
announced by a content-done event at the start of a later frame. The block is
52 bytes, every field little-endian, and must lie wholly in RAM (any
alignment):

| Offset | Size | Field | Written by |
|---|---|---|---|
| 0 | 1 | `op` | program |
| 1 | 1 | `status` | device |
| 2 | 2 | reserved (ignored) | — |
| 4 | 32 | `id` | program for get; device for put |
| 36 | 4 | `offset` (get: byte offset; records: first index) | program |
| 40 | 4 | `length`: 1 to 16,384 (get, records); on completion the bytes delivered | both |
| 44 | 4 | `addr`: guest address of the data. Get and records: the `length` bytes must be in RAM. Put: the `size` bytes may be in ROM or RAM | program |
| 48 | 4 | `size`: blob size (get: on completion), put: 1 to 1,048,576; records: total records on completion | both |

### 5.5 Charges

Each call costs 1 (the `ecall` itself) plus:

- 1 instruction per 4 bytes moved between the device and guest memory
  (rounded up, per call): `next_event` 1, `random` len/4, `controller` 2,
  `write` len/4, `cells_read`/`cells_write` count, `palette_set` 3 × count / 4,
  vector `submit` len/4 when accepted, content put size/4 when accepted;
- 150 for each terminal clear, scroll up and scroll down (SPEC 6.5's 600
  T-states scaled by the same 4), including those caused by `write`.

Rejected calls, errors, and content completions (which happen between
frames) cost nothing more.

## 6. Programs

`programs/rv/lib`: `crt0.S` (copies `.data`, calls `main`, exits with its
result), `terse.ld` (ROM from `0001_0000h`, RAM from `0010_0000h`), and
`terse.h`/`terse.c` with the API of `programs/lib/terse.h` where the machines
allow (calls instead of ports; buffers instead of byte-at-a-time I/O; pointers
instead of linear RAM addresses). Build with clang's riscv32 target, ld.lld
and llvm-objcopy, as in `programs/rv/*/build.sh`:

    clang --target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib \
        -fno-builtin -ffunction-sections -fdata-sections -I programs/rv/lib -c …
    ld.lld -T programs/rv/lib/terse.ld --gc-sections crt0.o prog.o terse.o -o prog.elf
    llvm-objcopy -O binary prog.elf prog.bin
    terse pack --profile 2 --rom prog.bin --title prog --ram-pages N --device … -o prog.terse

There is no compiler-rt: avoid 64-bit division and floating point, or bring
the routines (`crates/terse-rv/tests/softfloat/softfloat.c` is a tested IEEE
double add/sub/mul/div in integer C). The checked-in images are tested to be
up to date with their sources when the tools are present.

Ports: `programs/rv/hello` (assembly), `programs/rv/reader`,
`programs/rv/sparks` (plain C). Their tests (`crates/terse-rv/tests/`) replay
the profile-1 programs' inputs and compare with the profile-1 golden screens.

## 7. Open issues found by the experiment

Measured with `crates/terse-rv/tests/compare.rs` (release build; see that
file for how):

- **The budget is too large.** This interpreter (`crates/terse-rv/src/cpu.rs`, a
  plain decode-once switch interpreter) runs about 240 million instructions a
  second on an i7-10750H, so a full 500,000-instruction frame takes 2.1 ms
  here, against 0.6 ms for a full profile-1 frame (300,000 T-states). Profile 2's C
  needs 0.4–0.5 times as many instructions as SDCC's Z80 code for the same
  work, so a budget of about 125,000–150,000 gives the same worst-case host
  cost as profile 1 and still several times its useful work per frame.
- **Device charges must match host cost.** Frames spent in calls cost more per
  budget unit than instructions (about 4 ns): terminal `write` 11 ns (the
  host does a put-character per byte but charges a quarter), `cells_write`
  17 ns and `random` 10 ns (done a byte at a time through the profile-1
  port interface; bulk copies would make them cheap), an `ecall` that does
  nothing about 6 ns. Suggested: `write` 1 per byte; bulk implementations
  for cell, palette and random transfers; a base charge of 4 per call.
- **Random bytes per frame.** `random` can ask for far more than the 27,273
  bytes a host supplies per frame for profile 1; the rest are zero. A
  profile-2 host should supply a fixed, documented number, or the device
  should expand a per-frame seed.
