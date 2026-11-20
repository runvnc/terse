# Only the System device (no device listed): every call of device 1, absent
# devices, unknown calls, bad buffers; results kept in RAM; exit 42 in frame 2.
#include "common.inc"
    START
    SYS 0x102; SAVE                                 # frame_count
    SYS 0x103; SAVE                                 # unix_time
    addi a0, s0, 64; li a1, 8; SYS 0x104; SAVE      # random, 8 bytes
    addi a0, s0, 64; li a1, 257; SYS 0x104; SAVE    # random, too long
    li a0, 0; li a1, 0; SYS 0x104; SAVE             # random, empty at 0
    lui a0, 0x10; li a1, 4; SYS 0x104; SAVE         # random into ROM
    li a0, 0; addi a1, s0, 80; SYS 0x105; SAVE      # controller 0
    li a0, 9; addi a1, s0, 88; SYS 0x105; SAVE      # controller 9
    li a0, 0; lui a1, 0x10; SYS 0x105; SAVE         # controller into ROM
    addi a0, s0, 96; SYS 0x101; SAVE                # next_event, empty
    li a0, 0; SYS 0x101; SAVE                       # next_event, bad buffer
    li a0, 1; SYS 0x109; SAVE                       # device 1
    li a0, 2; SYS 0x109; SAVE                       # device 2: absent
    li a0, 300; SYS 0x109; SAVE                     # device 300
    SYS 0x10A; SAVE                                 # unknown System call
    li a0, 0x55; SYS 0x200; SAVE                    # terminal write: absent
    SYS 0x800; SAVE                                 # machines: absent
    SYS 0xA00; SAVE                                 # full text: no calls
    li a7, 0x10100; ecall; SAVE                     # a7 above any slot
    la a0, req1; SYS 0x107; SAVE                    # open, blob (ROM)
    la a0, req2; SYS 0x107; SAVE                    # open, program (ROM)
    la a0, req3; SYS 0x107; SAVE                    # open, name starts with a space
    la a0, req4; SYS 0x107; SAVE                    # open, kind 3
    li a0, 0; SYS 0x107; SAVE                       # open at 0
    la a0, req5; SYS 0x107; SAVE                    # open, name of 49 bytes
    la a0, req6; SYS 0x107; SAVE                    # open, name with a control character
    addi a0, s0, 128; SYS 0x108; SAVE               # argument: none
    lui a0, 0x10; SYS 0x108; SAVE                   # argument into ROM
1:  SYS 0x100; SAVE                                 # wait_frame
    li t0, 2
    bltu a0, t0, 1b
    li a0, 42 + 256
    SYS 0x106
    .section .rodata
req1: .byte 1
    .fill 32, 1, 0xAB
req2: .byte 2
    .fill 32, 1, 0
    .byte 5
    .ascii "hello"
req3: .byte 2
    .fill 32, 1, 0
    .byte 3
    .ascii " hi"
req4: .byte 3
    .fill 40, 1, 0
req5: .byte 2
    .fill 32, 1, 0
    .byte 49
    .fill 49, 1, 0x61
req6: .byte 2
    .fill 32, 1, 0
    .byte 3
    .ascii "a\tb"
