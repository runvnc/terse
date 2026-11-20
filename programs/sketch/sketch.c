/* sketch: draw with the pointer on the 2D vector display (device 3) and keep
 * the drawing as a named record (SPEC 9.6) or publish it as a document
 * (SPEC 9.7). The terminal's bottom row holds the status and the keys;
 * built with clang (see build.sh).
 *
 * Drawing: the primary button pressed starts a stroke, moving with it held
 * adds points (a point closer than 2 screen units to the last one, on both
 * axes, is skipped), releasing it ends the stroke.
 *
 * Keys: 1-6 choose the colour (white, red, yellow, green, blue, magenta;
 * the disc at the bottom right shows it), U undoes the last stroke, C
 * clears every stroke, F switches between the clean and the phosphor
 * display style (SPEC 10.6), S saves the drawing as the record "drawing",
 * P publishes it as a document entry (the host asks the user).
 *
 * Drawing data: a list of strokes and nothing else, at most 8,192 bytes
 * (an empty drawing is 0 bytes). Each stroke is its colour (1 byte, 0-5:
 * the keys 1-6 less one), its number of points n (2 bytes, little-endian,
 * 1 or more), then n points, each x (2 bytes, little-endian, 0-639) and y
 * (2 bytes, little-endian, 0-479) in screen units (SPEC 7). A stroke is
 * drawn as a move to its first point and a line to each of its points in
 * turn (so a stroke of one point is a dot), 2 screen units wide. Data that
 * breaks any of these rules is not a drawing. The display list of a stroke
 * of n points takes 10 + 5n bytes; strokes whose lists would not fit in the
 * display list (16,384 bytes less 32 kept for the colour disc) are dropped
 * from the end when a drawing is loaded, and no stroke or point is added
 * once either limit is reached ("Full").
 *
 * The record "drawing" holds the drawing data. It is asked for in the first
 * frame; until it has arrived (or is found missing) and been checked and
 * drawn (1,024 points a frame, a stroke counting as two more), the pointer
 * and the keys other than F do nothing. Data that is not a drawing is not
 * shown ("The saved drawing is damaged"). Invalid drawing data in a sketch
 * document given as the argument is not shown either ("Not a sketch
 * drawing"); the record is loaded instead.
 *
 * Publishing: the document is "TDOC", 00h, 32 zero bytes (the format of a
 * built-in program), 06h, "sketch", then the drawing data. It is put, then
 * published as an entry of kind 4 (document) titled "Sketch N": N is one
 * more than the number of drawings this program has published on this
 * machine, kept as the record "published" (4 bytes, little-endian; missing
 * means 0) and saved again after each publish that succeeds.
 *
 * Started with an argument (a blob ID, SPEC 4), sketch gets that blob and,
 * if it is a sketch document, shows its drawing read-only instead of the
 * record "drawing": U, C, S and P do nothing ("Draw to edit a copy"). The
 * first press of the primary button makes it an ordinary drawing, a copy
 * that S saves as "drawing" like any other. If the blob cannot be got or is
 * not a sketch document, sketch says so and loads the record as usual.
 *
 * Content (device 4) is optional (SPEC 2; build.sh packs it with
 * --optional): on a host without it, such as one of the program profile
 * (devices 1, 2 and 3), sketch says "Drawings are not kept", loads
 * nothing, ignores an argument, and S and P only say that they cannot
 * save or publish there. Drawing is the same. */

#include <stdint.h>
#include "terse.h"

#define DATA_MAX 8192
#define NAME_LEN 6
#define HEAD (TDOC_HEAD + NAME_LEN) /* 44: the document header */
#define NCOLOURS 6
#define MIN_STEP 2
#define WIDTH 32                    /* 2 screen units */
#define TAIL_MAX 32
#define STROKES_END (VEC_MAX_LIST - TAIL_MAX)
#define STATUS_ROW 29
#define STATUS_COLS 27

#define KEY_1 0x1E
#define KEY_6 0x23
#define KEY_C 0x06
#define KEY_F 0x09
#define KEY_P 0x13
#define KEY_S 0x16
#define KEY_U 0x18

static const uint8_t palette[NCOLOURS][3] = {
    { 240, 240, 240 }, { 255, 70, 60 }, { 255, 210, 60 },
    { 80, 220, 100 },  { 70, 150, 255 }, { 230, 90, 230 },
};

/* The document: the header, then the drawing data (used bytes of it). */
static uint8_t doc[HEAD + DATA_MAX];
#define data (doc + HEAD)
static uint32_t used;
/* The display list: the width, the strokes up to vend, then the tail. */
static uint8_t vlist[VEC_MAX_LIST];
static uint32_t vend, vlen;

