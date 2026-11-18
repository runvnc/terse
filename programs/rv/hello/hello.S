/* hello.S - Terse demo program for machine profile 2 (RV32IMC,
 * RV-PROFILE.md): the same screen as programs/hello/hello.asm (profile 1).
 *
 * Shows a title bar, some UTF-8 text, palette entries 0-15 written straight
 * into cell memory, and a frame counter; echoes typed text; Esc exits.
 * Build with build.sh (clang, ld.lld, llvm-objcopy, `terse pack --profile 2`).
 *
 * Devices are calls: a7 = (slot << 8) | function, arguments in a0-a2,
 * result in a0. Nothing is waiting on an interrupt: the program asks for the
 * next frame with wait_frame. */

    .equ WAIT_FRAME,    0x100
    .equ NEXT_EVENT,    0x101
    .equ FRAME,         0x102
    .equ EXIT,          0x106
    .equ TERM_WRITE,    0x200
    .equ TERM_COMMAND,  0x201
    .equ SET_CURSOR,    0x202
    .equ SET_COLOURS,   0x204
    .equ CELLS_WRITE,   0x207

    .equ CMD_CLEAR_EOL, 4
    .equ EV_KEY_DOWN,   1
    .equ EV_TEXT,       3
    .equ KEY_ENTER,     0x28
    .equ KEY_ESCAPE,    0x29
    .equ KEY_BACKSPACE, 0x2A

    .equ PALETTE_CELL,  7*80+2      /* cell index of the palette row */
    .equ FRAME_CELL,    9*80+8      /* the frame counter's eight digits */

    .section .text.start, "ax"
    .globl _start
_start:
    /* Title bar: white on blue across the whole of row 0. */
    li      a0, 15
    li      a1, 4
    li      a7, SET_COLOURS
    ecall
    la      a0, title
    call    print
    li      a0, CMD_CLEAR_EOL
    li      a7, TERM_COMMAND
    ecall

    /* Text lines: each entry is row, column, fg, bg, zero-terminated UTF-8. */
    la      s0, texts
    li      s1, 0xFF
text_loop:
    lbu     a1, 0(s0)               /* row */
    beq     a1, s1, texts_done
    lbu     a0, 1(s0)               /* column */
    li      a7, SET_CURSOR
    ecall
    lbu     a0, 2(s0)
    lbu     a1, 3(s0)
    li      a7, SET_COLOURS
    ecall
    addi    a0, s0, 4
    call    print
    mv      s0, a0                  /* past the terminator */
    j       text_loop
texts_done:

    /* Palette row: three cells per entry, background = entry, built in RAM
     * and written to cell memory in one call. A cell is the character
     * (16 bits), the foreground, the background. */
    la      s0, cells
    li      s1, 0                   /* entry */
pal_loop:
    li      t0, 15                  /* white on entries 0-6, black on the rest */
    sltiu   t1, s1, 7
    neg     t1, t1
    and     t0, t0, t1
    slli    t0, t0, 16
    slli    t1, s1, 24
    or      s2, t0, t1              /* the colours of this entry's cells */
    ori     t0, s2, ' '
    sw      t0, 0(s0)
    sw      t0, 8(s0)
    mv      a0, s1
    call    hex_digit
    or      t0, s2, a0
    sw      t0, 4(s0)
    addi    s0, s0, 12
    addi    s1, s1, 1
    li      t0, 16
    bne     s1, t0, pal_loop
    li      a0, PALETTE_CELL
    la      a1, cells
    li      a2, 48
    li      a7, CELLS_WRITE
    ecall

    /* Prompt; typed text appears after it. */
    li      a0, 0
    li      a1, 11
    li      a7, SET_CURSOR
    ecall
    li      a0, 10
    li      a1, 0
    li      a7, SET_COLOURS
    ecall
    la      a0, prompt
    call    print
    li      a0, 15
    li      a1, 0x100               /* background unchanged */
    li      a7, SET_COLOURS
    ecall

main:
    li      a7, WAIT_FRAME          /* one pass per frame */
    ecall
    call    show_frame
event_loop:
    la      a0, event
    li      a7, NEXT_EVENT
    ecall
    beqz    a0, main
    la      t0, event
    lbu     t1, 0(t0)               /* type */
    lbu     a0, 1(t0)
    li      t2, EV_TEXT
    bne     t1, t2, not_text
    lbu     t1, 2(t0)               /* code point: a | b << 8 | c << 16 */
    slli    t1, t1, 8
    or      a0, a0, t1
    lbu     t1, 3(t0)
    slli    t1, t1, 16
    or      a0, a0, t1
    call    put_code_point
    j       event_loop
