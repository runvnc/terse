# hello.asm - the hello-as program with descriptive names for instructions
# and registers. verbose.py translates it to plain RISC-V assembly for
# GNU as; see build.sh.

    .include "terse.inc"

    register event_address = temp0
    register event_type    = temp1
    register key_code      = temp1
    register wanted        = temp2

    .section .text.start, "ax"      # the linker script puts this first
    .globl _start
_start:
    load_value          arg0, CMD_CLEAR
    terse               TERM_COMMAND

    colours             15, 4               # white on blue
    at                  0, 0
    say                 title
    load_value          arg0, CMD_CLEAR_EOL # fill the rest of the row
    terse               TERM_COMMAND

    colours             11, 0               # yellow on black
    at                  2, 2
    say                 greeting

    colours             7, 0                # grey on black
    at                  2, 4
    say                 hint

next_frame:
    terse               WAIT_FRAME
next_event:
    load_address        arg0, event
    terse               NEXT_EVENT          # arg0 = 1 and the event, or 0
    branch_if_zero      arg0, next_frame
    load_address        event_address, event
    load_byte_unsigned  event_type, 0(event_address)
    load_value          wanted, EV_KEY_DOWN
    branch_if_not_equal event_type, wanted, next_event
    load_byte_unsigned  key_code, 1(event_address)
    load_value          wanted, KEY_ESCAPE
    branch_if_not_equal key_code, wanted, next_event

    load_value          arg0, 0
    terse               EXIT

    .section .rodata
title:    .asciz " Hello from descriptive assembly"
greeting: .asciz "Hello, Terse!"
hint:     .asciz "Press Esc to exit."

    .section .bss
    .balign 4
event:    .space 4                  # type, a, b, c