static uint8_t colour, phosphor, viewing, full, drawing, buttons, vdirty, sdirty;
static uint8_t keeps;     /* the machine has device 4: drawings are kept */
static uint32_t stroke;   /* where the stroke being drawn starts in data */
static uint16_t lastx, lasty;

static content_req load_req, count_req, save_req, count_save_req, pub_req, fetch_req;
static uint8_t load_h, count_h, save_h, count_save_h, put_h, pub_h, fetch_h;
static uint8_t arg[32], count_buf[4], blob[32];
static uint32_t published;
static char title[20];
static uint32_t title_len;

static const char *status = "";
static char status_buf[STATUS_COLS + 1];

static uint16_t get16(const uint8_t *p) { return p[0] | p[1] << 8; }
static void put16(uint8_t *p, uint16_t v)
{
    p[0] = (uint8_t)v;
    p[1] = v >> 8;
}

static void say(const char *s)
{
    status = s;
    sdirty = 1;
}

/* Appends a stroke's display list for its colour and first point. */
static void vec_stroke_start(uint8_t c, uint16_t x, uint16_t y)
{
    vec_colour(palette[c][0], palette[c][1], palette[c][2], 255);
    vec_move(x * VEC_UNIT, y * VEC_UNIT);
}

/* The colour disc after the strokes, and the End: the list to submit. */
static void tail(void)
{
    vec_ptr = vlist + vend;
    vec_colour(palette[colour][0], palette[colour][1], palette[colour][2], 255);
    vec_move(78 * 128 + 64, STATUS_ROW * 256 + 128);
    vec_disc(80);
    vec_colour(160, 160, 160, 255);
    vec_width(16);
    vec_circle(104);
    vec_end();
    vlen = vec_length(vlist);
}

/* Loading: the data at data[0..used) is checked and its display list
 * built, BUILD_POINTS points a frame, a new stroke counting as two more
 * (so that no frame runs long), from
 * data[bat], with bleft points of the stroke there still to come. Strokes
 * whose lists do not fit are dropped (used shrinks). Then built() is told
 * whether the data was a drawing (if not, used is 0). */
#define BUILD_POINTS 1024
static uint32_t bat, bleft;
static uint8_t building, from_doc;
static void built(int ok);

static void build_start(void)
{
    vend = 2;
    bat = bleft = 0;
    full = 0;
    building = 1;
}

static uint8_t *line_at(uint8_t *v, uint8_t op, const uint8_t *p)
{
    uint16_t x = get16(p) * VEC_UNIT, y = get16(p + 2) * VEC_UNIT;
    v[0] = op;
    v[1] = (uint8_t)x;
    v[2] = x >> 8;
    v[3] = (uint8_t)y;
    v[4] = y >> 8;
    return v + 5;
}

static int point_ok(const uint8_t *p) { return get16(p) < 640 && get16(p + 2) < 480; }

static void build_step(void)
{
    int todo = BUILD_POINTS;
    uint32_t n;
    uint8_t *v = vlist + vend;
    const uint8_t *c;
    while (todo > 0) {
        if (!bleft) {
            if (bat == used)
                break;
            if (used - bat < 7 || data[bat] >= NCOLOURS || !(n = get16(data + bat + 1)) || (used - bat - 3) / 4 < n
                || !point_ok(data + bat + 3))
                goto bad;
            if ((uint32_t)(v - vlist) + 10 + 5 * n > STROKES_END) {
                used = bat;
                break;
            }
            c = palette[data[bat]];
            v[0] = VEC_OP_COLOUR;
            v[1] = c[0];
            v[2] = c[1];
            v[3] = c[2];
            v[4] = 255;
            v = line_at(v + 5, VEC_OP_MOVE, data + bat + 3);
            bat += 3;
            bleft = n;
            todo -= 2;
        }
        if (!point_ok(data + bat))
            goto bad;
        v = line_at(v, VEC_OP_LINE, data + bat);
        bat += 4;
        bleft--;
        todo--;
    }
    vend = v - vlist;
    vdirty = 1;
    if (bleft || bat < used)
        return;
    building = 0;
    built(1);
    return;
bad:
    used = 0;
    vend = 2;
    vdirty = 1;
    building = 0;
    built(0);
}

/* Adds a point to the stroke being drawn: 0 if there is no room. */
static int add_point(uint16_t x, uint16_t y)
{
    if (used + 4 > DATA_MAX || vend + 5 > STROKES_END)
        return 0;
    put16(data + used, x);
    put16(data + used + 2, y);
    used += 4;
    put16(data + stroke + 1, get16(data + stroke + 1) + 1);
    vec_ptr = vlist + vend;
    vec_line(x * VEC_UNIT, y * VEC_UNIT);
    vend += 5;
    lastx = x;
    lasty = y;
    vdirty = 1;
    return 1;
}

