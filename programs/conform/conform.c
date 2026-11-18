/* conform.c: the Terse conformance image (SPEC 13), RV32IMC.
 *
 * Run with conform.events (README, "Conformance image"). Each section checks
 * what the program itself can observe and records one PASS or FAIL line; a
 * FAIL line says how many checks failed and the number of the first, as in
 * "(2 failed, first 7)". Checks are numbered from 1 in the order the section
 * makes them (for the CPU sections, in the order they appear in cpu.S;
 * wait_until counts as a check). The authoritative check is
 * the state hash every 10 frames (expected-hashes.txt, STATE.md), which also
 * covers what the program cannot see (the palette, the display list, the
 * registers). After the checks comes a stress phase (full-budget frames that
 * rewrite every cell and submit a new 16,384-byte display list each frame),
 * then the page device, then the results screen. Esc exits with the number
 * of failed sections.
 *
 * The page checks hold on any host, whatever its fonts: an event arrives,
 * the same document at the same width gives the same height, a narrower
 * rectangle a height no smaller, a clear 0, presses over the big anchored
 * text at the top left give its id and buttons. The exact heights and the
 * frames they arrive in (the reference layout, SPEC 11.6, in a headless
 * run) are in RAM, so the state hashes cover them.
 *
 * Self-contained: its own crt0.S and conform.ld, no programs/lib, so that
 * library changes do not change this image.
 *
 * Frames (the scripted input of conform.events is in the frames named):
 *   0-5     CPU, RAM code, terminal, vector display, system calls
 *   6       the events of frames 2-5;  9: the 70 text events of frame 8
 *   10-13   controllers
 *   14-31   the budget: timed loops (ROM, RAM, scroll carry, write)
 *   32-45   content requests (completions arrive one frame after submit)
 *   49-108  stress (STRESS_START + 1, STRESS_A + STRESS_B frames)
 *   110     page calls (PAGE_START); the frame ends with a document
 *   111-118 height events (headless: one a frame in 111-114, none in
 *           115-117 while the width is 0, one in 118)
 *   140     pointer presses over the page (PAGE_CLICK), read at 141;
 *           a clear, its height event at 142
 *   142-    the results; Esc exits. */

#include <stdint.h>
#include <stddef.h>

/* ---------------------------------------------------------------- machine */

/* SPEC 1.4, 11 and 13: the provisional values. The budget section predicts
 * its loop counts from these (model_*), so if the machine's constants are
 * retuned, change them here, rebuild (build.sh) and regenerate the expected
 * files (UPDATE_GOLDEN=1 cargo test -p terse-cli --test conform). */
#define BUDGET 125000           /* units per frame */
#define ROM_COST 1              /* an instruction fetched from ROM */
#define RAM_COST 4              /* an instruction fetched from RAM */
#define CHARGE_CALL 4           /* every call, on top of the ecall */
#define CHARGE_WRITE_BYTE 1     /* terminal write, per byte consumed */
#define CHARGE_SCROLL 150       /* a terminal clear or scroll */
#define COPY_BYTES 4            /* bytes copied per unit, rounded up */

#define ROM_BASE 0x00010000u
#define RAM_BASE 0x00100000u
#define RAM_PAGES 16            /* build.sh packs the image with this */
#define RAM_END (RAM_BASE + RAM_PAGES * 8192u)
/* The last 16 KiB of RAM, below which the stack starts (conform.ld). */
#define TOP (RAM_END - 16384u)
extern char __rom_end[];
#define ROM_END ((uint32_t)__rom_end)

#define ERR_UNKNOWN 0xFFFFFFFFu
#define ERR_BUFFER 0xFFFFFFFEu

enum {
    WAIT_FRAME = 0x100, NEXT_EVENT, FRAME, TIME, RANDOM, CONTROLLER, EXIT,
    T_WRITE = 0x200, T_COMMAND, T_SET_CURSOR, T_GET_CURSOR, T_SET_COLOURS,
    T_GET_COLOURS, T_CELLS_READ, T_CELLS_WRITE, T_PALETTE_SET, T_SIZE,
    V_SUBMIT = 0x300, V_CLEAR, V_STYLE, V_PERSISTENCE, V_FILL,
    C_SUBMIT = 0x400,
    P_SUBMIT = 0x700, P_CLEAR, P_REGION, P_SCROLL, P_BACKGROUND,
};

static inline uint32_t call(uint32_t n, uint32_t x, uint32_t y, uint32_t z)
{
    register uint32_t a0 __asm__("a0") = x;
    register uint32_t a1 __asm__("a1") = y;
    register uint32_t a2 __asm__("a2") = z;
    register uint32_t a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0), "+r"(a1) : "r"(a2), "r"(a7) : "memory");
    return a0;
}

/* A call with four arguments (a0-a3). */
static inline uint32_t call4(uint32_t n, uint32_t x, uint32_t y, uint32_t z, uint32_t w)
{
    register uint32_t a0 __asm__("a0") = x;
    register uint32_t a1 __asm__("a1") = y;
    register uint32_t a2 __asm__("a2") = z;
    register uint32_t a3 __asm__("a3") = w;
    register uint32_t a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0), "+r"(a1) : "r"(a2), "r"(a3), "r"(a7) : "memory");
    return a0;
}

/* A call with a second result (a1). */
static inline uint32_t call2(uint32_t n, uint32_t x, uint32_t y, uint32_t *second)
{
    register uint32_t a0 __asm__("a0") = x;
    register uint32_t a1 __asm__("a1") = y;
    register uint32_t a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0), "+r"(a1) : "r"(a7) : "memory");
    *second = a1;
    return a0;
}

#define A(p) ((uint32_t)(p))
static uint32_t frame(void) { return call(FRAME, 0, 0, 0); }
static uint32_t wait(void) { return call(WAIT_FRAME, 0, 0, 0); }

/* cpu.S */
void cpu_rv32i(uint32_t res[2]);
void cpu_m(uint32_t res[2]);
void cpu_c(uint32_t res[2]);
extern const char ram_add[], ram_add_end[], ram_smc[], ram_smc_end[];
extern const char ram_frame[], ram_frame_end[], ram_last[], ram_last_end[];
extern const char count_frames[], count_frames_end[];
void count_frames_rom(uint32_t *out, uint32_t n, uint32_t call, uint32_t arg) __asm__("count_frames");
void count_scroll(uint32_t *out, uint32_t n);
uint32_t count_write(const void *buf, uint32_t len);

/* ------------------------------------------------------- C library parts */

void *memcpy(void *d, const void *s, size_t n)
{
    uint8_t *dp = d;
    const uint8_t *sp = s;
    if (!((A(dp) | A(sp) | n) & 3)) {
        uint32_t *dw = d;
        const uint32_t *sw = s;
        for (n /= 4; n; n--)
            *dw++ = *sw++;
        return d;
    }
    while (n--)
        *dp++ = *sp++;
    return d;
}

void *memset(void *d, int c, size_t n)
{
    uint8_t *dp = d;
    if (!((A(dp) | n) & 3)) {
        uint32_t *dw = d, v = (uint8_t)c * 0x01010101u;
        for (n /= 4; n; n--)
            *dw++ = v;
        return d;
    }
    while (n--)
        *dp++ = c;
    return d;
}

int memcmp(const void *a, const void *b, size_t n)
{
    const uint8_t *p = a, *q = b;
    for (; n; n--, p++, q++)
        if (*p != *q)
            return *p - *q;
    return 0;
}

static uint32_t slen(const char *s)
{
    uint32_t n = 0;
    while (s[n])
        n++;
    return n;
}

/* ------------------------------------------------------------- terminal */

/* Writes all of p (the write call may stop early when the budget runs out). */
static void twn(const void *p, uint32_t n)
{
    const uint8_t *b = p;
    while (n) {
        uint32_t k = call(T_WRITE, A(b), n, 0);
        if (k > n)
            return;
        b += k;
        n -= k;
    }
}
static void tw(const char *s) { twn(s, slen(s)); }
static void at(uint32_t col, uint32_t row) { call(T_SET_CURSOR, col, row, 0); }
static void colours(uint32_t fg, uint32_t bg) { call(T_SET_COLOURS, fg, bg, 0); }
static void cmd(uint32_t c) { call(T_COMMAND, c, 0, 0); }

static void put_u(uint32_t v)
{
    char b[11];
    int i = 10;
    b[i] = 0;
    do {
        b[--i] = '0' + v % 10;
        v /= 10;
    } while (v);
    tw(b + i);
}

/* ---------------------------------------------------------------- checks */

static uint32_t c_n, c_fails, c_first;

static void ck(int ok)
{
    c_n++;
    if (!ok && !c_fails++)
        c_first = c_n;
}

static void begin(void) { c_n = c_fails = c_first = 0; }

typedef struct {
    const char *name;
    uint32_t fails, first;
    char value[40];
} result;

static result results[30];
static uint32_t n_results;
static char value[40];
static uint32_t value_len;

static void val_s(const char *s)
{
    while (*s && value_len < sizeof value - 1)
        value[value_len++] = *s++;
    value[value_len] = 0;
}

static void val_u(uint32_t v)
{
    char b[12];
    int i = 11;
    b[i] = 0;
    do {
        b[--i] = '0' + v % 10;
        v /= 10;
    } while (v);
    if (value_len)
        val_s(" ");
    val_s(b + i);
}

static void show_result(const result *r)
{
    colours(r->fails ? 9 : 10, 0x100);
    tw(r->fails ? "FAIL " : "PASS ");
    colours(7, 0x100);
    tw(r->name);
    if (r->value[0]) {
        tw(" = ");
        tw(r->value);
    }
    if (r->fails) {
        tw(" (");
        put_u(r->fails);
        tw(" failed, first ");
        put_u(r->first);
        tw(")");
    }
}

static void show_all(void)
{
    colours(7, 0);
    cmd(1);
    tw("Terse conformance image \xE2\x80\xA2 conform.events");
    for (uint32_t i = 0; i < n_results; i++) {
        tw("\n");
        show_result(&results[i]);
    }
}

static void record(const char *name, uint32_t fails, uint32_t first)
{
    result *r = &results[n_results++];
    r->name = name;
    r->fails = fails;
    r->first = first;
    memcpy(r->value, value, sizeof value);
    value_len = 0;
    value[0] = 0;
}

