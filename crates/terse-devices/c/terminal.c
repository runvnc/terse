/*
 * Build: clang -std=c11 -O2 -Wall -Wextra -o terse-dev-terminal-c terminal.c
 *
 * Terse device 2, the terminal (SPEC.md 8, version 0), as a device driver
 * of the device bus (DEVICES.md), in C11 with libc only. It draws nothing:
 * no layers, no native size. It keeps the device state and answers the
 * calls with their exact results and charges (SPEC.md 14), so that a run
 * with it gives the same state hashes (STATE.md 1.6) as the built-in
 * terminal of crates/terse-core.
 *
 * Environment: TERSE_BUS_SOCKET (the core's socket), TERSE_BUS_SLOT (must
 * be 2). With TERSE_C_SHM=1 the driver chooses the shared-memory transport
 * (DEVICES.md 4) when the core offers it; otherwise the socket.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/futex.h>
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
#include <sys/syscall.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

/* ---------------------------------------------------------------- device */

enum { COLS = 80, ROWS = 30, CELLS = COLS * ROWS, CELL_MEMORY = CELLS * 4 };
enum { CONTINUATION = 0xFFFF, REPLACEMENT = 0xFFFD, SPACE = 0x0020 };
enum { STATE_LEN = CELL_MEMORY + 6 + 768 + 4 }; /* 10,378 */
enum { CHARGE_OP = 150 };                      /* a clear or scroll */
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

/* The UTF-8 decoder of `write` (SPEC 8.3): WHATWG's, keeping the bytes of
 * the incomplete sequence (STATE.md 1.6). */
typedef struct {
    uint32_t cp;
    uint8_t needed, lower, upper, len;
    uint8_t bytes[3];
} Decoder;

static struct {
    uint8_t cells[CELL_MEMORY];
    uint8_t col, row, fg, bg, shown, wrap;
    uint8_t palette[768];
    Decoder dec;
} t;

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
    t.dec = (Decoder){ .cp = bits, .needed = needed, .lower = lower, .upper = upper,
                       .len = 1, .bytes = { b, 0, 0 } };
    return 0;
}

