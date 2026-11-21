# terse.inc - Terse call numbers and small macros, in descriptive form.
#
# Translated by verbose.py like the other files, then pulled in by as with
# `.include`. The numbers are those of programs/lib/terse.h.
#
# A call puts its number in arg7 and its arguments in arg0-arg2; the result
# comes back in arg0.

    register call_number = arg7

# Device 1, System
    .equ WAIT_FRAME,    0x100
    .equ NEXT_EVENT,    0x101
    .equ EXIT,          0x106

# Device 2, Terminal
    .equ TERM_WRITE,    0x200
    .equ TERM_COMMAND,  0x201
    .equ SET_CURSOR,    0x202
    .equ SET_COLOURS,   0x204

    .equ CMD_CLEAR,     1
    .equ CMD_CLEAR_EOL, 4
    .equ EV_KEY_DOWN,   1
    .equ KEY_ESCAPE,    0x29

# terse NAME: make the call NAME.
.macro terse name:req
    load_value          call_number, \name
    environment_call
.endm

# colours FG, BG: set the text colours (palette entries 0-15).
.macro colours fg:req, bg:req
    load_value          arg0, \fg
    load_value          arg1, \bg
    terse               SET_COLOURS
.endm

# at COLUMN, ROW: move the cursor.
.macro at column:req, row:req
    load_value          arg0, \column
    load_value          arg1, \row
    terse               SET_CURSOR
.endm

# say LABEL: print the zero-terminated string at LABEL (print.asm).
.macro say label:req
    load_address        arg0, \label
    call                print
.endm
