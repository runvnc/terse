/* reader.c: a feed reader for Terse (machine profile 1, RV32IMC).
 *
 * Shows the timeline of the feeds the host subscribes to (SPEC 9), newest
 * first, the subscribed feeds, a help page and articles (NETWORK 4.5: plain
 * UTF-8), all on the page device (SPEC 11) in the sans face. The terminal
 * shows only the status row (29: key hints, the reading position, the
 * outcome of a request) and, while a feed key is being typed, the entry row
 * (28). The rows under the page are blank, in the colour of the page's
 * background (xterm 234, (28, 28, 28)), which is opaque.
 *
 * Every frame it handles the queued events, rebuilds and submits the page's
 * document if what it shows changed, does a bounded share of the work in
 * hand (turning article bytes into the page document), draws the terminal
 * rows that changed and waits for the next frame.
 *
 * The lists (timeline and feeds). A program never learns where the page put
 * a line, only the height of the whole document, so a list is virtualised:
 * its document holds a header slot and the entries from `top` on that fit
 * on the screen, plus one more (partly shown), and it is submitted again
 * whenever the selection or `top` changes. Every slot has the same shape:
 * one no-wrap paragraph with SLOT_SPACE above it, a line of SLOT_TITLE text,
 * a line break and a line of SLOT_META text, no trailing break. A line's
 * height depends only on face and size, so a list document of s slots is s
 * times one slot high: the height event of a list document gives the slot
 * height e = H / s exactly, and floor(rectangle height / e) - 1 entries are
 * wholly visible. Until a height arrives e is estimated (1.35 x the sizes);
 * a height that is not a whole number of slots, or gives an implausible
 * slot, belongs to another document and is ignored. Titles are cut at the
 * rectangle's edge (no wrap); control characters and line or paragraph
 * separators in them become spaces, so a slot always has two lines. Each
 * entry is an anchor (its index + 1): a click on it opens it (timeline) or
 * selects it (feeds). The selection is shown by colour.
 *
 * The article view: the page's rectangle covers rows 0-28 (y 0-464), x
 * 64-576: 512 units, about 68 characters of 15-unit body text, centred.
 * The document is the title (26 units, bold), a grey line (feed key, entry
 * number, date, size), a rule, then the article. Its text, by this rule:
 *   - Lines end at LF. A line of only spaces, tabs and CRs is blank; blank
 *     lines separate blocks (any number of them, one break).
 *   - A block in which any line starts with a space or a tab is
 *     preformatted: one paragraph in the monospace face (13 units) that
 *     does not wrap, its LFs and leading spaces kept.
 *   - Any other block is a prose paragraph: its LFs become spaces and the
 *     host wraps it.
 *   - Every paragraph has 10 units of space above it. The LF after a
 *     block's last line is dropped; CRs are left for the device to ignore.
 *   - A link is "http://" or "https://" not preceded by a letter or digit,
 *     with at least one character after the "//", running to the first
 *     byte that is not printable ASCII or is <, > or a double quote; then
 *     trailing . , ; : ! ? ' are dropped, and a trailing ) or ] when the
 *     link has more of it than of its opening bracket. The first 512 links
 *     get anchors 1, 2, ... and the link colour; pressing one (event 09h)
 *     shows its URL in the status row (there is no browser).
 * The document holds at most 65,536 bytes (48 KiB of article plus
 * formatting); if the article is longer, or the formatting does not fit,
 * it ends where the bytes end, between characters, with a note saying how
 * much is shown. While the article is fetched (or if it cannot be), a
 * short document with its title and a message stands in.
 *
 * Building the document costs about 9 units per byte of prose (more for
 * short paragraphs and links), so it is spread over frames: each frame
 * spends at most BUILD_UNITS on it, by an estimate of the cost that is
 * never below the real one. The document is submitted at the end of the
 * first frame of building, partial if need be (so the first screenful
 * appears at once), and again, whole, when it is complete; the 48 KiB
 * fixture article takes 11 frames. The program keeps the scroll value
 * itself: Up/Down move one body line (LINE units), PgUp/PgDn/Space the
 * rectangle less a line, the wheel three lines a step, Home/End the ends.
 * It clamps the value to 0 .. height - 464 with the latest height event
 * (08h) and calls `scroll` whenever it changes, also after each submit
 * (which resets it). Until the first height arrives, moves are only kept
 * from going below 0, End is remembered (it also follows the height of the
 * whole document when it comes) and the status row shows no position. The
 * position is 0% at the top and 100% at the end, and not shown when the
 * document fits. The help page scrolls the same way.
 *
 * Feeds: the subscriptions (op 03h: key and highest seq), how many
 * timeline entries each has (counted by reading the whole timeline, one
 * request a frame) and a "you" mark on the host's own feed (op 08h). S
 * opens the entry row for a feed key (typed or pasted; whitespace is
 * ignored; Enter subscribes, op 05h), U unsubscribes from the selected
 * feed (op 06h). T (in every view but the article) publishes a test
 * article: put (02h), publish (07h), identity again (08h) and, if the own
 * feed is not subscribed, subscribe to it, so that the article shows in
 * the timeline. The host asks the user about each subscribe, unsubscribe
 * and publish; the status row says what came of it.
 *
 * Memory: everything is a static array in RAM (flat addresses): the 16 KiB
 * timeline buffer, a 16 KiB buffer for counting, 16 KiB of subscription
 * records, up to 48 KiB of the article (ART_CAP), the 64 KiB article
 * document and an 8 KiB document for the other views. The timeline buffer
 * holds one request's worth of records: 70 entries with 120-byte titles to
 * 147 with empty ones. */

#include <stdint.h>
#include "terse.h"

#define COLS TERM_COLS
#define ENTRY_ROW 28
#define STATUS_ROW 29

/* xterm palette indexes (SPEC 8.5). */
#define C_TEXT 252
#define C_DIM 244
#define C_BG 234 /* (28, 28, 28): the page's background */
#define C_BAR_FG 231
#define C_BAR_BG 24
#define C_BAR_DIM 109
#define C_ENTRY_BG 237
#define C_OK 120
#define C_ERROR 210
#define C_INFO 229

/* Timeline records (SPEC 9.5): L, flags, then the entry (NETWORK 4.2). */
#define R_FLAGS 1
#define R_FEED 2
#define R_SEQ 34
#define R_TIME 38
#define R_SIZE 43
#define R_BLOB 46
#define R_REPLY 78
#define R_N 110
#define R_TITLE 111

#define TL_BUF_LEN 16384
#define MAX_RECORDS 148 /* TL_BUF_LEN / 111, the smallest record */
#define MAX_SUBS 455    /* 16,384 / 36 */

#define ART_CAP 49152u
#define CHUNK 16384u

/* The page. Lists: a wide column; the article, help and the empty
 * timeline: a centred column 512 units wide. Both cover rows 0-28. */
#define PAGE_H 464
#define LIST_X 32
#define LIST_W 576
#define RECT_X 64
#define RECT_W 512
#define RECT_H PAGE_H
#define LINE 20 /* one line of body text: what Up and Down move */
#define TITLE_SIZE 26
#define META_SIZE 12
#define BODY_SIZE 15
#define PRE_SIZE 13
#define PRE_INDENT 0
#define PARA_SPACE 10
#define NOTE_SIZE 13
#define DOC_CAP 65536u
#define DOC_RESERVE 128 /* kept for the closing note and End */
#define LDOC_CAP 8192u
#define MAX_LINKS 512
/* List slots: sizes, the space above each and the estimate of its height
 * (1.35 x each size, rounded up) until the first height event; heights
 * that give a slot outside E_MIN .. E_MAX are not a list's. */
#define SLOT_TITLE 16
#define SLOT_META 12
#define SLOT_SPACE 8
#define E_GUESS (SLOT_SPACE + 22 + 17)
#define E_MIN (SLOT_SPACE + SLOT_TITLE + SLOT_META)
#define E_MAX (SLOT_SPACE + 3 * (SLOT_TITLE + SLOT_META))
#define MAX_VIS 24
/* Building the document is paid for from BUILD_UNITS estimated budget
 * units a frame, so that a frame that also draws every row and submits the
 * whole document ends well within its budget. The estimates (measured, and
 * rounded up): per byte looked at while finding blocks and while emitting
 * them (scanned a word at a time, and copied), per line, per LF or colon
 * met while emitting, per paragraph and per link. */
#define BUILD_UNITS 80000
#define W_FIND 3
#define W_EMIT 6
#define W_LINE 80
#define W_HIT 120
#define W_BLOCK 400
#define W_LINK 900

/* Keys (USB HID). */
#define KEY_F 0x09
#define KEY_S 0x16
#define KEY_T 0x17
#define KEY_U 0x18
#define KEY_F1 0x3A

enum { TIMELINE, FEEDS, HELP, ARTICLE };
enum { FETCHING, READY, NOT_FOUND, FAILED };

static uint8_t view;
static uint32_t now; /* Unix time of this frame; 0 if the host has none */
static terse_event ev;

static uint8_t dirty[TERM_ROWS];
/* The row being drawn: its cells, the column of the next character and the
 * colours it is drawn in. */
static uint32_t cells[COLS];
static uint8_t row_at, col;
static uint32_t attr; /* the colours, as in a cell: fg << 16 | bg << 24 */

/* The status message (the outcome of a request), shown instead of the key
 * hints until the next key; msg_n 0: none. */
static char msg[120];
static uint8_t msg_n, msg_fg;

/* --- Timeline ---------------------------------------------------------- */