/* Feeds one byte; returns how many characters (0-2) completed into out. */
static int dec_feed(uint8_t b, uint32_t out[2]) {
    Decoder *d = &t.dec;
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

static uint8_t *cell(int col, int row) { return &t.cells[(row * COLS + col) * 4]; }
static unsigned ch_at(int col, int row) { uint8_t *c = cell(col, row); return c[0] | (unsigned)c[1] << 8; }
static void set_ch(int col, int row, unsigned ch) { uint8_t *c = cell(col, row); c[0] = ch & 0xFF; c[1] = ch >> 8; }
static void set_cell(int col, int row, unsigned ch) {
    uint8_t *c = cell(col, row);
    c[0] = ch & 0xFF; c[1] = ch >> 8; c[2] = t.fg; c[3] = t.bg;
}

/* Cells col and col + 1 of row hold a wide pair. */
static int is_pair(int col, int row) {
    return col + 1 < COLS && ch_at(col, row) != CONTINUATION && ch_at(col + 1, row) == CONTINUATION;
}

static void fill_rows(int from, int to) {
    for (int r = from; r < to; r++)
        for (int c = 0; c < COLS; c++) set_cell(c, r, SPACE);
}

static unsigned clear_grid(void) {
    fill_rows(0, ROWS);
    t.col = t.row = 0;
    t.wrap = 0;
    return 1;
}

static unsigned scroll_up(void) {
    memmove(t.cells, t.cells + COLS * 4, CELL_MEMORY - COLS * 4);
    fill_rows(ROWS - 1, ROWS);
    return 1;
}

static unsigned scroll_down(void) {
    memmove(t.cells + COLS * 4, t.cells, CELL_MEMORY - COLS * 4);
    fill_rows(0, 1);
    return 1;
}

static unsigned next_row(void) {
    if (t.row == ROWS - 1) return scroll_up();
    t.row++;
    return 0;
}

/* Writes n (1 or 2) codes from col in the cursor row; a wide pair cut in
 * two loses its other half, which becomes a space keeping its colours
 * (SPEC 8.3 step 3). */
static void write_cells(int col, const unsigned *chars, int n) {
    int row = t.row, end = col + n;
    int left = col > 0 && is_pair(col - 1, row) ? col - 1 : -1;
    int right = is_pair(end - 1, row) ? end : -1;
    if (left >= 0) set_ch(left, row, SPACE);
    if (right >= 0) set_ch(right, row, SPACE);
    for (int i = 0; i < n; i++) set_cell(col + i, row, chars[i]);
}

/* SPEC 8.3 steps 1-4; returns the scrolls done. */
static unsigned print_char(unsigned ch) {
    int width = is_wide(ch) ? 2 : 1;
    unsigned ops = 0;
    if (t.wrap || (width == 2 && t.col == COLS - 1)) {
        if (!t.wrap) {
            unsigned sp = SPACE;
            write_cells(COLS - 1, &sp, 1);
        }
        t.wrap = 0;
        t.col = 0;
        ops = next_row();
    }
    int col = t.col;
    unsigned pair[2] = { ch, CONTINUATION };
    write_cells(col, pair, width);
    if (col + width > COLS - 1) {
        t.col = COLS - 1;
        t.wrap = 1;
    } else {
        t.col = (uint8_t)(col + width);
    }
    return ops;
}

static unsigned put_char(uint32_t cp) {
    unsigned ops = 0;
    switch (cp) {
    case 0x08: if (t.col > 0) t.col--; break;
    case 0x09: t.col = (uint8_t)((t.col / 8 + 1) * 8); if (t.col > COLS - 1) t.col = COLS - 1; break;
    case 0x0A: t.col = 0; ops = next_row(); break;
    case 0x0C: ops = clear_grid(); break;
    case 0x0D: t.col = 0; break;
    default:
        if (cp < 0x20 || cp == 0x7F) return 0; /* ignored: pending wrap stays */
        return print_char(cp > 0xFFFF || cp == CONTINUATION ? REPLACEMENT : cp);
    }
    t.wrap = 0;
    return ops;
}

static unsigned put_byte(uint8_t b) {
    uint32_t cps[2];
    int n = dec_feed(b, cps);
    unsigned ops = 0;
    for (int i = 0; i < n; i++) ops += put_char(cps[i]);
    return ops;
}

/* `write` (SPEC 8.2): stops after the byte whose charge brings the budget
 * to zero or below. Returns the bytes consumed; *charged the units. */
static uint32_t write_limited(const uint8_t *bytes, uint32_t n, int64_t budget, uint32_t *charged) {
    int64_t c = 0;
    for (uint32_t i = 0; i < n; i++) {
        c += 1 + (int64_t)put_byte(bytes[i]) * CHARGE_OP;
        if (budget - c <= 0) { *charged = (uint32_t)c; return i + 1; }
    }
    *charged = (uint32_t)c;
    return n;
}

static unsigned command(uint32_t v) {
    switch (v) {
    case 1: return clear_grid();
    case 2: return scroll_up();
    case 3: return scroll_down();
    case 4: {
        int col = t.col, row = t.row;
        if (col > 0 && is_pair(col - 1, row)) set_ch(col - 1, row, SPACE);
        for (int c = col; c < COLS; c++) set_cell(c, row, SPACE);
        return 0;
    }
    case 5: t.shown = 1; return 0;
    case 6: t.shown = 0; return 0;
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
    memset(&t, 0, sizeof t);
    t.fg = 7;
    t.shown = 1;
    fill_rows(0, ROWS);
    memcpy(t.palette, base, sizeof base);
    for (int i = 0; i < 216; i++) {
        uint8_t *p = &t.palette[(16 + i) * 3];
        p[0] = cube[i / 36]; p[1] = cube[i / 6 % 6]; p[2] = cube[i % 6];
    }
    for (int i = 0; i < 24; i++) memset(&t.palette[(232 + i) * 3], 8 + 10 * i, 3);
}

/* STATE.md 1.6, the bytes after the present flag. */
static void write_state(uint8_t out[STATE_LEN]) {
    uint8_t *p = out;
    memcpy(p, t.cells, CELL_MEMORY); p += CELL_MEMORY;
    *p++ = t.col; *p++ = t.row; *p++ = t.fg; *p++ = t.bg; *p++ = t.shown; *p++ = t.wrap;
    memcpy(p, t.palette, 768); p += 768;
    *p++ = t.dec.len;
    memcpy(p, t.dec.bytes, 3);
}

/* STATE.md 4: takes the section only if serializing it again gives the same
 * bytes; otherwise nothing changes. */
static int restore_state(const uint8_t *s, size_t n) {
    if (n != STATE_LEN) return 0;
    const uint8_t *p = s + CELL_MEMORY;
    if (p[0] >= COLS || p[1] >= ROWS || p[4] > 1 || p[5] > 1) return 0;
    uint8_t k = p[6 + 768];
    const uint8_t *seq = p + 6 + 768 + 1;
    if (k > 3) return 0;
    Decoder saved = t.dec;
    memset(&t.dec, 0, sizeof t.dec);
    int ok = 1;
    for (int i = 0; i < k && ok; i++) {
        uint32_t cps[2];
        if (dec_feed(seq[i], cps) != 0) ok = 0;
    }
    if (ok && (t.dec.len != k || memcmp(t.dec.bytes, seq, 3) != 0)) ok = 0;
    if (!ok) { t.dec = saved; return 0; }
    memcpy(t.cells, s, CELL_MEMORY);
    t.col = p[0]; t.row = p[1]; t.fg = p[2]; t.bg = p[3]; t.shown = p[4]; t.wrap = p[5];
    memcpy(t.palette, p + 6, 768);
    return 1;
}

/* ------------------------------------------------------------- messages */

typedef struct { uint8_t *p; size_t len, cap; } Buf;

static void need(Buf *b, size_t n) {
    if (b->len + n <= b->cap) return;
    size_t cap = b->cap ? b->cap : 256;
    while (cap < b->len + n) cap *= 2;
    uint8_t *p = realloc(b->p, cap);
    if (!p) { fputs("terse-dev-terminal-c: out of memory\n", stderr); exit(1); }
    b->p = p;
    b->cap = cap;
}
/* Every message is built after 4 bytes left for its length. */
static void begin(Buf *b) { b->len = 0; need(b, 4); b->len = 4; }
static void put8(Buf *b, uint8_t v) { need(b, 1); b->p[b->len++] = v; }
static void put16(Buf *b, uint16_t v) { put8(b, v & 0xFF); put8(b, v >> 8); }
static void put32(Buf *b, uint32_t v) { for (int i = 0; i < 4; i++) put8(b, (v >> (8 * i)) & 0xFF); }
static void putn(Buf *b, const void *s, size_t n) { need(b, n); if (n) memcpy(b->p + b->len, s, n); b->len += n; }
static void putstr(Buf *b, const char *s) { size_t n = strlen(s); put16(b, (uint16_t)n); putn(b, s, n); }

static uint32_t rd32(const uint8_t *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }
static uint16_t rd16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }
static int64_t rd64(const uint8_t *p) { return (int64_t)((uint64_t)rd32(p) | (uint64_t)rd32(p + 4) << 32); }

/* ------------------------------------------------------------ transport */

static int sock = -1;
static int shm;                      /* the shared-memory transport chosen */
static uint8_t *bus, *mem;           /* mapped bus file and memory file */
static size_t bus_size;
static uint32_t rom_len, ram_len;
static uint32_t req_off, req_cap, rep_off, rep_cap, last_seq;
static const uint8_t *pkt;           /* the current request packet */
static size_t pkt_len, pkt_pos;
static Buf in;

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
    if (atomic_load_explicit(word(136), memory_order_seq_cst) == 1)
        syscall(SYS_futex, (void *)word(128), FUTEX_WAKE, INT_MAX, NULL, NULL, 0);
}