static void set_full(void)
{
    full = 1;
    drawing = 0;
    say("Full: undo or clear");
}

static void begin_stroke(uint16_t x, uint16_t y)
{
    if (used + 7 > DATA_MAX || vend + 15 > STROKES_END) {
        set_full();
        return;
    }
    stroke = used;
    data[used] = colour;
    put16(data + used + 1, 0);
    used += 3;
    vec_ptr = vlist + vend;
    vec_stroke_start(colour, x, y);
    vend += 10;
    add_point(x, y);
    drawing = 1;
}

static void pointer(const terse_event *e)
{
    uint8_t b = EV_POINTER_BUTTONS(*e), was = buttons;
    uint16_t x = EV_POINTER_X(*e), y = EV_POINTER_Y(*e);
    buttons = b;
    if (x > 639)
        x = 639;
    if (y > 479)
        y = 479;
    if ((b & BUTTON_PRIMARY) && !(was & BUTTON_PRIMARY)) {
        if (viewing) {
            viewing = 0;
            say("Editing a copy; S saves it");
        }
        begin_stroke(x, y);
        return;
    }
    if (!drawing)
        return;
    int dx = x - lastx, dy = y - lasty;
    if (dx < 0)
        dx = -dx;
    if (dy < 0)
        dy = -dy;
    if ((dx >= MIN_STEP || dy >= MIN_STEP) && !add_point(x, y))
        set_full();
    if (!(b & BUTTON_PRIMARY))
        drawing = 0;
}

/* The offset in data of the last stroke (used must not be 0). */
static uint32_t last_stroke(void)
{
    uint32_t at = 0, prev = 0;
    while (at < used) {
        prev = at;
        at += 3 + get16(data + at + 1) * 4;
    }
    return prev;
}

static void undo(void)
{
    if (!used)
        return;
    uint32_t at = last_stroke();
    vend -= 10 + 5 * get16(data + at + 1);
    used = at;
    full = 0;
    vdirty = 1;
    say("Undone");
}

static void clear(void)
{
    used = 0;
    vend = 2;
    full = 0;
    vdirty = 1;
    say("Cleared");
}

static void save(void)
{
    if (!keeps) {
        say("No saving on this host");
        return;
    }
    if (save_h) {
        say("Saving...");
        return;
    }
    save_h = record_save(&save_req, "drawing", data, used);
    say(save_h ? "Saving..." : "Not saved");
}

static void publish(void)
{
    static const uint8_t name[NAME_LEN] = { 's', 'k', 'e', 't', 'c', 'h' };
    uint32_t n, i;
    char digits[10];
    if (!keeps) {
        say("No publishing on this host");
        return;
    }
    if (put_h || pub_h) {
        say("Publishing...");
        return;
    }
    if (!used) {
        say("Nothing to publish");
        return;
    }
    doc[0] = 'T';
    doc[1] = 'D';
    doc[2] = 'O';
    doc[3] = 'C';
    memset(doc + 4, 0, 33);
    doc[TDOC_HEAD - 1] = NAME_LEN;
    memcpy(doc + TDOC_HEAD, name, NAME_LEN);
    /* "Sketch N" */
    memcpy(title, "Sketch ", 7);
    n = published + 1;
    i = 0;
    do {
        digits[i++] = '0' + n % 10;
        n /= 10;
    } while (n);
    title_len = 7;
    while (i)
        title[title_len++] = digits[--i];
    put_h = content_put(&pub_req, doc, HEAD + used);
    say(put_h ? "Publishing..." : "Not published");
}

static void key(uint8_t k)
{
    if (k >= KEY_1 && k <= KEY_6) {
        colour = k - KEY_1;
        vdirty = 1;
        return;
    }
    if (k == KEY_F) {
        phosphor ^= 1;
        vec_style(phosphor ? VEC_STYLE_PHOSPHOR : VEC_STYLE_CLEAN);
        return;
    }
    if (k != KEY_U && k != KEY_C && k != KEY_S && k != KEY_P)
        return;
    if (viewing) {
        say("Draw to edit a copy");
        return;
    }
    drawing = 0;
    if (k == KEY_U)
        undo();
    else if (k == KEY_C)
        clear();
    else if (k == KEY_S)
        save();
    else
        publish();
}

static void load_drawing(void)
{
    load_h = record_load(&load_req, "drawing", 0, DATA_MAX, data);
}

