/* sheet: a small spreadsheet on the page (device 7), dark text on a white
 * page in proportional type, with the 2D vector display (device 3) drawn
 * over it for the grid lines and the selection; built with clang (see
 * build.sh).
 *
 * The sheet. 26 columns (A-Z) by 99 rows. A cell holds what was typed, at
 * most 120 bytes: a number ("1250", "-3.5", "1,250.50", "8.25%"), text (a
 * leading ' keeps text that looks like a number), or a formula starting
 * with "=". The screen shows 7 columns and 18 rows of it (8 rows when a
 * chart is shown); it scrolls with the selection, PageUp/PageDown and the
 * wheel.
 *
 * Numbers are signed 64-bit fixed point with 4 decimal places (a value is
 * an integer count of 1/10,000ths): exact for money-like sums, from
 * -922,337,203,685,477.5807 to 922,337,203,685,477.5807. Products and quotients
 * are computed exactly (128 bits) and rounded half away from zero to 4
 * places; a result outside the range is #NUM. A number is shown with
 * thousands separators and its decimals without trailing zeros, but with
 * at least 2 if it has any (12.5 shows as 12.50); one typed with % shows
 * as a percentage. A number too wide for its column loses its decimals,
 * then its separators, then shows as ####.
 *
 * Formulas: numbers, cell references (A1, a1), ranges (A1:B5) as function
 * arguments, + - * / ^ (a whole exponent, -64 to 64), unary minus, %
 * after a number or reference, parentheses, the comparisons = <> < > <=
 * >= (1 or 0), text in double quotes, and the functions SUM, AVERAGE,
 * MIN, MAX, COUNT, ROUND(x, places), ABS and IF(test, then[, else]).
 * Empty cells count 0; in ranges, text and empty cells are skipped (COUNT
 * counts numbers). Errors are values that spread to the cells that use
 * them: #DIV/0, #CYCLE (a cell that depends on itself, and every cell of
 * the cycle), #ERR (a formula that cannot be read), #REF (a reference
 * outside the sheet), #VALUE (text where a number is needed), #NUM (out
 * of range).
 *
 * Recalculation. After every change every formula is recalculated in
 * dependency order: a depth-first walk with an explicit stack that
 * computes a cell's references before the cell, and finds cycles where it
 * meets a cell that is still on the stack. The walk does at most
 * WORK_PER_FRAME steps of work a frame and goes on in the next frame, so
 * no frame runs long however large the sheet (the status says
 * "Calculating" meanwhile; the cells show their old values until then).
 *
 * The screen is one page document: the entry line (the selected cell's
 * name and content; while editing, the text with a caret), a table of 8
 * columns (the row numbers and 7 sheet columns), the chart if there is
 * one, and the status line. Every column is 80 screen units wide, on every
 * host: a last row of invisible text (white on white, size 6) makes each
 * column's natural width larger than 640/8, so the page narrows them all
 * to the same cap (SPEC §page.layout step 3). That is what lets the
 * vector display draw the vertical grid lines and the selection box at the
 * right x; the rows' y comes from `locate` (the row numbers are anchors),
 * so a host's fonts and line heights do not matter. A table column has one
 * alignment (SPEC §page.document), so a column is right-aligned when it
 * holds at least as many numbers as text, else left-aligned. A cell's text
 * is cut with an ellipsis where it would not fit (by an estimate of its
 * width), so that rows stay one line high.
 *
 * Keys. Arrows, Tab and Enter (with Shift: back) move; Shift+arrows or a
 * drag select a range; a click selects a cell. Typing starts a new entry,
 * F2 edits the cell's content; while editing, Left, Right, Home, End,
 * Backspace and Delete edit, Enter, Tab, Up and Down keep the entry and
 * move, Esc cancels. Delete clears the selection. Ctrl+C copies the
 * selection, Ctrl+Y pastes it at the selected cell, Ctrl+D fills the
 * selection down from its top row and Ctrl+R right from its left column
 * (references move with the copy: relative references only). Ctrl+G shows
 * a bar chart of the selection, Ctrl+L a line chart (the same key again
 * removes it). Ctrl+S saves, Ctrl+P publishes, Ctrl+N twice starts a new
 * sheet.
 *
 * Chart. One series: the values of the selection's first column that
 * holds numbers (a selection one row high: its row), at most 32; the
 * labels are the text of the nearest column to the left of it (or row
 * above it) that has text in those rows. It is a page figure under the
 * grid; with up to 8 values, a table under it gives each bar its label
 * and value, aligned with the bars by the same equal-columns rule.
 *
 * Saved form ("sheet" format): plain UTF-8 text, one line per row from row
 * 1 to the last row used, the row's cells up to its last one used
 * separated by tabs, each exactly as typed (formulas with their "=").
 * After the rows, a chart is one more line: a form feed (0Ch), "chart",
 * a space, "bar" or "line", a space and the range ("chart bar D3:D7").
 * Lines end in LF (a CR before it is ignored); rows past 99 and cells past
 * Z are ignored on reading; cells longer than 120 bytes are cut. At most
 * 65,536 bytes.
 *
 * Documents. Ctrl+S saves the sheet as the record "sheet" (SPEC
 * §content.records), which is loaded at start (if there is none, the
 * example, an estimate for a service, is shown). Ctrl+P publishes it as a
 * document (SPEC §content.documents: "TDOC", 0, 32 zero bytes, 5,
 * "sheet", then the saved form) titled with A1's text (or "Sheet"), after
 * the host asks the user. Started with a sheet document as its argument,
 * it opens that document instead; Ctrl+S then keeps it as the user's own
 * sheet. Content (device 4) is optional: without it nothing is loaded or
 * kept, and Ctrl+S and Ctrl+P say so. The 2D display (device 3) is
 * optional too: without it there are no grid lines and the selected cell
 * is shown by its colour (and that of its row number and column letter). */

#include <stdint.h>
#include "terse.h"

typedef int64_t i64;
typedef uint64_t u64;

#define NCOLS 26
#define NROWS 99
#define NCELLS (NCOLS * NROWS)
#define VCOLS 7
#define VROWS_ALL 17
#define VROWS_CHART 10
#define COLW 80
#define RAW_MAX 120
#define CODE_MAX 200
#define POOL 65520
#define DISP_MAX 20
#define DATA_MAX 65536
#define CHUNK 16384
#define FMT_NAME_LEN 5
#define HEAD (TDOC_HEAD + FMT_NAME_LEN)
#define SCALE 10000
#define WORK_PER_FRAME 1200
#define LOAD_BYTES 1024
#define SAVE_ROWS 6
#define BARS_MAX 32
#define CHART_H 90
#define STACK_MAX 40

#define KEY_C 0x06
#define KEY_D 0x07
#define KEY_G 0x0A
#define KEY_L 0x0F
#define KEY_N 0x11
#define KEY_P 0x13
#define KEY_R 0x15
#define KEY_S 0x16
#define KEY_Y 0x1C
#define KEY_F2 0x3B
#define KEY_DELETE 0x4C
#define MOD_SHIFT 0x01
#define MOD_CTRL 0x02

/* Anchors of the page document. */
#define A_HEAD 2000
#define A_ROW 1000
#define A_BOTTOM 999

/* Value kinds. */
enum { K_EMPTY, K_NUM, K_STR, K_ERR, K_RANGE };
/* Errors. */
enum { E_NONE, E_DIV, E_CYCLE, E_ERR, E_REF, E_VALUE, E_NUM };
static const char *const err_text[] = { "", "#DIV/0", "#CYCLE", "#ERR", "#REF", "#VALUE", "#NUM" };

/* Cell flags. */
#define F_FORMULA 0x01
#define F_TEXT 0x02
#define F_NUM 0x04
#define F_PCT 0x08
#define F_BAD 0x10 /* a formula that cannot be read: its code is the error */
/* Recalculation states. */
enum { S_CLEAN, S_DIRTY, S_BUSY };

/* Formula code: reverse Polish, each op a byte and its operands. */
enum {
    OP_NUM = 1, /* 8 bytes */
    OP_STR,     /* length, bytes */
    OP_REF,     /* row, column */
    OP_RANGE,   /* r0, c0, r1, c1 */
    OP_NEG,
    OP_PCT,
    OP_ADD,
    OP_SUB,
    OP_MUL,
    OP_DIV,
    OP_POW,
    OP_EQ,
    OP_NE,
    OP_LT,
    OP_GT,
    OP_LE,
    OP_GE,
    OP_FN,    /* function, argument count */
    OP_SMALL, /* a whole number, 2 bytes, signed */
};
enum { FN_SUM, FN_AVERAGE, FN_MIN, FN_MAX, FN_COUNT, FN_ROUND, FN_ABS, FN_IF, FN_N };
static const char *const fn_name[FN_N] = { "SUM", "AVERAGE", "MIN", "MAX", "COUNT", "ROUND", "ABS", "IF" };
static const uint8_t fn_min[FN_N] = { 1, 1, 1, 1, 1, 1, 1, 2 };
static const uint8_t fn_max[FN_N] = { 30, 30, 30, 30, 30, 2, 1, 3 };

/* --- The cells ------------------------------------------------------------ */

/* The pool holds each cell's block: owner (2 bytes, FFFFh once dead), then
 * the text as typed and the formula's code. */
static uint8_t pool[POOL];
static uint32_t pool_used;
static uint16_t c_off[NCELLS]; /* the text's offset in the pool */
static uint8_t c_rlen[NCELLS], c_clen[NCELLS], c_flag[NCELLS];
static uint8_t c_kind[NCELLS], c_state[NCELLS];
static i64 c_val[NCELLS];
static uint8_t c_disp[NCELLS][DISP_MAX];
static uint8_t c_dlen[NCELLS];

static uint16_t idx(uint32_t r, uint32_t c) { return (uint16_t)(r * NCOLS + c); }

/* --- Numbers -------------------------------------------------------------- */

/* n / d for d below 65,536, the remainder in *rem. */
static u64 divs(u64 n, uint32_t d, uint32_t *rem)
{
    uint32_t hi = (uint32_t)(n >> 32), lo = (uint32_t)n, r = 0, x, q3, q2, q1, q0;
    x = hi >> 16;
    q3 = x / d;
    r = x % d;
    x = r << 16 | (hi & 0xFFFF);
    q2 = x / d;
    r = x % d;
    x = r << 16 | lo >> 16;
    q1 = x / d;
    r = x % d;
    x = r << 16 | (lo & 0xFFFF);
    q0 = x / d;
    r = x % d;
    if (rem)
        *rem = r;
    return (u64)(q3 << 16 | q2) << 32 | (q1 << 16 | q0);
}

/* The 128-bit product of a and b, w[0] lowest. */
static void mul128(u64 a, u64 b, uint32_t w[4])
{
    u64 a0 = (uint32_t)a, a1 = a >> 32, b0 = (uint32_t)b, b1 = b >> 32;
    u64 p00 = a0 * b0, p01 = a0 * b1, p10 = a1 * b0, p11 = a1 * b1;
    u64 mid = (p00 >> 32) + (uint32_t)p01 + (uint32_t)p10;
    w[0] = (uint32_t)p00;
    w[1] = (uint32_t)mid;
    u64 high = (mid >> 32) + (p01 >> 32) + (p10 >> 32) + p11;
    w[2] = (uint32_t)high;
    w[3] = (uint32_t)(high >> 32);
}

/* Fits a magnitude and sign into an i64; 0 if out of range. */
static int fit(u64 m, int neg, i64 *out)
{
    if (m > (u64)INT64_MAX)
        return 0;
    *out = neg ? -(i64)m : (i64)m;
    return 1;
}

static u64 mag(i64 v) { return v < 0 ? (u64)0 - (u64)v : (u64)v; }

