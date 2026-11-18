/* terse.c: see terse.h. */

#include "terse.h"

/* terse_ticks when terse_wait_frame last returned. */
static uint8_t terse_seen;

uint8_t terse_wait_frame(void) __naked
{
    /* `ei` takes effect after the next instruction, so no interrupt can be
     * accepted between it and `halt`: either the frame interrupt has already
     * been counted (no halt) or it wakes the halt. */
    __asm
        di
        ld   hl, #_terse_seen
        ld   a, (_terse_ticks)
        cp   a, (hl)
        jr   nz, 1$
        ei
        halt
        di
1$:     ld   a, (_terse_ticks)
        ld   c, a
        sub  a, (hl)
        ld   (hl), c
        ei
        ret
    __endasm;
}

uint8_t terse_in(uint8_t port) __naked
{
    (void)port;
    __asm
        ld   c, a
        in   a, (c)
        ret
    __endasm;
}

void terse_out(uint8_t port, uint8_t value) __naked
{
    (void)port;
    (void)value;
    __asm
        ld   c, a
        out  (c), l
        ret
    __endasm;
}

uint8_t terse_event_pop(terse_event *e)
{
    uint8_t type = SYS_EVENT;
    if (type != EV_NONE) {
        e->type = type;
        e->a = SYS_EVENT_A;
        e->b = SYS_EVENT_B;
        e->c = SYS_EVENT_C;
        SYS_EVENT = 0;
    }
    return type;
}

static uint32_t read32(uint8_t port)
{
    union {
        uint32_t v;
        uint8_t b[4];
    } u;
    uint8_t i;
    for (i = 0; i < 4; i++)
        u.b[i] = terse_in(port + i);
    return u.v;
}

uint32_t terse_frame(void) { return read32(0x14); }
uint32_t terse_time(void) { return read32(0x18); }

void terse_pad_read(uint8_t n, terse_pad *pad)
{
    uint8_t *p = (uint8_t *)pad;
    uint8_t i;
    SYS_PAD_SELECT = n;
    for (i = 0; i < 7; i++)
        p[i] = SYS_PAD_DATA;
}

void term_puts(const char *s)
{
    while (*s)
        TERM_PUT = *s++;
}

uint8_t term_put_ascii(uint8_t max, const uint8_t *p) __naked
{
    (void)max;
    (void)p;
    __asm
        ld   c, a       ; max (a), p (de): sdcccall(1)
        or   a, a
        ret  z
        ld   b, a       ; counts down
        ex   de, hl
1$:     ld   a, (hl)
        cp   a, #0x20
        jr   c, 2$
        cp   a, #0x7F
        jr   nc, 2$
        out  (_TERM_PUT), a
        inc  hl
        djnz 1$
        ld   a, c
        ret
2$:     ld   a, c
        sub  a, b
        ret
    __endasm;
}

void term_put_cp(uint16_t cp) __naked
{
    (void)cp;
    __asm
        ld   a, h           ; cp in hl
        cp   a, #0x08
        jr   nc, 3$
        or   a, a
        jr   nz, 2$
        ld   a, l
        cp   a, #0x80
        jr   nc, 2$
        out  (_TERM_PUT), a ; one byte
        ret
2$:     ld   a, l           ; two bytes: C0h | cp >> 6
        rlca
        rlca
        and  a, #0x03
        ld   c, a
        ld   a, h
        add  a, a
        add  a, a
        or   a, c
        or   a, #0xC0
        out  (_TERM_PUT), a
        jr   4$
3$:     rrca                ; three bytes: E0h | cp >> 12
        rrca
        rrca
        rrca
        and  a, #0x0F
        or   a, #0xE0
        out  (_TERM_PUT), a
        ld   a, l           ; 80h | (cp >> 6 & 3Fh)
        rlca
        rlca
        and  a, #0x03
        ld   c, a
        ld   a, h
        and  a, #0x0F
        add  a, a
        add  a, a
        or   a, c
        or   a, #0x80
        out  (_TERM_PUT), a
4$:     ld   a, l           ; 80h | (cp & 3Fh)
        and  a, #0x3F
        or   a, #0x80
        out  (_TERM_PUT), a
        ret
    __endasm;
}

void term_cell_ptr(uint8_t col, uint8_t row)
{
    uint16_t at = ((uint16_t)row * TERM_COLS + col) * 4;
    TERM_PTR_LO = at;
    TERM_PTR_HI = at >> 8;
}

