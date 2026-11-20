/* terse.h: the Terse machine (SPEC.md, profile 1: RV32IMC) for C programs
 * built with clang. Devices are reached with `ecall`: a7 = (slot << 8) |
 * function, arguments in a0-a5, result in a0 (and a1); SPEC 3. */

#ifndef TERSE_H
#define TERSE_H

#include <stdint.h>
#include <stddef.h>

/* Memory (SPEC 1.2). */
#define TERSE_ROM_BASE 0x00010000u
#define TERSE_RAM_BASE 0x00100000u

/* Call numbers (SPEC 3.2). */
#define CALL_WAIT_FRAME 0x100
#define CALL_NEXT_EVENT 0x101
#define CALL_FRAME 0x102
#define CALL_TIME 0x103
#define CALL_RANDOM 0x104
#define CALL_CONTROLLER 0x105
#define CALL_EXIT 0x106
#define CALL_OPEN 0x107
#define CALL_ARGUMENT 0x108
#define CALL_TERM_WRITE 0x200
#define CALL_TERM_COMMAND 0x201
#define CALL_TERM_SET_CURSOR 0x202
#define CALL_TERM_GET_CURSOR 0x203
#define CALL_TERM_SET_COLOURS 0x204
#define CALL_TERM_GET_COLOURS 0x205
#define CALL_TERM_CELLS_READ 0x206
#define CALL_TERM_CELLS_WRITE 0x207
#define CALL_TERM_PALETTE_SET 0x208
#define CALL_TERM_SIZE 0x209
#define CALL_VEC_SUBMIT 0x300
#define CALL_VEC_CLEAR 0x301
#define CALL_VEC_STYLE 0x302
#define CALL_VEC_PERSISTENCE 0x303
#define CALL_VEC_FILL 0x304
#define CALL_CONTENT_SUBMIT 0x400
#define CALL_SND_SUBMIT 0x600
#define CALL_SND_WAVE 0x601
#define CALL_SND_SAMPLE 0x602
#define CALL_SND_INFO 0x603
#define CALL_PAGE_SUBMIT 0x700
#define CALL_PAGE_CLEAR 0x701
#define CALL_PAGE_REGION 0x702
#define CALL_PAGE_SCROLL 0x703
#define CALL_PAGE_BACKGROUND 0x704
#define CALL_PAGE_LOCATE 0x705
#define CALL_MACH_START 0x800
#define CALL_MACH_STOP 0x801
#define CALL_MACH_PLACE 0x802
#define CALL_MACH_RUN 0x803
#define CALL_MACH_FOCUS 0x804
#define CALL_MACH_INFO 0x805
#define CALL_MACH_BUILTIN 0x806
#define CALL_MACH_START_PROGRAM 0x807
#define CALL_MACH_IDENTITY 0x808
#define CALL_MACH_REQUEST 0x809
#define CALL_MACH_ARGUMENT 0x80A
#define CALL_MACH_SNAPSHOT 0x80B
#define CALL_MACH_RESTORE 0x80C
#define CALL_MACH_SNAPSHOT_INFO 0x80D
#define CALL_MACH_SNAPSHOT_DELETE 0x80E
#define CALL_MACH_FACE 0x80F
#define CALL_MACH_UNFACE 0x810
#define CALL_V3_SUBMIT 0x900
#define CALL_V3_CLEAR 0x901
#define CALL_V3_STYLE 0x902
#define CALL_V3_PERSISTENCE 0x903

#define TERSE_ERR_UNKNOWN 0xFFFFFFFFu
#define TERSE_ERR_BUFFER 0xFFFFFFFEu

static inline uint32_t terse_call(uint32_t n, uint32_t x, uint32_t y, uint32_t z)
{
    register uint32_t a0 __asm__("a0") = x;
    register uint32_t a1 __asm__("a1") = y;
    register uint32_t a2 __asm__("a2") = z;
    register uint32_t a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0), "+r"(a1) : "r"(a2), "r"(a7) : "memory");
    return a0;
}

/* Four arguments (a0-a3). */
static inline uint32_t terse_call4(uint32_t n, uint32_t x, uint32_t y, uint32_t z, uint32_t w)
{
    register uint32_t a0 __asm__("a0") = x;
    register uint32_t a1 __asm__("a1") = y;
    register uint32_t a2 __asm__("a2") = z;
    register uint32_t a3 __asm__("a3") = w;
    register uint32_t a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0), "+r"(a1) : "r"(a2), "r"(a3), "r"(a7) : "memory");
    return a0;
}

/* Five arguments (a0-a4). */
static inline uint32_t terse_call5(uint32_t n, uint32_t x, uint32_t y, uint32_t z, uint32_t w, uint32_t v)
{
    register uint32_t a0 __asm__("a0") = x;
    register uint32_t a1 __asm__("a1") = y;
    register uint32_t a2 __asm__("a2") = z;
    register uint32_t a3 __asm__("a3") = w;
    register uint32_t a4 __asm__("a4") = v;
    register uint32_t a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0), "+r"(a1) : "r"(a2), "r"(a3), "r"(a4), "r"(a7) : "memory");
    return a0;
}

/* Six arguments (a0-a5). */
static inline uint32_t terse_call6(uint32_t n, uint32_t x, uint32_t y, uint32_t z, uint32_t w, uint32_t v, uint32_t u)
{
    register uint32_t a0 __asm__("a0") = x;
    register uint32_t a1 __asm__("a1") = y;
    register uint32_t a2 __asm__("a2") = z;
    register uint32_t a3 __asm__("a3") = w;
    register uint32_t a4 __asm__("a4") = v;
    register uint32_t a5 __asm__("a5") = u;
    register uint32_t a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0), "+r"(a1) : "r"(a2), "r"(a3), "r"(a4), "r"(a5), "r"(a7) : "memory");
    return a0;
}

/* Two results: a0 in *lo's return, a1 in *second. */
static inline uint32_t terse_call2(uint32_t n, uint32_t x, uint32_t y, uint32_t *second)
{
    register uint32_t a0 __asm__("a0") = x;
    register uint32_t a1 __asm__("a1") = y;
    register uint32_t a7 __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(a0), "+r"(a1) : "r"(a7) : "memory");
    *second = a1;
    return a0;
}

/* Event types (SPEC 5). */
#define EV_NONE 0x00
#define EV_KEY_DOWN 0x01
#define EV_KEY_UP 0x02
#define EV_TEXT 0x03
#define EV_POINTER 0x04
#define EV_CONTENT_DONE 0x05
#define EV_WHEEL 0x06
#define EV_FEEDS_UPDATED 0x07
#define EV_PAGE_LAID_OUT 0x08
#define EV_PAGE_ANCHOR 0x09
/* For the shell (device 8, SPEC 12): machine a is now in state b; the
 * host's home key; a pointer press (buttons b) over machine a's picture. */
