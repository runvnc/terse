/* cpu.S: the conformance image's CPU checks (SPEC §cpu) and the timed loops
 * whose instruction counts conform.c models (SPEC §frames, section "budget").
 *
 * Check functions: void cpu_xxx(uint32_t res[2]) runs its checks and stores
 * res[0] = failed checks, res[1] = the number of the first failed check
 * (checks are numbered from 1 in the order they appear in this file, per
 * function). Registers: s0 failures, s1 first failure, s2 check number;
 * t5 and t6 belong to the check macros.
 *
 * Built without relaxation, so every instruction written here is the one
 * that runs (the timed loops depend on it). */

    .option norelax

/* ---------------------------------------------------------------- harness */

.macro CHECKPOINT
    addi s2, s2, 1
.endm

/* The current check fails. */
.macro FAILNOW
    jal t5, cfail
.endm

/* Check register \r == constant \v. */
.macro CHK r, v
    CHECKPOINT
    li t6, \v
    beq \r, t6, .Lok\@
    FAILNOW
.Lok\@:
.endm

/* Check register \r == register \q. */
.macro CHKR r, q
    CHECKPOINT
    beq \r, \q, .Lok\@
    FAILNOW
.Lok\@:
.endm

/* \r = absolute address of \sym (ROM and RAM are at fixed addresses). */
.macro ABS r, sym
    lui \r, %hi(\sym)
    addi \r, \r, %lo(\sym)
.endm

/* Branch \op \a, \b must be taken. */
.macro BT op, a, b
    CHECKPOINT
    \op \a, \b, .Lt\@
    FAILNOW
.Lt\@:
.endm

/* Branch \op \a, \b must not be taken. */
.macro BN op, a, b
    CHECKPOINT
    \op \a, \b, .Lbad\@
    j .Lok\@
.Lbad\@:
    FAILNOW
.Lok\@:
.endm

/* Register-register operation on constants: \op t0, a0, a1 == \want. */
.macro RR op, x, y, want
    li a0, \x
    li a1, \y
    \op t0, a0, a1
    CHK t0, \want
.endm

/* Register-immediate operation: \op t0, a0, \imm == \want. */
.macro RI op, x, imm, want
    li a0, \x
    \op t0, a0, \imm
    CHK t0, \want
.endm

.macro PROLOGUE
    addi sp, sp, -64
    sw ra, 60(sp)
    sw s0, 56(sp)
    sw s1, 52(sp)
    sw s2, 48(sp)
    sw s3, 44(sp)
    sw s4, 40(sp)
    sw s5, 36(sp)
    sw s6, 32(sp)
    sw s7, 28(sp)
    sw s8, 24(sp)
    sw s9, 20(sp)
    sw s10, 16(sp)
    sw s11, 12(sp)
    sw a0, 0(sp)
    li s0, 0
    li s1, 0
    li s2, 0
.endm

.macro EPILOGUE
    lw a0, 0(sp)
    sw s0, 0(a0)
    sw s1, 4(a0)
    lw ra, 60(sp)
    lw s0, 56(sp)
    lw s1, 52(sp)
    lw s2, 48(sp)
    lw s3, 44(sp)
    lw s4, 40(sp)
    lw s5, 36(sp)
    lw s6, 32(sp)
    lw s7, 28(sp)
    lw s8, 24(sp)
    lw s9, 20(sp)
    lw s10, 16(sp)
    lw s11, 12(sp)
    addi sp, sp, 64
    ret
.endm

    .text
/* Counts a failed check (number s2); returns to t5. */
cfail:
    addi s0, s0, 1
    bnez s1, 1f
    mv s1, s2
1:  jr t5

/* ------------------------------------------------- RV32I, 32-bit encodings */

    .globl cpu_rv32i
cpu_rv32i:
    PROLOGUE
    .option push
    .option norvc

    /* LUI */
    lui t0, 0x12345
    CHK t0, 0x12345000
    lui t0, 0xFFFFF
    CHK t0, 0xFFFFF000

    /* AUIPC: pc + imm << 12, forwards and backwards */
.Lauipc1:
    auipc t0, 1
    ABS t1, .Lauipc1 + 0x1000
    CHKR t0, t1
