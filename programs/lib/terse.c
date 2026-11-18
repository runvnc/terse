/* terse.c: see terse.h. */

#include "terse.h"

uint8_t terse_event_pop(terse_event *e)
{
    terse_event t;
    if (!terse_call(CALL_NEXT_EVENT, (uint32_t)&t, 0, 0))
        return EV_NONE;
    *e = t;
    return t.type;
}

uint8_t terse_wait_frame(void)
{
    static uint32_t last;
    uint32_t now = terse_call(CALL_WAIT_FRAME, 0, 0, 0), n = now - last;
    last = now;
    return n > 255 ? 255 : n;
}

void terse_random(void *buf, uint32_t len)
{
    uint8_t *p = buf;
    while (len) {
        uint32_t n = len > 256 ? 256 : len;
        terse_call(CALL_RANDOM, (uint32_t)p, n, 0);
        p += n;
        len -= n;
    }
}

void term_write(const void *p, uint32_t len)
{
    const uint8_t *b = p;
    while (len) {
        uint32_t n = terse_call(CALL_TERM_WRITE, (uint32_t)b, len, 0);
        if (n > len)
            return; /* a bad buffer */
        b += n;
        len -= n;
    }
}

void term_putc(uint8_t byte)
{
    term_write(&byte, 1);
}

void term_puts(const char *s)
{
    const char *e = s;
    while (*e)
        e++;
    term_write(s, e - s);
}

void term_fg(uint8_t fg)
{
    terse_call(CALL_TERM_SET_COLOURS, fg, 0x100, 0);
}

uint32_t term_put_ascii(uint32_t max, const uint8_t *p)
{
    uint32_t n = 0;
    while (n < max && p[n] >= 0x20 && p[n] < 0x7F)
        n++;
    if (n)
        term_write(p, n);
    return n;
}

void term_put_cp(uint32_t cp)
{
    uint8_t b[3];
    if (cp < 0x80) {
        b[0] = cp;
        term_write(b, 1);
    } else if (cp < 0x800) {
        b[0] = 0xC0 | cp >> 6;
        b[1] = 0x80 | (cp & 0x3F);
        term_write(b, 2);
    } else {
        b[0] = 0xE0 | (cp >> 12 & 0x0F);
        b[1] = 0x80 | (cp >> 6 & 0x3F);
        b[2] = 0x80 | (cp & 0x3F);
        term_write(b, 3);
    }
}

void term_fill(uint8_t col, uint8_t row, uint8_t n, uint16_t ch, uint8_t fg, uint8_t bg)
{
    static uint32_t cells[TERM_COLS];
    uint32_t cell = ch | (uint32_t)fg << 16 | (uint32_t)bg << 24, i, at = (uint32_t)row * TERM_COLS + col;
    while (n) {
        uint32_t k = n > TERM_COLS ? TERM_COLS : n;
        for (i = 0; i < k; i++)
            cells[i] = cell;
        term_cells_write(at, cells, k);
        at += k;
        n -= k;
    }
}

/* Code points U+0000-U+FFFF whose Unicode 16.0 East Asian Width is W or F,
 * as inclusive ranges (first, last): the table of
 * crates/terse-core/src/wide.rs (a test checks they agree). */
