/* shell: starts the host's built-in programs as machines of their own and
 * lays their screens out as tiles (device 8, SPEC §machines); built with clang
 * (see build.sh).
 *
 * Row 0 is a bar: the home segment, one segment per machine (handle, a
 * state mark, title, units used in its last frame as a share of the
 * budget) and the clock (UTC). The machines are tiled below it: one zoomed
 * to the whole area, a master on the left half with the others stacked on
 * the right, or a grid; each picture at the top of its tile. The focused machine has a blue frame, drawn with
 * the vector display in the gap around its picture. The fourth layout,
 * the gallery, stands the machines' screens on a ring in the 3D display
 * (device 9 Screens, SPEC §vector3d.list), each in a wireframe box on a stand over a
 * faint ground grid, the selected one at the front on the glass; the ring
 * turns (eased over 20 frames) to bring another to the front, and the
 * machine with the focus is faced (SPEC §machines.calls face): its panel flies flat
 * into the area below the bar and is shown there as a zoomed tile; the
 * home key flies it back.
 *
 * The host's home key gives the focus back to the shell: command mode. A
 * launcher on the page lists "Manual" (Enter starts the built-in reader
 * with the manual's index, manual_id.h, as its argument), the built-in
 * programs (the cursor starts on the first), then the programs of
 * the feeds the user follows (content op 0Dh: each name with its
 * publisher's name and short key, and a mark while its image is not yet
 * downloaded), then the machines; the tiles move right to make room for
 * it. Programs are started by their identity (start_program: a built-in
 * by the zero key and its title), so their named records follow them from
 * one version to the next. Keys in command mode: Up and Down move the
 * cursor, Enter starts the program under it (and focuses it) or focuses
 * the machine under it, Tab or Right and Shift+Tab or Left
 * select the next or previous machine (in the gallery, turning the ring),
 * Esc returns the focus to the selected machine, X stops it, P pauses or resumes it, Z zooms it (or
 * goes back to the layout before), L cycles the layouts, 1-8 select a
 * machine by handle. A press on a machine's picture (or panel) or its bar
 * segment focuses it (in the gallery a bar segment turns the ring to it, in
 * command mode); one on the home segment enters command mode. The layout
 * chosen with L and Z is kept in the named record "layout" (SPEC §content.records).
 *
 * Open requests (SPEC §system `open`, 12.2 `request`): a machine may ask the
 * shell to open a program or a blob. The shell takes one request at a
 * time (the others wait, so their machines' `open` returns 0) and only
 * from the machine that has the focus, at most one a second from each;
 * after a refusal that machine's requests are dropped for ten seconds.
 * A program identity: a built-in (zero key) starts at once; another is
 * started paused and kept out of the layout until it has loaded (the host
 * looks it up on its relays if need be), then the bar asks "<title> asks
 * to start <name> by <publisher> <short key>.", "Y/N" at its right end
 * (a long question is cut before it; the keyboard goes to
 * the shell meanwhile); Y runs it in a new tile with the focus, N or Esc
 * stops it. A blob: the shell gets its first 86 bytes. An image ("TERS")
 * is asked about as an unpublished program (its title and short ID) and
 * started by its ID; a document ("TDOC") starts its format's program
 * with the blob as its argument (a built-in at once; another program the
 * first time its identity is asked for in this run, asked about as above,
 * then without asking); anything else starts the built-in reader with the
 * blob as its argument. Failures (not found, no free handle) are a brief
 * message in the bar.
 *
 * Sessions (SPEC §machines.snapshots): S in command mode keeps every running or paused
 * machine as a snapshot, in slots 0, 1, ... in handle order, and then the
 * record "session": version 1, layout, before_zoom, the slot of the
 * selected machine (FFh: none) and the number of machines; the bar says
 * when it is done or failed. When the shell starts with a session record,
 * the launcher's first entry is "Restore last session (N programs)": Enter
 * restores the machines in their order (new handles, so the same tiles),
 * the layout and the focus; D discards the session (its slots and the
 * record). A machine that cannot be restored shows why, like a failed
 * start. */

#include <stdint.h>
#include "terse.h"
#include "manual_id.h"

#define BUDGET 125000u
#define MAX_PROGS 16
#define MAX_FEED_PROGS 16 /* programs of the followed feeds shown */
#define MAX_NAMES 16
#define AREA_Y 20 /* the tiles' area: below the bar and a gap */
#define AREA_H (480 - AREA_Y)
#define GAP 6     /* between tiles; the focus frame is drawn in it */
#define PANEL_W 200 /* the launcher beside the tiles in command mode */
#define G_CX 450    /* the gallery (see there) */
#define G_CY 250
#define G_GROUND 430
#define G_HW 152
#define G_HH 114
#define G_DEPTH 16

enum { ZOOM, MASTER, GRID, GALLERY, LAYOUTS };

#define KEY_D 0x07
#define KEY_S 0x16
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

/* The programs of the followed feeds (op 0Dh), by feed key then name. */
typedef struct {
    uint8_t key[32];
    uint8_t len, local;
    uint8_t name[48];
} feed_prog;
static feed_prog fprog[MAX_FEED_PROGS];
static uint8_t fprog_n;
static uint8_t progs_buf[MAX_FEED_PROGS * 160];
static content_req progs_req;
static uint8_t progs_handle, progs_again = 1;

/* The publishers' names (op 09h). */
typedef struct {
    uint8_t key[32];
    uint8_t len;
    uint8_t name[48];
} feed_name;
static feed_name names[MAX_NAMES];
static uint8_t names_n;
static uint8_t names_buf[MAX_NAMES * 83];
static content_req names_req;
static uint8_t names_handle, names_again = 1;

/* The layout, kept in the record "layout": layout, before_zoom. */
static content_req layout_req, layout_save_req;
static uint8_t layout_buf[2], layout_handle;

/* The session (record "session": version, layout, before_zoom, focused
 * slot, count). sess_n: machines of the session offered in the launcher
 * (0: none); save_left: snapshots of a save not yet done. */
