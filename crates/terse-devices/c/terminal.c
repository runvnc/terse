/*
 * Build: clang -std=c11 -O2 -Wall -Wextra -o terse-dev-terminal-c terminal.c
 *
 * Terse device 2, the terminal (SPEC.md §terminal, version 0), as a device driver
 * of the device bus (DEVICES.md), in C11 with libc only. It draws nothing:
 * no layers, no native size. It keeps the device state and answers the
 * calls with their exact results and charges (SPEC.md §charges), so that a run
 * with it gives the same state hashes (STATE.md 1.6) as the built-in
 * terminal of crates/terse-core.
 *
 * It speaks protocol version 1 and serves any number of machines at once
 * (DEVICES.md 1.2): each instance has its own terminal and, on shared
 * memory, its own memory file, mapped at OPEN and unmapped at CLOSE.
 *
 * Environment: TERSE_BUS_SOCKET (the core's socket), TERSE_BUS_CPU (a CPU
 * to run on). With TERSE_C_SHM=1 the driver chooses the shared-memory
 * transport (DEVICES.md 4) when the core offers it; otherwise the socket.
 * On shared memory it spins adaptively (DEVICES.md 4.2) up to
 * TERSE_BUS_SPIN_US (2,000) before it sleeps; TERSE_BUS_SPIN=fixed spins
 * that long every time.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

/* What is particular to a system (DEVICES.md 4.3): Linux sleeps on a futex
 * and pins to a CPU; macOS wakes a core asleep on its address with
 * __ulock_wake (shared, macOS 10.12 or later) and itself sleeps on the
 * socket; elsewhere both ends sleep on the socket. */
#ifdef __linux__
#include <linux/futex.h>
#include <sched.h>
#include <sys/syscall.h>
#elif defined(__APPLE__)
#include <mach-o/dyld.h>
extern int __ulock_wake(uint32_t operation, void *addr, uint64_t wake_value);
#define UL_COMPARE_AND_WAIT_SHARED 3
#define ULF_WAKE_ALL 0x100
#endif

/* ---------------------------------------------------------------- device */

enum { COLS = 80, ROWS = 30, CELLS = COLS * ROWS, CELL_MEMORY = CELLS * 4 };
enum { CONTINUATION = 0xFFFF, REPLACEMENT = 0xFFFD, SPACE = 0x0020 };
enum { STATE_LEN = CELL_MEMORY + 6 + 768 + 4 }; /* 10,378 */
enum { CHARGE_OP = 150 };                      /* a command's clear or scroll */
enum { CHARGE_BYTE = 2, MAX_WRITE = 4096 };    /* write: per byte, longest */
enum { ROW_BYTES = COLS * 4 };
#define ERR_BUFFER 0xFFFFFFFEu
#define ERR_UNKNOWN 0xFFFFFFFFu

/* Code points 0000h-FFFFh whose Unicode 16.0 East Asian Width is W or F,
 * unassigned default-W included: the same ranges as crates/terse-core/src/
 * wide.rs (inclusive). */
static const uint16_t WIDE[][2] = {
    {0x1100, 0x115F}, {0x231A, 0x231B}, {0x2329, 0x232A}, {0x23E9, 0x23EC},
    {0x23F0, 0x23F0}, {0x23F3, 0x23F3}, {0x25FD, 0x25FE}, {0x2614, 0x2615},
    {0x2630, 0x2637}, {0x2648, 0x2653}, {0x267F, 0x267F}, {0x268A, 0x268F},
    {0x2693, 0x2693}, {0x26A1, 0x26A1}, {0x26AA, 0x26AB}, {0x26BD, 0x26BE},
    {0x26C4, 0x26C5}, {0x26CE, 0x26CE}, {0x26D4, 0x26D4}, {0x26EA, 0x26EA},
    {0x26F2, 0x26F3}, {0x26F5, 0x26F5}, {0x26FA, 0x26FA}, {0x26FD, 0x26FD},
    {0x2705, 0x2705}, {0x270A, 0x270B}, {0x2728, 0x2728}, {0x274C, 0x274C},
    {0x274E, 0x274E}, {0x2753, 0x2755}, {0x2757, 0x2757}, {0x2795, 0x2797},
    {0x27B0, 0x27B0}, {0x27BF, 0x27BF}, {0x2B1B, 0x2B1C}, {0x2B50, 0x2B50},
    {0x2B55, 0x2B55}, {0x2E80, 0x2E99}, {0x2E9B, 0x2EF3}, {0x2F00, 0x2FD5},
    {0x2FF0, 0x303E}, {0x3041, 0x3096}, {0x3099, 0x30FF}, {0x3105, 0x312F},
    {0x3131, 0x318E}, {0x3190, 0x31E5}, {0x31EF, 0x321E}, {0x3220, 0x3247},
    {0x3250, 0xA48C}, {0xA490, 0xA4C6}, {0xA960, 0xA97C}, {0xAC00, 0xD7A3},
    {0xF900, 0xFAFF}, {0xFE10, 0xFE19}, {0xFE30, 0xFE52}, {0xFE54, 0xFE66},
    {0xFE68, 0xFE6B}, {0xFF01, 0xFF60}, {0xFFE0, 0xFFE6},
};

static int is_wide(unsigned ch) {
    size_t lo = 0, hi = sizeof WIDE / sizeof WIDE[0];
    while (lo < hi) {
        size_t mid = (lo + hi) / 2;
        if (WIDE[mid][1] < ch) lo = mid + 1;
        else if (WIDE[mid][0] > ch) hi = mid;
        else return 1;
    }
    return 0;
}

/* The UTF-8 decoder of `write` (SPEC §terminal.put): WHATWG's, keeping the bytes of
 * the incomplete sequence (STATE.md 1.6). */
typedef struct {
    uint32_t cp;
    uint8_t needed, lower, upper, len;
    uint8_t bytes[3];
} Decoder;

/* The rows are a ring: stored row (top + r) % ROWS is row r. A scroll or a
 * clear only marks rows blank (one cell for the whole row); a blank row is
 * filled when it is written to (SPEC §terminal.presentation: a scroll costs one row at most). A
 * stored row whose stamp is below `clears` holds clear_cell. */
typedef struct {
    uint8_t cells[CELL_MEMORY];
    uint8_t col, row, fg, bg, shown, wrap;
    uint8_t palette[768];
    Decoder dec;
    int top;
    uint8_t blank_set[ROWS], blank[ROWS][4], clear_cell[4];
    uint64_t stamp[ROWS], clears;
} Terminal;

/* The terminal of the instance (machine) the current message is about. */
static Terminal *t;

