/*
 * A minimal Terse device driver, meant to be copied: the terminal
 * (device 2, SPEC.md §terminal), drawing nothing, on the device bus's
 * socket transport (DEVICES.md). One file, C11 and POSIX, no dependencies.
 *
 *     cc -std=c11 -O2 -Wall -Wextra -o terminal terminal.c
 *     terse-bus prog.terse --hash --driver 2=$PWD/terminal
 *     tcore     prog.terse --hash --driver 2=$PWD/terminal
 *
 * A machine run with it has the same state hash after every frame as with
 * the reference terminal driver: the bus only carries the device's state,
 * and this file keeps exactly that state. It is the "minimal driver" of
 * DEVICES.md 13, step by step; the numbers in the comments (STEP 1 ... 10)
 * are that list's, and README.md explains each. What it leaves out: shared
 * memory, pictures (layers, PRESENT), properties, services, and RESTORE (a
 * run that restores a snapshot gets an ERROR from it). It serves any number
 * of machines at once, so it also works under `terse-bus --shell`.
 *
 * The file has two halves: the device itself (pure SPEC §terminal, nothing
 * about the bus), then the bus. A new device keeps the second half and
 * replaces the first.
 *
 * `terminal --version` prints its name, version and build identifier (the
 * same identity its HELLO reply states): `fnv:` and the FNV-1a 64-bit hash
 * of its own executable.
 *
 * For tests, three environment variables (none changes a state hash):
 *   TERSE_BUS_CPU=<n>      pin itself to CPU n before connecting
 *   TERSE_EXAMPLE_IDLE=1   run as SCHED_IDLE, so a CPU hog starves it
 *   TERSE_EXAMPLE_SLOW=<kind>:<ms>:<from>[:<to>]
 *                          a deliberately slow driver: at END of the frames
 *                          from <= f < to (to omitted: forever), wait ms
 *                          milliseconds (fractional allowed). kind `sleep`:
 *                          blocked (nanosleep); `spin`: busy for ms of wall
 *                          time; `cpu`: busy until its own CPU time has
 *                          advanced by ms (a fixed amount of work).
 */
#define _GNU_SOURCE                     /* sched_setaffinity, SCHED_IDLE */
#ifdef __linux__
#include <sched.h>                      /* the test switches only (Linux) */
#endif
#ifdef __APPLE__
#include <mach-o/dyld.h>                /* _NSGetExecutablePath */
#endif
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

/* ======================================================= the device ===== */

enum { COLS = 80, ROWS = 30, CELLS = COLS * ROWS, GRID = CELLS * 4 };
/* The state section (STATE.md 1.6, after the present flag): the grid,
   cursor column and row, colours, cursor shown, pending wrap, the palette,
   and the incomplete UTF-8 sequence (a count and 3 bytes). */
enum { SECTION = GRID + 6 + 768 + 4 };
#define ERR_BUFFER 0xFFFFFFFEu

/* SPEC §terminal.cells: the code points 0000h-FFFFh whose East Asian Width
   (Unicode 16.0) is W or F take two cells. Same ranges as
   crates/terse-core/src/wide.rs. */