#define SESSION_LEN 5
static content_req sess_req, sess_save_req, sess_del_req;
static uint8_t sess_buf[SESSION_LEN], sess_handle, sess_save_handle, sess_n;
static uint8_t save_left, save_bad;

/* Each machine's title (from its image's header, for a machine that is not
 * a built-in) and the name it was started under. */
static uint8_t mtitle[MACH_MAX + 1][17], mtitle_state[MACH_MAX + 1], mtitle_handle[MACH_MAX + 1];
static uint8_t mtitle_hdr[MACH_MAX + 1][32];
static content_req mtitle_req[MACH_MAX + 1];
static uint8_t mname[MACH_MAX + 1][17];

static mach_record info[MACH_MAX + 1]; /* by handle; [0] unused */
static rect tile[MACH_MAX + 1], pic[MACH_MAX + 1];
static uint8_t focus;        /* 0: the shell (command mode) */
static uint8_t sel;          /* the selected machine, 0 if none */
/* Command mode: the session (if offered), Manual, built-ins, feed
 * programs, then machines; it starts on the first built-in. */
static uint8_t cursor = 1;
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

/* --- Open requests ------------------------------------------------------ */

enum { OJ_IDLE, OJ_GET, OJ_LOAD, OJ_ASK };
#define REQ_GAP 60      /* frames between two requests of one machine */
#define REFUSED_GAP 600 /* frames a machine's requests are dropped after a no */
#define HDR_LEN (TDOC_HEAD + 48)
#define MAX_FORMATS 8
#define KEY_N 0x11
#define KEY_Y 0x1C
static uint8_t open_wait;              /* bit h - 1: a request of machine h waits */
static uint32_t open_next[MACH_MAX + 1]; /* the frame from which h's requests are taken */
static uint8_t oj, oj_from, oj_h;      /* the request in hand, its machine, the one started */
static uint8_t oj_req[82], oj_hdr[HDR_LEN];
static uint8_t oj_doc;                 /* it opens a document of a published format */
static content_req oj_creq;
static uint8_t oj_chandle;
static uint8_t question[200];
static uint8_t q_n;
/* A brief message in the bar, until frame msg_until. */
static uint8_t bmsg[80], bmsg_n;
static uint32_t msg_until;
/* The published formats the user allowed in this run: key, name. */
static uint8_t fmt_key[MAX_FORMATS][32], fmt_name[MAX_FORMATS][48], fmt_len[MAX_FORMATS], fmt_n;

static uint8_t in_use(uint8_t h) { return info[h].state != MACH_NONE; }
/* A machine waiting for the user's answer is in use but not shown. */
static uint8_t shown(uint8_t h) { return in_use(h) && !(h == oj_h && (oj == OJ_LOAD || oj == OJ_ASK)); }
static uint8_t live(uint8_t h)
{
    uint8_t s = info[h].state;
    return s == MACH_LOADING || s == MACH_RUNNING || s == MACH_PAUSED;
}

static uint8_t machine_count(void)
{
    uint8_t n = 0;
    for (uint8_t h = 1; h <= MACH_MAX; h++)
        n += shown(h);
    return n;
}

