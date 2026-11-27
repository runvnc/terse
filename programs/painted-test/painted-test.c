/* painted-test: a test of technique for a walk-around graphic adventure
 * (CONCEPT.md). The scene is painted once, as one figure of the page
 * device (scene.paths, compiled by scene.py into scene_data.h and
 * submitted from ROM); the traveller, the boulder and the occluders are
 * the 2D vector display's, rebuilt only when something moves; the
 * terminal's bottom eight rows are the text area: a status line, six
 * lines of replies and a prompt.
 *
 * Arrows walk. Typed commands: look, look tower/rock/mountains, open door,
 * close door, enter, help, phosphor, outline, clean, stats.
 *
 * Depth: the traveller's size follows the height of the feet on the
 * ground (smaller toward the horizon), and anything that stands in front
 * of the feet is drawn again after the traveller: the boulder (a vector
 * shape anyway) and the foot of the tower (the page's art, repeated in
 * vector shapes only while the traveller is behind it). */
#include "terse.h"
#include "scene_data.h"

#define U 16 /* sixteenths per screen unit */

/* The picture: the page's rectangle, the top 22 rows of the screen. */
#define PIC_H 352
#define ROW_STATUS 22
#define ROW_LOG 23
#define LOG_ROWS 6
#define ROW_PROMPT 29

/* Terminal palette entries set at start (term_palette_set). */
#define C_PANEL 16
#define C_TEXT 17
#define C_DIM 18
#define C_ACCENT 19
#define C_BAR 20
#define C_BARTEXT 21

/* The ground the feet may stand on, in units; the horizon where a figure
 * would shrink to nothing; the nearest feet, at full size. */
#define GROUND_TOP 226
#define GROUND_BOTTOM 346
#define GROUND_LEFT 10
#define GROUND_RIGHT 630
#define HORIZON 150
#define NEAR 346

/* The tower's foot (its base line at y 262) and the boulder's, in units:
 * the feet may not stand inside either box; feet above the base line and
 * a figure across the object put it behind. */
#define TOWER_X0 404
#define TOWER_X1 476
#define TOWER_Y0 250
#define TOWER_Y1 272
#define TOWER_BASE 262
#define ROCK_X0 88
#define ROCK_X1 192
#define ROCK_Y0 316
#define ROCK_Y1 342
#define ROCK_BASE 330

/* Where one stands to reach the door. */
#define DOOR_X 440
#define DOOR_Y0 268
#define DOOR_Y1 296

enum { FACE_DOWN, FACE_UP, FACE_LEFT, FACE_RIGHT };
enum { K_LEFT = 1, K_RIGHT = 2, K_UP = 4, K_DOWN = 8 };

static int32_t px = 300 * U, py = 318 * U; /* the feet, in sixteenths */
static uint8_t facing = FACE_DOWN, keys, phase, tick, moving;
static uint8_t door_open, look_style; /* look_style: 0 clean, 1 phosphor, 2 outline */
static uint8_t dirty = 1;
static uint8_t list[2048];
static uint32_t list_len;

/* ---- drawing the traveller: local coordinates in quarter units, feet at
 * (0, 0), y up negative, x toward the facing; s256 the scale ---- */

static int32_t fx, fy, s256, mir;

static int16_t X(int32_t lx) { return (int16_t)(fx + ((mir * lx * s256) >> 6)); }
static int16_t Y(int32_t ly) { return (int16_t)(fy + ((ly * s256) >> 6)); }
static void width_q(int32_t w)
{
    int32_t v = (w * s256) >> 6;
    vec_width((uint8_t)(v < 1 ? 1 : v > 255 ? 255 : v));
}
static void col(uint32_t rgb, uint8_t a) { vec_colour(rgb >> 16, rgb >> 8, rgb, a); }
static void seg(int32_t x0, int32_t y0, int32_t x1, int32_t y1)
{
    vec_move(X(x0), Y(y0));
    vec_line(X(x1), Y(y1));
}
static void tri(int32_t x0, int32_t y0, int32_t x1, int32_t y1, int32_t x2, int32_t y2)
{
    vec_move(X(x0), Y(y0));
    vec_tri(X(x1), Y(y1), X(x2), Y(y2));
}
static void dot(int32_t x, int32_t y, int32_t r)
{
    int32_t v = (r * s256) >> 6;
    vec_move(X(x), Y(y));
    vec_disc((uint16_t)(v < 1 ? 1 : v));
}
static void box(int32_t x0, int32_t y0, int32_t x1, int32_t y1)
{
    vec_move(X(x0), Y(y0));
    vec_rect(X(x1), Y(y1));
}