static const uint16_t WIDE[][2] = {
    {0x1100, 0x115F}, {0x231A, 0x231B}, {0x2329, 0x232A}, {0x23E9, 0x23EC}, {0x23F0, 0x23F0},
    {0x23F3, 0x23F3}, {0x25FD, 0x25FE}, {0x2614, 0x2615}, {0x2630, 0x2637}, {0x2648, 0x2653},
    {0x267F, 0x267F}, {0x268A, 0x268F}, {0x2693, 0x2693}, {0x26A1, 0x26A1}, {0x26AA, 0x26AB},
    {0x26BD, 0x26BE}, {0x26C4, 0x26C5}, {0x26CE, 0x26CE}, {0x26D4, 0x26D4}, {0x26EA, 0x26EA},
    {0x26F2, 0x26F3}, {0x26F5, 0x26F5}, {0x26FA, 0x26FA}, {0x26FD, 0x26FD}, {0x2705, 0x2705},
    {0x270A, 0x270B}, {0x2728, 0x2728}, {0x274C, 0x274C}, {0x274E, 0x274E}, {0x2753, 0x2755},
    {0x2757, 0x2757}, {0x2795, 0x2797}, {0x27B0, 0x27B0}, {0x27BF, 0x27BF}, {0x2B1B, 0x2B1C},
    {0x2B50, 0x2B50}, {0x2B55, 0x2B55}, {0x2E80, 0x2E99}, {0x2E9B, 0x2EF3}, {0x2F00, 0x2FD5},
    {0x2FF0, 0x303E}, {0x3041, 0x3096}, {0x3099, 0x30FF}, {0x3105, 0x312F}, {0x3131, 0x318E},
    {0x3190, 0x31E5}, {0x31EF, 0x321E}, {0x3220, 0x3247}, {0x3250, 0xA48C}, {0xA490, 0xA4C6},
    {0xA960, 0xA97C}, {0xAC00, 0xD7A3}, {0xF900, 0xFAFF}, {0xFE10, 0xFE19}, {0xFE30, 0xFE52},
    {0xFE54, 0xFE66}, {0xFE68, 0xFE6B}, {0xFF01, 0xFF60}, {0xFFE0, 0xFFE6},
};
static int wide(unsigned c)
{
    for (size_t i = 0; i < sizeof WIDE / sizeof *WIDE; i++)
        if (c >= WIDE[i][0] && c <= WIDE[i][1]) return 1;
    return 0;
}

typedef struct {
    uint8_t grid[GRID];                 /* cell n at 4n: code (2 bytes), fg, bg */
    uint8_t col, row, fg, bg, shown, wrap;
    uint8_t palette[768];
    uint8_t seq[3], nseq;               /* the bytes of an unfinished UTF-8 character */
    uint32_t cp; uint8_t need, lo, hi;  /* the decoder working on them */
} Term;

static Term *t;                         /* the machine the current message is about */

static unsigned code(int c, int r) { uint8_t *p = t->grid + 4 * (r * COLS + c); return p[0] | p[1] << 8; }
static void set_code(int c, int r, unsigned ch) { uint8_t *p = t->grid + 4 * (r * COLS + c); p[0] = ch; p[1] = ch >> 8; }
static void put_cell(int c, int r, unsigned ch) { set_code(c, r, ch); t->grid[4 * (r * COLS + c) + 2] = t->fg; t->grid[4 * (r * COLS + c) + 3] = t->bg; }
static int pair(int c, int r) { return c + 1 < COLS && code(c, r) != 0xFFFF && code(c + 1, r) == 0xFFFF; }

static void blank_rows(int from, int to)
{
    for (int r = from; r < to; r++)
        for (int c = 0; c < COLS; c++) put_cell(c, r, ' ');
}
static void clear(void) { blank_rows(0, ROWS); t->col = t->row = 0; t->wrap = 0; }
/* A real driver should make a scroll cost one row (SPEC §terminal.presentation:
   rows kept as a ring); a memmove of 9 KiB keeps this file short. */
static void scroll_up(void) { memmove(t->grid, t->grid + COLS * 4, GRID - COLS * 4); blank_rows(ROWS - 1, ROWS); }
static void scroll_down(void) { memmove(t->grid + COLS * 4, t->grid, GRID - COLS * 4); blank_rows(0, 1); }
static void next_row(void) { if (t->row == ROWS - 1) scroll_up(); else t->row++; }

/* SPEC §terminal.reset: spaces in colours 7 on 0, the xterm palette. */
static void reset(void)
{
    static const uint8_t base[16][3] = {
        {0, 0, 0}, {205, 0, 0}, {0, 205, 0}, {205, 205, 0}, {0, 0, 238}, {205, 0, 205}, {0, 205, 205},
        {229, 229, 229}, {127, 127, 127}, {255, 0, 0}, {0, 255, 0}, {255, 255, 0}, {92, 92, 255},
        {255, 0, 255}, {0, 255, 255}, {255, 255, 255}};
    static const uint8_t cube[6] = {0, 95, 135, 175, 215, 255};
    memset(t, 0, sizeof *t);
    t->fg = 7; t->shown = 1;
    blank_rows(0, ROWS);
    memcpy(t->palette, base, sizeof base);
    for (int i = 0; i < 216; i++) {
        t->palette[(16 + i) * 3] = cube[i / 36];
        t->palette[(16 + i) * 3 + 1] = cube[i / 6 % 6];
        t->palette[(16 + i) * 3 + 2] = cube[i % 6];
    }
    for (int i = 0; i < 24; i++) memset(&t->palette[(232 + i) * 3], 8 + 10 * i, 3);
}

