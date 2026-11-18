/* reader.c: a feed reader for Terse, machine profile 2 (RV32IMC).
 *
 * A port of programs/reader/reader.c (profile 1, SDCC) to the profile-2
 * library: the same screens and behaviour. Lists the timeline of the feeds
 * the host subscribes to (SPEC 7.7), newest first, and shows an article
 * (NETWORK 4.5: plain UTF-8) word-wrapped to the grid.
 *
 * Every frame it handles the queued events, wraps all the article bytes it
 * holds, draws every row that changed and waits for the next frame. There is
 * no work budget: if a frame's work runs out of instructions the machine
 * simply carries on in the next frame.
 *
 * Memory: everything is a static array in RAM (flat addresses): the 16 KiB
 * timeline buffer, the line index (MAX_LINES offsets) and up to 48 KiB of
 * the article (ART_CAP). The timeline buffer holds one request's worth of
 * records: 70 entries with 120-byte titles to 147 with empty ones. */

#include <stdint.h>
#include "terse.h"

#define COLS TERM_COLS
#define LIST_ROWS 28 /* rows 1-28; 0 is the title bar, 29 the status bar */
#define TEXT_TOP 2   /* article text: rows 2-28 */
#define TEXT_ROWS 27
#define STATUS_ROW 29

/* xterm palette indexes (SPEC 6.6). */
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

/* Timeline records (SPEC 7.7): L, flags, then the entry (NETWORK 4.2). */
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
#define MAX_LINES 2048
#define NEED_MORE 0xFFFF

enum { LIST, ARTICLE };
enum { FETCHING, READY, NOT_FOUND, FAILED };

static uint8_t view;
static uint32_t now; /* Unix time of this frame; 0 if the host has none */
static terse_event ev;
static uint8_t last_buttons;

static uint8_t dirty[TERM_ROWS];
static uint8_t col; /* column of the next character in the row being drawn */

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
static uint16_t lines[MAX_LINES];
static uint16_t line_count, wrap_pos;
static uint8_t wrap_done;
static uint16_t art_top, art_drawn_top;
static uint8_t art_to_end; /* End was pressed before the wrapping finished */

/* The reader position: rp points at offset rpos of the article. */
static const uint8_t *rp;
static uint16_t rpos;

static void seek(uint16_t off)
{
    rpos = off;
    rp = art + off;
}

static void skip(uint32_t n)
{
    rpos += n;
    rp += n;
}

/* --- Text -------------------------------------------------------------- */

/* Counts the printable ASCII bytes (20h-7Eh) at `p`, at most `max`;
 * scan_space is one past the last space among them (0: none). */
static uint32_t scan_space;

static uint32_t scan_ascii(uint32_t max, const uint8_t *p)
{
    uint32_t n;
    scan_space = 0;
    for (n = 0; n < max && p[n] >= 0x20 && p[n] < 0x7F; n++)
        if (p[n] == ' ')
            scan_space = n + 1;
    return n;
}

/* How many bytes from the reader position a run may cover: at most `max`,
 * not past `limit`. */
static uint32_t run_room(uint32_t max, uint16_t limit)
{
    uint32_t left = limit - rpos;
    return left < max ? left : max;
}

/* Decodes one character of the article before `limit`. Malformed UTF-8
 * gives U+FFFD per maximal invalid prefix, and the byte that ended it is
 * decoded afresh (SPEC 6.3); characters above U+FFFF give U+FFFD. */