static content_req tl_req, sub_req, cnt_req, id_req, op_req;
static uint8_t tl_handle, sub_handle, cnt_handle, id_handle, op_handle;
static uint8_t tl_buf[TL_BUF_LEN];
static uint16_t rec_at[MAX_RECORDS];
static uint16_t tl_base, tl_count, tl_total; /* window: tl_base .. +tl_count */
static uint16_t tl_req_base;
static uint8_t tl_loaded, tl_refresh, tl_again;
static uint8_t sel_id[36]; /* feed key and seq of the selection */
static uint8_t sel_id_ok;

/* --- Feeds ------------------------------------------------------------- */

static uint8_t sub_buf[MAX_SUBS * 36];
static uint16_t subs_n, subs_total;
static uint8_t subs_loaded, sub_again;
/* Entries per feed (same index as sub_buf), counted from cnt_pos on. */
static uint16_t counts[MAX_SUBS];
static uint8_t cnt_buf[TL_BUF_LEN];
static uint32_t cnt_pos;
static uint8_t cnt_done, cnt_again;
static uint8_t own_key[32], own_known;

/* The request that changes something (one at a time). */
enum { OP_IDLE, OP_PUT, OP_PUBLISH, OP_IDENT, OP_SELF_SUB, OP_SUB, OP_UNSUB };
static uint8_t op_state;
static uint8_t op_key[32];
static char test_title[24];
static uint8_t test_tlen;
static uint32_t test_seq, tests;

/* The feed key being typed: the non-blank characters (ASCII, anything else
 * as 0x80), how many there were. */
#define ENTRY_CAP 100
static uint8_t entry, entry_buf[ENTRY_CAP];
static uint16_t entry_n;

/* --- Lists ------------------------------------------------------------- */

typedef struct {
    uint16_t sel, top;
} listpos;
static listpos tlp, fdp;
static uint16_t slot_e = E_GUESS;
static uint16_t rect_h = PAGE_H;
static uint8_t last_slots; /* slots of the list document shown, 0 if not a list */
static uint8_t page_dirty; /* the view's document must be built and submitted again */
static uint8_t region_x;   /* the rectangle set: 0 none, else its x */
static uint16_t region_h;
static uint8_t ldoc[LDOC_CAP];

/* --- Article ----------------------------------------------------------- */

static content_req art_req;
static uint8_t art_handle, art_state;
static uint8_t art_title[121], art_tlen, art_feed[32], art_reply, art_local;
static uint32_t art_seq, art_time, art_size;
static uint8_t art_blob[32];
static uint16_t art_have, art_want;
static uint8_t art[ART_CAP];

/* The document and how far it is built (offsets into art). */
static uint8_t doc[DOC_CAP];
enum { B_FIND, B_EMIT, B_FINISH, B_DONE };
static uint8_t b_phase;
static uint16_t b_line;    /* B_FIND: start of the next line to look at */
static uint16_t b_scan;    /* B_FIND: where the search for its LF goes on */
static uint8_t b_in_line;  /* B_FIND: 0 at a line start, 1 in its leading blanks, 2 in its text */
static uint16_t b_lines;   /* lines in the block so far */
static uint16_t b_start, b_end; /* the block: its first byte, the end of its last line */
static uint8_t b_pre, b_was_pre;
static uint16_t b_emit;    /* B_EMIT: the next byte to look at */
static uint16_t b_text;    /* B_EMIT: the first byte not yet in the document */
static uint16_t b_shown;   /* the article bytes in the document */
static uint32_t b_work;    /* units left for this frame */
static const uint8_t *b_colour; /* the colour of the block's text */
static uint16_t link_at[MAX_LINKS], link_len[MAX_LINKS], links;

/* The scrolled views (article, help). */
static uint8_t doc_state; /* 0: nothing (of this view's) submitted; 1: a part; 2: all of it */
static uint32_t page_h;   /* the latest height (event 08h), if page_h_known */
static uint8_t page_h_known;
static int32_t scroll, scroll_sent;
static uint8_t to_end;    /* End was pressed: stay at the end as the height grows */
static uint16_t link_shown; /* the anchor whose URL the status row shows, 0 none */

static const uint8_t rgb_title[3] = {240, 240, 240};
static const uint8_t rgb_meta[3] = {140, 140, 140};
static const uint8_t rgb_reply[3] = {95, 175, 175};
static const uint8_t rgb_rule[3] = {70, 70, 70};
static const uint8_t rgb_body[3] = {218, 218, 218};
static const uint8_t rgb_pre[3] = {190, 205, 165};
static const uint8_t rgb_link[3] = {110, 170, 255};
static const uint8_t rgb_head[3] = {255, 255, 255};
static const uint8_t rgb_entry[3] = {225, 225, 225};
static const uint8_t rgb_sel[3] = {255, 200, 95};
static const uint8_t rgb_sel_meta[3] = {205, 170, 110};
static const uint8_t rgb_you[3] = {120, 210, 140};
static const uint8_t rgb_key[3] = {200, 215, 235};
static const uint8_t rgb_error[3] = {255, 135, 120};

/* --- Text -------------------------------------------------------------- */

/* Counts the printable ASCII bytes (20h-7Eh) at `p`, at most `max`. */
static uint32_t ascii_run(uint32_t max, const uint8_t *p)
{
    uint32_t n = 0;
    while (n < max && (uint8_t)(p[n] - 0x20) < 0x5F)
        n++;
    return n;
}

/* A title (valid UTF-8: the host verified the entry). */
static uint16_t title_char(const uint8_t **pp)
{
    const uint8_t *p = *pp;
    uint8_t b = *p++;
    uint16_t cp;
    if (b < 0x80) {
        cp = b;
    } else if (b < 0xE0) {
        cp = (uint16_t)(b & 0x1F) << 6 | (*p++ & 0x3F);
    } else if (b < 0xF0) {
        cp = (uint16_t)(b & 0x0F) << 12 | (uint16_t)(p[0] & 0x3F) << 6 | (p[1] & 0x3F);
        p += 2;
    } else {
        cp = 0xFFFD;
        p += 3;
    }
    *pp = p;
    return cp;
}

/* Control characters would move the cursor: show them as U+FFFD. */
static uint16_t printable(uint16_t cp)
{
    return cp < 0x20 || cp == 0x7F ? 0xFFFD : cp;
}

static uint8_t width(uint32_t cp)
{
    if (cp < 0x1100)
        return 1;
    /* The common cases first (as in the profile-1 library): CJK, kana,
     * Hangul, fullwidth forms. */
    if ((cp >= 0x3250 && cp <= 0xA48C) || (cp >= 0x3041 && cp <= 0x30FF && cp != 0x3097 && cp != 0x3098)
        || (cp >= 0xAC00 && cp <= 0xD7A3) || (cp >= 0xFF01 && cp <= 0xFF60))
        return 2;
    return term_is_wide(cp) ? 2 : 1;
}

/* Text being put together: a line of the page, a message, the test
 * article. */
static char tb[720];
static uint16_t tb_n;

static void tb_s(const char *s)
{
    while (*s && tb_n < sizeof tb)
        tb[tb_n++] = *s++;
}

static char num[11];

static const char *u32_str(uint32_t v)
{
    char *p = num + sizeof num - 1;
    *p = 0;
    do {
        *--p = '0' + v % 10;
        v /= 10;
    } while (v);
    return p;
}

static void tb_u(uint32_t v)
{
    tb_s(u32_str(v));
}

static void tb_two(uint32_t v)
{
    char s[3] = {'0' + v / 10, '0' + v % 10, 0};
    tb_s(s);
}

static void tb_hex(const uint8_t *b, uint8_t n)
{
    static const char hex[] = "0123456789abcdef";
    char s[3] = {0, 0, 0};
    uint8_t k;
    for (k = 0; k < n; k++) {
        s[0] = hex[b[k] >> 4];
        s[1] = hex[b[k] & 15];
        tb_s(s);
    }
}

/* A feed key's short form: its first and last 6 hex digits. */
static void tb_key(const uint8_t *k)
{
    tb_hex(k, 3);
    tb_s("\xE2\x80\xA6");
    tb_hex(k + 29, 3);
}

/* A size: "812 bytes", "6.1 KB", "1.0 MB" (KB = 1024 bytes). */
static void tb_size(uint32_t n)
{
    uint32_t tenths;
    const char *unit = " KB";
    if (n < 1024) {
        tb_u(n);
        tb_s(n == 1 ? " byte" : " bytes");
        return;
    }
    tenths = (n * 10 + 512) / 1024;
    if (tenths >= 10240) {
        tenths = (n / 1024 * 10 + 512) / 1024;
        unit = " MB";
    }
    tb_u(tenths / 10);
    tb_s(".");
    tb_u(tenths % 10);
    tb_s(unit);
}

static const char *plural(uint32_t n)
{
    return n == 1 ? "" : "s";
}

static const uint8_t month_days[12] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};

/* `t` as a UTC date and time: "YYYY-MM-DD HH:MM". The machine has a clock
 * but no time zone. Every fourth year is a leap year, which holds from 1901
 * to 2099. */
static void tb_date(uint32_t t)
{
    uint32_t days = t / 86400, secs = t % 86400, year, month = 0, leap, len;
    /* 1970-1973 is 365 + 365 + 366 + 365 days. */
    year = 1970 + days / 1461 * 4;
    days %= 1461;
    for (;;) {
        leap = (year & 3) == 0;
        if (days < 365 + leap)
            break;
        days -= 365 + leap;
        year++;
    }
    for (;;) {
        len = month_days[month] + (month == 1 && leap);
        if (days < len)
            break;
        days -= len;
        month++;
    }
    tb_u(year);
    tb_s("-");
    tb_two(month + 1);
    tb_s("-");
    tb_two(days + 1);
    tb_s(" ");
    tb_two(secs / 3600);
    tb_s(":");
    tb_two(secs % 3600 / 60);
}

