/* terse.inc - Terse call numbers and small macros for GNU as.
 *
 * Pulled in with `.include "terse.inc"`, which is plain text inclusion done
 * by as itself: no C preprocessor. The numbers are those of
 * programs/lib/terse.h (SPEC.md, the device call tables).
 *
 * A call is a7 = (slot << 8) | function, arguments in a0-a2, result in a0. */

/* Device 1, System */
    .equ WAIT_FRAME,    0x100
    .equ NEXT_EVENT,    0x101
    .equ EXIT,          0x106

/* Device 2, Terminal */
    .equ TERM_WRITE,    0x200
    .equ TERM_COMMAND,  0x201
    .equ SET_CURSOR,    0x202
    .equ SET_COLOURS,   0x204

    .equ CMD_CLEAR,     1
    .equ CMD_CLEAR_EOL, 4
    .equ EV_KEY_DOWN,   1
    .equ KEY_ESCAPE,    0x29

/* terse NAME: make the call NAME. */
.macro terse name:req
    li      a7, \name
    ecall
.endm

/* colours FG, BG: set the text colours (palette entries 0-15). */
.macro colours fg:req, bg:req
    li      a0, \fg
    li      a1, \bg
    terse   SET_COLOURS
.endm

/* at COLUMN, ROW: move the cursor. */
.macro at column:req, row:req
    li      a0, \column
    li      a1, \row
    terse   SET_CURSOR
.endm

/* say LABEL: print the zero-terminated string at LABEL (print.S). */
.macro say label:req
    la      a0, \label
    call    print
.endm