.Lauipc2:
    auipc t0, 0xFFFFF
    ABS t1, .Lauipc2 - 0x1000
    CHKR t0, t1
.Lauipc3:
    auipc t0, 0
    ABS t1, .Lauipc3
    CHKR t0, t1

    /* JAL: links the next instruction, forwards and backwards */
    CHECKPOINT
    jal t0, 2f
1:  FAILNOW
2:  ABS t1, 1b
    CHKR t0, t1
    j 4f
3:  jal t2, 5f              /* backwards target */
4:  CHECKPOINT
    jal t0, 3b
    FAILNOW
5:  ABS t1, 4b
    CHKR t2, t1

    /* JALR: bit 0 of the target cleared, negative offset, rd = rs1 */
    CHECKPOINT
    ABS t1, 2f + 1
    jalr t0, 0(t1)
1:  FAILNOW
2:  ABS t2, 1b
    CHKR t0, t2
    CHECKPOINT
    ABS t1, 2f + 8
    jalr t0, -8(t1)
1:  FAILNOW
2:  ABS t2, 1b
    CHKR t0, t2
    CHECKPOINT
    ABS t1, 2f
    jalr t1, 0(t1)          /* the target is taken before t1 is written */
1:  FAILNOW
2:  ABS t2, 1b
    CHKR t1, t2
    CHECKPOINT
    ABS t1, 2f - 2047
    jalr zero, 2047(t1)     /* no link */
    FAILNOW
2:  CHK zero, 0

    /* Branches, taken and not taken, signed and unsigned */
    li a0, -1
    li a1, 1
    li a2, 1
    li a3, 0x80000000
    BT beq, a1, a2
    BN beq, a0, a1
    BT bne, a0, a1
    BN bne, a1, a2
    BT blt, a0, a1
    BN blt, a1, a0
    BN blt, a1, a2
    BT blt, a3, a1
    BT bge, a1, a0
    BT bge, a1, a2
    BN bge, a0, a1
    BN bge, a3, a0
    BT bltu, a1, a0
    BN bltu, a0, a1
    BN bltu, a1, a2
    BN bltu, a3, a1
    BT bgeu, a0, a1
    BT bgeu, a1, a2
    BN bgeu, a1, a0
    BT bgeu, a3, a1
    BT bge, zero, zero
    BN bltu, zero, zero
    /* A backward branch: five times round */
    li t0, 5
    li t1, 0
