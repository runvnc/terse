# Vendored rustzx-z80

Source: crates.io `rustzx-z80` 0.16.0 (MIT; upstream https://github.com/rustzx/rustzx).
The published crate ships no licence file; `LICENSE` here is the standard MIT
text with the authors from the crate metadata. Replace it with upstream's file
when convenient. The crate's `tests/` directory (which contains ZEXALL) is not
vendored.

Patch (src/cpu.rs only):

- `Z80::emulate` now returns `bool`. A step either accepts a pending interrupt
  (returns `true`, nothing else executed) or executes one instruction
  (returns `false`). Upstream accepts the interrupt and then executes the first
  instruction of the handler in the same call; Terse's frame budget (SPEC 1.3)
  needs the acceptance as its own step.