static uint16_t art_char(uint16_t limit)
{
    uint8_t b = *rp, need, lo = 0x80, hi = 0xBF, c;
    uint16_t cp;
    skip(1);
    if (b < 0x80)
        return b;
    if (b >= 0xC2 && b <= 0xDF) {
        need = 1;
        cp = b & 0x1F;
    } else if (b >= 0xE0 && b <= 0xEF) {
        need = 2;
        cp = b & 0x0F;
        if (b == 0xE0)
            lo = 0xA0;
        if (b == 0xED)
            hi = 0x9F;
    } else if (b >= 0xF0 && b <= 0xF4) {
        need = 3;
        cp = 0;
        if (b == 0xF0)
            lo = 0x90;
        if (b == 0xF4)
            hi = 0x8F;
    } else {
        return 0xFFFD;
    }
    for (c = need; c; c--) {
        if (rpos >= limit || *rp < lo || *rp > hi)
            return 0xFFFD;
        cp = cp << 6 | (*rp & 0x3F);
        lo = 0x80;
        hi = 0xBF;
        skip(1);
    }
    return need == 3 ? 0xFFFD : cp;
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

static uint8_t width(uint16_t cp)
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

/* --- Drawing ------------------------------------------------------------ */

static void row_start(uint8_t row, uint8_t f, uint8_t b)
{
    term_goto(0, row);
    term_color(f, b);
    col = 0;
}

static void ink(uint8_t f)
{
    term_fg(f);
}

/* A character known to be narrow. */
static void put1(uint16_t cp)
{
    term_put_cp(cp);
    col++;
}

static void put(uint16_t cp)
{
    term_put_cp(cp);
    col += width(cp);
}

/* A string from this program: UTF-8, no wide characters. */
static void puts_(const char *s)
{
    const char *e;
    for (e = s; *e; e++)
        if ((*e & 0xC0) != 0x80)
            col++;
    term_write(s, e - s);
}

static void spaces(uint8_t n)
{
    static const char blank[COLS] = "                                                                                ";
    term_write(blank, n);
    col += n;
}

static void pad_to(uint8_t c)
{
    if (col < c)
        spaces(c - col);
}

/* Clears the rest of the row, unless the row is full: the cursor then
 * still sits on column 79 (pending wrap, SPEC 6.3). */
static void row_end(void)
{
    if (col < COLS)
        term_cmd(TERM_CLEAR_EOL);
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

static void put_u32(uint32_t v)
{
    puts_(u32_str(v));
}

static void put_hex(const uint8_t *b, uint8_t n)
{
    static const char hex[] = "0123456789abcdef";
    char out[8];
    uint8_t k;
    for (k = 0; k < n; k++) {
        out[2 * k] = hex[b[k] >> 4];
        out[2 * k + 1] = hex[b[k] & 15];
    }
    term_write(out, 2 * n);
    col += 2 * n;
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
        b = term_put_ascii(b, p);
        p += b;
        w += b;
        if (p == end || w == limit)
            break;
        q = p;
        cp = printable(title_char(&q));
        cw = width(cp);
        if (w + cw > limit)
            break;
        term_put_cp(cp);
        w += cw;
        p = q;
    }
    col += w;
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

static void two(uint32_t v)
{
    char d[2] = {'0' + v / 10, '0' + v % 10};
    term_write(d, 2);
    col += 2;
}

static const uint8_t month_days[12] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};

/* `t` as a UTC date and time: "YYYY-MM-DD HH:MM" (or "MM-DD HH:MM"). The
 * machine has a clock but no time zone. Every fourth year is a leap year,
 * which holds from 1901 to 2099. */
static void put_date(uint32_t t, uint8_t with_year)
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
        put_u32(year);
        puts_("-");
    }
    two(month + 1);
    puts_("-");
    two(days + 1);
    puts_(" ");
    two(secs / 3600);
    puts_(":");
    two(secs % 3600 / 60);
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
        put_date(t, 0);
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
        /* New entries arrive at index 0 (SPEC 7.7): the selection keeps its
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

/* --- Article ---------------------------------------------------------- */

static uint16_t article_end_top(void)
{
    return line_count > TEXT_ROWS ? line_count - TEXT_ROWS : 0;
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
    line_count = 0;
    wrap_pos = 0;
    wrap_done = 0;
    art_top = art_drawn_top = 0;
    art_to_end = 0;
    art_state = FETCHING;
    record_id(r, id);
    if (is_new(i, r)) {
        memcpy(seen[seen_next], id, 8);
        seen_next = (seen_next + 1) & 15;
    }
    request_chunk();
    view = ARTICLE;
    mark_rows(0, STATUS_ROW);
}

