/* shell: starts the host's built-in programs as machines of their own and
 * lays their screens out as tiles (device 8, SPEC 12); built with clang
 * (see build.sh).
 *
 * Row 0 is a bar: the home segment, one segment per machine (handle, a
 * state mark, title, units used in its last frame as a share of the
 * budget) and the clock (UTC). The machines are tiled below it: one zoomed
 * to the whole area, a master on the left half with the others stacked on
 * the right, or a grid; each picture at the top of its tile. The focused machine has a blue frame, drawn with
 * the vector display in the gap around its picture.
 *
 * The host's home key gives the focus back to the shell: command mode. A
 * launcher on the page lists the built-in programs and the machines, and
 * the tiles move right to make room for it. Keys in command mode: Up and
 * Down move the cursor, Enter starts the program under it (and focuses
 * it) or focuses the machine under it, Tab or Right and Shift+Tab or Left
 * select the next or previous machine, Esc returns the focus to the
 * selected machine, X stops it, P pauses or resumes it, Z zooms it (or
 * goes back to the layout before), L cycles the layouts, 1-8 select a
 * machine by handle. A press on a machine's picture or its bar segment
 * focuses it; one on the home segment enters command mode. */

#include <stdint.h>
#include "terse.h"

#define BUDGET 125000u
#define MAX_PROGS 16
#define AREA_Y 20 /* the tiles' area: below the bar and a gap */
#define AREA_H (480 - AREA_Y)
#define GAP 6     /* between tiles; the focus frame is drawn in it */
#define PANEL_W 200 /* the launcher beside the tiles in command mode */

enum { ZOOM, MASTER, GRID, LAYOUTS };

#define KEY_1 0x1E
#define KEY_8 0x25
#define KEY_L 0x0F
#define KEY_P 0x13
#define KEY_X 0x1B
#define KEY_Z 0x1D

/* Colours (xterm palette) of the bar. */
#define BAR_FG 250
#define BAR_BG 236
#define FOCUS_FG 15
#define FOCUS_BG 25
#define SEL_FG 16
#define SEL_BG 179

typedef struct {
    uint16_t x, y, w, h;
} rect;

/* The built-in programs: IDs, titles (from each image's header). */
static uint8_t prog_id[MAX_PROGS][32];
static uint8_t prog_title[MAX_PROGS][17];
static uint8_t prog_n, prog_known[MAX_PROGS], prog_asked[MAX_PROGS];
static content_req prog_req[MAX_PROGS];
static uint8_t prog_hdr[MAX_PROGS][32];

static mach_record info[MACH_MAX + 1]; /* by handle; [0] unused */
static rect tile[MACH_MAX + 1], pic[MACH_MAX + 1];
static uint8_t focus;        /* 0: the shell (command mode) */
static uint8_t sel;          /* the selected machine, 0 if none */
static uint8_t cursor;       /* command mode: 0..prog_n-1 programs, then machines */
static uint8_t layout = MASTER, before_zoom = MASTER;
static uint8_t dirty = 1;    /* the layout, labels, frames and launcher must be redone */
static uint8_t doc_dirty = 1;
static uint8_t bar_dirty = 1;
static uint32_t doc_height; /* the launcher's height, as last laid out */
static char note[40];        /* a message in the launcher */

static terse_event ev;
static uint32_t bar[TERM_COLS];
static uint8_t vlist[512];
static uint8_t doc[4096];

static uint8_t in_use(uint8_t h) { return info[h].state != MACH_NONE; }
static uint8_t live(uint8_t h)
{
    uint8_t s = info[h].state;
    return s == MACH_LOADING || s == MACH_RUNNING || s == MACH_PAUSED;
}

static uint8_t machine_count(void)
{
    uint8_t n = 0;
    for (uint8_t h = 1; h <= MACH_MAX; h++)
        n += in_use(h);
    return n;
}

/* The nth machine in use (0-based), 0 if none. */
static uint8_t nth_machine(uint8_t n)
{
    for (uint8_t h = 1; h <= MACH_MAX; h++)
        if (in_use(h) && n-- == 0)
            return h;
    return 0;
}

/* The program whose ID is `id`, or MAX_PROGS. */
static uint8_t prog_of(const uint8_t *id)
{
    for (uint8_t i = 0; i < prog_n; i++)
        if (!memcmp(prog_id[i], id, 32))
            return i;
    return MAX_PROGS;
}

static uint8_t prog_ix[MACH_MAX + 1]; /* each machine's program, MAX_PROGS if none */