/* SPEC §terminal.put, steps 1-4: prints one character. */
static void print(unsigned ch)
{
    int w = wide(ch) ? 2 : 1, c;
    if (t->wrap || (w == 2 && t->col == COLS - 1)) {
        if (!t->wrap) {                 /* a wide character does not fit: column 79 becomes a space */
            if (pair(COLS - 2, t->row)) set_code(COLS - 2, t->row, ' ');
            put_cell(COLS - 1, t->row, ' ');
        }
        t->wrap = 0; t->col = 0;
        next_row();
    }
    c = t->col;
    if (c > 0 && pair(c - 1, t->row)) set_code(c - 1, t->row, ' ');       /* step 3: a pair cut */
    if (pair(c + w - 1, t->row)) set_code(c + w, t->row, ' ');           /* in two loses its other half */
    put_cell(c, t->row, ch);
    if (w == 2) put_cell(c + 1, t->row, 0xFFFF);
    if (c + w > COLS - 1) { t->col = COLS - 1; t->wrap = 1; }
    else t->col = c + w;
}

/* A complete character from `write`: control characters, else print. */
static void put(uint32_t cp)
{
    switch (cp) {
    case 0x08: if (t->col) t->col--; break;
    case 0x09: t->col = t->col / 8 * 8 + 8 > COLS - 1 ? COLS - 1 : t->col / 8 * 8 + 8; break;
    case 0x0A: t->col = 0; next_row(); break;
    case 0x0C: clear(); break;
    case 0x0D: t->col = 0; break;
    default:
        if (cp < 0x20 || cp == 0x7F) return;            /* ignored; pending wrap stays */
        print(cp > 0xFFFF || cp == 0xFFFF ? 0xFFFD : cp);
        return;
    }
    t->wrap = 0;
}

/* SPEC §terminal.put: WHATWG UTF-8 decoding, one byte at a time, keeping the
   bytes of an unfinished character (they are part of the state). */
static void start(uint8_t b)
{
    t->lo = 0x80; t->hi = 0xBF;
    if (b < 0x80) { put(b); return; }
    if (b >= 0xC2 && b <= 0xDF) { t->need = 1; t->cp = b & 0x1F; }
    else if (b >= 0xE0 && b <= 0xEF) { t->need = 2; t->cp = b & 0x0F; if (b == 0xE0) t->lo = 0xA0; if (b == 0xED) t->hi = 0x9F; }
    else if (b >= 0xF0 && b <= 0xF4) { t->need = 3; t->cp = b & 0x07; if (b == 0xF0) t->lo = 0x90; if (b == 0xF4) t->hi = 0x8F; }
    else { put(0xFFFD); return; }
    t->seq[0] = b; t->nseq = 1;
}
static void byte(uint8_t b)
{
    if (!t->need) { start(b); return; }
    if (b < t->lo || b > t->hi) {       /* one U+FFFD, then the byte afresh */
        t->need = 0; t->nseq = 0; memset(t->seq, 0, 3);
        put(0xFFFD);
        start(b);
        return;
    }
    t->lo = 0x80; t->hi = 0xBF;
    t->cp = t->cp << 6 | (b & 0x3F);
    if (--t->need) { t->seq[t->nseq++] = b; return; }
    t->nseq = 0; memset(t->seq, 0, 3);
    put(t->cp);
}

/* SPEC §terminal.commands; returns the charge (150 for a clear or a scroll). */
static uint32_t command(uint32_t v)
{
    switch (v) {
    case 1: clear(); return 150;
    case 2: scroll_up(); return 150;
    case 3: scroll_down(); return 150;
    case 4:
        if (t->col > 0 && pair(t->col - 1, t->row)) set_code(t->col - 1, t->row, ' ');
        for (int c = t->col; c < COLS; c++) put_cell(c, t->row, ' ');
        return 0;
    case 5: t->shown = 1; return 0;
    case 6: t->shown = 0; return 0;
    }
    return 0;
}