#define CLOAK 0x2e7a78
#define CLOAK_DARK 0x215c5a
#define HAT 0x3b2f6e
#define GOLD 0xe0b84a
#define SKIN 0xeac6a0
#define BEARD 0xe6e2da
#define BOOTS 0x3a2a20
#define STAFF 0x8a5a2c

/* Stride of the walk cycle, quarter units: the front foot's x. */
static const int8_t stride[8] = {0, 12, 20, 12, 0, -12, -20, -12};

static void traveller(void)
{
    int32_t a = moving ? stride[phase] : 0;
    int32_t bob = (a == 20 || a == -20 || a == 0) ? 0 : -4;
    int side = facing == FACE_LEFT || facing == FACE_RIGHT;

    fx = px;
    fy = py;
    s256 = ((py - HORIZON * U) << 8) / ((NEAR - HORIZON) * U);
    mir = facing == FACE_LEFT || facing == FACE_UP ? -1 : 1;

    /* Shadow: a fat line is a capsule, the nearest thing to an ellipse. */
    col(0x000000, 70);
    width_q(26);
    seg(-36, 0, 36, 0);

    /* Legs. */
    col(BOOTS, 255);
    width_q(15);
    if (side) {
        seg(0, -88 + bob, -a, a < 0 ? -6 : 0);
        seg(0, -88 + bob, a, a > 0 ? -6 : 0);
    } else {
        seg(-10, -88 + bob, -12, a > 0 ? -10 : 0);
        seg(10, -88 + bob, 12, a < 0 ? -10 : 0);
    }
    fy += (bob * s256) >> 6;

    /* Cloak, with its shaded side. The width is that of the outlines
     * filled shapes become in the phosphor style and fill mode 1. */
    vec_width(U);
    col(facing == FACE_UP ? CLOAK_DARK : CLOAK, 255);
    tri(0, -188, -50, -56, 48, -56);
    col(CLOAK_DARK, 255);
    if (side)
        tri(0, -188, -50, -56, -18, -56);
    else
        tri(0, -188, 22, -56, 48, -56);
    col(GOLD, 255);
    box(-24, -117, 24, -109);

    /* Arm and staff, swinging a little with the stride. */
    int32_t sx = side ? 44 - a / 2 : 58;
    col(STAFF, 255);
    width_q(9);
    seg(sx - 2, -214, sx + 2, -2);
    col(0xffe58a, 60);
    dot(sx - 2, -218, 26);
    col(0xfff2b8, 255);
    dot(sx - 2, -218, 10);
    col(CLOAK_DARK, 255);
    width_q(15);
    seg(side ? 6 : 26, -160, sx - 6, -124);
    col(SKIN, 255);
    vec_width(U);
    dot(sx - 2, -124, 8);

    /* Head: a face seen from the front or the side, the hat from behind. */
    if (facing == FACE_UP) {
        col(0xb9b4aa, 255);
        dot(0, -202, 22);
    } else {
        col(SKIN, 255);
        dot(side ? 4 : 0, -204, 22);
        col(BEARD, 255);
        if (side)
            tri(-6, -198, 24, -198, 12, -156);
        else
            tri(-18, -198, 18, -198, 0, -150);
        col(0x2a2030, 255);
        if (side)
            dot(17, -210, 3);
        else {
            dot(-8, -210, 3);
            dot(8, -210, 3);
        }
    }
    col(HAT, 255);
    width_q(12);
    seg(-42, -220, 44, -220);
    if (side)
        tri(-26, -220, 30, -220, -16, -294);
    else
        tri(-27, -220, 27, -220, 6, -298);
    col(GOLD, 255);
    width_q(6);
    seg(-23, -228, 24, -228);
}

