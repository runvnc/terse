/* painted-test: a test of technique for a walk-around graphic adventure
 * (CONCEPT.md). The whole picture is the page device's: the scene, painted
 * once in two figures (scene.paths, compiled by scene.py into
 * scene_data.h and submitted from ROM), holds in layers (spec/page.md
 * §page.layers) the traveller's poses, the open door and copies of what
 * stands in front of him; the program only shows, hides, moves and scales
 * layers, and never submits the document again. The terminal's bottom
 * eight rows are the text area: a status line, six lines of replies and a
 * prompt.
 *
 * Arrows walk. Typed commands: look, look tower/rock/mountains, open door,
 * close door, enter, help, phosphor, outline, clean, stats.
 *
 * Depth: the traveller's size follows the height of the feet on the
 * ground (the layer's scale, smaller toward the horizon), and what stands
 * in front of the feet is in layers after his, shown while he is behind
 * it: the boulder and the foot of the tower, the same paths as the
 * scene's. The 2D vector display is used only for the style (phosphor,
 * fill mode). */
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

/* ---- the scene's layers (spec/page.md §page.layers; scene.paths) ---- */

/* The layers of the scene's lower figure: the traveller's poses (actor.py:
 * side poses for strides 0, 12, 20, -12, -20, then toward the viewer and
 * away for 0, 12, -12), the open door, and the copies of what stands in
 * front of him. */
#define LAYER_SIDE 1
#define LAYER_FRONT 6
#define LAYER_BACK 9
#define LAYER_DOOR 12
#define LAYER_ROCK_FRONT 13
#define LAYER_TOWER_FRONT 14
#define LAYER_DOOR_FRONT 15
#define LAYER_COUNT 16
/* Where actor.py painted the feet, in units of the scene; the lower
 * figure, which holds the layers, starts at y 176 (scene.paths, `split
 * 176`). */
#define FEET_X 30
#define FEET_Y 344
#define LOWER_TOP 176

/* What each layer was last set to, so a call is made only for a change. */
static uint8_t shown_flags[LAYER_COUNT];
static int32_t shown_x[LAYER_COUNT], shown_y[LAYER_COUNT], shown_s[LAYER_COUNT];
static uint32_t calls_last; /* layer calls in the last update, for STATS */

static void layer(uint32_t id, uint32_t flags, int32_t x, int32_t y, int32_t scale)
{
    if (!(flags & PAGE_LAYER_SHOWN))
        x = y = 0, scale = PAGE_SCALE_FULL, flags = 0;
    if (shown_flags[id] == flags && shown_x[id] == x && shown_y[id] == y && shown_s[id] == scale)
        return;
    shown_flags[id] = flags;
    shown_x[id] = x;
    shown_y[id] = y;
    shown_s[id] = scale;
    page_layer(id, flags, x, y, scale);
    calls_last++;
}

/* Stride of the walk cycle, quarter units: the front foot's x. */
static const int8_t stride[8] = {0, 12, 20, 12, 0, -12, -20, -12};
static uint8_t pose_now, scale_now;

/* The traveller's size at the feet's height: 256 at the bottom, smaller
 * toward the horizon. */
static int32_t scale_at(int32_t y)
{
    int32_t s = ((y - HORIZON * U) << 8) / ((NEAR - HORIZON) * U);
    return s < 1 ? 1 : s > 256 ? 256 : s;
}

/* Shows the pose for the facing and the stride, the others hidden, placed
 * so that the feet are at (px, py), scaled for depth; shows the open door
 * and the copies of the boulder and the tower in front of him while he
 * stands behind them (in the outline styles nothing is filled, so nothing
 * hides anything: the copies would only add lines). */
static void update_layers(void)
{
    int32_t a = moving ? stride[phase] : 0;
    uint8_t pose;
    if (facing == FACE_LEFT || facing == FACE_RIGHT)
        pose = LAYER_SIDE + (a == 0 ? 0 : a == 12 ? 1 : a == 20 ? 2 : a == -12 ? 3 : 4);
    else
        pose = (facing == FACE_DOWN ? LAYER_FRONT : LAYER_BACK) + (a == 0 ? 0 : a > 0 ? 1 : 2);
    int mirrored = facing == FACE_LEFT || facing == FACE_UP;
    int32_t s = scale_at(py);
    int32_t fx = (FEET_X * U * s) >> 8, fy = ((FEET_Y - LOWER_TOP) * U * s) >> 8;
    int32_t x = mirrored ? px + fx : px - fx;
    int32_t y = py - LOWER_TOP * U - fy;
    calls_last = 0;
    for (uint32_t id = LAYER_SIDE; id < LAYER_DOOR; id++)
        if (id != pose)
            layer(id, 0, 0, 0, 0);
    layer(pose, PAGE_LAYER_SHOWN | (mirrored ? PAGE_LAYER_MIRRORED : 0), x, y, s);
    pose_now = pose;
    scale_now = s > 255 ? 255 : s;

    int32_t half = s * 16 / 256; /* half the figure's width, units */
    int32_t ux = px / U, uy = py / U;
    int behind_rock = uy < ROCK_BASE && ux + half > ROCK_X0 && ux - half < ROCK_X1;
    int behind_tower = uy < TOWER_BASE && ux + half > 412 && ux - half < 468;
    int clean = look_style == 0;
    layer(LAYER_DOOR, door_open ? PAGE_LAYER_SHOWN : 0, 0, 0, PAGE_SCALE_FULL);
    layer(LAYER_ROCK_FRONT, behind_rock && clean ? PAGE_LAYER_SHOWN : 0, 0, 0, PAGE_SCALE_FULL);
    layer(LAYER_TOWER_FRONT, behind_tower && clean ? PAGE_LAYER_SHOWN : 0, 0, 0, PAGE_SCALE_FULL);
    layer(LAYER_DOOR_FRONT, behind_tower && clean && door_open ? PAGE_LAYER_SHOWN : 0, 0, 0, PAGE_SCALE_FULL);
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
        char m[80] = "page layer ";
        put_num(m + slen(m), pose_now);
        memcpy(m + slen(m), " at scale ", 11);
        put_num(m + slen(m), scale_now);
        memcpy(m + slen(m), "/256; layer calls last move ", 29);
        put_num(m + slen(m), calls_last);
        memcpy(m + slen(m), "; no vector list", 17);
        say(m, C_TEXT);
    } else if (is(cmd, "help") || is(cmd, "?")) {
        say("Arrows walk. Try LOOK, LOOK TOWER, OPEN DOOR (at the door), ENTER. PHOSPHOR, OUTLINE and CLEAN"
            " change how the scene is drawn; STATS shows the traveller's layer and scale.",
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
    /* Every layer is shown after reset: the program shows only its own. */
    for (uint32_t id = 1; id < LAYER_COUNT; id++) {
        page_layer(id, 0, 0, 0, PAGE_SCALE_FULL);
        shown_s[id] = PAGE_SCALE_FULL;
    }

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
            update_layers();
        }
        terse_wait_frame();
    }
}