/* Starts a sequence with byte b: 1 and *out if it is a whole character. */
static int dec_start(uint8_t b, uint32_t *out) {
    uint8_t needed, lower = 0x80, upper = 0xBF;
    if (b < 0x80) { *out = b; return 1; }
    if (b >= 0xC2 && b <= 0xDF) needed = 1;
    else if (b == 0xE0) { needed = 2; lower = 0xA0; }
    else if (b == 0xED) { needed = 2; upper = 0x9F; } /* no surrogates */
    else if (b >= 0xE1 && b <= 0xEF) needed = 2;
    else if (b == 0xF0) { needed = 3; lower = 0x90; }
    else if (b == 0xF4) { needed = 3; upper = 0x8F; } /* nothing above 10FFFFh */
    else if (b >= 0xF1 && b <= 0xF3) needed = 3;
    else { *out = REPLACEMENT; return 1; }
    uint32_t bits = needed == 1 ? (b & 0x1Fu) : needed == 2 ? (b & 0x0Fu) : (b & 0x07u);
    t->dec = (Decoder){ .cp = bits, .needed = needed, .lower = lower, .upper = upper,
                       .len = 1, .bytes = { b, 0, 0 } };
    return 0;
}

/* Feeds one byte; returns how many characters (0-2) completed into out. */
static int dec_feed(uint8_t b, uint32_t out[2]) {
    Decoder *d = &t->dec;
    if (d->needed == 0) return dec_start(b, &out[0]);
    if (b < d->lower || b > d->upper) {
        memset(d, 0, sizeof *d);
        out[0] = REPLACEMENT;
        return 1 + dec_start(b, &out[1]); /* the interrupting byte afresh */
    }
    d->lower = 0x80;
    d->upper = 0xBF;
    d->cp = (d->cp << 6) | (b & 0x3Fu);
    if (--d->needed == 0) {
        out[0] = d->cp;
        memset(d, 0, sizeof *d);
        return 1;
    }
    d->bytes[d->len++] = b;
    return 0;
}

static int stored(int row) { int p = t->top + row; return p >= ROWS ? p - ROWS : p; }
/* The cell all of stored row p holds, or NULL if it is not blank. */
static const uint8_t *blank_of(int p) {
    if (t->stamp[p] < t->clears) return t->clear_cell;
    return t->blank_set[p] ? t->blank[p] : NULL;
}
/* Row `row` to change: filled first if blank. */
static uint8_t *row_mut(int row) {
    int p = stored(row);
    const uint8_t *b = blank_of(p);
    uint8_t *r = &t->cells[p * ROW_BYTES];
    if (b) {
        uint8_t c[4];
        memcpy(c, b, 4);
        for (int i = 0; i < COLS; i++) memcpy(r + 4 * i, c, 4);
    }
    t->blank_set[p] = 0;
    t->stamp[p] = t->clears;
    return r;
}
/* Byte i of the grid (SPEC §terminal.cells order). */
static uint8_t grid_byte(uint32_t i) {
    int p = stored((int)(i / ROW_BYTES));
    const uint8_t *b = blank_of(p);
    return b ? b[i % 4] : t->cells[p * ROW_BYTES + i % ROW_BYTES];
}
static void set_grid_byte(uint32_t i, uint8_t v) { row_mut((int)(i / ROW_BYTES))[i % ROW_BYTES] = v; }
static unsigned ch_at(int col, int row) {
    uint32_t i = (uint32_t)(row * COLS + col) * 4;
    return grid_byte(i) | (unsigned)grid_byte(i + 1) << 8;
}
static void set_ch(int col, int row, unsigned ch) { uint8_t *c = row_mut(row) + col * 4; c[0] = ch & 0xFF; c[1] = ch >> 8; }
static void set_cell(int col, int row, unsigned ch) {
    uint8_t *c = row_mut(row) + col * 4;
    c[0] = ch & 0xFF; c[1] = ch >> 8; c[2] = t->fg; c[3] = t->bg;
}

/* Cells col and col + 1 of row hold a wide pair. */
static int is_pair(int col, int row) {
    return col + 1 < COLS && ch_at(col, row) != CONTINUATION && ch_at(col + 1, row) == CONTINUATION;
}

/* Blanks rows [from, to) with spaces in the current colours. */
static void fill_rows(int from, int to) {
    uint8_t c[4] = { SPACE & 0xFF, SPACE >> 8, t->fg, t->bg };
    if (from == 0 && to == ROWS) {
        t->clears++;
        memcpy(t->clear_cell, c, 4);
        return;
    }
    for (int r = from; r < to; r++) {
        int p = stored(r);
        t->blank_set[p] = 1;
        memcpy(t->blank[p], c, 4);
        t->stamp[p] = t->clears;
    }
}

static unsigned clear_grid(void) {
    fill_rows(0, ROWS);
    t->col = t->row = 0;
    t->wrap = 0;
    return 1;
}

static unsigned scroll_up(void) {
    t->top = stored(1);
    fill_rows(ROWS - 1, ROWS);
    return 1;
}

static unsigned scroll_down(void) {
    t->top = stored(ROWS - 1);
    fill_rows(0, 1);
    return 1;
}

static unsigned next_row(void) {
    if (t->row == ROWS - 1) return scroll_up();
    t->row++;
    return 0;
}

/* Writes n (1 or 2) codes from col in the cursor row; a wide pair cut in
 * two loses its other half, which becomes a space keeping its colours
 * (SPEC §terminal.put step 3). */
static void write_cells(int col, const unsigned *chars, int n) {
    int row = t->row, end = col + n;
    int left = col > 0 && is_pair(col - 1, row) ? col - 1 : -1;
    int right = is_pair(end - 1, row) ? end : -1;
    if (left >= 0) set_ch(left, row, SPACE);
    if (right >= 0) set_ch(right, row, SPACE);
    for (int i = 0; i < n; i++) set_cell(col + i, row, chars[i]);
}

/* SPEC §terminal.put steps 1-4; returns the scrolls done. */
static unsigned print_char(unsigned ch) {
    int width = is_wide(ch) ? 2 : 1;
    unsigned ops = 0;
    if (t->wrap || (width == 2 && t->col == COLS - 1)) {
        if (!t->wrap) {
            unsigned sp = SPACE;
            write_cells(COLS - 1, &sp, 1);
        }
        t->wrap = 0;
        t->col = 0;
        ops = next_row();
    }
    int col = t->col;
    unsigned pair[2] = { ch, CONTINUATION };
    write_cells(col, pair, width);
    if (col + width > COLS - 1) {
        t->col = COLS - 1;
        t->wrap = 1;
    } else {
        t->col = (uint8_t)(col + width);
    }
    return ops;
}