static void report(const char *name) { record(name, c_fails, c_first); }

/* Waits until frame f; a check fails if the program is already past it
 * (the scripted input would then be read in the wrong frame). */
static void wait_until(uint32_t f)
{
    ck(frame() <= f);
    while (frame() < f)
        wait();
}

/* ----------------------------------------------------------------- memory */

static uint32_t buf_a[4096], buf_b[4096]; /* 16 KiB each: stress lists, scratch */
static uint32_t ram_code[128];             /* code run from RAM */
#define BA ((uint8_t *)buf_a)
#define BB ((uint8_t *)buf_b)

/* ===================================================== 1. CPU (cpu.S) */

static void t_cpu(void)
{
    uint32_t r[2];
    cpu_rv32i(r);
    record("cpu: RV32I", r[0], r[1]);
    cpu_m(r);
    record("cpu: M extension", r[0], r[1]);
    cpu_c(r);
    record("cpu: compressed", r[0], r[1]);
}

/* ============================================= 2. code in RAM, rewritten */

typedef uint32_t (*fn1)(uint32_t);
typedef uint32_t (*fn2)(uint32_t, uint32_t);

static void t_ram_code(void)
{
    begin();
    uint8_t *code = (uint8_t *)ram_code;
    memcpy(code, ram_add, ram_add_end - ram_add);
    fn1 f = (fn1)code;
    ck(f(0x1000) == 0x1000 + 0x123 + 1);
    /* Rewrite both immediates: addi 0x456 and c.addi 7, then -1 and -1. */
    ram_code[0] = (ram_code[0] & 0x000FFFFF) | 0x456u << 20;
    uint16_t *c16 = (uint16_t *)(code + 4);
    *c16 = (*c16 & ~0x107C) | 7 << 2;
    __asm__ volatile("" ::: "memory");
    ck(f(0x1000) == 0x1000 + 0x456 + 7);
    ram_code[0] = (ram_code[0] & 0x000FFFFF) | 0xFFFu << 20;
    *c16 = *c16 | 0x107C;
    __asm__ volatile("" ::: "memory");
    ck(f(0x1000) == 0x1000 - 2);

    /* Code that stores over its own next-but-one instruction runs the new one. */
    memcpy(code + 64, ram_smc, ram_smc_end - ram_smc);
    fn2 g = (fn2)(code + 64);
    ck(g(A(code + 64), 0x00200513) == 2); /* addi a0, zero, 2 */
    ck(g(A(code + 64), 0x00300513) == 3); /* addi a0, zero, 3 */

    /* A call made from RAM. */
    memcpy(code + 128, ram_frame, ram_frame_end - ram_frame);
    uint32_t now = frame();
    ck(((fn1)(code + 128))(0) == now);

    /* Instructions in the last six bytes of RAM, the last ending at its end. */
    uint32_t n = ram_last_end - ram_last;
    ck(n == 6);
    memcpy((void *)(RAM_END - n), ram_last, n);
    __asm__ volatile("" ::: "memory");
    ck(((fn1)(RAM_END - n))(5) == 5 + 0x77);
    report("RAM: run code, rewrite it, run again");
}

/* ============================================================ 6. terminal */

static uint32_t cell(uint32_t col, uint32_t row)
{
    static uint32_t v;
    call(T_CELLS_READ, row * 80 + col, A(&v), 1);
    return v;
}
static void CH(uint32_t col, uint32_t row, uint32_t ch) { ck((cell(col, row) & 0xFFFF) == ch); }
static void CELL(uint32_t col, uint32_t row, uint32_t ch, uint32_t fg, uint32_t bg)
{
    ck(cell(col, row) == (ch | fg << 16 | bg << 24));
}
static void CUR(uint32_t col, uint32_t row)
{
    uint32_t r, c = call2(T_GET_CURSOR, 0, 0, &r);
    ck(c == col && r == row);
}
static void COLS(uint32_t fg, uint32_t bg)
{
    uint32_t b, f = call2(T_GET_COLOURS, 0, 0, &b);
    ck(f == fg && b == bg);
}

static void t_print(void)
{
    begin();
    colours(7, 0);
    cmd(1);
    ck(call(T_WRITE, A("AB"), 2, 0) == 2);
    CELL(0, 0, 'A', 7, 0);
    CELL(1, 0, 'B', 7, 0);
    CUR(2, 0);
    colours(12, 4);
    COLS(12, 4);
    tw("C");
    CELL(2, 0, 'C', 12, 4);
    at(200, 99); /* clamped */
    CUR(79, 29);
    at(0xFFFFFFFF, 3);
    CUR(79, 3);
    colours(7, 0);
    at(79, 29); /* the last cell: pending wrap, no scroll yet */
    tw("Z");
    CH(79, 29, 'Z');
    CUR(79, 29);
    CH(0, 0, 'A');
    tw("Y"); /* now it scrolls */
    CH(79, 28, 'Z');
    CH(0, 29, 'Y');
    CH(0, 0, ' ');
    CUR(1, 29);
    at(5, 3);
    tw("\b");
    CUR(4, 3);
    at(0, 3);
    tw("\b");
    CUR(0, 3);
    tw("\t");
    CUR(8, 3);
    tw("\t");
    CUR(16, 3);
    at(75, 3);
    tw("\t");
    CUR(79, 3);
    tw("\t");
    CUR(79, 3);
    tw("\r");
    CUR(0, 3);
    at(9, 3);
    tw("\n"); /* column 0 of the next row */
    CUR(0, 4);
    twn("\0\1\7\x1B\x1F\x7F", 6); /* ignored */
    CUR(0, 4);
    CH(0, 4, ' ');
    tw("x");
    colours(5, 0x100);
    tw("\f"); /* clear, cursor home */
    CUR(0, 0);
    CELL(0, 4, ' ', 5, 0);
    CELL(79, 29, ' ', 5, 0);
    colours(7, 0);
    uint32_t rows, cols = call2(T_SIZE, 0, 0, &rows);
    ck(cols == 80 && rows == 30);
    /* set_colours: a value above 255 leaves that colour as it is */
    colours(0x1FF, 3);
    COLS(7, 3);
    colours(9, 0x100);
    COLS(9, 3);
    colours(0xFFFFFFFF, 0xFFFFFFFF);
    COLS(9, 3);
    colours(7, 0);
    report("terminal: print, cursor, controls");
}

static void t_wrap(void)
{
    begin();
    cmd(1);
    at(79, 3); /* LF after pending wrap moves one row */
    tw("E\na");
    CH(0, 4, 'a');
    CUR(1, 4);
    at(79, 3); /* CR */
    tw("E\rb");
    CH(0, 3, 'b');
    CUR(1, 3);
    at(79, 3); /* BS */
    tw("E\b");
    CUR(78, 3);
    at(79, 3); /* TAB */
    tw("E\tc");
    CH(79, 3, 'c');
    CUR(79, 3);
    at(79, 3); /* set_cursor, even to the same place */
    tw("E");
    at(79, 3);
    tw("d");
    CH(79, 3, 'd');
    CUR(79, 3);
    at(79, 3); /* ignored controls, colours and other commands keep it */
    tw("E\x07\x1B\x7F");
    cmd(4);
    cmd(5);
    cmd(0x101);
    colours(9, 0x100);
    tw("f");
    CH(0, 4, 'f');
    CUR(1, 4);
    colours(7, 0x100);
    at(79, 3); /* command 01h clears it */
    tw("E");
    cmd(1);
    CUR(0, 0);
    tw("g");
    CH(0, 0, 'g');
    CUR(1, 0);
    at(79, 3); /* FF clears it */
    tw("E\fh");
    CH(0, 0, 'h');
    CUR(1, 0);
    at(79, 29); /* LF with pending wrap on the last row scrolls once */
    tw("E\ni");
    CH(79, 28, 'E');
    CH(0, 29, 'i');
    CUR(1, 29);
    report("terminal: pending wrap");
}

static void wide_setup(void)
{
    at(0, 10);
    colours(1, 2);
    cmd(4);
    tw("你你");
    colours(3, 4);
}

static void t_wide(void)
{
    begin();
    cmd(1);
    colours(3, 5);
    tw("你x");
    CELL(0, 0, 0x4F60, 3, 5);
    CELL(1, 0, 0xFFFF, 3, 5);
    CH(2, 0, 'x');
    CUR(3, 0);
    at(78, 0); /* at column 78: fills the row, pending wrap */
    tw("好");
    CH(78, 0, 0x597D);
    CH(79, 0, 0xFFFF);
    CUR(79, 0);
    tw("n");
    CH(0, 1, 'n');
    CH(78, 0, 0x597D);
    CH(79, 0, 0xFFFF);
    at(79, 5); /* at column 79: a space there, then the next row */
    colours(6, 2);
    tw("好");
    CELL(79, 5, ' ', 6, 2);
    CH(0, 6, 0x597D);
    CH(1, 6, 0xFFFF);
    CUR(2, 6);
    at(79, 29); /* on the last row it scrolls */
    tw("好");
    CH(0, 29, 0x597D);
    CH(1, 29, 0xFFFF);
    CH(79, 28, ' ');
    CH(0, 5, 0x597D);
    CUR(2, 29);
    at(79, 7); /* with pending wrap, column 79 keeps its character */
    tw("z好");
    CH(79, 7, 'z');
    CH(0, 8, 0x597D);
    CH(1, 8, 0xFFFF);
    /* Overwriting half of a wide pair (SPEC 8.3 step 3): pairs at 0-1 and
     * 2-3 in colours 1/2, then text in 3/4. */
    wide_setup();
    at(0, 10);
    tw("x");
    CELL(0, 10, 'x', 3, 4);
    CELL(1, 10, ' ', 1, 2);
    CELL(2, 10, 0x4F60, 1, 2);
    wide_setup();
    at(1, 10);
    tw("x");
    CELL(0, 10, ' ', 1, 2);
    CELL(1, 10, 'x', 3, 4);
    CELL(2, 10, 0x4F60, 1, 2);
    CELL(3, 10, 0xFFFF, 1, 2);
    wide_setup();
    at(1, 10);
    tw("好");
    CELL(0, 10, ' ', 1, 2);
    CELL(1, 10, 0x597D, 3, 4);
    CELL(2, 10, 0xFFFF, 3, 4);
    CELL(3, 10, ' ', 1, 2);
    wide_setup();
    at(3, 10);
    tw("好");
    CELL(2, 10, ' ', 1, 2);
    CELL(3, 10, 0x597D, 3, 4);
    CELL(4, 10, 0xFFFF, 3, 4);
    wide_setup();
    at(2, 10);
    tw("好");
    CELL(0, 10, 0x4F60, 1, 2);
    CELL(1, 10, 0xFFFF, 1, 2);
    CELL(2, 10, 0x597D, 3, 4);
    CELL(3, 10, 0xFFFF, 3, 4);
    colours(7, 0);
    at(0, 12); /* U+FFFF and above U+FFFF: U+FFFD, one cell */
    tw("\xEF\xBF\xBF");
    CH(0, 12, 0xFFFD);
    CUR(1, 12);
    tw("\xF0\x9F\x98\x80");
    CH(1, 12, 0xFFFD);
    CUR(2, 12);
    tw("\xEF\xBF\xBE"); /* U+FFFE is printed as itself */
    CH(2, 12, 0xFFFE);
    report("terminal: wide characters");
}

