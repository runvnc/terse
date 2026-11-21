# Building Tersenet programs: `terse build` and the toolchain

A Tersenet program is an image, a `.terse` file (SPEC.md §image): a 32-byte
header and the ROM, RV32IMC machine code and constants loaded at `0001_0000h` on
the Terse32 machine. `terse build` makes one from C, assembly or Rust:

    terse new hello.c              a starter program (also hello.rs, hello.S)
    terse build hello.c            -> hello.terse
    terse run hello.terse --frames 60 --dump

This document is the reference: what `terse build` runs, each flag and why it is
there, how to do the same by hand, a larger Rust project with cargo, and other
languages.

## 1. What you need

- **C and assembly:** `clang` with the RISC-V target and `ld.lld` (LLVM's C
  compiler and linker; most systems' packages include the RISC-V target).
- **Rust:** `rustc` from rustup with the `riscv32imc-unknown-none-elf` target:
  `rustup target add riscv32imc-unknown-none-elf`. rustc links with its own
  `rust-lld`, so neither clang nor lld is needed.

Nothing else: the C library, the start-up code, the linker script and the Rust
runtime of this project are built into `terse`. No include paths, environment
variables or files from this repository are needed. `llvm-objcopy` is not needed
either: `terse build` takes the ROM out of the linked ELF file itself.

When a tool is missing, `terse build` says in one sentence what to install,
for example

    terse: clang was not found: install clang and lld (LLVM's C compiler and linker; ...
    terse: Rust's riscv32imc-unknown-none-elf target is not installed: run `rustup target add riscv32imc-unknown-none-elf`.

The compiler's own errors and warnings about your code are shown as it prints
them.

## 2. The command

    terse build <source>... [-o <out.terse>] [--title <t>] [--ram-pages <n>]
                [--device <slot:version>]... [--optional <slot:version>]...
                [--cflag <flag>]... [--clang <path>] [--lld <path>] [--rustc <path>]
                [--show] [--run [the options of terse run]...]
    terse build --write-lib <dir>
    terse new <file.c|file.rs|file.S>

- **Sources.** The language is chosen by the extension: `.c` is C, `.S` and
  `.s` assembly (`.S` goes through the C preprocessor, `.s` does not), `.rs`
  Rust. Several C and assembly files may be given and are linked together, in
  the order given. A Rust program is one `.rs` file, the crate root; its
  modules (`mod x;`) are found from it as rustc always finds them.
- **`-o`**: the image. Default: the first source's stem with `.terse`, in the
  current directory (`terse build src/game.c` writes `game.terse`).
- **`--title`**: the image's title (SPEC §image), at most 16 bytes of UTF-8.
  Default: the first source's stem, cut to 16 bytes.
- **`--device`, `--optional`**: the devices the header lists, exactly as in
  `terse pack`: `--device` a device the program requires (a host without it
  refuses the image), `--optional` one it can run without (it asks with the
  `device` call). Without `--device`, the image requires the program profile
  (SPEC §profiles): `2:0`, the terminal, and `3:0`, the 2D vector display
  (device 1, System, is always there and is not listed). Giving `--device`
  replaces that list; `--optional` adds to it (and `--optional 3:0` makes the
  vector display optional instead of required). A program that prints
  characters beyond the basic repertoire (SPEC §terminal.text) adds
  `--optional 10:0`.
- **`--ram-pages`**: RAM in 8 KiB pages, 1 to 128. Default: the program's
  `.data` and `.bss` plus 8 KiB for the stack, rounded up to whole pages (2
  pages for a small C program, 1 for a small Rust one, whose `.bss` is often
  empty). The stack grows down from the end of RAM towards `.bss` and nothing
  stops it running into it, so give more if you put big arrays on the stack or
  recurse deeply. A count too small for `.data` and `.bss` alone is refused.
- **`--cflag`**: one more flag for clang (C and assembly), for example
  `--cflag -DDEBUG` or `--cflag -O2`; repeat it for several.
- **`--clang`, `--lld`, `--rustc`**: the tools to run. Default: the variables
  `CLANG`, `LLD` and `RUSTC` if set (the build.sh scripts of `programs/` read
  the same names), else `clang`, `ld.lld` and `rustc` from `PATH`.
- **`--show`**: print each command as it is run (on standard output), then the
  `terse pack` command that gives the same image. The files built into `terse`
  are written to a temporary directory for the build, named in the first line.
- **`--run`**: after building, run the image as `terse run` does. Everything
  after `--run` is given to `terse run`:
  `terse build hello.c --run --frames 60 --dump`.
- **`--write-lib <dir>`**: write the files built into `terse` to a directory:
  `terse.h`, `terse.c`, `crt0.S`, `terse.ld` (programs/lib) and `terse.rs`
  (programs/lib/rust), to read them or to build by hand (section 4).
- **`terse new <file>`** writes a small commented program that prints a line on
  the terminal, waits for each frame and exits on Esc, in C, Rust or assembly
  by the extension. It never overwrites a file.

`terse build` prints the image's size, its RAM and its ID on standard error:

    hello.terse: 1324 bytes (ROM 1288), RAM 2 pages (4 bytes of data, the rest stack), id de17f278…

The same sources and the same compiler give the same image, byte for byte.

## 3. What it runs, and why each flag

### C and assembly

    clang $CFLAGS -I <lib> -c <lib>/crt0.S -o 0.o       (only when there is C)
    clang $CFLAGS -I <lib> -c hello.c      -o 1.o       (each source, in order)
    clang $CFLAGS -I <lib> -c <lib>/terse.c -o 2.o
    ld.lld -T <lib>/terse.ld --gc-sections 0.o 1.o 2.o -o out.elf
    (the ROM: out.elf's loaded bytes from 00010000h, as llvm-objcopy -O binary writes them)
    terse pack --rom rom.bin --title hello --ram-pages 2 --device 2:0 --device 3:0 -o hello.terse

with

    CFLAGS = --target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os
             -ffreestanding -nostdlib -fno-builtin -ffunction-sections -fdata-sections

These are the flags of every `programs/*/build.sh`, and `terse build` makes
the same images: built with the header options of its build.sh, each of the
programs that use programs/lib (fetch, holo, hostile, reader, reports, shell,
sketch, sparks, tune, conform-perf, and hello from hello.S) comes out byte for
byte as its checked-in image (crates/terse-cli/tests/build.rs checks it).

| Flag | Why |
|---|---|
| `--target=riscv32-unknown-elf` | 32-bit RISC-V, no operating system: no system headers or libraries are assumed |
| `-march=rv32imc` | exactly the Terse32 CPU (SPEC §cpu): base integers, multiply and divide, compressed instructions; no atomics, no floating point registers |
| `-mabi=ilp32` | the calling convention without floating-point registers, matching `rv32imc` |
| `-Os` | small code: ROM is at most 128 KiB, and every instruction costs budget (SPEC §charges) |
| `-ffreestanding` | no hosted C library: `main` is not special, only freestanding headers (`stdint.h`, `stddef.h`) |
| `-nostdlib` | link no C library or start files (the link step is ld.lld's anyway) |
| `-fno-builtin` | do not treat `memcpy`, `strlen` and the like as known functions; terse.c provides `memcpy`, `memmove`, `memset` and `memcmp` |
| `-ffunction-sections -fdata-sections` | each function and variable in its own section, so `--gc-sections` drops the parts of terse.c a program does not use |
| `-I <lib>` | finds `terse.h` |
| `-T terse.ld` | the memory map (below) |
| `--gc-sections` | drop every section nothing refers to (the entry point is kept by the script) |

**crt0.S first, terse.c last.** `crt0.S` holds `_start`, in the section
`.text.start` that the linker script puts at the first byte of ROM, where the
machine starts (SPEC §reset). It copies `.data` from ROM to RAM (RAM is zero
at reset, so `.bss` needs nothing), calls `main` and exits with its return
value. It is linked when at least one source is C. A program only in assembly
starts itself: it defines `_start` in `.text.start` (see `programs/hello/hello.S`
or `terse new x.S`). terse.c is always linked; `--gc-sections` drops it
entirely from a program that calls nothing in it.

There is no C library beyond terse.c and no compiler runtime (libgcc or
compiler-rt): avoid 64-bit division and floating point in C, or bring the
routines.

### The linker script, terse.ld

    ROM (rx) : ORIGIN = 0x00010000, LENGTH = 128K
    RAM (rw) : ORIGIN = 0x00100000, LENGTH = 1M
    .text   : KEEP(*(.text.start)) then *(.text .text.*)   > ROM
    .rodata : constants                                    > ROM
    .data   : __data_start .. __data_end                   > RAM, loaded at __data_load in ROM
    .bss    : (NOLOAD) up to __bss_end                     > RAM
    /DISCARD/ : .comment, .note*, .riscv.attributes, .eh_frame*

The image's RAM is set by its header, not by the script (which allows the
largest, 1 MiB); `terse build` reads where `.bss` ends from the ELF file to
choose the default RAM and to refuse a count too small.

### Taking the ROM out of the ELF file

The ROM is the bytes of each loadable segment (ELF program header `PT_LOAD`
with a file size) placed at its load address, its physical address
(`p_paddr`), from `0001_0000h`, with the gaps between segments zero. So
`.data` is placed at its load address in ROM, after `.rodata`, where crt0
copies it from. This is what `llvm-objcopy -O binary` writes for these files.
`terse build` refuses an ELF file whose entry point is not `0001_0000h` (no
`_start` in `.text.start`) or which loads anything outside the 128 KiB of ROM.

### Rust

    rustc --edition 2024 --target riscv32imc-unknown-none-elf -C opt-level=s -Cpanic=abort \
          --remap-path-prefix=<lib>/= --crate-type rlib --crate-name terse \
          -o <tmp>/libterse.rlib <lib>/terse.rs
    rustc --edition 2024 --target riscv32imc-unknown-none-elf -C opt-level=s -Cpanic=abort \
          --remap-path-prefix=<lib>/= -C lto -C strip=debuginfo --crate-type bin --crate-name hello \
          -C link-arg=-T<lib>/terse.ld --extern terse=<tmp>/libterse.rlib -o out.elf hello.rs

then the ROM and `terse pack` as for C.

| Flag | Why |
|---|---|
| `--edition 2024` | rustc's own default is the 2015 edition |
| `--target riscv32imc-unknown-none-elf` | Rust's target for exactly RV32IMC with no operating system (`core` and `alloc` only, no `std`) |
| `-C opt-level=s` | small code, as `-Os` for C |
| `-Cpanic=abort` | no unwinding: a panic goes straight to the runtime's panic handler |
| `--remap-path-prefix=<lib>/=` | the runtime's panic messages name `terse.rs`, not the temporary directory, so the same source gives the same image |
| `--crate-type rlib --crate-name terse` | the runtime is an ordinary library crate, used as `terse` |
| `-C lto` | link-time optimisation across the program, the runtime and `core`: about 12% smaller for hello (1,964 to 1,716 bytes) |
| `-C strip=debuginfo` | `core` comes with debug information; it never reaches the ROM, but it makes the ELF file large |
| `-C link-arg=-T<lib>/terse.ld` | the same memory map as C. rustc links with `rust-lld` and passes `--gc-sections` itself |
| `--extern terse=...` | the runtime, so the program can say `use terse::*;` |

A Rust program is a `#![no_std]`, `#![no_main]` crate root that names its main
function with `terse::entry!`:

    #![no_std]
    #![no_main]
    use terse::*;
    entry!(main);

    fn main() {
        term_puts("Hello from Rust\n");
        loop {
            wait_frame();
        }
    }

### The Rust runtime, programs/lib/rust/terse.rs

One file, about 590 lines, the counterpart of crt0.S, terse.h and terse.c for
the program profile:

- `_start` (in `global_asm!`, in `.text.start`): copies `.data`, calls the
  function `entry!` names, and exits with code 0 if it returns.
- the panic handler: writes `panic at <file>:<line>` on the terminal and stops
  the machine with `ebreak`, a breakpoint fault, which the host shows as a fault
  (SPEC §stopping). It does not format the panic's message (that would pull in
  `core::fmt` for every program).
- `call`, `call2`, `call6`: the raw `ecall` (SPEC §calls), `unsafe`; and safe
  functions named as in terse.h without its `terse_` prefix: System
  (`wait_frame`, `frame`, `time`, `exit`, `random`, `random_byte`, `device`,
  `has_full_text`), events (`next_event` gives an `Event` with `kind`, `key()`,
  `modifiers()`, `char()`, `pointer()`, `buttons()`, `wheel()`; `EV_*`, `KEY_*`,
  `MOD_*`, `BUTTON_*`), controllers (`pad_read` gives a `Pad`; `PAD_*`), the
  terminal (`term_write`, `term_puts`, `term_putc`, `term_put_char`,
  `term_put_dec`, `term_goto`, `term_color`, `term_fg`, `term_cmd`,
  `term_get_cursor`, `term_get_colours`, `cell`, `term_cells_write`,
  `term_cells_read`, `term_palette_set`, `term_fill`; `TERM_*`) and the 2D vector
  display (`VecList`, a list builder over a buffer you own with `colour`,
  `width`, `origin`, `move_to`, `line_to`, `rect`, `circle`, `disc`, `tri` and
  `submit`; `vec_submit`, `vec_clear`, `vec_style`, `vec_persistence`,
  `vec_fill`; `VEC_*`). The functions that take buffers take slices, so they
  are safe: a device never touches memory it was not handed.
- `Term`, which implements `core::fmt::Write`: `write!(Term, "{} points", n)`.

The devices outside the program profile (content, the page, sound, the 3D
display, machines) are not in the Rust runtime; their calls are reached with
`call` and the numbers of SPEC §calls.table (terse.h has them all).

What it costs and what works, measured with rustc 1.100 and clang 23:

| Program | ROM |
|---|---|
| hello (the program of programs/hello) in assembly, hello.S | 898 bytes |
| the same in C, programs/hello/hello.c | 1,288 bytes |
| the same in Rust, programs/hello-rs/hello.rs | 1,716 bytes |
| `terse new` starter: C / Rust / assembly | 347 / 229 / 123 bytes |
| Rust, one `write!(Term, "{} {}", a, b)` of integers | about 1.9 KiB in all |
| Rust, formatting one `f32` with `{:.3}` | about 23 KiB in all |

- Integer formatting with `write!` is cheap; floating-point formatting is not.
  `term_put_dec` writes a number without `core::fmt`.
- Rust's `compiler_builtins` comes with `core`, so 64-bit division and software
  floating point work in Rust (unlike C here), at their cost in ROM and budget.
- `alloc` (`Vec`, `String`, `Box`) works if the program brings a global
  allocator (`#[global_allocator]`): the runtime has none, as most programs on a
  machine with 8 KiB pages are better with fixed buffers. A bump allocator over a
  `static mut` array is about 20 lines.
- `static mut` and `&raw mut` work as usual; there is one thread and no
  interrupts.
- No `std`: no files, threads, time or printing except through the devices.

## 4. Doing it by hand

`terse build --show` prints the commands; `terse build --write-lib lib` writes
the files they use. With those, a build needs no `terse build`:

    terse build --write-lib lib
    set CFLAGS --target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os -ffreestanding -nostdlib -fno-builtin -ffunction-sections -fdata-sections
    clang $CFLAGS -I lib -c lib/crt0.S -o 0.o
    clang $CFLAGS -I lib -c hello.c -o 1.o
    clang $CFLAGS -I lib -c lib/terse.c -o 2.o
    ld.lld -T lib/terse.ld --gc-sections 0.o 1.o 2.o -o hello.elf
    llvm-objcopy -O binary hello.elf hello.bin
    terse pack --rom hello.bin --title hello --ram-pages 2 --device 2:0 --device 3:0 -o hello.terse

(fish syntax for `set`; in sh, `CFLAGS="..."`). The `build.sh` of each program
in `programs/` is such a script.

For Rust:

    terse build --write-lib lib
    rustc --edition 2024 --target riscv32imc-unknown-none-elf -C opt-level=s -Cpanic=abort --crate-type rlib --crate-name terse -o libterse.rlib lib/terse.rs
    rustc --edition 2024 --target riscv32imc-unknown-none-elf -C opt-level=s -Cpanic=abort -C lto --crate-type bin -C link-arg=-Tlib/terse.ld --extern terse=libterse.rlib -o hello.elf hello.rs
    llvm-objcopy -O binary hello.elf hello.bin     (or rust-objcopy, which rustup's llvm-tools component installs)
    terse pack --rom hello.bin --title hello --ram-pages 1 --device 2:0 --device 3:0 -o hello.terse

## 5. A larger Rust project with cargo

For a program of several crates, or one that uses crates from crates.io that
support `no_std`, use cargo with the runtime as a path dependency:

    game/
      Cargo.toml
      .cargo/config.toml
      terse-rt/Cargo.toml     [package] name = "terse", edition = "2024"; [lib] path = "terse.rs"
      terse-rt/terse.rs       from terse build --write-lib
      terse.ld                from terse build --write-lib
      src/main.rs             #![no_std] #![no_main] use terse::*; entry!(main); ...

`Cargo.toml`:

    [package]
    name = "game"
    version = "0.1.0"
    edition = "2024"

    [dependencies]
    terse = { path = "terse-rt" }

    [profile.release]
    opt-level = "s"
    lto = true
    panic = "abort"
    debug = false

    [profile.dev]
    panic = "abort"

`.cargo/config.toml`:

    [build]
    target = "riscv32imc-unknown-none-elf"

    [target.riscv32imc-unknown-none-elf]
    rustflags = ["-C", "link-arg=-Tterse.ld"]

Then

    cargo build --release
    llvm-objcopy -O binary target/riscv32imc-unknown-none-elf/release/game game.bin
    terse pack --rom game.bin --title game --ram-pages 4 --device 2:0 --device 3:0 -o game.terse

The linker script path is relative to the directory cargo runs the linker in,
the package's root. This gives the same ROM as `terse build` for the same
source (checked with hello.rs). A debug build (`cargo build`, `opt-level = 0`)
works but is far larger and slower: 11,701 bytes of ROM for hello instead of
1,716, and a larger program may not fit in 128 KiB of ROM or in a frame's
budget.

## 6. Other languages

Anything that can produce RV32IMC machine code for a bare machine can make a
Tersenet program. What a compiler or its runtime must do:

- **Start.** Execution begins at the first byte of ROM, `0001_0000h`, with `sp`
  at the end of RAM (`0010_0000h` + 8,192 × RAM pages) and every other
  register zero (SPEC §reset). Nothing else is set up: copy initialised data
  to RAM yourself (RAM is zero, so zero-initialised data needs nothing). There
  is no `gp` set: a linker that relaxes to `gp`-relative addressing needs the
  start code to set `gp` (ld.lld does not relax to `gp` by default; GNU ld
  does, and then `__global_pointer$` must be loaded into `gp` first).
- **CPU.** RV32I with M and C only (SPEC §cpu): no atomics (`-march` without
  `a`), no floating-point instructions (soft float, `ilp32`), no CSRs, so no
  `rdcycle` or `rdtime`; `fence` does nothing. Loads and stores must be aligned
  to their size, or the machine faults: a compiler must not assume misaligned
  access is allowed (clang and rustc do not for this target).
- **Memory.** ROM is read and execute only, 1 to 128 KiB; a store to it
  faults. RAM is 8 KiB pages, 1 to 128, the count fixed in the image's header.
  Address 0 and everything outside ROM and RAM fault, so a null pointer
  dereference faults.
- **Calls.** Devices are reached with `ecall`: `a7` = slot × 256 + function,
  arguments in `a0` to `a5`, the result in `a0` (and `a1`); other registers are
  preserved (SPEC §calls).
- **Frames.** There are no interrupts or threads. A program works, then calls
  `wait_frame` (`a7` = 100h); each frame has a budget of 125,000 units and a
  program that uses it up is paused until the next frame (SPEC §frames). A
  garbage collector or any long computation must fit in this or be split across
  frames.
- **Exit.** `exit` (`a7` = 106h) with the code in `a0`.
- **Image.** The image is the 32-byte header (SPEC §image) and the ROM;
  `terse pack` writes it from the raw ROM.

Other compilers with a 32-bit RISC-V bare-metal target (Zig, for one) should
fit these rules with their own start code and the linker script above; none
has been tried.
