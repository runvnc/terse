/* terse.h: the Terse machine for C programs built with SDCC (SPEC.md).
 *
 * Ports are SDCC `__sfr` variables: `TERM_PUT = 'x';` is one OUT. Link
 * crt0.rel first (see programs/reader/build.sh). Machine profile 1. */

#ifndef TERSE_H
#define TERSE_H

#include <stdint.h>

/* Device 0, memory (SPEC 4): page register of each slot (slot n covers
 * n * 2000h). Pages 00h-0Fh are ROM, 80h + n is RAM page n. */
__sfr __at 0x00 MEM_SLOT0;
__sfr __at 0x01 MEM_SLOT1;
__sfr __at 0x02 MEM_SLOT2;
__sfr __at 0x03 MEM_SLOT3;
__sfr __at 0x04 MEM_SLOT4;
__sfr __at 0x05 MEM_SLOT5;
__sfr __at 0x06 MEM_SLOT6;
__sfr __at 0x07 MEM_SLOT7;
#define PAGE_RAM 0x80
#define PAGE_SIZE 0x2000

/* Device 1, system (SPEC 5). */
__sfr __at 0x10 SYS_EVENT;   /* read: head event type; write: remove it */
__sfr __at 0x11 SYS_EVENT_A;
__sfr __at 0x12 SYS_EVENT_B;
__sfr __at 0x13 SYS_EVENT_C;
__sfr __at 0x14 SYS_FRAME0;  /* frame counter, bytes 0-3 */
__sfr __at 0x18 SYS_TIME0;   /* Unix time latched at frame start, bytes 0-3 */
__sfr __at 0x1C SYS_RANDOM;
__sfr __at 0x1D SYS_PAD_SELECT; /* write: controller 0-3, restarts at byte 0 */
__sfr __at 0x1E SYS_PAD_DATA;   /* read: next byte of the selected controller */
__sfr __at 0x1F SYS_EXIT;

/* Any port by number (for a slot chosen at run time, say). */
uint8_t terse_in(uint8_t port);
void terse_out(uint8_t port, uint8_t value);

/* Event types (SPEC 5.1). */
#define EV_NONE 0x00
#define EV_KEY_DOWN 0x01
#define EV_KEY_UP 0x02
#define EV_TEXT 0x03
#define EV_POINTER 0x04
#define EV_CONTENT_DONE 0x05
#define EV_WHEEL 0x06
#define EV_FEEDS_UPDATED 0x07

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

/* Pointer position in screen units, 640 x 480 (SPEC 5.4); a cell is 8 x 16. */
#define EV_POINTER_X(e) ((e).a | (uint16_t)((e).c & 0x03) << 8)
#define EV_POINTER_Y(e) ((e).b | (uint16_t)((e).c & 0x04) << 6)
#define EV_POINTER_BUTTONS(e) ((e).c >> 3 & 0x07)

/* Copies the head event into *e and removes it; returns its type, EV_NONE
 * (and leaves *e alone) if the queue is empty. */
uint8_t terse_event_pop(terse_event *e);

/* Halts until the next frame interrupt, unless one has arrived since the
 * last call (the frame's work ran over). Returns the frames that passed. */
uint8_t terse_wait_frame(void);
extern volatile uint8_t terse_ticks; /* frame interrupts so far (crt0) */

uint32_t terse_frame(void);
uint32_t terse_time(void);
#define terse_exit(code) (SYS_EXIT = (code))

/* Controllers (SPEC 5.3). */
typedef struct {
    uint8_t connected;
    uint8_t buttons;  /* up, down, left, right, A, B, X, Y from bit 0 */
    uint8_t buttons2; /* L1, R1, L2, R2, select, start, L3, R3 */
    int8_t lx, ly, rx, ry;
} terse_pad;
void terse_pad_read(uint8_t n, terse_pad *pad);
#define PAD_UP 0x01      /* terse_pad.buttons */
#define PAD_DOWN 0x02
#define PAD_LEFT 0x04
#define PAD_RIGHT 0x08
#define PAD_A 0x10
#define PAD_B 0x20
#define PAD_X 0x40
#define PAD_Y 0x80
#define PAD_L1 0x01      /* terse_pad.buttons2 */
#define PAD_R1 0x02
#define PAD_L2 0x04
#define PAD_R2 0x08
#define PAD_SELECT 0x10
#define PAD_START 0x20