#define EV_MACHINE_CHANGED 0x0A
#define EV_HOME 0x0B
#define EV_MACHINE_PRESSED 0x0C
/* The answer to page_locate, always this pair, one after the other: the
 * anchor and its line's height, then the line's y (page_place_event). */
#define EV_PAGE_LOCATED 0x0D
#define EV_PAGE_LOCATED_Y 0x0E
/* For the shell: machine a asked to open something (mach_request). */
#define EV_OPEN_REQUESTED 0x0F
/* For the shell: the snapshot for slot a was stored (b = 1) or not (3). */
#define EV_SNAPSHOT_DONE 0x10

/* USB HID key codes used by most programs. */
#define KEY_ENTER 0x28
#define KEY_ESCAPE 0x29
#define KEY_BACKSPACE 0x2A
#define KEY_TAB 0x2B
#define KEY_SPACE 0x2C
#define KEY_HOME 0x4A
#define KEY_PAGE_UP 0x4B
#define KEY_END 0x4D
#define KEY_PAGE_DOWN 0x4E
#define KEY_RIGHT 0x4F
#define KEY_LEFT 0x50
#define KEY_DOWN 0x51
#define KEY_UP 0x52

#define MOD_SHIFT 0x01
#define MOD_CONTROL 0x02
#define MOD_ALT 0x04
#define MOD_META 0x08

#define BUTTON_PRIMARY 0x01
#define BUTTON_SECONDARY 0x02
#define BUTTON_MIDDLE 0x04

typedef struct {
    uint8_t type, a, b, c;
} terse_event;

#define EV_POINTER_X(e) ((e).a | (uint16_t)((e).c & 0x03) << 8)
#define EV_POINTER_Y(e) ((e).b | (uint16_t)((e).c & 0x04) << 6)
#define EV_POINTER_BUTTONS(e) ((e).c >> 3 & 0x07)
/* Text: the code point (a Unicode scalar value). Wheel: the vertical and
 * horizontal steps, signed (positive: down, right). */
#define EV_TEXT_CP(e) ((e).a | (uint32_t)(e).b << 8 | (uint32_t)((e).c & 0x1F) << 16)
#define EV_WHEEL_Y(e) ((int8_t)(e).a)
#define EV_WHEEL_X(e) ((int8_t)(e).b)
/* Page laid out: the height (0-FFFFFFh); page anchor: the id and the
 * buttons, in c bits 3-5 as in a pointer event. */
#define EV_PAGE_HEIGHT(e) ((e).a | (uint32_t)(e).b << 8 | (uint32_t)(e).c << 16)
#define EV_ANCHOR_ID(e) ((e).a | (uint16_t)(e).b << 8)
#define EV_ANCHOR_BUTTONS(e) ((e).c >> 3 & 0x07)

/* Copies the head event into *e and removes it; returns its type, EV_NONE
 * (and leaves *e alone) if the queue is empty. */
uint8_t terse_event_pop(terse_event *e);

/* Gives up the rest of this frame's budget (wait_frame, SPEC 4) and
 * returns when the machine runs again. Returns wait_frame's result less
 * the previous call's (the first call: the result itself), at most 255:
 * usually 1, more if the work in between ran out of budget. The result is
 * fixed at the call (one more than the frame counter then), so if the next
 * frame runs nothing (the budget is still below zero there) the machine
 * returns in a later frame than this counts; terse_frame() gives the
 * frame it is in. */
uint8_t terse_wait_frame(void);

static inline uint32_t terse_frame(void) { return terse_call(CALL_FRAME, 0, 0, 0); }
static inline uint32_t terse_time(void) { return terse_call(CALL_TIME, 0, 0, 0); }
static inline void terse_exit(uint32_t code) { terse_call(CALL_EXIT, code, 0, 0); }
/* Fills buf (in RAM) with the host's random bytes, 256 per call (SPEC 4). */
void terse_random(void *buf, uint32_t len);
static inline uint8_t terse_random_byte(void)
{
    uint8_t b;
    terse_random(&b, 1);
    return b;
}

/* Asks the shell to open something (SPEC 4): `request` (ROM or RAM) is
 * OPEN_BLOB and a blob ID (32), or OPEN_PROGRAM, a feed key (32; zero: a
 * built-in), the name's length (1) and the name (1-48). 1 if the request
 * was passed to a shell; 0 if there is none (the machine runs on its own)
 * or a request of this machine still waits; TERSE_ERR_BUFFER if malformed.
 * What the shell does with it, the machine is not told. */
#define OPEN_BLOB 1
#define OPEN_PROGRAM 2
static inline uint32_t terse_open(const void *request) { return terse_call(CALL_OPEN, (uint32_t)request, 0, 0); }
/* The blob ID the shell gave this machine when it started it, into 32
 * bytes of RAM: 1, or 0 if it has none. */
static inline uint32_t terse_argument(uint8_t *id) { return terse_call(CALL_ARGUMENT, (uint32_t)id, 0, 0); }

/* Controllers (SPEC 6): the 8-byte state. */
typedef struct {
    uint8_t connected;
    uint8_t buttons;  /* up, down, left, right, A, B, X, Y from bit 0 */
    uint8_t buttons2; /* L1, R1, L2, R2, select, start, L3, R3 */
    int8_t lx, ly, rx, ry;
    uint8_t zero;
} terse_pad;
static inline void terse_pad_read(uint8_t n, terse_pad *pad) { terse_call(CALL_CONTROLLER, n, (uint32_t)pad, 0); }
#define PAD_UP 0x01
#define PAD_DOWN 0x02
#define PAD_LEFT 0x04
#define PAD_RIGHT 0x08
#define PAD_A 0x10
#define PAD_B 0x20
#define PAD_X 0x40
#define PAD_Y 0x80
#define PAD_L1 0x01
#define PAD_R1 0x02
#define PAD_L2 0x04
#define PAD_R2 0x08
#define PAD_SELECT 0x10
#define PAD_START 0x20
#define PAD_L3 0x40
#define PAD_R3 0x80

/* Device 2, terminal (SPEC 8). */
#define TERM_COLS 80
#define TERM_ROWS 30

#define TERM_CLEAR 0x01
#define TERM_SCROLL_UP 0x02
#define TERM_SCROLL_DOWN 0x03
#define TERM_CLEAR_EOL 0x04
#define TERM_CURSOR_ON 0x05
#define TERM_CURSOR_OFF 0x06
#define TERM_CONTINUATION 0xFFFF

/* The longest write call (SPEC 8.2). */
#define TERM_WRITE_MAX 4096

