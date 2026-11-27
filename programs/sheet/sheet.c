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
 * places; a result outside the range is #NUM!. A number is shown with
 * thousands separators and its decimals without trailing zeros, but with
 * at least 2 if it has any (12.5 shows as 12.50); one typed with % shows
 * as a percentage. A number too wide for its column loses its decimals,
 * then its separators, then shows as ####.
 *
 * Formulas: numbers, cell references (A1, a1; $A$1, A$1, $A1 keep the
 * part after the $ when copied), ranges (A1:B5) as function arguments,
 * + - * / ^ (a whole exponent, -64 to 64), unary minus, % after a number
 * or reference, & (joins text), parentheses, the comparisons = <> < > <=
 * >= (TRUE or FALSE), TRUE, FALSE, text in double quotes, and the
 * functions SUM, AVERAGE, MIN, MAX, COUNT, COUNTA, AND, OR, ROUND(x,
 * places), ABS, INT, NOT and IF(test, then[, else]). A formula is kept with
 * its names and references in capitals and its open brackets closed. An
 * empty cell counts 0 (=A1 of an empty A1 is 0); in ranges, text, TRUE,
 * FALSE and empty cells are skipped (COUNT counts numbers, COUNTA what is
 * not empty, AND and OR take TRUE and FALSE). TRUE or FALSE typed into a
 * cell is that value. Errors are values that spread to the cells that use
 * them, named as other spreadsheets name them: #DIV/0!, #CYCLE! (a cell
 * that depends on itself, and every cell of the cycle), #ERROR! (a formula
 * that cannot be read), #NAME? (no such function or name), #REF! (a
 * reference outside the sheet), #VALUE! (text where a number is needed),
 * #NUM! (out of range); the content line says what the selected one means.
 *
 * Recalculation. After every change every formula is recalculated in
 * dependency order: a depth-first walk with an explicit stack that
 * computes a cell's references before the cell, and finds cycles where it
 * meets a cell that is still on the stack. The walk does at most
 * WORK_PER_FRAME steps of work a frame and goes on in the next frame, so
 * no frame runs long however large the sheet (the status says
 * "Calculating" meanwhile; the cells show their old values until then).
 *
 * The screen is one page document: the content line (the selection's
 * name and the active cell's content as typed, then a formula's value or
 * what its error means; while editing, the entry with its text cursor,
 * as much of it as fits around the cursor), a table of 8
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
 * Keys and the pointer, as in other spreadsheets. Arrows, Tab and Enter
 * (with Shift: back; Enter after Tabs goes back to the column they began
 * in) move; Ctrl+arrows go to the edge of the filled cells, Ctrl+Home to
 * A1, Ctrl+End to the last cell used, Ctrl+A selects the sheet.
 * Shift+arrows, Shift+click and a drag select a range, keeping the active
 * cell where it began; a click on a column letter or row number selects
 * it. Typing starts a new entry in the active cell ("enter" mode); F2, a
 * double-click (two presses on a cell DOUBLE_FRAMES apart or less) or a
 * press on the content line edits its content ("edit" mode). The entry
 * shows on the content line and in its cell, with a text cursor; every
 * character of both is a page anchor, so a press on one puts the cursor
 * before it. In enter mode an arrow keeps the entry and moves; in edit
 * mode Left, Right, Home and End move the cursor; F2 switches. Backspace
 * and Delete delete, Enter and Tab keep the entry and move, Esc cancels,
 * F4 cycles the $ of the reference at the cursor.
 *
 * Pointing. While a formula is typed and the cursor is where a reference
 * may go (after =, an operator, "(", "," or ":"), a press on another cell
 * puts its name in at the cursor, a drag a range ("B3:B7"), and in enter
 * mode the arrows (Shift: a range) move such a reference from the cell
 * being edited; the next press or arrow replaces the reference just put
 * in. The range pointed at is shaded orange. Anywhere else in an entry, a
 * press on another cell keeps the entry and selects that cell.
 *
 * Delete clears the selection (keeping its number formats). Ctrl+C copies
 * it, Ctrl+X cuts it, Ctrl+V pastes at the selection's top left (one cell
 * copied fills a selected range; after a cut, moves the cells, their
 * formulas unchanged), until Esc forgets what was copied (a dashed green
 * edge shows it meanwhile). A paste arrives as the paste key's down, the
 * text, its up (SPEC §events): with cells copied, the text is ignored;
 * else it goes into cells from the selection's top left, a tab to the
 * next cell and a line break to the next row (at most 16 KiB); while an
 * entry is typed it joins the entry. Ctrl+C and Ctrl+X also offer the
 * system clipboard the copied cells as text (SPEC §system `copy`: a line
 * per row, cells as shown, separated by tabs), which the host takes in
 * answer to the user's copy key. Text that arrives
 * while Control (without Alt) or Alt (without Control) is held is not
 * typing.
 *
 * Rows and columns. Ctrl+Plus (Ctrl+Shift+=, or the keypad's +) inserts
 * as many rows as the selection has, above it; Ctrl+Minus deletes them;
 * with whole columns selected (their letters clicked), columns. Every
 * formula's references move as in other spreadsheets: what lies past the
 * place moves (also after a $); a reference to a deleted cell is #REF!;
 * a range that loses an end shrinks, one wholly deleted is #REF!. The
 * chart's range follows. What an insertion would push off the sheet must
 * be empty. A job: JOB_PUTS cells a frame.
 *
 * Sorting. The Data menu's Sort A to Z and Sort Z to A (with Ctrl+Shift+A
 * and Ctrl+Shift+D, shown there, as shortcuts) sort the
 * selection, or the block of filled cells around the active cell, by the
 * active cell's column: numbers (and TRUE, FALSE) by value, then text
 * without regard to case, then errors; empty cells last either way; equal
 * keys keep their order. If the first row is text over a number in that
 * column it is a header and stays (the status line says so). Rows move
 * whole, with their relative references moved as a copy moves them, so a
 * row's own formula (=B3*C3) stays right.
 *
 * Number formats (c_fmt, per cell): general (as typed); Ctrl+Shift+1 two
 * decimals with thousands separators (again: without separators);
 * Ctrl+Shift+5 a percentage; Ctrl+period and Ctrl+comma one decimal more
 * or fewer (0 to 4); Ctrl+Shift+~ general again. They apply to the
 * selection, copy and move with cells, survive undo, insertions and
 * sorting, and are saved.
 *
 * Menus. A menu bar at the top (File, Edit, Data, Format, Chart, Help:
 * terminal characters over panels of the 2D display, as below) opens with
 * a press on a title, F10 or Alt and its letter; every item shows its
 * key. File, Open lists the saved sheets; File, Save as asks for a name
 * on the content line. F1 lists every key in place of the chart.
 * Ctrl+D fills the selection down from its top row and Ctrl+R right from
 * its left column (references move with the copy, except after a $).
 * Ctrl+Z undoes and Ctrl+Y (or Ctrl+Shift+Z) redoes every change of cells
 * (the log below). Ctrl+G shows a bar chart of the selection, Ctrl+L a
 * line chart (the same key again removes it). Ctrl+S saves, Ctrl+P
 * publishes, Ctrl+N twice starts a new sheet. The status line names the
 * mode of an entry and its keys, else what happened last and how to begin
 * an entry.
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
 * a space, "bar" or "line", a space and the range ("chart bar D3:D7");
 * then a line for each run of cells of a row with the same number format:
 * a form feed, "format", a space, the range, a space, the decimals, "s"
 * for separators, "%" for a percentage ("format D3:D7 2s"). (An older
 * sheet without these lines loads as general; an older reader ignores
 * them, as it ignores any line that starts with a form feed.)
 * Lines end in LF (a CR before it is ignored); rows past 99 and cells past
 * Z are ignored on reading; cells longer than 120 bytes are cut. At most
 * 65,536 bytes.
 *
 * Documents. Ctrl+S saves the sheet as the record "sheet" (SPEC
 * §content.records), which is loaded at start (if there is none, the
 * example, an estimate for a service, is shown); a sheet named with Save
 * as is the record "sheet:" and its name (at most 24 bytes), and Ctrl+S
 * then saves it there; File, Open lists these records (op 11h) and loads
 * the one chosen. Ctrl+P publishes it as a
 * document (SPEC §content.documents: "TDOC", 0, 32 zero bytes, 5,
 * "sheet", then the saved form) titled with A1's text (or "Sheet"), after
 * the host asks the user. Started with a sheet document as its argument,
 * it opens that document instead; Ctrl+S then keeps it as the user's own
 * sheet. Content (device 4) is optional: without it nothing is loaded or
 * kept, and Ctrl+S and Ctrl+P say so. The 2D display (device 3) is
 * optional too: without it there are no grid lines and the selected cell
 * is shown by its colour (and that of its row number and column letter).
 * The terminal (device 2) is optional; where it is there (with the 2D
 * display) it draws the menu bar's characters, its cursor hidden; without
 * either there is no menu bar, and the keys still work.
 *
 * Budget. Recalculation counts its steps in work units of roughly 40
 * budget units (WORK_PER_FRAME a frame), reading a saved sheet likewise
 * (PARSE_WORK), and copies, fills and clears do JOB_CELLS cells a frame;
 * input that arrives meanwhile waits, in order. The busiest frames
 * measured are about 70,000 of the 125,000 units (a key that commits an
 * entry: the entry, about 40,000 of recalculation and the page
 * document). */

#include <stdint.h>
#include "terse.h"

typedef int64_t i64;
typedef uint64_t u64;

#define NCOLS 26
#define NROWS 99
#define NCELLS (NCOLS * NROWS)
#define VCOLS 7
#define VROWS_ALL 16
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
#define WORK_PER_FRAME 1000
#define PARSE_WORK 1000
#define PARSE_CELL 16
#define SAVE_ROWS 6
#define BARS_MAX 32
#define CHART_H 72
#define STACK_MAX 40

#define KEY_A 0x04
#define KEY_B 0x05
#define KEY_C 0x06
#define KEY_D 0x07
#define KEY_G 0x0A
#define KEY_L 0x0F
#define KEY_N 0x11
#define KEY_P 0x13
#define KEY_R 0x15
#define KEY_S 0x16
#define KEY_V 0x19
#define KEY_X 0x1B
#define KEY_Y 0x1C
#define KEY_Z 0x1D
#define KEY_E 0x08
#define KEY_F_ 0x09
#define KEY_H 0x0B
#define KEY_O 0x12
#define KEY_F10 0x43
#define KEY_F1 0x3A
#define KEY_F2 0x3B
#define KEY_F4 0x3D
#define KEY_DELETE 0x4C
#define KEY_INSERT 0x49
#define KEY_1 0x1E
#define KEY_5 0x22
#define KEY_MINUS 0x2D
#define KEY_EQUAL 0x2E
#define KEY_GRAVE 0x35
#define KEY_COMMA 0x36
#define KEY_PERIOD 0x37
#define KEY_KP_MINUS 0x56
#define KEY_KP_PLUS 0x57
#define KEY_LSHIFT 0xE1
#define KEY_RSHIFT 0xE5
#define MOD_SHIFT 0x01
#define MOD_CTRL 0x02
/* Two presses on one cell this many frames apart or less are a double-click. */
#define DOUBLE_FRAMES 30

/* Anchors of the page document. */
#define A_HEAD 2000
#define A_ROW 1000
#define A_BOTTOM 999
/* Each character of the content line's text (A_LINE + its byte offset),
 * and of the text in the cell being edited: a press on one reports it. */
#define A_LINE 3000
#define A_CELL 3200

/* Value kinds. */
enum { K_EMPTY, K_NUM, K_STR, K_ERR, K_RANGE, K_BOOL };
/* Errors. */
enum { E_NONE, E_DIV, E_CYCLE, E_ERR, E_REF, E_VALUE, E_NUM, E_NAME };
static const char *const err_text[] = { "", "#DIV/0!", "#CYCLE!", "#ERROR!", "#REF!", "#VALUE!", "#NUM!", "#NAME?" };
static const char *const err_why[] = {
    "",
    "division by zero",
    "circular reference: the formula depends on its own cell",
    "the formula cannot be read",
    "a reference outside the sheet (A1 to Z99)",
    "text or a range where a number is needed",
    "a number out of range",
    "no function of that name",
};

/* Cell flags. */
#define F_FORMULA 0x01
#define F_TEXT 0x02
#define F_NUM 0x04
#define F_PCT 0x08
#define F_BAD 0x10 /* a formula that cannot be read: its code is the error */
#define F_BOOL 0x20 /* TRUE or FALSE typed */
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
    OP_CAT,   /* & */
    OP_BOOL,  /* 0 or 1 */
};
/* The functions over lists of values and ranges come first (aggregate). */
enum { FN_SUM, FN_AVERAGE, FN_MIN, FN_MAX, FN_COUNT, FN_COUNTA, FN_AND, FN_OR, FN_ROUND, FN_ABS, FN_INT, FN_NOT, FN_IF, FN_N };
#define FN_LISTS FN_OR
static const char *const fn_name[FN_N] = { "SUM", "AVERAGE", "MIN", "MAX", "COUNT", "COUNTA", "AND", "OR", "ROUND", "ABS", "INT", "NOT", "IF" };
static const uint8_t fn_min[FN_N] = { 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2 };
static const uint8_t fn_max[FN_N] = { 30, 30, 30, 30, 30, 30, 30, 30, 2, 1, 1, 1, 3 };

/* --- The cells ------------------------------------------------------------ */

/* The pool holds each cell's block: owner (2 bytes, FFFFh once dead), then
 * the text as typed and the formula's code. */
static uint8_t pool[POOL];
static uint32_t pool_used;
static uint16_t c_off[NCELLS]; /* the text's offset in the pool */
static uint8_t c_rlen[NCELLS] __attribute__((aligned(4))), c_clen[NCELLS] __attribute__((aligned(4))),
    c_flag[NCELLS] __attribute__((aligned(4)));
static uint8_t c_kind[NCELLS] __attribute__((aligned(4))), c_state[NCELLS] __attribute__((aligned(4)));
static i64 c_val[NCELLS];
static uint8_t c_disp[NCELLS][DISP_MAX];
static uint8_t c_dlen[NCELLS] __attribute__((aligned(4)));
static uint8_t c_fmt[NCELLS] __attribute__((aligned(4))); /* the number format (FMT_*) */

static uint16_t idx(uint32_t r, uint32_t c) { return (uint16_t)(r * NCOLS + c); }

/* Zeroes n bytes at a word-aligned p, a word at a time (the library's
 * memset goes a byte at a time). */
static void zero(void *p, uint32_t n)
{
    uint32_t *w = p;
    uint8_t *b;
    for (; n >= 16; n -= 16, w += 4)
        w[0] = w[1] = w[2] = w[3] = 0;
    for (b = (uint8_t *)w; n; n--)
        *b++ = 0;
}

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
    if (ch == '+' || ch == '=' || ch == '<' || ch == '>' || ch == '^' || ch == '~')
        return 100; /* the wide operators of formulas */
    if (ch == '/' || ch == '*')
        return 50;
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
#define LINE_WIDTH 4400 /* the content line's room for an entry, as size-12 tenths */

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

/* A cell's number format (c_fmt): 0 general (as typed: separators, the
 * decimals it has, at least 2 if any; a number typed with % as a
 * percentage); else FMT_SET with 0-4 fixed decimals, thousands
 * separators or not, a percentage or not. */
#define FMT_SET 0x80
#define FMT_SEP 0x08
#define FMT_PCT 0x10
#define FMT_DEC 0x07

/* A number in a fixed format; 0 if it does not fit in `width`. */
static uint32_t format_fixed(i64 v, uint8_t fmt, uint32_t width, uint8_t *out)
{
    uint8_t buf[40];
    uint32_t frac, dec = fmt & FMT_DEC, unit = 1, q, n, i, try;
    u64 ip;
    if (dec > 4)
        dec = 4;
    if (fmt & FMT_PCT && !mul_fix(v, 100 * SCALE, &v))
        return 0;
    for (i = dec; i < 4; i++)
        unit *= 10;
    ip = divs(mag(v), SCALE, &frac);
    q = (frac + unit / 2) / unit;
    if (q * unit >= SCALE) {
        ip++;
        q = 0;
    }
    for (try = fmt & FMT_SEP ? 0 : 1; try < 2; try++) {
        n = 0;
        if (v < 0 && (ip || q))
            buf[n++] = '-';
        n += put_int(ip, !try, buf + n);
        if (dec) {
            uint32_t d = q;
            buf[n++] = '.';
            for (i = 0; i < dec; i++) {
                buf[n + dec - 1 - i] = (uint8_t)('0' + d % 10);
                d /= 10;
            }
            n += dec;
        }
        if (fmt & FMT_PCT)
            buf[n++] = '%';
        if (!width || est_width(buf, n) <= width) {
            memcpy(out, buf, n);
            return n;
        }
    }
    return 0;
}

/* A number as shown in a cell of `width` tenths (0: no limit), in format
 * `fmt` (pct: typed with %, for the general format). */