static unsigned put_char(uint32_t cp) {
    unsigned ops = 0;
    switch (cp) {
    case 0x08: if (t->col > 0) t->col--; break;
    case 0x09: t->col = (uint8_t)((t->col / 8 + 1) * 8); if (t->col > COLS - 1) t->col = COLS - 1; break;
    case 0x0A: t->col = 0; ops = next_row(); break;
    case 0x0C: ops = clear_grid(); break;
    case 0x0D: t->col = 0; break;
    default:
        if (cp < 0x20 || cp == 0x7F) return 0; /* ignored: pending wrap stays */
        return print_char(cp > 0xFFFF || cp == CONTINUATION ? REPLACEMENT : cp);
    }
    t->wrap = 0;
    return ops;
}

static unsigned put_byte(uint8_t b) {
    uint32_t cps[2];
    int n = dec_feed(b, cps);
    unsigned ops = 0;
    for (int i = 0; i < n; i++) ops += put_char(cps[i]);
    return ops;
}

/* `write` (SPEC §terminal.calls): every byte; scrolls and clears are not charged. */
static void write_bytes(const uint8_t *bytes, uint32_t n) {
    for (uint32_t i = 0; i < n; i++) put_byte(bytes[i]);
}

static unsigned command(uint32_t v) {
    switch (v) {
    case 1: return clear_grid();
    case 2: return scroll_up();
    case 3: return scroll_down();
    case 4: {
        int col = t->col, row = t->row;
        if (col > 0 && is_pair(col - 1, row)) set_ch(col - 1, row, SPACE);
        for (int c = col; c < COLS; c++) set_cell(c, row, SPACE);
        return 0;
    }
    case 5: t->shown = 1; return 0;
    case 6: t->shown = 0; return 0;
    default: return 0;
    }
}

static void reset_device(void) {
    static const uint8_t base[16][3] = {
        {0x00, 0x00, 0x00}, {0xCD, 0x00, 0x00}, {0x00, 0xCD, 0x00}, {0xCD, 0xCD, 0x00},
        {0x00, 0x00, 0xEE}, {0xCD, 0x00, 0xCD}, {0x00, 0xCD, 0xCD}, {0xE5, 0xE5, 0xE5},
        {0x7F, 0x7F, 0x7F}, {0xFF, 0x00, 0x00}, {0x00, 0xFF, 0x00}, {0xFF, 0xFF, 0x00},
        {0x5C, 0x5C, 0xFF}, {0xFF, 0x00, 0xFF}, {0x00, 0xFF, 0xFF}, {0xFF, 0xFF, 0xFF},
    };
    static const uint8_t cube[6] = { 0x00, 0x5F, 0x87, 0xAF, 0xD7, 0xFF };
    memset(t, 0, sizeof *t);
    t->fg = 7;
    t->shown = 1;
    fill_rows(0, ROWS);
    memcpy(t->palette, base, sizeof base);
    for (int i = 0; i < 216; i++) {
        uint8_t *p = &t->palette[(16 + i) * 3];
        p[0] = cube[i / 36]; p[1] = cube[i / 6 % 6]; p[2] = cube[i % 6];
    }
    for (int i = 0; i < 24; i++) memset(&t->palette[(232 + i) * 3], 8 + 10 * i, 3);
}

/* STATE.md 1.6, the bytes after the present flag. */
static void write_state(uint8_t out[STATE_LEN]) {
    uint8_t *p = out;
    for (uint32_t i = 0; i < CELL_MEMORY; i++) *p++ = grid_byte(i);
    *p++ = t->col; *p++ = t->row; *p++ = t->fg; *p++ = t->bg; *p++ = t->shown; *p++ = t->wrap;
    memcpy(p, t->palette, 768); p += 768;
    *p++ = t->dec.len;
    memcpy(p, t->dec.bytes, 3);
}

/* STATE.md 4: takes the section at the start of the n bytes of s (the rest
 * of a serialization: it has a fixed length, STATE_LEN) only if serializing
 * it again gives the same bytes; otherwise nothing changes. */
static int restore_state(const uint8_t *s, size_t n) {
    if (n < STATE_LEN) return 0;
    const uint8_t *p = s + CELL_MEMORY;
    if (p[0] >= COLS || p[1] >= ROWS || p[4] > 1 || p[5] > 1) return 0;
    uint8_t k = p[6 + 768];
    const uint8_t *seq = p + 6 + 768 + 1;
    if (k > 3) return 0;
    Decoder saved = t->dec;
    memset(&t->dec, 0, sizeof t->dec);
    int ok = 1;
    for (int i = 0; i < k && ok; i++) {
        uint32_t cps[2];
        if (dec_feed(seq[i], cps) != 0) ok = 0;
    }
    if (ok && (t->dec.len != k || memcmp(t->dec.bytes, seq, 3) != 0)) ok = 0;
    if (!ok) { t->dec = saved; return 0; }
    memcpy(t->cells, s, CELL_MEMORY);
    t->top = 0;
    memset(t->blank_set, 0, sizeof t->blank_set);
    for (int r = 0; r < ROWS; r++) t->stamp[r] = t->clears;
    t->col = p[0]; t->row = p[1]; t->fg = p[2]; t->bg = p[3]; t->shown = p[4]; t->wrap = p[5];
    memcpy(t->palette, p + 6, 768);
    return 1;
}


/* ------------------------------------------------------------- messages */

typedef struct { uint8_t *p; size_t len, cap; } Buf;

/* The address of the message being handled (DEVICES.md 1): a reply has the
 * address of its request. */
static uint32_t to_instance;
static uint8_t to_slot;

static void need(Buf *b, size_t n) {
    if (b->len + n <= b->cap) return;
    size_t cap = b->cap ? b->cap : 256;
    while (cap < b->len + n) cap *= 2;
    uint8_t *p = realloc(b->p, cap);
    if (!p) { fputs("terse-dev-terminal-c: out of memory\n", stderr); exit(1); }
    b->p = p;
    b->cap = cap;
}
static void put8(Buf *b, uint8_t v) { need(b, 1); b->p[b->len++] = v; }
static void put16(Buf *b, uint16_t v) { put8(b, v & 0xFF); put8(b, v >> 8); }
static void put32(Buf *b, uint32_t v) { for (int i = 0; i < 4; i++) put8(b, (v >> (8 * i)) & 0xFF); }
static void putn(Buf *b, const void *s, size_t n) { need(b, n); if (n) memcpy(b->p + b->len, s, n); b->len += n; }
static void putstr(Buf *b, const char *s) { size_t n = strlen(s); put16(b, (uint16_t)n); putn(b, s, n); }
/* Every message is built after 4 bytes left for its length: the address of
 * the message being handled, then the type. */