static const uint8_t *title_of(uint8_t h)
{
    static const uint8_t unknown[] = "?";
    uint8_t p = prog_ix[h];
    return p < MAX_PROGS && prog_known[p] ? prog_title[p] : unknown;
}

static uint32_t title_len(const uint8_t *t)
{
    uint32_t n = 0;
    while (n < 16 && t[n])
        n++;
    return n;
}

/* Decodes one UTF-8 character at *p (BMP only; malformed bytes are U+FFFD). */
static uint16_t utf8_next(const uint8_t **p)
{
    const uint8_t *s = *p;
    uint16_t c = s[0];
    uint8_t n = 1;
    if (c >= 0xE0 && (s[1] & 0xC0) == 0x80 && (s[2] & 0xC0) == 0x80) {
        c = (c & 0x0F) << 12 | (s[1] & 0x3F) << 6 | (s[2] & 0x3F);
        n = 3;
    } else if (c >= 0xC0 && (s[1] & 0xC0) == 0x80) {
        c = (c & 0x1F) << 6 | (s[1] & 0x3F);
        n = 2;
    } else if (c >= 0x80) {
        c = 0xFFFD;
    }
    *p = s + n;
    return c;
}

/* --- Layout -------------------------------------------------------------- */

/* Where a 640 x 480 picture lies in r: scaled to fit and centred, exactly
 * as the host does it (SPEC 12.3). */
static rect fit(rect r)
{
    rect p;
    if ((uint32_t)r.w * 480 <= (uint32_t)r.h * 640) {
        p.w = r.w;
        p.h = (uint32_t)r.w * 480 / 640;
    } else {
        p.w = (uint32_t)r.h * 640 / 480;
        p.h = r.h;
    }
    p.x = r.x + (r.w - p.w) / 2;
    p.y = r.y + (r.h - p.h) / 2;
    return p;
}

static rect make(uint32_t x0, uint32_t y0, uint32_t x1, uint32_t y1)
{
    rect r = { x0 + GAP / 2, y0 + GAP / 2, x1 - x0 - GAP, y1 - y0 - GAP };
    return r;
}

/* The machine a zoom shows: the focused one, else the selected, else the
 * first. */
static uint8_t zoomed(void)
{
    if (focus && in_use(focus))
        return focus;
    if (sel && in_use(sel))
        return sel;
    return nth_machine(0);
}

/* Places every machine in the area (beside the launcher in command mode)
 * and remembers the tiles and pictures. */
static void place_all(void)
{
    uint32_t ax = focus ? 0 : PANEL_W, aw = 640 - ax, ay = AREA_Y, ah = AREA_H;
    uint8_t n = machine_count(), z = layout == ZOOM ? zoomed() : 0, cols = 1, rows;
    while (cols * cols < n)
        cols++;
    rows = n ? (n + cols - 1) / cols : 1;
    uint8_t i = 0;
    for (uint8_t h = 1; h <= MACH_MAX; h++) {
        rect r = { 0, 0, 0, 0 };
        if (in_use(h)) {
            if (layout == ZOOM) {
                if (h == z)
                    r = make(ax, ay, ax + aw, ay + ah);
            } else if (n == 1) {
                r = make(ax, ay, ax + aw, ay + ah);
            } else if (layout == MASTER) {
                if (i == 0)
                    r = make(ax, ay, ax + aw / 2, ay + ah);
                else
                    r = make(ax + aw / 2, ay + (i - 1) * ah / (n - 1), ax + aw, ay + i * ah / (n - 1));
            } else {
                uint8_t c = i % cols, w = i / cols;
                r = make(ax + c * aw / cols, ay + w * ah / rows, ax + (c + 1) * aw / cols, ay + (w + 1) * ah / rows);
            }
            i++;
        }
        /* The picture at the top of its tile, centred across it. */
        if (r.w && r.h) {
            rect p = fit(r);
            r.x = p.x;
            r.w = p.w;
            r.h = p.h;
        }
        tile[h] = r;
        pic[h] = r.w && r.h ? fit(r) : r;
        mach_place(h, r.x, r.y, r.w, r.h);
    }
}

/* --- Drawing ------------------------------------------------------------- */

static uint32_t cell(uint16_t ch, uint8_t fg, uint8_t bg) { return ch | (uint32_t)fg << 16 | (uint32_t)bg << 24; }

/* Writes `s` (UTF-8, at most `max` bytes or up to a 0) into bar cells from
 * column `col`, at most `width` cells; returns the cells written. */