static void t_utf8(void)
{
    begin();
    cmd(1);
    tw("\x80"); /* lone continuation byte */
    CH(0, 0, 0xFFFD);
    CUR(1, 0);
    at(0, 1);
    tw("\xFF");
    CH(0, 1, 0xFFFD);
    CUR(1, 1);
    at(0, 2); /* overlong lead byte */
    tw("\xC0\xAF");
    CH(0, 2, 0xFFFD);
    CH(1, 2, 0xFFFD);
    CUR(2, 2);
    at(0, 3); /* cut short: the byte is decoded afresh */
    tw("\xC3" "A");
    CH(0, 3, 0xFFFD);
    CH(1, 3, 'A');
    CUR(2, 3);
    at(0, 4);
    tw("\xE4\xBD" "A");
    CH(0, 4, 0xFFFD);
    CH(1, 4, 'A');
    CUR(2, 4);
    at(0, 5); /* overlong three-byte form */
    tw("\xE0\x80\x80");
    CH(0, 5, 0xFFFD);
    CH(1, 5, 0xFFFD);
    CH(2, 5, 0xFFFD);
    CUR(3, 5);
    at(0, 6); /* surrogate */
    tw("\xED\xA0\x80");
    CH(0, 6, 0xFFFD);
    CH(2, 6, 0xFFFD);
    CUR(3, 6);
    at(0, 7); /* above U+10FFFF */
    tw("\xF4\x90\x80\x80");
    CH(3, 7, 0xFFFD);
    CUR(4, 7);
    at(0, 8);
    tw("\xC3\xC3\xA9");
    CH(0, 8, 0xFFFD);
    CH(1, 8, 0x00E9);
    CUR(2, 8);
    at(0, 9); /* a malformed newline is still a newline */
    tw("\xE4\n");
    CH(0, 9, 0xFFFD);
    CUR(0, 10);
    at(5, 10); /* incomplete: nothing yet, then cut short */
    tw("\xF0\x9F\x98");
    CH(5, 10, ' ');
    CUR(5, 10);
    tw("Z");
    CH(5, 10, 0xFFFD);
    CH(6, 10, 'Z');
    at(0, 11);
    tw("é€");
    CH(0, 11, 0x00E9);
    CH(1, 11, 0x20AC);
    CUR(2, 11);
    /* A sequence split across write calls completes in the later call. */
    at(0, 12);
    ck(call(T_WRITE, A("\xE4\xBD"), 2, 0) == 2);
    CH(0, 12, ' ');
    CUR(0, 12);
    ck(call(T_WRITE, A("\xA0"), 1, 0) == 1);
    CH(0, 12, 0x4F60);
    CH(1, 12, 0xFFFF);
    CUR(2, 12);
    twn("\xF0", 1); /* a four-byte one in three calls: U+FFFD, one cell */
    twn("\x9F\x98", 2);
    CUR(2, 12);
    twn("\x80", 1);
    CH(2, 12, 0xFFFD);
    CUR(3, 12);
    twn("\xE4", 1); /* split, then interrupted in the next call */
    twn("Q", 1);
    CH(3, 12, 0xFFFD);
    CH(4, 12, 'Q');
    CUR(5, 12);
    report("terminal: malformed UTF-8");
}

static void t_cmds(void)
{
    begin();
    colours(7, 0);
    cmd(1);
    tw("row0\r\nrow1");
    colours(7, 2);
    cmd(2); /* scroll up */
    CH(3, 0, '1');
    CELL(0, 29, ' ', 7, 2);
    cmd(3); /* scroll down */
    CH(3, 1, '1');
    CELL(0, 0, ' ', 7, 2);
    CUR(4, 1);
    at(2, 1);
    cmd(4); /* clear to the end of the row */
    CH(1, 1, 'o');
    CELL(2, 1, ' ', 7, 2);
    CELL(79, 1, ' ', 7, 2);
    CUR(2, 1);
    cmd(6); /* hide, show, unknown, above 255 */
    cmd(5);
    cmd(0x77);
    cmd(0);
    cmd(0x101);
    cmd(0xFFFFFFFF);
    CH(1, 1, 'o');
    CUR(2, 1);
    ck(call(T_COMMAND, 1, 0, 0) == 0);
    CUR(0, 0);
    CELL(5, 5, ' ', 7, 2);
    colours(7, 0);
    cmd(1);
    report("terminal: commands");
}

static const uint8_t rom_cell[4] = {'R', 0, 11, 4};
static const uint8_t xterm_255_0_1[9] = {0xEE, 0xEE, 0xEE, 0, 0, 0, 0xCD, 0, 0};

static void t_cells(void)
{
    begin();
    static uint32_t c[3];
    cmd(1);
    c[0] = 'Q' | 9 << 16 | 1u << 24;
    ck(call(T_CELLS_WRITE, 1, A(c), 1) == 1);
    CELL(1, 0, 'Q', 9, 1);
    /* From the last cell, wrapping to the first. */
    c[0] = 'A' | 1 << 16 | 2u << 24;
    c[1] = 'B' | 3 << 16 | 4u << 24;
    ck(call(T_CELLS_WRITE, 2399, A(c), 2) == 2);
    CELL(79, 29, 'A', 1, 2);
    CELL(0, 0, 'B', 3, 4);
    c[0] = c[1] = c[2] = 0;
    ck(call(T_CELLS_READ, 2399, A(c), 3) == 3);
    ck(c[0] == ('A' | 1 << 16 | 2u << 24) && c[1] == ('B' | 3 << 16 | 4u << 24) && c[2] == ('Q' | 9 << 16 | 1u << 24));
    /* The whole grid, from the middle: every cell once. */
    ck(call(T_CELLS_READ, 0, A(buf_a), 2400) == 2400);
    ck(call(T_CELLS_READ, 1200, A(buf_b), 2400) == 2400);
    ck(!memcmp(BB, BA + 4800, 4800) && !memcmp(BB + 4800, BA, 4800));
    /* Bounds and buffers. */
    ck(call(T_CELLS_READ, 2400, A(c), 1) == ERR_BUFFER);
    ck(call(T_CELLS_READ, 0, A(buf_a), 2401) == ERR_BUFFER);
    ck(call(T_CELLS_READ, 0, A(buf_a), 0x40000001) == ERR_BUFFER); /* 4 x count overflows 32 bits */
    ck(call(T_CELLS_WRITE, 0, A(buf_a), 0x40000001) == ERR_BUFFER);
    ck(call(T_PALETTE_SET, 0, A(buf_a), 0x55555556) == ERR_BUFFER); /* so does 3 x count */
    ck(call(T_CELLS_READ, 0xFFFFFFFF, A(c), 1) == ERR_BUFFER);
    ck(call(T_CELLS_READ, 0, A(rom_cell), 1) == ERR_BUFFER); /* ROM is not writable */
    ck(call(T_CELLS_READ, 0, 0, 1) == ERR_BUFFER);
    ck(call(T_CELLS_READ, 0, RAM_END - 3, 1) == ERR_BUFFER);
    ck(call(T_CELLS_READ, 0, RAM_END - 4, 1) == 1);
    ck(call(T_CELLS_READ, 0, RAM_END, 0) == 0);
    ck(call(T_CELLS_READ, 2399, A(c), 0) == 0);
    ck(call(T_CELLS_WRITE, 2, A(rom_cell), 1) == 1); /* from ROM */
    CELL(2, 0, 'R', 11, 4);
    ck(call(T_CELLS_WRITE, 2400, A(c), 1) == ERR_BUFFER);
    ck(call(T_CELLS_WRITE, 0, A(c), 2401) == ERR_BUFFER);
    ck(call(T_CELLS_WRITE, 0, 0, 1) == ERR_BUFFER);
    ck(call(T_CELLS_WRITE, 0, ROM_END - 2, 1) == ERR_BUFFER);
    ck(call(T_CELLS_WRITE, 0, ROM_END, 0) == 0);
    CELL(0, 0, 'B', 3, 4);
    /* Not checked: a stray continuation code is stored as given. */
    c[0] = 0xFFFF | 7 << 16;
    ck(call(T_CELLS_WRITE, 80, A(c), 1) == 1);
    CELL(0, 1, 0xFFFF, 7, 0);
    /* Palette: wraps from 255 to 0; bounds; then the xterm colours back. */
    static const uint8_t p[6] = {1, 2, 3, 4, 5, 6};
    ck(call(T_PALETTE_SET, 255, A(p), 2) == 2);
    ck(call(T_PALETTE_SET, 7, A(p), 1) == 1);
    ck(call(T_PALETTE_SET, 256, A(p), 1) == ERR_BUFFER);
    ck(call(T_PALETTE_SET, 0, A(buf_a), 257) == ERR_BUFFER);
    ck(call(T_PALETTE_SET, 0, 0, 1) == ERR_BUFFER);
    ck(call(T_PALETTE_SET, 0, RAM_END - 2, 1) == ERR_BUFFER);
    ck(call(T_PALETTE_SET, 0, A(p), 0) == 0);
    ck(call(T_PALETTE_SET, 200, ROM_END - 3, 1) == 1); /* the last bytes of ROM */
    static const uint8_t restore[] = {0xFF, 0x00, 0xD7, 0xE5, 0xE5, 0xE5};
    ck(call(T_PALETTE_SET, 200, A(restore), 1) == 1);
    ck(call(T_PALETTE_SET, 7, A(restore + 3), 1) == 1);
    ck(call(T_PALETTE_SET, 255, A(xterm_255_0_1), 3) == 3);
    /* write: buffers in ROM or RAM, wholly. */
    ck(call(T_WRITE, 0, 1, 0) == ERR_BUFFER);
    ck(call(T_WRITE, ROM_END - 1, 2, 0) == ERR_BUFFER);
    ck(call(T_WRITE, ROM_END, 1, 0) == ERR_BUFFER);
    ck(call(T_WRITE, ROM_END, 0, 0) == 0);
    ck(call(T_WRITE, RAM_END - 1, 2, 0) == ERR_BUFFER);
    ck(call(T_WRITE, RAM_END, 0, 0) == 0);
    ck(call(T_WRITE, RAM_BASE - 1, 2, 0) == ERR_BUFFER);
    ck(call(T_WRITE, ROM_BASE - 1, 1, 0) == ERR_BUFFER);
    ck(call(T_WRITE, 0xFFFFFFFF, 2, 0) == ERR_BUFFER);
    CUR(0, 0);
    report("terminal: cells, palette, buffers");
}