static void article_done(uint8_t status)
{
    uint32_t size;
    art_handle = 0;
    if (status == STATUS_NOT_FOUND || status == STATUS_FAILED || status == STATUS_BAD_REQUEST) {
        art_state = status == STATUS_NOT_FOUND ? NOT_FOUND : FAILED;
        mark_rows(TEXT_TOP, STATUS_ROW);
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
        mark_rows(TEXT_TOP, STATUS_ROW);
    art_state = READY;
}

/* Finds where the line starting at the reader position ends: after a
 * newline, after the last space that fits, or (a word longer than the row)
 * before the first character that does not fit. NEED_MORE if the held
 * bytes end first and more are coming. */
static uint16_t wrap_line(uint16_t limit, uint8_t final)
{
    /* Until all bytes are in, stop short of the end: a character might be cut. */
    uint16_t brk = 0, at, stop = final ? limit : limit - 3;
    uint32_t c = 0, w;
    uint8_t b;
    if (!final && limit < 3)
        return NEED_MORE;
    for (;;) {
        if (rpos >= stop)
            return final ? limit : NEED_MORE;
        b = *rp;
        if (b >= 0x20 && b < 0x7F && c < COLS) {
            w = scan_ascii(run_room(COLS - c, stop), rp);
            if (scan_space)
                brk = rpos + scan_space;
            c += w;
            skip(w);
            continue;
        }
        at = rpos;
        if (b < 0x80) {
            skip(1);
            if (b == '\n')
                return rpos;
            w = b == '\t' ? 8 - (c & 7) : b == '\r' ? 0 : 1; /* other controls show as U+FFFD */
        } else {
            w = width(art_char(limit));
        }
        if (c + w > COLS)
            return brk ? brk : at;
        c += w;
        if (b == ' ' || b == '\t')
            brk = rpos;
    }
}

/* Wraps every held byte that can be wrapped. */
static void wrap_all(void)
{
    uint8_t final = art_have >= art_want;
    uint16_t next;
    if (art_state != READY || wrap_done)
        return;
    for (;;) {
        if (line_count == MAX_LINES || (final && wrap_pos >= art_have)) {
            wrap_done = 1;
            if (art_to_end)
                art_top = article_end_top();
            dirty[STATUS_ROW] = 1;
            if (line_count >= art_top && line_count < art_top + TEXT_ROWS)
                dirty[TEXT_TOP + line_count - art_top] = 1; /* the "continues" note */
            break;
        }
        seek(wrap_pos);
        next = wrap_line(art_have, final);
        if (next == NEED_MORE)
            break;
        if (line_count >= art_top && line_count < art_top + TEXT_ROWS)
            dirty[TEXT_TOP + line_count - art_top] = 1;
        lines[line_count++] = wrap_pos;
        wrap_pos = next;
        dirty[STATUS_ROW] = 1;
    }
}

static void draw_text_row(uint8_t row)
{
    uint16_t i = art_top + row - TEXT_TOP, end, cp;
    uint8_t w;
    row_start(row, C_TEXT, C_BG);
    if (art_state == FETCHING) {
        if (row == TEXT_TOP + 1) {
            spaces(2);
            puts_(art_local ? "Reading the article…" : "Fetching the article from the relay…");
        }
    } else if (art_state != READY) {
        if (row == TEXT_TOP + 1) {
            spaces(2);
            ink(C_ERROR);
            puts_(art_state == NOT_FOUND ? "Not found: neither this host nor its relay has the article."
                                         : "Could not fetch the article (no relay, network error or timeout).");
        } else if (row == TEXT_TOP + 3) {
            spaces(2);
            ink(C_DIM);
            puts_("Esc: back to the list");
        }
    } else if (i < line_count) {
        end = i + 1 < line_count ? lines[i + 1] : wrap_pos;
        seek(lines[i]);
        while (rpos < end) {
            uint8_t b = *rp;
            if (b >= 0x20 && b < 0x7F && col < COLS) {
                uint32_t n = term_put_ascii(run_room(COLS - col, end), rp);
                col += n;
                skip(n);
                continue;
            }
            if (b < 0x80) {
                skip(1);
                if (b >= ' ' && b != 0x7F)
                    break; /* the row is full */
                if (b == '\t') {
                    spaces(8 - (col & 7));
                    continue;
                }
                if (b == '\n' || b == '\r')
                    continue;
                cp = 0xFFFD;
            } else {
                cp = art_char(end);
            }
            w = width(cp);
            if (col + w > COLS)
                break;
            term_put_cp(cp);
            col += w;
        }
    } else if (i == line_count && wrap_done && (art_have < art_size || line_count == MAX_LINES)) {
        ink(C_DIM);
        puts_("[… the article continues: this reader holds ");
        if (line_count == MAX_LINES) {
            put_u32(MAX_LINES);
            puts_(" lines");
        } else {
            put_u32(art_have);
            puts_(" of ");
            put_u32(art_size);
            puts_(" bytes");
        }
        puts_("]");
    }
    row_end();
}

static void draw_article_header(uint8_t row)
{
    bar_row(row);
    if (row == 0) {
        spaces(1);
        put_title(art_title, art_tlen, COLS - 2);
    } else {
        row_start(row, C_DIM, C_BG);
        puts_(" feed ");
        ink(C_KEY);
        put_hex(art_feed, 4);
        ink(C_DIM);
        puts_("  #");
        put_u32(art_seq);
        puts_("  ");
        put_date(art_time, 1);
        puts_(" UTC  ");
        put_u32(art_size);
        puts_(" bytes");
        if (art_reply) {
            ink(C_REPLY);
            puts_("  ↳ reply");
        }
    }
    row_end();
}

static void draw_article_status(void)
{
    uint16_t last = art_top + TEXT_ROWS;
    bar_row(STATUS_ROW);
    if (art_state == READY) {
        if (last > line_count)
            last = line_count;
        puts_(" lines ");
        put_u32(line_count ? art_top + 1 : 0);
        puts_("–");
        put_u32(last);
        puts_(" of ");
        put_u32(line_count);
        if (!wrap_done) {
            puts_("+");
        } else {
            puts_("  ");
            put_u32(line_count ? (uint32_t)last * 100 / line_count : 100);
            puts_("%");
        }
    }
    pad_to(COLS - 42);
    puts_("↑↓ PgUp PgDn Home End wheel   Esc/←: back");
    row_end();
}

static void scroll_article(int32_t delta)
{
    int32_t t = (int32_t)art_top + delta;
    uint16_t max = article_end_top();
    art_to_end = 0;
    art_top = t < 0 ? 0 : t > max ? max : (uint16_t)t;
}

static void close_article(void)
{
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
    } else {
        switch (code) {
        case KEY_UP: scroll_article(-1); break;
        case KEY_DOWN: scroll_article(1); break;
        case KEY_PAGE_UP: scroll_article(-(TEXT_ROWS - 1)); break;
        case KEY_PAGE_DOWN: case KEY_SPACE: scroll_article(TEXT_ROWS - 1); break;
        case KEY_HOME: scroll_article(-(int32_t)art_top); break;
        case KEY_END: art_top = article_end_top(); art_to_end = !wrap_done; break;
        case KEY_ESCAPE: case KEY_BACKSPACE: case KEY_LEFT: close_article(); break;
        }
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
            if (view == LIST)
                scroll_list((int8_t)ev.a * 3);
            else
                scroll_article((int8_t)ev.a * 3);
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

/* Brings the view position up to date, scrolling the terminal for a
 * one-row move (SPEC 6.4) so that only the new row is drawn. Rows 0 and 29
 * move with it and are redrawn. */
static void follow(uint16_t want, uint16_t *drawn, uint8_t first, uint8_t last)
{
    if (want == *drawn)
        return;
    if (want == (uint16_t)(*drawn + 1)) {
        term_cmd(TERM_SCROLL_UP);
        mark_rows(0, first - 1);
        dirty[last] = dirty[STATUS_ROW] = 1;
    } else if ((uint16_t)(want + 1) == *drawn) {
        term_cmd(TERM_SCROLL_DOWN);
        mark_rows(0, first);
        dirty[STATUS_ROW] = 1;
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
        follow(top, &drawn_top, 1, LIST_ROWS);
        if (sel != drawn_sel) {
            if (drawn_sel >= top && drawn_sel < top + LIST_ROWS)
                dirty[drawn_sel - top + 1] = 1;
            dirty[sel - top + 1] = 1;
            drawn_sel = sel;
        }
        if (tl_handle)
            dirty[STATUS_ROW] = 1;
    } else {
        wrap_all();
        follow(art_top, &art_drawn_top, TEXT_TOP, TEXT_TOP + TEXT_ROWS - 1);
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
            else if (row < TEXT_TOP)
                draw_article_header(row);
            else
                draw_text_row(row);
        }
    }
}

int main(void)
{
    term_cmd(TERM_CURSOR_OFF);
    term_color(C_TEXT, C_BG);
    term_cmd(TERM_CLEAR);
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
