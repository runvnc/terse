/* reader.c: a feed reader for Terse (machine profile 1, RV32IMC).
 *
 * Lists the timeline of the feeds the host subscribes to (SPEC 9), newest
 * first, on the terminal, and shows an article (NETWORK 4.5: plain UTF-8)
 * on the page device (SPEC 11) as a centred column of typeset text.
 *
 * Every frame it handles the queued events, does a bounded share of the
 * work in hand (turning article bytes into the page document), draws every
 * terminal row that changed and waits for the next frame. A row is built
 * as 80 cells in RAM and written with one cells call (SPEC 8.2).
 *
 * The article view: row 0 (the title) and row 29 (status) are terminal
 * rows; the page's rectangle covers rows 1-28 (y 16-464), x 64-576: 512
 * units, about 68 characters of 15-unit body text, centred, with a
 * transparent background, and the cells under it blank. The document is
 * the title (26 units, bold), a grey line (feed key, sequence number, date,
 * size), a rule, then the article. Its text, by this rule:
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
 * much is shown.
 *
 * Building the document costs about 9 units per byte of prose (more for
 * short paragraphs and links), so it is spread over frames: each frame
 * spends at most BUILD_UNITS on it, by an estimate of the cost that is
 * never below the real one. The document is submitted at the end of the
 * first frame of building, partial if need be (so the first screenful
 * appears at once), and again, whole, when it is complete; the 48 KiB
 * fixture article takes 11 frames. The program keeps the scroll value itself: Up/Down move one
 * body line (LINE units), PgUp/PgDn/Space the rectangle less a line, the
 * wheel three lines a step, Home/End the ends. It clamps the value to
 * 0 .. height - 448 with the latest height event (08h) and calls `scroll`
 * whenever it changes, also after each submit (which resets it). Until the
 * first height arrives, moves are only kept from going below 0, End is
 * remembered (it also follows the height of the whole document when it
 * comes) and the status row shows no percentage.
 *
 * Memory: everything is a static array in RAM (flat addresses): the 16 KiB
 * timeline buffer, up to 48 KiB of the article (ART_CAP) and the 64 KiB
 * document. The timeline buffer holds one request's worth of records: 70
 * entries with 120-byte titles to 147 with empty ones. */

#include <stdint.h>
#include "terse.h"

#define COLS TERM_COLS
#define LIST_ROWS 28 /* rows 1-28; 0 is the title bar, 29 the status bar */
#define STATUS_ROW 29

/* xterm palette indexes (SPEC 8.5). */
#define C_TEXT 252
#define C_DIM 244
#define C_BG 0
#define C_BAR_FG 231
#define C_BAR_BG 24
#define C_BAR_DIM 109
#define C_SEL_BG 238
#define C_KEY 139
#define C_LOCAL 71
#define C_REPLY 73
#define C_NEW 220
#define C_ERROR 203

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

#define ART_CAP 49152u
#define CHUNK 16384u

/* The page: the rectangle, in screen units, and the document. */
#define RECT_X 64
#define RECT_Y 16
#define RECT_W 512
#define RECT_H 448
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
#define MAX_LINKS 512
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

enum { LIST, ARTICLE };
enum { FETCHING, READY, NOT_FOUND, FAILED };

static uint8_t view;
static uint32_t now; /* Unix time of this frame; 0 if the host has none */
static terse_event ev;
static uint8_t last_buttons;

static uint8_t dirty[TERM_ROWS];
/* The row being drawn: its cells, the column of the next character and the
 * colours it is drawn in. */
static uint32_t cells[COLS];
static uint8_t row_at, col;
static uint32_t attr; /* the colours, as in a cell: fg << 16 | bg << 24 */

/* --- Timeline ---------------------------------------------------------- */

static content_req tl_req, sub_req;
static uint8_t tl_handle, sub_handle;
static uint8_t tl_buf[TL_BUF_LEN];
static uint8_t sub_buf[36];
static uint16_t rec_at[MAX_RECORDS];
static uint16_t tl_base, tl_count, tl_total; /* window: tl_base .. +tl_count */
static uint16_t tl_req_base;
static uint8_t tl_loaded, tl_refresh, tl_again;
static uint16_t subs_total;

