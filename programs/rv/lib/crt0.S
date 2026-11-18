/* crt0 for Terse profile 2 (RV32IMC, RV-PROFILE.md).
 *
 * The machine starts here, at the first byte of ROM (00010000h), with sp at
 * the end of RAM and every other register zero. RAM is zero at reset, so
 * .bss needs no clearing; only .data is copied from its load address in
 * ROM. main's return value is the exit code. */

    .section .text.start, "ax"
    .globl _start
_start:
    la      a0, __data_load
    la      a1, __data_start
    la      a2, __data_end
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