/* ========================================================== the bus ===== */

typedef struct { uint8_t *p; size_t n, cap; } Buf;
static void put_bytes(Buf *b, const void *d, size_t n)   /* d = NULL: room only */
{
    if (b->n + n > b->cap) { b->cap = (b->n + n) * 2; if (!(b->p = realloc(b->p, b->cap))) exit(1); }
    if (d && n) memcpy(b->p + b->n, d, n);
    b->n += n;
}
static void u8(Buf *b, uint32_t v) { uint8_t c = v; put_bytes(b, &c, 1); }
static void u16(Buf *b, uint32_t v) { u8(b, v); u8(b, v >> 8); }
static void u32(Buf *b, uint32_t v) { u16(b, v); u16(b, v >> 16); }
static void str(Buf *b, const char *s) { u16(b, strlen(s)); put_bytes(b, s, strlen(s)); }
static uint32_t rd32(const uint8_t *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }

static int sock;
static uint32_t instance;               /* the address of the message being handled */
static uint8_t slot;

/* STEP 1. Every message: a u32 length n, then the address (instance u32,
   slot u8, 3 reserved bytes), then the body, whose first byte is the type.
   A reply carries the address of its request. */
static void reply(Buf *b, uint8_t type)
{
    b->n = 0;
    u32(b, 0); u32(b, instance); u8(b, slot); u8(b, 0); u16(b, 0); u8(b, type);
}
static void send_reply(Buf *b)
{
    uint32_t n = b->n - 4;
    memcpy(b->p, &(uint8_t[4]){n, n >> 8, n >> 16, n >> 24}, 4);
    for (size_t off = 0; off < b->n;) {
        ssize_t k = write(sock, b->p + off, b->n - off);
        if (k <= 0) exit(0);            /* the core has gone */
        off += k;
    }
}
static int read_full(uint8_t *p, size_t n)
{
    while (n) {
        ssize_t k = read(sock, p, n);
        if (k <= 0) return 0;
        p += k; n -= k;
    }
    return 1;
}
/* STEP 10. Anything this driver cannot handle: ERROR with a reason, then
   exit. The core then gives the driver up (DEVICES.md 10). */
static void fail(const char *why)
{
    Buf b = {0};
    reply(&b, 0x7F); put_bytes(&b, why, strlen(why));
    send_reply(&b);
    fprintf(stderr, "terminal example: %s\n", why);
    exit(1);
}

/* STEP 2's call table (DEVICES.md 5): one 64-byte entry per call. It is
   what lets the core check buffers and charge without knowing the device,
   and compute a deferred call's result and charge itself (then it sends the
   call later, in a BATCH). It must say exactly what SPEC §terminal says. */
