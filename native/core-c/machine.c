/* The machine: image (SPEC §image), memory map, System device (§system),
   the event queue (§events) and the state serialization (STATE.md 1). */
#include <stdio.h>
#include <stdlib.h>
#include "core.h"

int in_rom(M *m, uint32_t a, uint32_t n)
{
    return a >= ROM_BASE && (uint64_t)a + n <= (uint64_t)ROM_BASE + m->rom_len;
}
int in_ram(M *m, uint32_t a, uint32_t n)
{
    return a >= RAM_BASE && (uint64_t)a + n <= (uint64_t)RAM_BASE + m->ram_len;
}
/* A buffer the device reads: wholly in ROM or wholly in RAM. */
static const uint8_t *readable(M *m, uint32_t a, uint32_t n)
{
    if (in_ram(m, a, n)) return m->ram + (a - RAM_BASE);
    if (in_rom(m, a, n)) return m->rom + (a - ROM_BASE);
    return 0;
}

int image_load(M *m, uint8_t *f, size_t len, char *err, size_t el)
{
#define BAD(...) do { snprintf(err, el, __VA_ARGS__); return 0; } while (0)
    if (len < 32 || memcmp(f, "TERS", 4)) BAD("not a Terse image");
    if (f[4] != 0 || f[5] != 1) BAD("format %d, profile %d: not implemented", f[4], f[5]);
    uint32_t rom_len = f[6] | f[7] << 8 | f[8] << 16, pages = f[9], n = f[10];
    if (rom_len < 1 || rom_len > 131072) BAD("rom_len %u out of range", rom_len);
    if (pages < 1 || pages > 128) BAD("ram_pages %u out of range", pages);
    if (n > 16) BAD("%u device entries", n);
    for (int i = 11; i < 16; i++) if (f[i]) BAD("reserved header bytes are not zero");
    if (len != 32 + 2 * n + rom_len) BAD("file length %zu, header says %u", len, 32 + 2 * n + rom_len);
    memset(m, 0, sizeof *m);
    m->file = f; m->file_len = len;
    blake3(f, len, m->id);
    m->ndev = n;
    int seen[256] = {0};
    for (uint32_t i = 0; i < n; i++) {
        int s = f[32 + 2 * i], v = f[33 + 2 * i];
        if (seen[s]++) BAD("device %d listed twice", s);
        m->dslot[i] = s; m->dver[i] = v & 0x7F; m->dopt[i] = v >> 7;
        if ((s == 1 || s == 8) && m->dopt[i]) BAD("device %d listed as optional", s);
        if (s == 1 && m->dver[i] != 0) BAD("needs device 1 version %d", m->dver[i]);
    }
    m->rom = f + 32 + 2 * n; m->rom_len = rom_len;
    m->ram_len = pages * 8192;
    m->ram = calloc(1, m->ram_len);
    m->has[1] = 1;
    m->pc = ROM_BASE;
    m->x[2] = RAM_BASE + m->ram_len;
    return 1;
}

void queue_event(M *m, const uint8_t ev[4])
{
    if (m->qn == QMAX) return;
    memcpy(m->q[(m->qhead + m->qn++) % QMAX], ev, 4);
}

/* A valid display name, SPEC §display-names. */
static int white(uint32_t c)
{
    return (c >= 9 && c <= 13) || c == 0x20 || c == 0x85 || c == 0xA0 || c == 0x1680 ||
           (c >= 0x2000 && c <= 0x200A) || c == 0x2028 || c == 0x2029 || c == 0x202F ||
           c == 0x205F || c == 0x3000;
}
static int display_name(const uint8_t *p, uint32_t n)
{
    uint32_t i = 0, c = 0, first = 1;
    while (i < n) {
        uint8_t b = p[i];
        int k = b < 0x80 ? 0 : b >= 0xC2 && b < 0xE0 ? 1 : b >= 0xE0 && b < 0xF0 ? 2 : b >= 0xF0 && b < 0xF5 ? 3 : -1;
        if (k < 0 || i + k >= n) return 0;
        c = k == 0 ? b : b & (0x3F >> k);
        for (int j = 1; j <= k; j++) {
            if ((p[i + j] & 0xC0) != 0x80) return 0;
            c = c << 6 | (p[i + j] & 0x3F);
        }
        if ((k == 2 && (c < 0x800 || (c >= 0xD800 && c < 0xE000))) || (k == 3 && (c < 0x10000 || c > 0x10FFFF)))
            return 0;
        if (c < 0x20 || (c >= 0x7F && c < 0xA0)) return 0;
        if (first && white(c)) return 0;
        first = 0;
        i += k + 1;
    }
    return n >= 1 && n <= 48 && !white(c);
}