static void t_terminal(void)
{
    t_print();
    t_wrap();
    t_wide();
    t_utf8();
    t_cmds();
    t_cells();
}

/* ============================================================ 7. vector */

#define XY(x, y) (uint8_t)(x), (uint8_t)((x) >> 8), (uint8_t)(y), (uint8_t)((y) >> 8)
#define R16(r) (uint8_t)(r), (uint8_t)((r) >> 8)

/* Every opcode; a circle of radius 200 units at the centre of the screen,
 * round on a conforming host. */
static const uint8_t showcase[] = {
    0x01, 0x40, 0x80, 0xFF, 0xFF,       /* colour */
    0x02, 0,                            /* width 0 counts as 1 */
    0x04, XY(16 * 16, 400 * 16),        /* move */
    0x05, XY(120 * 16, 400 * 16),       /* line */
    0x02, 48,                           /* three units */
    0x05, XY(120 * 16, 460 * 16),
    0x05, XY(120 * 16, 460 * 16),       /* to the current point: a dot */
    0x01, 0xFF, 0x00, 0x00, 0x80,       /* translucent red */
    0x07, R16(24 * 16),                 /* disc */
    0x03, XY(320 * 16, 240 * 16),       /* origin: the centre of the screen */
    0x04, XY(0, 0),
    0x01, 0x00, 0xFF, 0x80, 0xFF,
    0x02, 32,
    0x06, R16(200 * 16),                /* circle, radius 200 units */
    0x01, 0xFF, 0xC0, 0x00, 0x60,
    0x08, XY(220 * 16, 100 * 16),       /* rectangle from the centre */
    0x09, XY(-300 * 16, 200 * 16), XY(-200 * 16, 230 * 16), /* triangle */
    0x03, XY(-16, -16),                 /* origin -1, -1 unit */
    0x04, XY(16, 16),                   /* (0, 0) */
    0x01, 0xFF, 0xFF, 0xFF, 0xFF,
    0x07, R16(64),                      /* a disc in the top left corner, partly off screen */
};

/* The fill cap (SPEC 10.4), after the showcase's own fills (93,128 square
 * units): three full-screen rectangles fit (1,014,728 in all); the fourth
 * would pass 1,228,800 and is not drawn, nor is the small disc after it,
 * which would fit; the line and the circle after them are drawn. The two
 * shapes that must not be drawn are opaque red. */
static const uint8_t capdemo[] = {
    0x03, XY(0, 0),
    0x04, XY(0, 0),
    0x01, 0x30, 0x30, 0x60, 0x20,
    0x08, XY(10240, 7680),
    0x08, XY(10240, 7680),
    0x08, XY(10240, 7680),
    0x01, 0xFF, 0x00, 0x00, 0xFF,
    0x08, XY(10240, 7680),              /* dropped: over the cap */
    0x04, XY(5120, 6400),
    0x07, R16(256),                     /* dropped: the cap is shut */
    0x01, 0xFF, 0xFF, 0x00, 0xFF,
    0x02, 16,
    0x04, XY(160, 7520),
    0x05, XY(10080, 7520),              /* drawn: lines are not capped */
    0x04, XY(5120, 6400),
    0x06, R16(512),                     /* drawn: circles are not capped */
};

static const uint8_t list_end[] = {
    0x00,                               /* end */
    0x05, XY(5000, 5000),               /* after the end: not drawn */
};

static const uint8_t cutoff[] = {
    0x04, XY(5120, 3840),
    0x07, R16(160),
    0x0A,                               /* unknown opcode */
    0x07, R16(320),
    0x05, 0x10, 0x00,                   /* operands cut off */
};

static uint8_t small_list[512];

/* The results screen's list: showcase, fill cap, end. */
static uint32_t build_final(uint8_t *p)
{
    uint8_t *s = p;
    memcpy(p, showcase, sizeof showcase);
    p += sizeof showcase;
    memcpy(p, capdemo, sizeof capdemo);
    p += sizeof capdemo;
    memcpy(p, list_end, sizeof list_end);
    p += sizeof list_end;
    return p - s;
}

static void t_vector(void)
{
    begin();
    uint8_t *l = small_list;
    memcpy(l, showcase, sizeof showcase);
    memcpy(l + sizeof showcase, list_end, sizeof list_end);
    uint32_t n = sizeof showcase + sizeof list_end;
    ck(call(V_SUBMIT, A(l), n, 0) == 0);
    ck(call(V_SUBMIT, A(l), 0, 0) == 1);              /* length 0 */
    ck(call(V_SUBMIT, A(buf_a), 16385, 0) == 1);      /* too long */
    ck(call(V_SUBMIT, A(showcase), sizeof showcase, 0) == 1); /* in ROM */
    ck(call(V_SUBMIT, 0, 5, 0) == 1);
    ck(call(V_SUBMIT, RAM_END - 1, 2, 0) == 1);       /* past the end of RAM */
    ck(call(V_SUBMIT, RAM_END, 0, 0) == 1);
    ck(call(V_SUBMIT, RAM_BASE - 2, 4, 0) == 1);
    memset((void *)TOP, 0x04, 16384);                 /* 3,276 moves and a cut-off one */
    ck(call(V_SUBMIT, RAM_END - 1, 1, 0) == 0);       /* the last byte: accepted */
    ck(call(V_SUBMIT, TOP, 16384, 0) == 0);           /* 16,384 bytes ending at the end of RAM */
    ck(call(V_SUBMIT, TOP + 1, 16384, 0) == 1);       /* one byte further */
    ck(call(V_SUBMIT, A(buf_a), 16384, 0) == 0);
    memcpy(l + 256, cutoff, sizeof cutoff);
    ck(call(V_SUBMIT, A(l + 256), sizeof cutoff, 0) == 0);     /* operands cut off */
    ck(call(V_SUBMIT, A(l + 256), 8, 0) == 0);                 /* ends at the unknown opcode */
    ck(call(V_CLEAR, 0, 0, 0) == 0);
    ck(call(V_STYLE, 1, 0, 0) == 0);                  /* phosphor */
    ck(call(V_STYLE, 2, 0, 0) == 1);
    ck(call(V_STYLE, 0x101, 0, 0) == 1);
    ck(call(V_STYLE, 0xFFFFFFFF, 0, 0) == 1);
    ck(call(V_PERSISTENCE, 200, 0, 0) == 0);
    ck(call(V_PERSISTENCE, 256, 0, 0) == 1);
    ck(call(V_PERSISTENCE, 0, 0, 0) == 0);
    ck(call(V_PERSISTENCE, 255, 0, 0) == 0);
    ck(call(V_PERSISTENCE, 0xFFFFFFFF, 0, 0) == 1);
    ck(call(V_FILL, 1, 0, 0) == 0);                   /* outlines */
    ck(call(V_FILL, 2, 0, 0) == 1);
    ck(call(V_FILL, 0x100, 0, 0) == 1);
    ck(call(V_FILL, 0, 0, 0) == 0);
    ck(call(V_STYLE, 0, 0, 0) == 0);                  /* clean again */
    ck(call(V_PERSISTENCE, 200, 0, 0) == 0);
    ck(call(0x305, 0, 0, 0) == ERR_UNKNOWN);
    ck(call(0x3FF, 0, 0, 0) == ERR_UNKNOWN);
    n = build_final(l);
    ck(call(V_SUBMIT, A(l), n, 0) == 0);
    report("vector: submit, reject, style, fill");
}

/* ===================================================== 5. system calls */

static uint8_t sbuf[300];