/* The argument's blob arrived (or did not). */
static void fetched(void)
{
    static const uint8_t magic[5] = { 'T', 'D', 'O', 'C', 0 };
    static const uint8_t name[NAME_LEN + 1] = { NAME_LEN, 's', 'k', 'e', 't', 'c', 'h' };
    uint32_t i, size = fetch_req.size;
    uint8_t zero = 0;
    if (fetch_req.status != STATUS_DONE) {
        say(fetch_req.status == STATUS_NOT_FOUND ? "Document not found" : "Could not get the document");
        load_drawing();
        return;
    }
    for (i = 0; i < 32; i++)
        zero |= doc[5 + i];
    if (size < HEAD || size > HEAD + DATA_MAX || fetch_req.length != size || memcmp(doc, magic, 5) || zero
        || memcmp(doc + TDOC_HEAD - 1, name, NAME_LEN + 1)) {
        say("Not a sketch document");
        used = 0;
        load_drawing();
        return;
    }
    used = size - HEAD;
    from_doc = 1;
    build_start();
}

/* The drawing is checked and shown (ok), or it was not a drawing. */
static void built(int ok)
{
    if (from_doc) {
        from_doc = 0;
        if (ok) {
            viewing = 1;
            say("Viewing a published sketch");
        } else {
            say("Not a sketch drawing");
            load_drawing();
        }
    } else if (!ok)
        say("The saved drawing is damaged");
}

static void done(uint8_t h)
{
    if (h == fetch_h) {
        fetch_h = 0;
        fetched();
    } else if (h == load_h) {
        load_h = 0;
        used = 0;
        if (load_req.status == STATUS_DONE && load_req.size <= DATA_MAX)
            used = load_req.length;
        if (load_req.status == STATUS_DONE && load_req.size > DATA_MAX)
            say("The saved drawing is damaged");
        build_start();
    } else if (h == count_h) {
        count_h = 0;
        if (count_req.status == STATUS_DONE && count_req.length == 4)
            published = count_buf[0] | count_buf[1] << 8 | (uint32_t)count_buf[2] << 16 | (uint32_t)count_buf[3] << 24;
    } else if (h == save_h) {
        save_h = 0;
        say(save_req.status == STATUS_DONE ? "Saved" : "Not saved");
    } else if (h == count_save_h) {
        count_save_h = 0;
    } else if (h == put_h) {
        put_h = 0;
        if (pub_req.status != STATUS_DONE) {
            say("Not published");
            return;
        }
        memcpy(blob, pub_req.id, 32);
        pub_h = content_publish_kind(&pub_req, blob, title, title_len, ENTRY_DOCUMENT);
        if (!pub_h)
            say("Not published");
    } else if (h == pub_h) {
        pub_h = 0;
        if (pub_req.status != STATUS_DONE) {
            say("Not published");
            return;
        }
        published++;
        for (int i = 0; i < 4; i++)
            count_buf[i] = (uint8_t)(published >> (8 * i));
        if (!count_save_h)
            count_save_h = record_save(&count_save_req, "published", count_buf, 4);
        memcpy(status_buf, "Published as ", 13);
        memcpy(status_buf + 13, title, title_len);
        status_buf[13 + title_len] = 0;
        say(status_buf);
    }
}

static void show_status(void)
{
    sdirty = 0;
    term_fill(1, STATUS_ROW, STATUS_COLS, ' ', 15, 0);
    term_goto(1, STATUS_ROW);
    term_color(full ? 214 : 15, 0);
    term_puts(status);
}

static int busy(void) { return load_h || fetch_h || count_h || building; }

int main(void)
{
    term_cmd(TERM_CURSOR_OFF);
    term_goto(28, STATUS_ROW);
    term_color(244, 0);
    term_puts("1-6 colour U undo C clear S save P publish F glow");
    vec_begin(vlist, sizeof vlist);
    vec_width(WIDTH);
    vend = 2;
    vdirty = 1;
    keeps = terse_has_device(4);
    if (!keeps)
        say("Drawings are not kept");
    else if (terse_argument(arg)) {
        fetch_h = content_get(&fetch_req, arg, 0, sizeof doc, doc);
        if (!fetch_h) {
            say("Could not get the document");
            load_drawing();
        } else
            say("Loading...");
    } else
        load_drawing();
    if (keeps)
        count_h = record_load(&count_req, "published", 0, 4, count_buf);
    for (;;) {
        terse_event ev;
        while (terse_event_pop(&ev)) {
            if (ev.type == EV_CONTENT_DONE) {
                done(ev.a);
                continue;
            }
            /* Nothing but the style until the drawing is in. */
            if (ev.type == EV_KEY_DOWN && (ev.a == KEY_F || !busy()))
                key(ev.a);
            else if (ev.type == EV_POINTER) {
                if (busy())
                    buttons = EV_POINTER_BUTTONS(ev);
                else
                    pointer(&ev);
            }
        }
        if (building)
            build_step();
        if (vdirty) {
            vdirty = 0;
            tail();
            vec_submit(vlist, vlen);
        }
        if (sdirty)
            show_status();
        terse_wait_frame();
    }
}
