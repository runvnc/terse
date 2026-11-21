/* print.S - a separate module: assembled on its own into print.o and joined
 * to hello.o by the linker. Only `print` is exported (.globl); anything
 * else defined here would stay private to this file. */

    .include "terse.inc"

    .text
    .globl print

/* print: write the zero-terminated string at a0 to the terminal.
 * A leaf routine: it uses only a0, a1, a7 and t0-t1. */
print:
    mv      t0, a0
1:  lbu     t1, 0(t0)               /* find the terminator */
    addi    t0, t0, 1
    bnez    t1, 1b
    sub     a1, t0, a0
    addi    a1, a1, -1              /* length, without the terminator */
    terse   TERM_WRITE              /* a0 = start, a1 = length */
    ret