/* Put character for each of `len` bytes (SPEC 8.3). A write call takes
 * its whole buffer, at most TERM_WRITE_MAX bytes, whatever the budget (2
 * units a byte); this splits a longer text into such calls. */
void term_write(const void *p, uint32_t len);
static inline void term_goto(uint8_t col, uint8_t row) { terse_call(CALL_TERM_SET_CURSOR, col, row, 0); }
static inline void term_color(uint8_t fg, uint8_t bg) { terse_call(CALL_TERM_SET_COLOURS, fg, bg, 0); }
static inline void term_cmd(uint8_t c) { terse_call(CALL_TERM_COMMAND, c, 0, 0); }
/* The cursor's column and row (get_cursor). */
static inline void term_get_cursor(uint8_t *col, uint8_t *row)
{
    uint32_t r;
    *col = terse_call2(CALL_TERM_GET_CURSOR, 0, 0, &r);
    *row = r;
}
/* The current foreground and background indexes (get_colours). */
static inline void term_get_colours(uint8_t *fg, uint8_t *bg)
{
    uint32_t b;
    *fg = terse_call2(CALL_TERM_GET_COLOURS, 0, 0, &b);
    *bg = b;
}
/* The grid's size (size): always 80 columns and 30 rows. */
static inline void term_size(uint8_t *cols, uint8_t *rows)
{
    uint32_t r;
    *cols = terse_call2(CALL_TERM_SIZE, 0, 0, &r);
    *rows = r;
}
/* Sets `count` palette entries (at most 256) from entry `first` (0-255)
 * on, wrapping from 255 to 0, from 3 x count bytes of red, green, blue at
 * `rgb` (ROM or RAM): the count, or TERSE_ERR_BUFFER if `first` or `count`
 * is out of range or the bytes are not wholly in ROM or RAM (SPEC 8.2). */
static inline uint32_t term_palette_set(uint32_t first, const void *rgb, uint32_t count)
{
    return terse_call(CALL_TERM_PALETTE_SET, first, (uint32_t)rgb, count);
}
void term_putc(uint8_t byte);
void term_puts(const char *s);
/* Sets the foreground only. */
void term_fg(uint8_t fg);
/* Writes the printable ASCII bytes (20h-7Eh) at `p`, at most `max`,
 * stopping at any other byte; returns how many it wrote. */
uint32_t term_put_ascii(uint32_t max, const uint8_t *p);
/* Writes one code point as UTF-8 (1 to 4 bytes); a surrogate or a value
 * above 10FFFFh is written as FFFDh. The terminal shows FFFDh for a code
 * point above FFFFh too (SPEC 8.3), in one cell. */
void term_put_cp(uint32_t cp);
/* Writes `n` cells of `ch` (narrow) with colours fg/bg from (col, row); the
 * cursor does not move. */
void term_fill(uint8_t col, uint8_t row, uint8_t n, uint16_t ch, uint8_t fg, uint8_t bg);
/* Cells in the 4-byte format of SPEC 8.1, from cell index `index`. */
static inline void term_cells_write(uint32_t index, const void *cells, uint32_t count)
{
    terse_call(CALL_TERM_CELLS_WRITE, index, (uint32_t)cells, count);
}
static inline void term_cells_read(uint32_t index, void *cells, uint32_t count)
{
    terse_call(CALL_TERM_CELLS_READ, index, (uint32_t)cells, count);
}
/* Whether a code point takes two cells (SPEC 8.1, Unicode 16.0 W/F). */
uint8_t term_is_wide(uint32_t cp);

/* Device 3, 2D vector display (SPEC 10). */
#define VEC_DONE 0x00
#define VEC_REJECTED 0x01
#define VEC_MAX_LIST 16384
#define VEC_UNIT 16
#define VEC_SCREEN_W 10240
#define VEC_SCREEN_H 7680

#define VEC_OP_END 0x00
#define VEC_OP_COLOUR 0x01
#define VEC_OP_WIDTH 0x02
#define VEC_OP_ORIGIN 0x03
#define VEC_OP_MOVE 0x04
#define VEC_OP_LINE 0x05
#define VEC_OP_CIRCLE 0x06
#define VEC_OP_DISC 0x07
#define VEC_OP_RECT 0x08
#define VEC_OP_TRIANGLE 0x09

/* Builder, like the document builder: appends commands at vec_ptr into a
 * buffer of `cap` bytes the caller owns, never past vec_limit; a command
 * that does not fit sets vec_overflow and is not written, nor is any later
 * one, so the list holds a clean prefix (end it with vec_end before
 * submitting: lower vec_limit by 1 to keep room for it, then raise it). The
 * caller may move vec_ptr back within the buffer to rewrite the list. */
extern uint8_t *vec_ptr, *vec_limit;
extern uint8_t vec_overflow;
void vec_begin(uint8_t *buf, uint32_t cap);
#define vec_length(buf) ((uint32_t)(vec_ptr - (uint8_t *)(buf)))
void vec_colour(uint8_t r, uint8_t g, uint8_t b, uint8_t a);
void vec_width(uint8_t w);
void vec_origin(int16_t x, int16_t y);
void vec_move(int16_t x, int16_t y);
void vec_line(int16_t x, int16_t y);
void vec_circle(uint16_t r);
void vec_disc(uint16_t r);
void vec_rect(int16_t x, int16_t y);
void vec_tri(int16_t x1, int16_t y1, int16_t x2, int16_t y2);
void vec_end(void);

/* Submits `len` bytes at `list` (which must be in RAM); returns VEC_DONE or
 * VEC_REJECTED. Costs 1 unit per 4 bytes. */
static inline uint8_t vec_submit(const void *list, uint32_t len) { return terse_call(CALL_VEC_SUBMIT, (uint32_t)list, len, 0); }
#define vec_submit_list(buf) vec_submit((buf), vec_length(buf))
static inline void vec_clear(void) { terse_call(CALL_VEC_CLEAR, 0, 0, 0); }
/* Display style (SPEC 10.6), persistence 0-255, and fill (SPEC 10.2):
 * each returns VEC_DONE, or VEC_REJECTED for a value out of range. */
#define VEC_STYLE_CLEAN 0
#define VEC_STYLE_PHOSPHOR 1
#define VEC_FILL_ON 0
#define VEC_FILL_OFF 1
static inline uint8_t vec_style(uint32_t s) { return terse_call(CALL_VEC_STYLE, s, 0, 0); }
static inline uint8_t vec_persistence(uint32_t p) { return terse_call(CALL_VEC_PERSISTENCE, p, 0, 0); }
static inline uint8_t vec_fill(uint32_t f) { return terse_call(CALL_VEC_FILL, f, 0, 0); }