/* Device 2, terminal (SPEC 6). */
__sfr __at 0x20 TERM_PUT; /* one byte of UTF-8 */
__sfr __at 0x21 TERM_COL;
__sfr __at 0x22 TERM_ROW;
__sfr __at 0x23 TERM_FG;
__sfr __at 0x24 TERM_BG;
__sfr __at 0x25 TERM_CMD;
__sfr __at 0x26 TERM_PTR_LO;
__sfr __at 0x27 TERM_PTR_HI;
__sfr __at 0x28 TERM_DATA; /* cell memory at the pointer, which then advances */
__sfr __at 0x29 TERM_PAL_INDEX;
__sfr __at 0x2A TERM_PAL_DATA;
#define TERM_COLS 80
#define TERM_ROWS 30

#define TERM_CLEAR 0x01
#define TERM_SCROLL_UP 0x02
#define TERM_SCROLL_DOWN 0x03
#define TERM_CLEAR_EOL 0x04
#define TERM_CURSOR_ON 0x05
#define TERM_CURSOR_OFF 0x06
#define TERM_CONTINUATION 0xFFFF

#define term_goto(col, row) (TERM_COL = (col), TERM_ROW = (row))
#define term_color(fg, bg) (TERM_FG = (fg), TERM_BG = (bg))
#define term_cmd(c) (TERM_CMD = (c))
#define term_putc(byte) (TERM_PUT = (byte))
void term_puts(const char *s);
/* Writes the printable ASCII bytes (20h-7Eh) at `p`, at most `max`,
 * stopping at any other byte; returns how many it wrote. About 70 T-states
 * a character. */
uint8_t term_put_ascii(uint8_t max, const uint8_t *p);
/* Writes one code point as UTF-8 (0000h-FFFFh). */
void term_put_cp(uint16_t cp);
/* Writes `n` cells of `ch` (narrow) with colours fg/bg through the cell
 * pointer from (col, row); the cursor does not move. */
void term_fill(uint8_t col, uint8_t row, uint8_t n, uint16_t ch, uint8_t fg, uint8_t bg);
/* Sets the cell pointer to (col, row); then TERM_DATA writes 4 bytes a cell. */
void term_cell_ptr(uint8_t col, uint8_t row);
/* Whether a code point takes two cells (SPEC 6.1, Unicode 16.0 W/F). */
uint8_t term_is_wide(uint16_t cp);

/* Device 3, 2D vector display (SPEC 8). */
__sfr __at 0x30 VEC_ADDR0; /* list address: linear RAM, bits 0-7 */
__sfr __at 0x31 VEC_ADDR1;
__sfr __at 0x32 VEC_ADDR2;
__sfr __at 0x33 VEC_LEN_LO; /* list length, 1 to 16384 */
__sfr __at 0x34 VEC_LEN_HI;
__sfr __at 0x35 VEC_CMD;    /* write: VEC_SUBMIT or VEC_CLEAR; read: VEC_DONE or VEC_REJECTED */
__sfr __at 0x36 VEC_STYLE;  /* write: VEC_STYLE_CLEAN or VEC_STYLE_PHOSPHOR (SPEC 8.7) */
__sfr __at 0x37 VEC_PERSISTENCE; /* write: 0-255, trail kept per frame in 256ths; 160 at reset */
#define VEC_SUBMIT 0x01
#define VEC_CLEAR 0x02
#define VEC_DONE 0x00
#define VEC_REJECTED 0x01
#define VEC_MAX_LIST 16384
#define VEC_STYLE_CLEAN 0x00
#define VEC_STYLE_PHOSPHOR 0x01
#define VEC_PERSISTENCE_DEFAULT 160

/* Coordinates are signed sixteenths of a screen unit: the screen is
 * 10240 x 7680 (VEC_UNIT per screen unit, a cell is 128 x 256). */
#define VEC_UNIT 16
#define VEC_SCREEN_W 10240
#define VEC_SCREEN_H 7680

/* Display-list opcodes (SPEC 8.3). */
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