/* System device, SPEC §system: returns the result, adds to the charge. */
static uint32_t sys(M *m, int fn, int64_t *ch)
{
    uint32_t *x = m->x, a0 = x[10], a1 = x[11];
    switch (fn) {
    case 0: m->yield = 1; return m->frame + 1;
    case 1:
        if (!in_ram(m, a0, 4)) return 0xFFFFFFFE;
        if (!m->qn) return 0;
        memcpy(m->ram + (a0 - RAM_BASE), m->q[m->qhead], 4);
        m->qhead = (m->qhead + 1) % QMAX; m->qn--;
        *ch += 1;
        return 1;
    case 2: return m->frame;
    case 3: return m->time;
    case 4:
        if (a1 > 256 || (a1 && !in_ram(m, a0, a1))) return 0xFFFFFFFE;
        if (a1) memset(m->ram + (a0 - RAM_BASE), 0, a1); /* no random bytes supplied */
        *ch += (a1 + 3) / 4;
        return a1;
    case 5: {
        if (!in_ram(m, a1, 8)) return 0xFFFFFFFE;
        static const uint8_t none[8];
        const uint8_t *s = a0 < 4 ? m->pads[a0] : none;
        memcpy(m->ram + (a1 - RAM_BASE), s, 8);
        *ch += 2;
        return s[0];
    }
    case 6: m->stopped = EXITED; m->exit_code = a0 & 0xFF; return 0; /* a0 = 0: SPEC-FINDINGS.md */
    case 7: { /* open: no shell, so 0 for a valid request */
        const uint8_t *p = readable(m, a0, 1);
        uint32_t n;
        if (!p) return 0xFFFFFFFE;
        if (p[0] == 1) n = 33;
        else if (p[0] == 2) {
            if (!(p = readable(m, a0, 34))) return 0xFFFFFFFE;
            n = 34 + p[33];
            if (!readable(m, a0, n) || !display_name(p + 34, p[33])) return 0xFFFFFFFE;
        } else return 0xFFFFFFFE;
        if (!readable(m, a0, n)) return 0xFFFFFFFE;
        *ch += (n + 3) / 4;
        return 0;
    }
    case 8: return in_ram(m, a0, 32) ? 0 : 0xFFFFFFFE;
    case 9: return a0 < 256 && m->has[a0] ? m->ver[a0] : 0xFFFFFFFF;
    default: return 0xFFFFFFFF;
    }
}

void ecall(M *m)
{
    uint32_t a7 = m->x[17], slot = a7 >> 8, r;
    int64_t ch = 4;
    if (slot == 1) r = sys(m, a7 & 0xFF, &ch);
    else if (slot < 256 && slot != 8 && slot != 10 && m->has[slot]) r = bus_call(m, slot, a7 & 0xFF, &ch);
    else r = 0xFFFFFFFF;
    m->x[10] = r;
    m->budget -= ch;
}

/* STATE.md 1: the serialization; device sections come from the drivers. */
typedef struct { uint8_t *p; size_t n, cap; } SB;
static void put(SB *b, const void *d, size_t n)
{
    if (b->n + n > b->cap) { b->cap = (b->n + n) * 2; b->p = realloc(b->p, b->cap); }
    memcpy(b->p + b->n, d, n); b->n += n;
}
static void p8(SB *b, uint32_t v) { uint8_t c = v; put(b, &c, 1); }
static void p32(SB *b, uint32_t v) { uint8_t c[4]; wr32(c, v); put(b, c, 4); }
static void section(M *m, SB *b, int slot, int flag)
{
    uint8_t *d; uint32_t n;
    if (flag) p8(b, 1);
    if (!bus_state(m, slot, &d, &n)) die(3, "driver of slot %d: no state", slot);
    put(b, d, n);
    free(d);
}

size_t serialize(M *m, uint8_t **out)
{
    SB b = {0};
    put(&b, "terse-state-1", 13); put(&b, m->id, 32);
    p32(&b, m->pc);
    for (int i = 1; i < 32; i++) p32(&b, m->x[i]);
    p32(&b, m->frame); p8(&b, m->stopped); p8(&b, m->exit_code); p8(&b, m->fault_kind);
    p32(&b, m->fault_enc); p32(&b, m->fault_pc); p32(&b, m->fault_addr);
    p32(&b, (uint32_t)(int32_t)m->budget);
    p32(&b, m->ram_len); put(&b, m->ram, m->ram_len);
    int lists8 = 0;
    for (int i = 0; i < m->ndev; i++) {
        if (m->dopt[i]) p8(&b, m->granted[i]);
        if (m->dslot[i] == 8) lists8 = 1;
    }
    p32(&b, m->time); p8(&b, m->qn);
    for (int i = 0; i < m->qn; i++) put(&b, m->q[(m->qhead + i) % QMAX], 4);
    put(&b, m->pads, 32);
    if (m->has[2]) section(m, &b, 2, 1); else p8(&b, 0);
    if (m->has[3] || m->has[9]) section(m, &b, 3, 1); else p8(&b, 0);
    if (m->has[4]) section(m, &b, 4, 1); else p8(&b, 0);
    if (m->has[7]) section(m, &b, 7, 1); else p8(&b, 0);
    if (lists8) p8(&b, 0);              /* a machine run on its own is never a shell */
    if (m->has[9]) section(m, &b, 9, 0);
    if (m->has[6]) section(m, &b, 6, 0);
    *out = b.p;
    return b.n;
}