static int add_fix(i64 a, i64 b, i64 *out)
{
    i64 r = (i64)((u64)a + (u64)b);
    if (((a ^ r) & (b ^ r)) < 0)
        return 0;
    *out = r;
    return 1;
}

/* a × b / SCALE, rounded half away from zero; 0 if out of range. */
static int mul_fix(i64 a, i64 b, i64 *out)
{
    uint32_t w[4], r = 0;
    int i;
    mul128(mag(a), mag(b), w);
    /* Divide the 128 bits by SCALE, 16 bits at a time. */
    for (i = 3; i >= 0; i--) {
        uint32_t x = r << 16 | w[i] >> 16, qh = x / SCALE;
        r = x % SCALE;
        x = r << 16 | (w[i] & 0xFFFF);
        w[i] = qh << 16 | x / SCALE;
        r = x % SCALE;
    }
    if (w[3] || w[2])
        return 0;
    u64 q = (u64)w[1] << 32 | w[0];
    if (2 * r >= SCALE)
        q++;
    return fit(q, (a < 0) != (b < 0), out);
}

/* a × SCALE / b, rounded; 0 if out of range (b is not 0). */
static int div_fix(i64 a, i64 b, i64 *out)
{
    uint32_t w[4];
    u64 d = mag(b), rem = 0, q = 0;
    int i;
    mul128(mag(a), SCALE, w);
    for (i = 127; i >= 0; i--) {
        uint32_t bit = w[i >> 5] >> (i & 31) & 1;
        if (!rem && !bit && !q)
            continue;
        rem = rem << 1 | bit;
        if (q >> 63)
            return 0;
        q <<= 1;
        if (rem >= d) {
            rem -= d;
            q |= 1;
        }
    }
    if (rem >= d - rem)
        q++;
    return fit(q, (a < 0) != (b < 0), out);
}

/* --- Showing a value ------------------------------------------------------ */

/* Estimated advance of a character of size-12 sans text, in tenths of a
 * screen unit: enough to cut text before it wraps in a column. */
static uint32_t est(uint8_t ch)
{
    if (ch >= 0x80)
        return ch >= 0xC0 ? 90 : 0; /* continuation bytes add nothing */
    if (ch >= '0' && ch <= '9')
        return 77;
    if (ch == ' ')
        return 39;
    if (ch == '.' || ch == ',' || ch == ':' || ch == ';' || ch == 'i' || ch == 'l' || ch == 'j' || ch == '!' || ch == '|'
        || ch == '\'')
        return 36;
    if (ch == 'f' || ch == 't' || ch == 'r' || ch == '(' || ch == ')' || ch == '-' || ch == 'I')
        return 50;
    if (ch == 'm' || ch == 'M' || ch == 'W' || ch == '%' || ch == '@')
        return 120;
    if (ch == 'w')
        return 100;
    if (ch >= 'A' && ch <= 'Z')
        return 86;
    return 75;
}
#define CELL_WIDTH 650 /* tenths: 80 less 12 of padding, less a margin */

static uint32_t est_width(const uint8_t *p, uint32_t n)
{
    uint32_t w = 0;
    while (n--)
        w += est(*p++);
    return w;
}

/* Writes the digits of v (an integer) with or without separators. */
static uint32_t put_int(u64 v, int commas, uint8_t *out)
{
    uint8_t d[24];
    uint32_t n = 0, k = 0, rem;
    do {
        if (commas && n && n % 3 == 0)
            d[k++] = ',';
        v = divs(v, 10, &rem);
        d[k++] = (uint8_t)('0' + rem);
        n++;
    } while (v);
    for (n = 0; n < k; n++)
        out[n] = d[k - 1 - n];
    return k;
}

/* A number as shown in a cell of `width` tenths (0: no limit). */
static uint32_t format_num(i64 v, int pct, uint32_t width, uint8_t *out)
{
    uint8_t buf[40];
    u64 m;
    uint32_t frac, n, i, try;
    if (pct && !mul_fix(v, 100 * SCALE, &v))
        pct = 0;
    m = mag(v);
    for (try = 0; try < 3; try++) {
        u64 ip = divs(m, SCALE, &frac);
        n = 0;
        if (try > 0) {
            if (2 * frac >= SCALE)
                ip++;
            frac = 0;
        }
        if (v < 0 && (ip || frac))
            buf[n++] = '-';
        n += put_int(ip, try < 2, buf + n);
        if (frac) {
            uint8_t f[4];
            uint32_t k = 4;
            for (i = 0; i < 4; i++) {
                f[3 - i] = (uint8_t)('0' + frac % 10);
                frac /= 10;
            }
            while (k > 2 && f[k - 1] == '0')
                k--;
            buf[n++] = '.';
            for (i = 0; i < k; i++)
                buf[n++] = f[i];
        }
        if (pct)
            buf[n++] = '%';
        if (!width || est_width(buf, n) <= width)
            break;
    }
    if (try == 3) {
        memcpy(out, "####", 4);
        return 4;
    }
    memcpy(out, buf, n);
    return n;
}

/* Text cut to `width` tenths, with an ellipsis if cut. */
static uint32_t cut_text(const uint8_t *p, uint32_t n, uint32_t width, uint8_t *out, uint32_t cap)
{
    uint32_t w = 0, i = 0, keep = 0;
    if (est_width(p, n) <= width && n <= cap) {
        memcpy(out, p, n);
        return n;
    }
    width -= 90; /* the ellipsis */
    while (i < n) {
        uint32_t len = 1;
        if (p[i] >= 0xC0)
            while (i + len < n && (p[i + len] & 0xC0) == 0x80)
                len++;
        w += est(p[i]);
        if (w > width || i + len + 3 > cap)
            break;
        i += len;
        keep = i;
    }
    memcpy(out, p, keep);
    memcpy(out + keep, "\xE2\x80\xA6", 3);
    return keep + 3;
}

/* The string value packed in an i64: pool offset and length. */
#define STR(off, len) ((i64)(off) | (i64)(len) << 16)
#define STR_OFF(v) ((uint32_t)(v) & 0xFFFF)
#define STR_LEN(v) ((uint32_t)((v) >> 16) & 0xFF)

/* A cell's raw text, without the ' that marks text. */
static const uint8_t *text_of(uint16_t i, uint32_t *n)
{
    const uint8_t *p = pool + c_off[i];
    *n = c_rlen[i];
    if (*n && p[0] == '\'') {
        p++;
        (*n)--;
    }
    return p;
}

static void update_disp(uint16_t i)
{
    uint8_t *d = c_disp[i];
    uint32_t n = 0;
    const uint8_t *p;
    uint8_t kind = c_flag[i] & F_FORMULA ? c_kind[i] : c_flag[i] & F_NUM ? K_NUM : c_flag[i] & F_TEXT ? K_STR : K_EMPTY;
    if (kind == K_NUM)
        n = format_num(c_val[i], c_flag[i] & F_PCT, CELL_WIDTH, d);
    else if (kind == K_ERR) {
        p = (const uint8_t *)err_text[c_val[i]];
        while (p[n]) {
            d[n] = p[n];
            n++;
        }
    } else if (kind == K_STR) {
        uint32_t len;
        if (c_flag[i] & F_FORMULA) {
            p = pool + STR_OFF(c_val[i]);
            len = STR_LEN(c_val[i]);
        } else
            p = text_of(i, &len);
        n = cut_text(p, len, CELL_WIDTH, d, DISP_MAX);
    }
    c_dlen[i] = (uint8_t)n;
}

/* --- Formulas: reading ---------------------------------------------------- */

static const uint8_t *src;
static uint32_t spos, send;
static uint8_t code[CODE_MAX];
static uint32_t clen;
static uint8_t perr; /* the error of a formula that cannot be read */

static void emit(uint8_t b)
{
    if (clen < CODE_MAX)
        code[clen++] = b;
    else if (!perr)
        perr = E_ERR;
}

static void skip_space(void)
{
    while (spos < send && src[spos] == ' ')
        spos++;
}

static int peek(void)
{
    skip_space();
    return spos < send ? src[spos] : 0;
}