static uint8_t bar_text(uint8_t col, uint8_t width, const uint8_t *s, uint32_t max, uint8_t fg, uint8_t bg)
{
    const uint8_t *p = s, *end = s + max;
    uint8_t n = 0;
    while (n < width && p < end && *p) {
        uint16_t c = utf8_next(&p);
        if (c >= 0x1100 && term_is_wide(c))
            c = '?';
        bar[col + n++] = cell(c, fg, bg);
    }
    return n;
}

static uint8_t bar_num(uint8_t col, uint32_t v, uint8_t fg, uint8_t bg)
{
    char b[10];
    uint8_t n = 0;
    do {
        b[n++] = '0' + v % 10;
        v /= 10;
    } while (v);
    for (uint8_t i = 0; i < n; i++)
        bar[col + i] = cell(b[n - 1 - i], fg, bg);
    return n;
}

static uint16_t mark(uint8_t state)
{
    switch (state) {
    case MACH_LOADING: return 0x2026; /* … */
    case MACH_PAUSED: return 0x2016;  /* ‖ */
    case MACH_EXITED: return 0x25A0;  /* ■ */
    case MACH_FAULTED: return '!';
    case MACH_FAILED: return 0x2717;  /* ✗ */
    default: return ' ';
    }
}

/* Row 0: home segment, machines, clock. */
static void draw_bar(void)
{
    for (uint8_t i = 0; i < TERM_COLS; i++)
        bar[i] = cell(' ', BAR_FG, BAR_BG);
    uint8_t home = !focus;
    bar[0] = cell(' ', home ? FOCUS_FG : BAR_FG, home ? FOCUS_BG : BAR_BG);
    bar[1] = cell(0x2302, home ? FOCUS_FG : BAR_FG, home ? FOCUS_BG : BAR_BG); /* ⌂ */
    bar[2] = bar[0];
    uint8_t n = machine_count(), col = 4;
    uint8_t width = n ? (TERM_COLS - 4 - 7) / n : 0;
    if (width > 20)
        width = 20;
    for (uint8_t h = 1; h <= MACH_MAX; h++) {
        if (!in_use(h))
            continue;
        uint8_t fg = BAR_FG, bg = 238;
        if (h == focus)
            fg = FOCUS_FG, bg = FOCUS_BG;
        else if (!focus && h == sel)
            fg = SEL_FG, bg = SEL_BG;
        uint8_t w = width - 1; /* a space between segments */
        for (uint8_t i = 0; i < w; i++)
            bar[col + i] = cell(' ', fg, bg);
        bar[col] = cell('0' + h, fg, bg);
        bar[col + 1] = cell(mark(info[h].state), fg, bg);
        /* The share of the budget, right-aligned. */
        uint32_t pct = info[h].units * 100 / BUDGET;
        if (pct > 999)
            pct = 999;
        uint8_t digits = pct >= 100 ? 3 : pct >= 10 ? 2 : 1;
        uint8_t pc = w >= 6 + digits ? col + w - digits - 1 : 0;
        if (pc) {
            bar_num(pc, pct, fg, bg);
            bar[pc + digits] = cell('%', fg, bg);
        }
        uint8_t room = pc ? pc - col - 3 : w - 2;
        if (w > 2)
            bar_text(col + 2, room, title_of(h), 16, fg, bg);
        col += width;
    }
    /* The clock, UTC. */
    uint32_t t = terse_time(), m = t / 60 % 60, hr = t / 3600 % 24;
    uint8_t c = TERM_COLS - 6;
    bar[c] = cell(' ', BAR_FG, BAR_BG);
    bar[c + 1] = cell('0' + hr / 10, BAR_FG, BAR_BG);
    bar[c + 2] = cell('0' + hr % 10, BAR_FG, BAR_BG);
    bar[c + 3] = cell(':', BAR_FG, BAR_BG);
    bar[c + 4] = cell('0' + m / 10, BAR_FG, BAR_BG);
    bar[c + 5] = cell('0' + m % 10, BAR_FG, BAR_BG);
    term_cells_write(0, bar, TERM_COLS);
}

/* Labels in the shell's own cells where a machine has no picture yet (or
 * never will): loading, failed. */