static const uint16_t wide[] = {
    0x1100, 0x115F, 0x231A, 0x231B, 0x2329, 0x232A, 0x23E9, 0x23EC,
    0x23F0, 0x23F0, 0x23F3, 0x23F3, 0x25FD, 0x25FE, 0x2614, 0x2615,
    0x2630, 0x2637, 0x2648, 0x2653, 0x267F, 0x267F, 0x268A, 0x268F,
    0x2693, 0x2693, 0x26A1, 0x26A1, 0x26AA, 0x26AB, 0x26BD, 0x26BE,
    0x26C4, 0x26C5, 0x26CE, 0x26CE, 0x26D4, 0x26D4, 0x26EA, 0x26EA,
    0x26F2, 0x26F3, 0x26F5, 0x26F5, 0x26FA, 0x26FA, 0x26FD, 0x26FD,
    0x2705, 0x2705, 0x270A, 0x270B, 0x2728, 0x2728, 0x274C, 0x274C,
    0x274E, 0x274E, 0x2753, 0x2755, 0x2757, 0x2757, 0x2795, 0x2797,
    0x27B0, 0x27B0, 0x27BF, 0x27BF, 0x2B1B, 0x2B1C, 0x2B50, 0x2B50,
    0x2B55, 0x2B55, 0x2E80, 0x2E99, 0x2E9B, 0x2EF3, 0x2F00, 0x2FD5,
    0x2FF0, 0x303E, 0x3041, 0x3096, 0x3099, 0x30FF, 0x3105, 0x312F,
    0x3131, 0x318E, 0x3190, 0x31E5, 0x31EF, 0x321E, 0x3220, 0x3247,
    0x3250, 0xA48C, 0xA490, 0xA4C6, 0xA960, 0xA97C, 0xAC00, 0xD7A3,
    0xF900, 0xFAFF, 0xFE10, 0xFE19, 0xFE30, 0xFE52, 0xFE54, 0xFE66,
    0xFE68, 0xFE6B, 0xFF01, 0xFF60, 0xFFE0, 0xFFE6,
};

uint8_t term_is_wide(uint32_t cp)
{
    uint32_t lo = 0, hi = sizeof wide / sizeof wide[0] / 2;
    if (cp < 0x1100)
        return 0;
    while (lo < hi) {
        uint32_t mid = (lo + hi) / 2;
        if (cp < wide[2 * mid])
            hi = mid;
        else if (cp > wide[2 * mid + 1])
            lo = mid + 1;
        else
            return 1;
    }
    return 0;
}

uint32_t get24(const uint8_t *p)
{
    return p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16;
}

uint8_t content_get(content_req *r, const uint8_t *id, uint32_t offset, uint32_t length, void *buf)
{
    r->op = CONTENT_GET;
    memcpy(r->id, id, 32);
    r->offset = offset;
    r->length = length;
    r->addr = (uint32_t)buf;
    return content_submit(r);
}

uint8_t content_feed(content_req *r, uint8_t op, uint32_t first, uint32_t length, void *buf)
{
    r->op = op;
    r->offset = first;
    r->length = length;
    r->addr = (uint32_t)buf;
    return content_submit(r);
}

uint8_t content_put(content_req *r, const void *data, uint32_t size)
{
    r->op = CONTENT_PUT;
    r->addr = (uint32_t)data;
    r->size = size;
    return content_submit(r);
}

uint8_t content_follow(content_req *r, uint8_t op, const uint8_t *key)
{
    r->op = op;
    memcpy(r->id, key, 32);
    return content_submit(r);
}

uint8_t content_publish(content_req *r, const uint8_t *blob, const void *title, uint32_t len)
{
    r->op = CONTENT_PUBLISH;
    memcpy(r->id, blob, 32);
    r->addr = (uint32_t)title;
    r->length = len;
    return content_submit(r);
}

uint8_t content_identity(content_req *r)
{
    r->op = CONTENT_IDENTITY;
    return content_submit(r);
}

/* Content: names, search, profile and nicknames. */
uint8_t content_names(content_req *r, uint32_t first, uint32_t length, void *buf)
{
    return content_feed(r, CONTENT_NAMES, first, length, buf);
}

uint8_t content_find(content_req *r, uint8_t scope, const void *query, uint32_t qlen, void *buf, uint32_t buflen)
{
    const uint8_t *q = query;
    uint8_t *b = buf;
    if (qlen > buflen) {
        r->status = STATUS_BAD_REQUEST;
        return 0;
    }
    for (uint32_t i = 0; i < qlen; i++)
        b[i] = q[i];
    r->op = CONTENT_FIND;
    r->offset = scope;
    r->length = buflen;
    r->addr = (uint32_t)buf;
    r->size = qlen;
    return content_submit(r);
}