/* ---- the scenery in the vector display: screen units ---- */

static void ux_rect(int32_t x0, int32_t y0, int32_t x1, int32_t y1)
{
    vec_move(x0 * U, y0 * U);
    vec_rect(x1 * U, y1 * U);
}
static void ux_disc(int32_t x, int32_t y, int32_t r)
{
    vec_move(x * U, y * U);
    vec_disc(r * U);
}
static void ux_line(int32_t x0, int32_t y0, int32_t x1, int32_t y1)
{
    vec_move(x0 * U, y0 * U);
    vec_line(x1 * U, y1 * U);
}

/* The boulder: low-poly facets about a point inside it, lit from the
 * left as the tower is. */
static const int16_t rock_pts[9][2] = {
    {92, 334}, {98, 312}, {114, 294}, {136, 284}, {160, 288}, {178, 302}, {188, 320}, {186, 336}, {140, 340},
};
static const uint32_t rock_tone[9] = {
    0x8a8f99, 0xa3a8b2, 0xb8bcc4, 0xa7acb5, 0x8f949e, 0x737882, 0x5f646e, 0x6a6f79, 0x7c818b,
};

static void rock_shadow(void)
{
    col(0x000000, 56);
    vec_width(12 * U);
    ux_line(104, 337, 186, 338);
}

static void rock(void)
{
    vec_width(U);
    for (int i = 0; i < 9; i++) {
        int j = (i + 1) % 9;
        col(rock_tone[i], 255);
        vec_move(138 * U, 316 * U);
        vec_tri(rock_pts[i][0] * U, rock_pts[i][1] * U, rock_pts[j][0] * U, rock_pts[j][1] * U);
    }
    col(0x55606a, 120);
    vec_width(U);
    ux_line(120, 300, 132, 306);
}

/* The open door: the dark inside and the lamplight spilling out. */
static void door_overlay(void)
{
    col(0x2a1c14, 255);
    ux_rect(428, 234, 452, 265);
    ux_disc(440, 234, 12);
    col(0xffcf73, 150);
    ux_rect(431, 244, 449, 265);
    ux_disc(440, 244, 9);
    col(0xffd88a, 48);
    vec_move(430 * U, 267 * U);
    vec_tri(450 * U, 267 * U, 462 * U, 292 * U);
    vec_move(430 * U, 267 * U);
    vec_tri(462 * U, 292 * U, 418 * U, 292 * U);
}

/* The foot of the tower again, as the page paints it (scene.paths), from
 * y 180 down to its base: drawn after the traveller who stands behind it. */
static void tower_occluder(void)
{
    col(0xc9bda9, 255);
    ux_rect(412, 180, 468, 262);
    col(0xddd3c1, 255);
    ux_rect(416, 180, 430, 263);
    col(0xa39784, 255);
    ux_rect(454, 180, 468, 262);
    col(0x8d826f, 255);
    ux_rect(462, 180, 468, 262);
    col(0xab9f8a, 255);
    vec_width(U);
    ux_line(412, 210, 468, 210);
    ux_line(412, 245, 468, 245);
    ux_line(436, 180, 436, 210);
    ux_line(424, 210, 424, 245);
    ux_line(456, 210, 456, 245);
    /* The small window on the curve of the wall. */
    col(0x4a3626, 255);
    ux_rect(453, 194, 461, 204);
    ux_disc(457, 194, 4);
    col(0xf5bd4f, 255);
    ux_rect(455, 194, 459, 202);
    ux_disc(457, 194, 2);
    col(0x7e7362, 255);
    ux_rect(425, 234, 455, 265);
    ux_disc(440, 234, 15);
    if (door_open) {
        door_overlay();
        return;
    }
    col(0x7a4a2a, 255);
    ux_rect(428, 234, 452, 265);
    ux_disc(440, 234, 12);
    col(0x5c361d, 255);
    ux_line(434, 224, 434, 265);
    ux_line(440, 222, 440, 265);
    ux_line(446, 224, 446, 265);
    col(0xe2c060, 255);
    vec_move(447 * U + 8, 246 * U);
    vec_disc(26);
}