static void draw_labels(void)
{
    term_cmd(TERM_CLEAR); /* the bar is written again at once */
    for (uint8_t h = 1; h <= MACH_MAX; h++) {
        uint8_t s = info[h].state;
        if ((s != MACH_LOADING && s != MACH_FAILED) || !tile[h].w || !tile[h].h)
            continue;
        uint8_t row = (tile[h].y + tile[h].h / 2) / 16, col = tile[h].x / 8 + 1;
        term_goto(col, row);
        term_color(s == MACH_FAILED ? 9 : 244, 0);
        term_putc('0' + h);
        term_putc(' ');
        const uint8_t *t = title_of(h);
        term_put_ascii(16, t);
        term_puts(s == MACH_FAILED ? ": could not start" : ": loading");
    }
    term_color(7, 0);
}

/* A rectangle outline 2 units outside picture p, in sixteenths. */
static void outline(rect p, uint8_t r, uint8_t g, uint8_t b)
{
    int16_t x0 = (p.x - 2) * 16, y0 = (p.y - 2) * 16, x1 = (p.x + p.w + 2) * 16, y1 = (p.y + p.h + 2) * 16;
    vec_colour(r, g, b, 255);
    vec_width(20);
    vec_move(x0, y0);
    vec_line(x1, y0);
    vec_line(x1, y1);
    vec_line(x0, y1);
    vec_line(x0, y0);
}

static void draw_frames(void)
{
    vec_begin(vlist);
    if (focus && pic[focus].w)
        outline(pic[focus], 70, 150, 255);
    else if (!focus && sel && pic[sel].w)
        outline(pic[sel], 230, 180, 60);
    vec_end();
    vec_submit_list(vlist);
}

/* --- The launcher (command mode) ----------------------------------------- */

static void doc_str(const char *s)
{
    uint32_t n = 0;
    while (s[n])
        n++;
    doc_text(s, n);
}

static void doc_num(uint32_t v, uint8_t hex)
{
    char b[12];
    uint8_t n = 0;
    do {
        uint8_t d = hex ? v & 15 : v % 10;
        b[n++] = d < 10 ? '0' + d : 'a' + d - 10;
        v = hex ? v >> 4 : v / 10;
    } while (v || (hex && n < 8));
    char o[12];
    for (uint8_t i = 0; i < n; i++)
        o[i] = b[n - 1 - i];
    doc_text(o, n);
}

static const char *state_name(uint8_t s)
{
    static const char *const names[] = { "", "loading", "running", "paused", "exited", "faulted", "failed" };
    return s < 7 ? names[s] : "?";
}

static const char *fault_name(uint8_t k)
{
    static const char *const names[] = { "?", "illegal instruction", "breakpoint", "fetch", "load", "store", "misaligned" };
    return k < 7 ? names[k] : "?";
}

/* One line of the launcher: highlighted when the cursor is on it. */
static void line(uint8_t item, uint16_t anchor)
{
    doc_paragraph(DOC_START, 6, 4, 0);
    doc_anchor(anchor);
    if (item == cursor)
        doc_colour(255, 200, 95);
    else
        doc_colour(220, 222, 228);
}

static void build_launcher(void)
{
    uint8_t n = machine_count();
    doc_begin(doc, sizeof doc);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(13);
    doc_colour(150, 160, 180);
    doc_paragraph(DOC_START, 6, 8, 0);
    doc_str("PROGRAMS");
    doc_font(DOC_SANS, 0);
    doc_size(15);
    for (uint8_t i = 0; i < prog_n; i++) {
        line(i, 100 + i);
        if (prog_known[i])
            doc_text(prog_title[i], title_len(prog_title[i]));
        else
            doc_str("\xE2\x80\xA6");
    }
    if (n) {
        doc_anchor(0);
        doc_font(DOC_SANS, DOC_BOLD);
        doc_size(13);
        doc_colour(150, 160, 180);
        doc_paragraph(DOC_START, 6, 14, 0);
        doc_str("MACHINES");
        doc_font(DOC_SANS, 0);
        for (uint8_t i = 0; i < n; i++) {
            uint8_t h = nth_machine(i);
            mach_record *r = &info[h];
            doc_size(15);
            line(prog_n + i, h);
            doc_num(h, 0);
            doc_str("  ");
            doc_text(title_of(h), title_len(title_of(h)));
            doc_size(12);
            doc_colour(150, 160, 180);
            doc_str("  ");
            doc_str(state_name(r->state));
            if (r->state == MACH_EXITED) {
                doc_str(" ");
                doc_num(r->exit_code, 0);
            }
            if (r->state == MACH_RUNNING || r->state == MACH_PAUSED) {
                doc_str("  ");
                doc_num(r->units * 100 / BUDGET, 0);
                doc_str("%");
            }
            if (r->state == MACH_FAULTED) {
                doc_paragraph(DOC_START, 24, 0, 0);
                doc_colour(255, 120, 110);
                doc_str(fault_name(r->fault_kind));
                doc_str(" at pc ");
                doc_num(r->fault_pc, 1);
            }
        }
    }
    doc_anchor(0);
    if (note[0]) {
        doc_size(12);
        doc_colour(255, 120, 110);
        doc_paragraph(DOC_START, 6, 12, 0);
        doc_str(note);
    }
    doc_size(11);
    doc_colour(130, 138, 155);
    doc_paragraph(DOC_START, 6, 16, 0);
    doc_str(n ? "Enter start or focus \xC2\xB7 Esc back \xC2\xB7 Tab next machine \xC2\xB7 X stop \xC2\xB7 P pause "
                "\xC2\xB7 Z zoom \xC2\xB7 L layout \xC2\xB7 1-8 select"
              : "Up, Down choose \xC2\xB7 Enter start \xC2\xB7 the home key comes back here");
    doc_end();
    page_submit_doc(doc);
}

