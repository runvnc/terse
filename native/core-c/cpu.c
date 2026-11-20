/* RV32IMC, user mode, SPEC §cpu: everything not listed faults. */
#include "core.h"

#define SX(v, bits) ((uint32_t)((int32_t)((uint32_t)(v) << (32 - (bits))) >> (32 - (bits))))

void fault(M *m, int kind, uint32_t addr)
{
    m->stopped = FAULTED;
    m->fault_kind = kind;
    m->fault_pc = m->pc;
    m->fault_addr = addr;
}

static int illegal(M *m, uint32_t enc) { fault(m, F_ILLEGAL, m->pc); m->fault_enc = enc; return 0; }

/* Memory: misalignment is checked before the address range (a guess,
   SPEC-FINDINGS.md). */
static uint8_t *where(M *m, uint32_t a, uint32_t n, int write)
{
    if (in_ram(m, a, n)) return m->ram + (a - RAM_BASE);
    if (!write && in_rom(m, a, n)) return (uint8_t *)m->rom + (a - ROM_BASE);
    return 0;
}

static int load(M *m, uint32_t a, int n, int sign, uint32_t *v)
{
    if (a & (n - 1)) { fault(m, F_MISALIGNED, a); return 0; }
    uint8_t *p = where(m, a, n, 0);
    if (!p) { fault(m, F_LOAD, a); return 0; }
    uint32_t x = n == 1 ? p[0] : n == 2 ? rd16(p) : rd32(p);
    *v = sign && n < 4 ? SX(x, n * 8) : x;
    return 1;
}

static int store(M *m, uint32_t a, int n, uint32_t v)
{
    if (a & (n - 1)) { fault(m, F_MISALIGNED, a); return 0; }
    uint8_t *p = where(m, a, n, 1);
    if (!p) { fault(m, F_STORE, a); return 0; }
    if (n == 1) p[0] = v; else if (n == 2) wr16(p, v); else wr32(p, v);
    return 1;
}

static uint32_t mulh(int64_t a, int64_t b) { return (uint32_t)((uint64_t)(a * b) >> 32); }

static uint32_t alu_m(int f3, uint32_t a, uint32_t b)
{
    int32_t sa = (int32_t)a, sb = (int32_t)b;
    switch (f3) {
    case 0: return a * b;
    case 1: return mulh(sa, sb);
    case 2: return (uint32_t)((uint64_t)((int64_t)sa * (int64_t)b) >> 32);
    case 3: return (uint32_t)(((uint64_t)a * b) >> 32);
    case 4: return b == 0 ? 0xFFFFFFFFu : (sa == INT32_MIN && sb == -1) ? a : (uint32_t)(sa / sb);
    case 5: return b == 0 ? 0xFFFFFFFFu : a / b;
    case 6: return b == 0 ? a : (sa == INT32_MIN && sb == -1) ? 0 : (uint32_t)(sa % sb);
    default: return b == 0 ? a : a % b;
    }
}