static void build_list(void)
{
    int32_t half = (((py - HORIZON * U) << 8) / ((NEAR - HORIZON) * U)) * 16 / 256; /* half the figure's width, units */
    int32_t x = px / U, y = py / U;
    int behind_rock = y < ROCK_BASE && x + half > ROCK_X0 && x - half < ROCK_X1;
    int behind_tower = y < TOWER_BASE && x + half > 412 && x - half < 468;

    vec_begin(list, sizeof list);
    vec_limit--;
    if (door_open)
        door_overlay();
    rock_shadow();
    if (!behind_rock)
        rock();
    traveller();
    if (behind_rock)
        rock();
    /* In the outline styles nothing is filled, so nothing hides anything:
     * the occluder would only add lines. */
    if (behind_tower && look_style == 0)
        tower_occluder();
    vec_limit++;
    vec_end();
    list_len = vec_length(list);
    vec_submit(list, list_len);
}

/* ---- walking ---- */

static int walkable(int32_t x, int32_t y)
{
    x /= U;
    y /= U;
    if (y < GROUND_TOP || y > GROUND_BOTTOM || x < GROUND_LEFT || x > GROUND_RIGHT)
        return 0;
    if (x > TOWER_X0 && x < TOWER_X1 && y > TOWER_Y0 && y < TOWER_Y1)
        return 0;
    if (x > ROCK_X0 && x < ROCK_X1 && y > ROCK_Y0 && y < ROCK_Y1)
        return 0;
    return 1;
}

static void walk(void)
{
    int dx = !!(keys & K_RIGHT) - !!(keys & K_LEFT);
    int dy = !!(keys & K_DOWN) - !!(keys & K_UP);
    if (!dx && !dy) {
        if (moving) {
            moving = 0;
            phase = 0;
            dirty = 1;
        }
        return;
    }
    int32_t s = ((py - HORIZON * U) << 8) / ((NEAR - HORIZON) * U);
    /* Sixteenths a frame: slower far away, but not in proportion (the
     * far ground would take too long to cross). */
    int32_t sx = 12 + 10 * s / 256, sy = 8 + 5 * s / 256;
    uint8_t f = dx > 0 ? FACE_RIGHT : dx < 0 ? FACE_LEFT : dy > 0 ? FACE_DOWN : FACE_UP;
    if (f != facing)
        dirty = 1;
    facing = f;
    if (dx && walkable(px + dx * sx, py))
        px += dx * sx, dirty = 1;
    if (dy && walkable(px, py + dy * sy))
        py += dy * sy, dirty = 1;
    moving = 1;
    if (++tick >= 5) {
        tick = 0;
        phase = (phase + 1) & 7;
        dirty = 1;
    }
}

/* ---- the text area ---- */

static char log_text[LOG_ROWS][80];
static uint8_t log_colour[LOG_ROWS];
static char input[64];
static uint8_t input_len;

static uint32_t slen(const char *s)
{
    uint32_t n = 0;
    while (s[n])
        n++;
    return n;
}

static void redraw_prompt(void)
{
    term_goto(0, ROW_PROMPT);
    term_color(C_ACCENT, C_PANEL);
    term_puts(" > ");
    term_color(C_TEXT, C_PANEL);
    term_write(input, input_len);
    term_cmd(4);
    term_goto(3 + input_len, ROW_PROMPT);
}

static void redraw_log(void)
{
    for (int i = 0; i < LOG_ROWS; i++) {
        term_goto(0, ROW_LOG + i);
        term_color(log_colour[i], C_PANEL);
        term_putc(' ');
        term_puts(log_text[i]);
        term_cmd(4);
    }
    redraw_prompt();
}

static void log_line(const char *s, uint32_t n, uint8_t colour)
{
    for (int i = 0; i < LOG_ROWS - 1; i++) {
        memcpy(log_text[i], log_text[i + 1], 80);
        log_colour[i] = log_colour[i + 1];
    }
    if (n > 78)
        n = 78;
    memcpy(log_text[LOG_ROWS - 1], s, n);
    log_text[LOG_ROWS - 1][n] = 0;
    log_colour[LOG_ROWS - 1] = colour;
}

