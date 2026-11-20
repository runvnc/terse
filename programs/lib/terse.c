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
    /* write takes the whole buffer, at most TERM_WRITE_MAX bytes a call. */
    const uint8_t *b = p;
    while (len) {
        uint32_t n = len > TERM_WRITE_MAX ? TERM_WRITE_MAX : len;
        if (terse_call(CALL_TERM_WRITE, (uint32_t)b, n, 0) != n)
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
    uint8_t b[4];
    if (cp > 0x10FFFF || (cp >= 0xD800 && cp < 0xE000))
        cp = 0xFFFD;
    if (cp < 0x80) {
        b[0] = cp;
        term_write(b, 1);
    } else if (cp < 0x800) {
        b[0] = 0xC0 | cp >> 6;
        b[1] = 0x80 | (cp & 0x3F);
        term_write(b, 2);
    } else if (cp < 0x10000) {
        b[0] = 0xE0 | cp >> 12;
        b[1] = 0x80 | (cp >> 6 & 0x3F);
        b[2] = 0x80 | (cp & 0x3F);
        term_write(b, 3);
    } else {
        b[0] = 0xF0 | cp >> 18;
        b[1] = 0x80 | (cp >> 12 & 0x3F);
        b[2] = 0x80 | (cp >> 6 & 0x3F);
        b[3] = 0x80 | (cp & 0x3F);
        term_write(b, 4);
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

uint32_t content_get(content_req *r, const uint8_t *id, uint32_t offset, uint32_t length, void *buf)
{
    r->op = CONTENT_GET;
    memcpy(r->id, id, 32);
    r->offset = offset;
    r->length = length;
    r->addr = (uint32_t)buf;
    return content_submit(r);
}

uint32_t content_feed(content_req *r, uint8_t op, uint32_t first, uint32_t length, void *buf)
{
    r->op = op;
    r->offset = first;
    r->length = length;
    r->addr = (uint32_t)buf;
    return content_submit(r);
}

uint32_t content_put(content_req *r, const void *data, uint32_t size)
{
    r->op = CONTENT_PUT;
    r->addr = (uint32_t)data;
    r->size = size;
    return content_submit(r);
}

static uint32_t follow_hints(content_req *r, uint8_t op, const uint8_t *key, const void *hints, uint32_t len)
{
    r->op = op;
    memcpy(r->id, key, 32);
    r->addr = (uint32_t)hints;
    r->length = len;
    return content_submit(r);
}

uint32_t content_follow(content_req *r, uint8_t op, const uint8_t *key)
{
    return follow_hints(r, op, key, 0, 0);
}

uint32_t content_subscribe(content_req *r, const uint8_t *key, const void *hints, uint32_t len)
{
    return follow_hints(r, CONTENT_SUBSCRIBE, key, hints, len);
}

uint32_t content_publish(content_req *r, const uint8_t *blob, const void *title, uint32_t len)
{
    return content_publish_kind(r, blob, title, len, 0);
}

uint32_t content_publish_kind(content_req *r, const uint8_t *blob, const void *title, uint32_t len, uint8_t kind)
{
    r->op = CONTENT_PUBLISH;
    memcpy(r->id, blob, 32);
    r->offset = kind;
    r->addr = (uint32_t)title;
    r->length = len;
    return content_submit(r);
}

uint32_t content_identity(content_req *r)
{
    return content_identity_hints(r, 0, 0);
}

uint32_t content_identity_hints(content_req *r, void *buf, uint32_t len)
{
    r->op = CONTENT_IDENTITY;
    r->addr = (uint32_t)buf;
    r->length = len;
    return content_submit(r);
}

/* Content: names, search, profile and nicknames. */
uint32_t content_names(content_req *r, uint32_t first, uint32_t length, void *buf)
{
    return content_feed(r, CONTENT_NAMES, first, length, buf);
}

uint32_t content_find(content_req *r, uint8_t scope, const void *query, uint32_t qlen, void *buf, uint32_t buflen)
{
    if (query != buf)
        memmove(buf, query, qlen);
    r->op = CONTENT_FIND;
    r->offset = scope;
    r->length = buflen;
    r->addr = (uint32_t)buf;
    r->size = qlen;
    return content_submit(r);
}

uint32_t content_set_profile(content_req *r, const void *name, uint32_t len, uint8_t listed)
{
    r->op = CONTENT_SET_PROFILE;
    r->offset = listed ? 1 : 0;
    r->length = len;
    r->addr = (uint32_t)name;
    return content_submit(r);
}

uint32_t content_set_nickname(content_req *r, const uint8_t *key, const void *name, uint32_t len)
{
    r->op = CONTENT_SET_NICKNAME;
    memcpy(r->id, key, 32);
    r->length = len;
    r->addr = (uint32_t)name;
    return content_submit(r);
}

/* Content: programs and named records (SPEC 9.5, 9.6). */
uint32_t content_programs(content_req *r, uint32_t first, uint32_t length, void *buf)
{
    return content_feed(r, CONTENT_PROGRAMS, first, length, buf);
}

/* The record name: `name` up to its 0, zero padded to 32 bytes. A name
 * longer than 32 bytes is not cut (a cut could also split a character):
 * 0, and r->status is STATUS_BAD_REQUEST, as the device answers an
 * invalid name. */
static uint8_t record_name(content_req *r, const char *name)
{
    uint8_t i = 0;
    for (; i < 32 && name[i]; i++)
        r->id[i] = name[i];
    if (i == 32 && name[i]) {
        r->status = STATUS_BAD_REQUEST;
        return 0;
    }
    for (; i < 32; i++)
        r->id[i] = 0;
    return 1;
}

uint32_t record_save(content_req *r, const char *name, const void *data, uint32_t size)
{
    if (!record_name(r, name))
        return 0;
    r->op = CONTENT_SAVE;
    r->addr = (uint32_t)data;
    r->size = size;
    return content_submit(r);
}

uint32_t record_load(content_req *r, const char *name, uint32_t offset, uint32_t length, void *buf)
{
    if (!record_name(r, name))
        return 0;
    r->op = CONTENT_LOAD;
    r->offset = offset;
    r->length = length;
    r->addr = (uint32_t)buf;
    return content_submit(r);
}

uint32_t record_delete(content_req *r, const char *name)
{
    if (!record_name(r, name))
        return 0;
    r->op = CONTENT_DELETE;
    return content_submit(r);
}

uint32_t record_list(content_req *r, uint32_t first, uint32_t length, void *buf)
{
    return content_feed(r, CONTENT_RECORDS, first, length, buf);
}

/* Device 8: start_program with a record built here. */
uint32_t mach_start_named(const uint8_t *key, const void *name, uint32_t len)
{
    uint8_t rec[33 + 48];
    if (len > 48)
        return TERSE_ERR_BUFFER;
    memcpy(rec, key, 32);
    rec[32] = (uint8_t)len;
    memcpy(rec + 33, name, len);
    return mach_start_program(rec);
}

/* Device 3 display-list builder. The list is packed: coordinates are
 * little-endian 16-bit values at any alignment. */
uint8_t *vec_ptr, *vec_limit;
uint8_t vec_overflow;

void vec_begin(uint8_t *buf, uint32_t cap)
{
    vec_ptr = buf;
    vec_limit = buf + cap;
    vec_overflow = 0;
}

/* Room for an n-byte command at vec_ptr, its opcode written; or 0 (and
 * vec_overflow set). */
static uint8_t *vec_room(uint32_t n, uint8_t op)
{
    if (vec_overflow || (uint32_t)(vec_limit - vec_ptr) < n) {
        vec_overflow = 1;
        return 0;
    }
    uint8_t *p = vec_ptr;
    vec_ptr = p + n;
    p[0] = op;
    return p + 1;
}

static void put16(uint8_t *p, uint32_t v)
{
    p[0] = v;
    p[1] = v >> 8;
}

static void vec_xy(uint8_t op, int16_t x, int16_t y)
{
    uint8_t *p = vec_room(5, op);
    if (p) {
        put16(p, (uint16_t)x);
        put16(p + 2, (uint16_t)y);
    }
}

static void vec_r(uint8_t op, uint16_t r)
{
    uint8_t *p = vec_room(3, op);
    if (p)
        put16(p, r);
}

void vec_move(int16_t x, int16_t y) { vec_xy(VEC_OP_MOVE, x, y); }
void vec_line(int16_t x, int16_t y) { vec_xy(VEC_OP_LINE, x, y); }
void vec_rect(int16_t x, int16_t y) { vec_xy(VEC_OP_RECT, x, y); }
void vec_origin(int16_t x, int16_t y) { vec_xy(VEC_OP_ORIGIN, x, y); }
void vec_circle(uint16_t r) { vec_r(VEC_OP_CIRCLE, r); }
void vec_disc(uint16_t r) { vec_r(VEC_OP_DISC, r); }

void vec_width(uint8_t w)
{
    uint8_t *p = vec_room(2, VEC_OP_WIDTH);
    if (p)
        p[0] = w;
}

void vec_end(void)
{
    vec_room(1, VEC_OP_END);
}

static void rgba(uint8_t *p, uint8_t r, uint8_t g, uint8_t b, uint8_t a)
{
    p[0] = r;
    p[1] = g;
    p[2] = b;
    p[3] = a;
}

void vec_colour(uint8_t r, uint8_t g, uint8_t b, uint8_t a)
{
    uint8_t *p = vec_room(5, VEC_OP_COLOUR);
    if (p)
        rgba(p, r, g, b, a);
}

void vec_tri(int16_t x1, int16_t y1, int16_t x2, int16_t y2)
{
    uint8_t *p = vec_room(9, VEC_OP_TRIANGLE);
    if (p) {
        put16(p, (uint16_t)x1);
        put16(p + 2, (uint16_t)y1);
        put16(p + 4, (uint16_t)x2);
        put16(p + 6, (uint16_t)y2);
    }
}

/* Device 9 display-list builder, packed the same way. */
uint8_t *v3_ptr, *v3_limit;
uint8_t v3_overflow;

void v3_begin(uint8_t *buf, uint32_t cap)
{
    v3_ptr = buf;
    v3_limit = buf + cap;
    v3_overflow = 0;
}

/* Room for an n-byte command at v3_ptr, its opcode written; or 0 (and
 * v3_overflow set). */
static uint8_t *v3_room(uint32_t n, uint8_t op)
{
    if (v3_overflow || (uint32_t)(v3_limit - v3_ptr) < n) {
        v3_overflow = 1;
        return 0;
    }
    uint8_t *p = v3_ptr;
    v3_ptr = p + n;
    p[0] = op;
    return p + 1;
}

static void v3_xyz(uint8_t op, int16_t x, int16_t y, int16_t z)
{
    uint8_t *p = v3_room(7, op);
    if (p) {
        put16(p, (uint16_t)x);
        put16(p + 2, (uint16_t)y);
        put16(p + 4, (uint16_t)z);
    }
}

static void v3_u16(uint8_t op, uint16_t v)
{
    uint8_t *p = v3_room(3, op);
    if (p)
        put16(p, v);
}

void v3_move(int16_t x, int16_t y, int16_t z) { v3_xyz(V3_OP_MOVE, x, y, z); }
void v3_line(int16_t x, int16_t y, int16_t z) { v3_xyz(V3_OP_LINE, x, y, z); }
void v3_translate(int16_t x, int16_t y, int16_t z) { v3_xyz(V3_OP_TRANSLATE, x, y, z); }
void v3_width(uint16_t w) { v3_u16(V3_OP_WIDTH, w); }
void v3_circle(uint16_t r) { v3_u16(V3_OP_CIRCLE, r); }
void v3_scale(uint16_t s) { v3_u16(V3_OP_SCALE, s); }
void v3_point(void) { v3_room(1, V3_OP_POINT); }
void v3_push(void) { v3_room(1, V3_OP_PUSH); }
void v3_pop(void) { v3_room(1, V3_OP_POP); }
void v3_end(void) { v3_room(1, V3_OP_END); }

void v3_screen(uint8_t handle, uint16_t width)
{
    uint8_t *p = v3_room(4, V3_OP_SCREEN);
    if (p) {
        p[0] = handle;
        put16(p + 1, width);
    }
}

void v3_surface(uint16_t x, uint16_t y, uint16_t w, uint16_t h, uint16_t width)
{
    uint8_t *p = v3_room(11, V3_OP_SURFACE);
    if (p) {
        put16(p, x);
        put16(p + 2, y);
        put16(p + 4, w);
        put16(p + 6, h);
        put16(p + 8, width);
    }
}

void v3_rotate(uint8_t axis, uint16_t angle)
{
    uint8_t *p = v3_room(4, V3_OP_ROTATE);
    if (p) {
        p[0] = axis;
        put16(p + 1, angle);
    }
}

/* A face of `n` corners. */
static void v3_face(uint8_t n, const int16_t *const *corners)
{
    uint8_t *p = v3_room(2 + 6 * (uint32_t)n, V3_OP_FACE);
    if (!p)
        return;
    *p++ = n;
    for (uint8_t i = 0; i < n; i++)
        for (uint8_t k = 0; k < 3; k++, p += 2)
            put16(p, (uint16_t)corners[i][k]);
}

void v3_face3(const int16_t *a, const int16_t *b, const int16_t *c)
{
    const int16_t *corners[3] = { a, b, c };
    v3_face(3, corners);
}

void v3_face4(const int16_t *a, const int16_t *b, const int16_t *c, const int16_t *d)
{
    const int16_t *corners[4] = { a, b, c, d };
    v3_face(4, corners);
}

void v3_colour(uint8_t r, uint8_t g, uint8_t b, uint8_t a)
{
    uint8_t *p = v3_room(5, V3_OP_COLOUR);
    if (p)
        rgba(p, r, g, b, a);
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

uint8_t page_place_event(const terse_event *e, page_place *p)
{
    if (e->type == EV_PAGE_LOCATED) {
        p->id = e->a | (uint16_t)e->b << 8;
        p->height = e->c;
        p->half = 1;
    } else if (e->type == EV_PAGE_LOCATED_Y && p->half) {
        p->y = e->a | (uint32_t)e->b << 8 | (uint32_t)e->c << 16;
        p->half = 0;
        return 1;
    }
    return 0;
}

void doc_end(void)
{
    uint8_t *p = doc_room(1);
    if (p)
        p[0] = DOC_OP_END;
}

void doc_figure(uint16_t width, uint16_t height, uint8_t align, const void *path, uint16_t n)
{
    uint8_t *p = doc_room(DOC_FIGURE_HEAD + n);
    if (p) {
        p[0] = DOC_OP_FIGURE;
        put16(p + 1, width);
        put16(p + 3, height);
        p[5] = align;
        put16(p + 6, n);
        if (p + DOC_FIGURE_HEAD != path)
            memmove(p + DOC_FIGURE_HEAD, path, n);
    }
}

/* Figure path builder. */
uint8_t *path_ptr, *path_limit;
uint8_t path_overflow;

void path_begin(uint8_t *buf, uint32_t cap)
{
    path_ptr = buf;
    path_limit = buf + cap;
    path_overflow = 0;
}

/* Room for an n-byte command at path_ptr, its opcode written; or 0 (and
 * path_overflow set). */
static uint8_t *path_room(uint32_t n, uint8_t op)
{
    if (path_overflow || (uint32_t)(path_limit - path_ptr) < n) {
        path_overflow = 1;
        return 0;
    }
    uint8_t *p = path_ptr;
    path_ptr = p + n;
    p[0] = op;
    return p + 1;
}

void path_fill(uint8_t r, uint8_t g, uint8_t b, uint8_t a)
{
    uint8_t *p = path_room(5, PATH_FILL);
    if (p)
        rgba(p, r, g, b, a);
}

void path_stroke(uint8_t r, uint8_t g, uint8_t b, uint8_t a, uint16_t width)
{
    uint8_t *p = path_room(7, PATH_STROKE);
    if (p) {
        rgba(p, r, g, b, a);
        put16(p + 4, width);
    }
}

void path_points(uint8_t op, uint32_t n, const int16_t *xy)
{
    uint8_t *p = path_room(1 + 4 * n, op);
    if (p)
        for (n *= 2; n; n--, xy++, p += 2)
            put16(p, (uint16_t)*xy);
}

void path_move(int16_t x, int16_t y)
{
    int16_t xy[2] = {x, y};
    path_points(PATH_MOVE, 1, xy);
}

void path_line(int16_t x, int16_t y)
{
    int16_t xy[2] = {x, y};
    path_points(PATH_LINE, 1, xy);
}

void path_quad(int16_t x1, int16_t y1, int16_t x, int16_t y)
{
    int16_t xy[4] = {x1, y1, x, y};
    path_points(PATH_QUAD, 2, xy);
}

void path_cubic(int16_t x1, int16_t y1, int16_t x2, int16_t y2, int16_t x, int16_t y)
{
    int16_t xy[6] = {x1, y1, x2, y2, x, y};
    path_points(PATH_CUBIC, 3, xy);
}

void path_close(void)
{
    path_room(1, PATH_CLOSE);
}

/* --- Page tables ------------------------------------------------------ */

void doc_table(uint8_t columns, uint8_t flags, const uint8_t *align)
{
    uint8_t *p = doc_room(3 + (uint32_t)columns), k;
    if (p) {
        p[0] = DOC_OP_TABLE;
        p[1] = columns;
        p[2] = flags;
        for (k = 0; k < columns; k++)
            p[3 + k] = align ? align[k] : DOC_START;
    }
}

void doc_cell(void)
{
    uint8_t *p = doc_room(1);
    if (p)
        p[0] = DOC_OP_CELL;
}

void doc_table_end(void)
{
    uint8_t *p = doc_room(1);
    if (p)
        p[0] = DOC_OP_END_TABLE;
}

/* Device 6 command-list builder (SPEC 18.3). Its state has a section of
 * its own: the compiler would otherwise merge it with the vector
 * builder's globals, and --gc-sections could not drop it from programs
 * that make no sound (their images would change). */
#define SND_STATE __attribute__((section(".bss.snd_builder")))
SND_STATE uint8_t *snd_ptr;
SND_STATE uint8_t *snd_limit;
SND_STATE uint8_t snd_overflow;

void snd_begin(uint8_t *buf, uint32_t cap)
{
    snd_ptr = buf;
    snd_limit = buf + cap;
    snd_overflow = 0;
}

static uint8_t *snd_room(uint32_t n, uint8_t op)
{
    if (snd_overflow || (uint32_t)(snd_limit - snd_ptr) < n) {
        snd_overflow = 1;
        return 0;
    }
    uint8_t *p = snd_ptr;
    snd_ptr = p + n;
    p[0] = op;
    return p + 1;
}

static void snd_op0(uint8_t op) { snd_room(1, op); }

static void snd_op1(uint8_t op, uint8_t a)
{
    uint8_t *p = snd_room(2, op);
    if (p)
        p[0] = a;
}

static void snd_op2(uint8_t op, uint8_t a, uint8_t b)
{
    uint8_t *p = snd_room(3, op);
    if (p) {
        p[0] = a;
        p[1] = b;
    }
}

void snd_voice(uint8_t v) { snd_op1(SND_OP_VOICE, v); }
void snd_wave(uint8_t slot) { snd_op1(SND_OP_WAVE, slot); }
void snd_noise(void) { snd_op0(SND_OP_NOISE); }
void snd_sample(uint8_t slot) { snd_op1(SND_OP_SAMPLE, slot); }
void snd_volume(uint8_t volume) { snd_op1(SND_OP_VOLUME, volume); }
void snd_pan(uint8_t left, uint8_t right) { snd_op2(SND_OP_PAN, left, right); }
void snd_key_on(void) { snd_op0(SND_OP_KEY_ON); }
void snd_key_off(void) { snd_op0(SND_OP_KEY_OFF); }
void snd_modulator(uint8_t m, uint8_t depth) { snd_op2(SND_OP_MODULATOR, m, depth); }
void snd_feedback(uint8_t depth) { snd_op1(SND_OP_FEEDBACK, depth); }
void snd_ring(uint8_t q) { snd_op1(SND_OP_RING, q); }
void snd_mix(uint8_t on) { snd_op1(SND_OP_MIX, on); }

void snd_frequency(uint32_t increment)
{
    uint8_t *p = snd_room(5, SND_OP_FREQUENCY);
    if (p) {
        p[0] = increment;
        p[1] = increment >> 8;
        p[2] = increment >> 16;
        p[3] = increment >> 24;
    }
}

void snd_envelope(uint16_t attack, uint16_t decay, uint8_t sustain, uint16_t release)
{
    uint8_t *p = snd_room(8, SND_OP_ENVELOPE);
    /* Bytes written one by one, not with put16: another caller of that
     * static helper would change how the vector builders compile. */
    if (p) {
        p[0] = attack;
        p[1] = attack >> 8;
        p[2] = decay;
        p[3] = decay >> 8;
        p[4] = sustain;
        p[5] = release;
        p[6] = release >> 8;
    }
}