static uint16_t sel, top, drawn_sel, drawn_top;
static uint8_t sel_id[8]; /* feed key prefix and seq of the selection */
static uint8_t sel_id_ok;
static uint16_t new_top;  /* entries above this index arrived while running */
static uint8_t seen[16][8]; /* new entries opened since */
static uint8_t seen_next;

/* --- Article ----------------------------------------------------------- */

static content_req art_req;
static uint8_t art_handle, art_state;
static uint8_t art_title[121], art_tlen, art_feed[4], art_reply, art_local;
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

static uint8_t doc_state; /* 0: nothing submitted; 1: a part; 2: all of it */
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

/* A line of ASCII text being put together (a date, the article's details). */
static char tb[96];
static uint8_t tb_n;

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

static const uint8_t month_days[12] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};

/* `t` as a UTC date and time: "YYYY-MM-DD HH:MM" (or "MM-DD HH:MM"). The
 * machine has a clock but no time zone. Every fourth year is a leap year,
 * which holds from 1901 to 2099. */
static void tb_date(uint32_t t, uint8_t with_year)
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
    if (with_year) {
        tb_u(year);
        tb_s("-");
    }
    tb_two(month + 1);
    tb_s("-");
    tb_two(days + 1);
    tb_s(" ");
    tb_two(secs / 3600);
    tb_s(":");
    tb_two(secs % 3600 / 60);
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

static void put_hex(const uint8_t *b, uint8_t n)
{
    tb_n = 0;
    tb_hex(b, n);
    put_ascii((const uint8_t *)tb, tb_n);
}

/* A title in at most `room` columns, with an ellipsis if cut. A title
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

static uint32_t get32(const uint8_t *p)
{
    return p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

/* The age of an entry, right-aligned in 11 columns; the date if the host
 * gives no time or the entry claims to be from the future. */
static void put_age(uint32_t t)
{
    uint32_t d = now - t;
    const char *unit;
    const char *s;
    uint8_t n;
    if (now == 0 || now < t) {
        tb_n = 0;
        tb_date(t, 0);
        put_ascii((const uint8_t *)tb, tb_n);
        return;
    }
    if (d < 60) {
        spaces(3);
        puts_("just now");
        return;
    }
    if (d < 3600) {
        d /= 60;
        unit = "m ago";
    } else if (d < 86400) {
        d /= 3600;
        unit = "h ago";
    } else {
        d /= 86400;
        unit = "d ago";
    }
    s = u32_str(d);
    for (n = 0; s[n]; n++)
        ;
    spaces(11 - 5 - n);
    puts_(s);
    puts_(unit);
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
    memcpy(id, r + R_FEED, 4);
    memcpy(id + 4, r + R_SEQ, 4);
}

static uint8_t same_id(const uint8_t *a, const uint8_t *b)
{
    return memcmp(a, b, 8) == 0;
}

static uint8_t is_new(uint16_t i, const uint8_t *r)
{
    uint8_t id[8], k;
    if (i >= new_top)
        return 0;
    record_id(r, id);
    for (k = 0; k < 16; k++)
        if (same_id(seen[k], id))
            return 0;
    return 1;
}

static void mark_rows(uint8_t from, uint8_t to)
{
    while (from <= to)
        dirty[from++] = 1;
}

static void load_timeline(uint16_t first)
{
    tl_req_base = first;
    tl_handle = content_feed(&tl_req, CONTENT_TIMELINE, first, TL_BUF_LEN, tl_buf);
}

static void load_subscriptions(void)
{
    sub_handle = content_feed(&sub_req, CONTENT_SUBSCRIPTIONS, 0, sizeof sub_buf, sub_buf);
}

static void keep_visible(void)
{
    if (sel < top)
        top = sel;
    if (sel >= top + LIST_ROWS)
        top = sel - LIST_ROWS + 1;
}