static void begin(Buf *b, uint8_t type) {
    b->len = 0;
    need(b, 4);
    b->len = 4;
    put32(b, to_instance);
    put8(b, to_slot); put8(b, 0); put16(b, 0);
    put8(b, type);
}

static uint32_t rd32(const uint8_t *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }
static uint16_t rd16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }

/* ------------------------------------------------------------ transport */

static int sock = -1;
static int shm;                      /* the shared-memory transport chosen */
static int sleep_on_socket;          /* sleep on the socket, not the futex */
static uint8_t *bus;                 /* the mapped bus file */
static size_t bus_size;
static uint32_t req_off, req_cap, rep_off, rep_cap, last_seq;
static const uint8_t *pkt;           /* the current request packet */
static size_t pkt_len, pkt_pos;
static Buf in;

/* The adaptive spin (DEVICES.md 4.2): up to twice the longest of the last
 * RECENT waits that ended while spinning, at least MIN_SPIN_US, at most
 * spin_max_us (TERSE_BUS_SPIN_US, 2 ms); MIN_SPIN_US if all of them slept.
 * TERSE_BUS_SPIN=fixed spins spin_max_us every time. */
enum { RECENT = 32, MIN_SPIN_US = 4 };
static uint64_t spin_max_us = 2000;
static int spin_fixed;
static uint32_t recent[RECENT];      /* µs; UINT32_MAX for a wait that slept */
static unsigned recent_next;

static void write_all(const uint8_t *p, size_t n) {
    while (n) {
        ssize_t w = write(sock, p, n);
        if (w < 0 && errno == EINTR) continue;
        if (w <= 0) exit(0); /* the core has gone */
        p += w;
        n -= (size_t)w;
    }
}

/* 1 if all n bytes were read, 0 at end of stream. */
static int read_all(uint8_t *p, size_t n) {
    while (n) {
        ssize_t r = read(sock, p, n);
        if (r < 0 && errno == EINTR) continue;
        if (r <= 0) return 0;
        p += r;
        n -= (size_t)r;
    }
    return 1;
}

static atomic_uint *word(size_t off) { return (atomic_uint *)(void *)(bus + off); }

static void send_msg(Buf *b) {
    uint32_t n = (uint32_t)(b->len - 4);
    for (int i = 0; i < 4; i++) b->p[i] = (n >> (8 * i)) & 0xFF;
    if (!shm) { write_all(b->p, b->len); return; }
    if (b->len > rep_cap) { fputs("terse-dev-terminal-c: reply too long\n", stderr); exit(1); }
    memcpy(bus + rep_off, b->p, b->len);
    atomic_store_explicit(word(132), (unsigned)b->len, memory_order_relaxed);
    atomic_fetch_add_explicit(word(128), 1, memory_order_seq_cst);
    /* Wake the core the way it sleeps (DEVICES.md 4.2): 1 on its address,
     * 2 on the socket (a byte of any value). */
    unsigned how = atomic_load_explicit(word(136), memory_order_seq_cst);
    if (how == 2) {
        char z = 0;
        if (write(sock, &z, 1) < 0) { /* the core has gone: its socket says so */ }
    } else if (how != 0) {
#ifdef __linux__
        syscall(SYS_futex, (void *)word(128), FUTEX_WAKE, INT_MAX, NULL, NULL, 0);
#elif defined(__APPLE__)
        __ulock_wake(UL_COMPARE_AND_WAIT_SHARED | ULF_WAKE_ALL, (void *)word(128), 0);
#endif
    }
}

/* Sends ERROR (to the address of the message being handled) and stops: the
 * core gives the driver up (DEVICES.md 10). */
static void fail(const char *fmt, ...) {
    char text[256];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(text, sizeof text, fmt, ap);
    va_end(ap);
    fprintf(stderr, "terse-dev-terminal-c: %s\n", text);
    Buf b = { 0 };
    begin(&b, 0x7F);
    putn(&b, text, strlen(text));
    if (sock >= 0) send_msg(&b);
    exit(1);
}

static int socket_closed(void) {
    struct pollfd p = { .fd = sock, .events = POLLIN };
    if (poll(&p, 1, 0) <= 0) return 0;
    if (p.revents & (POLLHUP | POLLERR)) return 1;
    char c;
    return recv(sock, &c, 1, MSG_PEEK | MSG_DONTWAIT) == 0;
}

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec;
}

/* How long to spin now, in ns. */
static uint64_t spin_limit_ns(void) {
    if (spin_fixed) return spin_max_us * 1000;
    uint64_t us = MIN_SPIN_US;
    for (int i = 0; i < RECENT; i++)
        if (recent[i] < spin_max_us && 2 * (uint64_t)recent[i] > us) us = 2 * (uint64_t)recent[i];
    return (us < spin_max_us ? us : spin_max_us) * 1000;
}

/* Waits for the core's next request packet (DEVICES.md 4.2): spins for a
 * while, then sleeps on the futex, checking now and then that the core is
 * still there. */
static void shm_wait(void) {
    atomic_uint *seq = word(64), *sleeping = word(72);
    uint64_t start = now_ns(), limit = spin_limit_ns();
    int slept = 0;
    for (unsigned spins = 1; atomic_load_explicit(seq, memory_order_acquire) == last_seq; spins++) {
        if (!slept && limit && ((spins & 63) != 0 || now_ns() - start < limit)) {
#if defined(__x86_64__) || defined(__i386__)
            __builtin_ia32_pause();
#endif
            continue;
        }
        slept = 1;
        /* Say how it sleeps, look again, sleep at most 50 ms. */
        atomic_store_explicit(sleeping, sleep_on_socket ? 2 : 1, memory_order_seq_cst);
        if (atomic_load_explicit(seq, memory_order_seq_cst) == last_seq) {
            if (sleep_on_socket) {
                /* Woken by a byte (dropped) or by the end of the stream. */
                struct pollfd p = { .fd = sock, .events = POLLIN };
                char drop[64];
                if (poll(&p, 1, 50) > 0) {
                    ssize_t k;
                    while ((k = recv(sock, drop, sizeof drop, MSG_DONTWAIT)) > 0) {}
                    if (k == 0) exit(0); /* the end of the stream: the core has gone */
                }
            } else {
#ifdef __linux__
                struct timespec to = { 0, 50000000 };
                syscall(SYS_futex, (void *)seq, FUTEX_WAIT, last_seq, &to, NULL, 0);
#endif
            }
        }
        atomic_store_explicit(sleeping, 0, memory_order_seq_cst);
        if (atomic_load_explicit(seq, memory_order_acquire) == last_seq && socket_closed()) exit(0);
    }
    uint64_t waited_us = (now_ns() - start) / 1000;
    recent[recent_next] = slept ? UINT32_MAX : (uint32_t)(waited_us < UINT32_MAX - 1 ? waited_us : UINT32_MAX - 1);
    recent_next = (recent_next + 1) % RECENT;
    last_seq = atomic_load_explicit(seq, memory_order_acquire);
    pkt_len = atomic_load_explicit(word(68), memory_order_relaxed);
    if (pkt_len > req_cap) fail("request packet longer than the request area");
    pkt = bus + req_off;
    pkt_pos = 0;
}

