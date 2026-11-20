# Shared by the test images: s0 = RAM base, s1 = where results are stored.
.macro SYS n
    li a7, \n
    ecall
.endm
.macro SAVE
    sw a0, 0(s1)
    addi s1, s1, 4
.endm
.macro START
    .section .text.start, "ax"
    .globl _start
_start:
    lui s0, 0x100
    addi s1, s0, 1024
.endm