void term_fill(uint8_t col, uint8_t row, uint8_t n, uint16_t ch, uint8_t fg, uint8_t bg) __naked
{
    (void)col;
    (void)row;
    (void)n;
    (void)ch;
    (void)fg;
    (void)bg;
    __asm
        ld   c, a           ; col (a), row (l); n, ch, fg, bg on the stack
        ld   h, #0
        ld   d, h
        ld   e, l
        add  hl, hl
        add  hl, hl
        add  hl, de         ; row * 5
        add  hl, hl
        add  hl, hl
        add  hl, hl
        add  hl, hl         ; row * 80
        ld   b, #0
        add  hl, bc
        add  hl, hl
        add  hl, hl         ; (row * 80 + col) * 4
        ld   a, l
        out  (_TERM_PTR_LO), a
        ld   a, h
        out  (_TERM_PTR_HI), a
        ld   hl, #2
        add  hl, sp
        ld   b, (hl)        ; n
        inc  hl
        ld   e, (hl)        ; ch
        inc  hl
        ld   d, (hl)
        inc  hl
        ld   a, (hl)        ; fg
        inc  hl
        ld   h, (hl)        ; bg
        ld   l, a
        inc  b
        ld   c, #_TERM_DATA
        jr   2$
1$:     out  (c), e         ; about 60 T-states a cell
        out  (c), d
        out  (c), l
        out  (c), h
2$:     djnz 1$
        pop  hl             ; return, dropping the 5 bytes of arguments
        inc  sp
        inc  sp
        inc  sp
        inc  sp
        inc  sp
        jp   (hl)
    __endasm;
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

uint8_t term_is_wide(uint16_t cp)
{
    uint8_t lo = 0, hi = sizeof wide / sizeof wide[0] / 2, mid;
    const uint16_t *r;
    if (cp < 0x1100)
        return 0;
    /* The common cases first: CJK, kana, Hangul, fullwidth forms. */
    if ((cp >= 0x3250 && cp <= 0xA48C) || (cp >= 0x3041 && cp <= 0x30FF && cp != 0x3097 && cp != 0x3098)
        || (cp >= 0xAC00 && cp <= 0xD7A3) || (cp >= 0xFF01 && cp <= 0xFF60))
        return 1;
    while (lo < hi) {
        mid = (lo + hi) >> 1;
        r = wide + (mid << 1);
        if (cp < r[0])
            hi = mid;
        else if (cp > r[1])
            lo = mid + 1;
        else
            return 1;
    }
    return 0;
}

uint32_t get24(const uint8_t *p)
{
    return p[0] | (uint16_t)p[1] << 8 | (uint32_t)p[2] << 16;
}

void set24(uint8_t *p, uint32_t v)
{
    p[0] = v;
    p[1] = v >> 8;
    p[2] = v >> 16;
}

uint32_t terse_linear(const void *p)
{
    uint16_t a = (uint16_t)p;
    uint8_t page = terse_in(a >> 13);
    if (page < PAGE_RAM)
        return 0xFFFFFFFF;
    return (uint32_t)(page - PAGE_RAM) << 13 | (a & 0x1FFF);
}

uint8_t content_submit(content_req *r)
{
    CONTENT_BLOCK_LO = (uint16_t)r;
    CONTENT_BLOCK_HI = (uint16_t)r >> 8;
    CONTENT_SUBMIT = 0;
    return CONTENT_SUBMIT;
}

uint8_t content_get(content_req *r, const uint8_t *id, uint32_t offset, uint16_t length, uint32_t addr)
{
    uint8_t i;
    r->op = CONTENT_GET;
    for (i = 0; i < 32; i++)
        r->id[i] = id[i];
    set24(r->offset, offset);
    r->length = length;
    set24(r->addr, addr);
    return content_submit(r);
}

uint8_t content_feed(content_req *r, uint8_t op, uint32_t first, uint16_t length, uint32_t addr)
{
    r->op = op;
    set24(r->offset, first);
    r->length = length;
    set24(r->addr, addr);
    return content_submit(r);
}

/* Device 3 display-list builder (see terse.h). */
uint8_t *vec_ptr;

void vec_move(int16_t x, int16_t y) __naked
{
    (void)x;
    (void)y;
    __asm
        ld   a, #0x04
vec_xy:                     ; opcode in a, x in hl, y in de (sdcccall(1))
        ld   c, l
        ld   b, h
        ld   hl, (_vec_ptr)
        ld   (hl), a
        inc  hl
        ld   (hl), c
        inc  hl
        ld   (hl), b
        inc  hl
        ld   (hl), e
        inc  hl
        ld   (hl), d
        inc  hl
        ld   (_vec_ptr), hl
        ret
    __endasm;
}

void vec_line(int16_t x, int16_t y) __naked
{
    (void)x;
    (void)y;
    __asm
        ld   a, #0x05
        jr   vec_xy
    __endasm;
}

void vec_rect(int16_t x, int16_t y) __naked
{
    (void)x;
    (void)y;
    __asm
        ld   a, #0x08
        jr   vec_xy
    __endasm;
}

void vec_origin(int16_t x, int16_t y) __naked
{
    (void)x;
    (void)y;
    __asm
        ld   a, #0x03
        jr   vec_xy
    __endasm;
}

void vec_circle(uint16_t r) __naked
{
    (void)r;
    __asm
        ld   a, #0x06
vec_r:                      ; opcode in a, radius in hl
        ex   de, hl
        ld   hl, (_vec_ptr)
        ld   (hl), a
        inc  hl
        ld   (hl), e
        inc  hl
        ld   (hl), d
        inc  hl
        ld   (_vec_ptr), hl
        ret
    __endasm;
}

void vec_disc(uint16_t r) __naked
{
    (void)r;
    __asm
        ld   a, #0x07
        jr   vec_r
    __endasm;
}

void vec_width(uint8_t w) __naked
{
    (void)w;
    __asm
        ld   hl, (_vec_ptr)
        ld   (hl), #0x02
        inc  hl
        ld   (hl), a
        inc  hl
        ld   (_vec_ptr), hl
        ret
    __endasm;
}

void vec_end(void) __naked
{
    __asm
        ld   hl, (_vec_ptr)
        ld   (hl), #0x00
        inc  hl
        ld   (_vec_ptr), hl
        ret
    __endasm;
}

void vec_colour(uint8_t r, uint8_t g, uint8_t b, uint8_t a) __naked
{
    (void)r;
    (void)g;
    (void)b;
    (void)a;
    __asm
        ld   b, a           ; r (a), g (l); b and a on the stack, popped here
        ld   c, l
        pop  de             ; return address
        pop  hl             ; l = b, h = a
        push de
        ex   de, hl
        ld   hl, (_vec_ptr)
        ld   (hl), #0x01
        inc  hl
        ld   (hl), b
        inc  hl
        ld   (hl), c
        inc  hl
        ld   (hl), e
        inc  hl
        ld   (hl), d
        inc  hl
        ld   (_vec_ptr), hl
        ret
    __endasm;
}

void vec_tri(int16_t x1, int16_t y1, int16_t x2, int16_t y2) __naked
{
    (void)x1;
    (void)y1;
    (void)x2;
    (void)y2;
    __asm
        ld   c, l           ; x1 (hl), y1 (de); x2, y2 on the stack, popped here
        ld   b, h
        ld   hl, (_vec_ptr)
        ld   (hl), #0x09
        inc  hl
        ld   (hl), c
        inc  hl
        ld   (hl), b
        inc  hl
        ld   (hl), e
        inc  hl
        ld   (hl), d
        inc  hl
        ex   de, hl
        pop  bc             ; return address
        pop  hl             ; x2
        ex   de, hl
        ld   (hl), e
        inc  hl
        ld   (hl), d
        inc  hl
        pop  de             ; y2
        push bc
        ld   (hl), e
        inc  hl
        ld   (hl), d
        inc  hl
        ld   (_vec_ptr), hl
        ret
    __endasm;
}

uint8_t vec_submit(uint32_t addr, uint16_t len)
{
    VEC_ADDR0 = addr;
    VEC_ADDR1 = addr >> 8;
    VEC_ADDR2 = addr >> 16;
    VEC_LEN_LO = len;
    VEC_LEN_HI = len >> 8;
    VEC_CMD = VEC_SUBMIT;
    return VEC_CMD;
}

uint8_t vec_submit_list(const uint8_t *buf)
{
    return vec_submit(terse_linear(buf), vec_length(buf));
}