1:  addi t1, t1, 3
    addi t0, t0, -1
    bnez t0, 1b
    CHK t1, 15

    /* Loads from ROM: sign and zero extension, negative offsets */
    ABS a0, rom_words
    lb t0, 0(a0)
    CHK t0, 0xFFFFFF80
    lbu t0, 0(a0)
    CHK t0, 0x80
    lb t0, 1(a0)
    CHK t0, 0xFFFFFFFF
    lbu t0, 1(a0)
    CHK t0, 0xFF
    lb t0, 2(a0)
    CHK t0, 1
    lb t0, 3(a0)
    CHK t0, 0xFFFFFF80
    lh t0, 0(a0)
    CHK t0, 0xFFFFFF80
    lhu t0, 0(a0)
    CHK t0, 0xFF80
    lh t0, 2(a0)
    CHK t0, 0xFFFF8001
    lhu t0, 2(a0)
    CHK t0, 0x8001
    lw t0, 0(a0)
    CHK t0, 0x8001FF80
    lb t0, 4(a0)
    CHK t0, 0x7F
    lh t0, 4(a0)
    CHK t0, 0x017F
    lh t0, 6(a0)
    CHK t0, 0x7FFF
    lw t0, 4(a0)
    CHK t0, 0x7FFF017F
    addi a1, a0, 8
    lw t0, -8(a1)
    CHK t0, 0x8001FF80
    lhu t0, -2(a1)
    CHK t0, 0x7FFF

    /* Stores to RAM write only their bytes; loads back from RAM */
    ABS a0, ram_words
    li t0, 0x11223344
    sw t0, 0(a0)
    sw t0, 4(a0)
    sw t0, 8(a0)
    li t1, 0xAABBCCDD
    sb t1, 1(a0)
    sh t1, 6(a0)
    addi a1, a0, 12
    sb t1, -1(a1)
    sh t1, -4(a1)
    lw t0, 0(a0)
    CHK t0, 0x1122DD44
    lw t0, 4(a0)
    CHK t0, 0xCCDD3344
    lw t0, 8(a0)
    CHK t0, 0xDD22CCDD
    lb t0, 1(a0)
    CHK t0, 0xFFFFFFDD
    lbu t0, 1(a0)
    CHK t0, 0xDD
    lh t0, 6(a0)
    CHK t0, 0xFFFFCCDD
    lhu t0, 6(a0)
    CHK t0, 0xCCDD
    sw zero, 4(a0)
    lw t0, 4(a0)
    CHK t0, 0

    /* ALU with immediates */
    RI addi, 5, -7, 0xFFFFFFFE
    RI addi, 0x7FFFFFFF, 1, 0x80000000
    RI addi, 0, -2048, 0xFFFFF800
    RI addi, 0, 2047, 0x7FF
    RI slti, 5, 6, 1
    RI slti, 5, 5, 0
    RI slti, -1, 0, 1
    RI slti, 5, -1, 0
    RI slti, 0x80000000, -2048, 1
    RI sltiu, 5, -1, 1
    RI sltiu, -1, 5, 0
    RI sltiu, 0, 1, 1
    RI sltiu, 1, 1, 0
    RI xori, 5, -1, 0xFFFFFFFA
    RI xori, 5, 3, 6
    RI ori, 5, 0x700, 0x705
    RI ori, 5, -2048, 0xFFFFF805
    RI andi, -1, 0x7FF, 0x7FF
    RI andi, -1, -2048, 0xFFFFF800
    RI andi, 0x12345678, 0, 0
    RI slli, 0x80000001, 0, 0x80000001
    RI slli, 0x80000001, 1, 2
    RI slli, 0x80000001, 31, 0x80000000
    RI slli, 3, 31, 0x80000000
    RI srli, 0x80000001, 0, 0x80000001
    RI srli, 0x80000001, 1, 0x40000000
    RI srli, 0x80000001, 31, 1
    RI srai, 0x80000001, 0, 0x80000001
    RI srai, 0x80000001, 1, 0xC0000000
    RI srai, 0x80000001, 31, 0xFFFFFFFF
    RI srai, 0x7FFFFFFF, 31, 0
    RI srai, 0x7FFFFFFF, 30, 1

    /* Register-register ALU; shifts use the low 5 bits of rs2 */
    RR add, 0x7FFFFFFF, 5, 0x80000004
    RR add, -1, 1, 0
    RR sub, 5, 0x7FFFFFFF, 0x80000006
    RR sub, 0, 5, 0xFFFFFFFB
    RR sub, 0x80000000, 1, 0x7FFFFFFF
    RR sll, 5, 33, 10
    RR sll, 0x80000001, 31, 0x80000000
    RR sll, 0x80000001, 0, 0x80000001
    RR sll, 1, 0xFFFFFFE0, 1
    RR slt, -1, 5, 1
    RR slt, 5, -1, 0
    RR slt, 5, 5, 0
    RR slt, 0x80000000, 0x7FFFFFFF, 1
    RR sltu, -1, 5, 0
    RR sltu, 5, -1, 1
    RR sltu, 5, 5, 0
    RR xor, 5, -1, 0xFFFFFFFA
    RR xor, 0xF0F0F0F0, 0x0FF00FF0, 0xFF00FF00
    RR srl, 0x80000001, 33, 0x40000000
    RR srl, 0x80000001, 31, 1
    RR srl, 0x80000001, 0, 0x80000001
    RR sra, 0x80000001, 33, 0xC0000000
    RR sra, 0x80000001, 31, 0xFFFFFFFF
    RR sra, 0x80000001, 32, 0x80000001
    RR sra, 0x40000000, 30, 1
    RR or, 0xF0F0F0F0, 0x0FF00FF0, 0xFFF0FFF0
    RR and, 0xF0F0F0F0, 0x0FF00FF0, 0x00F000F0
    li a0, 5
    sltu t0, zero, a0       /* snez */
    CHK t0, 1
    li a0, 0
    sltu t0, zero, a0
    CHK t0, 0

    /* Writes to x0 are discarded; x0 reads as zero afterwards */
    li a0, 5
    addi zero, zero, 5
    CHK zero, 0
    lui zero, 1
    CHK zero, 0
    auipc zero, 1
    CHK zero, 0
    add zero, a0, a0
    CHK zero, 0
    slli zero, a0, 3        /* a HINT encoding */
    CHK zero, 0
    ABS a1, rom_words
    lw zero, 0(a1)
    CHK zero, 0
    mul zero, a0, a0
    CHK zero, 0
    jal zero, 1f
    FAILNOW