enum { A1 = 1, DEFER = 2, BUF = 4 };                 /* entry flags */
enum { READS = 1, WRITES = 2, ROM_OK = 4 };          /* buffer access */
typedef struct {
    uint8_t fn, flags, addr, len, access, checked, rule, rule_arg, per4, charge_arg;
    uint32_t scale, most, checked_max, value, value2;
    uint16_t per_byte;
    uint32_t low, high, add;
} Entry;
#define NONE .len = 0xFF, .checked = 0xFF, .charge_arg = 0xFF   /* no buffer, nothing checked */
static const Entry TABLE[] = {
    /* write: buffer a0, a1 bytes (0-4,096), read, may be ROM; a0 = a1; 2 per byte */
    {.fn = 0, .flags = DEFER | BUF, .addr = 0, .len = 1, .scale = 1, .most = 4096, .access = READS | ROM_OK,
     .checked = 0xFF, .rule = 2, .rule_arg = 1, .charge_arg = 0xFF, .per_byte = 2},
    /* command: a0 = 0; 150 if 1 <= a0 <= 3 */
    {.fn = 1, .flags = DEFER, .len = 0xFF, .checked = 0xFF, .rule = 1, .charge_arg = 0, .low = 1, .high = 3, .add = 150},
    {.fn = 2, .flags = DEFER, NONE, .rule = 1},                          /* set_cursor: 0 */
    {.fn = 3, .flags = A1, NONE},                                        /* get_cursor: answered */
    {.fn = 4, .flags = DEFER, NONE, .rule = 1},                          /* set_colours: 0 */
    {.fn = 5, .flags = A1, NONE},                                        /* get_colours: answered */
    /* cells_read: buffer a1, a2 x 4 bytes (0-9,600), written; a0 <= 2,399; answered */
    {.fn = 6, .flags = BUF, .addr = 1, .len = 2, .scale = 4, .most = GRID, .access = WRITES,
     .checked = 0, .checked_max = CELLS - 1, .charge_arg = 0xFF},
    /* cells_write: buffer a1, a2 x 4 (0-9,600), read, ROM; a0 <= 2,399; a0 = a2; 1 per 4 bytes */
    {.fn = 7, .flags = DEFER | BUF, .addr = 1, .len = 2, .scale = 4, .most = GRID, .access = READS | ROM_OK,
     .checked = 0, .checked_max = CELLS - 1, .rule = 2, .rule_arg = 2, .per4 = 1, .charge_arg = 0xFF},
    /* palette_set: buffer a1, a2 x 3 (0-768), read, ROM; a0 <= 255; a0 = a2; 1 per 4 bytes */
    {.fn = 8, .flags = DEFER | BUF, .addr = 1, .len = 2, .scale = 3, .most = 768, .access = READS | ROM_OK,
     .checked = 0, .checked_max = 255, .rule = 2, .rule_arg = 2, .per4 = 1, .charge_arg = 0xFF},
    /* size: a0 = 80, a1 = 30 */
    {.fn = 9, .flags = A1 | DEFER, NONE, .rule = 1, .value = COLS, .value2 = ROWS},
};
static void table_entry(Buf *b, const Entry *e)
{
    u8(b, e->fn); u8(b, e->flags); u8(b, e->addr); u8(b, e->len);
    u32(b, e->scale); u32(b, 0); u32(b, e->most);              /* bytes per unit, least, greatest */
    u8(b, e->access); u8(b, e->checked); u16(b, 0); u32(b, e->checked_max);
    u32(b, ERR_BUFFER); u32(b, ERR_BUFFER);                    /* results: bad length, bad place */
    u8(b, e->rule); u8(b, e->rule_arg); u16(b, 0); u32(b, e->value); u32(b, e->value2);
    u32(b, 0);                                                  /* constant charge */
    u8(b, e->per4); u8(b, e->charge_arg); u16(b, e->per_byte);
    u32(b, e->low); u32(b, e->high); u32(b, e->add);
}

/* STEP 3. The machines served: one terminal each, by instance. */
static struct { uint32_t id; Term *t; } *machines;
static size_t nmachines;
static Term *find(uint32_t id)
{
    for (size_t i = 0; i < nmachines; i++) if (machines[i].id == id) return machines[i].t;
    return 0;
}

/* STEP 5. One call: the core has checked the buffer and the checked
   argument (DEVICES.md 5) and sent the bytes of the buffer the device reads.
   The reply: a0, a1, the charge on top of the base charge, and what the
   call writes into guest memory (only within its write buffer). */