/* Executes one 32-bit instruction; sets *npc. 0 on fault, 2 for ecall. */
static int exec32(M *m, uint32_t i, uint32_t *npc)
{
    uint32_t *x = m->x, pc = m->pc, rd = i >> 7 & 31, f3 = i >> 12 & 7;
    uint32_t a = x[i >> 15 & 31], b = x[i >> 20 & 31], f7 = i >> 25, v = 0;
    uint32_t immi = SX(i >> 20, 12), imms = SX((i >> 20 & ~31u) | rd, 12);
    *npc = pc + 4;
    switch (i & 0x7F) {
    case 0x37: v = i & 0xFFFFF000u; break;
    case 0x17: v = pc + (i & 0xFFFFF000u); break;
    case 0x6F:
        v = pc + 4;
        *npc = pc + SX((i >> 31) << 20 | (i >> 12 & 0xFF) << 12 | (i >> 20 & 1) << 11 | (i >> 21 & 0x3FF) << 1, 21);
        break;
    case 0x67:
        if (f3) return illegal(m, i);
        v = pc + 4; *npc = (a + immi) & ~1u; break;
    case 0x63: {
        int t;
        switch (f3) {
        case 0: t = a == b; break;
        case 1: t = a != b; break;
        case 4: t = (int32_t)a < (int32_t)b; break;
        case 5: t = (int32_t)a >= (int32_t)b; break;
        case 6: t = a < b; break;
        case 7: t = a >= b; break;
        default: return illegal(m, i);
        }
        if (t) *npc = pc + SX((i >> 31) << 12 | (i >> 7 & 1) << 11 | (i >> 25 & 0x3F) << 5 | (i >> 8 & 0xF) << 1, 13);
        return 1;
    }
    case 0x03:
        if (f3 == 3 || f3 > 5) return illegal(m, i);
        if (!load(m, a + immi, 1 << (f3 & 3), !(f3 & 4), &v)) return 0;
        break;
    case 0x23:
        if (f3 > 2) return illegal(m, i);
        return store(m, a + imms, 1 << f3, b);
    case 0x13: {
        uint32_t sh = i >> 20 & 31;
        switch (f3) {
        case 0: v = a + immi; break;
        case 1: if (f7) return illegal(m, i); v = a << sh; break;
        case 2: v = (int32_t)a < (int32_t)immi; break;
        case 3: v = a < immi; break;
        case 4: v = a ^ immi; break;
        case 5:
            if (f7 == 0) v = a >> sh;
            else if (f7 == 0x20) v = (uint32_t)((int32_t)a >> sh);
            else return illegal(m, i);
            break;
        case 6: v = a | immi; break;
        default: v = a & immi; break;
        }
        break;
    }
    case 0x33:
        if (f7 == 1) { v = alu_m(f3, a, b); break; }
        if (f7 == 0x20 && f3 != 0 && f3 != 5) return illegal(m, i);
        if (f7 != 0 && f7 != 0x20) return illegal(m, i);
        switch (f3) {
        case 0: v = f7 ? a - b : a + b; break;
        case 1: v = a << (b & 31); break;
        case 2: v = (int32_t)a < (int32_t)b; break;
        case 3: v = a < b; break;
        case 4: v = a ^ b; break;
        case 5: v = f7 ? (uint32_t)((int32_t)a >> (b & 31)) : a >> (b & 31); break;
        case 6: v = a | b; break;
        default: v = a & b; break;
        }
        break;
    case 0x0F:
        if (f3) return illegal(m, i);
        return 1; /* FENCE in every form: nothing */
    case 0x73:
        if (i == 0x00000073) return 2;
        if (i == 0x00100073) { fault(m, F_BREAK, pc); return 0; }
        return illegal(m, i);
    default: return illegal(m, i);
    }
    if (rd) x[rd] = v;
    return 1;
}