static void show_launcher(void)
{
    if (focus) {
        page_region(0, 0, 0, 0);
        return;
    }
    if (machine_count())
        page_region(GAP / 2, AREA_Y + GAP / 2, PANEL_W - GAP, AREA_H - GAP);
    else
        page_region(160, 80, 320, doc_height && doc_height < 340 ? doc_height + 8 : 340);
    if (doc_dirty)
        build_launcher();
    doc_dirty = 0;
}

/* --- Commands ------------------------------------------------------------ */

static void select_machine(uint8_t h)
{
    sel = h;
    for (uint8_t i = 0; i < machine_count(); i++)
        if (nth_machine(i) == h)
            cursor = prog_n + i;
    dirty = doc_dirty = 1;
}

/* Focus machine h (or enter command mode if it cannot take the focus). */
static void focus_machine(uint8_t h)
{
    if (h && mach_focus(h)) {
        focus = h;
        sel = h;
    } else {
        mach_focus(0);
        focus = 0;
        if (h)
            select_machine(h);
    }
    dirty = doc_dirty = 1;
}

static void enter_command_mode(void)
{
    if (focus)
        select_machine(focus);
    focus = 0;
    dirty = doc_dirty = 1;
}

static void start_program(uint8_t p)
{
    uint8_t h = mach_start(prog_id[p]);
    note[0] = 0;
    if (!h) {
        const char *m = "No free handle: stop a machine first (X).";
        uint8_t i = 0;
        while ((note[i] = m[i]))
            i++;
        doc_dirty = 1;
        return;
    }
    info[h].state = MACH_LOADING;
    memcpy(info[h].image, prog_id[p], 32);
    focus_machine(h);
}

static void step_machine(int8_t d)
{
    uint8_t n = machine_count();
    if (!n)
        return;
    uint8_t at = 0;
    for (uint8_t i = 0; i < n; i++)
        if (nth_machine(i) == sel)
            at = i;
    select_machine(nth_machine((at + n + d) % n));
}

static void command_key(uint8_t key, uint8_t mods)
{
    uint8_t items = prog_n + machine_count();
    switch (key) {
    case KEY_UP:
        if (cursor)
            cursor--;
        break;
    case KEY_DOWN:
        if (cursor + 1 < items)
            cursor++;
        break;
    case KEY_TAB:
        step_machine(mods & MOD_SHIFT ? -1 : 1);
        return;
    case KEY_RIGHT:
        step_machine(1);
        return;
    case KEY_LEFT:
        step_machine(-1);
        return;
    case KEY_ENTER:
        if (cursor < prog_n)
            start_program(cursor);
        else
            focus_machine(nth_machine(cursor - prog_n));
        return;
    case KEY_ESCAPE:
        if (sel && live(sel))
            focus_machine(sel);
        return;
    case KEY_X:
        if (sel) {
            mach_stop(sel);
            info[sel].state = MACH_NONE;
            sel = nth_machine(0);
            if (cursor >= prog_n + machine_count())
                cursor = prog_n + machine_count() - 1;
            if (sel)
                select_machine(sel);
            dirty = doc_dirty = 1;
        }
        return;
    case KEY_P:
        if (sel && (info[sel].state == MACH_RUNNING || info[sel].state == MACH_PAUSED)) {
            mach_run(sel, info[sel].state == MACH_PAUSED);
            dirty = doc_dirty = 1;
        }
        return;
    case KEY_Z:
        if (layout == ZOOM) {
            layout = before_zoom;
        } else {
            before_zoom = layout;
            layout = ZOOM;
        }
        dirty = 1;
        return;
    case KEY_L:
        layout = (layout + 1) % LAYOUTS;
        if (layout != ZOOM)
            before_zoom = layout;
        dirty = 1;
        return;
    default:
        if (key >= KEY_1 && key <= KEY_8 && in_use(key - KEY_1 + 1))
            select_machine(key - KEY_1 + 1);
        return;
    }
    /* The cursor moved: a machine under it is the selected one. */
    if (cursor >= prog_n)
        sel = nth_machine(cursor - prog_n);
    dirty = doc_dirty = 1;
}