1:  add t0, zero, zero
    CHK t0, 0

    /* FENCE in its forms does nothing */
    li t0, 7
    fence
    fence rw, rw
    .word 0x8330000F        /* fence.tso */
    .word 0x0100000F        /* pause */
    .word 0x0000000F        /* fence with no bits: a hint */
    CHK t0, 7

    /* ecall changes only a0 (and a1 where the call has a second result) */
    li a1, 0x1111
    li a2, 0x2222
    li a3, 0x3333
    li a4, 0x4444
    li a5, 0x5555
    li a6, 0x6666
    li t0, 0x7777
    li t1, 0x8888
    li t2, 0x9999
    li t3, 0xAAAA
    li t4, 0xBBBB
    li s3, 0xCCCC
    li s11, 0xDDDD
    li a7, 0x102            /* frame_count */
    ecall
    CHK a1, 0x1111
    CHK a2, 0x2222
    CHK a3, 0x3333
    CHK a4, 0x4444
    CHK a5, 0x5555
    CHK a6, 0x6666
    CHK a7, 0x102
    CHK t0, 0x7777
    CHK t1, 0x8888
    CHK t2, 0x9999
    CHK t3, 0xAAAA
    CHK t4, 0xBBBB
    CHK s3, 0xCCCC
    CHK s11, 0xDDDD
    li a7, 0x5FF            /* an unknown call: FFFF_FFFFh, a1 unchanged */
    ecall
    CHK a0, 0xFFFFFFFF
    CHK a1, 0x1111
    CHK a2, 0x2222
    CHK a7, 0x5FF

    .option pop
    EPILOGUE

/* ------------------------------------------------------------- M extension */

    .globl cpu_m
cpu_m:
    PROLOGUE
    .option push
    .option norvc
    RR mul, 7, -3, 0xFFFFFFEB
    RR mul, 0x80000000, 0xFFFFFFFF, 0x80000000
    RR mul, 0xFFFFFFFF, 0xFFFFFFFF, 1
    RR mul, 0x80000000, 0x80000000, 0
    RR mul, 0x12345678, 0x9ABCDEF0, 0x242D2080
    RR mul, 0x7FFFFFFF, 0xFFFFFFFF, 0x80000001
    RR mulh, 7, -3, 0xFFFFFFFF
    RR mulh, 0x80000000, 0xFFFFFFFF, 0
    RR mulh, 0xFFFFFFFF, 0xFFFFFFFF, 0
    RR mulh, 0x80000000, 0x80000000, 0x40000000
    RR mulh, 0x12345678, 0x9ABCDEF0, 0xF8CC93D6
    RR mulh, 0x80000000, 2, 0xFFFFFFFF
    RR mulh, 0x7FFFFFFF, 0x7FFFFFFF, 0x3FFFFFFF
    RR mulhsu, 7, -3, 6
    RR mulhsu, 0x80000000, 0xFFFFFFFF, 0x80000000
    RR mulhsu, 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF
    RR mulhsu, 0x80000000, 0x80000000, 0xC0000000
    RR mulhsu, 0x7FFFFFFF, 0xFFFFFFFF, 0x7FFFFFFE
    RR mulhsu, 0xFFFFFFFF, 1, 0xFFFFFFFF
    RR mulhsu, 0, 0xFFFFFFFF, 0
    RR mulhu, 7, -3, 6
    RR mulhu, 0x80000000, 0xFFFFFFFF, 0x7FFFFFFF
    RR mulhu, 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFE
    RR mulhu, 0x80000000, 2, 1
    RR mulhu, 0x12345678, 0x9ABCDEF0, 0x0B00EA4E
    RR mulhu, 0xFFFFFFFF, 1, 0
    RR div, -7, 2, 0xFFFFFFFD
    RR div, 7, -2, 0xFFFFFFFD
    RR div, -7, -2, 3
    RR div, 7, 0, 0xFFFFFFFF
    RR div, -7, 0, 0xFFFFFFFF
    RR div, 0x80000000, -1, 0x80000000
    RR div, 0x80000000, 1, 0x80000000
    RR div, 0, 5, 0
    RR divu, 0xFFFFFFFF, 2, 0x7FFFFFFF
    RR divu, 7, 0, 0xFFFFFFFF
    RR divu, 0x80000000, 0xFFFFFFFF, 0
    RR divu, 0xFFFFFFFF, 0xFFFFFFFF, 1
    RR rem, -7, 2, 0xFFFFFFFF
    RR rem, 7, -2, 1
    RR rem, -7, -2, 0xFFFFFFFF
    RR rem, 7, 0, 7
    RR rem, -7, 0, 0xFFFFFFF9
    RR rem, 0x80000000, -1, 0
    RR rem, 0x80000000, 3, 0xFFFFFFFE
    RR remu, 0xFFFFFFFF, 10, 5
    RR remu, 7, 0, 7
    RR remu, 7, 0xFFFFFFFF, 7
    RR remu, 0x80000000, 3, 2
    /* Destination equal to the sources */
    li a0, 12
    mul a0, a0, a0
    CHK a0, 144
    li a0, 144
    li a1, 12
    divu a0, a0, a1
    CHK a0, 12
    li a1, 5
    rem a1, a0, a1
    CHK a1, 2
    .option pop
    EPILOGUE