uint8_t content_set_profile(content_req *r, const void *name, uint32_t len, uint8_t listed)
{
    r->op = CONTENT_SET_PROFILE;
    r->offset = listed ? 1 : 0;
    r->length = len;
    r->addr = (uint32_t)name;
    return content_submit(r);
}

uint8_t content_set_nickname(content_req *r, const uint8_t *key, const void *name, uint32_t len)
{
    r->op = CONTENT_SET_NICKNAME;
    memcpy(r->id, key, 32);
    r->length = len;
    r->addr = (uint32_t)name;
    return content_submit(r);
}

/* Device 3 display-list builder. The list is packed: coordinates are
 * little-endian 16-bit values at any alignment. */
uint8_t *vec_ptr;

static void vec_xy(uint8_t op, int16_t x, int16_t y)
{
    uint8_t *p = vec_ptr;
    p[0] = op;
    p[1] = x;
    p[2] = (uint16_t)x >> 8;
    p[3] = y;
    p[4] = (uint16_t)y >> 8;
    vec_ptr = p + 5;
}

static void vec_r(uint8_t op, uint16_t r)
{
    uint8_t *p = vec_ptr;
    p[0] = op;
    p[1] = r;
    p[2] = r >> 8;
    vec_ptr = p + 3;
}

void vec_move(int16_t x, int16_t y) { vec_xy(VEC_OP_MOVE, x, y); }
void vec_line(int16_t x, int16_t y) { vec_xy(VEC_OP_LINE, x, y); }
void vec_rect(int16_t x, int16_t y) { vec_xy(VEC_OP_RECT, x, y); }
void vec_origin(int16_t x, int16_t y) { vec_xy(VEC_OP_ORIGIN, x, y); }
void vec_circle(uint16_t r) { vec_r(VEC_OP_CIRCLE, r); }
void vec_disc(uint16_t r) { vec_r(VEC_OP_DISC, r); }

void vec_width(uint8_t w)
{
    vec_ptr[0] = VEC_OP_WIDTH;
    vec_ptr[1] = w;
    vec_ptr += 2;
}

void vec_end(void)
{
    *vec_ptr++ = VEC_OP_END;
}

void vec_colour(uint8_t r, uint8_t g, uint8_t b, uint8_t a)
{
    uint8_t *p = vec_ptr;
    p[0] = VEC_OP_COLOUR;
    p[1] = r;
    p[2] = g;
    p[3] = b;
    p[4] = a;
    vec_ptr = p + 5;
}

void vec_tri(int16_t x1, int16_t y1, int16_t x2, int16_t y2)
{
    vec_xy(VEC_OP_TRIANGLE, x1, y1);
    vec_ptr[0] = x2;
    vec_ptr[1] = (uint16_t)x2 >> 8;
    vec_ptr[2] = y2;
    vec_ptr[3] = (uint16_t)y2 >> 8;
    vec_ptr += 4;
}

/* What the compiler may call. memcpy copies whole words where it can (about
 * 1.5 units a byte instead of 5): a word at a time when both pointers have
 * the same alignment, else shifting words together (never reading a word
 * that holds no byte of the source). */
void *memcpy(void *d, const void *s, size_t n)
{
    uint8_t *dp = d;
    const uint8_t *sp = s;
    if (n >= 8) {
        uint32_t *dw;
        while ((uint32_t)dp & 3) {
            *dp++ = *sp++;
            n--;
        }
        dw = (uint32_t *)dp;
        if (!((uint32_t)sp & 3)) {
            const uint32_t *sw = (const uint32_t *)sp;
            for (; n >= 4; n -= 4)
                *dw++ = *sw++;
            sp = (const uint8_t *)sw;
        } else {
            uint32_t sh = ((uint32_t)sp & 3) * 8;
            const uint32_t *sw = (const uint32_t *)((uint32_t)sp & ~3u);
            uint32_t w = *sw++;
            for (; n >= 8; n -= 4) {
                uint32_t next = *sw++;
                *dw++ = w >> sh | next << (32 - sh);
                w = next;
                sp += 4;
            }
        }
        dp = (uint8_t *)dw;
    }
    while (n--)
        *dp++ = *sp++;
    return d;
}