static void t_system(void)
{
    begin();
    uint32_t f = frame();
    ck(frame() == f);
    uint32_t w = wait();
    ck(w == f + 1);
    ck(frame() == w);
    /* A headless run (time 0) supplies no random bytes: they read as zeros.
     * A host with a clock (terse-host --selftest) supplies both. */
    uint32_t t = call(TIME, 0, 0, 0);
    int headless = t == 0;
    ck(call(TIME, 0, 0, 0) == t); /* latched for the frame */
    memset(sbuf, 0xAA, sizeof sbuf);
    ck(call(RANDOM, A(sbuf), 16, 0) == 16);
    ck((!headless || (sbuf[0] == 0 && sbuf[15] == 0)) && sbuf[16] == 0xAA);
    memset(sbuf, 0xAA, sizeof sbuf);
    ck(call(RANDOM, A(sbuf), 256, 0) == 256);
    ck((!headless || sbuf[255] == 0) && sbuf[256] == 0xAA);
    memset(sbuf, 0xAA, sizeof sbuf);
    ck(call(RANDOM, A(sbuf), 257, 0) == ERR_BUFFER);
    ck(sbuf[0] == 0xAA);
    ck(call(RANDOM, A(sbuf), 0, 0) == 0);
    ck(call(RANDOM, A(sbuf), 0xFFFFFFFF, 0) == ERR_BUFFER);
    ck(call(RANDOM, ROM_BASE, 4, 0) == ERR_BUFFER);
    ck(call(RANDOM, 0, 0, 0) == ERR_BUFFER);
    ck(call(RANDOM, RAM_END - 8, 16, 0) == ERR_BUFFER);
    ck(call(RANDOM, RAM_END - 4, 4, 0) == 4);
    ck(call(RANDOM, RAM_END, 0, 0) == 0);
    memset(sbuf, 0xAA, sizeof sbuf);
    ck(call(CONTROLLER, 4, A(sbuf), 0) == 0); /* above 3: not connected */
    ck(sbuf[0] == 0 && sbuf[7] == 0 && sbuf[8] == 0xAA);
    memset(sbuf, 0xAA, sizeof sbuf);
    ck(call(CONTROLLER, 0xFFFFFFFF, A(sbuf), 0) == 0);
    ck(sbuf[0] == 0 && sbuf[7] == 0);
    ck(call(CONTROLLER, 0, ROM_BASE, 0) == ERR_BUFFER);
    ck(call(CONTROLLER, 0, RAM_END - 4, 0) == ERR_BUFFER);
    ck(call(CONTROLLER, 0, 0, 0) == ERR_BUFFER);
    ck(call(NEXT_EVENT, 0, 0, 0) == ERR_BUFFER);
    ck(call(NEXT_EVENT, ROM_BASE, 0, 0) == ERR_BUFFER);
    ck(call(NEXT_EVENT, RAM_END - 2, 0, 0) == ERR_BUFFER);
    report("system calls");

    /* Unknown calls and absent devices: FFFF_FFFFh, a1 unchanged. */
    begin();
    static const uint32_t unknown[] = {
        0x000, 0x0FF, 0x107, 0x1FF, 0x20A, 0x2FF, 0x305, 0x3FF, 0x401, 0x4FF,
        0x500, 0x501, 0x5FF, 0x600, 0x800, 0xA00, 0xFF00, 0x10100, 0x10200, 0xFFFFFFFF,
    };
    for (uint32_t i = 0; i < sizeof unknown / sizeof unknown[0]; i++) {
        uint32_t a1, r = call2(unknown[i], 0x1234, 0x5678, &a1);
        ck(r == ERR_UNKNOWN && a1 == 0x5678);
    }
    report("unknown calls, absent devices");
}

/* =================================================== 4. events, pads */

static const uint8_t ev_want[][4] = {
    {1, 0x04, 0x03, 0}, {2, 0x04, 0, 0}, {1, 0x28, 0x0C, 0}, {2, 0x28, 0x0C, 0}, /* frame 2 */
    {3, 'a', 0, 0}, {3, 0xE9, 0, 0}, {3, 0x60, 0x4F, 0}, {3, 0x00, 0xF6, 0x01}, /* frame 3 */
    {4, 0x7F, 0xDF, 0x3E}, {4, 0, 0, 0},
    {4, 0x40, 0xF0, 0x09}, {4, 0x40, 0xF0, 0x01}, {6, 0xFD, 0x02, 0}, {6, 0x7F, 0x81, 0}, /* frame 4 */
    {7, 0, 0, 0}, {1, 0x45, 0x08, 0}, {2, 0xE7, 0, 0}, /* frame 5 */
};

static void t_events(void)
{
    begin();
    wait_until(6);
    static uint8_t e[4];
    /* A bad buffer removes nothing. */
    ck(call(NEXT_EVENT, 0, 0, 0) == ERR_BUFFER);
    for (uint32_t i = 0; i < sizeof ev_want / 4; i++) {
        e[0] = e[1] = e[2] = e[3] = 0xEE;
        ck(call(NEXT_EVENT, A(e), 0, 0) == 1);
        ck(!memcmp(e, ev_want[i], 4));
    }
    e[0] = 0xEE;
    ck(call(NEXT_EVENT, A(e), 0, 0) == 0); /* empty: nothing written */
    ck(e[0] == 0xEE);
    report("events: key, text, pointer, wheel, feeds");

    /* 70 text events in frame 8: the first 64 are kept. */
    begin();
    wait_until(9);
    uint32_t n = 0;
    while (call(NEXT_EVENT, A(e), 0, 0) == 1) {
        ck(e[0] == 3 && e[1] == '0' + n % 10 && e[2] == 0 && e[3] == 0);
        n++;
    }
    ck(n == 64);
    val_u(n);
    report("event queue overflow (70 -> 64)");
}

static void pad_is(uint32_t n, const uint8_t *want)
{
    static uint8_t b[9];
    memset(b, 0xAA, 9);
    ck(call(CONTROLLER, n, A(b), 0) == want[0]);
    ck(!memcmp(b, want, 8) && b[8] == 0xAA);
}

static void t_pads(void)
{
    static const uint8_t off[8] = {0};
    static const uint8_t p0[8] = {1, 0x13, 0x20, 0x81, 0x7F, 0x05, 0xFB, 0};
    static const uint8_t p0b[8] = {1, 0x01, 0, 0, 0, 0, 0, 0};
    static const uint8_t p2[8] = {1, 0xFF, 0xFF, 0, 0, 0, 0, 0};
    begin();
    pad_is(0, off); /* frame 9: none connected yet */
    pad_is(2, off);
    for (uint32_t f = 10; f <= 11; f++) {
        wait_until(f);
        pad_is(0, p0);
        pad_is(1, off);
        pad_is(2, p2);
        pad_is(3, off);
        pad_is(4, off);
    }
    wait_until(12);
    pad_is(0, p0b);
    pad_is(2, p2);
    wait_until(13);
    pad_is(0, off);
    pad_is(2, off);
    report("controllers");
}

/* ============================================================ 3. budget */

/* The model: the rules of SPEC 1.4 applied to the instructions of the timed
 * loops in cpu.S. An instruction runs if the budget is above zero before
 * it, and costs its fetch (ROM 1, RAM 4) plus, for an ecall, the call's
 * charge; when the budget is zero or below the frame ends, and the next
 * starts with BUDGET added to what is left (a deficit carries; a surplus
 * is dropped by wait_frame). Frames are counted from the one in which the
 * routine's second wait_frame returns, which starts with exactly BUDGET.
 *
 * For a loop of period p units whose call is k units into it, starting s
 * units into a frame of budget b, the calls made in that frame number
 * ceil((b - s - k) / p) (those that start with budget left); the frame
 * then ends with remainder r = b - s - k - p * (n - 1) - c (c the call's
 * cost) if that is not above zero, else with 0 after the loop's cheaper
 * instructions use up the rest, and r carries into the next frame. With
 * the values above: count_frames from ROM has p = 7 (addi 1, frame_count
 * 1 + 4, beq 1), from RAM p = 16; count_scroll p = 165 (a scroll 1 + 4 +
 * 150 among six other instructions). The code below evaluates exactly
 * that, instruction by instruction (skipping whole periods where nothing
 * can change), rather than with the closed form.
 *
 * Worked through for the values above. count_frames from ROM: the setup
 * after the second wait takes s = 15 units (two moves, the extra
 * frame_count 1 + 4, li, the first frame_count 1 + 4, mv, li) and the
 * loop's call is k = 1 unit in, so ceil((125000 - 16) / 7) = 17855 loop
 * calls return frame F, plus the setup's call: 17856. The frame ends at 0;
 * frames F + 1 and F + 2 start at other points of the loop and give 17856
 * and 17857 (ending at 0 and -4). From RAM: 7811, 7811, 7811.
 * count_scroll: 758, 758, 757, the frames ending at -71, -148 and -60; a
 * host that dropped the deficit would give 758 three times. count_write:
 * seven writes of 16,384 bytes and one of 10,234: 124,922. The content
 * delivery check: 17856, then 17853 in the frame that starts 25 units
 * short. */
static int32_t mb;            /* budget */
static uint32_t mf;           /* frame, relative */
static uint32_t m_dframe;     /* a frame whose budget content delivery reduces */
static int32_t m_dcharge;

static void m_start(void)
{
    mb = BUDGET;
    mf = 0;
    m_dframe = 0xFFFFFFFF;
    m_dcharge = 0;
}

/* One instruction costing c in all; returns the frame it runs in. */
static uint32_t m_ins(int32_t c)
{
    while (mb <= 0) {
        mf++;
        mb += BUDGET;
        if (mf == m_dframe)
            mb -= m_dcharge;
    }
    mb -= c;
    return mf;
}

/* count_frames run from ROM (ic = ROM_COST) or RAM (RAM_COST); `extra` is
 * the charge of its extra call beyond CHARGE_CALL. */
static void model_frames(uint32_t *out, uint32_t n, int32_t ic, int32_t extra)
{
    const int32_t call = ic + CHARGE_CALL, per = 3 * ic + CHARGE_CALL;
    m_ins(ic);                      /* mv a7, t4 */
    m_ins(ic);                      /* mv a0, t5 */
    m_ins(call + extra);            /* the extra call */
    m_ins(ic);                      /* li a7 */
    uint32_t t3 = m_ins(call), f;   /* frame_count */
    m_ins(ic);                      /* mv t3 */
    m_ins(ic);                      /* li t0 */
    uint32_t t0 = 1;
    for (uint32_t k = 0; k < n; k++) {
        for (;;) {
            if (mf == t3 && mb > 3 * per) {
                int32_t q = (mb - 2 * per) / per;
                t0 += q;
                mb -= q * per;
            }
            m_ins(ic);              /* addi */
            t0++;
            f = m_ins(call);        /* frame_count */
            m_ins(ic);              /* beq */
            if (f != t3)
                break;
        }
        out[k] = t0 - 1;
        for (int i = 0; i < 7; i++) /* addi sw addi mv li addi bnez */
            m_ins(ic);
        t3 = f;
        t0 = 1;
    }
}

