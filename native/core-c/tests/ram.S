# Code written to RAM, run (4 units an instruction), rewritten, run again,
# then a loop across several frames, then exit from RAM.
#include "common.inc"
    START
    la t0, code
    mv t1, s0
    li t2, 4
1:  lw t3, 0(t0)
    sw t3, 0(t1)
    addi t0, t0, 4
    addi t1, t1, 4
    addi t2, t2, -1
    bnez t2, 1b
    jalr s0
    li t3, 0x00900513           # li a0, 9
    sw t3, 0(s0)
    jalr s0
    li t3, 300000               # several frames of ROM loop
2:  addi t3, t3, -1
    bnez t3, 2b
    li t3, 0x10600893           # li a7, 0x106
    sw t3, 4(s0)
    li t3, 0x00000073           # ecall
    sw t3, 8(s0)
    jr s0
    .balign 4
    .option norvc
code: li a0, 5
    nop
    nop
    ret
