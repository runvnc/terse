/* hostile.S - a deliberately hostile program for the device bus
 * (DEVICES.md): it spends every frame's whole budget on device calls, the
 * cheapest of each kind (1 for the ecall, and 4 for a deferred call or 64
 * for an answered one, SPEC §charges), in phases of 60 frames, then exits:
 *
 *   frames   0- 59  terminal get_cursor    (answered: never batched)
 *   frames  60-119  vector clear           (deferred: batched)
 *   frames 120-179  page locate            (answered)
 *   frames 180-239  terminal write, 1 byte (deferred: batched)
 *   frames 240-299  terminal cells_read, all 2,400 cells (answered; 9,600
 *                   bytes copied to RAM per call: the most bytes a unit buys)
 *   frames 300-359  vector submit of 16,384 bytes (deferred, copied inline)
 *   frames 360-419  terminal write of 4,096 line feeds (deferred; a scroll
 *                   per byte, which a write does not charge)
 *
 * Eight calls an iteration, so the loop costs a quarter unit per call. */

    .equ FRAME,         0x102
    .equ EXIT,          0x106
    .equ TERM_WRITE,    0x200
    .equ GET_CURSOR,    0x203
    .equ CELLS_READ,    0x206
    .equ VEC_SUBMIT,    0x300
    .equ VEC_CLEAR,     0x301
    .equ PAGE_LOCATE,   0x705

    .macro EIGHT
    ecall; ecall; ecall; ecall; ecall; ecall; ecall; ecall
    .endm
    /* Eight calls that each need a0 (and a1, a2) set again. */
    .macro EIGHT_SET
    .rept 8
    mv a0, s2
    mv a1, s3
    mv a2, s4
    ecall
    .endr
    .endm

    .section .text.start, "ax"
    .globl _start
_start:
next:
    li      a7, FRAME
    ecall
    li      t0, 60
    bltu    a0, t0, p_cursor
    li      t0, 120
    bltu    a0, t0, p_clear
    li      t0, 180
    bltu    a0, t0, p_locate
    li      t0, 240
    bltu    a0, t0, p_write
    li      t0, 300
    bltu    a0, t0, p_cells
    li      t0, 360
    bltu    a0, t0, p_submit
    li      t0, 420
    bltu    a0, t0, p_feeds
    li      a0, 0
    li      a7, EXIT
    ecall

p_cursor:
    li      a7, GET_CURSOR
    li      s1, 64
1:  EIGHT
    addi    s1, s1, -1
    bnez    s1, 1b
    j       next

p_clear:
    li      a7, VEC_CLEAR
    li      s1, 64
1:  EIGHT
    addi    s1, s1, -1
    bnez    s1, 1b
    j       next

p_locate:
    li      a7, PAGE_LOCATE
    li      a0, 1
    li      s1, 64
1:  EIGHT
    addi    s1, s1, -1
    bnez    s1, 1b
    j       next

p_write:
    li      a7, TERM_WRITE
    la      s2, text
    li      s3, 1
    li      s4, 0
    li      s1, 16
1:  EIGHT_SET
    addi    s1, s1, -1
    bnez    s1, 1b
    j       next

p_cells:
    li      a7, CELLS_READ
    li      s2, 0
    li      s3, 0x00100000
    li      s4, 2400
    li      s1, 2
1:  EIGHT_SET
    addi    s1, s1, -1
    bnez    s1, 1b
    j       next

p_submit:
    li      a7, VEC_SUBMIT
    li      s2, 0x00100000
    li      s3, 16384
    li      s4, 0
    li      s1, 2
1:  EIGHT_SET
    addi    s1, s1, -1
    bnez    s1, 1b
    j       next

p_feeds:
    li      a7, TERM_WRITE
    la      s2, feeds
    li      s3, 4096
    li      s4, 0
    li      s1, 2
1:  EIGHT_SET
    addi    s1, s1, -1
    bnez    s1, 1b
    j       next

    .section .rodata
text:
    .ascii "x"
feeds:
    .fill   4096, 1, 10