static void model_scroll(uint32_t *out, uint32_t n)
{
    const int32_t call = ROM_COST + CHARGE_CALL, per = 5 * ROM_COST + 2 * CHARGE_CALL + CHARGE_SCROLL + 2 * ROM_COST;
    m_ins(ROM_COST);                /* li a7 */
    uint32_t t3 = m_ins(call), f;   /* frame_count */
    m_ins(ROM_COST);
    m_ins(ROM_COST);
    uint32_t t0 = 1;
    for (uint32_t k = 0; k < n; k++) {
        for (;;) {
            if (mf == t3 && mb > 3 * per) {
                int32_t q = (mb - 2 * per) / per;
                t0 += q;
                mb -= q * per;
            }
            m_ins(ROM_COST);        /* addi */
            t0++;
            m_ins(ROM_COST);        /* li a7 */
            m_ins(ROM_COST);        /* li a0 */
            m_ins(call + CHARGE_SCROLL);
            m_ins(ROM_COST);        /* li a7 */
            f = m_ins(call);
            m_ins(ROM_COST);        /* beq */
            if (f != t3)
                break;
        }
        out[k] = t0 - 1;
        for (int i = 0; i < 7; i++)
            m_ins(ROM_COST);
        t3 = f;
        t0 = 1;
    }
}

/* count_write: a write stops after the byte whose charge takes the budget
 * to zero or below (always one byte at least). */
static uint32_t model_write(uint32_t len)
{
    uint32_t total = 0, n;
    do {
        m_ins(ROM_COST);            /* li a7 */
        m_ins(ROM_COST);            /* mv a0 */
        m_ins(ROM_COST);            /* mv a1 */
        m_ins(ROM_COST);            /* ecall */
        int32_t left = mb - CHARGE_CALL;
        n = left <= 0 ? 1 : (uint32_t)(left + CHARGE_WRITE_BYTE - 1) / CHARGE_WRITE_BYTE;
        if (n > len)
            n = len;
        mb -= CHARGE_CALL + n * CHARGE_WRITE_BYTE;
        m_ins(ROM_COST);            /* add */
        m_ins(ROM_COST);            /* beq */
        total += n;
    } while (n == len);
    return total;
}

static void budget_line(const char *name, const uint32_t *got, const uint32_t *want, uint32_t n)
{
    begin();
    for (uint32_t i = 0; i < n; i++) {
        ck(got[i] == want[i]);
        val_u(got[i]);
    }
    report(name);
}

static void t_budget(void)
{
    uint32_t got[3], want[3];
    /* count_frames from ROM, three frames: see model_frames. */
    count_frames_rom(got, 3, FRAME, 0);
    m_start();
    model_frames(want, 3, ROM_COST, 0);
    budget_line("budget: ROM loop, calls per frame", got, want, 3);

    /* The same code copied to RAM: 4 units an instruction. */
    memcpy(ram_code, count_frames, count_frames_end - count_frames);
    __asm__ volatile("" ::: "memory");
    ((void (*)(uint32_t *, uint32_t, uint32_t, uint32_t))ram_code)(got, 3, FRAME, 0);
    m_start();
    model_frames(want, 3, RAM_COST, 0);
    budget_line("budget: RAM loop, calls per frame", got, want, 3);

    /* A scroll (150) in the loop: frames end up to 154 below zero and the
     * deficit carries into the next. */
    count_scroll(got, 3);
    m_start();
    model_scroll(want, 3);
    budget_line("budget: scroll loop, deficit carried", got, want, 3);
    colours(7, 0);
    cmd(1);

    /* write stops after the byte that uses up the budget. NUL bytes are
     * ignored by the terminal, so only the byte charge counts. */
    memset(buf_b, 0, 16384);
    got[0] = count_write(buf_b, 16384);
    m_start();
    want[0] = model_write(16384);
    budget_line("budget: bytes one frame's writes take", got, want, 1);
}

/* =========================================================== 8. content */

typedef struct {
    uint8_t op, status, r0, r1;
    uint8_t id[32];
    uint32_t offset, length, addr, size;
} req;

static req blk[10];
static uint8_t done_status[9];
static uint32_t done_count;
static uint8_t put_src[100];
static uint8_t get_dst[128];

/* BLAKE3-256 of the 100 bytes i x 7 + 3. */
static const uint8_t put_id[32] = {
    0x52, 0x03, 0xB1, 0x52, 0x25, 0x6C, 0xE5, 0xA2, 0xAE, 0xA1, 0x2F, 0x12, 0x4E, 0x3B, 0xD9, 0xDE,
    0xF9, 0xEB, 0xE2, 0x12, 0xB4, 0x9C, 0x27, 0x51, 0x23, 0x77, 0x39, 0xF6, 0x30, 0xEA, 0x51, 0x2A,
};
static const char rom_put[] = "Terse conformance image: a blob put from ROM.\n";
/* BLAKE3-256 of rom_put without its terminating NUL. */
static const uint8_t rom_put_id[32] = {
    0xAB, 0xB4, 0x1F, 0x07, 0xAA, 0x77, 0xB8, 0x7E, 0x97, 0x55, 0x5A, 0xAC, 0xAF, 0x40, 0xCE, 0xBD,
    0xB7, 0x36, 0xCA, 0xF2, 0x23, 0x2F, 0x73, 0xCF, 0x35, 0xF7, 0xD1, 0x29, 0x1F, 0x63, 0x62, 0xCD,
};

static void blk_clear(req *r)
{
    memset(r, 0, sizeof *r);
    r->status = 0xAA;
}

static uint32_t submit(req *r) { return call(C_SUBMIT, A(r), 0, 0); }

static void done_reset(void)
{
    done_count = 0;
    memset(done_status, 0xFF, sizeof done_status);
}

/* Takes the content-done events from the queue (others are dropped). */
static void collect(void)
{
    static uint8_t e[4];
    while (call(NEXT_EVENT, A(e), 0, 0) == 1) {
        if (e[0] == 5 && e[1] >= 1 && e[1] <= 8) {
            done_status[e[1]] = e[2];
            done_count++;
        }
    }
}

/* Waits until n content-done events have arrived, at most 600 frames. */
static void wait_done(uint32_t n)
{
    for (uint32_t t = 0; t < 600; t++) {
        collect();
        if (done_count >= n)
            return;
        wait();
    }
    ck(0);
}

static void get_req(req *r, const uint8_t *id, uint32_t offset, uint32_t length, void *addr)
{
    blk_clear(r);
    r->op = 1;
    memcpy(r->id, id, 32);
    r->offset = offset;
    r->length = length;
    r->addr = A(addr);
}