/* The nth machine in use (0-based), 0 if none. */
static uint8_t nth_machine(uint8_t n)
{
    for (uint8_t h = 1; h <= MACH_MAX; h++)
        if (shown(h) && n-- == 0)
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

/* The launcher's first entry restores the session, if one is offered;
 * then Manual (at top()), then the built-ins from first(). */
static uint8_t top(void) { return sess_n != 0; }
static uint8_t first(void) { return top() + 1; }

/* The entries the launcher lists before the machines. */
static uint8_t nprogs(void) { return first() + prog_n + fprog_n; }

/* A machine's title: a built-in's, else its image's, else the name it was
 * started under. */
static const uint8_t *title_of(uint8_t h)
{
    static const uint8_t unknown[] = "?";
    uint8_t p = prog_ix[h];
    if (p < MAX_PROGS && prog_known[p])
        return prog_title[p];
    if (mtitle_state[h] == 2 && mtitle[h][0])
        return mtitle[h];
    return mname[h][0] ? mname[h] : unknown;
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
 * as the host does it (SPEC §machines.screens). */
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

static uint8_t g_front(void);

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
        if (shown(h)) {
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
        pic[h] = r.w && r.h ? fit(r) : r;
        if (layout == GALLERY) {
            /* Panels, not tiles; the front one's frame holds its label. */
            rect f = { G_CX - G_HW, G_CY - G_HH, 2 * G_HW, 2 * G_HH }, z = { 0, 0, 0, 0 };
            r = shown(h) && h == g_front() ? f : z;
            pic[h] = z;
        }
        tile[h] = r;
        if (layout == GALLERY)
            r = pic[h];
        mach_place(h, r.x, r.y, r.w, r.h);
    }
}

/* --- The gallery ---------------------------------------------------------- */

/* Units: the ring's centre axis at (G_CX, *, radius) so the front panel is
 * on the glass, 2 G_HW wide about (G_CX, G_CY); the ground at G_GROUND. */
static const uint16_t g_radius[MACH_MAX + 1] = { 0, 0, 200, 210, 250, 300, 350, 405, 460 };
/* Smoothstep x 256 at t / 20. */
static const uint8_t g_ease[20] = { 0, 2, 7, 16, 27, 40, 55, 72, 90, 109, 128, 147, 166, 184, 201, 216, 229, 240, 249, 254 };
static uint8_t v3list[2048];
static uint16_t ring, ring_from, ring_to;
static uint8_t ring_on;                     /* ring_to is set */
static int16_t ring_delta;
static uint8_t ring_t = 20;                 /* frames into a turn; 20: none */
static uint8_t g_faced;                     /* the machine asked to be faced */
static uint8_t g_dirty = 1;                 /* the 3D list must be built again */
static uint16_t g_rot[MACH_MAX], g_len;     /* where each panel's angle is in v3list; its length */
static uint16_t g_face[MACH_MAX];           /* where each panel's own turn is */

/* A panel at angle a on the ring turns back by half of it: it faces
 * outward, towards the viewer's side. */
static uint16_t g_back(uint16_t a) { return (uint16_t)(-(int16_t)a / 2); }

/* The machine at the front: the selected one, else the first. */
static uint8_t g_front(void) { return sel && shown(sel) ? sel : nth_machine(0); }

static uint8_t has_picture(uint8_t h)
{
    uint8_t s = info[h].state;
    return s >= MACH_RUNNING && s <= MACH_FAULTED;
}

/* A box about the panel: its edges on the screen's, a back G_DEPTH behind,
 * the corners joined; then a stand to the ground and a cross at its foot.
 * Bit 7 of a step: move there instead of drawing a line. */
static const int8_t box_x[4] = { -1, 1, 1, -1 }, box_y[4] = { -1, -1, 1, 1 };
static const uint8_t box_path[] = { 0x80, 1, 2, 3, 0, 4, 5, 6, 7, 4, 0x81, 5, 0x82, 6, 0x83, 7 };

static void g_panel(uint8_t h, uint8_t i, uint8_t n)
{
    uint16_t r = g_radius[n];
    v3_push();
    v3_translate(G_CX * 16, G_CY * 16, r * 16);
    uint16_t a = ring + (uint16_t)(i * (65536u / n));
    v3_rotate(V3_AXIS_Y, a);
    g_rot[i] = v3_ptr - v3list - 2;
    v3_translate(0, 0, -r * 16);
    v3_rotate(V3_AXIS_Y, g_back(a));
    g_face[i] = v3_ptr - v3list - 2;
    if (h == focus)
        v3_colour(70, 150, 255, 255);
    else if (h == g_front())
        v3_colour(230, 180, 60, 255);
    else
        v3_colour(120, 130, 150, 255);
    v3_width(32);
    for (uint8_t k = 0; k < sizeof box_path; k++) {
        uint8_t b = box_path[k], c = b & 3;
        int16_t x = box_x[c] * G_HW * 16, y = box_y[c] * G_HH * 16, z = b & 4 ? G_DEPTH * 16 : 0;
        if (b & 0x80)
            v3_move(x, y, z);
        else
            v3_line(x, y, z);
    }
    int16_t foot = (G_GROUND - G_CY) * 16, mid = G_DEPTH * 8;
    v3_move(0, G_HH * 16, mid);
    v3_line(0, foot, mid);
    v3_move(-960, foot, mid);
    v3_line(960, foot, mid);
    v3_move(0, foot, mid - 640);
    v3_line(0, foot, mid + 640);
    v3_screen(h, 2 * G_HW * 16);
    v3_pop();
}

/* The ring: the ground grid, then each machine's panel. */
static void g_build(void)
{
    uint8_t n = machine_count();
    if (!n) {
        v3_clear();
        return;
    }
    int16_t zmax = 2 * g_radius[n] + 300, g = G_GROUND * 16;
    v3_begin(v3list, sizeof v3list);
    v3_colour(50, 70, 100, 255);
    v3_width(16);
    for (int16_t z = -64; z <= zmax; z += 128) {
        v3_move((G_CX - 256) * 16, g, z * 16);
        v3_line((G_CX + 512) * 16, g, z * 16);
    }
    for (int16_t x = G_CX - 256; x <= G_CX + 512; x += 128) {
        v3_move(x * 16, g, -64 * 16);
        v3_line(x * 16, g, zmax * 16);
    }
    for (uint8_t i = 0; i < n; i++)
        g_panel(nth_machine(i), i, n);
    v3_end();
    g_len = v3_length(v3list);
    v3_submit(v3list, g_len);
}

/* Only the ring turned: each panel's angle in the list, submitted again. */
static void g_turn(void)
{
    uint8_t n = machine_count();
    for (uint8_t i = 0; i < n; i++) {
        uint16_t a = ring + (uint16_t)(i * (65536u / n)), b = g_back(a);
        v3list[g_rot[i]] = a;
        v3list[g_rot[i] + 1] = a >> 8;
        v3list[g_face[i]] = b;
        v3list[g_face[i] + 1] = b >> 8;
    }
    if (n)
        v3_submit(v3list, g_len);
}

/* Once a frame: the ring turns towards the front machine, the list is
 * built if anything changed, and the machine with the focus is faced (or
 * the view returns) . */
static void gallery(void)
{
    if (layout != GALLERY) {
        if (g_faced)
            mach_unface(0);
        if (ring_on)
            v3_clear();
        g_faced = ring_on = 0;
        g_dirty = 1;
        return;
    }
    uint8_t n = machine_count(), f = g_front(), at = 0;
    for (uint8_t i = 0; i < n; i++)
        if (nth_machine(i) == f)
            at = i; /* the front one's place on the ring */
    uint16_t to = n ? -(uint16_t)(at * (65536u / n)) : 0;
    if (to != ring_to || !ring_on) {
        ring_from = ring;
        ring_delta = (int16_t)(to - ring);
        ring_t = ring_on ? 0 : 19; /* the first time: at once */
        ring_to = to;
        ring_on = 1;
    }
    uint8_t turned = ring_t < 20;
    if (turned) {
        ring_t++;
        ring = ring_t == 20 ? ring_to : ring_from + (int16_t)(ring_delta * g_ease[ring_t] / 256);
    }
    if (g_dirty)
        g_build();
    else if (turned)
        g_turn();
    g_dirty = 0;
    uint8_t want = focus && has_picture(focus) ? focus : 0;
    if (want == g_faced)
        return;
    if (!want) {
        mach_unface(20);
        g_faced = 0;
        return;
    }
    rect area = make(0, AREA_Y, 640, 480), p = fit(area);
    if (mach_face(want, 20, p.x, area.y, p.w, p.h))
        g_faced = want;
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
    if (oj == OJ_ASK || bmsg_n) {
        /* The question or a message instead of the machines. */
        uint8_t fg = oj == OJ_ASK ? SEL_FG : FOCUS_FG, bg = oj == OJ_ASK ? SEL_BG : FOCUS_BG;
        uint8_t w = TERM_COLS - 4 - 7;
        if (oj == OJ_ASK)
            w = TERM_COLS - 4; /* over the clock too */
        for (uint8_t i = 0; i < w; i++)
            bar[col + i] = cell(' ', fg, bg);
        if (oj == OJ_ASK) {
            /* "Y/N" always shows, at the right; a long question is cut. */
            if (bar_text(col + 1, w - 6, question, q_n, fg, bg) == w - 6)
                bar[col + w - 6] = cell(0x2026, fg, bg);
            bar_text(col + w - 4, 3, (const uint8_t *)"Y/N", 3, fg, bg);
        } else {
            bar_text(col + 1, w - 2, bmsg, bmsg_n, fg, bg);
        }
        n = 0;
    }
    for (uint8_t h = 1; h <= MACH_MAX && n; h++) {
        if (!shown(h))
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
    if (oj == OJ_ASK) {
        term_cells_write(0, bar, TERM_COLS);
        return;
    }
    bar[c] = cell(' ', BAR_FG, BAR_BG);
    bar[c + 1] = cell('0' + hr / 10, BAR_FG, BAR_BG);
    bar[c + 2] = cell('0' + hr % 10, BAR_FG, BAR_BG);
    bar[c + 3] = cell(':', BAR_FG, BAR_BG);
    bar[c + 4] = cell('0' + m / 10, BAR_FG, BAR_BG);
    bar[c + 5] = cell('0' + m % 10, BAR_FG, BAR_BG);
    term_cells_write(0, bar, TERM_COLS);
}

/* Why a start failed (SPEC §machines.calls), in words. */
static const char *failure_name(uint8_t f)
{
    static const char *const names[] = { "could not start", "not found", "not a valid image",
                                         "needs a device this host does not have", "no room for another machine",
                                         "no usable snapshot" };
    return f < 6 ? names[f] : names[0];
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
        /* Two lines, each cut to the tile's width: the handle and title,
         * then what is happening. */
        uint8_t row = (tile[h].y + tile[h].h / 2) / 16, col = tile[h].x / 8 + 1;
        uint8_t room = (tile[h].x + tile[h].w) / 8 > col + 1 ? (tile[h].x + tile[h].w) / 8 - col - 1 : 1;
        term_goto(col, row);
        term_color(s == MACH_FAILED ? 9 : 244, 0);
        term_putc('0' + h);
        if (room > 2) {
            term_putc(' ');
            term_put_ascii(room - 2 < 16 ? room - 2 : 16, title_of(h));
        }
        term_goto(col, row + 1);
        term_put_ascii(room, (const uint8_t *)(s == MACH_FAILED ? failure_name(info[h].failure) : "loading"));
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
    vec_begin(vlist, sizeof vlist);
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

/* The first and last six hex digits of a key, with an ellipsis between. */
static void doc_short_key(const uint8_t *key)
{
    static const char hex[] = "0123456789abcdef";
    char s[15];
    for (uint8_t i = 0; i < 3; i++) {
        s[2 * i] = hex[key[i] >> 4];
        s[2 * i + 1] = hex[key[i] & 15];
        s[9 + 2 * i] = hex[key[29 + i] >> 4];
        s[10 + 2 * i] = hex[key[29 + i] & 15];
    }
    s[6] = (char)0xE2; /* … */
    s[7] = (char)0x80;
    s[8] = (char)0xA6;
    doc_text(s, 15);
}

/* The publisher's name, if the names records give one. */
static const feed_name *name_of(const uint8_t *key)
{
    for (uint8_t i = 0; i < names_n; i++)
        if (!memcmp(names[i].key, key, 32))
            return &names[i];
    return 0;
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
    if (sess_n) {
        line(0, 300);
        doc_str("Restore last session (");
        doc_num(sess_n, 0);
        doc_str(sess_n == 1 ? " program)" : " programs)");
    }
    line(top(), 301);
    doc_str("Manual");
    for (uint8_t i = 0; i < prog_n; i++) {
        line(first() + i, 100 + i);
        if (prog_known[i])
            doc_text(prog_title[i], title_len(prog_title[i]));
        else
            doc_str("\xE2\x80\xA6");
    }
    if (fprog_n) {
        doc_anchor(0);
        doc_font(DOC_SANS, DOC_BOLD);
        doc_size(13);
        doc_colour(150, 160, 180);
        doc_paragraph(DOC_START, 6, 14, 0);
        doc_str("FROM YOUR FEEDS");
        doc_font(DOC_SANS, 0);
        for (uint8_t i = 0; i < fprog_n; i++) {
            feed_prog *f = &fprog[i];
            doc_size(15);
            line(first() + prog_n + i, 200 + i);
            doc_text(f->name, f->len);
            if (!f->local) {
                doc_colour(150, 160, 180);
                doc_str(" \xE2\x86\x93"); /* ↓: not downloaded yet */
            }
            /* The publisher: its name and short key. */
            doc_paragraph(DOC_START, 18, 0, DOC_NOWRAP | DOC_ELLIPSIS);
            doc_size(11);
            doc_colour(130, 138, 155);
            const feed_name *n = name_of(f->key);
            if (n && n->len) {
                doc_text(n->name, n->len);
                doc_str(" ");
            }
            doc_short_key(f->key);
        }
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
            line(nprogs() + i, h);
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
            if (r->state == MACH_FAILED) {
                doc_paragraph(DOC_START, 24, 0, 0);
                doc_colour(255, 120, 110);
                doc_str(failure_name(r->failure));
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
                "\xC2\xB7 Z zoom \xC2\xB7 L layout \xC2\xB7 1-8 select \xC2\xB7 S save session"
              : "Up, Down choose \xC2\xB7 Enter start \xC2\xB7 the home key comes back here");
    if (sess_n)
        doc_str(" \xC2\xB7 D discards the session");
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
            cursor = nprogs() + i;
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

static void set_note(const char *m)
{
    uint8_t i = 0;
    while ((note[i] = m[i]))
        i++;
    doc_dirty = 1;
}

/* Starts program `name` of feed `key` (all zero: a built-in, by title) by
 * its identity. */
static void start_named(const uint8_t *key, const uint8_t *name, uint8_t len)
{
    uint32_t h = mach_start_named(key, name, len);
    note[0] = 0;
    if (!h || h > MACH_MAX) {
        set_note(h ? "Not a program name." : "No free handle: stop a machine first (X).");
        return;
    }
    info[h].state = MACH_LOADING;
    memset(info[h].image, 0, 32);
    uint8_t n = len < 16 ? len : 16;
    memcpy(mname[h], name, n);
    mname[h][n] = 0;
    mtitle_state[h] = 0;
    focus_machine(h);
}

/* A built-in by its identity (the zero key and its title); while its
 * title is not known (or could not be had), by its image ID. */
static void start_program(uint8_t p)
{
    static const uint8_t zero[32];
    if (prog_known[p] && prog_title[p][0] != '?') {
        start_named(zero, prog_title[p], title_len(prog_title[p]));
        return;
    }
    uint8_t h = mach_start(prog_id[p]);
    note[0] = 0;
    if (!h) {
        set_note("No free handle: stop a machine first (X).");
        return;
    }
    info[h].state = MACH_LOADING;
    memcpy(info[h].image, prog_id[p], 32);
    mname[h][0] = 0;
    mtitle_state[h] = 0;
    focus_machine(h);
}

static void start_feed_program(uint8_t i)
{
    start_named(fprog[i].key, fprog[i].name, fprog[i].len);
}


/* --- Open requests (the policy is in the header comment) ---------------- */

static void q_add(const uint8_t *p, uint32_t n)
{
    while (n-- && q_n < sizeof question)
        question[q_n++] = *p++;
}

static void q_str(const char *s)
{
    while (*s && q_n < sizeof question)
        question[q_n++] = (uint8_t)*s++;
}

/* The first and last six hex digits of 32 bytes, an ellipsis between. */
static void q_short(const uint8_t *key)
{
    static const char hex[] = "0123456789abcdef";
    for (uint8_t i = 0; i < 3; i++) {
        uint8_t h[2] = { (uint8_t)hex[key[i] >> 4], (uint8_t)hex[key[i] & 15] };
        q_add(h, 2);
    }
    q_str("\xE2\x80\xA6");
    for (uint8_t i = 29; i < 32; i++) {
        uint8_t h[2] = { (uint8_t)hex[key[i] >> 4], (uint8_t)hex[key[i] & 15] };
        q_add(h, 2);
    }
}

/* A message in the bar for three seconds. */
static void bar_msg(const char *a, const uint8_t *b, uint32_t bn)
{
    bmsg_n = 0;
    while (*a && bmsg_n < sizeof bmsg)
        bmsg[bmsg_n++] = (uint8_t)*a++;
    while (bn-- && *b && bmsg_n < sizeof bmsg)
        bmsg[bmsg_n++] = *b++;
    msg_until = terse_frame() + 180;
    bar_dirty = 1;
}

/* Starts `name` of feed `key` by its identity, with the blob `arg` (if
 * any) as its argument; paused (and out of the layout) unless `run`.
 * Returns the handle, 0 (with a message) if none is free. */
static uint8_t open_start(const uint8_t *key, const uint8_t *name, uint8_t len, const uint8_t *arg, uint8_t run)
{
    uint32_t h = mach_start_named(key, name, len);
    if (!h || h > MACH_MAX) {
        bar_msg(h ? "Not a program name" : "No free handle: stop a machine first", 0, 0);
        return 0;
    }
    if (arg)
        mach_argument(h, arg);
    info[h].state = MACH_LOADING;
    memset(info[h].image, 0, 32);
    uint8_t n = len < 16 ? len : 16;
    memcpy(mname[h], name, n);
    mname[h][n] = 0;
    mtitle_state[h] = 0;
    if (run) {
        focus_machine(h);
    } else {
        mach_run(h, 0);
        oj_h = h;
        oj = OJ_LOAD;
    }
    return h;
}

/* Puts the question about the program the request names (a key and name
 * at oj_req + 1, or an image's title) in the bar; the keyboard comes to
 * the shell until it is answered. */
static void ask(void)
{
    q_n = 0;
    const uint8_t *t = title_of(oj_from);
    q_add(t, title_len(t));
    if (oj_req[0] == OPEN_BLOB && oj_hdr[0] == 'T' && oj_hdr[1] == 'E') {
        q_str(" asks to start an unpublished program ");
        q_add(oj_hdr + 16, title_len(oj_hdr + 16));
        q_str(" (");
        q_short(oj_req + 1);
        q_str(").");
    } else {
        const uint8_t *key = oj_doc ? oj_hdr + 5 : oj_req + 1;
        const uint8_t *name = oj_doc ? oj_hdr + TDOC_HEAD : oj_req + 34;
        q_str(" asks to start ");
        q_add(name, oj_doc ? oj_hdr[37] : oj_req[33]);
        q_str(" by ");
        const feed_name *n = name_of(key);
        if (n && n->len) {
            q_add(n->name, n->len);
            q_str(" ");
        }
        q_short(key);
        q_str(oj_doc ? " to open a document." : ".");
    }
    oj = OJ_ASK;
    mach_focus(0);
    dirty = bar_dirty = 1;
}

static void open_done(void)
{
    oj = OJ_IDLE;
    oj_h = 0;
    dirty = doc_dirty = bar_dirty = 1;
}

/* The answer to the question (or Esc, the home key, a press elsewhere). */
static void answer(uint8_t yes)
{
    if (oj != OJ_ASK)
        return;
    if (!yes) {
        open_next[oj_from] = terse_frame() + REFUSED_GAP;
        if (oj_h) {
            mach_stop(oj_h);
            info[oj_h].state = MACH_NONE;
        }
        uint8_t back = oj_from;
        open_done();
        focus_machine(live(back) ? back : 0);
        return;
    }
    if (oj_doc && fmt_n < MAX_FORMATS) {
        memcpy(fmt_key[fmt_n], oj_hdr + 5, 32);
        fmt_len[fmt_n] = oj_hdr[37];
        memcpy(fmt_name[fmt_n++], oj_hdr + TDOC_HEAD, oj_hdr[37]);
    }
    if (oj_h) {
        uint8_t h = oj_h;
        mach_run(h, 1);
        open_done();
        focus_machine(h);
        return;
    }
    /* An unpublished image: started now, by its ID. */
    open_done();
    uint8_t h = mach_start(oj_req + 1);
    if (!h) {
        bar_msg("No free handle: stop a machine first", 0, 0);
        return;
    }
    info[h].state = MACH_LOADING;
    memcpy(info[h].image, oj_req + 1, 32);
    mname[h][0] = 0;
    mtitle_state[h] = 0;
    focus_machine(h);
}

/* Whether the user allowed the document format at oj_hdr in this run. */
static uint8_t format_allowed(void)
{
    for (uint8_t i = 0; i < fmt_n; i++)
        if (!memcmp(fmt_key[i], oj_hdr + 5, 32) && fmt_len[i] == oj_hdr[37]
            && !memcmp(fmt_name[i], oj_hdr + TDOC_HEAD, fmt_len[i]))
            return 1;
    return 0;
}

/* The first bytes of the requested blob arrived. */
static void blob_head(void)
{
    static const uint8_t zero[32];
    const uint8_t *id = oj_req + 1;
    oj = OJ_IDLE;
    if (oj_creq.status != STATUS_DONE) {
        bar_msg(oj_creq.status == STATUS_NOT_FOUND ? "Could not open: not found" : "Could not open: the fetch failed", 0, 0);
        return;
    }
    uint32_t n = oj_creq.length;
    if (n >= 32 && !memcmp(oj_hdr, "TERS", 4)) {
        oj_h = 0;
        oj_doc = 0;
        ask();
        return;
    }
    if (n >= TDOC_HEAD + 1 && !memcmp(oj_hdr, "TDOC", 4) && !oj_hdr[4] && oj_hdr[37] && oj_hdr[37] <= 48
        && n >= TDOC_HEAD + oj_hdr[37]) {
        const uint8_t *name = oj_hdr + TDOC_HEAD;
        if (!memcmp(oj_hdr + 5, zero, 32)) {
            open_start(zero, name, oj_hdr[37], id, 1);
            return;
        }
        oj_doc = 1;
        uint8_t ok = format_allowed();
        if (open_start(oj_hdr + 5, name, oj_hdr[37], id, ok) && !ok)
            return; /* asked once it has loaded */
        oj_doc = 0;
        return;
    }
    open_start(zero, (const uint8_t *)"reader", 6, id, 1);
}

/* The Manual entry: the built-in reader with the manual's index (which
 * every host has in its store) as its argument. */
static void open_manual(void)
{
    static const uint8_t zero[32];
    note[0] = 0;
    doc_dirty = 1;
    open_start(zero, (const uint8_t *)"reader", 6, manual_index, 1);
}

/* Takes the next waiting request, if the shell is free for it. */
static void take_request(void)
{
    static const uint8_t zero[32];
    if (oj != OJ_IDLE || !open_wait)
        return;
    uint8_t h = 1;
    while (!(open_wait >> (h - 1) & 1))
        h++;
    open_wait &= ~(1 << (h - 1));
    if (!mach_request(h, oj_req))
        return;
    /* Only from the machine with the focus, at most one a second. */
    uint32_t now = terse_frame();
    if (h != focus || (int32_t)(now - open_next[h]) < 0)
        return;
    open_next[h] = now + REQ_GAP;
    oj_from = h;
    oj_h = 0;
    oj_doc = 0;
    if (oj_req[0] == OPEN_PROGRAM) {
        uint8_t builtin = !memcmp(oj_req + 1, zero, 32);
        open_start(oj_req + 1, oj_req + 34, oj_req[33], 0, builtin);
        return;
    }
    if ((oj_chandle = content_get(&oj_creq, oj_req + 1, 0, HDR_LEN, oj_hdr)))
        oj = OJ_GET;
    else
        bar_msg("Could not open: the shell is busy", 0, 0);
}

/* A machine changed state: the one started for a request has loaded (ask
 * about it) or could not be had. */
static void open_changed(uint8_t h, uint8_t state)
{
    if (oj != OJ_LOAD || h != oj_h)
        return;
    if (state == MACH_PAUSED) {
        ask();
        return;
    }
    mach_stop(h);
    info[h].state = MACH_NONE;
    open_done();
    bar_msg("Could not start ", oj_doc ? oj_hdr + TDOC_HEAD : oj_req + 34, oj_doc ? oj_hdr[37] : oj_req[33]);
}

/* Keeps the layout for the next run. */
static void save_layout(void)
{
    layout_buf[0] = layout;
    layout_buf[1] = before_zoom;
    record_save(&layout_save_req, "layout", layout_buf, 2);
}

/* The session entry goes (the cursor stays on the same line; from the
 * entry itself, it goes to the first built-in). */
static void drop_session_entry(void)
{
    if (sess_n)
        cursor = cursor ? cursor - 1 : 1;
    sess_n = 0;
    doc_dirty = 1;
}

/* S: every running or paused machine into slots 0, 1, ...; the record
 * follows once all are stored. */
static void save_session(void)
{
    uint8_t n = 0, f = 0xFF;
    if (save_left)
        return;
    for (uint8_t h = 1; h <= MACH_MAX; h++) {
        uint8_t s = info[h].state;
        if (shown(h) && (s == MACH_RUNNING || s == MACH_PAUSED) && mach_snapshot(h, n)) {
            if (h == sel)
                f = n;
            n++;
        }
    }
    if (!n) {
        bar_msg("No programs to save", 0, 0);
        return;
    }
    sess_buf[0] = 1;
    sess_buf[1] = layout;
    sess_buf[2] = before_zoom;
    sess_buf[3] = f;
    sess_buf[4] = n;
    save_left = n;
    save_bad = 0;
    drop_session_entry();
}

static void snapshot_done(uint8_t status)
{
    if (!save_left)
        return;
    save_bad |= status != 1;
    if (--save_left)
        return;
    if (save_bad) {
        bar_msg("Could not save the session", 0, 0);
        return;
    }
    /* Slots past the session's are not needed any more. */
    for (uint8_t k = sess_buf[4]; k < MACH_MAX; k++)
        mach_snapshot_delete(k);
    sess_save_handle = record_save(&sess_save_req, "session", sess_buf, SESSION_LEN);
}

/* Enter on the session entry: its machines in their order, then its
 * layout and focus. */
static void restore_session(void)
{
    uint8_t n = sess_n, fh = 0;
    mach_snapshot_record si;
    drop_session_entry();
    for (uint8_t k = 0; k < n; k++) {
        uint32_t h = mach_restore(k);
        if (!h) {
            bar_msg("No free handle: stop a machine first", 0, 0);
            break;
        }
        info[h].state = MACH_LOADING;
        memset(info[h].image, 0, 32);
        uint8_t len = mach_snapshot_info(k, &si) && si.name_len ? si.name_len : 0;
        if (len > 16)
            len = 16;
        memcpy(mname[h], si.name, len);
        mname[h][len] = 0;
        mtitle_state[h] = 0;
        if (k == sess_buf[3])
            fh = h;
    }
    layout = sess_buf[1];
    before_zoom = sess_buf[2];
    if (fh)
        focus_machine(fh);
    dirty = 1;
}

/* D on the session entry: its slots and the record go. */
static void discard_session(void)
{
    for (uint8_t k = 0; k < sess_n; k++)
        mach_snapshot_delete(k);
    record_delete(&sess_del_req, "session");
    drop_session_entry();
    bar_msg("Session discarded", 0, 0);
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
    uint8_t items = nprogs() + machine_count();
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
        if (cursor < top())
            restore_session();
        else if (cursor == top())
            open_manual();
        else if (cursor < first() + prog_n)
            start_program(cursor - first());
        else if (cursor < nprogs())
            start_feed_program(cursor - first() - prog_n);
        else
            focus_machine(nth_machine(cursor - nprogs()));
        return;
    case KEY_S:
        save_session();
        return;
    case KEY_D:
        if (cursor < top())
            discard_session();
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
            if (cursor >= nprogs() + machine_count())
                cursor = nprogs() + machine_count() - 1;
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
        save_layout();
        dirty = 1;
        return;
    case KEY_L:
        layout = (layout + 1) % LAYOUTS;
        if (layout != ZOOM)
            before_zoom = layout;
        save_layout();
        dirty = 1;
        return;
    default:
        if (key >= KEY_1 && key <= KEY_8 && in_use(key - KEY_1 + 1))
            select_machine(key - KEY_1 + 1);
        return;
    }
    /* The cursor moved: a machine under it is the selected one. */
    if (cursor >= nprogs())
        sel = nth_machine(cursor - nprogs());
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
    if (col >= 4 && width && (col - 4) / width < n) {
        uint8_t h = nth_machine((col - 4) / width);
        if (layout != GALLERY) {
            focus_machine(h);
            return;
        }
        /* The gallery: back to the ring, turned to it. */
        if (focus) {
            enter_command_mode();
            mach_focus(0);
        }
        select_machine(h);
    }
}

/* The programs records (timeline records, SPEC §content.feeds) into fprog. */
static void take_programs(void)
{
    uint32_t at = 0, len = progs_req.length;
    fprog_n = 0;
    while (at + 2 + 109 <= len && fprog_n < MAX_FEED_PROGS) {
        const uint8_t *r = progs_buf + at;
        const uint8_t *e = r + 2;
        uint8_t n = e[108];
        feed_prog *f = &fprog[fprog_n++];
        memcpy(f->key, e, 32);
        f->len = n > 48 ? 48 : n;
        memcpy(f->name, e + 109, f->len);
        f->local = r[1] & TIMELINE_LOCAL;
        at += 1 + r[0];
    }
    if (cursor >= nprogs() + machine_count() && cursor)
        cursor = nprogs() + machine_count() - 1;
}

/* The names records into names. */
static void take_names(void)
{
    uint32_t at = 0, len = names_req.length;
    names_n = 0;
    while (at + 2 + 32 <= len && names_n < MAX_NAMES) {
        const uint8_t *r = names_buf + at;
        feed_name *n = &names[names_n++];
        memcpy(n->key, r + 2, 32);
        n->len = r[0] - 33 > 48 ? 48 : r[0] - 33;
        memcpy(n->name, r + 34, n->len);
        at += 1 + r[0];
    }
}

static void handle(void)
{
    switch (ev.type) {
    case EV_HOME:
        answer(0);
        enter_command_mode();
        break;
    case EV_OPEN_REQUESTED:
        if (ev.a >= 1 && ev.a <= MACH_MAX)
            open_wait |= 1 << (ev.a - 1);
        break;
    case EV_MACHINE_CHANGED: {
        uint8_t h = ev.a;
        info[h].state = ev.b;
        open_changed(h, ev.b);
        if (h == focus && !live(h)) {
            focus = 0;
            select_machine(h);
        }
        dirty = doc_dirty = 1;
        break;
    }
    case EV_MACHINE_PRESSED:
        answer(0);
        if (ev.a != focus)
            focus_machine(ev.a);
        break;
    case EV_KEY_DOWN:
        if (oj == OJ_ASK) {
            if (ev.a == KEY_Y || ev.a == KEY_N || ev.a == KEY_ESCAPE)
                answer(ev.a == KEY_Y);
        } else if (!focus) {
            command_key(ev.a, ev.b);
        }
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
        if (id == 300 && sess_n)
            restore_session();
        else if (id == 301)
            open_manual();
        else if (id >= 100 && id < 100 + prog_n)
            start_program(id - 100);
        else if (id >= 200 && id < 200 + fprog_n)
            start_feed_program(id - 200);
        else if (id >= 1 && id <= MACH_MAX)
            focus_machine(id);
        break;
    }
    case EV_FEEDS_UPDATED:
        progs_again = names_again = 1;
        break;
    case EV_SNAPSHOT_DONE:
        snapshot_done(ev.b);
        break;
    case EV_CONTENT_DONE:
        if (oj == OJ_GET && ev.a == oj_chandle && oj_creq.status != STATUS_PENDING) {
            oj_chandle = 0;
            blob_head();
        } else if (ev.a == progs_handle && progs_req.status != STATUS_PENDING) {
            progs_handle = 0;
            if (progs_req.status == STATUS_DONE)
                take_programs();
            dirty = doc_dirty = 1;
        } else if (ev.a == names_handle && names_req.status != STATUS_PENDING) {
            names_handle = 0;
            if (names_req.status == STATUS_DONE)
                take_names();
            doc_dirty = 1;
        } else if (ev.a == sess_handle && sess_req.status != STATUS_PENDING) {
            sess_handle = 0;
            uint8_t *b = sess_buf;
            if (sess_req.status == STATUS_DONE && sess_req.length == SESSION_LEN && b[0] == 1 && b[1] < LAYOUTS
                && b[2] < LAYOUTS && b[4] && b[4] <= MACH_MAX) {
                sess_n = b[4];
                cursor = 0; /* on the session entry */
                doc_dirty = 1;
            }
        } else if (ev.a == sess_save_handle && sess_save_req.status != STATUS_PENDING) {
            sess_save_handle = 0;
            bar_msg(sess_save_req.status == STATUS_DONE ? "Session saved" : "Could not save the session", 0, 0);
        } else if (ev.a == layout_handle && layout_req.status != STATUS_PENDING) {
            layout_handle = 0;
            if (layout_req.status == STATUS_DONE && layout_req.length == 2 && layout_buf[0] < LAYOUTS
                && layout_buf[1] < LAYOUTS && layout_buf[1] != ZOOM) {
                layout = layout_buf[0];
                before_zoom = layout_buf[1];
                dirty = 1;
            }
        }
        for (uint8_t h = 1; h <= MACH_MAX; h++) {
            if (mtitle_state[h] == 1 && mtitle_handle[h] == ev.a && mtitle_req[h].status != STATUS_PENDING) {
                mtitle_state[h] = 2;
                mtitle[h][0] = 0;
                if (mtitle_req[h].status == STATUS_DONE && mtitle_req[h].length == 32) {
                    memcpy(mtitle[h], mtitle_hdr[h] + 16, 16);
                    mtitle[h][16] = 0;
                }
                dirty = doc_dirty = 1;
            }
        }
        for (uint8_t i = 0; i < prog_n; i++) {
            /* A handle may have been given to a newer request in the same
             * frame: only a request that is no longer pending is done. */
            if (prog_asked[i] == ev.a && !prog_known[i] && prog_req[i].status != STATUS_PENDING) {
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
    /* The layout kept from the last run, applied when it arrives. */
    layout_handle = record_load(&layout_req, "layout", 0, 2, layout_buf);
    /* A session kept from an earlier run is offered in the launcher. */
    sess_handle = record_load(&sess_req, "session", 0, SESSION_LEN, sess_buf);
    uint8_t last_state[MACH_MAX + 1] = { 0 };
    for (;;) {
        /* The programs of the followed feeds and their publishers' names,
         * again after every feeds-updated event. */
        if (progs_again && !progs_handle && (progs_handle = content_programs(&progs_req, 0, sizeof progs_buf, progs_buf)))
            progs_again = 0;
        if (names_again && !names_handle && (names_handle = content_names(&names_req, 0, sizeof names_buf, names_buf)))
            names_again = 0;
        /* The title of each machine that is not a built-in, from its
         * image's header. */
        for (uint8_t h = 1; h <= MACH_MAX; h++) {
            if (mtitle_state[h] == 0 && info[h].state >= MACH_RUNNING && info[h].state <= MACH_FAULTED
                && prog_ix[h] == MAX_PROGS
                && (mtitle_handle[h] = content_get(&mtitle_req[h], info[h].image, 0, 32, mtitle_hdr[h])))
                mtitle_state[h] = 1;
        }
        /* Ask for the titles (as many requests at once as the device takes). */
        for (uint8_t i = 0; i < prog_n; i++)
            if (!prog_known[i] && !prog_asked[i])
                prog_asked[i] = content_get(&prog_req[i], prog_id[i], 0, 32, prog_hdr[i]);
        while (terse_event_pop(&ev))
            handle();
        take_request();
        if (bmsg_n && (int32_t)(terse_frame() - msg_until) >= 0) {
            bmsg_n = 0;
            bar_dirty = 1;
        }
        for (uint8_t h = 1; h <= MACH_MAX; h++) {
            mach_info(h, &info[h]);
            if (info[h].state != last_state[h]) {
                if (info[h].state == MACH_NONE)
                    mtitle_state[h] = 0;
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
            g_dirty = 1;
        }
        gallery();
        show_launcher();
        /* The bar: on every change, and four times a second for the shares
         * and the clock. */
        if (bar_dirty || terse_frame() % 15 == 0)
            draw_bar();
        bar_dirty = 0;
        terse_wait_frame();
    }
}
