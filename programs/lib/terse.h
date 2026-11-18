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
#define CALL_PAGE_SUBMIT 0x700
#define CALL_PAGE_CLEAR 0x701
#define CALL_PAGE_REGION 0x702
#define CALL_PAGE_SCROLL 0x703
#define CALL_PAGE_BACKGROUND 0x704

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
/* Page laid out: the height (0-FFFFFFh); page anchor: the id and buttons. */
#define EV_PAGE_HEIGHT(e) ((e).a | (uint32_t)(e).b << 8 | (uint32_t)(e).c << 16)
#define EV_ANCHOR_ID(e) ((e).a | (uint16_t)(e).b << 8)
#define EV_ANCHOR_BUTTONS(e) ((e).c)

/* Copies the head event into *e and removes it; returns its type, EV_NONE
 * (and leaves *e alone) if the queue is empty. */
uint8_t terse_event_pop(terse_event *e);

/* Gives up the rest of this frame's budget and returns at the start of the
 * next frame. Returns the frames that passed since the last call (more than
 * 1 if the work in between ran out of budget). */
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

/* Put character for each of `len` bytes (SPEC 8.3). A write call stops
 * when the frame's budget runs out; this repeats it until every byte is
 * written (continuing in the next frame if need be). */
void term_write(const void *p, uint32_t len);
static inline void term_goto(uint8_t col, uint8_t row) { terse_call(CALL_TERM_SET_CURSOR, col, row, 0); }
static inline void term_color(uint8_t fg, uint8_t bg) { terse_call(CALL_TERM_SET_COLOURS, fg, bg, 0); }
static inline void term_cmd(uint8_t c) { terse_call(CALL_TERM_COMMAND, c, 0, 0); }
void term_putc(uint8_t byte);
void term_puts(const char *s);
/* Sets the foreground only. */
void term_fg(uint8_t fg);
/* Writes the printable ASCII bytes (20h-7Eh) at `p`, at most `max`,
 * stopping at any other byte; returns how many it wrote. */
uint32_t term_put_ascii(uint32_t max, const uint8_t *p);
/* Writes one code point as UTF-8 (0000h-FFFFh). */
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

/* Builder: appends commands at vec_ptr; the caller owns the buffer. */
extern uint8_t *vec_ptr;
#define vec_begin(buf) (vec_ptr = (buf))
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

static inline uint8_t content_submit(content_req *r) { return terse_call(CALL_CONTENT_SUBMIT, (uint32_t)r, 0, 0); }
uint8_t content_get(content_req *r, const uint8_t *id, uint32_t offset, uint32_t length, void *buf);
uint8_t content_feed(content_req *r, uint8_t op, uint32_t first, uint32_t length, void *buf);
/* Puts `size` bytes (ROM or RAM, copied at submit); on completion r->id is
 * the blob's ID. */
uint8_t content_put(content_req *r, const void *data, uint32_t size);
/* Subscribes to (CONTENT_SUBSCRIBE) or unsubscribes from
 * (CONTENT_UNSUBSCRIBE) the feed `key` (32 bytes). The host asks the user;
 * STATUS_REFUSED if the answer is no (SPEC 9.5). */
uint8_t content_follow(content_req *r, uint8_t op, const uint8_t *key);
/* Publishes blob `blob` (already in the host's store: put it first) as the
 * next entry of the host's feed, with a title of `len` bytes of UTF-8 (at
 * most 120, copied at submit). The host asks the user; on completion
 * r->size is the entry's seq. */
uint8_t content_publish(content_req *r, const uint8_t *blob, const void *title, uint32_t len);
/* The host's own feed key, in r->id on completion; STATUS_NOT_FOUND if the
 * host has no identity yet. */
uint8_t content_identity(content_req *r);

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

/* The C library the compiler may call. */
void *memcpy(void *d, const void *s, size_t n);
void *memset(void *d, int c, size_t n);
void *memmove(void *d, const void *s, size_t n);
int memcmp(const void *a, const void *b, size_t n);

#endif