static void t_content(void)
{
    req *r = &blk[0];
    begin();
    /* Put 100 bytes from RAM; the ID comes back. */
    for (uint32_t i = 0; i < 100; i++)
        put_src[i] = i * 7 + 3;
    blk_clear(r);
    r->op = 2;
    r->addr = A(put_src);
    r->size = 100;
    done_reset();
    ck(submit(r) == 1);
    ck(r->status == 0);
    wait_done(1);
    ck(done_status[1] == 1 && r->status == 1);
    ck(!memcmp(r->id, put_id, 32));
    /* Put from ROM. */
    blk_clear(r);
    r->op = 2;
    r->addr = A(rom_put);
    r->size = sizeof rom_put - 1;
    done_reset();
    ck(submit(r) == 1);
    wait_done(1);
    ck(done_status[1] == 1 && r->status == 1);
    ck(!memcmp(r->id, rom_put_id, 32));
    /* Get both back, whole. */
    memset(get_dst, 0xEE, sizeof get_dst);
    get_req(r, put_id, 0, 100, get_dst);
    get_req(&blk[1], rom_put_id, 0, 16384, BB);
    done_reset();
    ck(submit(r) == 1);
    ck(submit(&blk[1]) == 2);
    wait_done(2);
    ck(done_status[1] == 1 && done_status[2] == 1);
    ck(r->status == 1 && r->length == 100 && r->size == 100);
    ck(!memcmp(get_dst, put_src, 100) && get_dst[100] == 0xEE);
    ck(blk[1].status == 1 && blk[1].length == sizeof rom_put - 1 && blk[1].size == sizeof rom_put - 1);
    ck(!memcmp(BB, rom_put, sizeof rom_put - 1));
    /* Offset 90, length 20: 10 bytes delivered. */
    memset(get_dst, 0xEE, sizeof get_dst);
    get_req(r, put_id, 90, 20, get_dst);
    done_reset();
    ck(submit(r) == 1);
    wait_done(1);
    ck(r->status == 1 && r->length == 10 && r->size == 100);
    ck(!memcmp(get_dst, put_src + 90, 10) && get_dst[10] == 0xEE);
    /* At the end, and beyond it: done, length 0, nothing written. */
    memset(get_dst, 0xEE, sizeof get_dst);
    get_req(r, put_id, 100, 5, get_dst);
    get_req(&blk[1], put_id, 0xFFFFFFFF, 5, get_dst);
    done_reset();
    ck(submit(r) == 1);
    ck(submit(&blk[1]) == 2);
    wait_done(2);
    ck(r->status == 1 && r->length == 0 && r->size == 100);
    ck(blk[1].status == 1 && blk[1].length == 0 && blk[1].size == 100);
    ck(get_dst[0] == 0xEE);
    /* A blob nobody has. */
    static const uint8_t absent[32] = {
        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    };
    get_req(r, absent, 0, 5, get_dst);
    done_reset();
    ck(submit(r) == 1);
    wait_done(1);
    ck(done_status[1] == 2 && r->status == 2);
    report("content: put, get, not found");

    /* Content delivered at a frame's start is charged in that frame: the
     * get of 100 bytes submitted in frame F (by count_frames's extra call)
     * completes at the start of F + 1, which has 25 units less. */
    begin();
    uint32_t got[2], want[2];
    get_req(r, put_id, 0, 100, get_dst);
    done_reset();
    count_frames_rom(got, 2, C_SUBMIT, A(r));
    collect();
    ck(done_count == 1 && done_status[1] == 1 && r->status == 1 && r->length == 100);
    m_start();
    m_dframe = 1;
    m_dcharge = (100 + COPY_BYTES - 1) / COPY_BYTES;
    model_frames(want, 2, ROM_COST, 0);
    for (uint32_t i = 0; i < 2; i++) {
        ck(got[i] == want[i]);
        val_u(got[i]);
    }
    report("content: delivery charge, ROM loop");

    /* Bad requests: rejected, status 04h. */
    begin();
    static const struct {
        uint8_t op;
        uint32_t length, addr, size;
    } bad[] = {
        {0x00, 1, RAM_BASE, 1},              /* unknown op */
        {0x05, 1, RAM_BASE, 1},
        {0xFF, 1, RAM_BASE, 1},
        {0x01, 0, RAM_BASE, 0},              /* get, length 0 */
        {0x01, 16385, RAM_BASE, 0},          /* get, length 16,385 */
        {0x01, 0x10000, RAM_BASE, 0},
        {0x01, 2, RAM_END - 1, 0},           /* get past the end of RAM */
        {0x01, 1, ROM_BASE, 0},              /* get into ROM */
        {0x01, 1, 0, 0},
        {0x02, 0, RAM_BASE, 0},              /* put, size 0 */
        {0x02, 0, RAM_BASE, 1048577},        /* put, size 1,048,577 */
        {0x02, 0, RAM_END - 10, 11},         /* put past the end of RAM */
        {0x02, 0, 0, 1},                     /* put from address 0 */
        {0x03, 0, RAM_BASE, 0},              /* subscriptions, length 0 */
        {0x03, 36, ROM_BASE, 0},             /* subscriptions into ROM */
        {0x04, 16385, RAM_BASE, 0},          /* timeline, 16,385 */
        {0x04, 4, RAM_END - 3, 0},           /* timeline past the end of RAM */
    };
    for (uint32_t i = 0; i < sizeof bad / sizeof bad[0]; i++) {
        blk_clear(r);
        r->op = bad[i].op;
        r->length = bad[i].length;
        r->addr = bad[i].addr;
        r->size = bad[i].size;
        ck(submit(r) == 0);
        ck(r->status == 4);
    }
    blk_clear(r); /* put past the end of ROM */
    r->op = 2;
    r->addr = ROM_END - 1;
    r->size = 2;
    ck(submit(r) == 0);
    ck(r->status == 4);
    /* Blocks not wholly in RAM: rejected, nothing written. */
    ck(call(C_SUBMIT, ROM_BASE, 0, 0) == 0);
    ck(call(C_SUBMIT, 0, 0, 0) == 0);
    ck(call(C_SUBMIT, RAM_END, 0, 0) == 0);
    uint8_t *eb = (uint8_t *)(RAM_END - 51); /* straddles the end by one byte */
    memset(eb, 0, 51);
    eb[0] = 1;
    eb[1] = 0xAA;
    ck(call(C_SUBMIT, A(eb), 0, 0) == 0);
    ck(eb[1] == 0xAA);
    report("content: bad requests, blocks outside RAM");

    /* Eight pending requests (one block at an odd address, one ending at
     * the last byte of RAM); a ninth is rejected and its block left alone;
     * once they complete a handle is free again. */
    begin();
    uint8_t *b8[9];
    for (uint32_t i = 0; i < 9; i++)
        b8[i] = (uint8_t *)&blk[i];
    b8[6] = BB + 8193;
    b8[7] = (uint8_t *)(RAM_END - 52);
    done_reset();
    for (uint32_t i = 0; i < 9; i++) {
        static req tmp;
        get_req(&tmp, absent, 0, 1, get_dst);
        memcpy(b8[i], &tmp, sizeof tmp);
        ck(call(C_SUBMIT, A(b8[i]), 0, 0) == (i < 8 ? i + 1 : 0));
        ck(b8[i][1] == (i < 8 ? 0 : 0xAA));
    }
    wait_done(8);
    for (uint32_t i = 1; i <= 8; i++)
        ck(done_status[i] == 2 && b8[i - 1][1] == 2);
    ck(b8[8][1] == 0xAA);
    done_reset();
    ck(call(C_SUBMIT, A(b8[8]), 0, 0) == 1);
    wait_done(1);
    ck(done_status[1] == 2 && b8[8][1] == 2);
    report("content: eight pending, ninth rejected");

    /* Subscriptions and timeline: done (the records depend on the host's
     * feeds, so only the status is checked). */
    begin();
    blk_clear(r);
    r->op = 3;
    r->length = 1440;
    r->addr = A(BB);
    blk_clear(&blk[1]);
    blk[1].op = 4;
    blk[1].length = 4096;
    blk[1].addr = A(BB + 4096);
    done_reset();
    ck(submit(r) == 1);
    ck(submit(&blk[1]) == 2);
    wait_done(2);
    ck(done_status[1] == 1 && done_status[2] == 1);
    ck(r->status == 1 && blk[1].status == 1);
    report("content: subscriptions, timeline");
}

/* ============================================================ 9. stress */

#define STRESS_START 48 /* the content checks end at about frame 45 */
#define STRESS_A 30     /* frames of list A: mixed shapes */
#define STRESS_B 30     /* frames of list B: the fill cap's worst legal case */

static uint32_t rnd_state = 0x2545F491;

static uint32_t rnd(void)
{
    uint32_t x = rnd_state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    return rnd_state = x;
}

static uint8_t *put8(uint8_t *p, uint32_t v)
{
    *p = v;
    return p + 1;
}

static uint8_t *put16(uint8_t *p, uint32_t v)
{
    p[0] = v;
    p[1] = v >> 8;
    return p + 2;
}

static uint32_t rx(void) { return 1024 + (rnd() & 8191); }
static uint32_t ry(void) { return 1792 + (rnd() & 4095); }

/* List A: groups of colour, width, move, line, rectangle, triangle, move and
 * 30 concentric circles and discs, then End bytes to 16,384. The fill cap
 * stops the discs and rectangles after the first few groups. */
static void build_a(uint8_t *l)
{
    uint8_t *p = l;
    p = put8(p, 0x03); /* origin, changed every frame */
    p = put16(p, 0);
    p = put16(p, 0);
    while (p - l < 16384 - 126) {
        uint32_t c = rnd();
        p = put8(p, 0x01);
        p = put8(p, c);
        p = put8(p, c >> 8);
        p = put8(p, c >> 16);
        p = put8(p, 0xC0 | c >> 24);
        p = put8(p, 0x02);
        p = put8(p, 8 + (rnd() & 63));
        p = put8(p, 0x04);
        p = put16(p, rx());
        p = put16(p, ry());
        p = put8(p, 0x05);
        p = put16(p, rx());
        p = put16(p, ry());
        p = put8(p, 0x08);
        p = put16(p, rx());
        p = put16(p, ry());
        p = put8(p, 0x09);
        p = put16(p, rx());
        p = put16(p, ry());
        p = put16(p, rx());
        p = put16(p, ry());
        p = put8(p, 0x04);
        p = put16(p, rx());
        p = put16(p, ry());
        for (uint32_t k = 0; k < 30; k++) {
            p = put8(p, 0x06 + (k & 1)); /* circle, disc */
            p = put16(p, 16 + 24 * k);
        }
    }
    memset(p, 0, 16384 - (p - l));
}

/* List B, the worst legal fill: three discs that cover the screen and a
 * rectangle 639 x 480 units (1,228,320 square units of fill, just under the
 * cap of 1,228,800, all drawn), then the rest of the 16,384 bytes in lines
 * of the greatest width (255, 16 units) zig-zagging across the whole
 * screen: 3,267 lines, each about 640 x 16 units, about 108 screens of
 * area in all (lines are not capped). Translucent, so the host must blend
 * every one. */
static void build_b(uint8_t *l)
{
    uint8_t *p = l;
    p = put8(p, 0x03); /* origin, changed every frame */
    p = put16(p, 0);
    p = put16(p, 0);
    p = put8(p, 0x01);
    p = put8(p, 0xFF);
    p = put8(p, 0x60);
    p = put8(p, 0x20);
    p = put8(p, 0x10);
    p = put8(p, 0x04);
    p = put16(p, 5120);
    p = put16(p, 3840);
    for (int k = 0; k < 3; k++) {
        p = put8(p, 0x07);
        p = put16(p, 0xFFFF);
    }
    p = put8(p, 0x04);
    p = put16(p, 0);
    p = put16(p, 0);
    p = put8(p, 0x08);
    p = put16(p, 10224);
    p = put16(p, 7680);
    p = put8(p, 0x01);
    p = put8(p, 0x40);
    p = put8(p, 0xA0);
    p = put8(p, 0xFF);
    p = put8(p, 0x20);
    p = put8(p, 0x02);
    p = put8(p, 255);
    uint32_t k = 0;
    while (p - l <= 16384 - 5) {
        p = put8(p, 0x05);
        p = put16(p, k & 1 ? 0 : 10240);
        p = put16(p, (k * 37) % 7681);
        k++;
    }
    memset(p, 0, 16384 - (p - l));
}

static uint8_t rowbuf[256];
static uint32_t cjk_next = 0x4E00;

/* The next code point of CJK Unified Ideographs, then Extension A, then
 * Hangul syllables: 38,756 wide characters, none shown twice. */
static uint32_t next_cjk(void)
{
    uint32_t c = cjk_next++;
    if (cjk_next == 0xA000)
        cjk_next = 0x3400;
    else if (cjk_next == 0x4DC0)
        cjk_next = 0xAC00;
    else if (cjk_next == 0xD7A4)
        cjk_next = 0x4E00;
    return c;
}

/* Rewrites all 2,400 cells: even rows 40 CJK characters, odd rows 80 Latin
 * Extended-A or Cyrillic letters; colours change every frame. */
static void stress_text(uint32_t sf)
{
    for (uint32_t row = 0; row < 30; row++) {
        uint8_t *p = rowbuf;
        at(0, row);
        colours(9 + ((sf + row) & 7), 232 + (row & 15));
        if (!(row & 1)) {
            for (int i = 0; i < 40; i++) {
                uint32_t c = next_cjk();
                *p++ = 0xE0 | c >> 12;
                *p++ = 0x80 | (c >> 6 & 0x3F);
                *p++ = 0x80 | (c & 0x3F);
            }
        } else {
            uint32_t base = row >> 1 & 1 ? 0x400 : 0x100;
            for (uint32_t i = 0; i < 80; i++) {
                uint32_t c = base + ((sf + i) & 0x7F);
                *p++ = 0xC0 | c >> 6;
                *p++ = 0x80 | (c & 0x3F);
            }
        }
        twn(rowbuf, p - rowbuf);
    }
}