/* Device 9, 3D vector display (SPEC 13). Coordinates are sixteenths of a
 * screen unit: x right, y down, z away from the viewer, 0 at the glass.
 * Transforms apply to later coordinates, the last given first. A rotation
 * angle is in 1/65,536 of a turn: a positive angle about x turns y towards
 * z, about y z towards x, about z x towards y (clockwise on the screen). */
#define V3_MAX_LIST 16384
#define V3_UNIT 16
#define V3_TURN 65536u
#define V3_AXIS_X 0
#define V3_AXIS_Y 1
#define V3_AXIS_Z 2
#define V3_SCALE_ONE 256

#define V3_OP_END 0x00
#define V3_OP_COLOUR 0x01
#define V3_OP_WIDTH 0x02
#define V3_OP_MOVE 0x03
#define V3_OP_LINE 0x04
#define V3_OP_POINT 0x05
#define V3_OP_CIRCLE 0x06
#define V3_OP_PUSH 0x07
#define V3_OP_POP 0x08
#define V3_OP_TRANSLATE 0x09
#define V3_OP_ROTATE 0x0A
#define V3_OP_SCALE 0x0B
#define V3_OP_FACE 0x0C
#define V3_OP_SCREEN 0x0D
#define V3_OP_SURFACE 0x0E
#define V3_MAX_SURFACES 4

/* Builder, as the device 3 builder: appends at v3_ptr, never past
 * v3_limit (the end of the `cap` bytes given to v3_begin); a command that
 * does not fit sets v3_overflow and is not written, nor is any later one. */
extern uint8_t *v3_ptr, *v3_limit;
extern uint8_t v3_overflow;
void v3_begin(uint8_t *buf, uint32_t cap);
#define v3_length(buf) ((uint32_t)(v3_ptr - (uint8_t *)(buf)))
void v3_colour(uint8_t r, uint8_t g, uint8_t b, uint8_t a);
void v3_width(uint16_t w);
void v3_move(int16_t x, int16_t y, int16_t z);
void v3_line(int16_t x, int16_t y, int16_t z);
void v3_point(void);
void v3_circle(uint16_t r);
void v3_push(void);
void v3_pop(void);
void v3_translate(int16_t x, int16_t y, int16_t z);
void v3_rotate(uint8_t axis, uint16_t angle);
void v3_scale(uint16_t s);
void v3_end(void);
/* A face: a flat polygon that gives no light but hides what is behind it
 * (lines, points, circles), as the eye sees it. Each corner is an x, y, z
 * triple (transformed like Move's point; the current point stays). A
 * 4-corner face is the triangles a-b-c and a-c-d. */
void v3_face3(const int16_t *a, const int16_t *b, const int16_t *c);
void v3_face4(const int16_t *a, const int16_t *b, const int16_t *c, const int16_t *d);
/* Only in the shell's list: machine `handle`'s screen as a flat 4:3 panel
 * `width` sixteenths wide, centred on the transform's origin in its xy
 * plane, facing -z (as the glass faces the viewer). Opaque: it hides lines
 * behind it. The current point stays. */
void v3_screen(uint8_t handle, uint16_t width);
/* The rectangle x, y, w, h (screen units) of this machine's own 2D layers
 * (terminal, page, 2D shapes) as a flat panel `width` sixteenths wide and
 * as high as the rectangle's shape, centred on the transform's origin in
 * its xy plane, facing -z. Opaque: it hides lines behind it; the
 * rectangle is then not drawn on the glass, and a pointer over the panel
 * arrives as a pointer inside the rectangle. At most 4 take effect in a
 * list. The current point stays. */
void v3_surface(uint16_t x, uint16_t y, uint16_t w, uint16_t h, uint16_t width);

/* Submits `len` bytes at `list` (in RAM); returns VEC_DONE or VEC_REJECTED.
 * Costs 1 unit per 4 bytes. */
static inline uint8_t v3_submit(const void *list, uint32_t len) { return terse_call(CALL_V3_SUBMIT, (uint32_t)list, len, 0); }
#define v3_submit_list(buf) v3_submit((buf), v3_length(buf))
static inline void v3_clear(void) { terse_call(CALL_V3_CLEAR, 0, 0, 0); }
/* The display style and persistence, shared with device 3. */
static inline uint8_t v3_style(uint32_t s) { return terse_call(CALL_V3_STYLE, s, 0, 0); }
static inline uint8_t v3_persistence(uint32_t p) { return terse_call(CALL_V3_PERSISTENCE, p, 0, 0); }

/* Device 4, content (SPEC 9). */
#define CONTENT_GET 0x01
#define CONTENT_PUT 0x02
#define CONTENT_SUBSCRIPTIONS 0x03
#define CONTENT_TIMELINE 0x04
#define CONTENT_SUBSCRIBE 0x05
#define CONTENT_UNSUBSCRIBE 0x06
#define CONTENT_PUBLISH 0x07
#define CONTENT_IDENTITY 0x08

#define STATUS_PENDING 0x00
#define STATUS_DONE 0x01
#define STATUS_NOT_FOUND 0x02
#define STATUS_FAILED 0x03
#define STATUS_BAD_REQUEST 0x04
#define STATUS_REFUSED 0x05
#define STATUS_NO_ROOM 0x06

/* The 52-byte request block; it must lie in RAM. */
typedef struct {
    uint8_t op;
    volatile uint8_t status;
    uint8_t reserved[2];
    uint8_t id[32];
    uint32_t offset;
    volatile uint32_t length;
    uint32_t addr; /* a guest address */
    volatile uint32_t size;
} content_req;

#define TIMELINE_LOCAL 0x01

uint32_t get24(const uint8_t *p);

/* Submits the request (SPEC 9.2): its handle (1-8), 0 if it was rejected
 * (r->status is STATUS_BAD_REQUEST if it was invalid; unchanged if eight
 * requests are pending or the block is not in RAM), or TERSE_ERR_UNKNOWN
 * if the image does not list the content device. Every content_* and
 * record_* wrapper below returns this. A handle fits in a uint8_t, but
 * TERSE_ERR_UNKNOWN does not: keep the result in a uint32_t unless the
 * image lists device 4. */
static inline uint32_t content_submit(content_req *r) { return terse_call(CALL_CONTENT_SUBMIT, (uint32_t)r, 0, 0); }
uint32_t content_get(content_req *r, const uint8_t *id, uint32_t offset, uint32_t length, void *buf);
uint32_t content_feed(content_req *r, uint8_t op, uint32_t first, uint32_t length, void *buf);
/* Puts `size` bytes (ROM or RAM, copied at submit); on completion r->id is
 * the blob's ID. */