/* A press at (x, y) in the shell's own screen: the bar's segments. */
static void press(uint16_t x, uint16_t y)
{
    if (y >= 16)
        return;
    uint8_t col = x / 8, n = machine_count();
    if (col < 3) {
        enter_command_mode();
        mach_focus(0);
        return;
    }
    uint8_t width = n ? (TERM_COLS - 4 - 7) / n : 0;
    if (width > 20)
        width = 20;
    if (col >= 4 && width && (col - 4) / width < n)
        focus_machine(nth_machine((col - 4) / width));
}

static void handle(void)
{
    switch (ev.type) {
    case EV_HOME:
        enter_command_mode();
        break;
    case EV_MACHINE_CHANGED: {
        uint8_t h = ev.a;
        info[h].state = ev.b;
        if (h == focus && !live(h)) {
            focus = 0;
            select_machine(h);
        }
        dirty = doc_dirty = 1;
        break;
    }
    case EV_MACHINE_PRESSED:
        if (ev.a != focus)
            focus_machine(ev.a);
        break;
    case EV_KEY_DOWN:
        if (!focus)
            command_key(ev.a, ev.b);
        break;
    case EV_POINTER:
        if (EV_POINTER_BUTTONS(ev) & BUTTON_PRIMARY)
            press(EV_POINTER_X(ev), EV_POINTER_Y(ev));
        break;
    case EV_PAGE_LAID_OUT:
        doc_height = EV_PAGE_HEIGHT(ev);
        break;
    case EV_PAGE_ANCHOR: {
        uint16_t id = EV_ANCHOR_ID(ev);
        if (id >= 100 && id < 100 + prog_n)
            start_program(id - 100);
        else if (id >= 1 && id <= MACH_MAX)
            focus_machine(id);
        break;
    }
    case EV_CONTENT_DONE:
        for (uint8_t i = 0; i < prog_n; i++) {
            if (prog_asked[i] == ev.a && !prog_known[i]) {
                prog_asked[i] = 0;
                if (prog_req[i].status == STATUS_DONE) {
                    memcpy(prog_title[i], prog_hdr[i] + 16, 16);
                    prog_known[i] = 1;
                } else {
                    memcpy(prog_title[i], "?", 2);
                    prog_known[i] = 1;
                }
                dirty = doc_dirty = 1;
            }
        }
        break;
    }
}

int main(void)
{
    term_cmd(TERM_CURSOR_OFF);
    page_background(22, 25, 34, 255);
    while (prog_n < MAX_PROGS && mach_builtin(prog_n, prog_id[prog_n]))
        prog_n++;
    uint8_t last_state[MACH_MAX + 1] = { 0 };
    for (;;) {
        /* Ask for the titles (as many requests at once as the device takes). */
        for (uint8_t i = 0; i < prog_n; i++)
            if (!prog_known[i] && !prog_asked[i])
                prog_asked[i] = content_get(&prog_req[i], prog_id[i], 0, 32, prog_hdr[i]);
        while (terse_event_pop(&ev))
            handle();
        for (uint8_t h = 1; h <= MACH_MAX; h++) {
            mach_info(h, &info[h]);
            if (info[h].state != last_state[h]) {
                last_state[h] = info[h].state;
                prog_ix[h] = prog_of(info[h].image);
                dirty = doc_dirty = 1;
            }
        }
        if (sel && !in_use(sel))
            sel = nth_machine(0);
        if (focus && !in_use(focus))
            focus = 0;
        if (dirty) {
            bar_dirty = 1;
            place_all();
            draw_labels();
            draw_frames();
            dirty = 0;
        }
        show_launcher();
        /* The bar: on every change, and four times a second for the shares
         * and the clock. */
        if (bar_dirty || terse_frame() % 15 == 0)
            draw_bar();
        bar_dirty = 0;
        terse_wait_frame();
    }
}