/* Sends ERROR and stops: the core stops the machine with the message. */
static void fail(const char *fmt, ...) {
    char text[256];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(text, sizeof text, fmt, ap);
    va_end(ap);
    fprintf(stderr, "terse-dev-terminal-c: %s\n", text);
    Buf b = { 0 };
    begin(&b);
    put8(&b, 0x7F);
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

/* Waits for the core's next request packet (DEVICES.md 4.2): spins up to
 * 2 ms, then sleeps on the futex, checking now and then that the core is
 * still there. */
static void shm_wait(void) {
    atomic_uint *seq = word(64), *sleeping = word(72);
    uint64_t start = now_ns();
    for (unsigned spins = 0; atomic_load_explicit(seq, memory_order_acquire) == last_seq; spins++) {
        if ((spins & 255) == 255 && now_ns() - start > 2000000) {
            atomic_store_explicit(sleeping, 1, memory_order_seq_cst);
            while (atomic_load_explicit(seq, memory_order_seq_cst) == last_seq) {
                struct timespec to = { 0, 100000000 };
                syscall(SYS_futex, (void *)seq, FUTEX_WAIT, last_seq, &to, NULL, 0);
                if (atomic_load_explicit(seq, memory_order_acquire) == last_seq && socket_closed()) exit(0);
            }
            atomic_store_explicit(sleeping, 0, memory_order_relaxed);
            break;
        }
#if defined(__x86_64__) || defined(__i386__)
        __builtin_ia32_pause();
#endif
    }
    last_seq = atomic_load_explicit(seq, memory_order_acquire);
    pkt_len = atomic_load_explicit(word(68), memory_order_relaxed);
    if (pkt_len > req_cap) fail("request packet longer than the request area");
    pkt = bus + req_off;
    pkt_pos = 0;
}

/* The next message's body; 0 when the core has gone. */
static int recv_msg(const uint8_t **body, size_t *n) {
    uint32_t len;
    if (shm) {
        if (pkt_pos >= pkt_len) shm_wait();
        if (pkt_len - pkt_pos < 4) fail("malformed request packet");
        len = rd32(pkt + pkt_pos);
        if (len > pkt_len - pkt_pos - 4) fail("malformed request packet");
        *body = pkt + pkt_pos + 4;
        pkt_pos += 4 + (size_t)len;
    } else {
        uint8_t h[4];
        if (!read_all(h, 4)) return 0;
        len = rd32(h);
        if (len > 16777216) fail("message longer than 16 MiB");
        in.len = 0;
        need(&in, len);
        if (!read_all(in.p, len)) return 0;
        *body = in.p;
    }
    if (len == 0) fail("empty message");
    *n = len;
    return 1;
}

/* Guest memory in the memory file (shared memory only). */
static const uint8_t *guest(uint32_t addr, uint32_t len) {
    static const uint8_t none[1];
    uint64_t a = addr, end = (uint64_t)addr + len;
    if (len == 0) return none;
    if (a >= 0x10000 && end <= 0x10000 + (uint64_t)rom_len) return mem + (a - 0x10000);
    if (a >= 0x100000 && end <= 0x100000 + (uint64_t)ram_len) return mem + 131072 + (a - 0x100000);
    fail("buffer %08Xh+%u is not in ROM or RAM", addr, len);
    return NULL;
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

/* Maps both files; 0 (and the socket is used) if anything is wrong. */
static int shm_open_files(const char *mem_path, const char *bus_path) {
    size_t mem_size;
    mem = map_file(mem_path, &mem_size);
    if (!mem || mem_size < 131072 + (size_t)ram_len || rom_len > 131072) return 0;
    bus = map_file(bus_path, &bus_size);
    if (!bus || bus_size < 4096 || memcmp(bus, "TBUS", 4) != 0 || rd32(bus + 4) != 0) return 0;
    req_off = rd32(bus + 8); req_cap = rd32(bus + 12);
    rep_off = rd32(bus + 16); rep_cap = rd32(bus + 20);
    if ((uint64_t)req_off + req_cap > bus_size || (uint64_t)rep_off + rep_cap > bus_size) return 0;
    last_seq = atomic_load_explicit(word(64), memory_order_acquire);
    return 1;
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
    uint32_t low, high, units;
} Entry;

enum { F_A1 = 1, F_BATCH = 2, F_BUF = 4 };
enum { A_READ = 1, A_WRITE = 2, A_ROM = 4, A_BUDGET = 8 };

static const Entry TABLE[] = {
    /* write: a0, a1 x 1, any length, read, ROM, budget-bounded */
    { .fn = 0, .flags = F_BUF, .addr_arg = 0, .len_arg = 1, .scale = 1, .least = 0, .greatest = 0xFFFFFFFF,
      .access = A_READ | A_ROM | A_BUDGET, .checked = 0xFF,
      .len_result = ERR_BUFFER, .place_result = ERR_BUFFER, .charge_arg = 0xFF },
    /* command: 0; 150 if 1 <= a0 <= 3 */
    { .fn = 1, .flags = F_BATCH, .len_arg = 0xFF, .checked = 0xFF, .rule = 1,
      .charge_arg = 0, .low = 1, .high = 3, .units = CHARGE_OP },
    { .fn = 2, .flags = F_BATCH, .len_arg = 0xFF, .checked = 0xFF, .rule = 1, .charge_arg = 0xFF },
    { .fn = 3, .flags = F_A1, .len_arg = 0xFF, .checked = 0xFF, .charge_arg = 0xFF },
    { .fn = 4, .flags = F_BATCH, .len_arg = 0xFF, .checked = 0xFF, .rule = 1, .charge_arg = 0xFF },
    { .fn = 5, .flags = F_A1, .len_arg = 0xFF, .checked = 0xFF, .charge_arg = 0xFF },
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
    { .fn = 9, .flags = F_A1 | F_BATCH, .len_arg = 0xFF, .checked = 0xFF, .rule = 1, .value = COLS,
      .value2 = ROWS, .charge_arg = 0xFF },
};
enum { TABLE_LEN = sizeof TABLE / sizeof TABLE[0] };

static void put_entry(Buf *b, const Entry *e) {
    size_t at = b->len;
    put8(b, e->fn); put8(b, e->flags); put8(b, e->addr_arg); put8(b, e->len_arg);
    put32(b, e->scale); put32(b, e->least); put32(b, e->greatest);
    put8(b, e->access); put8(b, e->checked); put16(b, 0);
    put32(b, e->checked_max); put32(b, e->len_result); put32(b, e->place_result);
    put8(b, e->rule); put8(b, e->rule_arg); put16(b, 0);
    put32(b, e->value); put32(b, e->value2); put32(b, e->charge);
    put8(b, e->charge_flags); put8(b, e->charge_arg); put16(b, 0);
    put32(b, e->low); put32(b, e->high); put32(b, e->units);
    if (b->len - at != 64) fail("call table entry is not 64 bytes");
}

/* ---------------------------------------------------------------- calls */

/* Carries out one CALL (body m, n bytes) and builds its RESULT in r. A call
 * from a batch has its bytes inline on both transports. */
static void do_call(const uint8_t *m, size_t n, int batched, Buf *r) {
    if (n < 40) fail("CALL too short");
    uint8_t fn = m[1];
    uint32_t a[6];
    for (int i = 0; i < 6; i++) a[i] = rd32(m + 4 + 4 * i);
    int64_t budget = rd64(m + 28);
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
    case 0: { /* write */
        uint32_t len = a[1], avail = len;
        if (inline_bytes) {
            /* The core sends at most max(1, budget) bytes (access bit 3). */
            uint64_t want = len == 0 ? 0 : budget < 1 ? 1 : (uint64_t)budget < len ? (uint64_t)budget : len;
            if (nb < want) fail("write: %u bytes sent, %llu needed", nb, (unsigned long long)want);
            avail = nb < len ? nb : len;
        } else {
            data = guest(a[0], len);
        }
        r0 = write_limited(data, avail, budget, &charge);
        if (r0 == avail && avail < len && budget - (int64_t)charge > 0)
            fail("write: buffer cut short before the budget was used");
        break;
    }
    case 1:
        if (a[0] < 0x100) charge = command(a[0]) * CHARGE_OP;
        break;
    case 2:
        t.col = a[0] > COLS - 1 ? COLS - 1 : (uint8_t)a[0];
        t.row = a[1] > ROWS - 1 ? ROWS - 1 : (uint8_t)a[1];
        t.wrap = 0;
        break;
    case 3: r0 = t.col; r1 = t.row; has_r1 = 1; break;
    case 4:
        if (a[0] <= 0xFF) t.fg = (uint8_t)a[0];
        if (a[1] <= 0xFF) t.bg = (uint8_t)a[1];
        break;
    case 5: r0 = t.fg; r1 = t.bg; has_r1 = 1; break;
    case 6:   /* cells_read */
    case 7: { /* cells_write */
        if (a[0] >= CELLS || a[2] > CELLS) { r0 = ERR_BUFFER; break; }
        uint32_t len = a[2] * 4, at = a[0] * 4;
        if (fn == 6) {
            for (uint32_t done = 0; done < len; done++) out[done] = t.cells[(at + done) % CELL_MEMORY];
            wbytes = out; waddr = a[1]; wlen = len;
        } else {
            if (inline_bytes && nb < len) fail("cells_write: %u bytes sent, %u needed", nb, len);
            const uint8_t *src = inline_bytes ? data : guest(a[1], len);
            for (uint32_t done = 0; done < len; done++) t.cells[(at + done) % CELL_MEMORY] = src[done];
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
        for (uint32_t i = 0; i < a[2]; i++) memcpy(&t.palette[(a[0] + i) % 256 * 3], src + 3 * i, 3);
        charge = (len + 3) / 4;
        r0 = a[2];
        break;
    }
    case 9: r0 = COLS; r1 = ROWS; has_r1 = 1; break;
    default: r0 = ERR_UNKNOWN; break;
    }

    begin(r);
    put8(r, 0x90); put8(r, (uint8_t)has_r1); put16(r, 0);
    put32(r, r0); put32(r, r1); put32(r, charge);
    put32(r, wbytes ? 1 : 0);
    if (wbytes) { put32(r, waddr); put32(r, wlen); putn(r, wbytes, wlen); }
}

/* ----------------------------------------------------------------- main */

/* Reads a u16-prefixed string of HELLO into dst (NUL-terminated). */
static int hello_str(const uint8_t *m, size_t n, size_t *at, char *dst, size_t cap) {
    if (*at + 2 > n) return 0;
    size_t len = rd16(m + *at);
    if (*at + 2 + len > n || len >= cap) return 0;
    memcpy(dst, m + *at + 2, len);
    dst[len] = 0;
    *at += 2 + len;
    return 1;
}

static void hello(const uint8_t *m, size_t n, Buf *r) {
    char mem_path[4096], bus_path[4096], session[4096];
    size_t at = 48;
    if (n < 48 || m[1] != 0) fail("HELLO: unknown protocol");
    if (m[2] != 2) fail("this is the terminal's driver (slot 2), not slot %u's", m[2]);
    if (m[3] != 0) fail("the terminal driver implements version 0, not %u", m[3]);
    uint32_t flags = rd32(m + 4);
    rom_len = rd32(m + 40);
    ram_len = rd32(m + 44);
    if (!hello_str(m, n, &at, mem_path, sizeof mem_path) || !hello_str(m, n, &at, bus_path, sizeof bus_path) ||
        !hello_str(m, n, &at, session, sizeof session))
        fail("HELLO: malformed");
    const char *want = getenv("TERSE_C_SHM");
    int use_shm = want && strcmp(want, "1") == 0 && (flags & 1) && mem_path[0] && bus_path[0] &&
                  shm_open_files(mem_path, bus_path);

    begin(r);
    put8(r, 0x81); put8(r, 0); put8(r, 2); put8(r, 0);
    put8(r, (uint8_t)use_shm);
    putstr(r, "terminal (C)");
    put16(r, 0); put16(r, 0); /* no native size */
    put8(r, 0);               /* no layers */
    put8(r, TABLE_LEN);
    for (int i = 0; i < TABLE_LEN; i++) put_entry(r, &TABLE[i]);
    send_msg(r); /* on the socket: shm is not chosen yet */
    shm = use_shm;
}

int main(void) {
    const char *path = getenv("TERSE_BUS_SOCKET"), *slot = getenv("TERSE_BUS_SLOT");
    if (!path || !slot) { fputs("terse-dev-terminal-c: started without TERSE_BUS_SOCKET\n", stderr); return 1; }
    if (strcmp(slot, "2") != 0) { fprintf(stderr, "terse-dev-terminal-c: slot %s is not 2\n", slot); return 1; }
    struct sockaddr_un addr = { .sun_family = AF_UNIX };
    if (strlen(path) >= sizeof addr.sun_path) { fputs("terse-dev-terminal-c: socket path too long\n", stderr); return 1; }
    strcpy(addr.sun_path, path);
    sock = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (sock < 0 || connect(sock, (struct sockaddr *)&addr, sizeof addr) != 0) {
        perror("terse-dev-terminal-c: connect");
        return 1;
    }
    reset_device();

    Buf r = { 0 }, scratch = { 0 };
    const uint8_t *m;
    size_t n;
    int greeted = 0;
    while (recv_msg(&m, &n)) {
        uint8_t type = m[0];
        if (!greeted && type != 0x01) fail("expected HELLO, got %02Xh", type);
        switch (type) {
        case 0x01: /* HELLO */
            if (greeted) fail("second HELLO");
            hello(m, n, &r);
            greeted = 1;
            break;
        case 0x02: /* RESET */
            reset_device();
            begin(&r); put8(&r, 0x82); send_msg(&r);
            break;
        case 0x10: /* CALL */
            do_call(m, n, 0, &r);
            send_msg(&r);
            break;
        case 0x11: { /* BATCH: each call carried out, none answered */
            if (n < 8) fail("BATCH too short");
            uint32_t count = rd32(m + 4);
            size_t at = 8;
            for (uint32_t i = 0; i < count; i++) {
                if (n - at < 4) fail("BATCH cut short");
                uint32_t len = rd32(m + at);
                if (len > n - at - 4 || len < 1 || m[at + 4] != 0x10) fail("BATCH: malformed CALL");
                do_call(m + at + 4, len, 1, &scratch);
                at += 4 + (size_t)len;
            }
            break;
        }
        case 0x30: /* BEGIN: the terminal takes no input */
            begin(&r);
            put8(&r, 0xB0); put8(&r, 0); put16(&r, 0);
            put32(&r, 0); /* charge */
            put32(&r, 0); /* events */
            put32(&r, 0); /* writes */
            put32(&r, 0); /* input record */
            send_msg(&r);
            break;
        case 0x31: /* END */
            break;
        case 0x40: { /* STATE */
            uint8_t s[STATE_LEN];
            write_state(s);
            begin(&r);
            put8(&r, 0xC0); put8(&r, 0); put16(&r, 0);
            put32(&r, STATE_LEN);
            putn(&r, s, STATE_LEN);
            send_msg(&r);
            break;
        }
        case 0x41: { /* RESTORE */
            int ok = n >= 8 && rd32(m + 4) == n - 8 && restore_state(m + 8, n - 8);
            begin(&r);
            put8(&r, 0xC1); put8(&r, (uint8_t)ok); put16(&r, 0);
            send_msg(&r);
            break;
        }
        case 0x50: /* PRESENT: no layers, no properties */
            begin(&r);
            put8(&r, 0xD0); put8(&r, 0); put16(&r, 0);
            put32(&r, 0);
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