uint32_t content_put(content_req *r, const void *data, uint32_t size);
/* Subscribes to (CONTENT_SUBSCRIBE) or unsubscribes from
 * (CONTENT_UNSUBSCRIBE) the feed `key` (32 bytes). The host asks the user;
 * STATUS_REFUSED if the answer is no (SPEC 9.5). */
uint32_t content_follow(content_req *r, uint8_t op, const uint8_t *key);
/* Subscribes to the feed `key` with the hint list of its address (`len`
 * bytes at `hints`, ROM or RAM, copied at submit; NETWORK 4.9), e.g.
 * "relay.example:7000,b.example": where the host looks for the feed if its
 * own relays do not hold it. STATUS_BAD_REQUEST if the hints are
 * malformed or over CONTENT_MAX_HINTS bytes; len 0: no hints. */
#define CONTENT_MAX_HINTS 161
uint32_t content_subscribe(content_req *r, const uint8_t *key, const void *hints, uint32_t len);
/* Publishes blob `blob` (already in the host's store: put it first) as the
 * next entry of the host's feed, with a title of `len` bytes of UTF-8 (at
 * most 120, copied at submit). The host asks the user; on completion
 * r->size is the entry's seq. */
uint32_t content_publish(content_req *r, const uint8_t *blob, const void *title, uint32_t len);
/* The same with the entry's kind: 0 or ENTRY_ARTICLE, or ENTRY_DOCUMENT for
 * a blob that starts with a document header (TDOC_*); STATUS_BAD_REQUEST
 * on completion if it does not. */
#define ENTRY_ARTICLE 1
#define ENTRY_PROFILE 2
#define ENTRY_PROGRAM 3
#define ENTRY_DOCUMENT 4
uint32_t content_publish_kind(content_req *r, const uint8_t *blob, const void *title, uint32_t len, uint8_t kind);
/* A document (SPEC 9.7) starts with "TDOC", version 0, the format owner's
 * feed key (32; zero: a built-in program's format), the format name's
 * length (1) and the name (1-48), then the data. */
#define TDOC_HEAD 38
/* The host's own feed key, in r->id on completion; STATUS_NOT_FOUND if the
 * host has no identity yet. */
uint32_t content_identity(content_req *r);
/* The same, and the hint list of the host's own feed address in the `len`
 * bytes at `buf` (RAM): on completion r->length is the bytes written (the
 * whole hints that fit; 0: the host has none, so its address is the bare
 * key). */
uint32_t content_identity_hints(content_req *r, void *buf, uint32_t len);

/* Content: names, search, profile and nicknames (SPEC 9.5). */
#define CONTENT_NAMES 0x09
#define CONTENT_FIND 0x0A
#define CONTENT_SET_PROFILE 0x0B
#define CONTENT_SET_NICKNAME 0x0C

/* Names record flags: the name is the user's nickname; the feed has a
 * profile in force; the feed is not subscribed (listed for its nickname). */
#define NAME_NICKNAME 0x01
#define NAME_PROFILE 0x02
#define NAME_UNSUBSCRIBED 0x04
/* The host follows the feed through a relay a hint of its address names. */
#define NAME_HINTED 0x08

/* Find scopes. */
#define FIND_FEEDS 1
#define FIND_ENTRIES 2
#define FIND_PROGRAMS 3

/* Names records from index `first`, as many as fit in `length` bytes at
 * `buf`: L, flags, feed key (32), name (L - 33 bytes, 0 to 48). Names are
 * self-declared; show the short key beside one. */
uint32_t content_names(content_req *r, uint32_t first, uint32_t length, void *buf);
/* Searches the host's relays for `qlen` bytes of UTF-8 (1 to 48) at
 * `query`, FIND_FEEDS (profile entries), FIND_ENTRIES (articles and
 * documents) or FIND_PROGRAMS (programs, by name). The query is copied to
 * the start of `buf` first (unless it is there already), so `buf` (in RAM)
 * must have room for the query and for `buflen` bytes (1 to 16,384) of
 * results; `buflen` may be shorter than the query. The results, timeline
 * records, replace the query: r->length is the bytes delivered, r->size
 * the number of results (at most 16). */
uint32_t content_find(content_req *r, uint8_t scope, const void *query, uint32_t qlen, void *buf, uint32_t buflen);
/* Publishes a profile for the host's feed with display name `name` (`len`
 * bytes, 1 to 48, no control characters, no leading or trailing space);
 * `listed` lets search find it. The host asks the user; on completion
 * r->size is the entry's seq. */
uint32_t content_set_profile(content_req *r, const void *name, uint32_t len, uint8_t listed);
/* Sets the local nickname of feed `key` (32 bytes) to `len` bytes at
 * `name`, or removes it if `len` is 0. The host asks the user. */
uint32_t content_set_nickname(content_req *r, const uint8_t *key, const void *name, uint32_t len);

/* Content: programs and named records (SPEC 9.5, 9.6). */
#define CONTENT_PROGRAMS 0x0D
#define CONTENT_SAVE 0x0E
#define CONTENT_LOAD 0x0F
#define CONTENT_DELETE 0x10
#define CONTENT_RECORDS 0x11

/* The current version of each program of the subscribed feeds, from
 * index `first`, as many as fit in `length` bytes: timeline records (L,
 * flags with TIMELINE_LOCAL if the image is in the host's store, the entry
 * without its signature: the name is the title). By feed key, then name. */
uint32_t content_programs(content_req *r, uint32_t first, uint32_t length, void *buf);
/* Named records: data this program keeps on this machine, under its
 * identity (SPEC 9.6). `name` is 1 to 32 bytes up to its 0 (no control
 * characters); a longer one is not cut: the call returns 0 and r->status
 * is STATUS_BAD_REQUEST, as for any invalid name. A save copies `size` bytes (0 to 65,536, ROM or RAM) at
 * submit and replaces the record; STATUS_NO_ROOM if the program would keep
 * more than 256 records or 1,048,576 bytes. A load delivers up to `length`
 * bytes (1 to 16,384) from `offset` to `buf`: r->length is the bytes
 * delivered, r->size the record's size; STATUS_NOT_FOUND if there is no
 * such record. A delete is done whether or not the record existed. */
uint32_t record_save(content_req *r, const char *name, const void *data, uint32_t size);
uint32_t record_load(content_req *r, const char *name, uint32_t offset, uint32_t length, void *buf);
uint32_t record_delete(content_req *r, const char *name);
/* The records, from index `first`, as many as fit in `length` bytes: 36
 * bytes each, the name (zero padded to 32) and the size (4), by name. */
uint32_t record_list(content_req *r, uint32_t first, uint32_t length, void *buf);