static void t_stress(void)
{
    begin();
    build_a(BA);
    build_b(BB);
    wait_until(STRESS_START);
    uint32_t f = wait(), sf;
    for (sf = 0; sf < STRESS_A + STRESS_B; sf++) {
        ck(frame() == f + sf); /* the work of a frame fits in it */
        stress_text(sf);
        uint8_t *l = sf < STRESS_A ? BA : BB;
        l[1] = l[3] = (sf & 15) << 4; /* move it: a new list every frame */
        ck(call(V_SUBMIT, A(l), 16384, 0) == 0);
        while (frame() == f + sf) /* use the whole budget */
            ;
    }
    val_u(sf);
    report("stress frames run");
}

/* ============================================================== 10. page */

#define PAGE_START 110  /* the calls; then the height events */
#define PAGE_CLICK 140  /* conform.events presses the pointer in this frame */
#define ANCHOR_ID 0x2A17
#define PX 40           /* the rectangle: x, y, the wide and narrow widths, height */
#define PY 40
#define PW 560
#define PW_NARROW 140
#define PH 400

/* Big anchored text at the top left (the press target, wherever a host's
 * fonts put it), then a paragraph that wraps to more lines when the
 * rectangle is narrower. */
#define PAGE_TEXT                                                                  \
    "The page device lays out this paragraph with the host's own fonts. The "      \
    "conformance image only learns its height, which arrives as an event after "   \
    "the submit, and the anchor of text under a pointer press. A narrower "        \
    "rectangle gives more lines and so a greater height; a clear gives a height "  \
    "of zero."
#define PAGE_TEXT_LEN (sizeof PAGE_TEXT - 1)
static const struct {
    uint8_t head[54];
    char text[PAGE_TEXT_LEN] __attribute__((nonstring)); /* no NUL */
    uint8_t end;
} page_doc = {
    {
        0x03, 48, 0x02, 0, 1, 0x04, 255, 220, 120, 0x07, ANCHOR_ID & 0xFF, ANCHOR_ID >> 8,
        0x01, 19, 0, 'M', 'M', 'M', 'M', ' ', 'M', 'M', 'M', 'M', ' ', 'M', 'M', 'M', 'M', ' ', 'M', 'M', 'M', 'M',
        0x07, 0, 0, 0x02, 0, 0, 0x03, 14, 0x04, 229, 229, 229, 0x05, 0, 0, 8, 0,
        0x01, PAGE_TEXT_LEN & 0xFF, PAGE_TEXT_LEN >> 8,
    },
    PAGE_TEXT,
    0x00,
};
static uint8_t page_ram[sizeof page_doc];
/* The frame each height event arrived in, and the heights (in RAM, so the
 * state hash covers the exact frames of the headless run). */
static uint32_t page_frames[8], page_heights[8], page_events;

/* Waits for a height event (08h) until frame `last`; returns it, or
 * 0xFFFFFFFF (and a failed check) if none came. Other events are dropped;
 * more than one height event in a frame fails a check. */
static uint32_t page_height(uint32_t last)
{
    static uint8_t e[4];
    for (;;) {
        uint32_t n = 0, h = 0;
        while (call(NEXT_EVENT, A(e), 0, 0) == 1) {
            if (e[0] == 8) {
                n++;
                h = e[1] | (uint32_t)e[2] << 8 | (uint32_t)e[3] << 16;
            }
        }
        ck(n <= 1);
        if (n) {
            if (page_events < 8) {
                page_frames[page_events] = frame();
                page_heights[page_events] = h;
            }
            page_events++;
            return h;
        }
        if (frame() >= last) {
            ck(0);
            return 0xFFFFFFFF;
        }
        wait();
    }
}

/* Whether a height event is queued (they are all taken). */
static uint32_t page_pending(void)
{
    static uint8_t e[4];
    uint32_t n = 0;
    while (call(NEXT_EVENT, A(e), 0, 0) == 1)
        n += e[0] == 8;
    return n;
}

static void t_page(void)
{
    /* The calls, in one frame. */
    begin();
    wait_until(PAGE_START);
    ck(page_pending() == 0); /* nothing changed yet: no height */
    memcpy(page_ram, &page_doc, sizeof page_doc);
    ck(call4(P_REGION, 600, 400, 100, 100) == 0);         /* clamped to 40 x 80 */
    ck(call4(P_REGION, 0xFFFFFFFF, 0xFFFFFFFF, 5, 5) == 0); /* to 640, 480, 0, 0 */
    ck(call(P_SUBMIT, A(&page_doc), sizeof page_doc, 0) == 0); /* from ROM */
    ck(call(P_SUBMIT, A(page_ram), sizeof page_doc, 0) == 0); /* from RAM */
    ck(call(P_SUBMIT, A(page_ram), 0, 0) == 1);           /* length 0 */
    ck(call(P_SUBMIT, A(page_ram), 65537, 0) == 1);       /* too long */
    ck(call(P_SUBMIT, 0, 0, 0) == 1);                     /* the length first */
    ck(call(P_SUBMIT, 0, 5, 0) == ERR_BUFFER);            /* null */
    ck(call(P_SUBMIT, RAM_END - 4, 8, 0) == ERR_BUFFER);  /* past the end of RAM */
    ck(call(P_SUBMIT, ROM_END - 4, 8, 0) == ERR_BUFFER);  /* past the end of ROM */
    ck(call(P_SUBMIT, RAM_BASE - 2, 4, 0) == ERR_BUFFER);
    ck(call(P_SUBMIT, RAM_END - 65535, 65536, 0) == ERR_BUFFER);
    ck(call(P_SUBMIT, RAM_END - 65536, 65536, 0) == 0);   /* 65,536 bytes ending at the end of RAM */
    ck(call(P_SUBMIT, RAM_END - 1, 1, 0) == 0);           /* the last byte */
    ck(call(P_SUBMIT, ROM_END - 1, 1, 0) == 0);           /* the last byte of ROM */
    ck(call(P_SCROLL, 0x7FFFFFFF, 0, 0) == 0);
    ck(call(P_BACKGROUND, 0x80402010, 0, 0) == 0);
    ck(call(P_CLEAR, 0, 0, 0) == 0);
    ck(call(P_CLEAR, 0, 0, 0) == 0);
    ck(call(0x705, 0, 0, 0) == ERR_UNKNOWN);
    ck(call(0x7FF, 0, 0, 0) == ERR_UNKNOWN);
    /* The state the frame ends with: the document, the wide rectangle, a
     * negative scroll value (shown as 0). */
    ck(call4(P_REGION, PX, PY, PW, PH) == 0);
    ck(call(P_SUBMIT, A(page_ram), sizeof page_doc, 0) == 0);
    ck(call(P_SCROLL, (uint32_t)-40, 0, 0) == 0);
    report("page: calls, rejections, buffers");

    /* Height events: only what any host's layout must satisfy. */
    begin();
    uint32_t last = PAGE_CLICK - 1, wide, h;
    wide = page_height(last); /* the frame after the submit */
    ck(wide > 0 && wide != 0xFFFFFFFF);
    call(P_SUBMIT, A(page_ram), sizeof page_doc, 0); /* the same bytes: a new document */
    ck(page_height(last) == wide);
    call4(P_REGION, PX, PY, PW_NARROW, PH);          /* narrower: more lines */
    h = page_height(last);
    ck(h >= wide && h != 0xFFFFFFFF);
    call4(P_REGION, PX, PY, PW, PH);
    ck(page_height(last) == wide);
    call4(P_REGION, PX, PY, 0, PH);                  /* no width: no layout, no event */
    call(P_SUBMIT, A(page_ram), sizeof page_doc, 0);
    for (int i = 0; i < 3; i++) {
        wait();
        ck(page_pending() == 0);
    }
    call4(P_REGION, PX, PY, PW, PH);                 /* a width again: the event */
    ck(page_height(last) == wide);

    /* Anchor events: the presses of conform.events in frame PAGE_CLICK. */
    wait_until(PAGE_CLICK + 1);
    static uint8_t e[4], prev[4];
    uint32_t anchors = 0;
    while (call(NEXT_EVENT, A(e), 0, 0) == 1) {
        if (e[0] == 9) {
            /* After the pointer event of the press, with its buttons. */
            ck(prev[0] == 4 && (prev[3] >> 3 & 7) == e[3]);
            ck((e[1] | e[2] << 8) == ANCHOR_ID);
            ck(e[3] == (anchors ? 2 : 1));
            anchors++;
        }
        memcpy(prev, e, 4);
    }
    ck(anchors == 2);
    val_u(anchors);

    /* A clear: height 0. Then the page is hidden, with a negative scroll
     * value (a clear keeps it). */
    call(P_CLEAR, 0, 0, 0);
    ck(page_height(PAGE_CLICK + 10) == 0);
    call4(P_REGION, 0, 0, 0, 0);
    call(P_SCROLL, (uint32_t)-40, 0, 0);
    report("page: height and anchor events");
}

/* ================================================================== main */

static uint8_t final_list[512];

int main(void)
{
    t_cpu();
    t_ram_code();
    t_terminal();
    t_vector();
    t_system();
    show_all();
    t_events();
    t_pads();
    show_all();
    t_budget();
    show_all();
    t_content();
    show_all();
    t_stress();
    t_page();

    /* The results. */
    uint32_t failed = 0;
    for (uint32_t i = 0; i < n_results; i++)
        failed += results[i].fails != 0;
    show_all();
    tw("\n");
    if (failed) {
        colours(9, 0x100);
        tw("Sections failed: ");
        put_u(failed);
    } else {
        colours(10, 0x100);
        tw("All sections pass.");
    }
    colours(7, 0x100);
    tw("  Esc: exit (code = sections failed).");
    call(V_SUBMIT, A(final_list), build_final(final_list), 0);
    for (;;) {
        static uint8_t e[4];
        wait();
        while (call(NEXT_EVENT, A(e), 0, 0) == 1)
            if (e[0] == 1 && e[1] == 0x29)
                call(EXIT, failed, 0, 0);
    }
}