static void call(const uint8_t *m, size_t n, Buf *r)
{
    static uint8_t out[GRID];
    uint32_t a[6], ret = 0, a1 = 0, charge = 0, wlen = 0, waddr = 0;
    int sets_a1 = 0;
    if (n < 40 || slot != 2) fail("a call this driver does not serve");
    for (int i = 0; i < 6; i++) a[i] = rd32(m + 4 + 4 * i);
    /* m + 28: the budget (i64), not needed by any terminal call */
    const uint8_t *data = m + 40;
    if (rd32(m + 36) > n - 40) fail("CALL cut short");
    switch (m[1]) {
    case 0:                                                    /* write */
        for (uint32_t i = 0; i < a[1]; i++) byte(data[i]);
        ret = a[1]; charge = 2 * a[1];
        break;
    case 1: charge = command(a[0]); break;                      /* command */
    case 2:                                                    /* set_cursor */
        t->col = a[0] > COLS - 1 ? COLS - 1 : a[0];
        t->row = a[1] > ROWS - 1 ? ROWS - 1 : a[1];
        t->wrap = 0;
        break;
    case 3: ret = t->col; a1 = t->row; sets_a1 = 1; break;     /* get_cursor */
    case 4:                                                    /* set_colours */
        if (a[0] < 256) t->fg = a[0];
        if (a[1] < 256) t->bg = a[1];
        break;
    case 5: ret = t->fg; a1 = t->bg; sets_a1 = 1; break;       /* get_colours */
    case 6:                                                    /* cells_read: wraps at the end */
    case 7:                                                    /* cells_write */
        wlen = a[2] * 4;
        for (uint32_t i = 0; i < wlen; i++) {
            uint8_t *cell = &t->grid[(a[0] * 4 + i) % GRID];
            if (m[1] == 6) out[i] = *cell; else *cell = data[i];
        }
        ret = a[2]; charge = (wlen + 3) / 4;
        if (m[1] == 6) waddr = a[1]; else wlen = 0;
        break;
    case 8:                                                    /* palette_set: wraps at 255 */
        for (uint32_t i = 0; i < a[2]; i++) memcpy(&t->palette[(a[0] + i) % 256 * 3], data + 3 * i, 3);
        ret = a[2]; charge = (a[2] * 3 + 3) / 4;
        break;
    case 9: ret = COLS; a1 = ROWS; sets_a1 = 1; break;          /* size */
    default: fail("a function that is not in the call table");
    }
    reply(r, 0x90);
    u8(r, sets_a1); u16(r, 0); u32(r, ret); u32(r, a1); u32(r, charge);
    u32(r, wlen ? 1 : 0);                                       /* writes: address, length, bytes */
    if (wlen) { u32(r, waddr); u32(r, wlen); put_bytes(r, out, wlen); }
}

/* The identity a HELLO reply ends with (DEVICES.md 1.1): a version and a
   build identifier, here FNV-1a 64-bit of this program's own file. Self-
   reported, for accountability among honest drivers, not for security. */
#define VERSION "0.1.0"
static char build[32] = "unknown";
static void compute_build(void)
{
#ifdef __APPLE__
    char self[4096];
    uint32_t len = sizeof self;
    FILE *f = _NSGetExecutablePath(self, &len) == 0 ? fopen(self, "rb") : NULL;
#else
    FILE *f = fopen("/proc/self/exe", "rb");
#endif
    if (!f) return;
    uint64_t h = 14695981039346656037ull;
    for (int c; (c = getc(f)) != EOF;) { h ^= (uint8_t)c; h *= 1099511628211ull; }
    if (!ferror(f)) snprintf(build, sizeof build, "fnv:%016llx", (unsigned long long)h);
    fclose(f);
}

/* TERSE_EXAMPLE_SLOW (see the top of the file): a test of how a core
   copes with a driver that is late at END. */