/* Device 6, sound (SPEC 18): 16 voices of a wavetable synthesizer,
 * 48,000 Hz stereo, 800 sample pairs a frame. A program sets the voices
 * with a command list; the commands take effect from the next samples the
 * device renders (at the end of the frame). */
#define SND_VOICES 16
#define SND_WAVES 32
#define SND_WAVE_LEN 64
#define SND_SAMPLE_SLOTS 4
#define SND_MAX_SAMPLE 65536
#define SND_MAX_LIST 4096
#define SND_DONE 0x00
#define SND_REJECTED 0x01
#define SND_NONE 0xFF
#define SND_CAP_SAMPLES 0x01

#define SND_OP_END 0x00
#define SND_OP_VOICE 0x01
#define SND_OP_WAVE 0x02
#define SND_OP_NOISE 0x03
#define SND_OP_SAMPLE 0x04
#define SND_OP_FREQUENCY 0x05
#define SND_OP_VOLUME 0x06
#define SND_OP_PAN 0x07
#define SND_OP_ENVELOPE 0x08
#define SND_OP_KEY_ON 0x09
#define SND_OP_KEY_OFF 0x0A
#define SND_OP_MODULATOR 0x0B
#define SND_OP_FEEDBACK 0x0C
#define SND_OP_RING 0x0D
#define SND_OP_MIX 0x0E

/* The phase increment of a wavetable or noise voice for `hz` (SPEC 18.4):
 * round(hz * 2^32 / 48,000). A constant expression for a constant `hz`
 * (folded by the compiler: no floating point at run time). */
#define SND_HZ(hz) ((uint32_t)((double)(hz) * 89478.48533333333 + 0.5))
/* The increment of a sample voice playing a sample recorded at `rate`
 * samples per second (16.16 positions a sample). */
#define SND_RATE(rate) ((uint32_t)((double)(rate) * 65536.0 / 48000.0 + 0.5))

/* Builder, like vec_: appends commands at snd_ptr into a buffer of `cap`
 * bytes; a command that does not fit sets snd_overflow and is not written,
 * nor is any later one. */
extern uint8_t *snd_ptr, *snd_limit;
extern uint8_t snd_overflow;
void snd_begin(uint8_t *buf, uint32_t cap);
#define snd_length(buf) ((uint32_t)(snd_ptr - (uint8_t *)(buf)))
void snd_voice(uint8_t v);
void snd_wave(uint8_t slot);
void snd_noise(void);
void snd_sample(uint8_t slot);
void snd_frequency(uint32_t increment);
void snd_volume(uint8_t volume);
void snd_pan(uint8_t left, uint8_t right);
/* Rates are envelope steps a sample (the level runs 0 to 0xFFFFFF); 0 is
 * at once. Sustain is a level 0-255. */
void snd_envelope(uint16_t attack, uint16_t decay, uint8_t sustain, uint16_t release);
void snd_key_on(void);
void snd_key_off(void);
/* Voice `m` (or SND_NONE) offsets this voice's wavetable phase by its
 * output times `depth`. */
void snd_modulator(uint8_t m, uint8_t depth);
void snd_feedback(uint8_t depth);
/* This voice's output is multiplied by voice `q`'s (or SND_NONE). */
void snd_ring(uint8_t q);
/* 1: the voice is heard; 0: muted (it may still modulate). */
void snd_mix(uint8_t on);

/* Carries out `len` bytes of commands at `list` (ROM or RAM, 1 to 4,096
 * bytes); SND_DONE or SND_REJECTED. Costs 2 units a byte. */
static inline uint8_t snd_submit(const void *list, uint32_t len) { return terse_call(CALL_SND_SUBMIT, (uint32_t)list, len, 0); }
#define snd_submit_list(buf) snd_submit((buf), snd_length(buf))
/* Loads wavetable `slot` (0-31) from 64 signed bytes; 0, or
 * TERSE_ERR_BUFFER. */
static inline uint32_t snd_load_wave(uint32_t slot, const int8_t *wave) { return terse_call(CALL_SND_WAVE, slot, (uint32_t)wave, 0); }
/* Loads sample slot `slot` (0-3) with `len` signed bytes (at most 65,536);
 * it loops from byte `loop_start`, or plays once if that is `len` or more. */
static inline uint32_t snd_load_sample(uint32_t slot, const int8_t *data, uint32_t len, uint32_t loop_start)
{
    return terse_call4(CALL_SND_SAMPLE, slot, (uint32_t)data, len, loop_start);
}
/* The number of voices; *caps gets the capabilities (SND_CAP_SAMPLES). */
static inline uint32_t snd_info(uint32_t *caps) { return terse_call2(CALL_SND_INFO, 0, 0, caps); }

/* Device 7, page (SPEC 11): typeset text in a rectangle. */
#define PAGE_DONE 0x00
#define PAGE_REJECTED 0x01
#define PAGE_MAX_DOC 65536u

/* Submits the document of `len` bytes at `doc` (ROM or RAM): PAGE_DONE, and
 * the scroll value is 0 again; PAGE_REJECTED for a length of 0 or more
 * than 65,536; TERSE_ERR_BUFFER if the bytes are not wholly in ROM or RAM.
 * Costs 1 unit per 4 bytes. The height arrives later, as EV_PAGE_LAID_OUT. */
static inline uint32_t page_submit(const void *doc, uint32_t len) { return terse_call(CALL_PAGE_SUBMIT, (uint32_t)doc, len, 0); }
#define page_submit_doc(buf) page_submit((buf), doc_length(buf))
/* No document: only the background shows (the scroll value is kept). */
static inline void page_clear(void) { terse_call(CALL_PAGE_CLEAR, 0, 0, 0); }
/* The rectangle in screen units (clamped to the screen); w or h 0 hides it. */
static inline void page_region(uint32_t x, uint32_t y, uint32_t w, uint32_t h) { terse_call4(CALL_PAGE_REGION, x, y, w, h); }
/* Units from the top of the document to the top of the rectangle; the host
 * clamps it to 0 .. max(0, height - rectangle height) when it draws. */
static inline void page_scroll(int32_t y) { terse_call(CALL_PAGE_SCROLL, (uint32_t)y, 0, 0); }
/* Asks where anchor `id` (1-65535) is laid out in the current document:
 * PAGE_DONE, or PAGE_REJECTED (no document, id 0, or 8 locates already this
 * frame). The answer arrives in a later frame as EV_PAGE_LOCATED and
 * EV_PAGE_LOCATED_Y (page_place_event), unless the document is replaced
 * first. */
static inline uint32_t page_locate(uint32_t id) { return terse_call(CALL_PAGE_LOCATE, id, 0, 0); }