/* Adds a reply, wrapped at 76 columns between words. */
static void say(const char *s, uint8_t colour)
{
    uint32_t n = slen(s);
    while (n) {
        uint32_t take = n;
        if (take > 76) {
            take = 76;
            while (take && s[take] != ' ')
                take--;
            if (!take)
                take = 76;
        }
        log_line(s, take, colour);
        s += take;
        n -= take;
        while (n && *s == ' ')
            s++, n--;
    }
}

static void status_bar(void)
{
    static const char *names[3] = {"clean", "phosphor", "outline"};
    term_goto(0, ROW_STATUS);
    term_color(C_BARTEXT, C_BAR);
    term_puts("  THE STILLWIND SPIRE  \xc2\xb7  a test of technique");
    term_cmd(4);
    term_goto(48, ROW_STATUS);
    term_color(C_DIM, C_BAR);
    term_puts("arrows walk  \xc2\xb7  style ");
    term_puts(names[look_style]);
    redraw_prompt();
}

static void set_style(uint8_t s)
{
    look_style = s;
    vec_style(s == 1 ? VEC_STYLE_PHOSPHOR : VEC_STYLE_CLEAN);
    vec_fill(s == 2 ? VEC_FILL_OFF : VEC_FILL_ON);
    dirty = 1;
    status_bar();
}

static int near_door(void)
{
    int32_t x = px / U, y = py / U;
    return x > DOOR_X - 26 && x < DOOR_X + 26 && y >= DOOR_Y0 && y <= DOOR_Y1;
}

static int is(const char *a, const char *b)
{
    while (*a && *a == *b)
        a++, b++;
    return *a == *b;
}

static void put_num(char *p, uint32_t v)
{
    char t[12];
    int n = 0;
    do
        t[n++] = '0' + v % 10;
    while (v /= 10);
    while (n)
        *p++ = t[--n];
    *p = 0;
}

static void command(void)
{
    char cmd[64];
    uint32_t n = 0;
    /* Lower case, single spaces, no spaces at the ends. */
    for (uint32_t i = 0; i < input_len; i++) {
        char c = input[i];
        if (c >= 'A' && c <= 'Z')
            c += 32;
        if (c == ' ' && (n == 0 || cmd[n - 1] == ' '))
            continue;
        cmd[n++] = c;
    }
    while (n && cmd[n - 1] == ' ')
        n--;
    cmd[n] = 0;
    if (!n)
        return;
    char echo[70] = "> ";
    memcpy(echo + 2, cmd, n + 1);
    say(echo, C_DIM);

    if (is(cmd, "look") || is(cmd, "l")) {
        say(door_open ? "A stone tower stands alone in the meadow, its door open on a warm light. Behind it the mountains"
                        " are going blue with the evening, and not a blade of grass stirs."
                      : "A stone tower stands alone in the meadow, its roof a dark cone against the dusk. Behind it the"
                        " mountains are going blue, and not a blade of grass stirs. A path leads to a small arched door.",
            C_TEXT);
    } else if (is(cmd, "look tower") || is(cmd, "examine tower") || is(cmd, "x tower")) {
        say("Old grey stone, well laid. A pennant hangs from the spike of the roof, limp, though up there it"
            " ought to be snapping in the wind.",
            C_TEXT);
    } else if (is(cmd, "look rock") || is(cmd, "look boulder") || is(cmd, "x rock")) {
        say("A boulder as tall as your shoulder, split into flat faces like a cut stone. Someone has scratched"
            " an arrow on it, pointing at the tower.",
            C_TEXT);
    } else if (is(cmd, "look mountains") || is(cmd, "look sun")) {
        say("The sun sits in a notch of the range and has sat there, they say in the valley, since spring.", C_TEXT);
    } else if (is(cmd, "open door") || is(cmd, "open")) {
        if (!near_door())
            say("You are too far from the door. Walk up the path to it.", C_TEXT);
        else if (door_open)
            say("It is already open.", C_TEXT);
        else {
            door_open = 1;
            dirty = 1;
            say("The door swings inward without a sound. Lamplight, and a stair going up.", C_ACCENT);
        }
    } else if (is(cmd, "close door") || is(cmd, "close")) {
        if (!near_door())
            say("You are too far from the door.", C_TEXT);
        else if (!door_open)
            say("It is closed.", C_TEXT);
        else {
            door_open = 0;
            dirty = 1;
            say("You pull the door to.", C_TEXT);
        }
    } else if (is(cmd, "enter") || is(cmd, "go in") || is(cmd, "in") || is(cmd, "climb stair")) {
        say(door_open && near_door() ? "The stair is for another day: this is only a test of the scene."
                                     : "The way in is through the door.",
            C_TEXT);
    } else if (is(cmd, "phosphor")) {
        set_style(1);
        say("The scene as a vector tube draws it: every fill an outline, on black.", C_TEXT);
    } else if (is(cmd, "outline")) {
        set_style(2);
        say("Fill mode 1: the same outlines, on the clean style's backgrounds.", C_TEXT);
    } else if (is(cmd, "clean")) {
        set_style(0);
        say("Filled again.", C_TEXT);
    } else if (is(cmd, "stats")) {
        char m[80] = "vector list ";
        put_num(m + slen(m), list_len);
        memcpy(m + slen(m), " bytes, ink ", 13);
        put_num(m + slen(m), vec_list_ink(list, list_len, 0));
        memcpy(m + slen(m), " filled / ", 11);
        put_num(m + slen(m), vec_list_ink(list, list_len, 1));
        memcpy(m + slen(m), " outlined of 230400", 20);
        say(m, C_TEXT);
    } else if (is(cmd, "help") || is(cmd, "?")) {
        say("Arrows walk. Try LOOK, LOOK TOWER, OPEN DOOR (at the door), ENTER. PHOSPHOR, OUTLINE and CLEAN"
            " change how the scene is drawn; STATS shows the vector list's size and ink.",
            C_TEXT);
    } else {
        say("Nothing happens. (Try LOOK, OPEN DOOR or HELP.)", C_TEXT);
    }
    redraw_log();
}