/* ------------------------------------------------------ compressed forms */

    .globl cpu_c
cpu_c:
    PROLOGUE
    /* C.LI, C.ADDI */
    c.li a0, -32
    CHK a0, 0xFFFFFFE0
    c.li a1, 31
    CHK a1, 31
    c.addi a0, 5
    CHK a0, 0xFFFFFFE5
    c.addi a1, -32
    CHK a1, 0xFFFFFFFF
    c.li t0, 0
    c.addi t0, 31
    CHK t0, 31

    /* C.LUI: sign-extended 6-bit immediate in bits 17:12 */
    c.lui a2, 1
    CHK a2, 0x1000
    c.lui a3, 1048575
    CHK a3, 0xFFFFF000
    c.lui a4, 31
    CHK a4, 0x1F000
    c.lui a5, 1048544
    CHK a5, 0xFFFE0000

    /* C.ADDI16SP and C.ADDI4SPN */
    mv s3, sp
    c.addi16sp sp, -64
    addi t0, s3, -64
    CHKR sp, t0
    c.addi16sp sp, 496
    addi t0, s3, 432
    CHKR sp, t0
    c.addi16sp sp, -496
    c.addi4spn a0, sp, 4
    addi t0, sp, 4
    CHKR a0, t0
    c.addi4spn a1, sp, 1020
    addi t0, sp, 1020
    CHKR a1, t0
    c.addi16sp sp, 64
    CHKR sp, s3

    /* C.SWSP, C.LWSP (on the stack frame below sp) */
    c.addi16sp sp, -128
    li t0, 0x5A5A1234
    c.swsp t0, 124(sp)
    c.swsp zero, 0(sp)
    lw t1, 124(sp)
    CHK t1, 0x5A5A1234
    c.lwsp t2, 124(sp)
    CHK t2, 0x5A5A1234
    c.lwsp t2, 0(sp)
    CHK t2, 0
    c.addi16sp sp, 128

    /* C.SW, C.LW */
    ABS a0, ram_words
    li a1, 0x87654321
    c.sw a1, 4(a0)
    c.sw a1, 124(a0)
    lw t0, 4(a0)
    CHK t0, 0x87654321
    c.lw a2, 124(a0)
    CHK a2, 0x87654321
    c.li a4, 0
    c.sw a4, 0(a0)
    c.lw a3, 0(a0)
    CHK a3, 0

    /* C.SRLI, C.SRAI, C.ANDI, C.SLLI */
    li a0, 0x80000010
    c.srli a0, 4
    CHK a0, 0x08000001
    li a0, 0x80000010
    c.srli a0, 31
    CHK a0, 1
    li a0, 0x80000010
    c.srai a0, 4
    CHK a0, 0xF8000001
    li a0, 0x80000010
    c.srai a0, 31
    CHK a0, 0xFFFFFFFF
    li a0, 0x12345678
    c.andi a0, -32
    CHK a0, 0x12345660
    li a0, 0x12345678
    c.andi a0, 31
    CHK a0, 0x18
    li a0, 0x12345678
    c.andi a0, 0
    CHK a0, 0
    li t0, 3
    c.slli t0, 31
    CHK t0, 0x80000000
    li t0, 3
    c.slli t0, 1
    CHK t0, 6

    /* C.SUB, C.XOR, C.OR, C.AND */
    li a0, 5
    li a1, 7
    c.sub a0, a1
    CHK a0, 0xFFFFFFFE
    li a0, 0xF0F0F0F0
    li a1, 0x0FF00FF0
    c.xor a0, a1
    CHK a0, 0xFF00FF00
    li a0, 0xF0F0F0F0
    c.or a0, a1
    CHK a0, 0xFFF0FFF0
    li a0, 0xF0F0F0F0
    c.and a0, a1
    CHK a0, 0x00F000F0

    /* C.MV, C.ADD */
    li a0, 0x1234
    c.mv t3, a0
    CHK t3, 0x1234
    li t4, -0x34
    c.add t3, t4
    CHK t3, 0x1200

    /* C.J, C.BEQZ, C.BNEZ both ways */
    CHECKPOINT
    c.j 1f
    FAILNOW