static struct { char kind; double ms; uint32_t from, to; } slow;
static double secs(clockid_t c)
{
    struct timespec ts;
    clock_gettime(c, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}
static void parse_slow(const char *s)
{
    char *e;
    if (!strncmp(s, "sleep:", 6)) slow.kind = 's';
    else if (!strncmp(s, "spin:", 5)) slow.kind = 'w';
    else if (!strncmp(s, "cpu:", 4)) slow.kind = 'c';
    else goto bad;
    s = strchr(s, ':') + 1;
    slow.ms = strtod(s, &e);
    if (e == s || *e != ':' || !(slow.ms >= 0)) goto bad;
    slow.from = strtoul(s = e + 1, &e, 10);
    if (e == s) goto bad;
    slow.to = UINT32_MAX;
    if (*e == ':') { slow.to = strtoul(s = e + 1, &e, 10); if (e == s) goto bad; }
    if (*e) goto bad;
    return;
bad:
    fputs("terminal example: TERSE_EXAMPLE_SLOW is <sleep|spin|cpu>:<ms>:<from>[:<to>]\n", stderr);
    exit(2);
}
static void be_slow(uint32_t frame)
{
    if (!slow.kind || frame < slow.from || frame >= slow.to) return;
    double s = slow.ms / 1000;
    if (slow.kind == 's') {
        struct timespec ts = {(time_t)s, (long)((s - (time_t)s) * 1e9)};
        while (nanosleep(&ts, &ts)) {}
    } else {
        clockid_t c = slow.kind == 'w' ? CLOCK_MONOTONIC : CLOCK_THREAD_CPUTIME_ID;
        double until = secs(c) + s;
        while (secs(c) < until) {}
    }
}

int main(int argc, char **argv)
{
    compute_build();
    if (argc > 1 && !strcmp(argv[1], "--version")) { printf("terminal " VERSION " %s\n", build); return 0; }
    const char *env = getenv("TERSE_EXAMPLE_SLOW");
    if (env && *env) parse_slow(env);
#ifdef __linux__
    if ((env = getenv("TERSE_BUS_CPU")) && *env) {
        cpu_set_t set;
        CPU_ZERO(&set); CPU_SET(atoi(env), &set);
        if (sched_setaffinity(0, sizeof set, &set)) perror("terminal example: TERSE_BUS_CPU");
    }
    if ((env = getenv("TERSE_EXAMPLE_IDLE")) && !strcmp(env, "1")
        && sched_setscheduler(0, SCHED_IDLE, &(struct sched_param){0}))
        perror("terminal example: TERSE_EXAMPLE_IDLE");
#endif

    /* STEP 1. Connect to the socket the core named. */
    const char *path = getenv("TERSE_BUS_SOCKET");
    struct sockaddr_un sa = {.sun_family = AF_UNIX};
    if (!path || strlen(path) >= sizeof sa.sun_path) { fputs("start me from a Terse core\n", stderr); return 2; }
    strcpy(sa.sun_path, path);
    sock = socket(AF_UNIX, SOCK_STREAM, 0);
    if (sock < 0 || connect(sock, (struct sockaddr *)&sa, sizeof sa)) { perror("connect"); return 1; }

    Buf in = {0}, r = {0}, scratch = {0};
    for (;;) {
        uint8_t h[4];
        if (!read_full(h, 4)) return 0;                         /* STEP 10: the core has gone */
        uint32_t len = rd32(h);
        if (len < 9 || len > 16777216) fail("a malformed message");
        in.n = 0; put_bytes(&in, 0, len);
        if (!read_full(in.p, len)) return 0;
        instance = rd32(in.p); slot = in.p[4];
        const uint8_t *m = in.p + 8;
        size_t n = len - 8;
        t = find(instance);
        if (!t && m[0] != 0x01 && m[0] != 0x03 && m[0] != 0x04 && m[0] != 0x11 && m[0] != 0x60)
            fail("a message about a machine that was not opened");

        switch (m[0]) {
        case 0x01: { /* STEP 2. HELLO: answer for exactly the slots asked, 2 and maybe 10 */
            if (n < 9 || m[1] != 1) fail("not protocol version 1");
            int s2 = 0, s10 = 0;
            for (int i = 0; i < m[8]; i++) { s2 |= m[9 + i] == 2; s10 |= m[9 + i] == 10; }
            if (!s2 || m[8] != 1 + s10) fail("this driver serves slot 2 (and 10)");
            reply(&r, 0x81);
            u8(&r, 1); u8(&r, 0); u8(&r, 0);                   /* version 1, the socket, reserved */
            str(&r, "terminal (example)");
            u16(&r, 0); u16(&r, 0);                             /* no native size: draws nothing */
            u8(&r, 1 + s10);
            u8(&r, 2); u8(&r, 0); u8(&r, 0);                   /* slot 2, version 0, no layers */
            u8(&r, sizeof TABLE / sizeof *TABLE);
            for (size_t i = 0; i < sizeof TABLE / sizeof *TABLE; i++) table_entry(&r, &TABLE[i]);
            /* Slot 10, full text (SPEC §terminal.text): a capability with no
               calls and no state. A driver that draws nothing lacks no
               character, so it gives it. */
            if (s10) { u8(&r, 10); u8(&r, 0); u8(&r, 0); u8(&r, 0); }
            u8(&r, 0); u8(&r, 0); u8(&r, 0);                    /* no properties, subscriptions, services */
            str(&r, VERSION); str(&r, build);                   /* identity: version, build */
            send_reply(&r);
            break;
        }
        case 0x03: { /* STEP 3. OPEN: a machine starts, its terminal at reset */
            size_t at = 44;                                     /* after the image ID, ROM and RAM lengths */
            if (n < at + 3) fail("OPEN cut short");
            at += 2 + (m[at] | m[at + 1] << 8);                /* the memory file: empty on the socket */
            int version = 0;
            for (int i = 0; i < m[at] && at + 2 + 2 * i < n; i++)
                if (m[at + 1 + 2 * i] == 2) version = m[at + 2 + 2 * i];
            reply(&r, 0x83);
            if (version != 0) { u8(&r, 0); u16(&r, 0); str(&r, "this driver implements terminal version 0"); }
            else {
                if (!t) {
                    machines = realloc(machines, (nmachines + 1) * sizeof *machines);
                    machines[nmachines].id = instance;
                    t = machines[nmachines++].t = malloc(sizeof *t);
                }
                reset();
                u8(&r, 1); u16(&r, 0); str(&r, "");
            }
            send_reply(&r);
            break;
        }
        case 0x04: /* STEP 4. CLOSE: the machine is gone; no reply */
            for (size_t i = 0; i < nmachines; i++)
                if (machines[i].id == instance) { free(machines[i].t); machines[i] = machines[--nmachines]; break; }
            break;
        case 0x02: /* RESET (the reference core never sends it): back to reset */
            reset();
            reply(&r, 0x82); send_reply(&r);
            break;
        case 0x10: /* STEP 5. CALL, answered with RESULT */
            call(m, n, &r);
            send_reply(&r);
            break;
        case 0x11: { /* STEP 6. BATCH: deferred calls, each a CALL message; no replies */
            size_t at = 8;
            for (uint32_t i = 0, count = n >= 8 ? rd32(m + 4) : 0; i < count; i++) {
                uint32_t l = at + 4 <= n ? rd32(m + at) : 0;
                if (l < 9 || at + 4 + l > n) fail("a malformed BATCH");
                instance = rd32(m + at + 4); slot = m[at + 8];
                if (!(t = find(instance))) fail("a batched call for a machine that was not opened");
                call(m + at + 12, l - 8, &scratch);
                at += 4 + l;
            }
            break;
        }
        case 0x30: /* STEP 7. BEGIN: the terminal has no inputs */
            reply(&r, 0xB0);
            u8(&r, 0); u16(&r, 0);
            u32(&r, 0); u32(&r, 0); u32(&r, 0); u32(&r, 0);    /* charge, events, writes, record */
            send_reply(&r);
            break;
        case 0x31: /* STEP 8. END: nothing to do between frames (but the slow test mode) */
            if (n >= 8) be_slow(rd32(m + 4));
            break;
        case 0x40: { /* STEP 9. STATE: the section of STATE.md 1.6 */
            if (slot != 2) fail("STATE of a slot that has no section here");
            reply(&r, 0xC0);
            u8(&r, 0); u16(&r, 0); u32(&r, SECTION);
            put_bytes(&r, t->grid, GRID);
            put_bytes(&r, &t->col, 1); put_bytes(&r, &t->row, 1); put_bytes(&r, &t->fg, 1);
            put_bytes(&r, &t->bg, 1); put_bytes(&r, &t->shown, 1); put_bytes(&r, &t->wrap, 1);
            put_bytes(&r, t->palette, 768);
            put_bytes(&r, &t->nseq, 1); put_bytes(&r, t->seq, 3);
            send_reply(&r);
            break;
        }
        case 0x60: /* STEP 10. SHUTDOWN */
            return 0;
        default:   /* STEP 10. RESTORE, GET, SET, SERVICE, PRESENT, ERROR, ...: not this driver's */
            fail("a message this minimal driver does not handle");
        }
    }
}