/* Builder: appends commands at vec_ptr (assembly, 60-150 T-states each).
 * The caller owns the buffer and its size:
 *
 *     static uint8_t list[512];
 *     vec_begin(list);
 *     vec_colour(255, 200, 0, 255); vec_move(x, y); vec_disc(48);
 *     vec_submit_list(list);
 */
extern uint8_t *vec_ptr;
#define vec_begin(buf) (vec_ptr = (buf))
#define vec_length(buf) ((uint16_t)(vec_ptr - (uint8_t *)(buf)))
void vec_colour(uint8_t r, uint8_t g, uint8_t b, uint8_t a);
void vec_width(uint8_t w);                 /* sixteenths; 0 counts as 1 */
void vec_origin(int16_t x, int16_t y);     /* added to later coordinates */
void vec_move(int16_t x, int16_t y);       /* sets the current point */
void vec_line(int16_t x, int16_t y);       /* line to (x, y), the new current point */
void vec_circle(uint16_t r);               /* outline around the current point */
void vec_disc(uint16_t r);                 /* filled, around the current point */
void vec_rect(int16_t x, int16_t y);       /* corners: current point and (x, y) */
void vec_tri(int16_t x1, int16_t y1, int16_t x2, int16_t y2); /* with the current point */
void vec_end(void);

/* Submits `len` bytes at linear RAM address `addr` (SPEC 8.2); returns
 * VEC_DONE or VEC_REJECTED. Costs 1 T-state per byte. */
uint8_t vec_submit(uint32_t addr, uint16_t len);
/* Submits the list built since vec_begin(buf); `buf` must show RAM. */
uint8_t vec_submit_list(const uint8_t *buf);
#define vec_clear() (VEC_CMD = VEC_CLEAR)
/* Phosphor style (SPEC 8.7): shapes add light, glow, are not filled and
 * leave a trail that fades by persistence / 256 each frame. The program
 * cannot see the trail; the user may turn it off. */
#define vec_style(s) (VEC_STYLE = (s))
#define vec_persistence(p) (VEC_PERSISTENCE = (p))

/* Device 4, content (SPEC 7). A request block must lie in RAM. */
__sfr __at 0x40 CONTENT_BLOCK_LO;
__sfr __at 0x41 CONTENT_BLOCK_HI;
__sfr __at 0x42 CONTENT_SUBMIT; /* write: submit; read: handle 1-8, or 0 */

#define CONTENT_GET 0x01
#define CONTENT_PUT 0x02
#define CONTENT_SUBSCRIPTIONS 0x03
#define CONTENT_TIMELINE 0x04

#define STATUS_PENDING 0x00
#define STATUS_DONE 0x01
#define STATUS_NOT_FOUND 0x02
#define STATUS_FAILED 0x03
#define STATUS_BAD_REQUEST 0x04

/* SPEC 7.2. 24-bit fields are byte arrays; see get24/set24. */
typedef struct {
    uint8_t op;
    volatile uint8_t status;
    uint8_t id[32];
    uint8_t offset[3];
    uint16_t length;
    uint8_t addr[3];
    uint8_t size[3];
} content_req;

/* Timeline record flags (SPEC 7.7). */
#define TIMELINE_LOCAL 0x01

uint32_t get24(const uint8_t *p);
void set24(uint8_t *p, uint32_t v);

/* The linear RAM address (SPEC 7.1) that `p` shows in the current mapping,
 * or 0xFFFFFFFF if it does not show RAM. */
uint32_t terse_linear(const void *p);

/* Submits a request block; returns the handle (1-8), or 0 if rejected (a
 * bad request also sets status 04h). Completion: an EV_CONTENT_DONE event
 * with the handle in `a` and the status in `b`; `r->status` changes too. */
uint8_t content_submit(content_req *r);
/* Fills and submits a get of `length` bytes of blob `id` at `offset`,
 * delivered to linear RAM address `addr`. */
uint8_t content_get(content_req *r, const uint8_t *id, uint32_t offset, uint16_t length, uint32_t addr);
/* Fills and submits a subscriptions or timeline request (SPEC 7.7):
 * records from index `first` into the `length`-byte buffer at `addr`. */
uint8_t content_feed(content_req *r, uint8_t op, uint32_t first, uint16_t length, uint32_t addr);

#endif