static uint32_t format_num(i64 v, uint8_t fmt, int pct, uint32_t width, uint8_t *out)
{
    uint8_t buf[40];
    u64 m;
    uint32_t frac, n, i, try;
    if (fmt & FMT_SET) {
        n = format_fixed(v, fmt, width, out);
        if (!n) {
            memcpy(out, "####", 4);
            n = 4;
        }
        return n;
    }
    if (pct && !mul_fix(v, 100 * SCALE, &v))
        pct = 0;
    m = mag(v);
    /* Tries: as it is; to 2 places; to none; without separators. */
    for (try = 0; try < 4; try++) {
        u64 ip = divs(m, SCALE, &frac);
        n = 0;
        if (try == 1) {
            frac = (frac + 50) / 100 * 100;
            if (frac == SCALE) {
                ip++;
                frac = 0;
            }
        } else if (try > 1) {
            if (2 * frac >= SCALE)
                ip++;
            frac = 0;
        }
        if (v < 0 && (ip || frac))
            buf[n++] = '-';
        n += put_int(ip, try < 3, buf + n);
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
    if (try == 4) {
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

/* The string value packed in an i64: offset, length, and whether it is in
 * the pool (bit 24 clear) or in the strings that & made during this
 * recalculation (set). */
#define STR(off, len) ((i64)(off) | (i64)(len) << 16)
#define STR_OFF(v) ((uint32_t)(v) & 0xFFFF)
#define STR_LEN(v) ((uint32_t)((v) >> 16) & 0xFF)
#define STR_MADE ((i64)1 << 24)
#define MADE_MAX 4096
static uint8_t made[MADE_MAX];
static uint32_t made_used;

static const uint8_t *str_p(i64 v) { return (v & STR_MADE ? made : pool) + STR_OFF(v); }

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
    uint8_t kind = c_flag[i] & F_FORMULA ? c_kind[i]
                   : c_flag[i] & F_NUM   ? K_NUM
                   : c_flag[i] & F_BOOL  ? K_BOOL
                   : c_flag[i] & F_TEXT  ? K_STR
                                         : K_EMPTY;
    if (kind == K_NUM)
        n = format_num(c_val[i], c_fmt[i], c_flag[i] & F_PCT, CELL_WIDTH, d);
    else if (kind == K_BOOL) {
        p = (const uint8_t *)(c_val[i] ? "TRUE" : "FALSE");
        while (p[n]) {
            d[n] = p[n];
            n++;
        }
    } else if (kind == K_ERR) {
        p = (const uint8_t *)err_text[c_val[i]];
        while (p[n]) {
            d[n] = p[n];
            n++;
        }
    } else if (kind == K_STR) {
        uint32_t len;
        if (c_flag[i] & F_FORMULA) {
            p = str_p(c_val[i]);
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

/* A reference at the current position (a letter and a row number, each
 * with an optional $ before it), or 0. */
static int read_ref(uint8_t *r, uint8_t *c)
{
    uint32_t i = spos, row = 0, nd = 0;
    if (i < send && src[i] == '$')
        i++;
    if (i >= send || !is_alpha(src[i]))
        return 0;
    int col = upper(src[i]) - 'A';
    i++;
    if (i < send && is_alpha(src[i])) {
        /* Two letters and digits: a column past Z. */
        uint32_t j = i;
        while (j < send && is_alpha(src[j]))
            j++;
        if (j < send && src[j] == '$')
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
    if (i < send && src[i] == '$')
        i++;
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
        /* "#REF!" left by a copy that moved a reference off the sheet. */
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
        if (peek() != '(') {
            /* TRUE and FALSE; any other name is unknown. */
            static const char *const tf[2] = { "FALSE", "TRUE" };
            for (uint32_t t = 0; t < 2; t++) {
                uint32_t k = 0;
                while (k < len && tf[t][k] && upper(src[start + k]) == tf[t][k])
                    k++;
                if (k == len && !tf[t][k]) {
                    emit(OP_BOOL);
                    emit((uint8_t)t);
                    return;
                }
            }
            perr = E_NAME;
            return;
        }
        if (f == FN_N) {
            perr = E_NAME;
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

static void p_cat(int depth)
{
    p_add(depth);
    while (!perr && peek() == '&') {
        spos++;
        p_add(depth);
        emit(OP_CAT);
    }
}

static void p_expr(int depth)
{
    p_cat(depth);
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
        p_cat(depth);
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
    /* TRUE and FALSE, in any case. */
    if (n == 4 || n == 5) {
        const char *w = n == 4 ? "TRUE" : "FALSE";
        for (at = 0; at < n && upper(p[at]) == w[at]; at++)
            ;
        if (at == n) {
            *val = n == 4 ? SCALE : 0;
            return F_BOOL;
        }
    }
    return F_TEXT;
}

/* --- The pool ------------------------------------------------------------- */

static uint8_t dirty_all; /* a recalculation is wanted */
/* How many formulas each row holds: the scans for them skip the rows
 * without any. */
static uint8_t row_f[NROWS] __attribute__((aligned(4)));

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
        if (c_flag[i] & F_FORMULA)
            row_f[i / NCOLS]--;
        c_rlen[i] = c_flag[i] = c_kind[i] = c_dlen[i] = 0;
        c_state[i] = S_CLEAN;
        compact();
        if (pool_used + size > POOL)
            return 0;
    } else
        free_block(i);
    if (c_flag[i] & F_FORMULA)
        row_f[i / NCOLS]--;
    if (flag & F_FORMULA)
        row_f[i / NCOLS]++;
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

static const uint8_t op_size[] = { 1, 9, 0, 3, 5, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 3, 1, 2 };

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
    } else if (c_flag[i] & (F_NUM | F_BOOL)) {
        x.k = c_flag[i] & F_NUM ? K_NUM : K_BOOL;
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
static value boolean(int t)
{
    value x = { K_BOOL, t ? SCALE : 0 };
    return x;
}

static i64 as_num(value x, uint8_t *e)
{
    if (x.k == K_NUM || x.k == K_BOOL)
        return x.v;
    if (x.k == K_EMPTY)
        return 0;
    if (!*e)
        *e = x.k == K_ERR ? (uint8_t)x.v : E_VALUE;
    return 0;
}

static int cmp_text(value a, value b)
{
    const uint8_t *p = str_p(a.v), *q = str_p(b.v);
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
    return boolean(c);
}

/* Writes a number as & joins it: no separators, no trailing zeros. */
static uint32_t plain_num(i64 v, uint8_t *out)
{
    uint32_t frac, n = 0, k = 4;
    u64 ip = divs(mag(v), SCALE, &frac);
    uint8_t f[4];
    if (v < 0)
        out[n++] = '-';
    n += put_int(ip, 0, out + n);
    for (uint32_t i = 0; i < 4; i++) {
        f[3 - i] = (uint8_t)('0' + frac % 10);
        frac /= 10;
    }
    while (k && f[k - 1] == '0')
        k--;
    if (k) {
        out[n++] = '.';
        for (uint32_t i = 0; i < k; i++)
            out[n++] = f[i];
    }
    return n;
}

/* a & b: the text of each, joined in the strings made this recalculation
 * (at most 255 bytes). */
static value concat(value a, value b)
{
    value two[2] = { a, b };
    uint32_t start = made_used, n = 0;
    for (uint32_t k = 0; k < 2; k++) {
        value x = two[k];
        uint8_t buf[40];
        const uint8_t *p = buf;
        uint32_t len = 0;
        if (x.k == K_ERR)
            return x;
        if (x.k == K_RANGE)
            return err(E_VALUE);
        if (x.k == K_STR) {
            p = str_p(x.v);
            len = STR_LEN(x.v);
        } else if (x.k == K_NUM)
            len = plain_num(x.v, buf);
        else if (x.k == K_BOOL) {
            p = (const uint8_t *)(x.v ? "TRUE" : "FALSE");
            len = x.v ? 4 : 5;
        }
        if (n + len > 255 || start + n + len > MADE_MAX)
            return err(E_VALUE);
        memcpy(made + start + n, p, len);
        n += len;
        work += (int32_t)len / 4 + 4;
    }
    made_used = start + n;
    return (value){ K_STR, STR(start, n) | STR_MADE };
}

static value power(i64 x, i64 y)
{
    uint32_t frac;
    u64 m = divs(mag(y), SCALE, &frac);
    i64 r = SCALE;
    if (frac || m > 64)
        return err(E_NUM);
    while (m--) {
        work += 20;
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

/* SUM, AVERAGE, MIN, MAX, COUNT, COUNTA, AND, OR over the n values at a.
 * In ranges, cells that are not numbers are skipped (AND and OR take
 * TRUE and FALSE too; COUNTA counts every cell that is not empty). */
static value aggregate(uint8_t f, value *a, uint32_t n)
{
    i64 sum = 0, best = 0, count = 0;
    uint32_t i, zeros = 0;
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
                work += 3;
                if (x.k == K_RANGE)
                    y = cell_value(idx(r, c));
                if (f == FN_COUNTA) {
                    if (y.k != K_EMPTY)
                        count += SCALE;
                    continue;
                }
                if (y.k == K_EMPTY)
                    continue;
                if (y.k == K_ERR) {
                    if (f == FN_COUNT)
                        continue;
                    return y;
                }
                if (x.k == K_RANGE) {
                    if (y.k != K_NUM && !(y.k == K_BOOL && f >= FN_AND))
                        continue;
                } else if (y.k == K_STR) {
                    if (f == FN_COUNT)
                        continue;
                    return err(E_VALUE);
                }
                if (!count || (f == FN_MIN && y.v < best) || (f == FN_MAX && y.v > best))
                    best = y.v;
                count += SCALE;
                zeros += !y.v;
                if (!add_fix(sum, y.v, &sum))
                    return err(E_NUM);
            }
    }
    switch (f) {
    case FN_SUM: return num(sum);
    case FN_COUNT:
    case FN_COUNTA: return num(count);
    case FN_AND:
    case FN_OR:
        if (!count)
            return err(E_VALUE);
        return boolean(f == FN_AND ? !zeros : (u64)zeros * SCALE < (u64)count);
    case FN_MIN:
    case FN_MAX: return num(count ? best : 0);
    default:
        if (!count)
            return err(E_DIV);
        work += 120;
        return div_fix(sum, count, &sum) ? num(sum) : err(E_NUM);
    }
}

/* Computes formula cell i (every cell it refers to is computed). */
static value evaluate(uint16_t i)
{
    value st[STACK_MAX];
    uint32_t sp = 0, pc = 0, n;
    const uint8_t *p = code_of(i, &n);
    work += 8;
    if (c_flag[i] & F_BAD)
        return err((uint8_t)c_val[i]);
    while (pc < n) {
        uint8_t op = p[pc], e = 0;
        value a, b;
        work += 2;
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
        case OP_BOOL: st[sp++] = boolean(p[pc + 1]); break;
        case OP_CAT:
            b = st[--sp];
            st[sp - 1] = concat(st[sp - 1], b);
            break;
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
            if (f <= FN_LISTS)
                r = aggregate(f, args, k);
            else if (f == FN_IF) {
                a = args[0];
                if (a.k == K_ERR)
                    r = a;
                else if (a.k == K_STR || a.k == K_RANGE)
                    r = err(E_VALUE);
                else
                    r = a.v ? args[1] : k == 3 ? args[2] : boolean(0);
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
                else if (f == FN_NOT)
                    r = boolean(!x);
                else if (f == FN_INT) {
                    /* Down to the whole number below. */
                    uint32_t frac;
                    i64 w = (i64)divs(mag(x), SCALE, &frac) * SCALE;
                    r = num(x < 0 ? -w - (frac ? SCALE : 0) : w);
                } else
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
                    work += 20;
                    if (!mul_fix(x, y, &r))
                        e = E_NUM;
                    break;
                case OP_DIV:
                    work += 120;
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
    if (st[0].k == K_EMPTY)
        return num(0); /* =A1 of an empty A1 shows 0 */
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
    uint32_t r, i;
    for (r = 0; r < NROWS; r++)
        if (row_f[r])
            for (i = r * NCOLS; i < r * NCOLS + NCOLS; i++)
                if (c_flag[i] & F_FORMULA)
                    c_state[i] = S_DIRTY;
    rsp = rscan = 0;
    recalcing = 1;
    dirty_all = 0;
    made_used = 0;
}

static void set_result(uint16_t i, value v)
{
    c_state[i] = S_CLEAN;
    if (c_kind[i] == v.k && c_val[i] == v.v && v.k != K_STR)
        return; /* shown as it was */
    work += 48; /* formatting the value */
    c_kind[i] = v.k;
    c_val[i] = v.v;
    update_disp(i);
}

/* Cell d is needed: 0 if it is computed, 1 if it was pushed, 2 if it closed
 * a cycle (the cells of the cycle are #CYCLE and off the stack). */
static int need(uint16_t d)
{
    if (!(c_flag[d] & F_FORMULA) || c_state[d] == S_CLEAN)
        return 0;
    if (c_state[d] == S_DIRTY) {
        work += 4;
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
                if (work >= WORK_PER_FRAME)
                    return;
                if (!(rscan % NCOLS) && !row_f[rscan / NCOLS])
                    rscan += NCOLS;
                else
                    rscan++;
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
            work += 2;
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
                    work += 2;
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
static uint32_t sr, sc, ar, ac;    /* the active cell (where typing goes) and the range's far corner */
/* Editing: the entry in ebuf, the text cursor at ecaret. An entry started
 * by typing is in "enter" mode (the arrows keep it and move, or point at a
 * cell for a formula); F2, a double-click or a click on the text is "edit"
 * mode (the arrows move the cursor). */
static uint8_t editing, emode;
enum { M_ENTER, M_EDIT };
static uint8_t ebuf[RAW_MAX];
static uint32_t elen, ecaret;
/* Pointing: a reference put into the formula by a click, a drag or the
 * arrows, at ebuf[pt_s, pt_e), for the range (pr0, pc0)-(pr1, pc1); the
 * next one replaces it while the cursor stays at its end. */
static uint8_t pt_on, pt_drag;
static uint32_t pt_s, pt_e, pr0, pc0, pr1, pc1;
static int32_t tab_col = -1; /* where a row of Tab entries started: Enter goes back to it */
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

static uint8_t help_on; /* F1: the keys in place of the chart */
/* The menu bar and the named sheets (below). */
#define NAME_MAX 24
static uint8_t has_term, menu_on, prompting;
static char pr_buf[NAME_MAX + 1];
static uint32_t pr_n;
static char rec_name[33] = "sheet";
static content_req list_req;
static uint32_t list_h;
static void menu_shapes(void);
static void listed(void);
static uint32_t append(char *out, uint32_t n, const char *w);
static int menus_shown(void) { return has_term && has_vec; }
static uint32_t vrows(void) { return chart_kind || help_on ? VROWS_CHART : VROWS_ALL; }
static uint32_t rmin(void) { return sr < ar ? sr : ar; }
static uint32_t rmax(void) { return sr < ar ? ar : sr; }
static uint32_t cmin(void) { return sc < ac ? sc : ac; }
static uint32_t cmax(void) { return sc < ac ? ac : sc; }

/* The geometry of the shown table, from locate (screen units): g_y[0] the
 * header's top, g_y[1 + i] the top of shown row i, g_y[1 + rows] the
 * table's bottom. */
static uint32_t g_y[VROWS_ALL + 2];
/* What a page document showed: the rows, the first row and column, the
 * selection. The overlay draws the one shown (vis_v), not the newest. */
static struct {
    uint32_t top, left, sr, sc, ar, ac, rows, pt, pr0, pc0, pr1, pc1;
} pend_v, vis_v;
static uint16_t loc_ids[VROWS_ALL + 2];
static uint32_t loc_n, loc_at;

static void default_geometry(void)
{
    /* The reference layout's: the entry line (4 + 18), 4 more above the
     * table, rows of 21. */
    for (uint32_t i = 0; i < VROWS_ALL + 2; i++)
        g_y[i] = 40 + 21 * i;
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

static const uint8_t ACC[3] = { 26, 100, 210 };
static const uint8_t PT[3] = { 214, 98, 0 }; /* the range being pointed at */

/* The text cursor: a bar in the accent colour. */
static void caret_mark(void)
{
    doc_anchor(0);
    doc_colour(ACC[0], ACC[1], ACC[2]);
    doc_font(DOC_SANS, DOC_BOLD);
    text("|");
    doc_font(DOC_SANS, 0);
    doc_colour(25, 25, 25);
}

/* p[from, to) with each character in its own anchor (base + its offset),
 * so that a press on it tells which one it is; the text cursor before
 * offset `at` (none if `at` is past `to`). */
static void anchored(const uint8_t *p, uint32_t from, uint32_t to, uint16_t base, uint32_t at)
{
    uint32_t k = from;
    for (;;) {
        if (k == at)
            caret_mark();
        if (k >= to)
            break;
        uint32_t q = k + 1;
        while (q < to && (p[q] & 0xC0) == 0x80)
            q++;
        doc_anchor((uint16_t)(base + k));
        doc_text(p + k, q - k);
        k = q;
    }
    doc_anchor(0);
}

/* The part of the entry that the cell being edited shows: as much as fits
 * around the cursor. */
static void edit_window(uint32_t room, uint32_t *from, uint32_t *to)
{
    uint32_t w = 0, s = ecaret, e = ecaret;
    if (est_width(ebuf, elen) > room)
        room -= 240; /* an ellipsis (1 em) at each end */
    while (s) {
        uint32_t p = s - 1, cw;
        while (p && (ebuf[p] & 0xC0) == 0x80)
            p--;
        cw = est_width(ebuf + p, s - p);
        if (w + cw > room)
            break;
        w += cw;
        s = p;
    }
    while (e < elen) {
        uint32_t q = e + 1, cw;
        while (q < elen && (ebuf[q] & 0xC0) == 0x80)
            q++;
        cw = est_width(ebuf + e, q - e);
        if (w + cw > room)
            break;
        w += cw;
        e = q;
    }
    *from = s;
    *to = e;
}

static int can_point(void);
static int whole_rows(void);
static int whole_cols(void);

static int pointed(uint32_t r, uint32_t c)
{
    uint32_t a = pr0 < pr1 ? pr0 : pr1, b = pr0 < pr1 ? pr1 : pr0, x = pc0 < pc1 ? pc0 : pc1, y = pc0 < pc1 ? pc1 : pc0;
    return editing && pt_on && r >= a && r <= b && c >= x && c <= y;
}

static void build_doc(void)
{
    uint8_t align[8];
    char nm[8];
    uint32_t n, i, k, rows = vrows();
    uint16_t sel = idx(sr, sc);
    columns_align();
    doc_begin(pdoc, sizeof pdoc - 1);
    /* The content line: the selection's name, then the selected cell's
     * content as typed (a formula, not its value), or the entry being
     * typed with its text cursor. Every character is an anchor: a press
     * on one puts the cursor there. */
    doc_colour(25, 25, 25);
    doc_size(14);
    /* Below the menu bar (16 units), where there is one. */
    doc_paragraph(DOC_START, 6, menus_shown() ? 18 : 4, DOC_NOWRAP | DOC_ELLIPSIS);
    doc_font(DOC_SANS, DOC_BOLD);
    range_name(nm, &n, rmin(), cmin(), rmax(), cmax());
    if (editing)
        n = name_of(sr, sc, (uint8_t *)nm);
    if (prompting) {
        text("Save as");
        doc_font(DOC_SANS, 0);
        text("   ");
        doc_text(pr_buf, pr_n);
        caret_mark();
        doc_colour(120, 120, 120);
        text("   Enter saves the sheet under this name \xC2\xB7 Esc cancels");
    } else
        doc_text(nm, n);
    doc_font(DOC_SANS, 0);
    text("   ");
    if (prompting) {
    } else if (editing) {
        /* As much as the line holds around the cursor (estimated at size
         * 12: the line's width less the name, scaled from 14). */
        uint32_t s, e;
        edit_window(LINE_WIDTH, &s, &e);
        if (s)
            text("\xE2\x80\xA6");
        anchored(ebuf, s, e, A_LINE, ecaret);
        if (e < elen)
            text("\xE2\x80\xA6");
    } else if (c_rlen[sel]) {
        anchored(pool + c_off[sel], 0, c_rlen[sel], A_LINE, RAW_MAX + 1);
        if (c_flag[sel] & F_FORMULA) {
            /* Then its value, or what its error means. */
            uint8_t b[40];
            text("   ");
            if (c_kind[sel] == K_ERR) {
                doc_colour(200, 40, 40);
                text(err_text[c_val[sel]]);
                text(" ");
                text(err_why[c_val[sel]]);
            } else {
                doc_colour(120, 120, 120);
                text("= ");
                if (c_kind[sel] == K_NUM)
                    doc_text(b, format_num(c_val[sel], 0, 0, 0, b));
                else if (c_kind[sel] == K_STR)
                    doc_text(str_p(c_val[sel]), STR_LEN(c_val[sel]));
                else
                    doc_text(c_disp[sel], c_dlen[sel]);
            }
        }
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
            if (c == sel && editing) {
                /* The entry being typed, in its cell too. */
                uint32_t s, e;
                edit_window(CELL_WIDTH - 60, &s, &e);
                doc_colour(25, 25, 25);
                if (s)
                    text("\xE2\x80\xA6");
                anchored(ebuf, s, e, A_CELL, ecaret);
                if (e < elen)
                    text("\xE2\x80\xA6");
                continue;
            }
            int here = c == sel && !has_vec, pt = !has_vec && pointed(r, left + k);
            if (here || pt) {
                doc_colour(pt ? PT[0] : ACC[0], pt ? PT[1] : ACC[1], pt ? PT[2] : ACC[2]);
                doc_font(DOC_SANS, DOC_BOLD);
            }
            if (c_dlen[c]) {
                if (!here && !pt) {
                    int bad = c_flag[c] & F_FORMULA && c_kind[c] == K_ERR;
                    doc_colour(bad ? 200 : 25, bad ? 40 : 25, bad ? 40 : 25);
                }
                doc_text(c_disp[c], c_dlen[c]);
            } else if (here || pt)
                text("\xC2\xB7");
            if (here || pt)
                doc_font(DOC_SANS, 0);
        }
    }
    padding_row(8, A_BOTTOM);
    doc_table_end();
    if (help_on) {
        /* Every key, two to a line. */
        static const char *const keys[] = {
            "Type: fill a cell \xC2\xB7 = starts a formula: click or drag over cells to put them in",
            "F2 or double-click: edit \xC2\xB7 Enter, Tab: keep \xC2\xB7 Esc: cancel, or forget a copy \xC2\xB7 F4: $A$1",
            "Ctrl+C, X, V: copy, cut, paste \xC2\xB7 Ctrl+Z, Y: undo, redo \xC2\xB7 Delete: clear \xC2\xB7 Ctrl+D, R: fill",
            "Ctrl+Plus, Minus: insert, delete the selected rows (columns: select them by their letters)",
            "Data menu (F10, then Right): sort the selection by the active cell's column \xC2\xB7 Ctrl+A: all",
            "Ctrl+Shift+1, 5, ~: number, percent, general \xC2\xB7 Ctrl+period, comma: more, fewer decimals",
            "Ctrl+G, L: bar, line chart \xC2\xB7 Ctrl+S: save \xC2\xB7 Ctrl+P: publish \xC2\xB7 Ctrl+N twice: new \xC2\xB7 F1: close",
        };
        doc_size(10);
        doc_colour(60, 60, 60);
        for (k = 0; k < sizeof keys / sizeof keys[0]; k++) {
            doc_paragraph(DOC_START, k ? 1 : 8, 0, DOC_NOWRAP | DOC_ELLIPSIS);
            text(keys[k]);
        }
    } else if (chart_kind)
        chart_doc();
    /* The status line: while an entry is typed, its mode and keys; else
     * what happened last and how to begin an entry. */
    doc_paragraph(DOC_START, 6, 6, DOC_NOWRAP | DOC_ELLIPSIS);
    doc_size(11);
    doc_font(DOC_SANS, 0);
    if (recalcing) {
        doc_colour(180, 100, 0);
        text("Calculating\xE2\x80\xA6  ");
    }
    doc_colour(90, 90, 90);
    if (editing) {
        int point = pt_on || (emode == M_ENTER && can_point());
        doc_font(DOC_SANS, DOC_BOLD);
        if (point)
            doc_colour(PT[0], PT[1], PT[2]);
        text(point ? "Point  " : emode == M_EDIT ? "Edit  " : "Enter  ");
        doc_font(DOC_SANS, 0);
        doc_colour(90, 90, 90);
        if (point)
            text("Click a cell or drag over a range (or use the arrows) to put it in the formula \xC2\xB7 Enter keeps \xC2\xB7 Esc cancels");
        else if (emode == M_EDIT)
            text("\xE2\x86\x90 \xE2\x86\x92 Home End move the cursor \xC2\xB7 Enter keeps \xC2\xB7 Esc cancels \xC2\xB7 F4 makes $A$1 \xC2\xB7 F2 points with the arrows");
        else
            text("Enter keeps \xC2\xB7 Tab or an arrow keeps and moves \xC2\xB7 F2 moves the cursor in the text \xC2\xB7 Esc cancels");
    } else {
        if (*status) {
            doc_colour(26, 100, 210);
            text(status);
            text("   ");
            doc_colour(130, 130, 130);
        }
        if (rmin() != rmax() || cmin() != cmax())
            text(whole_rows() || whole_cols() ? "Ctrl+Plus inserts, Ctrl+Minus deletes them \xC2\xB7 the Data menu sorts \xC2\xB7 F1 all keys"
                                              : "the Data menu sorts it \xC2\xB7 Ctrl+Shift+1/5 number/percent, Ctrl+period/comma decimals \xC2\xB7 "
                                                "Ctrl+G/L chart \xC2\xB7 F1 all keys");
        else
            text("Type to fill a cell, = to start a formula \xC2\xB7 F2 or double-click edits \xC2\xB7 F1 all keys \xC2\xB7 Ctrl+Z undo \xC2\xB7 "
                 "Ctrl+C/X/V copy, cut, paste \xC2\xB7 Ctrl+S save");
    }
    doc_end();
    page_submit(pdoc, doc_length(pdoc));
    /* Where the rows are, in this layout. */
    loc_n = 0;
    loc_ids[loc_n++] = A_HEAD;
    for (i = 0; i < rows; i++)
        loc_ids[loc_n++] = (uint16_t)(A_ROW + i);
    loc_ids[loc_n++] = A_BOTTOM;
    loc_at = 0;
    /* The overlay follows when this document is shown: the answers to its
     * locates come with its layout. Drawn now, it would show the new
     * selection over the old page for the frames the host takes. */
    pend_v.top = top;
    pend_v.left = left;
    pend_v.sr = sr;
    pend_v.sc = sc;
    pend_v.ar = ar;
    pend_v.ac = ac;
    pend_v.rows = rows;
    pend_v.pt = editing && pt_on;
    pend_v.pr0 = pr0;
    pend_v.pc0 = pc0;
    pend_v.pr1 = pr1;
    pend_v.pc1 = pc1;
}

static void located(const page_place *p)
{
    uint32_t y = p->y, rows;
    if (!p->height)
        return;
    /* Answers come only for the document submitted last, once laid out. */
    vis_v = pend_v;
    rows = vis_v.rows;
    if (p->id == A_HEAD)
        g_y[0] = y - 3;
    else if (p->id == A_BOTTOM)
        g_y[rows + 1] = y - 3;
    else if (p->id >= A_ROW && p->id < A_ROW + rows)
        g_y[1 + p->id - A_ROW] = y - 3;
    vec_dirty = 1;
}

/* --- The vector overlay --------------------------------------------------- */

static uint8_t vlist[4096];
static uint8_t clip_on, clip_cut, cl_r0, cl_c0, cl_r1, cl_c1; /* what Ctrl+C or Ctrl+X took */

static void vrect(int32_t x0, int32_t y0, int32_t x1, int32_t y1)
{
    vec_move((int16_t)(x0 * 16), (int16_t)(y0 * 16));
    vec_rect((int16_t)(x1 * 16), (int16_t)(y1 * 16));
}

static void build_vec(void)
{
    /* The selection as the page shown has it. */
    uint32_t rows = vis_v.rows, k, top = vis_v.top, left = vis_v.left, sr = vis_v.sr, sc = vis_v.sc;
    uint32_t rmin = sr < vis_v.ar ? sr : vis_v.ar, rmax = sr < vis_v.ar ? vis_v.ar : sr;
    uint32_t cmin = sc < vis_v.ac ? sc : vis_v.ac, cmax = sc < vis_v.ac ? vis_v.ac : sc;
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
    int32_t r0 = (int32_t)rmin - (int32_t)top, r1 = (int32_t)rmax - (int32_t)top;
    int32_t c0 = (int32_t)cmin - (int32_t)left, c1 = (int32_t)cmax - (int32_t)left;
    if (r0 < 0)
        r0 = 0;
    if (r1 >= (int32_t)rows)
        r1 = (int32_t)rows - 1;
    if (c0 < 0)
        c0 = 0;
    if (c1 >= VCOLS)
        c1 = VCOLS - 1;
    if ((rmin != rmax || cmin != cmax) && r0 <= r1 && c0 <= c1) {
        vec_colour(26, 100, 210, 36);
        vrect((c0 + 1) * COLW, (int32_t)g_y[1 + r0], (c1 + 2) * COLW, (int32_t)g_y[2 + r1]);
    }
    if (clip_on) {
        /* What was copied: a dashed edge, until Esc. */
        int32_t a = (int32_t)cl_r0 - (int32_t)top, b = (int32_t)cl_r1 - (int32_t)top;
        int32_t x = (int32_t)cl_c0 - (int32_t)left, y = (int32_t)cl_c1 - (int32_t)left;
        a = a < 0 ? 0 : a;
        b = b >= (int32_t)rows ? (int32_t)rows - 1 : b;
        x = x < 0 ? 0 : x;
        y = y >= VCOLS ? VCOLS - 1 : y;
        if (a <= b && x <= y) {
            int32_t x0 = (x + 1) * COLW * 16, x1 = (y + 2) * COLW * 16, ya = (int32_t)g_y[1 + a] * 16 + 24, yb = (int32_t)g_y[2 + b] * 16 - 24;
            vec_colour(30, 150, 70, 255);
            vec_width(24);
            for (int32_t t = x0 + 24; t < x1 - 24; t += 160) {
                int32_t e = t + 96 < x1 - 24 ? t + 96 : x1 - 24;
                vec_move((int16_t)t, (int16_t)ya);
                vec_line((int16_t)e, (int16_t)ya);
                vec_move((int16_t)t, (int16_t)yb);
                vec_line((int16_t)e, (int16_t)yb);
            }
            for (int32_t t = ya; t < yb; t += 160) {
                int32_t e = t + 96 < yb ? t + 96 : yb;
                vec_move((int16_t)(x0 + 24), (int16_t)t);
                vec_line((int16_t)(x0 + 24), (int16_t)e);
                vec_move((int16_t)(x1 - 24), (int16_t)t);
                vec_line((int16_t)(x1 - 24), (int16_t)e);
            }
        }
    }
    if (vis_v.pt) {
        /* The range a formula is pointing at: shaded, with an orange edge. */
        int32_t a = (int32_t)(vis_v.pr0 < vis_v.pr1 ? vis_v.pr0 : vis_v.pr1) - (int32_t)top;
        int32_t b = (int32_t)(vis_v.pr0 < vis_v.pr1 ? vis_v.pr1 : vis_v.pr0) - (int32_t)top;
        int32_t x = (int32_t)(vis_v.pc0 < vis_v.pc1 ? vis_v.pc0 : vis_v.pc1) - (int32_t)left;
        int32_t y = (int32_t)(vis_v.pc0 < vis_v.pc1 ? vis_v.pc1 : vis_v.pc0) - (int32_t)left;
        a = a < 0 ? 0 : a;
        b = b >= (int32_t)rows ? (int32_t)rows - 1 : b;
        x = x < 0 ? 0 : x;
        y = y >= VCOLS ? VCOLS - 1 : y;
        if (a <= b && x <= y) {
            int32_t x0 = (x + 1) * COLW, x1 = (y + 2) * COLW, ya = (int32_t)g_y[1 + a], yb = (int32_t)g_y[2 + b];
            vec_colour(214, 98, 0, 40);
            vrect(x0, ya, x1, yb);
            vec_colour(214, 98, 0, 255);
            vec_width(24);
            vec_move((int16_t)(x0 * 16), (int16_t)(ya * 16));
            vec_line((int16_t)(x1 * 16), (int16_t)(ya * 16));
            vec_line((int16_t)(x1 * 16), (int16_t)(yb * 16));
            vec_line((int16_t)(x0 * 16), (int16_t)(yb * 16));
            vec_line((int16_t)(x0 * 16), (int16_t)(ya * 16));
        }
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
    if (menus_shown())
        menu_shapes();
    vec_end();
    vec_submit(vlist, vec_length(vlist));
}

/* --- Moving --------------------------------------------------------------- */

/* Scrolls so that cell (r, c) is shown. */
static void show_cell(uint32_t r, uint32_t c)
{
    uint32_t rows = vrows();
    if (r < top)
        top = r;
    if (r >= top + rows)
        top = r - rows + 1;
    if (c < left)
        left = c;
    if (c >= left + VCOLS)
        left = c - VCOLS + 1;
    doc_dirty = 1;
}

static void show_selection(void) { show_cell(sr, sc); }

static uint32_t clamp(int32_t v, uint32_t n) { return v < 0 ? 0 : v >= (int32_t)n ? n - 1 : (uint32_t)v; }

/* Selects cell (r, c); extending, moves the range's far corner there and
 * keeps the active cell (where typing goes), as spreadsheets do. */
static void move_to(int32_t r, int32_t c, int extend)
{
    ar = clamp(r, NROWS);
    ac = clamp(c, NCOLS);
    if (!extend) {
        sr = ar;
        sc = ac;
    }
    new_armed = 0;
    tab_col = -1;
    show_cell(ar, ac);
}

/* An arrow: from the active cell, or (extending) from the far corner. */
static void step(int32_t dr, int32_t dc, int extend)
{
    move_to((int32_t)(extend ? ar : sr) + dr, (int32_t)(extend ? ac : sc) + dc, extend);
}

/* Enter: down (Shift: up); after a row of entries kept with Tab, to the
 * column the row started in, as spreadsheets do. */
static void enter_move(int shift)
{
    int32_t c = tab_col >= 0 && !shift ? tab_col : (int32_t)sc;
    move_to((int32_t)sr + (shift ? -1 : 1), c, 0);
}

static void tab_move(int shift)
{
    int32_t t = tab_col >= 0 ? tab_col : (int32_t)sc;
    move_to((int32_t)sr, (int32_t)sc + (shift ? -1 : 1), 0);
    tab_col = t;
}

static int filled(int32_t r, int32_t c) { return r >= 0 && r < NROWS && c >= 0 && c < NCOLS && c_rlen[idx((uint32_t)r, (uint32_t)c)]; }

/* Ctrl+arrow: to the edge of the block of filled cells, or to the next
 * filled cell, or to the sheet's edge. */
static void jump(int dr, int dc, int extend)
{
    int32_t r = (int32_t)(extend ? ar : sr), c = (int32_t)(extend ? ac : sc);
    if (filled(r, c) && filled(r + dr, c + dc))
        while (filled(r + dr, c + dc)) {
            r += dr;
            c += dc;
        }
    else {
        while (r + dr >= 0 && r + dr < NROWS && c + dc >= 0 && c + dc < NCOLS) {
            r += dr;
            c += dc;
            if (filled(r, c))
                break;
        }
    }
    move_to(r, c, extend);
}

/* --- Undo ----------------------------------------------------------------- */

/* Every change the user makes to cells (an entry kept, a paste, a fill, a
 * clear, a move) is a group in this log: a mark with the selection, then
 * one record per cell changed with its text and number format before and
 * after (cell, 2 bytes; formats before and after; length and text before;
 * length and text after). Ctrl+Z puts back the cell as it was before,
 * Ctrl+Y as it was after; a new change drops what
 * was undone. Records are padded to 4 bytes and end in their size, so the
 * log is walked both ways; when it is full the oldest groups go. A single
 * change too large for it cannot be undone (and the log is emptied). */
#define ULOG 32768
#define MARK 20 /* a group's mark, in bytes */
static uint8_t ulog[ULOG] __attribute__((aligned(4)));
static uint32_t u_cur, u_end, u_group;
static uint8_t u_off;

static int is_mark(uint32_t at) { return ulog[at] == 0xFF && ulog[at + 1] == 0xFF; }

static uint32_t rec_size(uint32_t at)
{
    uint32_t ol, nl;
    if (is_mark(at))
        return MARK;
    ol = ulog[at + 4];
    nl = ulog[at + 5 + ol];
    return (4 + 1 + ol + 1 + nl + 2 + 3) & ~3u;
}

/* Room for `size` more bytes, dropping the oldest groups (down to half the
 * log, so that it is seldom moved), never the one being logged. */
static int make_room(uint32_t size)
{
    uint32_t at = 0, cut = 0, *d = (uint32_t *)ulog, *s, k;
    if (u_end + size <= ULOG)
        return 1;
    while (at < u_group) {
        at += rec_size(at);
        if (at >= u_group || is_mark(at)) {
            cut = at;
            if (u_end - cut + size <= ULOG / 2)
                break;
        }
    }
    if (!cut || u_end - cut + size > ULOG) {
        u_off = 1;
        u_cur = u_end = u_group = 0;
        return 0;
    }
    s = (uint32_t *)(ulog + cut);
    for (k = 0; k < (u_end - cut) / 4; k++)
        d[k] = s[k];
    u_end -= cut;
    u_group -= cut;
    u_cur = u_end;
    return 1;
}

/* The chart before a change (kind and range), when the change moves it
 * (an insertion or deletion); the mark keeps it and the chart after. */
static uint8_t chart_before[5], chart_set;

/* A new group: the mark (FFFFh, the selection to show when it is undone,
 * the chart before and after the change, the size). */
static void ubegin(void)
{
    u_end = u_cur;
    u_off = 0;
    u_group = u_end;
    if (!make_room(MARK))
        return;
    uint8_t *w = ulog + u_end;
    w[0] = w[1] = 0xFF;
    w[2] = (uint8_t)sr;
    w[3] = (uint8_t)sc;
    w[4] = (uint8_t)ar;
    w[5] = (uint8_t)ac;
    w[11] = chart_kind;
    w[12] = ch_r0;
    w[13] = ch_c0;
    w[14] = ch_r1;
    w[15] = ch_c1;
    if (chart_set)
        memcpy(w + 6, chart_before, 5);
    else
        memcpy(w + 6, w + 11, 5);
    chart_set = 0;
    w[16] = w[17] = 0;
    w[MARK - 2] = MARK;
    w[MARK - 1] = 0;
    u_end += MARK;
    u_cur = u_end;
}

/* Cell i is about to become p[0, n) in format fmt. */
static void ulog_put(uint16_t i, const uint8_t *p, uint32_t n, uint8_t fmt)
{
    uint32_t ol = c_rlen[i], size;
    const uint8_t *o = pool + c_off[i];
    if (u_off)
        return;
    if (n > RAW_MAX)
        n = RAW_MAX;
    if (ol == n && !memcmp(o, p, n) && fmt == c_fmt[i])
        return;
    size = (4 + 1 + ol + 1 + n + 2 + 3) & ~3u;
    if (!make_room(size))
        return;
    uint8_t *w = ulog + u_end;
    w[0] = (uint8_t)i;
    w[1] = (uint8_t)(i >> 8);
    w[2] = c_fmt[i];
    w[3] = fmt;
    w[4] = (uint8_t)ol;
    memcpy(w + 5, o, ol);
    w[5 + ol] = (uint8_t)n;
    memcpy(w + 6 + ol, p, n);
    w[size - 2] = (uint8_t)size;
    w[size - 1] = (uint8_t)(size >> 8);
    u_end += size;
    u_cur = u_end;
}

/* Sets cell i's text and format (unchanged text is not compiled again). */
static int set_cell_fmt(uint16_t i, const uint8_t *p, uint32_t n, uint8_t fmt)
{
    int same = c_rlen[i] == n && !memcmp(pool + c_off[i], p, n);
    if (c_fmt[i] != fmt) {
        c_fmt[i] = fmt;
        update_disp(i);
        doc_dirty = 1;
    }
    return same || set_cell(i, p, n);
}

/* Changes the cell as the user asked: logged for undo. */
static int put_cell(uint16_t i, const uint8_t *p, uint32_t n, uint8_t fmt)
{
    ulog_put(i, p, n, fmt);
    return set_cell_fmt(i, p, n, fmt);
}

/* --- Editing -------------------------------------------------------------- */

static void start_edit(int keep, uint8_t mode)
{
    uint16_t i = idx(sr, sc);
    editing = 1;
    emode = mode;
    pt_on = pt_drag = 0;
    elen = keep ? c_rlen[i] : 0;
    memcpy(ebuf, pool + c_off[i], elen);
    ecaret = elen;
    ar = sr;
    ac = sc;
    show_selection();
    say("");
}

static void cancel_edit(void)
{
    editing = pt_on = pt_drag = 0;
    doc_dirty = 1;
}

/* A formula as spreadsheets keep it: names and references in capitals
 * (outside text in quotes), and the brackets left open closed. */
static void tidy_formula(void)
{
    uint32_t k, open = 0;
    int q = 0;
    for (k = 1; k < elen; k++) {
        uint8_t ch = ebuf[k];
        if (ch == '"')
            q = !q;
        else if (!q) {
            ebuf[k] = (uint8_t)upper(ch);
            if (ch == '(')
                open++;
            else if (ch == ')' && open)
                open--;
        }
    }
    while (!q && open-- && elen < RAW_MAX)
        ebuf[elen++] = ')';
}

static void commit(void)
{
    uint16_t i = idx(sr, sc);
    editing = pt_on = pt_drag = 0;
    doc_dirty = 1;
    if (elen && ebuf[0] == '=')
        tidy_formula();
    if (c_rlen[i] == elen && !memcmp(pool + c_off[i], ebuf, elen))
        return;
    ubegin();
    if (!put_cell(i, ebuf, elen, c_fmt[i]))
        say("The sheet is full: no room for that entry");
    else if (c_flag[i] & F_BAD)
        say(c_val[i] == E_NAME ? "#NAME?: no function of that name (F2 edits the formula)" : c_val[i] == E_REF ? "#REF!: a reference outside A1:Z99" : "#ERROR!: the formula cannot be read (F2 edits it)");
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
    pt_on = 0;
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

static int one_of(uint8_t ch, const char *set)
{
    while (*set)
        if (ch == (uint8_t)*set++)
            return 1;
    return 0;
}

/* The text cursor is where a reference may go in a formula: after "=",
 * an operator, "(", "," or ":" (outside quotes), and before the end, a
 * space, an operator or ")"; or right after the reference just pointed at. */
static int can_point(void)
{
    uint32_t k = ecaret, j;
    int q = 0;
    if (!editing || !elen || ebuf[0] != '=')
        return 0;
    if (pt_on && ecaret == pt_e)
        return 1;
    for (j = 0; j < ecaret; j++)
        if (ebuf[j] == '"')
            q = !q;
    if (q)
        return 0;
    while (k > 1 && ebuf[k - 1] == ' ')
        k--;
    if (!k || !one_of(ebuf[k - 1], "=+-*/^&<>(,:"))
        return 0;
    k = ecaret;
    while (k < elen && ebuf[k] == ' ')
        k++;
    return k == elen || one_of(ebuf[k], ")+-*/^&<>=,:%");
}

/* Puts the reference to (a_r, a_c):(b_r, b_c) at the cursor, or in place
 * of the one pointed at before. */
static void point_set(uint32_t a_r, uint32_t a_c, uint32_t b_r, uint32_t b_c)
{
    char t[8];
    uint32_t n, old;
    pr0 = a_r;
    pc0 = a_c;
    pr1 = b_r;
    pc1 = b_c;
    range_name(t, &n, a_r < b_r ? a_r : b_r, a_c < b_c ? a_c : b_c, a_r < b_r ? b_r : a_r, a_c < b_c ? b_c : a_c);
    if (!pt_on)
        pt_s = pt_e = ecaret;
    old = pt_e - pt_s;
    if (elen - old + n > RAW_MAX)
        return;
    memmove(ebuf + pt_s + n, ebuf + pt_e, elen - pt_e);
    memcpy(ebuf + pt_s, t, n);
    elen = elen - old + n;
    pt_e = pt_s + n;
    ecaret = pt_e;
    pt_on = 1;
    show_cell(b_r, b_c);
}

/* An arrow while pointing: the reference moves (Shift: grows) from the
 * cell being edited, or from the one pointed at. */
static void point_key(int dr, int dc, int extend)
{
    if (!pt_on) {
        pr0 = pr1 = sr;
        pc0 = pc1 = sc;
    }
    uint32_t r = clamp((int32_t)pr1 + dr, NROWS), c = clamp((int32_t)pc1 + dc, NCOLS);
    if (extend)
        point_set(pr0, pc0, r, c);
    else
        point_set(r, c, r, c);
}

/* F4: the reference at the cursor goes A1, $A$1, A$1, $A1 and back. */
static void toggle_dollar(void)
{
    uint32_t s = ecaret, j, cabs = 0, rabs = 0, n = 0, d0;
    uint8_t t[8];
    while (s > 1 && (is_alpha(ebuf[s - 1]) || is_digit(ebuf[s - 1]) || ebuf[s - 1] == '$'))
        s--;
    j = s;
    if (j < elen && ebuf[j] == '$') {
        cabs = 1;
        j++;
    }
    if (j >= elen || !is_alpha(ebuf[j]) || (j > 1 && is_alpha(ebuf[j - 1])))
        return;
    uint8_t col = ebuf[j++];
    if (j < elen && ebuf[j] == '$') {
        rabs = 1;
        j++;
    }
    if (j >= elen || !is_digit(ebuf[j]))
        return;
    d0 = j;
    while (j < elen && is_digit(ebuf[j]) && j - d0 < 2)
        j++;
    if (j < ecaret || (j < elen && (is_alpha(ebuf[j]) || is_digit(ebuf[j]))))
        return;
    /* (0,0) -> (1,1) -> (0,1) -> (1,0) -> (0,0) */
    if (!cabs && !rabs)
        cabs = rabs = 1;
    else if (cabs && rabs)
        cabs = 0;
    else if (rabs) {
        cabs = 1;
        rabs = 0;
    } else
        cabs = 0;
    if (cabs)
        t[n++] = '$';
    t[n++] = col;
    if (rabs)
        t[n++] = '$';
    for (uint32_t k = d0; k < j; k++)
        t[n++] = ebuf[k];
    if (elen - (j - s) + n > RAW_MAX)
        return;
    memmove(ebuf + s + n, ebuf + j, elen - j);
    memcpy(ebuf + s, t, n);
    elen = elen - (j - s) + n;
    ecaret = s + n;
    pt_on = 0;
    doc_dirty = 1;
}

static void edit_key(uint8_t k, int shift, int ctrl)
{
    int dr = 0, dc = 0;
    if (ctrl) {
        if (k == KEY_HOME || k == KEY_END) {
            ecaret = k == KEY_HOME ? 0 : elen;
            pt_on = 0;
            doc_dirty = 1;
        } else if (k == KEY_Z)
            cancel_edit();
        return;
    }
    switch (k) {
    case KEY_ESCAPE: cancel_edit(); return;
    case KEY_ENTER:
        commit();
        enter_move(shift);
        return;
    case KEY_TAB:
        commit();
        tab_move(shift);
        return;
    case KEY_UP: dr = -1; break;
    case KEY_DOWN: dr = 1; break;
    case KEY_LEFT: dc = -1; break;
    case KEY_RIGHT: dc = 1; break;
    case KEY_HOME:
    case KEY_END:
        ecaret = k == KEY_HOME ? 0 : elen;
        pt_on = 0;
        doc_dirty = 1;
        return;
    case KEY_BACKSPACE:
        if (ecaret) {
            uint32_t p = prev_char(ecaret);
            memmove(ebuf + p, ebuf + ecaret, elen - ecaret);
            elen -= ecaret - p;
            ecaret = p;
        }
        pt_on = 0;
        doc_dirty = 1;
        return;
    case KEY_DELETE:
        if (ecaret < elen) {
            uint32_t q = next_char(ecaret);
            memmove(ebuf + ecaret, ebuf + q, elen - q);
            elen -= q - ecaret;
        }
        pt_on = 0;
        doc_dirty = 1;
        return;
    case KEY_F2:
        emode = emode == M_EDIT ? M_ENTER : M_EDIT;
        pt_on = 0;
        doc_dirty = 1;
        return;
    case KEY_F4: toggle_dollar(); return;
    default: return;
    }
    /* An arrow: points at a cell in a formula being entered, moves the
     * cursor in edit mode, else keeps the entry and moves. */
    if (emode == M_ENTER && can_point()) {
        point_key(dr, dc, shift);
        return;
    }
    if (emode == M_EDIT && dc) {
        ecaret = dc < 0 ? prev_char(ecaret) : next_char(ecaret);
        pt_on = 0;
        doc_dirty = 1;
        return;
    }
    commit();
    step(dr, dc, shift);
}

/* --- Copying, filling, clearing ------------------------------------------- */

/* Writes the name of cell (r, c) with $ where the reference is absolute. */
static uint32_t ref_text(uint32_t r, uint32_t c, int cabs, int rabs, uint8_t *out)
{
    uint32_t n = 0;
    if (cabs)
        out[n++] = '$';
    out[n++] = (uint8_t)('A' + c);
    if (rabs)
        out[n++] = '$';
    if (r + 1 >= 10)
        out[n++] = (uint8_t)('0' + (r + 1) / 10);
    out[n++] = (uint8_t)('0' + (r + 1) % 10);
    return n;
}

/* Copies a formula's text moved by (dr, dc): relative references move,
 * the parts after a $ stay; those that would leave the sheet become #REF!.
 * Returns the length (at most RAW_MAX). */
static uint32_t shift_refs(const uint8_t *p, uint32_t n, int32_t dr, int32_t dc, uint8_t *out)
{
    uint32_t i = 0, o = 0;
    int in_str = 0;
    if (!n || p[0] != '=' || (!dr && !dc)) {
        memcpy(out, p, n);
        return n;
    }
    while (i < n && o < RAW_MAX) {
        uint8_t ch = p[i];
        if (ch == '"')
            in_str = !in_str;
        if (!in_str && (is_alpha(ch) || ch == '$') && (i == 0 || !(is_alpha(p[i - 1]) || is_digit(p[i - 1]) || p[i - 1] == '$'))) {
            uint32_t j = i, row = 0, cabs = 0, rabs = 0, col;
            if (p[j] == '$') {
                cabs = 1;
                j++;
            }
            if (j < n && is_alpha(p[j])) {
                col = (uint32_t)(upper(p[j]) - 'A');
                j++;
                if (j < n && p[j] == '$') {
                    rabs = 1;
                    j++;
                }
                if (j < n && is_digit(p[j])) {
                    while (j < n && is_digit(p[j])) {
                        if (row < 1000)
                            row = row * 10 + (p[j] - '0');
                        j++;
                    }
                    if (j >= n || (p[j] != '(' && !is_alpha(p[j]))) {
                        int32_t r = (int32_t)row - 1 + (rabs ? 0 : dr), c = (int32_t)col + (cabs ? 0 : dc);
                        uint8_t nm[6];
                        uint32_t k, len;
                        const uint8_t *w;
                        if (r < 0 || r >= NROWS || c < 0 || c >= NCOLS || row < 1 || row > NROWS) {
                            w = (const uint8_t *)"#REF!";
                            len = 5;
                        } else {
                            len = ref_text((uint32_t)r, (uint32_t)c, (int)cabs, (int)rabs, nm);
                            w = nm;
                        }
                        for (k = 0; k < len && o < RAW_MAX; k++)
                            out[o++] = w[k];
                        i = j;
                        continue;
                    }
                }
            }
        }
        out[o++] = ch;
        i++;
    }
    return o;
}


static uint8_t tmp[RAW_MAX];

/* Copying, filling, clearing and undoing are a job over the target's
 * cells, JOB_CELLS a frame (each may compile a formula and format its
 * value): target cell (r, c) takes the source block's cell (sr0 + (r -
 * tr0) mod its height, sc0 + (c - tc0) mod its width), its references
 * moved by as much, so a block pastes once and a row or column fills by
 * repeating. Where the target overlaps the source, the order reads every
 * source cell before it is written. A clear empties the target. A move
 * (a paste after Ctrl+X) copies without moving references, then clears
 * the source cells outside the target. Undo and redo apply a group of
 * the log. */
#define JOB_CELLS 6
enum { JOB_NONE, JOB_COPY, JOB_CLEAR, JOB_UNDO, JOB_REDO, JOB_FMT, JOB_SHIFT, JOB_SORT, JOB_TEXT };
static uint8_t job, job_full, j_cut, j_keep;
static uint32_t js_r0, js_c0, js_h, js_w, jt_r0, jt_c0, jt_h, jt_w, j_at, jk_r0, jk_c0, jk_r1, jk_c1;
static uint32_t j_pos, j_stop;
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
    job_full = j_cut = j_keep = 0;
    job_msg = msg;
    ubegin();
}

static void job_step2(void);

static void job_step(void)
{
    uint32_t k;
    if (job >= JOB_FMT) {
        job_step2();
        return;
    }
    if (job == JOB_UNDO || job == JOB_REDO) {
        for (k = 0; k < JOB_CELLS; k++) {
            uint32_t at;
            if (job == JOB_UNDO) {
                if (j_pos <= j_stop)
                    break;
                j_pos -= (uint32_t)(ulog[j_pos - 2] | ulog[j_pos - 1] << 8);
                at = j_pos;
                set_cell_fmt((uint16_t)(ulog[at] | ulog[at + 1] << 8), ulog + at + 5, ulog[at + 4], ulog[at + 2]);
            } else {
                if (j_pos >= j_stop)
                    break;
                at = j_pos;
                j_pos += rec_size(at);
                set_cell_fmt((uint16_t)(ulog[at] | ulog[at + 1] << 8), ulog + at + 6 + ulog[at + 4], ulog[at + 5 + ulog[at + 4]], ulog[at + 3]);
            }
        }
        if (job == JOB_UNDO ? j_pos > j_stop : j_pos < j_stop)
            return;
        say(job == JOB_UNDO ? "Undone (Ctrl+Y redoes)" : "Redone");
        job = JOB_NONE;
        doc_dirty = 1;
        return;
    }
    for (k = 0; k < JOB_CELLS && j_at < jt_h * jt_w; k++, j_at++) {
        uint32_t a = j_at / jt_w, b = j_at % jt_w;
        uint32_t y = j_dy < 0 ? jt_h - 1 - a : a, x = j_dx < 0 ? jt_w - 1 - b : b;
        uint32_t tr = jt_r0 + y, tc = jt_c0 + x;
        if (job == JOB_CLEAR) {
            if (j_keep && tr >= jk_r0 && tr <= jk_r1 && tc >= jk_c0 && tc <= jk_c1)
                continue;
            /* A clear keeps the format; the source of a move loses it. */
            put_cell(idx(tr, tc), 0, 0, j_keep ? 0 : c_fmt[idx(tr, tc)]);
            continue;
        }
        uint32_t r = js_r0 + y % js_h, c = js_c0 + x % js_w;
        uint16_t s = idx(r, c);
        int32_t dr = j_cut ? 0 : (int32_t)tr - (int32_t)r, dc = j_cut ? 0 : (int32_t)tc - (int32_t)c;
        uint32_t n = shift_refs(pool + c_off[s], c_rlen[s], dr, dc, tmp);
        if (!put_cell(idx(tr, tc), tmp, n, c_fmt[s]))
            job_full = 1;
    }
    if (j_at < jt_h * jt_w)
        return;
    if (j_cut) {
        /* A move: now the source cells the target did not cover. */
        j_cut = 0;
        j_keep = 1;
        jk_r0 = jt_r0;
        jk_c0 = jt_c0;
        jk_r1 = jt_r0 + jt_h - 1;
        jk_c1 = jt_c0 + jt_w - 1;
        jt_r0 = js_r0;
        jt_c0 = js_c0;
        jt_h = js_h;
        jt_w = js_w;
        j_dy = j_dx = 1;
        j_at = 0;
        job = JOB_CLEAR;
        return;
    }
    job = JOB_NONE;
    say(job_full ? "The sheet is full" : u_off ? "Done; it is too large to undo" : job_msg);
}

/* Ctrl+Z (redo 0) and Ctrl+Y (redo 1). */
static void undo(int redo)
{
    uint32_t mark;
    if (!redo) {
        if (!u_cur) {
            say("Nothing to undo");
            return;
        }
        mark = u_cur;
        do
            mark -= (uint32_t)(ulog[mark - 2] | ulog[mark - 1] << 8);
        while (mark && !is_mark(mark));
        j_pos = u_cur;
        j_stop = mark + MARK;
        u_cur = mark;
    } else {
        if (u_cur >= u_end) {
            say("Nothing to redo");
            return;
        }
        mark = u_cur;
        j_pos = j_stop = mark + MARK;
        while (j_stop < u_end && !is_mark(j_stop))
            j_stop += rec_size(j_stop);
        u_cur = j_stop;
    }
    job = redo ? JOB_REDO : JOB_UNDO;
    {
        /* The chart as it was before (undo) or after (redo) the change. */
        const uint8_t *ch = ulog + mark + (redo ? 11 : 6);
        if (ch[0] != chart_kind || ch[1] != ch_r0 || ch[2] != ch_c0 || ch[3] != ch_r1 || ch[4] != ch_c1) {
            chart_kind = ch[0];
            ch_r0 = ch[1];
            ch_c0 = ch[2];
            ch_r1 = ch[3];
            ch_c1 = ch[4];
            doc_dirty = 1;
        }
    }
    sr = ulog[mark + 2];
    sc = ulog[mark + 3];
    ar = ulog[mark + 4];
    ac = ulog[mark + 5];
    show_selection();
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

static void clipboard_text(void);

static void copy(int cut)
{
    clip_on = 1;
    clip_cut = (uint8_t)cut;
    cl_r0 = (uint8_t)rmin();
    cl_c0 = (uint8_t)cmin();
    cl_r1 = (uint8_t)rmax();
    cl_c1 = (uint8_t)cmax();
    clipboard_text();
    say(cut ? "Cut: Ctrl+V moves it to the selected cell (Esc forgets it)"
            : "Copied: Ctrl+V pastes it at the selected cell (Esc forgets it; then Ctrl+V pastes text from other programs)");
}

/* Pastes what was copied at the selected cell; one cell copied fills a
 * selected range. After a cut, moves it (once). */
static void paste(void)
{
    uint32_t h = cl_r1 - cl_r0 + 1u, w = cl_c1 - cl_c0 + 1u, th = h, tw = w, r0 = rmin(), c0 = cmin();
    if (!clip_on) {
        say("Nothing copied: Ctrl+C copies, Ctrl+X cuts");
        return;
    }
    if (h == 1 && w == 1 && !clip_cut && (rmin() != rmax() || cmin() != cmax())) {
        r0 = rmin();
        c0 = cmin();
        th = rmax() - r0 + 1;
        tw = cmax() - c0 + 1;
    }
    sr = r0;
    sc = c0;
    ar = clamp((int32_t)(r0 + th - 1), NROWS);
    ac = clamp((int32_t)(c0 + tw - 1), NCOLS);
    show_selection();
    start_job(JOB_COPY, cl_r0, cl_c0, h, w, r0, c0, th, tw, clip_cut ? "Moved" : "Pasted");
    if (clip_cut) {
        j_cut = 1;
        clip_on = clip_cut = 0;
    }
}

static void set_chart(uint8_t kind)
{
    /* The same key again removes the chart: with its range selected, or one
     * cell (nothing to chart). */
    if (chart_kind == kind && ((ch_r0 == rmin() && ch_c0 == cmin() && ch_r1 == rmax() && ch_c1 == cmax()) || (rmin() == rmax() && cmin() == cmax()))) {
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
        /* "Bar chart of D3:D7", and whether its values have labels. */
        static char m[64];
        const char *w = kind == 1 ? "Bar chart of " : "Line chart of ";
        uint32_t n = 0, len, many = ch_r0 == ch_r1 ? ch_c1 - ch_c0 >= 8 : ch_r1 - ch_r0 >= 8;
        while (*w)
            m[n++] = *w++;
        range_name(m + n, &len, ch_r0, ch_c0, ch_r1, ch_c1);
        n += len;
        w = many ? " (labels for at most 8)" : " (the same key again removes it)";
        while (*w)
            m[n++] = *w++;
        m[n] = 0;
        say(m);
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

/* Reading the saved form at fdata[0..flen), PARSE_WORK a frame. */
static uint32_t parse_at, parse_row, parse_col;
static uint8_t parsing, cut;

static void clear_sheet(void)
{
    zero(c_rlen, sizeof c_rlen);
    zero(c_clen, sizeof c_clen);
    zero(c_flag, sizeof c_flag);
    zero(c_kind, sizeof c_kind);
    zero(c_dlen, sizeof c_dlen);
    zero(c_fmt, sizeof c_fmt);
    zero(c_state, sizeof c_state);
    zero(row_f, sizeof row_f);
    pool_used = 0;
    chart_kind = 0;
    recalcing = 0;
    rsp = 0;
    top = left = sr = sc = ar = ac = 0;
    editing = pt_on = pt_drag = 0;
    tab_col = -1;
    u_cur = u_end = u_group = 0; /* a new or loaded sheet has nothing to undo */
    clip_on = clip_cut = 0;
    doc_dirty = 1;
}

static void start_parse(void)
{
    clear_sheet();
    parse_at = parse_row = parse_col = 0;
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

/* "format D3:D7 2s": decimals, s for separators, % for a percentage. */
static void parse_format(const uint8_t *p, uint32_t n)
{
    uint32_t at = 7, r, c;
    uint8_t r0, c0, r1 = 0, c1 = 0, f;
    if (n < 10 || memcmp(p, "format ", 7) || !parse_name(p, n, &at, &r0, &c0))
        return;
    r1 = r0;
    c1 = c0;
    if (at < n && p[at] == ':') {
        at++;
        if (!parse_name(p, n, &at, &r1, &c1))
            return;
    }
    if (at + 1 >= n || p[at] != ' ' || !is_digit(p[at + 1]) || p[at + 1] > '4')
        return;
    f = (uint8_t)(FMT_SET | (p[at + 1] - '0'));
    for (at += 2; at < n; at++)
        f |= p[at] == 's' ? FMT_SEP : p[at] == '%' ? FMT_PCT : 0;
    for (r = r0; r <= r1; r++)
        for (c = c0; c <= c1; c++) {
            c_fmt[idx(r, c)] = f;
            update_disp(idx(r, c));
        }
}

static void parse_step(void)
{
    int32_t budget = PARSE_WORK;
    while (parse_at < flen && budget > 0) {
        uint32_t e = parse_at, len;
        while (e < flen && fdata[e] != '\t' && fdata[e] != '\n')
            e++;
        len = e - parse_at;
        if (len && (e == flen || fdata[e] == '\n') && fdata[e - 1] == '\r')
            len--;
        if (!parse_col && len && fdata[parse_at] == 0x0C) {
            parse_chart(fdata + parse_at + 1, len - 1);
            parse_format(fdata + parse_at + 1, len - 1);
        }
        else if (parse_row < NROWS && parse_col < NCOLS) {
            if (len > RAW_MAX)
                cut = 1;
            if (len && !set_cell(idx(parse_row, parse_col), fdata + parse_at, len))
                cut = 1;
        } else if (len)
            cut = 1;
        /* About what reading and showing a cell costs, in work units. */
        budget -= (int32_t)(PARSE_CELL + len);
        if (e < flen && fdata[e] == '\t')
            parse_col++;
        else {
            parse_row++;
            parse_col = 0;
        }
        parse_at = e + 1;
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
    /* The number formats: a line for each run of cells of a row in the
     * same format ("format D3:D7 2s"). */
    for (uint32_t r = 0; r < NROWS; r++)
        for (uint32_t c = 0; c < NCOLS;) {
            uint8_t f = c_fmt[idx(r, c)];
            uint32_t e = c + 1, n;
            char nm[8];
            if (!f) {
                c++;
                continue;
            }
            while (e < NCOLS && c_fmt[idx(r, e)] == f)
                e++;
            if (flen + 32 > DATA_MAX)
                return 1;
            memcpy(fdata + flen, "\x0C" "format ", 8);
            flen += 8;
            range_name(nm, &n, r, c, r, e - 1);
            memcpy(fdata + flen, nm, n);
            flen += n;
            fdata[flen++] = ' ';
            fdata[flen++] = (uint8_t)('0' + (f & FMT_DEC));
            if (f & FMT_SEP)
                fdata[flen++] = 's';
            if (f & FMT_PCT)
                fdata[flen++] = '%';
            fdata[flen++] = '\n';
            c = e;
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
        save_h = record_save(&save_req, rec_name, fdata, flen);
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
        load_h = record_load(&load_req, rec_name, flen, want, fdata + flen);
    if (!load_h) {
        loading = 0;
        say("Could not load the sheet");
    }
}

/* The example: an estimate for a service. */
static const char example[] =
    "Estimate\n"
    "Item\tQty\tPrice\tAmount\n"
    "Panels\t12\t45.50\t=B3*C3\n"
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
    say(arg_msg ? arg_msg : "An example estimate: column D is formulas, select one to see it");
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
        /* No saved sheet: the example. A host that keeps no records
         * (the reading profile may refuse them) shows it too. */
        if (load_req.status == STATUS_REFUSED && !arg_msg)
            arg_msg = "Sheets are not kept on this host";
        else if (load_req.status != STATUS_NOT_FOUND && !arg_msg)
            arg_msg = "Could not load the saved sheet";
        load_example();
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
    else if (rec_name[5] == ':') {
        static char m[48];
        uint32_t n = append(m, 0, "Opened ");
        append(m, n, rec_name + 6);
        say(m);
    }
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
    if (h == list_h)
        listed();
    else if (h == head_h)
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

/* --- Jobs over many cells: formats, inserting and deleting, sorting,
 * pasting text ------------------------------------------------------------ */

/* The cells a job changes in a frame: each may compile a formula and
 * format its value. Cells it only looks at cost a little (JOB_LOOK). */
#define JOB_PUTS 12
#define JOB_LOOK 400
static uint8_t j_fmt_op, j_axis, j_del;
static uint32_t j_at0, j_cnt, j_n, j_r0, j_c0, j_r1, j_c1;
static uint8_t j_perm[NROWS];
static uint16_t j_soff[NCELLS]; /* sort: each source cell's offset in the stash */
static const char *j_msg2;
static char msg_buf[96];

/* The paste of text from other programs (between the paste key's down
 * and up, SPEC §events): tabs split cells, line breaks rows. */
#define PBUF_MAX 16384
static uint8_t pbuf[PBUF_MAX];
static uint32_t pbuf_n, pbuf_at;
static uint8_t pbuf_cut;

static uint32_t append(char *out, uint32_t n, const char *w)
{
    while (*w && n < sizeof msg_buf - 1)
        out[n++] = *w++;
    out[n] = 0;
    return n;
}

static uint32_t append_num(char *out, uint32_t n, uint32_t v)
{
    uint8_t d[12];
    uint32_t k = put_int(v, 0, d);
    for (uint32_t i = 0; i < k && n < sizeof msg_buf - 1; i++)
        out[n++] = (char)d[i];
    out[n] = 0;
    return n;
}

/* --- Number formats --- */

enum { FO_GENERAL, FO_NUMBER, FO_PERCENT, FO_MORE, FO_FEWER };

/* The format a cell gets from operation op. */
static uint8_t new_format(uint8_t f, uint8_t op)
{
    uint8_t dec = f & FMT_SET ? f & FMT_DEC : 2;
    switch (op) {
    case FO_GENERAL: return 0;
    case FO_NUMBER:
        /* Two decimals with separators; again: without separators. */
        if ((f & (FMT_SET | FMT_SEP | FMT_PCT)) == (FMT_SET | FMT_SEP))
            return (uint8_t)(FMT_SET | dec);
        return (uint8_t)(FMT_SET | FMT_SEP | (f & FMT_SET ? dec : 2));
    case FO_PERCENT: return f & FMT_PCT ? 0 : FMT_SET | FMT_PCT;
    case FO_MORE:
        if (!(f & FMT_SET))
            return FMT_SET | FMT_SEP | 3;
        return (uint8_t)((f & ~FMT_DEC) | (dec < 4 ? dec + 1 : 4));
    default:
        if (!(f & FMT_SET))
            return FMT_SET | FMT_SEP | 1;
        return (uint8_t)((f & ~FMT_DEC) | (dec ? dec - 1 : 0));
    }
}

static const char *const fmt_said[] = { "General format", "Number format", "Percent format", "One more decimal", "One decimal fewer" };

/* Ctrl+Shift+~ general, Ctrl+Shift+1 number, Ctrl+Shift+5 percent,
 * Ctrl+. and Ctrl+, one decimal more or fewer: the selection, as a job.
 * The operation is decided by the active cell's format, so that every
 * cell of the selection ends in the same format. */
static void set_format(uint8_t op)
{
    j_fmt_op = new_format(c_fmt[idx(sr, sc)], op);
    j_r0 = rmin();
    j_c0 = cmin();
    j_r1 = rmax();
    j_c1 = cmax();
    j_n = 0;
    j_msg2 = fmt_said[op];
    ubegin();
    job = JOB_FMT;
}

/* --- Inserting and deleting rows and columns --- */

/* A row or column number moved by an insertion (del 0) or deletion of cnt
 * at `at`: -1 if it was deleted. */
static int32_t moved(int32_t v, int del)
{
    if (!del)
        return v >= (int32_t)j_at0 ? v + (int32_t)j_cnt : v;
    if (v >= (int32_t)(j_at0 + j_cnt))
        return v - (int32_t)j_cnt;
    return v >= (int32_t)j_at0 ? -1 : v;
}

/* A reference (optional $, letter, optional $, digits) at p[i], not part
 * of a longer name: 1, its end in *e, column, row (0-based) and its $s. */
static int ref_at(const uint8_t *p, uint32_t n, uint32_t i, uint32_t *e, int32_t *col, int32_t *row, uint32_t *cabs, uint32_t *rabs)
{
    uint32_t j = i, v = 0;
    if (i && (is_alpha(p[i - 1]) || is_digit(p[i - 1]) || p[i - 1] == '$'))
        return 0;
    *cabs = *rabs = 0;
    if (j < n && p[j] == '$') {
        *cabs = 1;
        j++;
    }
    if (j >= n || !is_alpha(p[j]))
        return 0;
    *col = upper(p[j]) - 'A';
    j++;
    if (j < n && p[j] == '$') {
        *rabs = 1;
        j++;
    }
    if (j >= n || !is_digit(p[j]))
        return 0;
    while (j < n && is_digit(p[j])) {
        if (v < 1000)
            v = v * 10 + (p[j] - '0');
        j++;
    }
    if (j < n && (p[j] == '(' || is_alpha(p[j])))
        return 0;
    *row = (int32_t)v - 1;
    *e = j;
    return 1;
}

/* A formula's text with its references moved for the insertion or
 * deletion j_axis/j_at0/j_cnt/j_del, as spreadsheets move them: whatever
 * is past the place moves (also after a $), a reference to a deleted cell
 * becomes #REF!, a range that loses one end shrinks, one that loses all
 * of itself is #REF!. */
static uint32_t move_refs(const uint8_t *p, uint32_t n, uint8_t *out)
{
    uint32_t i = 0, o = 0;
    int in_str = 0;
    if (!n || p[0] != '=') {
        memcpy(out, p, n);
        return n;
    }
    while (i < n && o < RAW_MAX) {
        uint32_t e, e2, ca, ra, ca2, ra2;
        int32_t c1, r1, c2, r2;
        if (p[i] == '"')
            in_str = !in_str;
        if (!in_str && ref_at(p, n, i, &e, &c1, &r1, &ca, &ra)) {
            uint8_t w[16];
            uint32_t len = 0, k;
            int range = e < n && p[e] == ':' && ref_at(p, n, e + 1, &e2, &c2, &r2, &ca2, &ra2);
            int32_t *v1 = j_axis ? &c1 : &r1, *v2 = j_axis ? &c2 : &r2, lim = j_axis ? NCOLS : NROWS;
            int bad = 0;
            if (!range) {
                *v1 = moved(*v1, j_del);
                bad = *v1 < 0 || *v1 >= lim;
            } else {
                int32_t lo = *v1 < *v2 ? *v1 : *v2, hi = *v1 < *v2 ? *v2 : *v1, nlo, nhi;
                if (!j_del) {
                    nlo = moved(lo, 0);
                    nhi = moved(hi, 0);
                } else {
                    nlo = lo < (int32_t)j_at0 ? lo : lo < (int32_t)(j_at0 + j_cnt) ? (int32_t)j_at0 : lo - (int32_t)j_cnt;
                    nhi = hi < (int32_t)j_at0 ? hi : hi < (int32_t)(j_at0 + j_cnt) ? (int32_t)j_at0 - 1 : hi - (int32_t)j_cnt;
                }
                bad = nlo > nhi || nlo < 0 || nhi >= lim;
                if (*v1 <= *v2) {
                    *v1 = nlo;
                    *v2 = nhi;
                } else {
                    *v1 = nhi;
                    *v2 = nlo;
                }
            }
            if (bad || c1 < 0 || r1 < 0 || c1 >= NCOLS || r1 >= NROWS) {
                memcpy(w, "#REF!", 5);
                len = 5;
            } else {
                len = ref_text((uint32_t)r1, (uint32_t)c1, (int)ca, (int)ra, w);
                if (range) {
                    w[len++] = ':';
                    len += ref_text((uint32_t)r2, (uint32_t)c2, (int)ca2, (int)ra2, w + len);
                }
            }
            for (k = 0; k < len && o < RAW_MAX; k++)
                out[o++] = w[k];
            i = range ? e2 : e;
            continue;
        }
        out[o++] = p[i++];
    }
    return o;
}

static int whole_rows(void) { return cmin() == 0 && cmax() == NCOLS - 1; }
static int whole_cols(void) { return rmin() == 0 && rmax() == NROWS - 1; }

/* Ctrl+Plus inserts (Ctrl+Minus deletes) the selected columns when whole
 * columns are selected (their letters clicked), else the selection's rows. */
static void insert_delete(int del)
{
    uint32_t i, lim;
    j_axis = whole_cols() && !whole_rows();
    j_del = (uint8_t)del;
    j_at0 = j_axis ? cmin() : rmin();
    j_cnt = j_axis ? cmax() - cmin() + 1 : rmax() - rmin() + 1;
    lim = j_axis ? NCOLS : NROWS;
    if (j_cnt >= lim) {
        say("Select some rows (their numbers) or columns (their letters), not the whole sheet");
        return;
    }
    if (!del)
        /* What would be pushed off the sheet must be empty. */
        for (i = 0; i < NCELLS; i++) {
            uint32_t v = j_axis ? i % NCOLS : i / NCOLS;
            if (v >= lim - j_cnt && c_rlen[i]) {
                say(j_axis ? "No room: the last columns are not empty" : "No room: the last rows are not empty");
                return;
            }
        }
    /* The chart's range follows; a chart whose range is deleted goes (undo
     * brings it back). */
    chart_before[0] = chart_kind;
    chart_before[1] = ch_r0;
    chart_before[2] = ch_c0;
    chart_before[3] = ch_r1;
    chart_before[4] = ch_c1;
    chart_set = 1;
    if (chart_kind) {
        int32_t a = moved(j_axis ? ch_c0 : ch_r0, del), b = moved(j_axis ? ch_c1 : ch_r1, del);
        if (del) {
            int32_t lo = j_axis ? ch_c0 : ch_r0, hi = j_axis ? ch_c1 : ch_r1;
            a = lo < (int32_t)j_at0 ? lo : lo < (int32_t)(j_at0 + j_cnt) ? (int32_t)j_at0 : lo - (int32_t)j_cnt;
            b = hi < (int32_t)j_at0 ? hi : hi < (int32_t)(j_at0 + j_cnt) ? (int32_t)j_at0 - 1 : hi - (int32_t)j_cnt;
        }
        if (a > b || a < 0 || b >= (int32_t)lim)
            chart_kind = 0;
        else if (j_axis) {
            ch_c0 = (uint8_t)a;
            ch_c1 = (uint8_t)b;
        } else {
            ch_r0 = (uint8_t)a;
            ch_r1 = (uint8_t)b;
        }
    }
    clip_on = 0;
    {
        uint32_t n = append(msg_buf, 0, del ? "Deleted " : "Inserted ");
        n = append_num(msg_buf, n, j_cnt);
        n = append(msg_buf, n, j_axis ? (j_cnt > 1 ? " columns" : " column") : (j_cnt > 1 ? " rows" : " row"));
        append(msg_buf, n, j_axis ? " (Ctrl+Z undoes)" : " (Ctrl+Z undoes; click column letters to work on columns)");
        j_msg2 = msg_buf;
    }
    j_n = 0;
    ubegin();
    job = JOB_SHIFT;
}

/* --- Sorting --- */

/* Sorting order: numbers (and TRUE/FALSE) by value, then text without
 * regard to case, then errors, then empty cells, which stay last also in
 * descending order. */
static int sort_cmp(uint16_t a, uint16_t b, int desc)
{
    value x = cell_value(a), y = cell_value(b);
    int kx = x.k == K_EMPTY ? 3 : x.k == K_ERR ? 2 : x.k == K_STR ? 1 : 0;
    int ky = y.k == K_EMPTY ? 3 : y.k == K_ERR ? 2 : y.k == K_STR ? 1 : 0;
    int c;
    if (kx == 3 || ky == 3)
        return kx - ky;
    if (kx != ky)
        c = kx - ky;
    else if (kx == 1)
        c = cmp_text(x, y);
    else if (kx == 0)
        c = x.v < y.v ? -1 : x.v > y.v;
    else
        c = 0;
    return desc ? -c : c;
}

/* The block of filled cells around the active cell (as spreadsheets find
 * the data to sort when one cell is selected). */
static void current_region(uint32_t *r0, uint32_t *c0, uint32_t *r1, uint32_t *c1)
{
    int32_t a = (int32_t)sr, b = (int32_t)sc, c = (int32_t)sr, d = (int32_t)sc, grew = 1, k;
    while (grew) {
        grew = 0;
        for (k = b - 1; k <= d + 1; k++)
            if (filled(a - 1, k)) {
                a--;
                grew = 1;
                break;
            }
        for (k = b - 1; k <= d + 1; k++)
            if (filled(c + 1, k)) {
                c++;
                grew = 1;
                break;
            }
        for (k = a; k <= c; k++)
            if (filled(k, b - 1)) {
                b--;
                grew = 1;
                break;
            }
        for (k = a; k <= c; k++)
            if (filled(k, d + 1)) {
                d++;
                grew = 1;
                break;
            }
    }
    *r0 = (uint32_t)a;
    *c0 = (uint32_t)b;
    *r1 = (uint32_t)c;
    *c1 = (uint32_t)d;
}

static uint32_t range_text(char *out, uint32_t n, uint32_t r0, uint32_t c0, uint32_t r1, uint32_t c1)
{
    uint32_t len;
    range_name(out + n, &len, r0, c0, r1, c1);
    out[n + len] = 0;
    return n + len;
}

/* Ctrl+Shift+A sorts ascending, Ctrl+Shift+D descending: the selection (or
 * the block of data around the active cell) by the active cell's column.
 * A first row of text over a number in that column is a header and stays.
 * Rows move whole, their relative references moving with them as in a
 * copy (so =B3*C3 in row 3 becomes =B5*C5 in row 5). */
static void sort_range(int desc)
{
    uint32_t r0 = rmin(), c0 = cmin(), r1 = rmax(), c1 = cmax(), key, i, k, n, at = 0, head = 0;
    if (saving || recalcing || dirty_all) {
        say("Busy: sort again in a moment");
        return;
    }
    if (r0 == r1 && c0 == c1)
        current_region(&r0, &c0, &r1, &c1);
    key = sc >= c0 && sc <= c1 ? sc : c0;
    if (r1 > r0 && cell_value(idx(r0, key)).k == K_STR && (cell_value(idx(r0 + 1, key)).k == K_NUM || cell_value(idx(r0 + 1, key)).k == K_BOOL)) {
        head = 1;
        r0++;
    }
    if (r0 >= r1) {
        say("Nothing to sort: select the rows, or a cell in a block of data");
        return;
    }
    /* The order: a stable insertion sort of the rows. */
    for (i = 0; i <= r1 - r0; i++) {
        uint8_t v = (uint8_t)(r0 + i);
        k = i;
        while (k && sort_cmp(idx(j_perm[k - 1], key), idx(v, key), desc) > 0) {
            j_perm[k] = j_perm[k - 1];
            k--;
        }
        j_perm[k] = v;
    }
    /* The cells as they are, in the stash (fdata: a sort waits while the
     * sheet is saved or loaded): format, length, text. */
    for (i = r0; i <= r1; i++)
        for (k = c0; k <= c1; k++) {
            uint16_t c = idx(i, k);
            if (at + 2 + c_rlen[c] > DATA_MAX) {
                say("Too much to sort at once");
                return;
            }
            j_soff[(i - r0) * (c1 - c0 + 1) + (k - c0)] = (uint16_t)at;
            fdata[at] = c_fmt[c];
            fdata[at + 1] = c_rlen[c];
            memcpy(fdata + at + 2, pool + c_off[c], c_rlen[c]);
            at += 2 + c_rlen[c];
        }
    j_r0 = r0;
    j_c0 = c0;
    j_r1 = r1;
    j_c1 = c1;
    j_n = 0;
    clip_on = 0;
    n = append(msg_buf, 0, "Sorted ");
    n = range_text(msg_buf, n, r0, c0, r1, c1);
    n = append(msg_buf, n, " by column ");
    msg_buf[n++] = (char)('A' + key);
    msg_buf[n] = 0;
    n = append(msg_buf, n, desc ? ", Z to A" : ", A to Z");
    if (head) {
        n = append(msg_buf, n, " (row ");
        n = append_num(msg_buf, n, r0);
        n = append(msg_buf, n, ", text over numbers, kept as the header)");
    }
    j_msg2 = msg_buf;
    ubegin();
    job = JOB_SORT;
    if (head) {
        /* The selection: the rows sorted. */
        sr = r0;
        ar = r1;
    }
}

/* --- Pasting text --- */

static void paste_text_start(void)
{
    if (!pbuf_n) {
        say("Nothing to paste (Ctrl+C copies cells; the clipboard holds no text)");
        return;
    }
    pbuf_at = 0;
    j_r0 = rmin();
    j_c0 = cmin();
    j_r1 = j_r0;
    j_c1 = j_c0;
    sr = ar = j_r0;
    sc = ac = j_c0;
    j_n = 0;
    j_msg2 = pbuf_cut ? "Pasted (cut at 16 KiB, Z or row 99)" : "Pasted";
    ubegin();
    job = JOB_TEXT;
}

/* --- The clipboard --- */

/* The copied cells as other programs take them (SPEC §system `copy`): a
 * line per row, the cells' values as shown (in their formats, at full
 * width) separated by tabs. The host keeps it only when the copy came
 * from the user's copy key in this frame. */
static void clipboard_text(void)
{
    uint32_t n = 0, r, c;
    for (r = cl_r0; r <= cl_r1; r++) {
        for (c = cl_c0; c <= cl_c1; c++) {
            uint16_t i = idx(r, c);
            uint8_t b[48];
            const uint8_t *p = b;
            uint32_t len = 0;
            value v = cell_value(i);
            if (c > cl_c0)
                pbuf[n++] = '\t';
            if (v.k == K_NUM)
                len = format_num(v.v, c_fmt[i], c_flag[i] & F_PCT, 0, b);
            else if (v.k == K_STR) {
                p = str_p(v.v);
                len = STR_LEN(v.v);
            } else if (v.k == K_BOOL || v.k == K_ERR) {
                const char *w = v.k == K_BOOL ? (v.v ? "TRUE" : "FALSE") : err_text[v.v];
                while (w[len])
                    len++;
                p = (const uint8_t *)w;
            }
            if (n + len + 2 > PBUF_MAX)
                goto done;
            memcpy(pbuf + n, p, len);
            n += len;
        }
        pbuf[n++] = '\n';
    }
done:
    terse_copy(pbuf, n);
}

/* --- The jobs --- */

static void job_done(void)
{
    job = JOB_NONE;
    say(job_full ? "The sheet is full" : u_off ? "Done; it is too large to undo" : j_msg2);
    doc_dirty = 1;
}

static void job_step2(void)
{
    int32_t budget = JOB_LOOK;
    uint32_t puts = 0;
    if (job == JOB_FMT) {
        uint32_t w = j_c1 - j_c0 + 1, total = (j_r1 - j_r0 + 1) * w;
        for (; j_n < total && budget > 0; j_n++) {
            uint16_t i = idx(j_r0 + j_n / w, j_c0 + j_n % w);
            budget -= c_flag[i] ? 40 : 2;
            if (c_fmt[i] != j_fmt_op)
                put_cell(i, pool + c_off[i], c_rlen[i], j_fmt_op);
        }
        if (j_n >= total)
            job_done();
        return;
    }
    if (job == JOB_SHIFT) {
        uint32_t lim = j_axis ? NCOLS : NROWS, other = j_axis ? NROWS : NCOLS;
        for (; j_n < NCELLS && budget > 0 && puts < JOB_PUTS; j_n++) {
            /* Inserting: from the far end back; deleting: from the start. */
            uint32_t line = j_n / other, pos = j_n % other;
            uint32_t v = j_del ? line : lim - 1 - line, r = j_axis ? pos : v, c = j_axis ? v : pos;
            int32_t sv;
            uint16_t t = idx(r, c), s;
            if (!j_del)
                sv = v >= j_at0 + j_cnt ? (int32_t)(v - j_cnt) : v >= j_at0 ? -1 : (int32_t)v;
            else
                sv = v + j_cnt >= lim ? -1 : v >= j_at0 ? (int32_t)(v + j_cnt) : (int32_t)v;
            budget--;
            if (sv < 0) {
                if (c_rlen[t] || c_fmt[t]) {
                    put_cell(t, 0, 0, 0);
                    puts++;
                }
                continue;
            }
            s = j_axis ? idx(r, (uint32_t)sv) : idx((uint32_t)sv, c);
            if (!c_rlen[s] && !c_rlen[t] && c_fmt[s] == c_fmt[t])
                continue;
            uint32_t n = c_flag[s] & F_FORMULA ? move_refs(pool + c_off[s], c_rlen[s], tmp) : c_rlen[s];
            if (!(c_flag[s] & F_FORMULA))
                memcpy(tmp, pool + c_off[s], n);
            budget -= 10;
            if (c_rlen[t] != n || memcmp(pool + c_off[t], tmp, n) || c_fmt[t] != c_fmt[s]) {
                if (!put_cell(t, tmp, n, c_fmt[s]))
                    job_full = 1;
                puts++;
            }
        }
        if (j_n >= NCELLS)
            job_done();
        return;
    }
    if (job == JOB_SORT) {
        uint32_t w = j_c1 - j_c0 + 1, total = (j_r1 - j_r0 + 1) * w;
        for (; j_n < total && puts < JOB_PUTS; j_n++) {
            uint32_t row = j_r0 + j_n / w, col = j_c0 + j_n % w, from = j_perm[j_n / w];
            const uint8_t *e = fdata + j_soff[(from - j_r0) * w + j_n % w];
            uint32_t n = shift_refs(e + 2, e[1], (int32_t)row - (int32_t)from, 0, tmp);
            uint16_t t = idx(row, col);
            if (c_rlen[t] != n || memcmp(pool + c_off[t], tmp, n) || c_fmt[t] != e[0]) {
                if (!put_cell(t, tmp, n, e[0]))
                    job_full = 1;
                puts++;
            }
        }
        if (j_n >= total)
            job_done();
        return;
    }
    /* JOB_TEXT: the next cells of the pasted text. */
    while (pbuf_at < pbuf_n && puts < JOB_PUTS) {
        uint32_t e = pbuf_at, r = j_r0 + j_n, c = j_c1;
        while (e < pbuf_n && pbuf[e] != '\t' && pbuf[e] != '\n')
            e++;
        if (r < NROWS && c < NCOLS) {
            uint16_t t = idx(r, c);
            uint32_t n = e - pbuf_at > RAW_MAX ? RAW_MAX : e - pbuf_at;
            if (!put_cell(t, pbuf + pbuf_at, n, c_fmt[t]))
                job_full = 1;
            if (c > ac)
                ac = c;
            if (r > ar)
                ar = r;
            puts++;
        }
        if (e < pbuf_n && pbuf[e] == '\t')
            j_c1++;
        else {
            j_n++;
            j_c1 = j_c0;
        }
        pbuf_at = e + 1;
        /* A last line break ends the last row, it starts none. */
        if (pbuf_at == pbuf_n && e < pbuf_n && pbuf[e] == '\n')
            break;
    }
    if (pbuf_at >= pbuf_n) {
        sr = j_r0;
        sc = j_c0;
        show_selection();
        job_done();
    }
}

/* --- The menu bar ------------------------------------------------------------
 *
 * A row of titles at the top of the screen, and the open menu under its
 * title, drawn in terminal characters (device 2: a fixed grid of 8 x 16
 * screen units, so a press is placed exactly) over panels the 2D display
 * draws over the page (SPEC §vector.layers: the page, then the shapes, then
 * the terminal's characters). Opened by a press on a title, F10, or Alt and
 * the title's letter; Left and Right go from menu to menu, Up and Down from
 * item to item, Enter or a press chooses, Esc or a press elsewhere closes.
 * Each item does what its key does (and shows the key). The same panel
 * lists the saved sheets for File, Open. Without the terminal or the 2D
 * display there is no menu bar; the keys and F1 still work. */

static void key(uint8_t k, uint8_t mods);

typedef struct {
    const char *label, *keys;
    uint8_t key, mods;
} mitem;

#define CTRL MOD_CTRL
#define SHIFT MOD_SHIFT
static const mitem m_file[] = {
    { "New", "Ctrl+N", KEY_N, CTRL },
    { "Open\xE2\x80\xA6", "Ctrl+O", KEY_O, CTRL },
    { "Save", "Ctrl+S", KEY_S, CTRL },
    { "Save as\xE2\x80\xA6", "Ctrl+Shift+S", KEY_S, CTRL | SHIFT },
    { "Publish\xE2\x80\xA6", "Ctrl+P", KEY_P, CTRL },
};
static const mitem m_edit[] = {
    { "Undo", "Ctrl+Z", KEY_Z, CTRL },
    { "Redo", "Ctrl+Y", KEY_Y, CTRL },
    { "Cut", "Ctrl+X", KEY_X, CTRL },
    { "Copy", "Ctrl+C", KEY_C, CTRL },
    { "Paste", "Ctrl+V", KEY_V, CTRL },
    { "Clear", "Delete", KEY_DELETE, 0 },
    { "Fill down", "Ctrl+D", KEY_D, CTRL },
    { "Fill right", "Ctrl+R", KEY_R, CTRL },
    { "Insert rows or columns", "Ctrl+Plus", KEY_EQUAL, CTRL | SHIFT },
    { "Delete rows or columns", "Ctrl+Minus", KEY_MINUS, CTRL },
    { "Select all", "Ctrl+A", KEY_A, CTRL },
};
static const mitem m_data[] = {
    { "Sort A to Z", "Ctrl+Shift+A", KEY_A, CTRL | SHIFT },
    { "Sort Z to A", "Ctrl+Shift+D", KEY_D, CTRL | SHIFT },
};
static const mitem m_format[] = {
    { "General", "Ctrl+Shift+~", KEY_GRAVE, CTRL | SHIFT },
    { "Number 1,234.50", "Ctrl+Shift+1", KEY_1, CTRL | SHIFT },
    { "Percent", "Ctrl+Shift+5", KEY_5, CTRL | SHIFT },
    { "More decimals", "Ctrl+.", KEY_PERIOD, CTRL },
    { "Fewer decimals", "Ctrl+,", KEY_COMMA, CTRL },
};
static const mitem m_chart[] = {
    { "Bar chart of the selection", "Ctrl+G", KEY_G, CTRL },
    { "Line chart of the selection", "Ctrl+L", KEY_L, CTRL },
};
static const mitem m_help[] = {
    { "Keys", "F1", KEY_F1, 0 },
};
static const struct {
    const char *title;
    uint8_t letter; /* Alt and this key opens it */
    const mitem *items;
    uint8_t n;
} menus[] = {
    { "File", KEY_F_, m_file, sizeof m_file / sizeof m_file[0] },
    { "Edit", KEY_E, m_edit, sizeof m_edit / sizeof m_edit[0] },
    { "Data", KEY_D, m_data, sizeof m_data / sizeof m_data[0] },
    { "Format", KEY_O, m_format, sizeof m_format / sizeof m_format[0] },
    { "Chart", KEY_C, m_chart, sizeof m_chart / sizeof m_chart[0] },
    { "Help", KEY_H, m_help, sizeof m_help / sizeof m_help[0] },
};
#define MENUS (sizeof menus / sizeof menus[0])
#define M_OPEN MENUS /* the list of saved sheets */
#define OPEN_MAX 24
static uint8_t menu_dirty, menu_drawn;
static uint8_t menu_at, item_at; /* menu_on: 1 + the open menu */
static char open_names[OPEN_MAX][NAME_MAX + 1];
static uint8_t open_n;
static uint32_t menu_col[MENUS + 1];
static uint8_t menu_w; /* the open panel's width in cells */

static int menu_items(void) { return menu_at == M_OPEN ? (open_n ? open_n : 1) : menus[menu_at].n; }

/* The bar's titles from column 1, two cells apart. */
static void menu_layout(void)
{
    uint32_t c = 1;
    for (uint32_t m = 0; m < MENUS; m++) {
        uint32_t n = 0;
        while (menus[m].title[n])
            n++;
        menu_col[m] = c;
        c += n + 3;
    }
    menu_col[M_OPEN] = 1;
}

static uint32_t utf8_len(const char *s)
{
    uint32_t n = 0;
    for (; *s; s++)
        n += ((uint8_t)*s & 0xC0) != 0x80;
    return n;
}

/* Writes s at (col, row) of the cells in fg, at most max cells. */
static void cells_put(uint8_t *row, uint32_t col, const char *s, uint8_t fg, uint32_t max)
{
    while (*s && max) {
        uint32_t cp = (uint8_t)*s++, more = cp >= 0xF0 ? 3 : cp >= 0xE0 ? 2 : cp >= 0xC0 ? 1 : 0;
        if (more)
            cp &= 0x3F >> more;
        while (more-- && *s)
            cp = cp << 6 | ((uint8_t)*s++ & 0x3F);
        if (col >= TERM_COLS)
            return;
        row[col * 4] = (uint8_t)cp;
        row[col * 4 + 1] = (uint8_t)(cp >> 8);
        row[col * 4 + 2] = fg;
        col++;
        max--;
    }
}

#define FG_TEXT 235
#define FG_DIM 244
#define FG_ON 15

/* The terminal's rows 0 to the panel's last: the bar, the open menu. */
static void menu_draw(void)
{
    static uint8_t row[TERM_COLS * 4];
    uint32_t rows = menu_on ? 1 + (uint32_t)menu_items() : 1, r, k;
    if (menu_drawn > rows)
        rows = menu_drawn;
    menu_w = 0;
    if (menu_on) {
        uint32_t w = 0;
        for (k = 0; k < (uint32_t)menu_items(); k++) {
            uint32_t n;
            if (menu_at == M_OPEN)
                n = open_n ? utf8_len(open_names[k]) + 2 : 50;
            else
                n = utf8_len(menus[menu_at].items[k].label) + utf8_len(menus[menu_at].items[k].keys) + 5;
            if (n > w)
                w = n;
        }
        menu_w = (uint8_t)(w < 16 ? 16 : w);
    }
    for (r = 0; r < rows; r++) {
        for (k = 0; k < TERM_COLS; k++) {
            row[k * 4] = ' ';
            row[k * 4 + 1] = 0;
            row[k * 4 + 2] = FG_TEXT;
            row[k * 4 + 3] = 0;
        }
        if (!r) {
            for (k = 0; k < MENUS; k++)
                cells_put(row, menu_col[k], menus[k].title, menu_on && menu_at == k ? FG_ON : FG_TEXT, 20);
            /* The sheet's name, at the right. */
            if (rec_name[5] == ':')
                cells_put(row, TERM_COLS - 1 - utf8_len(rec_name + 6), rec_name + 6, FG_DIM, NAME_MAX);
        } else if (menu_on && r - 1 < (uint32_t)menu_items()) {
            uint32_t i = r - 1, c0 = menu_col[menu_at], on = i == item_at;
            if (menu_at == M_OPEN) {
                if (open_n)
                    cells_put(row, c0 + 1, open_names[i], on ? FG_ON : FG_TEXT, menu_w - 1);
                else
                    cells_put(row, c0 + 1, "No saved sheets yet: File, Save as names one", FG_DIM, menu_w - 1);
            } else {
                const mitem *it = &menus[menu_at].items[i];
                cells_put(row, c0 + 1, it->label, on ? FG_ON : FG_TEXT, menu_w);
                cells_put(row, c0 + menu_w - 1 - utf8_len(it->keys), it->keys, on ? FG_ON : FG_DIM, 20);
            }
        }
        term_cells_write(r * TERM_COLS, row, TERM_COLS);
    }
    menu_drawn = (uint8_t)(menu_on ? 1 + menu_items() : 1);
    menu_dirty = 0;
    vec_dirty = 1;
}

/* The panels under the characters, drawn last in the 2D list. */
static void menu_shapes(void)
{
    vec_colour(242, 242, 242, 255);
    vrect(0, 0, 640, 16);
    vec_colour(0, 0, 0, 40);
    vrect(0, 16, 640, 17);
    if (!menu_on)
        return;
    int32_t x0 = (int32_t)menu_col[menu_at] * 8, x1 = x0 + menu_w * 8, y1 = 16 + 16 * menu_items();
    if (menu_at < MENUS) {
        vec_colour(ACC[0], ACC[1], ACC[2], 255);
        vrect(x0 - 8, 0, x0 + (int32_t)utf8_len(menus[menu_at].title) * 8 + 8, 16);
    }
    vec_colour(0, 0, 0, 50);
    vrect(x0 + 3, 19, x1 + 3, y1 + 3);
    vec_colour(255, 255, 255, 255);
    vrect(x0, 16, x1, y1);
    vec_colour(190, 190, 190, 255);
    vec_width(16);
    vec_move((int16_t)(x0 * 16), (int16_t)(16 * 16));
    vec_line((int16_t)(x0 * 16), (int16_t)(y1 * 16));
    vec_line((int16_t)(x1 * 16), (int16_t)(y1 * 16));
    vec_line((int16_t)(x1 * 16), (int16_t)(16 * 16));
    if (menu_at != M_OPEN || open_n) {
        vec_colour(ACC[0], ACC[1], ACC[2], 255);
        vrect(x0 + 1, 16 + 16 * item_at, x1 - 1, 32 + 16 * item_at);
    }
}

static void menu_open(uint32_t m)
{
    menu_on = 1;
    menu_at = (uint8_t)m;
    item_at = 0;
    menu_dirty = 1;
}

static void menu_close(void)
{
    menu_on = 0;
    menu_dirty = 1;
}

/* Chooses item i of the open menu. */
static void menu_choose(uint32_t i);

/* A key while a menu is open: 1 if it was the menu's. */
static int menu_key(uint8_t k)
{
    uint32_t n = (uint32_t)menu_items();
    switch (k) {
    case KEY_ESCAPE:
    case KEY_F10: menu_close(); return 1;
    case KEY_LEFT:
    case KEY_RIGHT:
        if (menu_at < MENUS)
            menu_open((menu_at + (k == KEY_LEFT ? MENUS - 1 : 1)) % MENUS);
        return 1;
    case KEY_UP: item_at = (uint8_t)((item_at + n - 1) % n); break;
    case KEY_DOWN: item_at = (uint8_t)((item_at + 1) % n); break;
    case KEY_HOME: item_at = 0; break;
    case KEY_END: item_at = (uint8_t)(n - 1); break;
    case KEY_ENTER:
    case KEY_SPACE: menu_choose(item_at); return 1;
    default: return 1; /* the menu has the keyboard */
    }
    menu_dirty = 1;
    return 1;
}

/* A press at (x, y) while menus are shown: 1 if the menu took it. */
static int menu_press(uint32_t x, uint32_t y)
{
    uint32_t col = x / 8, row = y / 16;
    if (row == 0) {
        for (uint32_t m = 0; m < MENUS; m++)
            if (col + 1 >= menu_col[m] && col <= menu_col[m] + utf8_len(menus[m].title)) {
                if (menu_on && menu_at == m)
                    menu_close();
                else
                    menu_open(m);
                return 1;
            }
        if (menu_on)
            menu_close();
        return 1;
    }
    if (!menu_on)
        return 0;
    if (row >= 1 && row <= (uint32_t)menu_items() && col >= menu_col[menu_at] && col < menu_col[menu_at] + menu_w) {
        menu_choose(row - 1);
        return 1;
    }
    menu_close(); /* a press elsewhere closes the menu, and is used up */
    return 1;
}

/* --- Named sheets: File, Open and Save as --- */

/* The record a sheet is kept in: "sheet" for the one that opens at start,
 * "sheet:" and its name for one saved with Save as. */

static void set_rec_name(const char *name)
{
    uint32_t n = 0;
    menu_dirty = 1;
    if (!*name) {
        memcpy(rec_name, "sheet", 6);
        return;
    }
    memcpy(rec_name, "sheet:", 6);
    while (name[n] && n < NAME_MAX) {
        rec_name[6 + n] = name[n];
        n++;
    }
    rec_name[6 + n] = 0;
}

static const char *sheet_name(void) { return rec_name[5] == ':' ? rec_name + 6 : ""; }

static void open_list(void)
{
    if (!keeps) {
        say("Sheets are not kept on this host");
        return;
    }
    if (list_h)
        return;
    list_h = record_list(&list_req, 0, 256 * 36 > PBUF_MAX ? PBUF_MAX / 36 * 36 : 256 * 36, pbuf);
    if (!list_h)
        say("Could not list the saved sheets");
}

/* The records listed: the sheets among them, in the Open panel. */
static void listed(void)
{
    uint32_t n = list_req.status == STATUS_DONE ? list_req.length / 36 : 0, i;
    list_h = 0;
    open_n = 0;
    for (i = 0; i < n && open_n < OPEN_MAX; i++) {
        const char *nm = (const char *)pbuf + i * 36;
        uint32_t k = 0;
        if (memcmp(nm, "sheet", 5) || (nm[5] && nm[5] != ':'))
            continue;
        if (!nm[5])
            memcpy(open_names[open_n], "(the sheet opened at start)", 28);
        else {
            for (k = 0; k < NAME_MAX && nm[6 + k] && 6 + k < 32; k++)
                open_names[open_n][k] = nm[6 + k];
            open_names[open_n][k] = 0;
        }
        open_n++;
    }
    menu_open(M_OPEN);
}

static void load_chunk(void);
static void begin_save(uint8_t how);

static void menu_choose(uint32_t i)
{
    if (menu_at == M_OPEN) {
        menu_close();
        if (!open_n)
            return;
        if (saving || save_h || put_h || pub_h || loading || parsing) {
            say("Busy: try again in a moment");
            return;
        }
        set_rec_name(open_names[i][0] == '(' ? "" : open_names[i]);
        from_arg = 0;
        loading = 1;
        flen = 0;
        load_chunk();
        return;
    }
    const mitem *it = &menus[menu_at].items[i];
    menu_close();
    key(it->key, it->mods);
}

static void prompt_start(void)
{
    if (!keeps) {
        say("No saving on this host");
        return;
    }
    if (editing)
        commit();
    prompting = 1;
    pr_n = 0;
    const char *n = sheet_name();
    while (n[pr_n] && pr_n < NAME_MAX) {
        pr_buf[pr_n] = n[pr_n];
        pr_n++;
    }
    pr_buf[pr_n] = 0;
    doc_dirty = 1;
}

/* A key while the name is asked for. */
static void prompt_key(uint8_t k)
{
    if (k == KEY_ESCAPE) {
        prompting = 0;
        say("Not saved");
    } else if (k == KEY_BACKSPACE && pr_n) {
        do
            pr_n--;
        while (pr_n && ((uint8_t)pr_buf[pr_n] & 0xC0) == 0x80);
        pr_buf[pr_n] = 0;
    } else if (k == KEY_ENTER) {
        if (!pr_n) {
            say("Type a name, or Esc");
            return;
        }
        prompting = 0;
        set_rec_name(pr_buf);
        begin_save(1);
    }
    doc_dirty = 1;
}

static void prompt_text(uint32_t cp)
{
    uint8_t b[4];
    uint32_t n = 1;
    if (cp < 0x20 || (cp >= 0x7F && cp < 0xA0))
        return;
    if (cp < 0x80)
        b[0] = (uint8_t)cp;
    else if (cp < 0x800) {
        b[0] = (uint8_t)(0xC0 | cp >> 6);
        b[1] = (uint8_t)(0x80 | (cp & 0x3F));
        n = 2;
    } else if (cp < 0x10000) {
        b[0] = (uint8_t)(0xE0 | cp >> 12);
        b[1] = (uint8_t)(0x80 | (cp >> 6 & 0x3F));
        b[2] = (uint8_t)(0x80 | (cp & 0x3F));
        n = 3;
    } else
        return;
    if (pr_n + n > NAME_MAX)
        return;
    memcpy(pr_buf + pr_n, b, n);
    pr_n += n;
    pr_buf[pr_n] = 0;
    doc_dirty = 1;
}

/* --- Input ---------------------------------------------------------------- */

static uint8_t buttons, shift_held, ctrl_held, alt_held, sel_drag;
static uint32_t frame, ev_frame, press_frame, press_r = 0xFFFF, press_c;

/* A paste (SPEC §events: the paste key's down, the text, its up): into
 * the entry being typed, ignored for the sheet's own copied cells, or
 * gathered and put into cells at its end. */
enum { P_NONE, P_TEXT, P_SKIP, P_EDIT };
static uint8_t pasting, paste_key;

static void paste_down(uint8_t k)
{
    if (pasting)
        return; /* the key repeating */
    paste_key = k;
    if (editing)
        pasting = P_EDIT;
    else if (clip_on) {
        paste();
        pasting = P_SKIP;
    } else {
        pasting = P_TEXT;
        pbuf_n = 0;
        pbuf_cut = 0;
    }
}

static void paste_text(uint32_t cp)
{
    if (pasting == P_EDIT)
        insert_cp(cp == '\t' || cp == '\n' ? ' ' : cp);
    else if (pasting == P_TEXT) {
        uint8_t b[4];
        uint32_t n = 1;
        if (cp < 0x80)
            b[0] = (uint8_t)cp;
        else if (cp < 0x800) {
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
        if ((cp < 0x20 && cp != '\t' && cp != '\n') || (cp >= 0x7F && cp < 0xA0))
            return;
        if (pbuf_n + n > PBUF_MAX) {
            pbuf_cut = 1;
            return;
        }
        memcpy(pbuf + pbuf_n, b, n);
        pbuf_n += n;
    }
}

static void paste_up(void)
{
    if (pasting == P_TEXT)
        paste_text_start();
    pasting = P_NONE;
}

static void key(uint8_t k, uint8_t mods)
{
    int shift = mods & MOD_SHIFT, ctrl = mods & MOD_CTRL;
    press_r = 0xFFFF; /* a key between two presses: not a double-click */
    /* The modifier keys themselves (a window sends them as keys too): only
     * the keys they modify count, but Shift is followed for Shift+click,
     * and Control so that the text of Ctrl+Shift+1 is not typed. */
    if (k >= 0xE0 && k <= 0xE7) {
        if (k == KEY_LSHIFT || k == KEY_RSHIFT)
            shift_held = 1;
        if (k == 0xE0 || k == 0xE4)
            ctrl_held = 1;
        if (k == 0xE2 || k == 0xE6)
            alt_held = 1;
        return;
    }
    if (!shift)
        shift_held = 0; /* its release was missed */
    if (!ctrl)
        ctrl_held = 0;
    if (!(mods & 0x04))
        alt_held = 0;
    if (prompting) {
        prompt_key(k);
        return;
    }
    if (menu_on) {
        menu_key(k);
        return;
    }
    if (menus_shown() && (k == KEY_F10 || ((mods & 0x04) && !ctrl))) {
        /* F10 opens File; Alt and a title's letter its menu (and the
         * letter's text, which a window may send too, is not typed). */
        for (uint32_t m = 0; m < MENUS; m++)
            if (k == KEY_F10 ? !m : k == menus[m].letter) {
                if (editing)
                    commit();
                menu_open(m);
                return;
            }
        return;
    }
    if ((ctrl && !(mods & 0x04) && k == KEY_V) || (shift && !ctrl && k == KEY_INSERT)) {
        paste_down(k);
        return;
    }
    if (editing) {
        if (ctrl && (k == KEY_S || k == KEY_P))
            commit();
        else {
            edit_key(k, shift, ctrl);
            return;
        }
    }
    if (ctrl) {
        if (k != KEY_N)
            new_armed = 0;
        switch (k) {
        case KEY_C: copy(0); return;
        case KEY_X: copy(1); return;
        case KEY_Z: undo(shift); return;
        case KEY_Y: undo(1); return;
        case KEY_A:
            if (shift) {
                sort_range(0);
                return;
            }
            sr = sc = 0;
            ar = NROWS - 1;
            ac = NCOLS - 1;
            doc_dirty = 1;
            return;
        case KEY_D:
            if (shift)
                sort_range(1);
            else
                fill(1);
            return;
        case KEY_EQUAL:
        case KEY_KP_PLUS: insert_delete(0); return;
        case KEY_MINUS:
        case KEY_KP_MINUS: insert_delete(1); return;
        case KEY_GRAVE:
            if (shift)
                set_format(FO_GENERAL);
            return;
        case KEY_1:
            if (shift)
                set_format(FO_NUMBER);
            return;
        case KEY_5:
            if (shift)
                set_format(FO_PERCENT);
            return;
        case KEY_PERIOD: set_format(FO_MORE); return;
        case KEY_COMMA: set_format(FO_FEWER); return;
        case KEY_R: fill(0); return;
        case KEY_G: set_chart(1); return;
        case KEY_L: set_chart(2); return;
        case KEY_S:
            if (shift)
                prompt_start();
            else
                begin_save(1);
            return;
        case KEY_O: open_list(); return;
        case KEY_P: begin_save(2); return;
        case KEY_N:
            if (!new_armed) {
                new_armed = 1;
                say("Press Ctrl+N again to clear the whole sheet");
                return;
            }
            new_armed = 0;
            clear_sheet();
            set_rec_name(""); /* saved as the sheet that opens at start, until Save as names it */
            say("A new sheet (File, Save as names it)");
            return;
        case KEY_HOME: move_to(0, 0, shift); return;
        case KEY_END: {
            /* The last row and column that hold anything. */
            int32_t r = 0, c = 0;
            for (uint32_t i = 0; i < NCELLS; i++)
                if (c_rlen[i]) {
                    r = (int32_t)(i / NCOLS);
                    if ((int32_t)(i % NCOLS) > c)
                        c = (int32_t)(i % NCOLS);
                }
            move_to(r, c, shift);
            return;
        }
        case KEY_UP: jump(-1, 0, shift); return;
        case KEY_DOWN: jump(1, 0, shift); return;
        case KEY_LEFT: jump(0, -1, shift); return;
        case KEY_RIGHT: jump(0, 1, shift); return;
        default: return;
        }
    }
    new_armed = 0;
    switch (k) {
    case KEY_UP: step(-1, 0, shift); return;
    case KEY_DOWN: step(1, 0, shift); return;
    case KEY_LEFT: step(0, -1, shift); return;
    case KEY_RIGHT: step(0, 1, shift); return;
    case KEY_TAB: tab_move(shift); return;
    case KEY_ENTER: enter_move(shift); return;
    case KEY_PAGE_UP: step(-(int32_t)vrows(), 0, shift); return;
    case KEY_PAGE_DOWN: step((int32_t)vrows(), 0, shift); return;
    case KEY_HOME: move_to((int32_t)(shift ? ar : sr), 0, shift); return;
    case KEY_F2: start_edit(1, M_EDIT); return;
    case KEY_F1:
        help_on = !help_on;
        show_selection();
        return;
    case KEY_DELETE:
    case KEY_BACKSPACE: clear_range(); return;
    case KEY_ESCAPE:
        ar = sr;
        ac = sc;
        if (clip_on) {
            clip_on = 0;
            vec_dirty = 1;
            say("Copy forgotten: Ctrl+V now pastes text from other programs");
        } else
            say("");
        return;
    default: return;
    }
}

/* What is under a point of the screen. */
enum { H_NONE, H_LINE, H_CORNER, H_COLUMN, H_ROW, H_CELL };

static int hit(uint32_t x, uint32_t y, uint32_t *r, uint32_t *c)
{
    uint32_t rows = vrows(), i;
    if (y < g_y[0])
        return H_LINE;
    if (y >= g_y[rows + 1] || x >= 640)
        return H_NONE;
    if (y < g_y[1]) {
        *c = left + x / COLW - 1;
        return x < COLW ? H_CORNER : H_COLUMN;
    }
    for (i = 0; i < rows && y >= g_y[2 + i]; i++)
        ;
    *r = top + i;
    *c = left + x / COLW - 1;
    return x < COLW ? H_ROW : H_CELL;
}

static void pointer(const terse_event *e)
{
    uint8_t b = EV_POINTER_BUTTONS(*e), was = buttons;
    uint32_t r = 0, c = 0;
    int h, dbl;
    buttons = b;
    if (!(b & BUTTON_PRIMARY)) {
        pt_drag = sel_drag = 0;
        return;
    }
    if (!(was & BUTTON_PRIMARY) && menus_shown() && (menu_on || EV_POINTER_Y(*e) < 16)) {
        if (editing && !menu_on)
            commit();
        menu_press(EV_POINTER_X(*e), EV_POINTER_Y(*e));
        press_r = 0xFFFF;
        return;
    }
    if (menu_on)
        return;
    h = hit(EV_POINTER_X(*e), EV_POINTER_Y(*e), &r, &c);
    if (was & BUTTON_PRIMARY) {
        /* A drag: over the range a formula points at, or the selection. */
        if (h != H_CELL)
            return;
        if (editing && pt_drag) {
            if (r != pr1 || c != pc1)
                point_set(pr0, pc0, r, c);
        } else if (!editing && sel_drag && (r != ar || c != ac))
            move_to((int32_t)r, (int32_t)c, 1);
        return;
    }
    /* A second press on the selected cell, soon after the first and with
     * no key between: a double-click. */
    dbl = h == H_CELL && r == press_r && c == press_c && r == sr && c == sc && ev_frame - press_frame <= DOUBLE_FRAMES;
    press_r = h == H_CELL ? r : 0xFFFF;
    press_c = c;
    press_frame = ev_frame;
    if (h == H_NONE)
        return;
    if (h == H_LINE) {
        /* The content line: edit there, the cursor at the end, or on the
         * character pressed (its anchor event follows). */
        if (!editing)
            start_edit(1, M_EDIT);
        emode = M_EDIT;
        ecaret = elen;
        pt_on = 0;
        doc_dirty = 1;
        return;
    }
    if (editing) {
        if (h == H_CELL && r == sr && c == sc) {
            /* The cell being edited: the cursor goes to the character
             * pressed (its anchor event follows), else stays. */
            emode = M_EDIT;
            pt_on = 0;
            doc_dirty = 1;
            return;
        }
        if (h == H_CELL && can_point()) {
            point_set(r, c, r, c);
            pt_drag = 1;
            press_r = 0xFFFF;
            return;
        }
        commit();
        dbl = 0;
    }
    switch (h) {
    case H_CORNER:
        sr = sc = 0;
        ar = NROWS - 1;
        ac = NCOLS - 1;
        break;
    case H_COLUMN:
        sr = 0;
        sc = ac = c;
        ar = NROWS - 1;
        break;
    case H_ROW:
        sr = ar = r;
        sc = 0;
        ac = NCOLS - 1;
        break;
    default:
        if (dbl) {
            start_edit(1, M_EDIT);
            return;
        }
        move_to((int32_t)r, (int32_t)c, shift_held);
        sel_drag = 1;
        return;
    }
    new_armed = 0;
    tab_col = -1;
    doc_dirty = 1;
}

/* A press on a character of the entry (content line or cell) puts the
 * text cursor before it. */
static void anchor_pressed(uint16_t id)
{
    uint32_t k;
    if (!editing)
        return;
    if (id >= A_LINE && id < A_LINE + RAW_MAX)
        k = id - A_LINE;
    else if (id >= A_CELL && id < A_CELL + RAW_MAX)
        k = id - A_CELL;
    else
        return;
    ecaret = k > elen ? elen : k;
    while (ecaret && ecaret < elen && (ebuf[ecaret] & 0xC0) == 0x80)
        ecaret--;
    emode = M_EDIT;
    pt_on = 0;
    doc_dirty = 1;
}

static int busy(void) { return loading || parsing || head_h || job; }

/* Input that arrives while the sheet is loading or a copy runs waits here,
 * with the frame it came in. */
#define HELD_MAX 128
static terse_event held[HELD_MAX];
static uint32_t held_f[HELD_MAX];
static uint32_t held_n, held_at;

static void input(const terse_event *ev)
{
    if (ev->type == EV_KEY_DOWN)
        key(ev->a, ev->b);
    else if (ev->type == EV_KEY_UP) {
        if (ev->a == KEY_LSHIFT || ev->a == KEY_RSHIFT)
            shift_held = 0;
        if (ev->a == 0xE0 || ev->a == 0xE4)
            ctrl_held = 0;
        if (ev->a == 0xE2 || ev->a == 0xE6)
            alt_held = 0;
        if (pasting && ev->a == paste_key)
            paste_up();
    } else if (ev->type == EV_TEXT) {
        uint32_t cp = EV_TEXT_CP(*ev);
        if (pasting) {
            paste_text(cp);
            return;
        }
        /* The text a window may send with Ctrl+Shift+1, Alt+F and the like
         * is not typing (AltGr, Control with Alt, still types). */
        if (ctrl_held != alt_held)
            return;
        if (prompting) {
            prompt_text(cp);
            return;
        }
        if (menu_on)
            return;
        if (cp == '\n' || cp == '\r') {
            /* A line break in pasted text: as Enter. */
            if (editing) {
                commit();
                enter_move(0);
            }
            return;
        }
        if (cp < 0x20 || (cp >= 0x7F && cp < 0xA0))
            return;
        if (!editing) {
            new_armed = 0;
            start_edit(0, M_ENTER);
        }
        insert_cp(cp);
    } else if (ev->type == EV_POINTER)
        pointer(ev);
    else if (ev->type == EV_PAGE_ANCHOR) {
        if (EV_ANCHOR_BUTTONS(*ev) & BUTTON_PRIMARY)
            anchor_pressed(EV_ANCHOR_ID(*ev));
    } else if (ev->type == EV_WHEEL) {
        int32_t t = (int32_t)top + 3 * EV_WHEEL_Y(*ev), max = NROWS - (int32_t)vrows();
        top = (uint32_t)(t < 0 ? 0 : t > max ? max : t);
        doc_dirty = 1;
    }
}

int main(void)
{
    page_region(0, 0, 640, 480);
    page_background(255, 255, 255, 255);
    /* The terminal is not used: its cursor would show over the page. */
    if (terse_has_device(2))
        term_cmd(TERM_CURSOR_OFF);
    has_vec = terse_has_device(3);
    has_term = terse_has_device(2);
    menu_layout();
    menu_dirty = 1;
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
            if (ev.type == EV_TEXT && (pasting == P_TEXT || pasting == P_SKIP) && held_at == held_n) {
                /* The text of a paste being gathered: at once, so that a
                 * long paste never fills the queue below. */
                paste_text(EV_TEXT_CP(ev));
                continue;
            }
            if (ev.type == EV_KEY_DOWN || ev.type == EV_KEY_UP || ev.type == EV_TEXT || ev.type == EV_POINTER || ev.type == EV_WHEEL
                || ev.type == EV_PAGE_ANCHOR) {
                /* Kept in order until the sheet is free for them. */
                if (held_n < HELD_MAX) {
                    held_f[held_n] = frame;
                    held[held_n++] = ev;
                }
            }
        }
        /* At most 8 a frame, as a host spreads a paste; the text of a
         * paste the sheet gathers is cheap and does not count. */
        for (uint32_t k = 0; k < 8 && held_at < held_n && !busy();) {
            if (!(pasting == P_TEXT || pasting == P_SKIP) || held[held_at].type != EV_TEXT)
                k++;
            ev_frame = held_f[held_at];
            input(&held[held_at++]);
        }
        if (held_at == held_n)
            held_at = held_n = 0;
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
        if (doc_dirty && !parsing && !loading && !job) {
            doc_dirty = 0;
            build_doc();
        }
        while (loc_at < loc_n && page_locate(loc_ids[loc_at]) == PAGE_DONE)
            loc_at++;
        if (vec_dirty && has_vec && !parsing && !loading) {
            vec_dirty = 0;
            build_vec();
        }
        if (menu_dirty && menus_shown())
            menu_draw();
        frame++;
        terse_wait_frame();
    }
}