/* Where an anchor is: y and height of the first line (or figure) carrying
 * it, in screen units from the top of the document; height 0 (and y
 * PAGE_ABSENT) if the document has no such anchor. */
#define PAGE_ABSENT 0xFFFFFFu
typedef struct {
    uint16_t id;
    uint8_t height, half; /* half: the EV_PAGE_LOCATED half has arrived */
    uint32_t y;
} page_place;
/* Give it every EV_PAGE_LOCATED and EV_PAGE_LOCATED_Y event: returns 1
 * when *p holds a whole answer (at the second of the pair), else 0. */
uint8_t page_place_event(const terse_event *e, page_place *p);

/* The colour behind the text; a = 0 lets the cell backgrounds show. */
static inline void page_background(uint8_t r, uint8_t g, uint8_t b, uint8_t a)
{
    terse_call(CALL_PAGE_BACKGROUND, r | (uint32_t)g << 8 | (uint32_t)b << 16 | (uint32_t)a << 24, 0, 0);
}

/* Document opcodes (SPEC 11.3). */
#define DOC_OP_END 0x00
#define DOC_OP_TEXT 0x01
#define DOC_OP_FONT 0x02
#define DOC_OP_SIZE 0x03
#define DOC_OP_COLOUR 0x04
#define DOC_OP_PARAGRAPH 0x05
#define DOC_OP_RULE 0x06
#define DOC_OP_ANCHOR 0x07
#define DOC_OP_FIGURE 0x08

/* Faces and font flags (doc_font), alignment and flags (doc_paragraph). */
#define DOC_SANS 0
#define DOC_SERIF 1
#define DOC_MONO 2
#define DOC_BOLD 0x01
#define DOC_ITALIC 0x02
#define DOC_START 0
#define DOC_CENTRE 1
#define DOC_END 2
#define DOC_NOWRAP 0x01
/* With DOC_NOWRAP: a line that does not fit ends in an ellipsis. */
#define DOC_ELLIPSIS 0x02
/* Longest text of one Text command. */
#define DOC_TEXT_MAX 65535u

/* Document builder: appends commands at doc_ptr into a buffer the caller
 * owns, never past doc_limit. A command that does not fit is not written:
 * doc_overflow is set and every later command is dropped too, so the
 * document holds a clean prefix. The caller may lower doc_limit to keep
 * room in reserve (for a closing note, the End byte) and raise it later
 * (clearing doc_overflow), but never past the end of its buffer. */
extern uint8_t *doc_ptr, *doc_limit;
extern uint8_t doc_overflow;
void doc_begin(uint8_t *buf, uint32_t cap);
#define doc_length(buf) ((uint32_t)(doc_ptr - (uint8_t *)(buf)))
/* Appends `len` bytes of UTF-8 as Text commands of at most 65,535 bytes
 * each, split between characters (a sequence split across two commands
 * would be malformed in both). If the buffer fills, appends as much as
 * fits, cut between characters, and sets doc_overflow. Returns the bytes
 * of text appended. */
uint32_t doc_text(const void *p, uint32_t len);
void doc_font(uint8_t face, uint8_t flags);
void doc_size(uint8_t size);
void doc_colour(uint8_t r, uint8_t g, uint8_t b);
void doc_paragraph(uint8_t align, uint8_t indent, uint8_t space, uint8_t flags);
void doc_rule(void);
void doc_anchor(uint16_t id);
void doc_end(void);
/* A Figure command: a box of width x height screen units (1-1024), aligned
 * DOC_START, DOC_CENTRE or DOC_END, drawn by the `n` bytes of path commands
 * at `path` (built with path_*). Written whole or not at all. `path` may
 * lie at doc_ptr + DOC_FIGURE_HEAD, where the bytes already are (build them
 * there to save a copy), or anywhere else. */
#define DOC_FIGURE_HEAD 8
void doc_figure(uint16_t width, uint16_t height, uint8_t align, const void *path, uint16_t n);

/* Path commands of a figure (SPEC 11.3): coordinates in sixteenths of a
 * screen unit from the figure's top left. */
#define PATH_FILL 0x01
#define PATH_STROKE 0x02
#define PATH_MOVE 0x03
#define PATH_LINE 0x04
#define PATH_QUAD 0x05
#define PATH_CUBIC 0x06
#define PATH_CLOSE 0x07

/* Path builder, like the document builder: appends at path_ptr, never past
 * path_limit; a command that does not fit sets path_overflow and is not
 * written, nor is any later one. */
extern uint8_t *path_ptr, *path_limit;
extern uint8_t path_overflow;
void path_begin(uint8_t *buf, uint32_t cap);
#define path_length(buf) ((uint32_t)(path_ptr - (uint8_t *)(buf)))
/* a = 0: no fill (no stroke); stroke width in sixteenths, 0: no stroke. */
void path_fill(uint8_t r, uint8_t g, uint8_t b, uint8_t a);
void path_stroke(uint8_t r, uint8_t g, uint8_t b, uint8_t a, uint16_t width);
void path_move(int16_t x, int16_t y);
void path_line(int16_t x, int16_t y);
void path_quad(int16_t x1, int16_t y1, int16_t x, int16_t y);
void path_cubic(int16_t x1, int16_t y1, int16_t x2, int16_t y2, int16_t x, int16_t y);
void path_close(void);
/* Opcode `op` (PATH_MOVE ... PATH_CUBIC) and its `n` points, x and y
 * alternating in `xy`. */
void path_points(uint8_t op, uint32_t n, const int16_t *xy);

/* Device 8, machines (SPEC 12): only the shell has it. Handles 1-8. */
#define MACH_MAX 8
#define MACH_NONE 0
#define MACH_LOADING 1
#define MACH_RUNNING 2
#define MACH_PAUSED 3
#define MACH_EXITED 4
#define MACH_FAULTED 5
#define MACH_FAILED 6

/* Why a start failed (mach_record.failure, when the state is failed). */
#define MACH_FAIL_NOT_FOUND 1
#define MACH_FAIL_INVALID 2
#define MACH_FAIL_NEEDS_DEVICE 3
#define MACH_FAIL_NO_ROOM 4
#define MACH_FAIL_SNAPSHOT 5 /* restore: no snapshot, unreadable, or not of its image */

/* The 48-byte record of mach_info. */
typedef struct {
    uint8_t state, exit_code, fault_kind, failure;
    uint32_t units;    /* units used in the machine's last frame */
    uint32_t frames;   /* its frame counter */
    uint32_t fault_pc;
    uint8_t image[32]; /* the image's blob ID */
} mach_record;

/* Starts the image with blob ID `id` (32 bytes): its handle, 0 if none is
 * free. The machine is loading until the host has the image. */