not_text:
    li      t2, EV_KEY_DOWN
    bne     t1, t2, event_loop
    li      t2, KEY_ESCAPE
    beq     a0, t2, quit
    la      t1, newline
    li      t2, KEY_ENTER
    beq     a0, t2, 1f
    la      t1, rubout
    li      t2, KEY_BACKSPACE
    bne     a0, t2, event_loop
1:  mv      a0, t1
    call    print
    j       event_loop

quit:
    li      a0, 0
    li      a7, EXIT
    ecall

/* Prints the zero-terminated string at a0; returns a0 past the terminator. */
print:
    mv      t0, a0
1:  lbu     t1, 0(t0)
    addi    t0, t0, 1
    bnez    t1, 1b
    sub     a1, t0, a0
    addi    a1, a1, -1              /* length without the terminator */
    mv      t2, t0
    li      a7, TERM_WRITE
    ecall
    mv      a0, t2
    ret

/* Returns the ASCII hex digit for the low nibble of a0. */
hex_digit:
    andi    a0, a0, 15
    addi    a0, a0, '0'
    li      t0, '9' + 1
    bltu    a0, t0, 1f
    addi    a0, a0, 7
1:  ret

/* Writes the 32-bit frame counter as eight hex digits, yellow on black,
 * into cell memory. */
show_frame:
    mv      s3, ra
    li      a7, FRAME
    ecall
    mv      s1, a0
    la      s0, cells
    li      s2, 28                  /* shift of the most significant digit */
1:  srl     a0, s1, s2
    call    hex_digit
    li      t0, 14 << 16
    or      a0, a0, t0
    sw      a0, 0(s0)
    addi    s0, s0, 4
    addi    s2, s2, -4
    bgez    s2, 1b
    li      a0, FRAME_CELL
    la      a1, cells
    li      a2, 8
    li      a7, CELLS_WRITE
    ecall
    mv      ra, s3
    ret

/* Writes code point a0 (at most 21 bits) as UTF-8. Above FFFFh: U+FFFD. */
put_code_point:
    la      a1, utf8
    li      t0, 0x80
    bltu    a0, t0, one
    li      t0, 0x800
    bltu    a0, t0, two
    li      t0, 0x10000
    bltu    a0, t0, three
    li      a0, 0xFFFD
three:                              /* 1110zzzz 10yyyyyy 10xxxxxx */
    srli    t0, a0, 12
    ori     t0, t0, 0xE0
    sb      t0, 0(a1)
    srli    t0, a0, 6
    andi    t0, t0, 0x3F
    ori     t0, t0, 0x80
    sb      t0, 1(a1)
    andi    t0, a0, 0x3F
    ori     t0, t0, 0x80
    sb      t0, 2(a1)
    li      t1, 3
    j       write
two:                                /* 110yyyyy 10xxxxxx */
    srli    t0, a0, 6
    ori     t0, t0, 0xC0
    sb      t0, 0(a1)
    andi    t0, a0, 0x3F
    ori     t0, t0, 0x80
    sb      t0, 1(a1)
    li      t1, 2
    j       write
one:
    sb      a0, 0(a1)
    li      t1, 1
write:
    mv      a0, a1
    mv      a1, t1
    li      a7, TERM_WRITE
    ecall
    ret

    .section .rodata
title:  .asciz " Terse · hello"
texts:  .byte 2, 2, 11, 0
        .asciz "héllo, wörld — 你好"
        .byte 3, 2, 7, 0
        .asciz "Ελληνικά · кириллица · ┌─┬─┐ · ♥ ★ ✓"
        .byte 4, 2, 8, 0
        .asciz "Type to echo text below. Enter: new line, Backspace, Esc: exit."
        .byte 6, 2, 7, 0
        .asciz "Palette entries 0-15, written through the cell pointer:"
        .byte 9, 2, 7, 0
        .asciz "frame"
        .byte 0xFF
prompt: .asciz "> "
newline: .asciz "\n> "
rubout: .byte 8, ' ', 8, 0

    .section .bss
    .balign 4
cells:  .space 48 * 4
event:  .space 4
utf8:   .space 4