1:  li a0, 0
    li a1, 1
    CHECKPOINT
    c.beqz a0, 1f
    FAILNOW
1:  CHECKPOINT
    c.beqz a1, 2f
    j 3f
2:  FAILNOW
3:  CHECKPOINT
    c.bnez a1, 1f
    FAILNOW
1:  CHECKPOINT
    c.bnez a0, 2f
    j 3f
2:  FAILNOW
3:  li a2, 4                /* backwards: four times round */
    li a3, 0
1:  c.addi a3, 2
    c.addi a2, -1
    c.bnez a2, 1b
    CHK a3, 8
    CHECKPOINT
    c.j 2f
1:  c.j 3f                  /* a backward C.J */
2:  c.j 1b
    FAILNOW
3:

    /* C.JAL links pc + 2 */
    CHECKPOINT
    c.jal 2f
1:  FAILNOW
2:  ABS t0, 1b
    CHKR ra, t0

    /* C.JALR links pc + 2; C.JR jumps without linking */
    CHECKPOINT
    ABS t1, 2f
    c.jalr t1
1:  FAILNOW
2:  ABS t0, 1b
    CHKR ra, t0
    CHECKPOINT
    ABS t1, 2f
    mv t2, ra
    c.jr t1
    FAILNOW
2:  CHKR ra, t2
    /* C.JALR through ra itself: the target is read before ra is written */
    CHECKPOINT
    ABS ra, 2f
    c.jalr ra
1:  FAILNOW
2:  ABS t0, 1b
    CHKR ra, t0

    /* HINT encodings run as the instructions they are (SPEC §cpu) */
    li a0, 0x55
    .hword 0x0015           /* c.addi x0, 5 (c.nop with an immediate) */
    .hword 0x4015           /* c.li x0, 5 */
    .hword 0x6005           /* c.lui x0, 1 */
    .hword 0x0006           /* c.slli x0, 1 */
    .hword 0x802A           /* c.mv x0, a0 */
    .hword 0x902A           /* c.add x0, a0 */
    .hword 0x8101           /* c.srli a0, 0 */
    .hword 0x8501           /* c.srai a0, 0 */
    .hword 0x0001           /* c.nop */
    CHK zero, 0
    CHK a0, 0x55

    EPILOGUE

/* ------------------------------------------------------------------- data */

    .section .rodata
    .balign 4
rom_words:
    .word 0x8001FF80, 0x7FFF017F

    .section .bss
    .balign 4
ram_words:
    .space 128

/* ------------------------------------------------- code copied into RAM */
/* These run from RAM after conform.c copies them there; they use only
 * pc-relative branches. 32-bit forms where conform.c patches them. */

    .text
    .balign 4
    .globl ram_add, ram_add_end
/* a0 + 0x123 + 1; conform.c rewrites both immediates. */
ram_add:
    .option push
    .option norvc
    addi a0, a0, 0x123
    .option pop
    c.addi a0, 1
    c.jr ra