static inline uint32_t mach_start(const uint8_t *id) { return terse_call(CALL_MACH_START, (uint32_t)id, 0, 0); }
static inline uint32_t mach_stop(uint32_t h) { return terse_call(CALL_MACH_STOP, h, 0, 0); }
/* Shows machine h's screen in the rectangle (scaled to fit, centred);
 * w or h 0 hides it. */
static inline uint32_t mach_place(uint32_t h, uint32_t x, uint32_t y, uint32_t w, uint32_t ht)
{
    return terse_call5(CALL_MACH_PLACE, h, x, y, w, ht);
}
static inline uint32_t mach_run(uint32_t h, uint32_t run) { return terse_call(CALL_MACH_RUN, h, run, 0); }
/* Keyboard and controllers go to machine h (0: the shell); 1 if done. */
static inline uint32_t mach_focus(uint32_t h) { return terse_call(CALL_MACH_FOCUS, h, 0, 0); }
static inline uint32_t mach_info(uint32_t h, mach_record *r) { return terse_call(CALL_MACH_INFO, h, (uint32_t)r, 0); }
/* The blob ID of the host's nth built-in program: 1, or 0 past the end. */
static inline uint32_t mach_builtin(uint32_t n, uint8_t *id) { return terse_call(CALL_MACH_BUILTIN, n, (uint32_t)id, 0); }
/* Starts the current version of a program by its identity: `record` is the
 * publisher's feed key (32 bytes; all zero: a built-in, by title), the
 * name's length (1) and the name (1 to 48 bytes). Its handle, 0 if none is
 * free; TERSE_ERR_BUFFER if the name is not a valid name. Loading until the
 * host has the image; failed (MACH_FAIL_NOT_FOUND) if it holds no such
 * program. */
static inline uint32_t mach_start_program(const uint8_t *record) { return terse_call(CALL_MACH_START_PROGRAM, (uint32_t)record, 0, 0); }
/* The same, with the key and the name given apart. */
uint32_t mach_start_named(const uint8_t *key, const void *name, uint32_t len);
/* Machine h's identity into 81 bytes at `out`: feed key (32), name length
 * (1), name (48, zero padded); 1, or 0 if it has none. */
static inline uint32_t mach_identity(uint32_t h, uint8_t *out) { return terse_call(CALL_MACH_IDENTITY, h, (uint32_t)out, 0); }

/* Machine h's open request (after EV_OPEN_REQUESTED) into 82 bytes of RAM:
 * the kind, then the blob ID, or the feed key, name length and name (zero
 * padded to 48). 1, and the request is no longer waiting; 0 if none waits. */
static inline uint32_t mach_request(uint32_t h, uint8_t *out) { return terse_call(CALL_MACH_REQUEST, h, (uint32_t)out, 0); }
/* Gives loading machine h the blob ID `id` as its argument (terse_argument);
 * 1, or 0 if h is not loading. */
static inline uint32_t mach_argument(uint32_t h, const uint8_t *id) { return terse_call(CALL_MACH_ARGUMENT, h, (uint32_t)id, 0); }

/* Snapshots (SPEC 12.7): slots 0-15 of the host's snapshots for this
 * shell. mach_snapshot keeps machine h (running or paused) in a slot at
 * the end of this frame; EV_SNAPSHOT_DONE tells how it went. 1 if
 * accepted. */
#define MACH_SNAPSHOT_SLOTS 16
static inline uint32_t mach_snapshot(uint32_t h, uint32_t slot) { return terse_call(CALL_MACH_SNAPSHOT, h, slot, 0); }
/* A new machine from the slot's snapshot, as it was (running, hidden): its
 * handle, 0 if none is free. Loading until the host has the image; failed
 * (MACH_FAIL_SNAPSHOT) if the slot holds no snapshot it can restore. */
static inline uint32_t mach_restore(uint32_t slot) { return terse_call(CALL_MACH_RESTORE, slot, 0, 0); }
/* The 122-byte record of mach_snapshot_info. */
typedef struct __attribute__((packed)) {
    uint8_t present;
    uint8_t image[32];   /* the image's blob ID */
    uint8_t key[32];     /* its identity: publisher (all zero: a built-in) */
    uint8_t name_len;    /* and name; 0: no identity */
    uint8_t name[48];
    uint32_t frames;     /* frames it had run */
    uint32_t time;       /* Unix time it was taken */
} mach_snapshot_record;
/* The slot's record: 1 if it holds a snapshot, else 0 (present 0). */
static inline uint32_t mach_snapshot_info(uint32_t slot, mach_snapshot_record *r)
{
    return terse_call(CALL_MACH_SNAPSHOT_INFO, slot, (uint32_t)r, 0);
}
static inline uint32_t mach_snapshot_delete(uint32_t slot) { return terse_call(CALL_MACH_SNAPSHOT_DELETE, slot, 0, 0); }

/* Facing a panel (SPEC 12.2): over `frames` (0-600) the host moves its
 * view of the whole 3D scene until machine h's panel (a Screen in the
 * list submitted now) lies flat in the rectangle, shown there as by
 * mach_place; 1, or 0 if h has no panel or the arguments are bad.
 * mach_unface moves back to the scene as listed. */
static inline uint32_t mach_face(uint32_t h, uint32_t frames, uint32_t x, uint32_t y, uint32_t w, uint32_t ht)
{
    return terse_call6(CALL_MACH_FACE, h, frames, x, y, w, ht);
}
static inline uint32_t mach_unface(uint32_t frames) { return terse_call(CALL_MACH_UNFACE, frames, 0, 0); }

/* --- Page tables (SPEC 11.3, 11.4) -------------------------------------
 * A table is a block of cells laid out by the host: doc_table starts it
 * (1-8 columns, DOC_TABLE_HEADER makes the first row a header, `align`
 * gives each column's DOC_START, DOC_CENTRE or DOC_END, or 0 for all
 * start), doc_cell starts each next cell in row order (the text, font,
 * size, colour and anchor commands after it go into that cell), and
 * doc_table_end ends it. A table holds at most 1,024 cells, a document
 * 4,096. Like every doc_* command these write whole or set doc_overflow. */
#define DOC_OP_TABLE 0x09
#define DOC_OP_CELL 0x0A
#define DOC_OP_END_TABLE 0x0B
#define DOC_TABLE_HEADER 0x01
#define DOC_TABLE_COLUMNS 8
void doc_table(uint8_t columns, uint8_t flags, const uint8_t *align);
void doc_cell(void);
void doc_table_end(void);

/* The C library the compiler may call. */
void *memcpy(void *d, const void *s, size_t n);
void *memset(void *d, int c, size_t n);
void *memmove(void *d, const void *s, size_t n);
int memcmp(const void *a, const void *b, size_t n);

#endif