void *memmove(void *d, const void *s, size_t n)
{
    uint8_t *dp = d;
    const uint8_t *sp = s;
    if (dp < sp)
        return memcpy(d, s, n);
    while (n--)
        dp[n] = sp[n];
    return d;
}

void *memset(void *d, int c, size_t n)
{
    uint8_t *dp = d;
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

/* Device 7 document builder. */
uint8_t *doc_ptr, *doc_limit;
uint8_t doc_overflow;

void doc_begin(uint8_t *buf, uint32_t cap)
{
    doc_ptr = buf;
    doc_limit = buf + cap;
    doc_overflow = 0;
}

/* Room for an n-byte command at doc_ptr, or 0 (and doc_overflow set). */
static uint8_t *doc_room(uint32_t n)
{
    if (doc_overflow || (uint32_t)(doc_limit - doc_ptr) < n) {
        doc_overflow = 1;
        return 0;
    }
    uint8_t *p = doc_ptr;
    doc_ptr = p + n;
    return p;
}

/* Where to cut the text at p (longer than n) at most n bytes in: moves the
 * cut back to the start of a character (ASCII or a lead byte), at most 3
 * bytes; a longer run of continuation bytes is malformed anyway and is cut
 * where it is. */
static uint32_t doc_cut(const uint8_t *p, uint32_t n)
{
    uint32_t k = n;
    while (k && n - k < 3 && (p[k] & 0xC0) == 0x80)
        k--;
    return (p[k] & 0xC0) == 0x80 ? n : k;
}

uint32_t doc_text(const void *src, uint32_t len)
{
    const uint8_t *p = src;
    uint32_t done = 0;
    while (len) {
        uint32_t n = len > DOC_TEXT_MAX ? DOC_TEXT_MAX : len;
        uint32_t room = doc_overflow ? 0 : (uint32_t)(doc_limit - doc_ptr);
        uint8_t full = room < 3 + n;
        if (full)
            n = room > 3 ? room - 3 : 0;
        if (n < len)
            n = doc_cut(p, n);
        if (n) {
            uint8_t *d = doc_ptr;
            d[0] = DOC_OP_TEXT;
            d[1] = n;
            d[2] = n >> 8;
            memcpy(d + 3, p, n);
            doc_ptr = d + 3 + n;
        }
        p += n;
        len -= n;
        done += n;
        if (full) {
            doc_overflow = 1;
            break;
        }
    }
    return done;
}

void doc_font(uint8_t face, uint8_t flags)
{
    uint8_t *p = doc_room(3);
    if (p) {
        p[0] = DOC_OP_FONT;
        p[1] = face;
        p[2] = flags;
    }
}

void doc_size(uint8_t size)
{
    uint8_t *p = doc_room(2);
    if (p) {
        p[0] = DOC_OP_SIZE;
        p[1] = size;
    }
}

void doc_colour(uint8_t r, uint8_t g, uint8_t b)
{
    uint8_t *p = doc_room(4);
    if (p) {
        p[0] = DOC_OP_COLOUR;
        p[1] = r;
        p[2] = g;
        p[3] = b;
    }
}

void doc_paragraph(uint8_t align, uint8_t indent, uint8_t space, uint8_t flags)
{
    uint8_t *p = doc_room(5);
    if (p) {
        p[0] = DOC_OP_PARAGRAPH;
        p[1] = align;
        p[2] = indent;
        p[3] = space;
        p[4] = flags;
    }
}

void doc_rule(void)
{
    uint8_t *p = doc_room(1);
    if (p)
        p[0] = DOC_OP_RULE;
}

void doc_anchor(uint16_t id)
{
    uint8_t *p = doc_room(3);
    if (p) {
        p[0] = DOC_OP_ANCHOR;
        p[1] = id;
        p[2] = id >> 8;
    }
}

void doc_end(void)
{
    uint8_t *p = doc_room(1);
    if (p)
        p[0] = DOC_OP_END;
}