/* The next message: its address (into to_instance, to_slot) and its body;
 * 0 when the core has gone. */
static int recv_msg(const uint8_t **body, size_t *n) {
    const uint8_t *m;
    uint32_t len;
    if (shm) {
        if (pkt_pos >= pkt_len) shm_wait();
        if (pkt_len - pkt_pos < 4) fail("malformed request packet");
        len = rd32(pkt + pkt_pos);
        if (len > pkt_len - pkt_pos - 4) fail("malformed request packet");
        m = pkt + pkt_pos + 4;
        pkt_pos += 4 + (size_t)len;
    } else {
        uint8_t h[4];
        if (!read_all(h, 4)) return 0;
        len = rd32(h);
        if (len > 16777216) fail("message longer than 16 MiB");
        in.len = 0;
        need(&in, len);
        if (!read_all(in.p, len)) return 0;
        m = in.p;
    }
    if (len < 9) fail("a message without an address or a type");
    to_instance = rd32(m);
    to_slot = m[4];
    *body = m + 8;
    *n = len - 8;
    return 1;
}

static void *map_file(const char *path, size_t *size) {
    int fd = open(path, O_RDWR);
    if (fd < 0) return NULL;
    struct stat st;
    void *p = MAP_FAILED;
    if (fstat(fd, &st) == 0 && st.st_size > 0)
        p = mmap(NULL, (size_t)st.st_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    close(fd);
    if (p == MAP_FAILED) return NULL;
    *size = (size_t)st.st_size;
    return p;
}

/* Maps the bus file and takes the core's sequence number; 0 (and the socket
 * is used) if anything is wrong. */
static int shm_open_bus(const char *bus_path) {
    bus = map_file(bus_path, &bus_size);
    if (!bus) return 0;
    req_off = bus_size >= 4096 ? rd32(bus + 8) : 0; req_cap = bus_size >= 4096 ? rd32(bus + 12) : 0;
    rep_off = bus_size >= 4096 ? rd32(bus + 16) : 0; rep_cap = bus_size >= 4096 ? rd32(bus + 20) : 0;
    if (bus_size < 4096 || memcmp(bus, "TBUS", 4) != 0 || rd32(bus + 4) != 0 ||
        (uint64_t)req_off + req_cap > bus_size || (uint64_t)rep_off + rep_cap > bus_size) {
        munmap(bus, bus_size);
        bus = NULL;
        return 0;
    }
    last_seq = atomic_load_explicit(word(64), memory_order_acquire);
    return 1;
}

/* ------------------------------------------------------------ instances */

/* A machine the driver serves (DEVICES.md 1.2): its terminal and, on shared
 * memory, its memory file (ROM at 0, RAM at 131,072). */
typedef struct {
    uint32_t id;
    Terminal *t;
    uint8_t *mem;
    size_t mem_size;
    uint32_t rom_len, ram_len;
} Instance;

static Instance *insts;
static size_t n_insts, cap_insts;
static Instance *cur;                /* the instance of the message being handled */

static Instance *find(uint32_t id) {
    for (size_t i = 0; i < n_insts; i++)
        if (insts[i].id == id) return &insts[i];
    return NULL;
}

/* CLOSE: the instance's state is dropped and its memory file unmapped. */
static void close_instance(uint32_t id) {
    Instance *i = find(id);
    if (!i) return;
    if (i->mem) munmap(i->mem, i->mem_size);
    free(i->t);
    *i = insts[--n_insts];
    cur = NULL;
    t = NULL;
}

/* Makes instance id the current one (cur and t); 0 if it is not open. */
static int select_instance(uint32_t id) {
    cur = find(id);
    t = cur ? cur->t : NULL;
    return cur != NULL;
}

/* Guest memory of the current instance in its memory file (shared memory
 * only). */
static const uint8_t *guest(uint32_t addr, uint32_t len) {
    static const uint8_t none[1];
    uint64_t a = addr, end = (uint64_t)addr + len;
    if (len == 0) return none;
    if (!cur->mem) fail("instance %u has no memory file", cur->id);
    if (a >= 0x10000 && end <= 0x10000 + (uint64_t)cur->rom_len) return cur->mem + (a - 0x10000);
    if (a >= 0x100000 && end <= 0x100000 + (uint64_t)cur->ram_len) return cur->mem + 131072 + (a - 0x100000);
    fail("buffer %08Xh+%u is not in ROM or RAM", addr, len);
    return NULL;
}

/* Reads a u16-prefixed string at *at into dst (NUL-terminated). */
static int get_str(const uint8_t *m, size_t n, size_t *at, char *dst, size_t cap) {
    if (*at + 2 > n) return 0;
    size_t len = rd16(m + *at);
    if (*at + 2 + len > n || len >= cap) return 0;
    memcpy(dst, m + *at + 2, len);
    dst[len] = 0;
    *at += 2 + len;
    return 1;
}

/* OPEN (DEVICES.md 1.2): a new instance at reset, with its memory file
 * mapped if the driver chose shared memory. Builds the reply in r. */
static void open_instance(const uint8_t *m, size_t n, Buf *r) {
    char mem_path[4096];
    size_t at = 44;
    if (n < at) fail("OPEN too short");
    uint32_t rom_len = rd32(m + 36), ram_len = rd32(m + 40);
    if (!get_str(m, n, &at, mem_path, sizeof mem_path) || at + 1 > n) fail("OPEN: malformed");
    unsigned listed = m[at++];
    if (at + 2 * listed > n) fail("OPEN: malformed");
    int version_ok = 1;
    for (unsigned k = 0; k < listed; k++, at += 2)
        if (m[at] == 2 && m[at + 1] != 0) version_ok = 0;
    /* The identity and the ROM that follow are not needed: on the socket
     * every call brings its buffer, and the memory file holds the ROM. */
    close_instance(to_instance); /* if it was open: it begins again */
    if (!version_ok) {
        begin(r, 0x83); put8(r, 0); put16(r, 0); putstr(r, "the terminal driver implements device 2 version 0");
        return;
    }
    Instance i = { .id = to_instance, .rom_len = rom_len, .ram_len = ram_len };
    if (mem_path[0]) {
        i.mem = map_file(mem_path, &i.mem_size);
        if (!i.mem) fail("%s: cannot map the memory file", mem_path);
        if (i.mem_size < 131072 + (size_t)ram_len || rom_len > 131072) fail("%s: the memory file is too short", mem_path);
    } else if (shm) {
        fail("shared memory without a memory file");
    }
    i.t = malloc(sizeof *i.t);
    if (n_insts == cap_insts) {
        cap_insts = cap_insts ? 2 * cap_insts : 8;
        insts = realloc(insts, cap_insts * sizeof *insts);
    }
    if (!i.t || !insts) { fputs("terse-dev-terminal-c: out of memory\n", stderr); exit(1); }
    insts[n_insts++] = i;
    select_instance(i.id);
    reset_device();
    begin(r, 0x83); put8(r, 1); put16(r, 0); put16(r, 0);
}

/* ---------------------------------------------------------- call table */

/* One 64-byte entry (DEVICES.md 5). */
typedef struct {
    uint8_t fn, flags, addr_arg, len_arg;
    uint32_t scale, least, greatest;
    uint8_t access, checked;
    uint32_t checked_max, len_result, place_result;
    uint8_t rule, rule_arg;
    uint32_t value, value2, charge;
    uint8_t charge_flags, charge_arg;
    uint16_t per_byte;
    uint32_t low, high, units;
} Entry;

enum { F_A1 = 1, F_BATCH = 2, F_BUF = 4 };
enum { A_READ = 1, A_WRITE = 2, A_ROM = 4 };

/* As the Rust driver's table (DEVICES.md 5.1); an entry without a buffer
 * keeps the buffer results FFFF_FFFEh, as CallEntry::plain does. */
#define PLAIN .len_arg = 0xFF, .checked = 0xFF, .len_result = ERR_BUFFER, .place_result = ERR_BUFFER
static const Entry TABLE[] = {
    /* write: a0, a1 x 1, 0-4,096, read, ROM; a0 = a1; 2 per byte */
    { .fn = 0, .flags = F_BUF | F_BATCH, .addr_arg = 0, .len_arg = 1, .scale = 1, .least = 0, .greatest = MAX_WRITE,
      .access = A_READ | A_ROM, .checked = 0xFF,
      .len_result = ERR_BUFFER, .place_result = ERR_BUFFER, .rule = 2, .rule_arg = 1,
      .charge_arg = 0xFF, .per_byte = CHARGE_BYTE },
    /* command: 0; 150 if 1 <= a0 <= 3 */
    { .fn = 1, .flags = F_BATCH, PLAIN, .rule = 1, .charge_arg = 0, .low = 1, .high = 3, .units = CHARGE_OP },
    { .fn = 2, .flags = F_BATCH, PLAIN, .rule = 1, .charge_arg = 0xFF },
    { .fn = 3, .flags = F_A1, PLAIN, .charge_arg = 0xFF },
    { .fn = 4, .flags = F_BATCH, PLAIN, .rule = 1, .charge_arg = 0xFF },
    { .fn = 5, .flags = F_A1, PLAIN, .charge_arg = 0xFF },
    /* cells_read: a1, a2 x 4, 0-9,600, written, RAM; a0 <= 2,399 */
    { .fn = 6, .flags = F_BUF, .addr_arg = 1, .len_arg = 2, .scale = 4, .least = 0, .greatest = CELL_MEMORY,
      .access = A_WRITE, .checked = 0, .checked_max = CELLS - 1,
      .len_result = ERR_BUFFER, .place_result = ERR_BUFFER, .charge_arg = 0xFF },
    /* cells_write: a1, a2 x 4, 0-9,600, read, ROM; a0 <= 2,399; a0 = a2; 1 per 4 bytes */
    { .fn = 7, .flags = F_BATCH | F_BUF, .addr_arg = 1, .len_arg = 2, .scale = 4, .least = 0,
      .greatest = CELL_MEMORY, .access = A_READ | A_ROM, .checked = 0, .checked_max = CELLS - 1,
      .len_result = ERR_BUFFER, .place_result = ERR_BUFFER, .rule = 2, .rule_arg = 2,
      .charge_flags = 1, .charge_arg = 0xFF },
    /* palette_set: a1, a2 x 3, 0-768, read, ROM; a0 <= 255; a0 = a2; 1 per 4 bytes */
    { .fn = 8, .flags = F_BATCH | F_BUF, .addr_arg = 1, .len_arg = 2, .scale = 3, .least = 0,
      .greatest = 768, .access = A_READ | A_ROM, .checked = 0, .checked_max = 255,
      .len_result = ERR_BUFFER, .place_result = ERR_BUFFER, .rule = 2, .rule_arg = 2,
      .charge_flags = 1, .charge_arg = 0xFF },
    /* size: 80 and 30 */
    { .fn = 9, .flags = F_A1 | F_BATCH, PLAIN, .rule = 1, .value = COLS, .value2 = ROWS, .charge_arg = 0xFF },
};
#undef PLAIN
enum { TABLE_LEN = sizeof TABLE / sizeof TABLE[0] };

static void put_entry(Buf *b, const Entry *e) {
    size_t at = b->len;
    put8(b, e->fn); put8(b, e->flags); put8(b, e->addr_arg); put8(b, e->len_arg);
    put32(b, e->scale); put32(b, e->least); put32(b, e->greatest);
    put8(b, e->access); put8(b, e->checked); put16(b, 0);
    put32(b, e->checked_max); put32(b, e->len_result); put32(b, e->place_result);
    put8(b, e->rule); put8(b, e->rule_arg); put16(b, 0);
    put32(b, e->value); put32(b, e->value2); put32(b, e->charge);
    put8(b, e->charge_flags); put8(b, e->charge_arg); put16(b, e->per_byte);
    put32(b, e->low); put32(b, e->high); put32(b, e->units);
    if (b->len - at != 64) fail("call table entry is not 64 bytes");
}

/* ---------------------------------------------------------------- calls */

/* Carries out one CALL (body m, n bytes) on the current instance and builds
 * its RESULT in r. A call from a batch has its bytes inline on both
 * transports. */
static void do_call(const uint8_t *m, size_t n, int batched, Buf *r) {
    if (n < 40) fail("CALL too short");
    if (to_slot != 2) fail("a call for slot %u: this driver serves slot 2", to_slot);
    uint8_t fn = m[1];
    uint32_t a[6];
    for (int i = 0; i < 6; i++) a[i] = rd32(m + 4 + 4 * i);
    /* the budget (m + 28) is not needed: every call's charge follows from its arguments */
    uint32_t nb = rd32(m + 36);
    if (nb > n - 40) fail("CALL buffer bytes cut short");
    const uint8_t *data = m + 40;
    int inline_bytes = !shm || batched;

    uint32_t r0 = 0, r1 = 0, charge = 0;
    int has_r1 = 0;
    const uint8_t *wbytes = NULL;
    uint32_t waddr = 0, wlen = 0;
    static uint8_t out[CELL_MEMORY];

    switch (fn) {
    case 0: { /* write: the whole buffer (the core checked its length) */
        uint32_t len = a[1];
        if (len > MAX_WRITE) { r0 = ERR_BUFFER; break; }
        if (inline_bytes) {
            if (nb < len) fail("write: %u bytes sent, %u needed", nb, len);
        } else {
            data = guest(a[0], len);
        }
        write_bytes(data, len);
        charge = CHARGE_BYTE * len;
        r0 = len;
        break;
    }
    case 1:
        if (a[0] < 0x100) charge = command(a[0]) * CHARGE_OP;
        break;
    case 2:
        t->col = a[0] > COLS - 1 ? COLS - 1 : (uint8_t)a[0];
        t->row = a[1] > ROWS - 1 ? ROWS - 1 : (uint8_t)a[1];
        t->wrap = 0;
        break;
    case 3: r0 = t->col; r1 = t->row; has_r1 = 1; break;
    case 4:
        if (a[0] <= 0xFF) t->fg = (uint8_t)a[0];
        if (a[1] <= 0xFF) t->bg = (uint8_t)a[1];
        break;
    case 5: r0 = t->fg; r1 = t->bg; has_r1 = 1; break;
    case 6:   /* cells_read */
    case 7: { /* cells_write */
        if (a[0] >= CELLS || a[2] > CELLS) { r0 = ERR_BUFFER; break; }
        uint32_t len = a[2] * 4, at = a[0] * 4;
        if (fn == 6) {
            for (uint32_t done = 0; done < len; done++) out[done] = grid_byte((at + done) % CELL_MEMORY);
            wbytes = out; waddr = a[1]; wlen = len;
        } else {
            if (inline_bytes && nb < len) fail("cells_write: %u bytes sent, %u needed", nb, len);
            const uint8_t *src = inline_bytes ? data : guest(a[1], len);
            for (uint32_t done = 0; done < len; done++) set_grid_byte((at + done) % CELL_MEMORY, src[done]);
        }
        charge = (len + 3) / 4;
        r0 = a[2];
        break;
    }
    case 8: { /* palette_set */
        if (a[0] > 255 || a[2] > 256) { r0 = ERR_BUFFER; break; }
        uint32_t len = a[2] * 3;
        if (inline_bytes && nb < len) fail("palette_set: %u bytes sent, %u needed", nb, len);
        const uint8_t *src = inline_bytes ? data : guest(a[1], len);
        for (uint32_t i = 0; i < a[2]; i++) memcpy(&t->palette[(a[0] + i) % 256 * 3], src + 3 * i, 3);
        charge = (len + 3) / 4;
        r0 = a[2];
        break;
    }
    case 9: r0 = COLS; r1 = ROWS; has_r1 = 1; break;
    default: r0 = ERR_UNKNOWN; break;
    }

    begin(r, 0x90);
    put8(r, (uint8_t)has_r1); put16(r, 0);
    put32(r, r0); put32(r, r1); put32(r, charge);
    put32(r, wbytes ? 1 : 0);
    if (wbytes) { put32(r, waddr); put32(r, wlen); putn(r, wbytes, wlen); }
}

/* ----------------------------------------------------------------- main */

/* The identity a HELLO reply ends with (DEVICES.md 1.1): the version and
 * a build identifier, `fnv:` and the FNV-1a 64-bit hash of this program's
 * own executable, computed once at start. */
#define PROGRAM_VERSION "0.1.0"
static char build_id[32] = "unknown";
static void compute_build_id(void) {
#ifdef __APPLE__
    char self[4096];
    uint32_t len = sizeof self;
    FILE *f = _NSGetExecutablePath(self, &len) == 0 ? fopen(self, "rb") : NULL;
#else
    FILE *f = fopen("/proc/self/exe", "rb");
#endif
    if (!f) return;
    uint64_t h = 14695981039346656037ull;
    uint8_t buf[65536];
    size_t k;
    while ((k = fread(buf, 1, sizeof buf, f)) > 0)
        for (size_t i = 0; i < k; i++) { h ^= buf[i]; h *= 1099511628211ull; }
    if (!ferror(f)) snprintf(build_id, sizeof build_id, "fnv:%016llx", (unsigned long long)h);
    fclose(f);
}

/* HELLO (DEVICES.md 1.1): the core must ask for slot 2, and may ask for
 * slot 10 with it (full text, SPEC §terminal.text). */
static void hello(const uint8_t *m, size_t n, Buf *r) {
    char bus_path[4096], session[4096];
    if (n < 9 || m[1] != 1) fail("HELLO: not protocol version 1");
    uint32_t flags = rd32(m + 4);
    size_t at = 9 + (size_t)m[8];
    if (at > n) fail("HELLO: malformed");
    int full_text = m[8] == 2 && ((m[9] == 2 && m[10] == 10) || (m[9] == 10 && m[10] == 2));
    if (!(m[8] == 1 && m[9] == 2) && !full_text) fail("this is the terminal's driver: it serves slot 2 (and 10)");
    if (!get_str(m, n, &at, session, sizeof session) || !get_str(m, n, &at, bus_path, sizeof bus_path))
        fail("HELLO: malformed");
    const char *want = getenv("TERSE_C_SHM");
    int use_shm = want && strcmp(want, "1") == 0 && (flags & 1) && bus_path[0] && shm_open_bus(bus_path);

    begin(r, 0x81);
    put8(r, 1); put8(r, (uint8_t)use_shm); put8(r, 0);
    putstr(r, "terminal (C)");
    put16(r, 0); put16(r, 0);           /* no native size */
    put8(r, full_text ? 2 : 1);         /* one or two slots: */
    put8(r, 2); put8(r, 0); put8(r, 0); /* 2, version 0, no layers, */
    put8(r, TABLE_LEN);                 /* and its call table; */
    for (int i = 0; i < TABLE_LEN; i++) put_entry(r, &TABLE[i]);
    /* 10, full text (SPEC §terminal.text), version 0: a capability with no
     * layers and no calls. This driver draws nothing, so it lacks no
     * character. */
    if (full_text) { put8(r, 10); put8(r, 0); put8(r, 0); put8(r, 0); }
    put8(r, 0); put8(r, 0); put8(r, 0); /* no properties published or subscribed, no services */
    putstr(r, PROGRAM_VERSION); putstr(r, build_id); /* the identity */
    send_msg(r); /* on the socket: shm is not chosen yet */
    shm = use_shm;
}

int main(int argc, char **argv) {
    compute_build_id();
    if (argc > 1 && strcmp(argv[1], "--version") == 0) {
        printf("terse-dev-terminal-c " PROGRAM_VERSION " %s\n", build_id);
        return 0;
    }
    const char *path = getenv("TERSE_BUS_SOCKET"), *cpu = getenv("TERSE_BUS_CPU");
    const char *spin_us = getenv("TERSE_BUS_SPIN_US"), *spin = getenv("TERSE_BUS_SPIN");
    if (!path) { fputs("terse-dev-terminal-c: started without TERSE_BUS_SOCKET\n", stderr); return 1; }
#ifdef __linux__
    if (cpu) {
        cpu_set_t set;
        CPU_ZERO(&set);
        CPU_SET(atoi(cpu), &set);
        sched_setaffinity(0, sizeof set, &set);
    }
    const char *wake = getenv("TERSE_BUS_WAKE");
    sleep_on_socket = wake && strcmp(wake, "socket") == 0;
#else
    (void)cpu;
    sleep_on_socket = 1;
#endif
    if (spin_us && *spin_us) spin_max_us = strtoull(spin_us, NULL, 10);
    spin_fixed = spin && strcmp(spin, "fixed") == 0;
    struct sockaddr_un addr = { .sun_family = AF_UNIX };
    if (strlen(path) >= sizeof addr.sun_path) { fputs("terse-dev-terminal-c: socket path too long\n", stderr); return 1; }
    strcpy(addr.sun_path, path);
    sock = socket(AF_UNIX, SOCK_STREAM, 0);
    if (sock >= 0) fcntl(sock, F_SETFD, FD_CLOEXEC);
    if (sock < 0 || connect(sock, (struct sockaddr *)&addr, sizeof addr) != 0) {
        perror("terse-dev-terminal-c: connect");
        return 1;
    }

    Buf r = { 0 }, scratch = { 0 };
    const uint8_t *m;
    size_t n;
    int greeted = 0;
    while (recv_msg(&m, &n)) {
        uint8_t type = m[0];
        if (!greeted && type != 0x01) fail("expected HELLO, got %02Xh", type);
        /* A message about an instance needs it open; HELLO, OPEN, CLOSE,
         * BATCH, SHUTDOWN and ERROR do not. */
        if (type != 0x01 && type != 0x03 && type != 0x04 && type != 0x11 && type != 0x60 && type != 0x7F &&
            !select_instance(to_instance))
            fail("no instance %u", to_instance);
        switch (type) {
        case 0x01: /* HELLO */
            if (greeted) fail("second HELLO");
            hello(m, n, &r);
            greeted = 1;
            break;
        case 0x02: /* RESET */
            reset_device();
            begin(&r, 0x82); send_msg(&r);
            break;
        case 0x03: /* OPEN */
            open_instance(m, n, &r);
            send_msg(&r);
            break;
        case 0x04: /* CLOSE: no reply */
            close_instance(to_instance);
            break;
        case 0x10: /* CALL */
            do_call(m, n, 0, &r);
            send_msg(&r);
            break;
        case 0x11: { /* BATCH: each call (a blob: address and body) carried out, none answered */
            if (n < 8) fail("BATCH too short");
            uint32_t count = rd32(m + 4);
            size_t at = 8;
            for (uint32_t i = 0; i < count; i++) {
                if (n - at < 4) fail("BATCH cut short");
                uint32_t len = rd32(m + at);
                if (len > n - at - 4 || len < 9 || m[at + 12] != 0x10) fail("BATCH: malformed CALL");
                to_instance = rd32(m + at + 4);
                to_slot = m[at + 8];
                if (!select_instance(to_instance)) fail("a batched call for no instance %u", to_instance);
                do_call(m + at + 12, len - 8, 1, &scratch);
                at += 4 + (size_t)len;
            }
            break;
        }
        case 0x30: /* BEGIN: the terminal takes no input */
            begin(&r, 0xB0);
            put8(&r, 0); put16(&r, 0);
            put32(&r, 0); /* charge */
            put32(&r, 0); /* events */
            put32(&r, 0); /* writes */
            put32(&r, 0); /* input record */
            send_msg(&r);
            break;
        case 0x31: /* END */
            break;
        case 0x40: { /* STATE */
            if (to_slot != 2) fail("STATE of slot %u: this driver serves slot 2", to_slot);
            uint8_t s[STATE_LEN];
            write_state(s);
            begin(&r, 0xC0);
            put8(&r, 0); put16(&r, 0);
            put32(&r, STATE_LEN);
            putn(&r, s, STATE_LEN);
            send_msg(&r);
            break;
        }
        case 0x41: { /* RESTORE: the section from the start of the rest of a serialization */
            if (to_slot != 2) fail("RESTORE of slot %u: this driver serves slot 2", to_slot);
            int ok = n >= 8 && rd32(m + 4) == n - 8 && restore_state(m + 8, n - 8);
            begin(&r, 0xC1);
            put8(&r, (uint8_t)ok); put16(&r, 0);
            put32(&r, ok ? STATE_LEN : 0);
            send_msg(&r);
            break;
        }
        case 0x42: /* GET: it publishes no property */
            begin(&r, 0xC2); put8(&r, 0); send_msg(&r);
            break;
        case 0x43: /* SET: it subscribes to none */
            break;
        case 0x44: /* SERVICE: it gives none */
            begin(&r, 0xC4);
            put8(&r, 0); putstr(&r, "the terminal driver gives no services"); put32(&r, 0);
            send_msg(&r);
            break;
        case 0x50: /* PRESENT: no layers */
            begin(&r, 0xD0);
            put8(&r, 0); put16(&r, 0);
            send_msg(&r);
            break;
        case 0x60: /* SHUTDOWN */
            return 0;
        case 0x7F: /* ERROR from the core: it is about to close */
            fprintf(stderr, "terse-dev-terminal-c: core error: %.*s\n", (int)(n - 1), (const char *)m + 1);
            return 1;
        default:
            fail("unknown message type %02Xh", type);
        }
    }
    return 0;
}
