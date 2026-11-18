/* crt0 for the conformance image. As programs/lib/crt0.S, but the stack
 * starts at STACK_TOP (conform.ld), 16 KiB below the end of RAM, so the
 * checks can use the last 16 KiB of RAM (a display list ending at the last
 * byte, buffers straddling the end). */

    .option norelax
    .section .text.start, "ax"
    .globl _start
_start:
    lui     sp, %hi(__stack_top)
    addi    sp, sp, %lo(__stack_top)
    lui     a0, %hi(__data_load)
    addi    a0, a0, %lo(__data_load)
    lui     a1, %hi(__data_start)
    addi    a1, a1, %lo(__data_start)
    lui     a2, %hi(__data_end)
    addi    a2, a2, %lo(__data_end)
    j       2f
1:  lw      a3, 0(a0)
    sw      a3, 0(a1)
    addi    a0, a0, 4
    addi    a1, a1, 4
2:  bltu    a1, a2, 1b
    call    main
    li      a7, 0x106           /* exit(a0) */
    ecall
3:  j       3b
