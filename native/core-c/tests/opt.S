# Lists the terminal, content and full text as optional, run with none of
# them granted: device answers FFFF_FFFFh and their calls do nothing.
#include "common.inc"
    START
    li a0, 2; SYS 0x109; SAVE
    li a0, 4; SYS 0x109; SAVE
    li a0, 10; SYS 0x109; SAVE
    la a0, msg; li a1, 2; SYS 0x200; SAVE
    SYS 0x203; SAVE; mv a0, a1; SAVE
    SYS 0x100
    li a0, 3; SYS 0x106
msg: .ascii "hi"
