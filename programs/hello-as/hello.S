/* hello.S - a minimal Terse terminal program for GNU as.
 *
 * Clears the screen, prints a title bar and a greeting, then waits for Esc.
 * Shared constants and macros come from terse.inc with `.include`; `print`
 * lives in its own module, print.S. Build with build.sh.
 *
 * Despite the capital .S, nothing here needs the C preprocessor: as reads
 * this file directly. */

    .include "terse.inc"

    .section .text.start, "ax"      /* the linker script puts this first */
    .globl _start
_start:
    li      a0, CMD_CLEAR
    terse   TERM_COMMAND

    colours 15, 4                   /* white on blue */
    at      0, 0
    say     title
    li      a0, CMD_CLEAR_EOL       /* fill the rest of the row in blue */
    terse   TERM_COMMAND

    colours 11, 0                   /* yellow on black */
    at      2, 2
    say     greeting

    colours 7, 0                    /* grey on black */
    at      2, 4
    say     hint

main:
    terse   WAIT_FRAME              /* once per frame */
events:
    la      a0, event
    terse   NEXT_EVENT              /* a0 = 1 and the event, or 0 */
    beqz    a0, main
    la      t0, event
    lbu     t1, 0(t0)               /* type */
    li      t2, EV_KEY_DOWN
    bne     t1, t2, events
    lbu     t1, 1(t0)               /* key code */
    li      t2, KEY_ESCAPE
    bne     t1, t2, events

    li      a0, 0
    terse   EXIT

    .section .rodata
title:    .asciz " Hello from GNU as"
greeting: .asciz "Hello, Terse!"
hint:     .asciz "Press Esc to exit."

    .section .bss
    .balign 4
event:    .space 4                  /* type, a, b, c */