static void key(uint8_t code, int down)
{
    uint8_t bit = code == KEY_LEFT ? K_LEFT : code == KEY_RIGHT ? K_RIGHT : code == KEY_UP ? K_UP : code == KEY_DOWN ? K_DOWN : 0;
    if (bit) {
        if (down)
            keys |= bit;
        else
            keys &= ~bit;
        return;
    }
    if (!down)
        return;
    if (code == KEY_ENTER) {
        command();
        input_len = 0;
        redraw_prompt();
    } else if (code == KEY_BACKSPACE && input_len) {
        input_len--;
        redraw_prompt();
    }
}

static const uint8_t palette[6][3] = {
    {0x16, 0x18, 0x26}, /* panel */
    {0xec, 0xe2, 0xc8}, /* text */
    {0x8a, 0x8c, 0xa6}, /* dim */
    {0xf2, 0xc4, 0x6a}, /* accent */
    {0x2c, 0x25, 0x46}, /* status bar */
    {0xec, 0xe2, 0xc8}, /* status text */
};

int main(void)
{
    term_palette_set(C_PANEL, palette, 6);
    for (int r = ROW_STATUS; r < 30; r++) {
        term_goto(0, r);
        term_color(C_TEXT, C_PANEL);
        term_cmd(4);
    }

    page_region(0, 0, 640, PIC_H);
    page_background(0x26, 0x38, 0x6c, 255);
    page_submit(scene_doc, sizeof scene_doc);

    say("The path ends at a tower. Arrows walk; type a command and press Enter (HELP for a list).", C_TEXT);
    status_bar();
    redraw_log();

    for (;;) {
        terse_event e;
        while (terse_event_pop(&e) != EV_NONE) {
            if (e.type == EV_KEY_DOWN || e.type == EV_KEY_UP)
                key(e.a, e.type == EV_KEY_DOWN);
            else if (e.type == EV_TEXT) {
                uint32_t cp = EV_TEXT_CP(e);
                if (cp >= 32 && cp < 127 && input_len < 60) {
                    input[input_len++] = (char)cp;
                    redraw_prompt();
                }
            }
        }
        walk();
        if (dirty) {
            dirty = 0;
            build_list();
        }
        terse_wait_frame();
    }
}