static uint32_t get32(const uint8_t *p)
{
    return p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

/* The status message: tb, in colour fg. */
static void say(uint8_t fg)
{
    uint16_t n = tb_n > sizeof msg ? sizeof msg : tb_n;
    /* Cut between characters. */
    while (n < tb_n && n && ((uint8_t)tb[n] & 0xC0) == 0x80)
        n--;
    memcpy(msg, tb, n);
    msg_n = n;
    msg_fg = fg;
    dirty[STATUS_ROW] = 1;
}

/* --- Drawing ------------------------------------------------------------ */

static void row_start(uint8_t row, uint8_t f, uint8_t b)
{
    row_at = row;
    attr = (uint32_t)f << 16 | (uint32_t)b << 24;
    col = 0;
}

static void ink(uint8_t f)
{
    attr = (attr & 0xFF000000u) | (uint32_t)f << 16;
}

static void cell(uint32_t ch)
{
    if (col < COLS)
        cells[col++] = ch | attr;
}

/* A character known to be narrow. */
static void put1(uint16_t cp)
{
    cell(cp);
}

/* Any character: a wide one takes its cell and a continuation cell. U+FFFF
 * would be a continuation, so it shows as U+FFFD (as the write call shows
 * it). */
static void put_w(uint16_t cp, uint8_t w)
{
    if (cp == TERM_CONTINUATION)
        cp = 0xFFFD;
    cell(cp);
    if (w == 2)
        cell(TERM_CONTINUATION);
}

static void put(uint16_t cp)
{
    put_w(cp, width(cp));
}

/* `n` printable ASCII bytes; they fit in the row. */
static void put_ascii(const uint8_t *p, uint32_t n)
{
    uint32_t *c = cells + col, a = attr;
    col += n;
    while (n--)
        *c++ = *p++ | a;
}

/* A string from this program: UTF-8, no wide characters. */
static void puts_(const char *s)
{
    const uint8_t *p = (const uint8_t *)s;
    while (*p)
        cell(title_char(&p));
}

static void spaces(uint32_t n)
{
    uint32_t *c = cells + col, a = attr | ' ';
    if (n > COLS - col)
        n = COLS - col;
    col += n;
    while (n--)
        *c++ = a;
}

static void pad_to(uint8_t c)
{
    if (col < c)
        spaces(c - col);
}

/* Clears the rest of the row in the current colours and writes it. */
static void row_end(void)
{
    spaces(COLS - col);
    term_cells_write((uint32_t)row_at * COLS, cells, COLS);
}

static void put_u32(uint32_t v)
{
    puts_(u32_str(v));
}

/* UTF-8 text in at most `room` columns, with an ellipsis if cut. Text
 * never has more columns than bytes. */
static void put_title(const uint8_t *t, uint8_t n, uint8_t room)
{
    const uint8_t *p = t, *q, *end = t + n;
    uint32_t w = 0, cw, b, limit = n <= room ? room : room - 1;
    uint16_t cp;
    while (p < end) {
        b = limit - w;
        if ((uint32_t)(end - p) < b)
            b = end - p;
        b = ascii_run(b, p);
        put_ascii(p, b);
        p += b;
        w += b;
        if (p == end || w == limit)
            break;
        q = p;
        cp = printable(title_char(&q));
        cw = width(cp);
        if (w + cw > limit)
            break;
        put_w(cp, cw);
        w += cw;
        p = q;
    }
    if (p == end)
        return;
    /* Cut: unless just one character is left and it fits after all. */
    q = p;
    cp = printable(title_char(&q));
    if (q == end && w + width(cp) <= room)
        put(cp);
    else
        put1(0x2026);
}

/* --- The document ------------------------------------------------------ */

static void spend(uint32_t units)
{
    b_work = b_work > units ? b_work - units : 0;
}

static void doc_rgb(const uint8_t *c)
{
    doc_colour(c[0], c[1], c[2]);
}

/* Article bytes from b_text to `to` into the document as text. */
static void flush(uint32_t to)
{
    if (to > b_text && !doc_overflow)
        b_shown = b_text + doc_text(art + b_text, to - b_text);
    b_text = to;
}

static uint8_t is_alnum(uint8_t c)
{
    return (uint8_t)(c - '0') < 10 || (uint8_t)((c | 0x20) - 'a') < 26;
}

/* The length of the link at art[i] (an 'h'), 0 if none; the block ends at
 * `end`. See the rule at the top. */
static uint32_t link_length(uint32_t i, uint32_t end)
{
    const uint8_t *p = art + i;
    uint32_t n = end - i, k, head;
    int32_t paren = 0, bracket = 0;
    if (n < 8 || p[1] != 't' || p[2] != 't' || p[3] != 'p' || (i > b_start && is_alnum(p[-1])))
        return 0;
    k = p[4] == 's' ? 5 : 4;
    if (p[k] != ':' || p[k + 1] != '/' || p[k + 2] != '/')
        return 0;
    head = k += 3;
    for (; k < n; k++) {
        uint8_t c = p[k];
        if (c <= 0x20 || c >= 0x7F || c == '<' || c == '>' || c == '"')
            break;
        paren += (c == '(') - (c == ')');
        bracket += (c == '[') - (c == ']');
    }
    while (k > head) {
        uint8_t c = p[k - 1];
        if (c == ')' && paren < 0)
            paren++;
        else if (c == ']' && bracket < 0)
            bracket++;
        else if (c != '.' && c != ',' && c != ';' && c != ':' && c != '!' && c != '?' && c != '\'')
            break;
        k--;
    }
    return k > head ? k : 0;
}

/* A link of `len` bytes at art[i]: an anchor (the first MAX_LINKS) in the
 * link colour. */
static void put_link(uint32_t i, uint32_t len)
{
    b_text = i + len;
    spend(W_LINK + len * W_EMIT);
    if (doc_overflow)
        return;
    if (links < MAX_LINKS) {
        link_at[links] = i;
        link_len[links] = len;
        doc_anchor(++links);
        doc_rgb(rgb_link);
        b_shown = i + doc_text(art + i, len);
        doc_anchor(0);
        doc_rgb(b_colour);
    } else {
        b_shown = i + doc_text(art + i, len);
    }
}

/* The first byte in [p, e) that is `a` or `b`, or e: a word at a time
 * where it can (a word holds a byte equal to `a` if, with x = w ^ a * ONES,
 * (x - ONES) & ~x & HIGHS is not 0). */
#define ONES 0x01010101u
#define HIGHS 0x80808080u
static inline __attribute__((always_inline)) uint8_t *find2(uint8_t *p, const uint8_t *e, uint8_t a, uint8_t b)
{
    uint32_t ma = a * ONES, mb = b * ONES;
    while (p < e && ((uint32_t)p & 3)) {
        if (*p == a || *p == b)
            return p;
        p++;
    }
    while (e - p >= 4) {
        uint32_t w = *(const uint32_t *)p, x = w ^ ma, y = w ^ mb;
        if ((((x - ONES) & ~x) | ((y - ONES) & ~y)) & HIGHS)
            break;
        p += 4;
    }
    while (p < e && *p != a && *p != b)
        p++;
    return p;
}

/* The first LF in [p, e), or e. */
static uint8_t *find_lf(uint8_t *p, const uint8_t *e)
{
    while (p < e && ((uint32_t)p & 3)) {
        if (*p == '\n')
            return p;
        p++;
    }
    while (e - p >= 4) {
        uint32_t x = *(const uint32_t *)p ^ ('\n' * ONES);
        if ((x - ONES) & ~x & HIGHS)
            break;
        p += 4;
    }
    while (p < e && *p != '\n')
        p++;
    return p;
}

static uint8_t is_blank(uint8_t c)
{
    return c == ' ' || c == '\t' || c == '\r';
}

/* B_FIND: looks for the next block in art[0 .. limit) (`final`: no more
 * bytes are coming). Returns 1 with b_start, b_end and b_pre set, or 0 if
 * it needs more bytes or this frame's share is used; at the end of the
 * article it moves on to B_FINISH. Resumable: b_in_line is 0 at the start
 * of a line, 1 in its leading blanks, 2 looking for its LF (b_scan). */
static uint8_t find_block(uint32_t limit, uint8_t final)
{
    uint8_t *p;
    const uint8_t *e;
    for (;;) {
        if (!b_work)
            return 0;
        if (b_in_line == 0) {
            spend(W_LINE);
            b_scan = b_line;
            b_in_line = 1;
        }
        p = art + b_scan;
        e = art + limit;
        if ((uint32_t)(e - p) > b_work / W_FIND)
            e = p + b_work / W_FIND;
        if (b_in_line == 1) {
            uint32_t q;
            while (p < e && is_blank(*p))
                p++;
            spend((p - (art + b_scan)) * W_FIND);
            b_scan = q = p - art;
            if (q < limit && is_blank(art[q]))
                return 0; /* this frame's share is used */
            if (q == limit && !final)
                return 0;
            if (q == limit || art[q] == '\n') { /* a blank line, or the end */
                b_in_line = 0;
                b_line = q < limit ? q + 1 : q;
                if (b_lines)
                    return 1;
                if (q == limit) {
                    b_phase = B_FINISH;
                    return 0;
                }
                continue;
            }
            if (!b_lines) {
                b_start = b_line;
                b_pre = 0;
            }
            if (art[b_line] == ' ' || art[b_line] == '\t')
                b_pre = 1;
            b_lines++;
            b_in_line = 2;
            continue;
        }
        p = find_lf(p, e);
        spend((p - (art + b_scan)) * W_FIND);
        b_scan = p - art;
        if (b_scan < limit && *p != '\n')
            return 0; /* this frame's share is used */
        if (b_scan == limit && !final)
            return 0;
        b_end = b_scan;
        b_line = b_scan < limit ? b_scan + 1 : b_scan;
        b_in_line = 0;
    }
}

/* B_EMIT: starts the block's paragraph. */
static void start_block(void)
{
    spend(W_BLOCK);
    if (b_pre) {
        doc_font(DOC_MONO, 0);
        doc_size(PRE_SIZE);
        b_colour = rgb_pre;
        doc_rgb(b_colour);
        doc_paragraph(DOC_START, PRE_INDENT, PARA_SPACE, DOC_NOWRAP);
    } else {
        if (b_was_pre) {
            doc_font(DOC_SANS, 0);
            doc_size(BODY_SIZE);
            b_colour = rgb_body;
            doc_rgb(b_colour);
        }
        doc_paragraph(DOC_START, 0, PARA_SPACE, 0);
    }
    b_was_pre = b_pre;
    b_emit = b_text = b_start;
}

/* B_EMIT: the block's text, with LFs as spaces in prose and links marked.
 * Returns 1 when the block is in the document, 0 if this frame's share is
 * used (the text so far is added, cut between characters). */
static uint8_t emit_block(void)
{
    uint32_t end = b_end, i, k;
    uint8_t *p = art + b_emit, *e = art + end, *stop, *q;
    for (;;) {
        stop = (uint32_t)(e - p) > b_work / W_EMIT ? p + b_work / W_EMIT : e;
        q = find2(p, stop, '\n', ':');
        spend((q - p) * W_EMIT);
        p = q;
        if (p >= stop)
            break;
        spend(W_HIT);
        if (*p == '\n') {
            if (!b_pre)
                *p = ' ';
        } else {
            /* A link has "http:" or "https:" before its "//". */
            uint32_t c = p - art, at = c - 4, len;
            if (c >= b_text + 5 && art[c - 1] == 's')
                at = c - 5;
            if (at >= b_text && c >= b_text + 4 && art[at] == 'h' && (len = link_length(at, end))) {
                flush(at);
                put_link(at, len);
                p = art + at + len;
                continue;
            }
        }
        p++;
    }
    i = p - art;
    if (i >= end) {
        flush(end);
        return 1;
    }
    /* This frame's share is used: add the text so far, cut after a space
     * (so that no link is cut) or else at least between characters. */
    for (k = i; k > b_text && i - k < 64 && art[k - 1] > ' '; k--)
        ;
    if (k > b_text && art[k - 1] <= ' ')
        i = k;
    else
        while (i > b_text && (art[i] & 0xC0) == 0x80)
            i--;
    flush(i);
    b_emit = i;
    return 0;
}

static void send_scroll(void)
{
    if (scroll != scroll_sent) {
        page_scroll(scroll);
        scroll_sent = scroll;
    }
}

static void submit_doc(void)
{
    page_submit(doc, doc_length(doc));
    scroll_sent = 0; /* a submit sets the scroll value to 0 */
    dirty[STATUS_ROW] = 1;
}

/* B_FINISH: the closing note if not all of the article is shown, End, and
 * the whole document to the device. */
static void finish_doc(void)
{
    uint8_t cut = doc_overflow || art_have < art_size;
    doc_limit = doc + DOC_CAP;
    doc_overflow = 0;
    if (cut) {
        doc_paragraph(DOC_START, 0, 16, 0);
        doc_font(DOC_SANS, DOC_ITALIC);
        doc_size(NOTE_SIZE);
        doc_rgb(rgb_meta);
        tb_n = 0;
        tb_s("[\xE2\x80\xA6 the article continues: this reader shows ");
        tb_u(b_shown);
        tb_s(" of its ");
        tb_u(art_size);
        tb_s(" bytes]");
        doc_text(tb, tb_n);
    }
    doc_end();
    submit_doc();
    doc_state = 2;
    b_phase = B_DONE;
}

/* A frame's share of building the document; the first frame of it submits
 * what there is. */
static void build(void)
{
    uint32_t limit = art_have;
    uint8_t final = art_have >= art_want;
    if (art_state != READY || b_phase == B_DONE)
        return;
    if (final && art_have < art_size) {
        /* Cut short: end at a character, not in one. */
        uint32_t j = limit;
        while (j && limit - j < 3 && (art[j - 1] & 0xC0) == 0x80)
            j--;
        if (j) {
            uint8_t c = art[j - 1];
            uint32_t need = c >= 0xF0 ? 4 : c >= 0xE0 ? 3 : c >= 0xC0 ? 2 : 1;
            if (limit - (j - 1) < need)
                limit = j - 1;
        }
    }
    b_work = BUILD_UNITS;
    while (b_work && b_phase != B_FINISH) {
        if (doc_overflow) {
            b_phase = B_FINISH;
            break;
        }
        if (b_phase == B_FIND) {
            if (!find_block(limit, final))
                break;
            start_block();
            b_phase = B_EMIT;
        } else {
            if (!emit_block())
                break;
            b_lines = 0;
            b_phase = B_FIND;
        }
    }
    if (doc_overflow || b_phase == B_FINISH)
        finish_doc();
    else if (doc_state == 0) {
        submit_doc();
        doc_state = 1;
    }
}

/* --- Pages ------------------------------------------------------------- */

static void doc_s(const char *s)
{
    uint32_t n = 0;
    while (s[n])
        n++;
    doc_text(s, n);
}

/* Sets the rectangle (x RECT_X or LIST_X, rows 0 .. h) if it changed. */
static void region(uint8_t x, uint16_t h)
{
    if (region_x != x || region_h != h) {
        page_region(x, 0, x == LIST_X ? LIST_W : RECT_W, h);
        region_x = x;
        region_h = h;
    }
}

static void ldoc_begin(void)
{
    doc_begin(ldoc, LDOC_CAP - 1); /* the last byte for End */
}

static void ldoc_submit(void)
{
    doc_limit = ldoc + LDOC_CAP;
    doc_overflow = 0;
    doc_end();
    page_submit(ldoc, doc_length(ldoc));
}

/* A list document of `slots` slots, in the list's rectangle. */
static void list_submit(uint8_t slots)
{
    region(LIST_X, rect_h);
    ldoc_submit();
    last_slots = slots;
}

/* Any other document, in the centred column. */
static void column_submit(void)
{
    region(RECT_X, PAGE_H);
    ldoc_submit();
    last_slots = 0;
}

/* A slot (see the top): its paragraph and first line's font, colour and
 * anchor; slot_meta ends the first line and starts the second; slot_end
 * ends the anchor. */
static void slot_begin(const uint8_t *rgb, uint8_t flags, uint16_t anchor)
{
    doc_paragraph(DOC_START, 0, SLOT_SPACE, DOC_NOWRAP);
    doc_font(DOC_SANS, flags);
    doc_size(SLOT_TITLE);
    doc_rgb(rgb);
    if (anchor)
        doc_anchor(anchor);
}

static void slot_meta(const uint8_t *rgb)
{
    doc_text("\n", 1);
    doc_font(DOC_SANS, 0);
    doc_size(SLOT_META);
    doc_rgb(rgb);
}

static void slot_end(uint16_t anchor)
{
    if (anchor)
        doc_anchor(0);
}

/* A title on one line: control characters and line and paragraph
 * separators (U+2028, U+2029) become spaces; no title is "Untitled". */
static void doc_title(const uint8_t *t, uint8_t n)
{
    uint8_t buf[120], k = 0, i, c;
    if (!n) {
        doc_s("Untitled");
        return;
    }
    for (i = 0; i < n; i++) {
        c = t[i];
        if (c < 0x20 || c == 0x7F) {
            c = ' ';
        } else if (c == 0xE2 && i + 2 < n && t[i + 1] == 0x80 && (t[i + 2] == 0xA8 || t[i + 2] == 0xA9)) {
            c = ' ';
            i += 2;
        }
        buf[k++] = c;
    }
    doc_text(buf, k);
}

/* --- Lists ------------------------------------------------------------- */

/* Entries wholly visible below the header slot. */
static uint16_t vis(void)
{
    uint16_t n = rect_h / slot_e;
    n = n >= 2 ? n - 1 : 1;
    return n > MAX_VIS ? MAX_VIS : n;
}

/* Keeps the selection in the list and visible, and the screen filled. */
static void lp_keep(listpos *p, uint16_t total)
{
    uint16_t v = vis();
    if (!total) {
        p->sel = p->top = 0;
        return;
    }
    if (p->sel >= total)
        p->sel = total - 1;
    if (p->sel < p->top)
        p->top = p->sel;
    if (p->sel >= p->top + v)
        p->top = p->sel - v + 1;
    if (p->top + v > total)
        p->top = total > v ? total - v : 0;
}

static void lp_set(listpos *p, uint16_t total, int32_t sel, int32_t top)
{
    listpos old = *p;
    p->sel = sel < 0 ? 0 : sel >= total ? (total ? total - 1 : 0) : sel;
    p->top = top < 0 ? 0 : top > 0xFFFF ? 0xFFFF : top;
    lp_keep(p, total);
    if (old.sel != p->sel || old.top != p->top)
        page_dirty = 1;
}

/* Up, Down, PgUp, PgDn (the selection keeps its place on the screen unless
 * the list cannot scroll further, when it moves instead), Home, End. */
static uint8_t lp_key(listpos *p, uint16_t total, uint8_t code)
{
    int32_t s = p->sel, t = p->top, v = vis(), max = total > v ? total - v : 0, d, nt;
    switch (code) {
    case KEY_UP: s--; break;
    case KEY_DOWN: s++; break;
    case KEY_PAGE_UP:
    case KEY_PAGE_DOWN:
        d = code == KEY_PAGE_UP ? -v : v;
        nt = t + d < 0 ? 0 : t + d > max ? max : t + d;
        if (nt == t) {
            s += d;
        } else {
            s += nt - t;
            t = nt;
        }
        break;
    case KEY_HOME: s = 0; break;
    case KEY_END: s = total; break;
    default: return 0;
    }
    lp_set(p, total, s, t);
    return 1;
}

/* The wheel scrolls three entries a step; the selection stays on screen. */
static void lp_wheel(listpos *p, uint16_t total, int8_t steps)
{
    int32_t v = vis(), max = total > v ? total - v : 0, t = p->top + steps * 3, s = p->sel;
    t = t < 0 ? 0 : t > max ? max : t;
    if (s < t)
        s = t;
    if (s > t + v - 1)
        s = t + v - 1;
    lp_set(p, total, s, t);
}

/* A height event in a list view: the slot height, if it is a list's. */
static void list_height(uint32_t h)
{
    uint16_t e, before;
    if (!last_slots || h % last_slots)
        return;
    e = h / last_slots;
    if (e < E_MIN || e > E_MAX || e == slot_e)
        return;
    before = vis();
    slot_e = e;
    if (vis() != before)
        page_dirty = 1;
    lp_keep(&tlp, tl_total);
    lp_keep(&fdp, subs_n);
}

/* --- Timeline ------------------------------------------------------------ */

static uint8_t in_window(uint16_t i)
{
    return i >= tl_base && i - tl_base < tl_count;
}

static uint8_t *record(uint16_t i)
{
    return tl_buf + rec_at[i - tl_base];
}

static void record_id(const uint8_t *r, uint8_t *id)
{
    memcpy(id, r + R_FEED, 36); /* the feed key and seq */
}

static void load_timeline(uint16_t first)
{
    tl_req_base = first;
    tl_handle = content_feed(&tl_req, CONTENT_TIMELINE, first, TL_BUF_LEN, tl_buf);
}

static void count_request(void)
{
    cnt_handle = content_feed(&cnt_req, CONTENT_TIMELINE, cnt_pos, TL_BUF_LEN, cnt_buf);
}

/* Counts each feed's entries in the timeline again. */
static void start_count(void)
{
    if (cnt_handle) {
        cnt_again = 1; /* when this one is in: its buffer is in use */
        return;
    }
    memset(counts, 0, sizeof counts);
    cnt_pos = 0;
    cnt_done = 0;
    count_request();
}

static void load_subscriptions(void)
{
    if (sub_handle) {
        sub_again = 1;
        return;
    }
    sub_handle = content_feed(&sub_req, CONTENT_SUBSCRIPTIONS, 0, sizeof sub_buf, sub_buf);
}

/* The subscriptions and the timeline again (after a change). */
static void reload(void)
{
    load_subscriptions();
    if (tl_handle) {
        tl_again = 1; /* reload once this load is in */
        return;
    }
    tl_refresh = 1;
    load_timeline(tl_base);
}

static void timeline_done(uint8_t status)
{
    uint32_t at = 0, n = 0, total, i;
    uint16_t old_total = tl_total;
    uint8_t id[36];
    tl_handle = 0;
    if (status == STATUS_DONE) {
        while (at < tl_req.length && n < MAX_RECORDS) {
            rec_at[n++] = at;
            at += 1 + tl_buf[at];
        }
        total = tl_req.size;
        tl_base = tl_req_base;
        tl_count = n;
        tl_total = total > 0xFFFF ? 0xFFFF : total;
        if (tl_refresh && tl_loaded) {
            /* New entries arrive at index 0 (SPEC 9.5): the selection keeps
             * its entry, found by its identity or else moved down by the
             * arrivals. */
            if (tl_total > old_total)
                tlp.sel += tl_total - old_total;
            for (i = 0; sel_id_ok && i < tl_count; i++) {
                record_id(tl_buf + rec_at[i], id);
                if (memcmp(id, sel_id, 36) == 0) {
                    tlp.sel = tl_base + i;
                    break;
                }
            }
        }
        tl_refresh = 0;
        tl_loaded = 1;
        lp_keep(&tlp, tl_total);
        page_dirty = 1;
    }
    if (tl_again) {
        tl_again = 0;
        tl_refresh = 1;
        load_timeline(tl_base);
    }
}

/* Loads the part of the timeline on screen if the window does not hold it. */
static void ensure_window(void)
{
    uint16_t last = tlp.top + vis();
    if (tl_handle || !tl_loaded || tl_total == 0)
        return;
    if (last >= tl_total)
        last = tl_total - 1;
    if (!in_window(tlp.top) || !in_window(last))
        load_timeline(tlp.top > 32 ? tlp.top - 32 : 0);
}

/* The subscription record of `key`, or -1 (records are in key order). */
static int32_t find_sub(const uint8_t *key)
{
    int32_t lo = 0, hi = (int32_t)subs_n - 1, mid, c;
    while (lo <= hi) {
        mid = (lo + hi) >> 1;
        c = memcmp(sub_buf + mid * 36, key, 32);
        if (!c)
            return mid;
        if (c < 0)
            lo = mid + 1;
        else
            hi = mid - 1;
    }
    return -1;
}

static int32_t own_index(void)
{
    return own_known ? find_sub(own_key) : -1;
}

static void subs_done(uint8_t status)
{
    sub_handle = 0;
    if (status == STATUS_DONE) {
        subs_n = sub_req.length / 36;
        subs_total = sub_req.size;
        subs_loaded = 1;
        lp_keep(&fdp, subs_n);
        if (view == FEEDS)
            start_count();
        page_dirty = 1;
    }
    if (sub_again) {
        sub_again = 0;
        load_subscriptions();
    }
}

static void count_done(uint8_t status)
{
    uint32_t at = 0, n = 0;
    int32_t k;
    cnt_handle = 0;
    if (cnt_again) {
        cnt_again = 0;
        start_count();
        return;
    }
    if (status == STATUS_DONE) {
        while (at < cnt_req.length) {
            k = find_sub(cnt_buf + at + R_FEED);
            if (k >= 0)
                counts[k]++;
            n++;
            at += 1 + cnt_buf[at];
        }
        cnt_pos += n;
        if (n && cnt_pos < cnt_req.size) {
            count_request();
            return;
        }
    }
    cnt_done = 1;
    if (view == FEEDS)
        page_dirty = 1;
}

/* The header slot: a title and a line of details. */
static void header_slot(const char *title)
{
    slot_begin(rgb_head, DOC_BOLD, 0);
    doc_s(title);
    slot_meta(rgb_meta);
    doc_text(tb, tb_n);
}

/* The friendly page for an empty timeline. */
static void empty_doc(void)
{
    ldoc_begin();
    doc_paragraph(DOC_START, 0, 48, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(TITLE_SIZE);
    doc_rgb(rgb_title);
    doc_s("No articles yet");
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_font(DOC_SANS, 0);
    doc_size(BODY_SIZE);
    doc_rgb(rgb_body);
    doc_s("The timeline shows the articles of the feeds this host subscribes to, newest first. "
          "New articles appear here as they arrive.");
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_s("Press F to see your feeds and subscribe to one by its key.");
    doc_paragraph(DOC_START, 0, 10, 0);
    doc_s("Press T to publish a test article of your own; it shows up here.");
    doc_paragraph(DOC_START, 0, 10, 0);
    doc_s("Press ? for help, Esc to quit.");
    column_submit();
}

static void timeline_doc(void)
{
    uint16_t i, end, slots = 1;
    uint8_t *r, k, reply, s;
    if (!tl_loaded)
        return;
    if (!tl_total) {
        empty_doc();
        return;
    }
    ldoc_begin();
    tb_n = 0;
    tb_u(tl_total);
    tb_s(" article");
    tb_s(plural(tl_total));
    tb_s("  \xC2\xB7  ");
    tb_u(subs_total);
    tb_s(" feed");
    tb_s(plural(subs_total));
    header_slot("Timeline");
    end = tlp.top + vis() + 1;
    if (end > tl_total)
        end = tl_total;
    for (i = tlp.top; i < end; i++, slots++) {
        s = i == tlp.sel;
        if (!in_window(i)) {
            slot_begin(rgb_meta, 0, 0);
            doc_s("\xE2\x80\xA6");
            slot_meta(rgb_meta);
            doc_s("loading");
            continue;
        }
        r = record(i);
        if (s) {
            record_id(r, sel_id);
            sel_id_ok = 1;
        }
        slot_begin(s ? rgb_sel : rgb_entry, 0, i + 1);
        doc_title(r + R_TITLE, r[R_N]);
        slot_meta(s ? rgb_sel_meta : rgb_meta);
        tb_n = 0;
        tb_key(r + R_FEED);
        tb_s("  \xC2\xB7  ");
        tb_date(get32(r + R_TIME));
        tb_s("  \xC2\xB7  ");
        tb_size(get24(r + R_SIZE));
        for (k = reply = 0; k < 32; k++)
            reply |= r[R_REPLY + k];
        if (reply)
            tb_s("  \xC2\xB7  \xE2\x86\xB3 reply");
        if (!(r[R_FLAGS] & TIMELINE_LOCAL))
            tb_s("  \xC2\xB7  not downloaded");
        doc_text(tb, tb_n);
        slot_end(i + 1);
    }
    list_submit(slots);
    dirty[STATUS_ROW] = 1; /* the position */
}

static void feeds_doc(void)
{
    uint16_t i, end, slots = 1;
    int32_t own = own_index();
    const uint8_t *r;
    uint32_t seq;
    uint8_t s;
    ldoc_begin();
    tb_n = 0;
    tb_u(subs_total);
    tb_s(" subscription");
    tb_s(plural(subs_total));
    if (own_known) {
        tb_s("  \xC2\xB7  your key ");
        tb_key(own_key);
    } else {
        tb_s("  \xC2\xB7  no key of your own yet");
    }
    header_slot("Feeds");
    if (!subs_n) {
        slot_begin(rgb_entry, 0, 0);
        doc_s("No subscriptions yet");
        slot_meta(rgb_meta);
        doc_s("Press S to subscribe to a feed by its key, or T to publish a test article");
        slots++;
    }
    end = fdp.top + vis() + 1;
    if (end > subs_n)
        end = subs_n;
    for (i = fdp.top; i < end; i++, slots++) {
        r = sub_buf + i * 36;
        s = i == fdp.sel;
        slot_begin(s ? rgb_sel : rgb_entry, 0, i + 1);
        tb_n = 0;
        tb_key(r);
        doc_text(tb, tb_n);
        if (i == own) {
            doc_rgb(rgb_you);
            doc_s("  you");
        }
        slot_meta(s ? rgb_sel_meta : rgb_meta);
        tb_n = 0;
        seq = get32(r + 32);
        if (seq) {
            tb_s("highest #");
            tb_u(seq);
        } else {
            tb_s("no entries yet");
        }
        tb_s("  \xC2\xB7  ");
        if (cnt_done) {
            tb_u(counts[i]);
            tb_s(" article");
            tb_s(plural(counts[i]));
            tb_s(" held");
        } else {
            tb_s("counting\xE2\x80\xA6");
        }
        doc_text(tb, tb_n);
        slot_end(i + 1);
    }
    list_submit(slots);
}

/* --- Help -------------------------------------------------------------- */

static void help_head(const char *s)
{
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(17);
    doc_rgb(rgb_head);
    doc_s(s);
}

static void help_line(const char *keys, const char *what)
{
    doc_paragraph(DOC_START, 12, 4, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(14);
    doc_rgb(rgb_sel);
    doc_s(keys);
    doc_font(DOC_SANS, 0);
    doc_rgb(rgb_body);
    doc_s("   ");
    doc_s(what);
}

static const char *const help_keys[] = {
    0, "Timeline",
    "\xE2\x86\x91 \xE2\x86\x93  PgUp PgDn  Home End  wheel", "move the selection",
    "Enter, click", "open the article",
    "F", "feeds",
    "? or F1", "this help",
    "T", "publish a test article",
    "Esc", "quit",
    0, "Article",
    "\xE2\x86\x91 \xE2\x86\x93  PgUp PgDn Space  Home End  wheel", "scroll",
    "Click a link", "show its address",
    "Esc, Backspace, \xE2\x86\x90", "back to the timeline",
    0, "Feeds",
    "\xE2\x86\x91 \xE2\x86\x93  PgUp PgDn  Home End  wheel, click", "select a feed",
    "S", "subscribe: type or paste a feed key, then Enter (Esc cancels)",
    "U", "unsubscribe from the selected feed",
    "T, ? or F1", "as in the timeline",
    "Esc, Backspace, \xE2\x86\x90", "back to the timeline",
    0, "Help",
    "T", "publish a test article, and subscribe to your own feed so that it shows in the timeline",
    "\xE2\x86\x91 \xE2\x86\x93  PgUp PgDn  Home End  wheel", "scroll",
    "F", "feeds",
    "Esc, Backspace, \xE2\x86\x90", "back to the timeline",
};

/* The help page: your feed key, then the keys of every view. */
static void help_doc(void)
{
    uint8_t g;
    uint16_t i;
    ldoc_begin();
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(24);
    doc_rgb(rgb_title);
    doc_s("Help");
    help_head("Your feed key");
    doc_font(DOC_SANS, 0);
    doc_size(BODY_SIZE);
    doc_rgb(rgb_body);
    if (own_known) {
        /* Two lines of four groups of eight hex digits; a key pasted into
         * the subscribe entry may hold spaces and line breaks. */
        doc_paragraph(DOC_START, 12, 8, DOC_NOWRAP);
        doc_font(DOC_MONO, 0);
        doc_rgb(rgb_key);
        tb_n = 0;
        for (g = 0; g < 8; g++) {
            tb_hex(own_key + g * 4, 4);
            tb_s(g == 3 ? "\n" : g == 7 ? "" : " ");
        }
        doc_text(tb, tb_n);
        doc_paragraph(DOC_START, 0, 8, 0);
        doc_font(DOC_SANS, 0);
        doc_size(13);
        doc_rgb(rgb_meta);
        doc_s("Others subscribe to your feed with this key: in this reader, F, then S, then paste it.");
    } else {
        doc_paragraph(DOC_START, 0, 6, 0);
        doc_s("This host has no feed key yet. Press T to publish a test article: that creates one.");
    }
    for (i = 0; i < sizeof help_keys / sizeof help_keys[0]; i += 2) {
        if (help_keys[i])
            help_line(help_keys[i], help_keys[i + 1]);
        else
            help_head(help_keys[i + 1]);
    }
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_font(DOC_SANS, 0);
    doc_size(13);
    doc_rgb(rgb_meta);
    doc_s("Subscribing, unsubscribing and publishing need your consent: the host asks you with its own "
          "prompt, and the status row says what came of it.");
    column_submit();
    doc_state = 2;
    page_h_known = 0;
    scroll = scroll_sent = 0;
    to_end = 0;
}

/* --- Article ----------------------------------------------------------- */

/* The title, the details line and the rule. */
static void doc_head(void)
{
    doc_paragraph(DOC_START, 0, 14, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(TITLE_SIZE);
    doc_rgb(rgb_title);
    if (art_tlen)
        doc_text(art_title, art_tlen);
    else
        doc_s("Untitled");
    doc_paragraph(DOC_START, 0, 6, 0);
    doc_font(DOC_SANS, 0);
    doc_size(META_SIZE);
    doc_rgb(rgb_meta);
    tb_n = 0;
    tb_key(art_feed);
    tb_s("  \xC2\xB7  #");
    tb_u(art_seq);
    tb_s("  \xC2\xB7  ");
    tb_date(art_time);
    tb_s(" UTC  \xC2\xB7  ");
    tb_size(art_size);
    doc_text(tb, tb_n);
    if (art_reply) {
        doc_rgb(rgb_reply);
        doc_s("  \xE2\x86\xB3 reply");
    }
    doc_paragraph(DOC_START, 0, 10, 0);
    doc_rgb(rgb_rule);
    doc_rule();
    doc_size(BODY_SIZE);
    doc_rgb(rgb_body);
    b_colour = rgb_body;
    b_was_pre = 0;
}

/* The article's head and a message, while there is no article text. */
static void article_message(const char *text, const uint8_t *rgb, uint8_t back)
{
    ldoc_begin();
    doc_head();
    doc_paragraph(DOC_START, 0, PARA_SPACE, 0);
    doc_rgb(rgb);
    doc_s(text);
    if (back) {
        doc_paragraph(DOC_START, 0, PARA_SPACE, 0);
        doc_rgb(rgb_meta);
        doc_s("Esc: back to the timeline");
    }
    column_submit();
}

static uint32_t max_scroll(void)
{
    return page_h > RECT_H ? page_h - RECT_H : 0;
}

/* Sets the scroll value, kept from going below 0 and, once the height is
 * known, past the end. */
static void scroll_to(int32_t s)
{
    if (s < 0)
        s = 0;
    if (page_h_known && (uint32_t)s > max_scroll())
        s = max_scroll();
    if (s != scroll)
        dirty[STATUS_ROW] = 1;
    scroll = s;
}

static void scroll_by(int32_t delta)
{
    to_end = 0;
    scroll_to(scroll + delta);
}

static void page_laid_out(uint32_t h)
{
    /* Before the first submit (a height left from another view) or 0 (no
     * document): not ours. */
    if (!doc_state || !h)
        return;
    page_h = h;
    page_h_known = 1;
    scroll_to(to_end ? (int32_t)max_scroll() : scroll);
    dirty[STATUS_ROW] = 1;
}

static void request_chunk(void)
{
    uint32_t n = art_want - art_have;
    if (n > CHUNK)
        n = CHUNK;
    art_handle = content_get(&art_req, art_blob, art_have, n, art + art_have);
}

static void open_article(uint16_t i)
{
    uint8_t *r, k;
    if (i >= tl_total || !in_window(i) || art_handle)
        return;
    r = record(i);
    art_tlen = r[R_N];
    memcpy(art_title, r + R_TITLE, art_tlen);
    memcpy(art_feed, r + R_FEED, 32);
    memcpy(art_blob, r + R_BLOB, 32);
    art_reply = 0;
    for (k = 0; k < 32; k++)
        art_reply |= r[R_REPLY + k];
    art_local = r[R_FLAGS] & TIMELINE_LOCAL;
    art_seq = get32(r + R_SEQ);
    art_time = get32(r + R_TIME);
    art_size = get24(r + R_SIZE);
    art_want = art_size > ART_CAP ? ART_CAP : art_size;
    art_have = 0;
    art_state = FETCHING;
    request_chunk();
    view = ARTICLE;
    article_message(art_local ? "Reading the article\xE2\x80\xA6" : "Fetching the article from the relay\xE2\x80\xA6", rgb_meta, 0);
    /* The document: its head now, the article as it arrives. */
    doc_begin(doc, DOC_CAP - DOC_RESERVE);
    doc_head();
    b_phase = B_FIND;
    b_line = b_lines = b_in_line = 0;
    b_shown = links = 0;
    doc_state = 0;
    page_h = page_h_known = 0;
    scroll = scroll_sent = 0;
    to_end = 0;
    link_shown = 0;
    dirty[STATUS_ROW] = 1;
}

static void article_done(uint8_t status)
{
    uint32_t size;
    const char *m;
    art_handle = 0;
    if (status == STATUS_NOT_FOUND || status == STATUS_FAILED || status == STATUS_BAD_REQUEST) {
        art_state = status == STATUS_NOT_FOUND ? NOT_FOUND : FAILED;
        if (view != ARTICLE)
            return;
        m = art_state == NOT_FOUND ? "Not found: neither this host nor its relay has the article."
                                   : "Could not fetch the article (no relay, network error or timeout).";
        if (doc_state == 0) {
            article_message(m, rgb_error, 1);
        } else {
            tb_n = 0;
            tb_s(m);
            say(C_ERROR);
        }
        dirty[STATUS_ROW] = 1;
        return;
    }
    size = art_req.size;
    art_size = size;
    if (size < art_want)
        art_want = size;
    art_have += art_req.length;
    if (art_have < art_want && art_req.length)
        request_chunk();
    else
        art_want = art_have;
    art_state = READY;
}

/* How far through the page the rectangle is: 0% at the top, 100% at the
 * end (only when the page does not fit). */
static uint32_t percent(void)
{
    int32_t s = scroll;
    uint32_t m = max_scroll();
    if (s < 0)
        s = 0;
    if ((uint32_t)s > m)
        s = m;
    return (uint32_t)s * 100 / m;
}

/* --- Requests that change something ------------------------------------- */

static void op_start(uint8_t state, uint8_t handle)
{
    op_handle = handle;
    op_state = handle ? state : OP_IDLE;
}

static uint8_t op_busy(void)
{
    if (!op_state)
        return 0;
    tb_n = 0;
    tb_s("Busy: the previous request is not answered yet");
    say(C_ERROR);
    return 1;
}

static void say_key(const char *before, const uint8_t *key, const char *after, uint8_t fg)
{
    tb_n = 0;
    tb_s(before);
    tb_key(key);
    tb_s(after);
    say(fg);
}

static void published(void)
{
    tb_n = 0;
    tb_s("Published \xE2\x80\x9C");
    tb_s(test_title);
    tb_s("\xE2\x80\x9D as #");
    tb_u(test_seq);
}

/* T: put a short sample article (it differs with the frame and time),
 * then publish it (op_done carries on). */
static void test_publish(void)
{
    int32_t k;
    uint32_t n, frame = terse_frame();
    if (op_busy())
        return;
    k = own_index();
    n = k >= 0 ? get32(sub_buf + k * 36 + 32) + 1 : tests + 1;
    tb_n = 0;
    tb_s("Test article ");
    tb_u(n);
    memcpy(test_title, tb, tb_n);
    test_title[tb_n] = 0;
    test_tlen = tb_n;
    tb_n = 0;
    tb_s("This is a test article, published from the Terse reader to check that publishing works. "
         "It was written in frame ");
    tb_u(frame);
    if (now) {
        tb_s(", at ");
        tb_date(now);
        tb_s(" UTC");
    }
    tb_s(".\n\nA second paragraph gives it some shape. The reader splits an article into paragraphs at "
         "blank lines and wraps each one to the width of the page, but keeps the lines of a block\n"
         "that has indented lines:\n\n    frame  ");
    tb_u(frame);
    tb_s("\n    time   ");
    tb_u(now);
    tb_s("\n\nThe last paragraph has a link, https://example.com/terse, which shows its address when "
         "pressed.\n");
    op_start(OP_PUT, content_put(&op_req, tb, tb_n));
    tb_n = 0;
    tb_s("Publishing \xE2\x80\x9C");
    tb_s(test_title);
    tb_s("\xE2\x80\x9D\xE2\x80\xA6");
    say(C_INFO);
}

static void unsubscribe(void)
{
    if (!subs_n) {
        tb_n = 0;
        tb_s("No feed to unsubscribe from");
        say(C_ERROR);
        return;
    }
    if (op_busy())
        return;
    memcpy(op_key, sub_buf + fdp.sel * 36, 32);
    op_start(OP_UNSUB, content_follow(&op_req, CONTENT_UNSUBSCRIBE, op_key));
    say_key("Unsubscribing from ", op_key, "\xE2\x80\xA6 (the host asks you)", C_INFO);
}

static void op_done(uint8_t st)
{
    uint8_t s = op_state;
    op_handle = 0;
    op_state = OP_IDLE;
    tb_n = 0;
    switch (s) {
    case OP_PUT:
        if (st != STATUS_DONE) {
            tb_s("Could not store the test article");
            say(C_ERROR);
            break;
        }
        memcpy(op_key, op_req.id, 32);
        op_start(OP_PUBLISH, content_publish(&op_req, op_key, test_title, test_tlen));
        tb_s("Publishing \xE2\x80\x9C");
        tb_s(test_title);
        tb_s("\xE2\x80\x9D\xE2\x80\xA6 (the host asks you)");
        say(C_INFO);
        break;
    case OP_PUBLISH:
        if (st != STATUS_DONE) {
            tb_s(st == STATUS_REFUSED ? "Publishing refused" : "Publishing failed");
            say(C_ERROR);
            break;
        }
        test_seq = op_req.size;
        if (test_seq > tests)
            tests = test_seq;
        published();
        say(C_OK);
        /* Publishing creates the identity if there was none. */
        op_start(OP_IDENT, content_identity(&op_req));
        break;
    case OP_IDENT:
        if (st == STATUS_DONE) {
            memcpy(own_key, op_req.id, 32);
            own_known = 1;
            page_dirty = 1;
            if (subs_loaded && find_sub(own_key) < 0) {
                op_start(OP_SELF_SUB, content_follow(&op_req, CONTENT_SUBSCRIBE, own_key));
                published();
                tb_s("; subscribing to your own feed so that it shows in the timeline\xE2\x80\xA6");
                say(C_INFO);
                break;
            }
        }
        reload();
        break;
    case OP_SELF_SUB:
        published();
        if (st == STATUS_DONE)
            tb_s(" and subscribed to your own feed");
        else
            tb_s(st == STATUS_REFUSED ? "; subscribing to your own feed was refused, so it is not in the timeline"
                                      : "; subscribing to your own feed failed");
        say(st == STATUS_DONE ? C_OK : C_ERROR);
        reload();
        break;
    case OP_SUB:
    case OP_UNSUB:
        tb_s(s == OP_SUB ? "Subscrib" : "Unsubscrib");
        if (st == STATUS_DONE) {
            tb_s(s == OP_SUB ? "ed to " : "ed from ");
            tb_key(op_key);
            say(C_OK);
            reload();
        } else {
            tb_s(s == OP_SUB ? "ing to " : "ing from ");
            tb_key(op_key);
            tb_s(st == STATUS_REFUSED ? " refused" : " failed");
            say(C_ERROR);
        }
        break;
    }
}

/* --- The feed key entry -------------------------------------------------- */

static void entry_show(uint8_t on)
{
    entry = on;
    entry_n = 0;
    rect_h = on ? PAGE_H - 16 : PAGE_H;
    lp_keep(&fdp, subs_n);
    page_dirty = 1;
    dirty[ENTRY_ROW] = dirty[STATUS_ROW] = 1;
    if (!on)
        term_cmd(TERM_CURSOR_OFF);
}

static uint8_t hex_value(uint8_t c)
{
    if ((uint8_t)(c - '0') < 10)
        return c - '0';
    c |= 0x20;
    if ((uint8_t)(c - 'a') < 6)
        return c - 'a' + 10;
    return 255;
}

/* A character typed or pasted; blanks are left out. */
static void entry_text(uint32_t cp)
{
    if (cp == ' ' || cp == '\t' || cp == '\n' || cp == '\r')
        return;
    if (!entry_n && (cp == 's' || cp == 'S'))
        return; /* the text of the S that opened the entry */
    if (entry_n < ENTRY_CAP)
        entry_buf[entry_n] = cp > 0x20 && cp < 0x7F ? cp : 0x80;
    if (entry_n < 0xFFFF)
        entry_n++;
    dirty[ENTRY_ROW] = 1;
    if (msg_n) {
        msg_n = 0;
        dirty[STATUS_ROW] = 1;
    }
}

static void entry_submit(void)
{
    uint8_t key[32];
    uint16_t i, n = entry_n < ENTRY_CAP ? entry_n : ENTRY_CAP;
    tb_n = 0;
    if (!entry_n) {
        tb_s("Type or paste a feed key (64 hex digits), or Esc to cancel");
        say(C_ERROR);
        return;
    }
    for (i = 0; i < n; i++)
        if (hex_value(entry_buf[i]) > 15)
            break;
    if (i < n) {
        tb_s("Not a feed key: a key has only the hex digits 0-9 and a-f");
        say(C_ERROR);
        return;
    }
    if (entry_n != 64) {
        tb_s("Not a feed key: ");
        tb_u(entry_n);
        tb_s(" hex digits, but a key has 64");
        say(C_ERROR);
        return;
    }
    for (i = 0; i < 32; i++)
        key[i] = hex_value(entry_buf[2 * i]) << 4 | hex_value(entry_buf[2 * i + 1]);
    if (find_sub(key) >= 0) {
        entry_show(0);
        say_key("Already subscribed to ", key, "", C_INFO);
        return;
    }
    if (op_busy())
        return;
    entry_show(0);
    memcpy(op_key, key, 32);
    op_start(OP_SUB, content_follow(&op_req, CONTENT_SUBSCRIBE, op_key));
    say_key("Subscribing to ", key, "\xE2\x80\xA6 (the host asks you)", C_INFO);
}

/* --- Terminal rows ------------------------------------------------------- */

static void bar_row(uint8_t row)
{
    row_start(row, C_BAR_FG, C_BAR_BG);
}

static void draw_entry_row(void)
{
    uint16_t n = entry_n < ENTRY_CAP ? entry_n : ENTRY_CAP, from = 0, i;
    if (!entry) {
        row_start(ENTRY_ROW, C_TEXT, C_BG);
        row_end();
        return;
    }
    row_start(ENTRY_ROW, C_TEXT, C_ENTRY_BG);
    puts_(" Feed key: ");
    if (n > COLS - 13) {
        from = n - (COLS - 14);
        ink(C_DIM);
        put1(0x2026);
        ink(C_TEXT);
    }
    for (i = from; i < n; i++)
        put1(entry_buf[i] < 0x80 ? entry_buf[i] : 0xFFFD);
    i = col;
    row_end();
    term_goto(i, ENTRY_ROW);
    term_cmd(TERM_CURSOR_ON);
}

static void draw_status(void)
{
    bar_row(STATUS_ROW);
    if (view == ARTICLE && link_shown) {
        uint32_t n = link_len[link_shown - 1];
        puts_(" Link: ");
        put_title(art + link_at[link_shown - 1], n > 255 ? 255 : n, COLS - 1 - col);
    } else if (msg_n) {
        spaces(1);
        ink(msg_fg);
        put_title((const uint8_t *)msg, msg_n, COLS - 2);
    } else if (entry) {
        puts_(" Type or paste a feed key   Enter: subscribe   Esc: cancel");
    } else if (view == TIMELINE) {
        puts_(" \xE2\x86\x91\xE2\x86\x93 move  Enter open  F feeds  ? help  T publish a test  Esc quit");
        if (tl_total) {
            ink(C_BAR_DIM);
            tb_n = 0;
            tb_u(tlp.sel + 1);
            tb_s("/");
            tb_u(tl_total);
            pad_to(COLS - 1 - tb_n);
            put_title((const uint8_t *)tb, tb_n, tb_n);
        }
    } else if (view == FEEDS) {
        puts_(" \xE2\x86\x91\xE2\x86\x93 move  S subscribe  U unsubscribe  T publish a test  ? help  Esc back");
    } else {
        if (doc_state && page_h_known && max_scroll()) {
            spaces(1);
            put_u32(percent());
            puts_(view == HELP || doc_state == 2 ? "%" : "%\xE2\x80\xA6");
        }
        if (view == HELP) {
            pad_to(COLS - 48);
            puts_("\xE2\x86\x91\xE2\x86\x93 scroll  T publish a test  F feeds  Esc back");
        } else {
            pad_to(COLS - 42);
            puts_("\xE2\x86\x91\xE2\x86\x93 PgUp PgDn Home End wheel   Esc/\xE2\x86\x90: back");
        }
    }
    row_end();
}

/* --- Events and frames ------------------------------------------------- */

static void show(uint8_t v)
{
    if (view == v)
        return;
    if (entry)
        entry_show(0);
    view = v;
    link_shown = 0;
    page_dirty = 1;
    dirty[STATUS_ROW] = 1;
    if (v == FEEDS) {
        lp_keep(&fdp, subs_n);
        start_count();
    }
}

/* Scrolling keys of the article and help. */
static void scroll_key(uint8_t code)
{
    switch (code) {
    case KEY_UP: scroll_by(-LINE); break;
    case KEY_DOWN: scroll_by(LINE); break;
    case KEY_PAGE_UP: scroll_by(-(RECT_H - LINE)); break;
    case KEY_PAGE_DOWN: case KEY_SPACE: scroll_by(RECT_H - LINE); break;
    case KEY_HOME: scroll_by(-scroll); break;
    case KEY_END:
        to_end = 1;
        if (page_h_known)
            scroll_to(max_scroll());
        break;
    }
}

/* Returns 1 to quit. */
static uint8_t key(uint8_t code, uint8_t mods)
{
    uint8_t back = code == KEY_ESCAPE || code == KEY_BACKSPACE || code == KEY_LEFT;
    if (msg_n) {
        msg_n = 0;
        dirty[STATUS_ROW] = 1;
    }
    if (entry) {
        if (code == KEY_ENTER) {
            entry_submit();
        } else if (code == KEY_ESCAPE) {
            entry_show(0);
        } else if (code == KEY_BACKSPACE && entry_n) {
            entry_n--;
            dirty[ENTRY_ROW] = 1;
        }
        return 0;
    }
    if (view == ARTICLE) {
        if (link_shown) {
            link_shown = 0;
            dirty[STATUS_ROW] = 1;
        }
        if (back)
            show(TIMELINE);
        else
            scroll_key(code);
        return 0;
    }
    if (!(mods & (MOD_CONTROL | MOD_ALT | MOD_META))) {
        switch (code) {
        case KEY_F: show(FEEDS); return 0;
        case KEY_F1: show(HELP); return 0;
        case KEY_T: test_publish(); return 0;
        case KEY_S:
            if (view == FEEDS)
                entry_show(1);
            return 0;
        case KEY_U:
            if (view == FEEDS)
                unsubscribe();
            return 0;
        }
    }
    if (view == TIMELINE) {
        if (code == KEY_ESCAPE)
            return 1;
        if (code == KEY_ENTER)
            open_article(tlp.sel);
        else
            lp_key(&tlp, tl_total, code);
    } else if (back) {
        show(TIMELINE);
    } else if (view == FEEDS) {
        lp_key(&fdp, subs_n, code);
    } else {
        scroll_key(code);
    }
    return 0;
}

static void anchor(uint16_t id, uint8_t buttons)
{
    if (!id || entry)
        return;
    if (view == ARTICLE) {
        if (id <= links) {
            link_shown = id;
            dirty[STATUS_ROW] = 1;
        }
    } else if (!(buttons & BUTTON_PRIMARY)) {
        return;
    } else if (view == TIMELINE && id <= tl_total) {
        lp_set(&tlp, tl_total, id - 1, tlp.top);
        open_article(id - 1);
    } else if (view == FEEDS && id <= subs_n) {
        lp_set(&fdp, subs_n, id - 1, fdp.top);
    }
}

/* A completion is for `r` if its handle is `h` and `r` is no longer
 * pending: a request submitted since, which may have been given the same
 * handle, is still pending. */
static uint8_t done_for(uint8_t h, const content_req *r)
{
    return h && h == ev.a && r->status != STATUS_PENDING;
}

/* Returns 1 to quit. */
static uint8_t handle_events(void)
{
    uint32_t cp;
    while (terse_event_pop(&ev)) {
        switch (ev.type) {
        case EV_KEY_DOWN:
            if (key(ev.a, ev.b))
                return 1;
            break;
        case EV_TEXT:
            cp = ev.a | (uint32_t)ev.b << 8 | (uint32_t)(ev.c & 0x1F) << 16;
            if (entry)
                entry_text(cp);
            else if (cp == '?' && view != ARTICLE)
                show(HELP);
            break;
        case EV_WHEEL:
            if (view == TIMELINE) {
                lp_wheel(&tlp, tl_total, (int8_t)ev.a);
            } else if (view == FEEDS) {
                if (!entry)
                    lp_wheel(&fdp, subs_n, (int8_t)ev.a);
            } else {
                if (link_shown) {
                    link_shown = 0;
                    dirty[STATUS_ROW] = 1;
                }
                scroll_by((int8_t)ev.a * 3 * LINE);
            }
            break;
        case EV_PAGE_LAID_OUT:
            if (view == TIMELINE || view == FEEDS)
                list_height(EV_PAGE_HEIGHT(ev));
            else
                page_laid_out(EV_PAGE_HEIGHT(ev));
            break;
        case EV_PAGE_ANCHOR: anchor(EV_ANCHOR_ID(ev), EV_ANCHOR_BUTTONS(ev)); break;
        case EV_FEEDS_UPDATED: reload(); break;
        case EV_CONTENT_DONE:
            if (done_for(tl_handle, &tl_req))
                timeline_done(ev.b);
            else if (done_for(art_handle, &art_req))
                article_done(ev.b);
            else if (done_for(sub_handle, &sub_req))
                subs_done(ev.b);
            else if (done_for(cnt_handle, &cnt_req))
                count_done(ev.b);
            else if (done_for(op_handle, &op_req))
                op_done(ev.b);
            else if (done_for(id_handle, &id_req)) {
                id_handle = 0;
                if (ev.b == STATUS_DONE) {
                    memcpy(own_key, id_req.id, 32);
                    own_known = 1;
                    page_dirty = 1;
                }
            }
            break;
        }
    }
    return 0;
}

static void render(void)
{
    if (view == TIMELINE)
        ensure_window();
    if (page_dirty && view != ARTICLE) {
        page_dirty = 0;
        if (view == TIMELINE)
            timeline_doc();
        else if (view == FEEDS)
            feeds_doc();
        else
            help_doc();
    }
    if (view == ARTICLE)
        build();
    if (view == ARTICLE || view == HELP)
        send_scroll();
    if (dirty[STATUS_ROW]) {
        dirty[STATUS_ROW] = 0;
        draw_status();
    }
    if (dirty[ENTRY_ROW]) {
        dirty[ENTRY_ROW] = 0;
        draw_entry_row();
    }
}

int main(void)
{
    term_cmd(TERM_CURSOR_OFF);
    term_color(C_TEXT, C_BG);
    term_cmd(TERM_CLEAR);
    page_background(28, 28, 28, 255);
    load_subscriptions();
    load_timeline(0);
    id_handle = content_identity(&id_req);
    dirty[STATUS_ROW] = 1;
    for (;;) {
        now = terse_time();
        if (handle_events())
            break;
        render();
        terse_wait_frame();
    }
    page_clear();
    page_region(0, 0, 0, 0);
    term_color(C_TEXT, 0);
    term_cmd(TERM_CLEAR);
    term_cmd(TERM_CURSOR_ON);
    return 0;
}