ram_add_end:

    .balign 4
    .globl ram_smc, ram_smc_end
/* a0 = the address this code runs at, a1 = an instruction word. Stores a1
 * over the instruction at +8 and runs it: returns 2 if a1 is
 * `addi a0, zero, 2`, as the store takes effect at once. */
ram_smc:
    .option push
    .option norvc
    sw a1, 8(a0)
    addi zero, zero, 0
    addi a0, zero, 1
    jalr zero, 0(ra)
    .option pop
ram_smc_end:

    .balign 4
    .globl ram_frame, ram_frame_end
/* frame_count, called from RAM. */
ram_frame:
    li a7, 0x102
    ecall
    ret
ram_frame_end:

    .balign 4
    .globl ram_last, ram_last_end
/* Six bytes copied to the very end of RAM: a 32-bit and a 16-bit
 * instruction, the last ending at the last byte. */
ram_last:
    .option push
    .option norvc
    addi a0, a0, 0x77
    .option pop
    c.jr ra
ram_last_end:

/* ------------------------------------------------------------ timed loops */
/* The instruction counts of these routines are modelled by conform.c
 * (model_*); change them together. Each starts by calling wait_frame twice,
 * so the frame after the second starts with a full budget whatever came
 * before. All are position independent: count_frames also runs from RAM. */

    .balign 4
    .globl count_frames, count_frames_end
/* void count_frames(uint32_t *out, uint32_t n, uint32_t call, uint32_t arg)
 * After the waits makes one call (a7 = call, a0 = arg), then for n frames
 * stores in out[] how many frame_count calls returned each frame. */
count_frames:
    mv t1, a0
    mv t2, a1
    mv t4, a2
    mv t5, a3
    li a7, 0x100
    ecall                   /* wait_frame */
    ecall                   /* wait_frame: frame F starts, full budget */
    mv a7, t4
    mv a0, t5
    ecall                   /* the extra call */
    li a7, 0x102
    ecall                   /* frame_count: F */
    mv t3, a0
    li t0, 1
1:  addi t0, t0, 1
    ecall
    beq a0, t3, 1b
    addi t0, t0, -1         /* the last call returned the next frame */
    sw t0, 0(t1)
    addi t1, t1, 4
    mv t3, a0
    li t0, 1
    addi t2, t2, -1
    bnez t2, 1b
    ret
count_frames_end:

    .balign 4
    .globl count_scroll
/* void count_scroll(uint32_t *out, uint32_t n)
 * As count_frames, with a terminal scroll (command 02h, charge 150) before
 * each frame_count call, so a frame can end up to 154 units below zero. */
count_scroll:
    mv t1, a0
    mv t2, a1
    li a7, 0x100
    ecall
    ecall
    li a7, 0x102
    ecall
    mv t3, a0
    li t0, 1
1:  addi t0, t0, 1
    li a7, 0x201
    li a0, 2
    ecall                   /* scroll up */
    li a7, 0x102
    ecall
    beq a0, t3, 1b
    addi t0, t0, -1
    sw t0, 0(t1)
    addi t1, t1, 4
    mv t3, a0
    li t0, 1
    addi t2, t2, -1
    bnez t2, 1b
    ret

    .balign 4
    .globl count_write
/* void count_write(uint32_t *out, uint32_t n, const void *buf, uint32_t len)
 * As count_scroll, with a terminal write of buf (len bytes) before each
 * frame_count call: a write takes its whole buffer, so a frame can end up
 * to 4 + 2 x len + 4 units below zero. */
count_write:
    mv t1, a0
    mv t2, a1
    mv t4, a2
    mv t5, a3
    li a7, 0x100
    ecall
    ecall
    li a7, 0x102
    ecall
    mv t3, a0
    li t0, 1
1:  addi t0, t0, 1
    li a7, 0x200
    mv a0, t4
    mv a1, t5
    ecall                   /* write */
    li a7, 0x102
    ecall
    beq a0, t3, 1b
    addi t0, t0, -1
    sw t0, 0(t1)
    addi t1, t1, 4
    mv t3, a0
    li t0, 1
    addi t2, t2, -1
    bnez t2, 1b
    ret
