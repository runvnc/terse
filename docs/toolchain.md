# Building Tersenet programs: `terse build` and the toolchain

This reference now lives in the manual, as four articles (the single source;
`terse manual site` makes them web pages too):

- `docs/manual/toolchain.txt`, "Building a program": what you need, every
  option of `terse build` and `terse new`, what they run and why each flag is
  there, the linker script, the ROM taken from the ELF file, and the devices
  an image lists.
- `docs/manual/toolchain-c.txt`, "The C library": crt0.S, terse.ld, terse.h
  and terse.c, and the hello program in assembly and in C.
- `docs/manual/toolchain-rust.txt`, "Rust programs": the rustc commands, the
  runtime `programs/lib/rust/terse.rs`, what things cost, and a larger program
  with cargo.
- `docs/manual/toolchain-other.txt`, "Building by hand, and other languages":
  the commands without `terse build`, and what any other compiler or runtime
  must do (start code, `.data`, the unguarded stack, no `gp`, display lists in
  RAM, calls, frames).

In short:

    terse new hello.c              a starter program (also hello.rs, hello.S)
    terse build hello.c            -> hello.terse
    terse run hello.terse --frames 60 --dump