static int is_digit(int c) { return c >= '0' && c <= '9'; }
static int is_alpha(int c) { return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z'); }
static int upper(int c) { return c >= 'a' && c <= 'z' ? c - 32 : c; }

/* A number at p[*at..n): digits, optionally with comma groups, a point
 * and decimals (rounded to 4 places). 0 if there is none or it is too
 * large. */
static int read_number(const uint8_t *p, uint32_t n, uint32_t *at, int commas, i64 *out)
{
    uint32_t i = *at, digits = 0, dec = 0;
    u64 v = 0;
    int round = 0;
    while (i < n && (is_digit(p[i]) || (commas && p[i] == ',' && digits && i + 1 < n && is_digit(p[i + 1])))) {
        if (p[i] != ',') {
            if (v > 92233720368547ull)
                return 0;
            v = v * 10 + (p[i] - '0');
            digits++;
        }
        i++;
    }
    if (i < n && p[i] == '.') {
        i++;
        while (i < n && is_digit(p[i])) {
            if (dec < 4) {
                v = v * 10 + (p[i] - '0');
                dec++;
            } else if (dec == 4) {
                round = p[i] >= '5';
                dec++;
            }
            digits++;
            i++;
        }
    }
    if (!digits || v > 922337203685477ull)
        return 0;
    while (dec < 4) {
        v *= 10;
        dec++;
    }
    v += round;
    if (v > (u64)INT64_MAX)
        return 0;
    *at = i;
    *out = (i64)v;
    return 1;
}

static void emit_num(i64 v)
{
    int k;
    uint32_t frac;
    u64 whole = divs(mag(v), SCALE, &frac);
    if (!frac && whole < 32768) {
        emit(OP_SMALL);
        emit((uint8_t)whole);
        emit((uint8_t)(whole >> 8));
        return;
    }
    emit(OP_NUM);
    for (k = 0; k < 8; k++)
        emit((uint8_t)((u64)v >> (8 * k)));
}

/* A reference at the current position (a letter and a row number), or 0. */
static int read_ref(uint8_t *r, uint8_t *c)
{
    uint32_t i = spos, row = 0, nd = 0;
    if (i >= send || !is_alpha(src[i]))
        return 0;
    int col = upper(src[i]) - 'A';
    i++;
    if (i < send && is_alpha(src[i])) {
        /* Two letters and digits: a column past Z. */
        uint32_t j = i;
        while (j < send && is_alpha(src[j]))
            j++;
        if (j < send && is_digit(src[j])) {
            while (j < send && is_digit(src[j]))
                j++;
            spos = j;
            if (!perr)
                perr = E_REF;
            *r = *c = 0;
            return 1;
        }
        return 0;
    }
    while (i < send && is_digit(src[i])) {
        if (row < 1000)
            row = row * 10 + (src[i] - '0');
        nd++;
        i++;
    }
    if (!nd)
        return 0;
    spos = i;
    if (row < 1 || row > NROWS) {
        if (!perr)
            perr = E_REF;
        row = 1;
    }
    *r = (uint8_t)(row - 1);
    *c = (uint8_t)col;
    return 1;
}

static void p_expr(int depth);

static void p_primary(int depth)
{
    int ch = peek();
    uint8_t r0, c0, r1, c1;
    i64 v;
    if (depth > 24) {
        perr = E_ERR;
        return;
    }
    if (is_digit(ch) || ch == '.') {
        if (!read_number(src, send, &spos, 0, &v)) {
            perr = E_ERR;
            return;
        }
        emit_num(v);
        return;
    }
    if (ch == '"') {
        uint32_t start = ++spos;
        while (spos < send && src[spos] != '"')
            spos++;
        if (spos >= send) {
            perr = E_ERR;
            return;
        }
        emit(OP_STR);
        emit((uint8_t)(spos - start));
        while (start < spos)
            emit(src[start++]);
        spos++;
        return;
    }
    if (ch == '(') {
        spos++;
        p_expr(depth + 1);
        if (peek() != ')') {
            perr = E_ERR;
            return;
        }
        spos++;
        return;
    }
    if (ch == '#') {
        /* "#REF" left by a copy that moved a reference off the sheet. */
        perr = perr ? perr : (send - spos >= 4 && src[spos + 1] == 'R' && src[spos + 2] == 'E' && src[spos + 3] == 'F' ? E_REF : E_ERR);
        spos = send;
        return;
    }
    if (read_ref(&r0, &c0)) {
        if (peek() == ':') {
            spos++;
            skip_space();
            if (!read_ref(&r1, &c1)) {
                perr = E_ERR;
                return;
            }
            emit(OP_RANGE);
            emit(r0 < r1 ? r0 : r1);
            emit(c0 < c1 ? c0 : c1);
            emit(r0 < r1 ? r1 : r0);
            emit(c0 < c1 ? c1 : c0);
            return;
        }
        emit(OP_REF);
        emit(r0);
        emit(c0);
        return;
    }
    if (is_alpha(ch)) {
        uint32_t start = spos, f, len, nargs = 0;
        while (spos < send && is_alpha(src[spos]))
            spos++;
        len = spos - start;
        for (f = 0; f < FN_N; f++) {
            const char *name = fn_name[f];
            uint32_t k = 0;
            while (k < len && name[k] && upper(src[start + k]) == name[k])
                k++;
            if (k == len && !name[k])
                break;
        }
        if (f == FN_N || peek() != '(') {
            perr = E_ERR;
            return;
        }
        spos++;
        if (peek() != ')') {
            for (;;) {
                p_expr(depth + 1);
                nargs++;
                if (perr)
                    return;
                if (peek() == ',') {
                    spos++;
                    continue;
                }
                break;
            }
        }
        if (peek() != ')' || nargs < fn_min[f] || nargs > fn_max[f]) {
            perr = E_ERR;
            return;
        }
        spos++;
        emit(OP_FN);
        emit((uint8_t)f);
        emit((uint8_t)nargs);
        return;
    }
    perr = E_ERR;
}

static void p_postfix(int depth)
{
    p_primary(depth);
    while (!perr && peek() == '%') {
        spos++;
        emit(OP_PCT);
    }
}

static void p_unary(int depth);

static void p_pow(int depth)
{
    p_postfix(depth);
    if (!perr && peek() == '^') {
        spos++;
        p_unary(depth + 1);
        emit(OP_POW);
    }
}

static void p_unary(int depth)
{
    int ch = peek();
    if (depth > 24) {
        perr = E_ERR;
        return;
    }
    if (ch == '-' || ch == '+') {
        spos++;
        p_unary(depth + 1);
        if (ch == '-')
            emit(OP_NEG);
        return;
    }
    p_pow(depth);
}

static void p_mul(int depth)
{
    p_unary(depth);
    while (!perr) {
        int ch = peek();
        if (ch != '*' && ch != '/')
            break;
        spos++;
        p_unary(depth);
        emit(ch == '*' ? OP_MUL : OP_DIV);
    }
}

static void p_add(int depth)
{
    p_mul(depth);
    while (!perr) {
        int ch = peek();
        if (ch != '+' && ch != '-')
            break;
        spos++;
        p_mul(depth);
        emit(ch == '+' ? OP_ADD : OP_SUB);
    }
}

static void p_expr(int depth)
{
    p_add(depth);
    while (!perr) {
        int ch = peek(), op;
        if (ch == '=') {
            op = OP_EQ;
            spos++;
        } else if (ch == '<' || ch == '>') {
            spos++;
            if (spos < send && src[spos] == '=') {
                op = ch == '<' ? OP_LE : OP_GE;
                spos++;
            } else if (ch == '<' && spos < send && src[spos] == '>') {
                op = OP_NE;
                spos++;
            } else
                op = ch == '<' ? OP_LT : OP_GT;
        } else
            break;
        p_add(depth);
        emit((uint8_t)op);
    }
}

/* Reads a whole entry: sets the flags, the value of a literal and the code
 * of a formula (in `code`, `clen` bytes). */
static uint8_t compile(const uint8_t *p, uint32_t n, i64 *val)
{
    uint32_t at = 0;
    clen = 0;
    *val = 0;
    if (!n)
        return 0;
    if (p[0] == '=') {
        src = p;
        spos = 1;
        send = n;
        perr = 0;
        p_expr(0);
        if (!perr && peek())
            perr = E_ERR;
        if (perr) {
            clen = 0;
            *val = perr;
            return F_FORMULA | F_BAD;
        }
        return F_FORMULA;
    }
    /* A number: an optional sign, digits (with comma groups), a %. */
    {
        int neg = 0;
        i64 v;
        if (p[0] == '-' || p[0] == '+') {
            neg = p[0] == '-';
            at = 1;
        }
        if (read_number(p, n, &at, 1, &v)) {
            uint8_t flag = F_NUM;
            if (at < n && p[at] == '%') {
                at++;
                flag |= F_PCT;
                v = (i64)divs((u64)v, 100, 0); /* exact enough: 4 places kept */
            }
            if (at == n) {
                *val = neg ? -v : v;
                return flag;
            }
        }
    }
    return F_TEXT;
}

/* --- The pool ------------------------------------------------------------- */

static uint8_t dirty_all; /* a recalculation is wanted */

/* Moves the live blocks down over the dead ones. The string values of
 * formulas point into the pool, so everything is recalculated after. */
static void compact(void)
{
    uint32_t at = 0, to = 0;
    while (at < pool_used) {
        uint16_t owner = pool[at] | pool[at + 1] << 8;
        uint32_t size;
        if (owner == 0xFFFF) {
            /* A dead block keeps its size after the owner. */
            size = pool[at + 2] | pool[at + 3] << 8;
            at += size;
            continue;
        }
        size = 2 + c_rlen[owner] + c_clen[owner];
        if (to != at)
            memmove(pool + to, pool + at, size);
        c_off[owner] = (uint16_t)(to + 2);
        to += size;
        at += size;
    }
    pool_used = to;
    dirty_all = 1;
}

static void free_block(uint16_t i)
{
    if (!c_rlen[i])
        return;
    uint32_t at = c_off[i] - 2, size = 2 + c_rlen[i] + c_clen[i];
    pool[at] = pool[at + 1] = 0xFF;
    /* Blocks are at least 3 bytes; the size goes after the owner when it
     * fits (a 3-byte block stays dead with size 3 written in the owner's
     * stead: it is read as size below). */
    if (size >= 4) {
        pool[at + 2] = (uint8_t)size;
        pool[at + 3] = (uint8_t)(size >> 8);
    }
}

/* Sets cell i to the n bytes at p (n 0: empty): 1, or 0 if the pool is
 * full. */
static int set_cell(uint16_t i, const uint8_t *p, uint32_t n)
{
    i64 v;
    uint8_t flag;
    uint32_t size;
    if (n > RAW_MAX)
        n = RAW_MAX;
    flag = compile(p, n, &v);
    size = 2 + n + clen;
    if (size < 4 && n)
        size = 4; /* room for a dead block's size */
    if (n && pool_used + size > POOL) {
        free_block(i);
        c_rlen[i] = c_flag[i] = c_kind[i] = c_dlen[i] = 0;
        c_state[i] = S_CLEAN;
        compact();
        if (pool_used + size > POOL)
            return 0;
    } else
        free_block(i);
    c_flag[i] = flag;
    c_rlen[i] = (uint8_t)n;
    c_clen[i] = (uint8_t)clen;
    c_kind[i] = K_EMPTY;
    c_val[i] = v;
    c_state[i] = S_CLEAN;
    if (n) {
        pool[pool_used] = (uint8_t)i;
        pool[pool_used + 1] = (uint8_t)(i >> 8);
        c_off[i] = (uint16_t)(pool_used + 2);
        memmove(pool + pool_used + 2, p, n);
        memcpy(pool + pool_used + 2 + n, code, clen);
        pool_used += size;
        /* A padded block: its last byte is spare; the size counts it. */
        if (2 + n + clen < size)
            c_clen[i] = (uint8_t)(size - 2 - n);
    }
    if (flag & F_BAD) {
        c_kind[i] = K_ERR;
        c_clen[i] = 0;
        if (2 + n < size)
            c_clen[i] = (uint8_t)(size - 2 - n);
    }
    dirty_all = 1;
    update_disp(i);
    return 1;
}

/* --- Formulas: computing -------------------------------------------------- */

static const uint8_t op_size[] = { 1, 9, 0, 3, 5, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 3 };

static uint32_t op_len(const uint8_t *p) { return *p == OP_STR ? 2 + p[1] : op_size[*p]; }

/* The code of formula cell i (its length in *n); a bad formula has none. */
static const uint8_t *code_of(uint16_t i, uint32_t *n)
{
    *n = c_flag[i] & F_BAD ? 0 : c_clen[i];
    return pool + c_off[i] + c_rlen[i];
}

typedef struct {
    uint8_t k;
    i64 v;
} value;

static value cell_value(uint16_t i)
{
    value x = { K_EMPTY, 0 };
    if (c_flag[i] & F_FORMULA) {
        x.k = c_kind[i];
        x.v = c_val[i];
    } else if (c_flag[i] & F_NUM) {
        x.k = K_NUM;
        x.v = c_val[i];
    } else if (c_flag[i] & F_TEXT) {
        uint32_t n;
        const uint8_t *p = text_of(i, &n);
        x.k = K_STR;
        x.v = STR(p - pool, n);
    }
    return x;
}

static int32_t work;

static value err(uint8_t e)
{
    value x = { K_ERR, e };
    return x;
}

static value num(i64 v)
{
    value x = { K_NUM, v };
    return x;
}

/* A number from a value: 0 with *e set for an error or text. */
static i64 as_num(value x, uint8_t *e)
{
    if (x.k == K_NUM)
        return x.v;
    if (x.k == K_EMPTY)
        return 0;
    if (!*e)
        *e = x.k == K_ERR ? (uint8_t)x.v : E_VALUE;
    return 0;
}

static int cmp_text(value a, value b)
{
    const uint8_t *p = pool + STR_OFF(a.v), *q = pool + STR_OFF(b.v);
    uint32_t n = STR_LEN(a.v), m = STR_LEN(b.v), i;
    for (i = 0; i < n && i < m; i++) {
        int x = upper(p[i]), y = upper(q[i]);
        if (x != y)
            return x < y ? -1 : 1;
    }
    return n == m ? 0 : n < m ? -1 : 1;
}

static value compare(uint8_t op, value a, value b)
{
    int c;
    if (a.k == K_ERR)
        return a;
    if (b.k == K_ERR)
        return b;
    if (a.k == K_RANGE || b.k == K_RANGE)
        return err(E_VALUE);
    /* Empty is 0 against a number and "" against text. */
    if (a.k == K_EMPTY)
        a = b.k == K_STR ? (value){ K_STR, STR(0, 0) } : num(0);
    if (b.k == K_EMPTY)
        b = a.k == K_STR ? (value){ K_STR, STR(0, 0) } : num(0);
    if (a.k == K_STR && b.k == K_STR)
        c = cmp_text(a, b);
    else if (a.k == K_STR)
        c = 1; /* text sorts after numbers */
    else if (b.k == K_STR)
        c = -1;
    else
        c = a.v < b.v ? -1 : a.v > b.v;
    switch (op) {
    case OP_EQ: c = c == 0; break;
    case OP_NE: c = c != 0; break;
    case OP_LT: c = c < 0; break;
    case OP_GT: c = c > 0; break;
    case OP_LE: c = c <= 0; break;
    default: c = c >= 0; break;
    }
    return num(c ? SCALE : 0);
}

static value power(i64 x, i64 y)
{
    uint32_t frac;
    u64 m = divs(mag(y), SCALE, &frac);
    i64 r = SCALE;
    if (frac || m > 64)
        return err(E_NUM);
    while (m--) {
        work++;
        if (!mul_fix(r, x, &r))
            return err(E_NUM);
    }
    if (y < 0) {
        if (!r)
            return err(E_DIV);
        if (!div_fix(SCALE, r, &r))
            return err(E_NUM);
    }
    return num(r);
}

static value round_to(i64 x, i64 places)
{
    uint32_t frac;
    u64 p = divs(mag(places), SCALE, &frac), m = mag(x), q;
    int k;
    if (places < 0 && p > 0) {
        /* ROUND(x, -n): to tens, hundreds... */
        k = (int)(4 + p);
    } else {
        if (p >= 4)
            return num(x);
        k = (int)(4 - p);
    }
    if (k > 18)
        return num(0);
    u64 f = 1;
    q = m;
    for (int i = 0; i < k; i++) {
        q = divs(q, 10, 0);
        f *= 10;
    }
    if (2 * (m - q * f) >= f)
        q++;
    i64 r;
    (void)frac;
    if (!fit(q * f, x < 0, &r))
        return err(E_NUM);
    return num(r);
}

/* SUM, AVERAGE, MIN, MAX, COUNT over the n values at a. */
static value aggregate(uint8_t f, value *a, uint32_t n)
{
    i64 sum = 0, best = 0, count = 0;
    uint32_t i;
    for (i = 0; i < n; i++) {
        value x = a[i];
        uint32_t r, c, r0 = 0, c0 = 0, r1 = 0, c1 = 0;
        if (x.k == K_RANGE) {
            r0 = x.v & 0xFF;
            c0 = x.v >> 8 & 0xFF;
            r1 = x.v >> 16 & 0xFF;
            c1 = x.v >> 24 & 0xFF;
        }
        for (r = r0; r <= r1; r++)
            for (c = c0; c <= c1; c++) {
                value y = x;
                work++;
                if (x.k == K_RANGE) {
                    y = cell_value(idx(r, c));
                    if (y.k == K_ERR && f != FN_COUNT)
                        return y;
                    if (y.k != K_NUM)
                        continue;
                } else {
                    if (y.k == K_EMPTY)
                        continue;
                    if (y.k == K_ERR) {
                        if (f == FN_COUNT)
                            continue;
                        return y;
                    }
                    if (y.k == K_STR) {
                        if (f == FN_COUNT)
                            continue;
                        return err(E_VALUE);
                    }
                }
                if (!count || (f == FN_MIN && y.v < best) || (f == FN_MAX && y.v > best))
                    best = y.v;
                count += SCALE;
                if (!add_fix(sum, y.v, &sum))
                    return err(E_NUM);
            }
    }
    switch (f) {
    case FN_SUM: return num(sum);
    case FN_COUNT: return num(count);
    case FN_MIN:
    case FN_MAX: return num(count ? best : 0);
    default:
        if (!count)
            return err(E_DIV);
        work += 40;
        return div_fix(sum, count, &sum) ? num(sum) : err(E_NUM);
    }
}

/* Computes formula cell i (every cell it refers to is computed). */
static value evaluate(uint16_t i)
{
    value st[STACK_MAX];
    uint32_t sp = 0, pc = 0, n;
    const uint8_t *p = code_of(i, &n);
    if (c_flag[i] & F_BAD)
        return err((uint8_t)c_val[i]);
    while (pc < n) {
        uint8_t op = p[pc], e = 0;
        value a, b;
        work++;
        if (sp + 1 >= STACK_MAX)
            return err(E_ERR);
        switch (op) {
        case OP_NUM: {
            u64 v = 0;
            for (int k = 7; k >= 0; k--)
                v = v << 8 | p[pc + 1 + k];
            st[sp++] = num((i64)v);
            break;
        }
        case OP_SMALL: st[sp++] = num((i64)(p[pc + 1] | p[pc + 2] << 8) * SCALE); break;
        case OP_STR: st[sp++] = (value){ K_STR, STR(p + pc + 2 - pool, p[pc + 1]) }; break;
        case OP_REF: st[sp++] = cell_value(idx(p[pc + 1], p[pc + 2])); break;
        case OP_RANGE:
            st[sp++] = (value){ K_RANGE, p[pc + 1] | p[pc + 2] << 8 | p[pc + 3] << 16 | (i64)p[pc + 4] << 24 };
            break;
        case OP_NEG:
        case OP_PCT:
            a = st[sp - 1];
            if (a.k == K_RANGE)
                e = E_VALUE;
            a.v = as_num(a, &e);
            if (op == OP_NEG)
                a.v = -a.v;
            else
                a.v = (i64)divs(mag(a.v), 100, 0) * (a.v < 0 ? -1 : 1);
            st[sp - 1] = e ? err(e) : num(a.v);
            break;
        case OP_FN: {
            uint8_t f = p[pc + 1], k = p[pc + 2];
            value *args = st + sp - k, r;
            if (f <= FN_COUNT)
                r = aggregate(f, args, k);
            else if (f == FN_IF) {
                a = args[0];
                if (a.k == K_ERR)
                    r = a;
                else if (a.k == K_STR || a.k == K_RANGE)
                    r = err(E_VALUE);
                else
                    r = a.v ? args[1] : k == 3 ? args[2] : num(0);
                if (r.k == K_RANGE)
                    r = err(E_VALUE);
            } else {
                for (uint32_t j = 0; j < k; j++)
                    if (args[j].k == K_RANGE)
                        e = E_VALUE;
                i64 x = as_num(args[0], &e), y = k == 2 ? as_num(args[1], &e) : 0;
                if (e)
                    r = err(e);
                else if (f == FN_ABS)
                    r = num(x < 0 ? -x : x);
                else
                    r = round_to(x, y);
            }
            sp -= k;
            st[sp++] = r;
            break;
        }
        default: {
            b = st[--sp];
            a = st[sp - 1];
            if (op >= OP_EQ) {
                st[sp - 1] = compare(op, a, b);
                break;
            }
            if (a.k == K_RANGE || b.k == K_RANGE)
                e = E_VALUE;
            i64 x = as_num(a, &e), y = as_num(b, &e), r = 0;
            if (!e)
                switch (op) {
                case OP_ADD:
                    if (!add_fix(x, y, &r))
                        e = E_NUM;
                    break;
                case OP_SUB:
                    if (y == INT64_MIN || !add_fix(x, -y, &r))
                        e = E_NUM;
                    break;
                case OP_MUL:
                    work += 4;
                    if (!mul_fix(x, y, &r))
                        e = E_NUM;
                    break;
                case OP_DIV:
                    work += 40;
                    if (!y)
                        e = E_DIV;
                    else if (!div_fix(x, y, &r))
                        e = E_NUM;
                    break;
                default: {
                    value w = power(x, y);
                    if (w.k == K_ERR)
                        e = (uint8_t)w.v;
                    r = w.v;
                }
                }
            st[sp - 1] = e ? err(e) : num(r);
        }
        }
        pc += op_len(p + pc);
    }
    if (sp != 1)
        return err(E_ERR);
    if (st[0].k == K_RANGE)
        return err(E_VALUE);
    return st[0];
}

/* --- Recalculation -------------------------------------------------------- */

static uint16_t rs_cell[NCELLS];
static uint8_t rs_pc[NCELLS];
static uint16_t rs_ri[NCELLS];
static uint32_t rsp, rscan;
static uint8_t recalcing;
static uint8_t doc_dirty, vec_dirty, chart_dirty;

static void start_recalc(void)
{
    uint32_t i;
    for (i = 0; i < NCELLS; i++)
        if (c_flag[i] & F_FORMULA)
            c_state[i] = S_DIRTY;
    rsp = rscan = 0;
    recalcing = 1;
    dirty_all = 0;
}

static void set_result(uint16_t i, value v)
{
    work += 12; /* formatting the value */
    c_kind[i] = v.k;
    c_val[i] = v.v;
    c_state[i] = S_CLEAN;
    update_disp(i);
}

/* Cell d is needed: 0 if it is computed, 1 if it was pushed, 2 if it closed
 * a cycle (the cells of the cycle are #CYCLE and off the stack). */
static int need(uint16_t d)
{
    if (!(c_flag[d] & F_FORMULA) || c_state[d] == S_CLEAN)
        return 0;
    if (c_state[d] == S_DIRTY) {
        c_state[d] = S_BUSY;
        rs_cell[rsp] = d;
        rs_pc[rsp] = 0;
        rs_ri[rsp] = 0;
        rsp++;
        return 1;
    }
    uint32_t j = rsp;
    while (j && rs_cell[j - 1] != d)
        j--;
    j--;
    for (uint32_t k = j; k < rsp; k++)
        set_result(rs_cell[k], err(E_CYCLE));
    rsp = j;
    return 2;
}

static void recalc_step(void)
{
    work = 0;
    while (work < WORK_PER_FRAME) {
        if (!rsp) {
            while (rscan < NCELLS && c_state[rscan] != S_DIRTY) {
                rscan++;
                if (!(rscan & 15))
                    work++;
            }
            if (rscan == NCELLS) {
                recalcing = 0;
                doc_dirty = chart_dirty = 1;
                return;
            }
            need((uint16_t)rscan);
            continue;
        }
        uint32_t t = rsp - 1, n;
        uint16_t cell = rs_cell[t];
        const uint8_t *p = code_of(cell, &n);
        int pushed = 0;
        while (rs_pc[t] < n) {
            const uint8_t *o = p + rs_pc[t];
            work++;
            if (*o == OP_REF) {
                int r = need(idx(o[1], o[2]));
                if (r) {
                    pushed = 1;
                    break;
                }
            } else if (*o == OP_RANGE) {
                uint32_t w = o[4] - o[2] + 1, count = (o[3] - o[1] + 1) * w;
                while (rs_ri[t] < count) {
                    uint32_t k = rs_ri[t];
                    work++;
                    if (need(idx(o[1] + k / w, o[2] + k % w))) {
                        pushed = 1;
                        break;
                    }
                    rs_ri[t]++;
                }
                if (pushed)
                    break;
                rs_ri[t] = 0;
            }
            rs_pc[t] += (uint8_t)op_len(o);
        }
        if (pushed)
            continue;
        rsp--;
        set_result(cell, evaluate(cell));
    }
}

/* --- Screen state --------------------------------------------------------- */

static uint32_t top, left;         /* the first row and column shown */
static uint32_t sr, sc, ar, ac;    /* the selected cell and the range's other corner */
static uint8_t editing;
static uint8_t ebuf[RAW_MAX];
static uint32_t elen, ecaret;
static uint8_t has_vec, keeps;
static uint8_t chart_kind;         /* 0 none, 1 bar, 2 line */
static uint8_t ch_r0, ch_c0, ch_r1, ch_c1;
static uint8_t new_armed;
static const char *status = "";
static char status_buf[80];

static void say(const char *s)
{
    status = s;
    doc_dirty = 1;
}

static uint32_t vrows(void) { return chart_kind ? VROWS_CHART : VROWS_ALL; }
static uint32_t rmin(void) { return sr < ar ? sr : ar; }
static uint32_t rmax(void) { return sr < ar ? ar : sr; }
static uint32_t cmin(void) { return sc < ac ? sc : ac; }
static uint32_t cmax(void) { return sc < ac ? ac : sc; }

/* The geometry of the shown table, from locate (screen units): g_y[0] the
 * header's top, g_y[1 + i] the top of shown row i, g_y[1 + rows] the
 * table's bottom. */
static uint32_t g_y[VROWS_ALL + 2];
static uint16_t loc_ids[VROWS_ALL + 2];
static uint32_t loc_n, loc_at;

static void default_geometry(void)
{
    /* The reference layout's: the entry line (4 + 18), 4 more above the
     * table, rows of 21. */
    for (uint32_t i = 0; i < VROWS_ALL + 2; i++)
        g_y[i] = 26 + 21 * i;
}

static uint32_t name_of(uint32_t r, uint32_t c, uint8_t *out)
{
    uint32_t n = 0;
    out[n++] = (uint8_t)('A' + c);
    if (r + 1 >= 10)
        out[n++] = (uint8_t)('0' + (r + 1) / 10);
    out[n++] = (uint8_t)('0' + (r + 1) % 10);
    return n;
}

/* --- The page document ---------------------------------------------------- */

static uint8_t pdoc[16384];
static uint8_t col_end[NCOLS]; /* the column is right-aligned */

static void text(const char *s)
{
    uint32_t n = 0;
    while (s[n])
        n++;
    doc_text(s, n);
}

static void columns_align(void)
{
    for (uint32_t c = left; c < left + VCOLS; c++) {
        int n = 0;
        for (uint32_t i = c; i < NCELLS; i += NCOLS) {
            uint8_t f = c_flag[i];
            if (f & F_TEXT || (f & F_FORMULA && c_kind[i] == K_STR))
                n--;
            else if (f)
                n++;
        }
        col_end[c] = n >= 0;
    }
}

/* The chart's series: values and labels (cells, or FFFFh for none). */
static uint16_t ch_val[BARS_MAX], ch_lab[BARS_MAX];
static uint32_t ch_n;

static int is_text(uint16_t i) { return c_rlen[i] && cell_value(i).k == K_STR; }

static void chart_series(void)
{
    uint32_t r0 = ch_r0, c0 = ch_c0, r1 = ch_r1, c1 = ch_c1, k, j;
    int across = r0 == r1 && c0 != c1; /* one row: values along it */
    ch_n = 0;
    if (!across) {
        /* The first column with a number. */
        for (k = c0; k <= c1; k++) {
            for (j = r0; j <= r1; j++)
                if (cell_value(idx(j, k)).k == K_NUM)
                    break;
            if (j <= r1)
                break;
        }
        if (k > c1)
            k = c0;
        int lab = -1;
        for (int c = (int)k - 1; c >= 0 && lab < 0; c--)
            for (j = r0; j <= r1; j++)
                if (is_text(idx(j, c))) {
                    lab = c;
                    break;
                }
        for (j = r0; j <= r1 && ch_n < BARS_MAX; j++, ch_n++) {
            ch_val[ch_n] = idx(j, k);
            ch_lab[ch_n] = lab >= 0 ? idx(j, lab) : 0xFFFF;
        }
    } else {
        int lab = -1;
        for (int r = (int)r0 - 1; r >= 0 && lab < 0; r--)
            for (k = c0; k <= c1; k++)
                if (is_text(idx(r, k))) {
                    lab = r;
                    break;
                }
        for (k = c0; k <= c1 && ch_n < BARS_MAX; k++, ch_n++) {
            ch_val[ch_n] = idx(r0, k);
            ch_lab[ch_n] = lab >= 0 ? idx(lab, k) : 0xFFFF;
        }
    }
}

static const uint8_t BAR[3] = { 66, 120, 210 };

/* The chart figure: bars (or a line) from 0, in 80-unit slots when there
 * are at most 8 values (under the label table's columns). */
static void chart_figure(void)
{
    uint8_t *base = doc_ptr + DOC_FIGURE_HEAD;
    i64 vmax = 0, vmin = 0, v[BARS_MAX];
    uint32_t n = ch_n, k, slot = n <= 8 ? COLW * 16 : 640 * 16 / (n ? n : 1);
    int32_t top_y = 8 * 16, plot = (CHART_H - 16) * 16;
    if (doc_limit - base < 64)
        return;
    path_begin(base, (uint32_t)(doc_limit - base));
    for (k = 0; k < n; k++) {
        value x = cell_value(ch_val[k]);
        v[k] = x.k == K_NUM ? x.v : 0;
        if (v[k] > vmax)
            vmax = v[k];
        if (v[k] < vmin)
            vmin = v[k];
    }
    /* Scale to 32 bits: span below 2^20, so value × plot fits. */
    u64 span = (u64)vmax - (u64)vmin;
    uint32_t shift = 0;
    while ((span >> shift) >= (1u << 20))
        shift++;
    int32_t s = (int32_t)(span >> shift);
    if (!s)
        s = 1;
    int32_t zero = top_y + (int32_t)(vmax >> shift) * plot / s;
    /* Light horizontal lines at the top, middle and bottom of the plot. */
    path_stroke(225, 225, 225, 255, 16);
    for (k = 0; k < 3; k++) {
        path_move(0, (int16_t)(top_y + plot * (int32_t)k / 2));
        path_line(640 * 16, (int16_t)(top_y + plot * (int32_t)k / 2));
    }
    if (chart_kind == 1) {
        path_stroke(0, 0, 0, 0, 0);
        path_fill(BAR[0], BAR[1], BAR[2], 255);
        for (k = 0; k < n; k++) {
            int32_t x0 = (int32_t)(k * slot + slot / 4), x1 = (int32_t)(k * slot + slot - slot / 4);
            int32_t y = top_y + (int32_t)(((u64)vmax - (u64)v[k]) >> shift) * plot / s;
            if (y == zero)
                y = zero - 8; /* a zero shows as a sliver */
            path_move((int16_t)x0, (int16_t)zero);
            path_line((int16_t)x1, (int16_t)zero);
            path_line((int16_t)x1, (int16_t)y);
            path_line((int16_t)x0, (int16_t)y);
            path_close();
        }
    } else {
        path_fill(0, 0, 0, 0);
        path_stroke(BAR[0], BAR[1], BAR[2], 255, 40);
        for (k = 0; k < n; k++) {
            int32_t x = (int32_t)(k * slot + slot / 2), y = top_y + (int32_t)(((u64)vmax - (u64)v[k]) >> shift) * plot / s;
            if (k)
                path_line((int16_t)x, (int16_t)y);
            else
                path_move((int16_t)x, (int16_t)y);
        }
        path_fill(BAR[0], BAR[1], BAR[2], 255);
        path_stroke(255, 255, 255, 255, 16);
        for (k = 0; k < n; k++) {
            int32_t x = (int32_t)(k * slot + slot / 2), y = top_y + (int32_t)(((u64)vmax - (u64)v[k]) >> shift) * plot / s;
            path_move((int16_t)(x - 48), (int16_t)(y - 48));
            path_line((int16_t)(x + 48), (int16_t)(y - 48));
            path_line((int16_t)(x + 48), (int16_t)(y + 48));
            path_line((int16_t)(x - 48), (int16_t)(y + 48));
            path_close();
        }
    }
    /* The zero line. */
    path_fill(0, 0, 0, 0);
    path_stroke(120, 120, 120, 255, 16);
    path_move(0, (int16_t)zero);
    path_line(640 * 16, (int16_t)zero);
    if (!path_overflow)
        doc_figure(640, CHART_H, DOC_START, base, (uint16_t)path_length(base));
}

/* The hidden row that makes every column wider than 640 / 8. */
static void padding_row(uint32_t cols, uint16_t anchor)
{
    doc_size(6);
    doc_colour(255, 255, 255);
    for (uint32_t k = 0; k < cols; k++) {
        doc_cell();
        if (!k)
            doc_anchor(anchor);
        text("________________________________");
        if (!k)
            doc_anchor(0);
    }
}

static void chart_doc(void)
{
    static const uint8_t mid[8] = { 1, 1, 1, 1, 1, 1, 1, 1 };
    uint32_t k;
    chart_series();
    doc_paragraph(0, 0, 6, 0);
    chart_figure();
    if (ch_n > 8 || !ch_n)
        return;
    doc_size(11);
    doc_colour(255, 255, 255); /* no rule under the labels */
    doc_table(8, 0, mid);
    for (k = 0; k < 8; k++) {
        doc_cell();
        if (k < ch_n && ch_lab[k] != 0xFFFF && c_rlen[ch_lab[k]]) {
            doc_colour(70, 70, 70);
            doc_text(c_disp[ch_lab[k]], c_dlen[ch_lab[k]]);
        }
    }
    for (k = 0; k < 8; k++) {
        doc_cell();
        if (k < ch_n) {
            uint16_t i = ch_val[k];
            doc_colour(BAR[0], BAR[1], BAR[2]);
            doc_text(c_disp[i], c_dlen[i]);
        }
    }
    padding_row(8, 0);
    doc_table_end();
}

static void range_name(char *out, uint32_t *n, uint32_t r0, uint32_t c0, uint32_t r1, uint32_t c1)
{
    *n = name_of(r0, c0, (uint8_t *)out);
    if (r0 != r1 || c0 != c1) {
        out[(*n)++] = ':';
        *n += name_of(r1, c1, (uint8_t *)out + *n);
    }
}

static void build_doc(void)
{
    static const uint8_t ACC[3] = { 26, 100, 210 };
    uint8_t align[8];
    char nm[8];
    uint32_t n, i, k, rows = vrows();
    uint16_t sel = idx(sr, sc);
    columns_align();
    doc_begin(pdoc, sizeof pdoc - 1);
    /* The entry line. */
    doc_colour(25, 25, 25);
    doc_size(14);
    doc_paragraph(DOC_START, 6, 4, DOC_NOWRAP | DOC_ELLIPSIS);
    doc_font(DOC_SANS, DOC_BOLD);
    range_name(nm, &n, rmin(), cmin(), rmax(), cmax());
    if (editing)
        n = name_of(sr, sc, (uint8_t *)nm);
    doc_text(nm, n);
    doc_font(DOC_SANS, 0);
    text("   ");
    if (editing) {
        doc_text(ebuf, ecaret);
        doc_colour(ACC[0], ACC[1], ACC[2]);
        doc_font(DOC_SANS, DOC_BOLD);
        text("|");
        doc_font(DOC_SANS, 0);
        doc_colour(25, 25, 25);
        doc_text(ebuf + ecaret, elen - ecaret);
    } else if (c_rlen[sel]) {
        if (c_flag[sel] & F_FORMULA && c_kind[sel] == K_NUM) {
            /* The formula, then its value. */
            doc_text(pool + c_off[sel], c_rlen[sel]);
            doc_colour(120, 120, 120);
            text("   ");
            uint8_t b[40];
            doc_text(b, format_num(c_val[sel], 0, 0, b));
        } else
            doc_text(pool + c_off[sel], c_rlen[sel]);
    }
    /* The table: row numbers and VCOLS columns. */
    doc_paragraph(DOC_START, 0, 4, 0);
    doc_size(12);
    align[0] = DOC_END;
    for (k = 0; k < VCOLS; k++)
        align[1 + k] = col_end[left + k] ? DOC_END : DOC_START;
    doc_colour(150, 150, 150);
    doc_table(8, DOC_TABLE_HEADER, align);
    doc_cell();
    for (k = 0; k < VCOLS; k++) {
        uint32_t c = left + k;
        doc_cell();
        if (c >= cmin() && c <= cmax())
            doc_colour(ACC[0], ACC[1], ACC[2]);
        else
            doc_colour(90, 90, 90);
        if (!k)
            doc_anchor(A_HEAD);
        char l = (char)('A' + c);
        doc_text(&l, 1);
        if (!k)
            doc_anchor(0);
    }
    for (i = 0; i < rows; i++) {
        uint32_t r = top + i;
        uint8_t b[4];
        doc_cell();
        if (r >= rmin() && r <= rmax()) {
            doc_colour(ACC[0], ACC[1], ACC[2]);
            doc_font(DOC_SANS, DOC_BOLD);
        } else
            doc_colour(130, 130, 130);
        doc_anchor((uint16_t)(A_ROW + i));
        n = name_of(r, 0, b) - 1;
        doc_text(b + 1, n);
        doc_anchor(0);
        doc_font(DOC_SANS, 0);
        for (k = 0; k < VCOLS; k++) {
            uint16_t c = idx(r, left + k);
            doc_cell();
            int here = c == sel && !has_vec;
            if (here) {
                doc_colour(ACC[0], ACC[1], ACC[2]);
                doc_font(DOC_SANS, DOC_BOLD);
            }
            if (c_dlen[c]) {
                if (!here)
                    doc_colour(c_flag[c] & F_FORMULA && c_kind[c] == K_ERR ? 200 : 25, c_flag[c] & F_FORMULA && c_kind[c] == K_ERR ? 40 : 25,
                               c_flag[c] & F_FORMULA && c_kind[c] == K_ERR ? 40 : 25);
                doc_text(c_disp[c], c_dlen[c]);
            } else if (here)
                text("\xC2\xB7");
            if (here)
                doc_font(DOC_SANS, 0);
        }
    }
    padding_row(8, A_BOTTOM);
    doc_table_end();
    if (chart_kind)
        chart_doc();
    /* The status line. */
    doc_paragraph(DOC_START, 6, 6, DOC_NOWRAP | DOC_ELLIPSIS);
    doc_size(11);
    doc_font(DOC_SANS, 0);
    if (recalcing) {
        doc_colour(180, 100, 0);
        text("Calculating\xE2\x80\xA6  ");
    }
    if (*status) {
        doc_colour(26, 100, 210);
        text(status);
        text("   ");
    }
    doc_colour(130, 130, 130);
    if (chart_kind) {
        range_name(nm, &n, ch_r0, ch_c0, ch_r1, ch_c1);
        text(chart_kind == 1 ? "Bar chart of " : "Line chart of ");
        doc_text(nm, n);
        text(ch_n > 8 ? " (labels for at most 8)   " : "   ");
    }
    text("F2 edit  Ctrl+C/Y copy/paste  Ctrl+D/R fill  Ctrl+G/L chart  Ctrl+S save  Ctrl+P publish  Ctrl+N new");
    doc_end();
    page_submit(pdoc, doc_length(pdoc));
    /* Where the rows are, in this layout. */
    loc_n = 0;
    loc_ids[loc_n++] = A_HEAD;
    for (i = 0; i < rows; i++)
        loc_ids[loc_n++] = (uint16_t)(A_ROW + i);
    loc_ids[loc_n++] = A_BOTTOM;
    loc_at = 0;
    vec_dirty = 1;
}

static void located(const page_place *p)
{
    uint32_t y = p->y, rows = vrows();
    if (!p->height)
        return;
    if (p->id == A_HEAD)
        g_y[0] = y - 3;
    else if (p->id == A_BOTTOM)
        g_y[rows + 1] = y - 3;
    else if (p->id >= A_ROW && p->id < A_ROW + rows)
        g_y[1 + p->id - A_ROW] = y - 3;
    vec_dirty = 1;
}

/* --- The vector overlay --------------------------------------------------- */

static uint8_t vlist[2048];

static void vrect(int32_t x0, int32_t y0, int32_t x1, int32_t y1)
{
    vec_move((int16_t)(x0 * 16), (int16_t)(y0 * 16));
    vec_rect((int16_t)(x1 * 16), (int16_t)(y1 * 16));
}

static void build_vec(void)
{
    uint32_t rows = vrows(), k;
    int32_t y0 = (int32_t)g_y[0], y1 = (int32_t)g_y[1], bottom = (int32_t)g_y[rows + 1];
    vec_begin(vlist, sizeof vlist);
    /* The headers' shade. */
    vec_colour(0, 0, 0, 12);
    vrect(0, y0, 640, y1);
    vrect(0, y1, COLW, bottom);
    /* The column lines. */
    vec_colour(208, 208, 208, 255);
    vec_width(16);
    for (k = 1; k <= VCOLS; k++) {
        vec_move((int16_t)(k * COLW * 16), (int16_t)(y0 * 16));
        vec_line((int16_t)(k * COLW * 16), (int16_t)(bottom * 16));
    }
    vec_move(0, (int16_t)(bottom * 16));
    vec_line(640 * 16, (int16_t)(bottom * 16));
    /* The range, then the selected cell's box, where shown. */
    int32_t r0 = (int32_t)rmin() - (int32_t)top, r1 = (int32_t)rmax() - (int32_t)top;
    int32_t c0 = (int32_t)cmin() - (int32_t)left, c1 = (int32_t)cmax() - (int32_t)left;
    if (r0 < 0)
        r0 = 0;
    if (r1 >= (int32_t)rows)
        r1 = (int32_t)rows - 1;
    if (c0 < 0)
        c0 = 0;
    if (c1 >= VCOLS)
        c1 = VCOLS - 1;
    if ((rmin() != rmax() || cmin() != cmax()) && r0 <= r1 && c0 <= c1) {
        vec_colour(26, 100, 210, 36);
        vrect((c0 + 1) * COLW, (int32_t)g_y[1 + r0], (c1 + 2) * COLW, (int32_t)g_y[2 + r1]);
    }
    int32_t r = (int32_t)sr - (int32_t)top, c = (int32_t)sc - (int32_t)left;
    if (r >= 0 && r < (int32_t)rows && c >= 0 && c < VCOLS) {
        int32_t x0 = (c + 1) * COLW * 16, x1 = (c + 2) * COLW * 16;
        int32_t ya = (int32_t)g_y[1 + r] * 16, yb = (int32_t)g_y[2 + r] * 16;
        vec_colour(26, 100, 210, 255);
        vec_width(32);
        vec_move((int16_t)x0, (int16_t)ya);
        vec_line((int16_t)x1, (int16_t)ya);
        vec_line((int16_t)x1, (int16_t)yb);
        vec_line((int16_t)x0, (int16_t)yb);
        vec_line((int16_t)x0, (int16_t)ya);
    }
    vec_end();
    vec_submit(vlist, vec_length(vlist));
}

/* --- Moving and editing --------------------------------------------------- */

static void show_selection(void)
{
    uint32_t rows = vrows();
    if (sr < top)
        top = sr;
    if (sr >= top + rows)
        top = sr - rows + 1;
    if (sc < left)
        left = sc;
    if (sc >= left + VCOLS)
        left = sc - VCOLS + 1;
    doc_dirty = 1;
}

static void move_to(int32_t r, int32_t c, int extend)
{
    if (r < 0)
        r = 0;
    if (r >= NROWS)
        r = NROWS - 1;
    if (c < 0)
        c = 0;
    if (c >= NCOLS)
        c = NCOLS - 1;
    sr = (uint32_t)r;
    sc = (uint32_t)c;
    if (!extend) {
        ar = sr;
        ac = sc;
    }
    new_armed = 0;
    show_selection();
}

static void start_edit(int keep)
{
    uint16_t i = idx(sr, sc);
    editing = 1;
    elen = keep ? c_rlen[i] : 0;
    memcpy(ebuf, pool + c_off[i], elen);
    ecaret = elen;
    ar = sr;
    ac = sc;
    doc_dirty = 1;
}

static void commit(void)
{
    editing = 0;
    if (!set_cell(idx(sr, sc), ebuf, elen))
        say("The sheet is full: no room for that entry");
    doc_dirty = 1;
}

static void insert_cp(uint32_t cp)
{
    uint8_t b[4];
    uint32_t n;
    if (cp < 0x20 || (cp >= 0x7F && cp < 0xA0))
        return;
    if (cp < 0x80) {
        b[0] = (uint8_t)cp;
        n = 1;
    } else if (cp < 0x800) {
        b[0] = (uint8_t)(0xC0 | cp >> 6);
        b[1] = (uint8_t)(0x80 | (cp & 0x3F));
        n = 2;
    } else if (cp < 0x10000) {
        b[0] = (uint8_t)(0xE0 | cp >> 12);
        b[1] = (uint8_t)(0x80 | (cp >> 6 & 0x3F));
        b[2] = (uint8_t)(0x80 | (cp & 0x3F));
        n = 3;
    } else {
        b[0] = (uint8_t)(0xF0 | cp >> 18);
        b[1] = (uint8_t)(0x80 | (cp >> 12 & 0x3F));
        b[2] = (uint8_t)(0x80 | (cp >> 6 & 0x3F));
        b[3] = (uint8_t)(0x80 | (cp & 0x3F));
        n = 4;
    }
    if (elen + n > RAW_MAX)
        return;
    memmove(ebuf + ecaret + n, ebuf + ecaret, elen - ecaret);
    memcpy(ebuf + ecaret, b, n);
    elen += n;
    ecaret += n;
    doc_dirty = 1;
}

static uint32_t prev_char(uint32_t at)
{
    if (!at)
        return 0;
    at--;
    while (at && (ebuf[at] & 0xC0) == 0x80)
        at--;
    return at;
}

static uint32_t next_char(uint32_t at)
{
    if (at >= elen)
        return elen;
    at++;
    while (at < elen && (ebuf[at] & 0xC0) == 0x80)
        at++;
    return at;
}

/* Copies a formula's text moved by (dr, dc): references move, those that
 * would leave the sheet become #REF. Returns the length (at most RAW_MAX). */
static uint32_t shift_refs(const uint8_t *p, uint32_t n, int32_t dr, int32_t dc, uint8_t *out)
{
    uint32_t i = 0, o = 0;
    int in_str = 0;
    if (!n || p[0] != '=') {
        memcpy(out, p, n);
        return n;
    }
    while (i < n && o < RAW_MAX) {
        uint8_t ch = p[i];
        if (ch == '"')
            in_str = !in_str;
        if (!in_str && is_alpha(ch) && (i == 0 || !is_alpha(p[i - 1])) && i + 1 < n && is_digit(p[i + 1])) {
            uint32_t j = i + 1, row = 0;
            while (j < n && is_digit(p[j])) {
                if (row < 1000)
                    row = row * 10 + (p[j] - '0');
                j++;
            }
            if (j >= n || (p[j] != '(' && !is_alpha(p[j]))) {
                int32_t r = (int32_t)row - 1 + dr, c = upper(ch) - 'A' + dc;
                uint8_t nm[4];
                uint32_t k, len;
                const uint8_t *w;
                if (r < 0 || r >= NROWS || c < 0 || c >= NCOLS || row < 1 || row > NROWS) {
                    w = (const uint8_t *)"#REF";
                    len = 4;
                } else {
                    len = name_of((uint32_t)r, (uint32_t)c, nm);
                    w = nm;
                }
                for (k = 0; k < len && o < RAW_MAX; k++)
                    out[o++] = w[k];
                i = j;
                continue;
            }
        }
        out[o++] = ch;
        i++;
    }
    return o;
}

static uint8_t clip_on, cl_r0, cl_c0, cl_r1, cl_c1;
static uint8_t tmp[RAW_MAX];

/* Copying, filling and clearing are a job over the target's cells,
 * JOB_CELLS a frame (each may compile a formula and format its value):
 * target cell (r, c) takes the source block's cell (sr0 + (r - tr0) mod
 * its height, sc0 + (c - tc0) mod its width), its references moved by as
 * much, so a block pastes once and a row or column fills by repeating.
 * Where the target overlaps the source, the order reads every source
 * cell before it is written. A clear empties the target. */
#define JOB_CELLS 12
enum { JOB_NONE, JOB_COPY, JOB_CLEAR };
static uint8_t job, job_full;
static uint32_t js_r0, js_c0, js_h, js_w, jt_r0, jt_c0, jt_h, jt_w, j_at;
static int8_t j_dy, j_dx;
static const char *job_msg;

static void start_job(uint8_t kind, uint32_t sr0, uint32_t sc0, uint32_t sh, uint32_t sw, uint32_t tr0, uint32_t tc0, uint32_t th, uint32_t tw,
                      const char *msg)
{
    if (tr0 + th > NROWS)
        th = NROWS - tr0;
    if (tc0 + tw > NCOLS)
        tw = NCOLS - tc0;
    job = kind;
    js_r0 = sr0;
    js_c0 = sc0;
    js_h = sh;
    js_w = sw;
    jt_r0 = tr0;
    jt_c0 = tc0;
    jt_h = th;
    jt_w = tw;
    j_dy = tr0 > sr0 ? -1 : 1;
    j_dx = tc0 > sc0 ? -1 : 1;
    j_at = 0;
    job_full = 0;
    job_msg = msg;
}

static void job_step(void)
{
    uint32_t k;
    for (k = 0; k < JOB_CELLS && j_at < jt_h * jt_w; k++, j_at++) {
        uint32_t a = j_at / jt_w, b = j_at % jt_w;
        uint32_t y = j_dy < 0 ? jt_h - 1 - a : a, x = j_dx < 0 ? jt_w - 1 - b : b;
        uint32_t tr = jt_r0 + y, tc = jt_c0 + x;
        if (job == JOB_CLEAR) {
            set_cell(idx(tr, tc), 0, 0);
            continue;
        }
        uint32_t r = js_r0 + y % js_h, c = js_c0 + x % js_w;
        uint16_t s = idx(r, c);
        uint32_t n = shift_refs(pool + c_off[s], c_rlen[s], (int32_t)tr - (int32_t)r, (int32_t)tc - (int32_t)c, tmp);
        if (!set_cell(idx(tr, tc), tmp, n))
            job_full = 1;
    }
    if (j_at < jt_h * jt_w)
        return;
    job = JOB_NONE;
    say(job_full ? "The sheet is full" : job_msg);
}

/* Fills the selection from its first row down (or first column right);
 * a selection one row high (one column wide) fills from the row above
 * (column to the left). */
static void fill(int down)
{
    uint32_t r0 = rmin(), r1 = rmax(), c0 = cmin(), c1 = cmax();
    if (down && r0 == r1) {
        if (!r0)
            return;
        r0--;
    }
    if (!down && c0 == c1) {
        if (!c0)
            return;
        c0--;
    }
    if (down)
        start_job(JOB_COPY, r0, c0, 1, c1 - c0 + 1, r0 + 1, c0, r1 - r0, c1 - c0 + 1, "Filled down");
    else
        start_job(JOB_COPY, r0, c0, r1 - r0 + 1, 1, r0, c0 + 1, r1 - r0 + 1, c1 - c0, "Filled right");
}

static void clear_range(void)
{
    start_job(JOB_CLEAR, 0, 0, 1, 1, rmin(), cmin(), rmax() - rmin() + 1, cmax() - cmin() + 1, "");
}

static void set_chart(uint8_t kind)
{
    if (chart_kind == kind && ch_r0 == rmin() && ch_c0 == cmin() && ch_r1 == rmax() && ch_c1 == cmax()) {
        chart_kind = 0;
        say("Chart removed");
    } else if (rmin() == rmax() && cmin() == cmax()) {
        say("Select the values first (Shift+arrows or a drag)");
        return;
    } else {
        chart_kind = kind;
        ch_r0 = (uint8_t)rmin();
        ch_c0 = (uint8_t)cmin();
        ch_r1 = (uint8_t)rmax();
        ch_c1 = (uint8_t)cmax();
        say("");
    }
    show_selection();
}

/* --- Saving and loading --------------------------------------------------- */

static uint8_t fdoc[HEAD + DATA_MAX];
#define fdata (fdoc + HEAD)
static uint32_t flen;
static content_req load_req, save_req, put_req;
static uint32_t load_h, save_h, put_h, pub_h;
static uint8_t loading, from_arg, saving; /* saving: 1 save, 2 publish */
static uint32_t ser_row, ser_last;
static uint8_t arg[32];
static char title[121];
static uint32_t title_len;

/* Reading the saved form at fdata[0..flen), LOAD_BYTES a frame. */
static uint32_t parse_at, parse_row;
static uint8_t parsing, cut;

static void clear_sheet(void)
{
    for (uint32_t i = 0; i < NCELLS; i++) {
        c_rlen[i] = c_clen[i] = c_flag[i] = c_kind[i] = c_dlen[i] = 0;
        c_state[i] = S_CLEAN;
    }
    pool_used = 0;
    chart_kind = 0;
    recalcing = 0;
    rsp = 0;
    top = left = sr = sc = ar = ac = 0;
    editing = 0;
    doc_dirty = 1;
}

static void start_parse(void)
{
    clear_sheet();
    parse_at = parse_row = 0;
    parsing = 1;
    cut = 0;
}

/* "D3" at p: 1 and the cell. */
static int parse_name(const uint8_t *p, uint32_t n, uint32_t *at, uint8_t *r, uint8_t *c)
{
    uint32_t i = *at, row = 0;
    if (i >= n || !is_alpha(p[i]))
        return 0;
    *c = (uint8_t)(upper(p[i]) - 'A');
    i++;
    while (i < n && is_digit(p[i]) && row < 1000)
        row = row * 10 + (p[i++] - '0');
    if (row < 1 || row > NROWS)
        return 0;
    *r = (uint8_t)(row - 1);
    *at = i;
    return 1;
}

static void parse_chart(const uint8_t *p, uint32_t n)
{
    uint32_t at;
    uint8_t kind = 0, r0, c0, r1, c1;
    if (n > 10 && !memcmp(p, "chart bar ", 10)) {
        kind = 1;
        at = 10;
    } else if (n > 11 && !memcmp(p, "chart line ", 11)) {
        kind = 2;
        at = 11;
    } else
        return;
    if (!parse_name(p, n, &at, &r0, &c0) || at >= n || p[at++] != ':' || !parse_name(p, n, &at, &r1, &c1) || at != n)
        return;
    chart_kind = kind;
    ch_r0 = r0 < r1 ? r0 : r1;
    ch_r1 = r0 < r1 ? r1 : r0;
    ch_c0 = c0 < c1 ? c0 : c1;
    ch_c1 = c0 < c1 ? c1 : c0;
}

static void parse_step(void)
{
    uint32_t budget = LOAD_BYTES;
    while (parse_at < flen && budget) {
        uint32_t end = parse_at;
        while (end < flen && fdata[end] != '\n')
            end++;
        uint32_t len = end - parse_at;
        const uint8_t *line = fdata + parse_at;
        if (len && line[len - 1] == '\r')
            len--;
        if (len && line[0] == 0x0C)
            parse_chart(line + 1, len - 1);
        else if (parse_row < NROWS) {
            uint32_t c = 0, s = 0, i;
            for (i = 0; i <= len; i++)
                if (i == len || line[i] == '\t') {
                    if (c < NCOLS && i > s) {
                        if (i - s > RAW_MAX)
                            cut = 1;
                        if (!set_cell(idx(parse_row, c), line + s, i - s))
                            cut = 1;
                    } else if (c >= NCOLS && i > s)
                        cut = 1;
                    c++;
                    s = i + 1;
                }
            parse_row++;
        } else if (len)
            cut = 1;
        budget = len + 1 >= budget ? 0 : budget - len - 1;
        parse_at = end + 1;
    }
    if (parse_at >= flen) {
        parsing = 0;
        dirty_all = 1;
        if (cut)
            say("Parts past Z99 or 120 bytes a cell were left out");
        doc_dirty = 1;
    }
}

/* Appends the rows from ser_row on, SAVE_ROWS a frame; then the chart. */
static int serialize_step(void)
{
    uint32_t k;
    if (!ser_row) {
        flen = 0;
        ser_last = 0;
        for (uint32_t i = 0; i < NCELLS; i++)
            if (c_rlen[i])
                ser_last = i / NCOLS + 1;
    }
    for (k = 0; k < SAVE_ROWS && ser_row < ser_last; k++, ser_row++) {
        int last = -1;
        for (int c = NCOLS - 1; c >= 0; c--)
            if (c_rlen[idx(ser_row, (uint32_t)c)]) {
                last = c;
                break;
            }
        for (int c = 0; c <= last; c++) {
            uint16_t i = idx(ser_row, (uint32_t)c);
            if (c)
                fdata[flen++] = '\t';
            memcpy(fdata + flen, pool + c_off[i], c_rlen[i]);
            flen += c_rlen[i];
        }
        fdata[flen++] = '\n';
    }
    if (ser_row < ser_last)
        return 0;
    if (chart_kind) {
        char nm[8];
        uint32_t n;
        const char *w = chart_kind == 1 ? "\x0C" "chart bar " : "\x0C" "chart line ";
        while (*w)
            fdata[flen++] = (uint8_t)*w++;
        range_name(nm, &n, ch_r0, ch_c0, ch_r1, ch_c1);
        memcpy(fdata + flen, nm, n);
        flen += n;
        fdata[flen++] = '\n';
    }
    return 1;
}

static void header(void)
{
    static const uint8_t h[5] = { 's', 'h', 'e', 'e', 't' };
    memcpy(fdoc, "TDOC", 4);
    memset(fdoc + 4, 0, 33);
    fdoc[TDOC_HEAD - 1] = FMT_NAME_LEN;
    memcpy(fdoc + TDOC_HEAD, h, FMT_NAME_LEN);
}

static void finish_save(void)
{
    if (saving == 1) {
        save_h = record_save(&save_req, "sheet", fdata, flen);
        say(save_h ? "Saving\xE2\x80\xA6" : "Not saved");
    } else {
        /* The title: A1's text, else "Sheet". */
        uint32_t n;
        const uint8_t *p;
        if (c_rlen[0] && cell_value(0).k == K_STR) {
            p = text_of(0, &n);
            if (n > 120)
                n = 120;
        } else {
            p = (const uint8_t *)"Sheet";
            n = 5;
        }
        memcpy(title, p, n);
        title_len = n;
        header();
        put_h = content_put(&put_req, fdoc, HEAD + flen);
        say(put_h ? "Publishing\xE2\x80\xA6" : "Not published");
    }
    saving = 0;
}

static void begin_save(uint8_t how)
{
    if (!keeps) {
        say(how == 1 ? "No saving on this host" : "No publishing on this host");
        return;
    }
    if (saving || save_h || put_h || pub_h || loading || parsing) {
        say("Busy: try again in a moment");
        return;
    }
    if (editing)
        commit();
    saving = how;
    ser_row = 0;
    say(how == 1 ? "Saving\xE2\x80\xA6" : "Publishing\xE2\x80\xA6");
}

static void load_chunk(void)
{
    uint32_t want = DATA_MAX - flen > CHUNK ? CHUNK : DATA_MAX - flen;
    if (from_arg)
        load_h = content_get(&load_req, arg, HEAD + flen, want, fdata + flen);
    else
        load_h = record_load(&load_req, "sheet", flen, want, fdata + flen);
    if (!load_h) {
        loading = 0;
        say("Could not load the sheet");
    }
}

/* The example: an estimate for a service. */
static const char example[] =
    "Estimate\n"
    "Item\tQty\tPrice\tAmount\n"
    "Fence panels\t12\t45.50\t=B3*C3\n"
    "Posts\t13\t18.75\t=B4*C4\n"
    "Concrete\t13\t6.20\t=B5*C5\n"
    "Labour (h)\t16\t38\t=B6*C6\n"
    "Delivery\t1\t40\t=B7*C7\n"
    "\t\tSubtotal\t=SUM(D3:D7)\n"
    "\t8.25%\tTax\t=ROUND(D8*B9,2)\n"
    "\t\tTotal\t=D8+D9\n"
    "\x0C" "chart bar D3:D7\n";

static const char *arg_msg; /* why the argument was not opened */

static void load_example(void)
{
    flen = sizeof example - 1;
    memcpy(fdata, example, flen);
    start_parse();
    say(arg_msg ? arg_msg : "An example: an estimate for a service. Ctrl+N twice starts a new sheet");
}

static void loaded(void)
{
    uint32_t got = load_req.status == STATUS_DONE ? load_req.length : 0;
    load_h = 0;
    if (load_req.status != STATUS_DONE) {
        loading = 0;
        if (from_arg) {
            from_arg = 0;
            arg_msg = load_req.status == STATUS_NOT_FOUND ? "Document not found" : "Could not get the document";
            say(arg_msg);
            flen = 0;
            loading = 1;
            load_chunk();
            return;
        }
        if (load_req.status == STATUS_NOT_FOUND)
            load_example();
        else
            say("Could not load the saved sheet");
        return;
    }
    flen += got;
    uint32_t size = from_arg ? load_req.size - HEAD : load_req.size;
    if (from_arg && load_req.size < HEAD)
        size = 0;
    if (size > DATA_MAX) {
        loading = 0;
        say("The sheet is larger than 64 KiB");
        return;
    }
    if (flen < size && got) {
        load_chunk();
        return;
    }
    loading = 0;
    start_parse();
    if (from_arg)
        say("A published sheet; Ctrl+S keeps it as your sheet");
}

/* The argument's header (fetched alone first). */
static content_req head_req;
static uint32_t head_h;

static void got_head(void)
{
    static uint8_t got[HEAD];
    head_h = 0;
    memcpy(got, fdoc, HEAD);
    header();
    if (head_req.status == STATUS_DONE && head_req.length == HEAD && !memcmp(fdoc, got, HEAD)) {
        from_arg = 1;
        flen = 0;
        load_chunk();
        return;
    }
    arg_msg = head_req.status == STATUS_DONE ? "Not a sheet document" : head_req.status == STATUS_NOT_FOUND ? "Document not found" : "Could not get the document";
    say(arg_msg);
    from_arg = 0;
    flen = 0;
    load_chunk();
}

static void done(uint32_t h)
{
    if (h == head_h)
        got_head();
    else if (h == load_h)
        loaded();
    else if (h == save_h) {
        save_h = 0;
        say(save_req.status == STATUS_DONE ? "Saved" : save_req.status == STATUS_NO_ROOM ? "Not saved: no room" : "Not saved");
    } else if (h == put_h) {
        put_h = 0;
        if (put_req.status != STATUS_DONE) {
            say("Not published");
            return;
        }
        static uint8_t blob[32];
        memcpy(blob, put_req.id, 32);
        pub_h = content_publish_kind(&put_req, blob, title, title_len, ENTRY_DOCUMENT);
        if (!pub_h)
            say("Not published");
    } else if (h == pub_h) {
        pub_h = 0;
        if (put_req.status != STATUS_DONE) {
            say(put_req.status == STATUS_REFUSED ? "Not published (refused)" : "Not published");
            return;
        }
        uint32_t n = 0;
        const char *w = "Published as \"";
        while (*w)
            status_buf[n++] = *w++;
        for (uint32_t k = 0; k < title_len && n < 70; k++)
            status_buf[n++] = title[k];
        status_buf[n++] = '"';
        status_buf[n] = 0;
        say(status_buf);
    }
}

/* --- Input ---------------------------------------------------------------- */

static uint8_t buttons;

static void key(uint8_t k, uint8_t mods)
{
    int shift = mods & MOD_SHIFT, ctrl = mods & MOD_CTRL;
    if (editing) {
        switch (k) {
        case KEY_ESCAPE:
            editing = 0;
            doc_dirty = 1;
            return;
        case KEY_ENTER:
            commit();
            move_to((int32_t)sr + (shift ? -1 : 1), (int32_t)sc, 0);
            return;
        case KEY_TAB:
            commit();
            move_to((int32_t)sr, (int32_t)sc + (shift ? -1 : 1), 0);
            return;
        case KEY_UP:
        case KEY_DOWN:
            commit();
            move_to((int32_t)sr + (k == KEY_UP ? -1 : 1), (int32_t)sc, 0);
            return;
        case KEY_LEFT: ecaret = prev_char(ecaret); break;
        case KEY_RIGHT: ecaret = next_char(ecaret); break;
        case KEY_HOME: ecaret = 0; break;
        case KEY_END: ecaret = elen; break;
        case KEY_BACKSPACE:
            if (ecaret) {
                uint32_t p = prev_char(ecaret);
                memmove(ebuf + p, ebuf + ecaret, elen - ecaret);
                elen -= ecaret - p;
                ecaret = p;
            }
            break;
        case KEY_DELETE:
            if (ecaret < elen) {
                uint32_t q = next_char(ecaret);
                memmove(ebuf + ecaret, ebuf + q, elen - q);
                elen -= q - ecaret;
            }
            break;
        default: return;
        }
        doc_dirty = 1;
        return;
    }
    if (ctrl) {
        if (k != KEY_N)
            new_armed = 0;
        switch (k) {
        case KEY_C:
            clip_on = 1;
            cl_r0 = (uint8_t)rmin();
            cl_c0 = (uint8_t)cmin();
            cl_r1 = (uint8_t)rmax();
            cl_c1 = (uint8_t)cmax();
            say("Copied: Ctrl+Y pastes at the selected cell");
            return;
        case KEY_Y:
            if (!clip_on) {
                say("Nothing copied: Ctrl+C copies");
                return;
            }
            start_job(JOB_COPY, cl_r0, cl_c0, cl_r1 - cl_r0 + 1u, cl_c1 - cl_c0 + 1u, sr, sc, cl_r1 - cl_r0 + 1u, cl_c1 - cl_c0 + 1u, "Pasted");
            ar = sr + (cl_r1 - cl_r0);
            ac = sc + (cl_c1 - cl_c0);
            if (ar >= NROWS)
                ar = NROWS - 1;
            if (ac >= NCOLS)
                ac = NCOLS - 1;
            doc_dirty = 1;
            return;
        case KEY_D: fill(1); return;
        case KEY_R: fill(0); return;
        case KEY_G: set_chart(1); return;
        case KEY_L: set_chart(2); return;
        case KEY_S: begin_save(1); return;
        case KEY_P: begin_save(2); return;
        case KEY_N:
            if (!new_armed) {
                new_armed = 1;
                say("Press Ctrl+N again to clear the whole sheet");
                return;
            }
            new_armed = 0;
            clear_sheet();
            say("A new sheet");
            return;
        case KEY_HOME: move_to(0, 0, shift); return;
        default: return;
        }
    }
    new_armed = 0;
    switch (k) {
    case KEY_UP: move_to((int32_t)sr - 1, (int32_t)sc, shift); return;
    case KEY_DOWN: move_to((int32_t)sr + 1, (int32_t)sc, shift); return;
    case KEY_LEFT: move_to((int32_t)sr, (int32_t)sc - 1, shift); return;
    case KEY_RIGHT: move_to((int32_t)sr, (int32_t)sc + 1, shift); return;
    case KEY_TAB: move_to((int32_t)sr, (int32_t)sc + (shift ? -1 : 1), 0); return;
    case KEY_ENTER: move_to((int32_t)sr + (shift ? -1 : 1), (int32_t)sc, 0); return;
    case KEY_PAGE_UP: move_to((int32_t)sr - (int32_t)vrows(), (int32_t)sc, shift); return;
    case KEY_PAGE_DOWN: move_to((int32_t)sr + (int32_t)vrows(), (int32_t)sc, shift); return;
    case KEY_HOME: move_to((int32_t)sr, 0, shift); return;
    case KEY_F2: start_edit(1); return;
    case KEY_DELETE:
    case KEY_BACKSPACE: clear_range(); return;
    case KEY_ESCAPE:
        ar = sr;
        ac = sc;
        say("");
        return;
    default: return;
    }
}

/* The cell under a point of the screen: 1 and (r, c), else 0. */
static int cell_at(uint32_t x, uint32_t y, uint32_t *r, uint32_t *c)
{
    uint32_t rows = vrows(), i;
    if (y < g_y[1] || y >= g_y[rows + 1] || x < COLW || x >= 640)
        return 0;
    for (i = 0; i < rows && y >= g_y[2 + i]; i++)
        ;
    *r = top + i;
    *c = left + x / COLW - 1;
    return 1;
}

static void pointer(const terse_event *e)
{
    uint8_t b = EV_POINTER_BUTTONS(*e), was = buttons;
    uint32_t r, c;
    buttons = b;
    if (!(b & BUTTON_PRIMARY))
        return;
    if (!cell_at(EV_POINTER_X(*e), EV_POINTER_Y(*e), &r, &c))
        return;
    if (!(was & BUTTON_PRIMARY)) {
        if (editing)
            commit();
        move_to((int32_t)r, (int32_t)c, 0);
    } else if (r != sr || c != sc)
        move_to((int32_t)r, (int32_t)c, 1);
}

static int busy(void) { return loading || parsing || head_h || job; }

int main(void)
{
    page_region(0, 0, 640, 480);
    page_background(255, 255, 255, 255);
    has_vec = terse_has_device(3);
    keeps = terse_has_device(4);
    default_geometry();
    if (has_vec)
        vec_style(VEC_STYLE_CLEAN);
    if (!keeps)
        load_example(), say("Sheets are not kept on this host");
    else if (terse_argument(arg)) {
        loading = 1;
        head_h = content_get(&head_req, arg, 0, HEAD, fdoc);
        if (!head_h) {
            say("Could not get the document");
            load_chunk();
        }
    } else {
        loading = 1;
        flen = 0;
        load_chunk();
    }
    for (;;) {
        terse_event ev;
        page_place place;
        while (terse_event_pop(&ev)) {
            if (ev.type == EV_CONTENT_DONE) {
                done(ev.a);
                continue;
            }
            if (ev.type == EV_PAGE_LOCATED || ev.type == EV_PAGE_LOCATED_Y) {
                if (page_place_event(&ev, &place))
                    located(&place);
                continue;
            }
            if (busy())
                continue;
            if (ev.type == EV_KEY_DOWN)
                key(ev.a, ev.b);
            else if (ev.type == EV_TEXT) {
                if (!editing) {
                    new_armed = 0;
                    start_edit(0);
                }
                insert_cp(EV_TEXT_CP(ev));
            } else if (ev.type == EV_POINTER)
                pointer(&ev);
            else if (ev.type == EV_WHEEL) {
                int32_t t = (int32_t)top + 3 * EV_WHEEL_Y(ev), max = NROWS - (int32_t)vrows();
                top = (uint32_t)(t < 0 ? 0 : t > max ? max : t);
                doc_dirty = 1;
            }
        }
        if (parsing)
            parse_step();
        else if (job)
            job_step();
        else {
            if (dirty_all && !loading)
                start_recalc();
            if (recalcing)
                recalc_step();
        }
        if (saving && !recalcing && serialize_step())
            finish_save();
        if (doc_dirty && !parsing && !loading) {
            doc_dirty = 0;
            build_doc();
        }
        while (loc_at < loc_n && page_locate(loc_ids[loc_at]) == PAGE_DONE)
            loc_at++;
        if (vec_dirty && has_vec && !parsing && !loading) {
            vec_dirty = 0;
            build_vec();
        }
        terse_wait_frame();
    }
}