static int exec16(M *m, uint32_t i, uint32_t *npc)
{
    uint32_t *x = m->x, pc = m->pc, f3 = i >> 13, v;
    uint32_t r7 = i >> 7 & 31, r2 = i >> 2 & 31, rs1p = 8 + (i >> 7 & 7), rs2p = 8 + (i >> 2 & 7);
    uint32_t imm6 = SX((i >> 7 & 0x20) | (i >> 2 & 31), 6);
    *npc = pc + 2;
    switch (i & 3) {
    case 0:
        switch (f3) {
        case 0: { /* C.ADDI4SPN */
            uint32_t u = (i >> 7 & 0x30) | (i >> 1 & 0x3C0) | (i >> 4 & 4) | (i >> 2 & 8);
            if (!u) return illegal(m, i);
            x[rs2p] = x[2] + u; return 1;
        }
        case 2: case 6: {
            uint32_t off = (i >> 7 & 0x38) | (i >> 4 & 4) | (i << 1 & 0x40);
            if (f3 == 6) return store(m, x[rs1p] + off, 4, x[rs2p]);
            if (!load(m, x[rs1p] + off, 4, 0, &v)) return 0;
            x[rs2p] = v; return 1;
        }
        default: return illegal(m, i); /* FP loads and stores, reserved */
        }
    case 1:
        switch (f3) {
        case 0: if (r7) x[r7] += imm6; return 1;                /* C.ADDI, C.NOP */
        case 1: case 5: {                                         /* C.JAL, C.J */
            uint32_t o = SX((i >> 1 & 0x800) | (i >> 7 & 0x10) | (i >> 1 & 0x300) | (i << 2 & 0x400) |
                            (i >> 1 & 0x40) | (i << 1 & 0x80) | (i >> 2 & 0xE) | (i << 3 & 0x20), 12);
            if (f3 == 1) x[1] = pc + 2;
            *npc = pc + o; return 1;
        }
        case 2: if (r7) x[r7] = imm6; return 1;                 /* C.LI */
        case 3:
            if (r7 == 2) {                                        /* C.ADDI16SP */
                uint32_t o = SX((i >> 3 & 0x200) | (i >> 2 & 0x10) | (i << 1 & 0x40) | (i << 4 & 0x180) | (i << 3 & 0x20), 10);
                if (!o) return illegal(m, i);
                x[2] += o; return 1;
            }
            if (!imm6) return illegal(m, i);                      /* C.LUI */
            if (r7) x[r7] = imm6 << 12;
            return 1;
        case 4: {
            uint32_t sh = r2 | (i >> 7 & 0x20);
            switch (i >> 10 & 3) {
            case 0: if (sh & 32) return illegal(m, i); x[rs1p] >>= sh; return 1;
            case 1: if (sh & 32) return illegal(m, i); x[rs1p] = (uint32_t)((int32_t)x[rs1p] >> sh); return 1;
            case 2: x[rs1p] &= imm6; return 1;
            default:
                if (i & 0x1000) return illegal(m, i);             /* SUBW, ADDW, reserved */
                switch (i >> 5 & 3) {
                case 0: x[rs1p] -= x[rs2p]; break;
                case 1: x[rs1p] ^= x[rs2p]; break;
                case 2: x[rs1p] |= x[rs2p]; break;
                default: x[rs1p] &= x[rs2p]; break;
                }
                return 1;
            }
        }
        default: {                                                /* C.BEQZ, C.BNEZ */
            uint32_t o = SX((i >> 4 & 0x100) | (i >> 7 & 0x18) | (i << 1 & 0xC0) | (i >> 2 & 6) | (i << 3 & 0x20), 9);
            if ((x[rs1p] == 0) == (f3 == 6)) *npc = pc + o;
            return 1;
        }
        }
    default:
        switch (f3) {
        case 0:                                                   /* C.SLLI */
            if (i & 0x1000) return illegal(m, i);
            if (r7) x[r7] <<= r2;
            return 1;
        case 2:                                                   /* C.LWSP */
            if (!r7) return illegal(m, i);
            if (!load(m, x[2] + ((i >> 7 & 0x20) | (i >> 2 & 0x1C) | (i << 4 & 0xC0)), 4, 0, &v)) return 0;
            x[r7] = v; return 1;
        case 4:
            if (!(i & 0x1000)) {
                if (!r2) {                                        /* C.JR */
                    if (!r7) return illegal(m, i);
                    *npc = x[r7] & ~1u; return 1;
                }
                if (r7) x[r7] = x[r2];                            /* C.MV */
                return 1;
            }
            if (!r7 && !r2) { fault(m, F_BREAK, pc); return 0; } /* C.EBREAK */
            if (!r2) { v = x[r7]; x[1] = pc + 2; *npc = v & ~1u; return 1; } /* C.JALR */
            if (r7) x[r7] += x[r2];                               /* C.ADD */
            return 1;
        case 6:                                                   /* C.SWSP */
            return store(m, x[2] + ((i >> 7 & 0x3C) | (i >> 1 & 0xC0)), 4, x[r2]);
        default: return illegal(m, i);                            /* FP, reserved */
        }
    }
}

void cpu_run(M *m)
{
    m->yield = 0;
    while (m->budget > 0 && m->stopped == RUNNING && !m->yield) {
        uint32_t pc = m->pc, ins, npc;
        int cost;
        if (in_rom(m, pc, 2)) cost = 1;
        else if (in_ram(m, pc, 2)) cost = 4;
        else { fault(m, F_FETCH, pc); break; }
        uint8_t *p = cost == 1 ? (uint8_t *)m->rom + (pc - ROM_BASE) : m->ram + (pc - RAM_BASE);
        ins = rd16(p);
        int r;
        if ((ins & 3) == 3) {
            if (!(cost == 1 ? in_rom(m, pc + 2, 2) : in_ram(m, pc + 2, 2))) { fault(m, F_FETCH, pc + 2); break; }
            ins |= rd16(p + 2) << 16;
            r = (ins & 0x1F) == 0x1F ? illegal(m, ins) : exec32(m, ins, &npc);
        } else
            r = exec16(m, ins, &npc);
        if (!r) break;
        m->budget -= cost;
        m->pc = npc;
        if (r == 2) ecall(m);
    }
}