static void timeline_done(uint8_t status)
{
    uint32_t at = 0, n = 0, total, i;
    uint16_t old_total = tl_total;
    tl_handle = 0;
    if (status != STATUS_DONE)
        return;
    while (at < tl_req.length && n < MAX_RECORDS) {
        rec_at[n++] = at;
        at += 1 + tl_buf[at];
    }
    total = tl_req.size;
    tl_base = tl_req_base;
    tl_count = n;
    tl_total = total > 0xFFFF ? 0xFFFF : total;
    if (tl_refresh && tl_loaded) {
        /* New entries arrive at index 0 (SPEC 9.5): the selection keeps its
         * entry, found by its identity or else moved down by the arrivals. */
        if (tl_total > old_total) {
            new_top += tl_total - old_total;
            sel += tl_total - old_total;
        }
        for (i = 0; sel_id_ok && i < tl_count; i++) {
            uint8_t id[8];
            record_id(tl_buf + rec_at[i], id);
            if (same_id(id, sel_id)) {
                sel = tl_base + i;
                break;
            }
        }
    }
    tl_refresh = 0;
    tl_loaded = 1;
    if (sel >= tl_total)
        sel = tl_total ? tl_total - 1 : 0;
    if (top + LIST_ROWS > tl_total)
        top = tl_total > LIST_ROWS ? tl_total - LIST_ROWS : 0;
    keep_visible();
    if (view == LIST) {
        drawn_top = top;
        mark_rows(0, STATUS_ROW);
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
    uint16_t last = top + LIST_ROWS - 1;
    if (tl_handle || !tl_loaded || tl_total == 0)
        return;
    if (last >= tl_total)
        last = tl_total - 1;
    if (!in_window(top) || !in_window(last))
        load_timeline(top > 32 ? top - 32 : 0);
}

static void feeds_updated(void)
{
    load_subscriptions();
    if (tl_handle) {
        tl_again = 1; /* reload once this load is in */
        return;
    }
    tl_refresh = 1;
    load_timeline(tl_base);
}

static void bar_row(uint8_t row)
{
    row_start(row, C_BAR_FG, C_BAR_BG);
}

static void draw_list_title(void)
{
    bar_row(0);
    puts_(" Terse reader");
    ink(C_BAR_DIM);
    puts_("  ·  ");
    ink(C_BAR_FG);
    put_u32(tl_total);
    puts_(tl_total == 1 ? " entry" : " entries");
    ink(C_BAR_DIM);
    puts_("  ·  ");
    ink(C_BAR_FG);
    put_u32(subs_total);
    puts_(subs_total == 1 ? " subscription" : " subscriptions");
    pad_to(COLS - 22);
    ink(C_LOCAL);
    puts_("✓");
    ink(C_BAR_FG);
    puts_(" local ");
    ink(C_REPLY);
    puts_("↳");
    ink(C_BAR_FG);
    puts_(" reply ");
    ink(C_NEW);
    puts_("•");
    ink(C_BAR_FG);
    puts_(" new ");
    row_end();
}

static void draw_list_status(void)
{
    bar_row(STATUS_ROW);
    puts_(" ↑↓ PgUp PgDn Home End wheel: move   Enter/click: open   Esc: quit");
    if (tl_handle) {
        pad_to(COLS - 10);
        puts_("loading…");
    }
    row_end();
}

static const char *const empty_text[] = {
    "No entries yet.",
    "",
    "The timeline shows the articles of the feeds this host subscribes to,",
    "newest first. To subscribe, run on the host:",
    "",
    "    terse feed subscribe <feed key>",
    "",
    "and run this program with a relay (--relay ws://…). New entries appear",
    "here as they arrive; no need to restart.",
};

static void draw_list_row(uint8_t row)
{
    uint16_t i = top + row - 1;
    uint8_t *r, flags;
    row_start(row, C_TEXT, i == sel && i < tl_total ? C_SEL_BG : C_BG);
    if (tl_loaded && tl_total == 0) {
        if (row >= 4 && row - 4 < sizeof empty_text / sizeof empty_text[0]) {
            spaces(6);
            puts_(empty_text[row - 4]);
        }
    } else if (!tl_loaded && row == 2) {
        spaces(2);
        puts_("Loading the timeline…");
    } else if (i < tl_total && !in_window(i)) {
        ink(C_DIM);
        spaces(2);
        puts_("…");
    } else if (i < tl_total) {
        uint8_t k, reply = 0;
        r = record(i);
        flags = r[R_FLAGS];
        if (i == sel) {
            record_id(r, sel_id);
            sel_id_ok = 1;
        }
        ink(C_NEW);
        put1(is_new(i, r) ? 0x2022 : ' ');
        ink(C_DIM);
        put_age(get32(r + R_TIME));
        spaces(1);
        ink(C_KEY);
        put_hex(r + R_FEED, 4);
        spaces(1);
        ink(C_LOCAL);
        put1(flags & TIMELINE_LOCAL ? 0x2713 : ' ');
        ink(C_REPLY);
        for (k = 0; k < 32; k++)
            reply |= r[R_REPLY + k];
        put1(reply ? 0x21B3 : ' ');
        spaces(1);
        ink(C_TEXT);
        put_title(r + R_TITLE, r[R_N], COLS - col);
    }
    row_end();
}

static void select_entry(uint16_t i)
{
    if (tl_total == 0)
        return;
    if (i >= tl_total)
        i = tl_total - 1;
    if (i != sel)
        sel_id_ok = 0; /* set again when its row is drawn */
    sel = i;
    keep_visible();
}

static void move_by(int32_t delta)
{
    int32_t i = (int32_t)sel + delta;
    select_entry(i < 0 ? 0 : i > 0xFFFF ? 0xFFFF : (uint16_t)i);
}

static void scroll_list(int32_t delta)
{
    int32_t t = (int32_t)top + delta;
    uint16_t max = tl_total > LIST_ROWS ? tl_total - LIST_ROWS : 0;
    top = t < 0 ? 0 : t > max ? max : (uint16_t)t;
    if (sel < top)
        select_entry(top);
    else if (sel >= top + LIST_ROWS)
        select_entry(top + LIST_ROWS - 1);
}

/* Page up or down: the selection keeps its row unless the list cannot
 * scroll further, when it moves instead. */
static void page_list(int32_t delta)
{
    uint16_t old_top = top, row = sel - top;
    scroll_list(delta);
    if (top == old_top)
        move_by(delta);
    else
        select_entry(top + row);
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

/* The title, the details line and the rule. */
static void doc_head(void)
{
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(TITLE_SIZE);
    doc_rgb(rgb_title);
    if (art_tlen)
        doc_text(art_title, art_tlen);
    else
        doc_text("Untitled", 8);
    doc_paragraph(DOC_START, 0, 6, 0);
    doc_font(DOC_SANS, 0);
    doc_size(META_SIZE);
    doc_rgb(rgb_meta);
    tb_n = 0;
    tb_s("feed ");
    tb_hex(art_feed, 4);
    tb_s("  \xC2\xB7  #");
    tb_u(art_seq);
    tb_s("  \xC2\xB7  ");
    tb_date(art_time, 1);
    tb_s(" UTC  \xC2\xB7  ");
    tb_u(art_size);
    tb_s(" bytes");
    doc_text(tb, tb_n);
    if (art_reply) {
        doc_rgb(rgb_reply);
        doc_text("  \xE2\x86\xB3 reply", 11);
    }
    doc_paragraph(DOC_START, 0, 10, 0);
    doc_rgb(rgb_rule);
    doc_rule();
    doc_size(BODY_SIZE);
    doc_rgb(rgb_body);
    b_colour = rgb_body;
    b_was_pre = 0;
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

/* --- Article ----------------------------------------------------------- */

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
    /* Before the first submit (a height left from the list) or 0 (no
     * document, which this view never has once it submitted): not ours. */
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
    uint8_t *r, id[8];
    if (i >= tl_total || !in_window(i) || art_handle)
        return;
    r = record(i);
    art_tlen = r[R_N];
    memcpy(art_title, r + R_TITLE, art_tlen);
    memcpy(art_feed, r + R_FEED, 4);
    memcpy(art_blob, r + R_BLOB, 32);
    art_reply = 0;
    for (uint8_t k = 0; k < 32; k++)
        art_reply |= r[R_REPLY + k];
    art_local = r[R_FLAGS] & TIMELINE_LOCAL;
    art_seq = get32(r + R_SEQ);
    art_time = get32(r + R_TIME);
    art_size = get24(r + R_SIZE);
    art_want = art_size > ART_CAP ? ART_CAP : art_size;
    art_have = 0;
    art_state = FETCHING;
    record_id(r, id);
    if (is_new(i, r)) {
        memcpy(seen[seen_next], id, 8);
        seen_next = (seen_next + 1) & 15;
    }
    request_chunk();
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
    page_region(RECT_X, RECT_Y, RECT_W, RECT_H);
    view = ARTICLE;
    mark_rows(0, STATUS_ROW);
}

static void article_done(uint8_t status)
{
    uint32_t size;
    art_handle = 0;
    if (status == STATUS_NOT_FOUND || status == STATUS_FAILED || status == STATUS_BAD_REQUEST) {
        art_state = status == STATUS_NOT_FOUND ? NOT_FOUND : FAILED;
        mark_rows(1, STATUS_ROW);
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
    if (art_state == FETCHING)
        mark_rows(1, STATUS_ROW);
    art_state = READY;
}

/* Rows 1-28 under the page: blank, or a message while there is no text. */
static void draw_article_row(uint8_t row)
{
    row_start(row, C_TEXT, C_BG);
    if (art_state == FETCHING) {
        if (row == 2) {
            spaces(2);
            puts_(art_local ? "Reading the article…" : "Fetching the article from the relay…");
        }
    } else if (art_state != READY) {
        if (row == 2) {
            spaces(2);
            ink(C_ERROR);
            puts_(art_state == NOT_FOUND ? "Not found: neither this host nor its relay has the article."
                                         : "Could not fetch the article (no relay, network error or timeout).");
        } else if (row == 4) {
            spaces(2);
            ink(C_DIM);
            puts_("Esc: back to the list");
        }
    }
    row_end();
}

static void draw_article_title(void)
{
    bar_row(0);
    spaces(1);
    put_title(art_title, art_tlen, COLS - 2);
    row_end();
}

/* How far down the page the bottom of the rectangle is, in per cent. */
static uint32_t percent(void)
{
    int32_t s = scroll;
    if (page_h <= RECT_H)
        return 100;
    if (s < 0)
        s = 0;
    if ((uint32_t)s > max_scroll())
        s = max_scroll();
    return ((uint32_t)s + RECT_H) * 100 / page_h;
}

static void draw_article_status(void)
{
    bar_row(STATUS_ROW);
    if (link_shown) {
        uint32_t n = link_len[link_shown - 1];
        puts_(" Link: ");
        put_title(art + link_at[link_shown - 1], n > 255 ? 255 : n, COLS - 1 - col);
    } else {
        if (art_state == READY && page_h_known) {
            spaces(1);
            put_u32(percent());
            puts_(doc_state == 2 ? "%" : "%…");
        }
        pad_to(COLS - 42);
        puts_("↑↓ PgUp PgDn Home End wheel   Esc/←: back");
    }
    row_end();
}

static void close_article(void)
{
    page_clear();
    page_region(0, 0, 0, 0);
    view = LIST;
    drawn_top = top;
    mark_rows(0, STATUS_ROW);
}

/* --- Events and frames ------------------------------------------------- */

static void key(uint8_t code)
{
    if (view == LIST) {
        switch (code) {
        case KEY_UP: move_by(-1); break;
        case KEY_DOWN: move_by(1); break;
        case KEY_PAGE_UP: page_list(-LIST_ROWS); break;
        case KEY_PAGE_DOWN: page_list(LIST_ROWS); break;
        case KEY_HOME: select_entry(0); break;
        case KEY_END: select_entry(0xFFFF); break;
        case KEY_ENTER: open_article(sel); break;
        }
        return;
    }
    if (link_shown) {
        link_shown = 0;
        dirty[STATUS_ROW] = 1;
    }
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
    case KEY_ESCAPE: case KEY_BACKSPACE: case KEY_LEFT: close_article(); break;
    }
}

static void pointer(void)
{
    uint8_t pressed = EV_POINTER_BUTTONS(ev) & ~last_buttons & BUTTON_PRIMARY;
    uint8_t row = EV_POINTER_Y(ev) >> 4;
    last_buttons = EV_POINTER_BUTTONS(ev);
    if (!pressed || view != LIST || row < 1 || row > LIST_ROWS || top + row - 1 >= tl_total)
        return;
    if (top + row - 1 == sel)
        open_article(sel);
    else
        select_entry(top + row - 1);
}

/* Returns 1 to quit. */
static uint8_t handle_events(void)
{
    while (terse_event_pop(&ev)) {
        switch (ev.type) {
        case EV_KEY_DOWN:
            if (view == LIST && ev.a == KEY_ESCAPE)
                return 1;
            key(ev.a);
            break;
        case EV_POINTER: pointer(); break;
        case EV_WHEEL:
            if (view == LIST) {
                scroll_list((int8_t)ev.a * 3);
            } else {
                if (link_shown) {
                    link_shown = 0;
                    dirty[STATUS_ROW] = 1;
                }
                scroll_by((int8_t)ev.a * 3 * LINE);
            }
            break;
        case EV_PAGE_LAID_OUT:
            if (view == ARTICLE)
                page_laid_out(EV_PAGE_HEIGHT(ev));
            break;
        case EV_PAGE_ANCHOR:
            if (view == ARTICLE && EV_ANCHOR_ID(ev) && EV_ANCHOR_ID(ev) <= links) {
                link_shown = EV_ANCHOR_ID(ev);
                dirty[STATUS_ROW] = 1;
            }
            break;
        case EV_FEEDS_UPDATED: feeds_updated(); break;
        case EV_CONTENT_DONE:
            if (ev.a == tl_handle)
                timeline_done(ev.b);
            else if (ev.a == art_handle)
                article_done(ev.b);
            else if (ev.a == sub_handle) {
                sub_handle = 0;
                subs_total = sub_req.size;
                dirty[0] = 1;
            }
            break;
        }
    }
    return 0;
}

/* Brings the list position up to date, scrolling the terminal for a
 * one-row move (SPEC 8.4) so that only the new row is drawn. Rows 0 and 29
 * move with it and are redrawn. */
static void follow(uint16_t want, uint16_t *drawn)
{
    if (want == *drawn)
        return;
    if (want == (uint16_t)(*drawn + 1)) {
        term_cmd(TERM_SCROLL_UP);
        dirty[0] = dirty[LIST_ROWS] = dirty[STATUS_ROW] = 1;
    } else if ((uint16_t)(want + 1) == *drawn) {
        term_cmd(TERM_SCROLL_DOWN);
        dirty[0] = dirty[1] = dirty[STATUS_ROW] = 1;
    } else {
        mark_rows(0, STATUS_ROW);
    }
    *drawn = want;
}

static void render(void)
{
    uint8_t row;
    if (view == LIST) {
        ensure_window();
        follow(top, &drawn_top);
        if (sel != drawn_sel) {
            if (drawn_sel >= top && drawn_sel < top + LIST_ROWS)
                dirty[drawn_sel - top + 1] = 1;
            dirty[sel - top + 1] = 1;
            drawn_sel = sel;
        }
        if (tl_handle)
            dirty[STATUS_ROW] = 1;
    } else {
        build();
        send_scroll();
    }
    for (row = 0; row < TERM_ROWS; row++) {
        if (!dirty[row])
            continue;
        dirty[row] = 0;
        if (view == LIST) {
            if (row == 0)
                draw_list_title();
            else if (row == STATUS_ROW)
                draw_list_status();
            else
                draw_list_row(row);
        } else {
            if (row == STATUS_ROW)
                draw_article_status();
            else if (row == 0)
                draw_article_title();
            else
                draw_article_row(row);
        }
    }
}

int main(void)
{
    term_cmd(TERM_CURSOR_OFF);
    term_color(C_TEXT, C_BG);
    term_cmd(TERM_CLEAR);
    page_background(0, 0, 0, 0);
    load_subscriptions();
    load_timeline(0);
    mark_rows(0, STATUS_ROW);
    for (;;) {
        now = terse_time();
        if (handle_events())
            break;
        render();
        terse_wait_frame();
    }
    term_color(C_TEXT, C_BG);
    term_cmd(TERM_CLEAR);
    term_cmd(TERM_CURSOR_ON);
    return 0;
}
