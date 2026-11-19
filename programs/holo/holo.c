/* holo: a few scenes on the 3D vector display (device 9, SPEC 13): a
 * rotating cube and icosahedron labelled on the glass, nested rotating
 * rings, a starfield receding in depth, a small orrery, a turning city
 * of solid blocks on a ground grid, and a cockpit whose consoles show parts
 * of the terminal (Surfaces: the program's own 2D layers on panels in its
 * scene, with a list on one that can be clicked). The host does
 * every rotation and the projection (it chooses the eye: move the pointer
 * to look around the models); the program only changes angles, so it does
 * no trigonometry and stays tiny. The terminal (device 2) has the labels
 * and the key hints, on the glass in front of the models.
 *
 * Coordinates are sixteenths of a screen unit: x right, y down, z away
 * from the viewer, 0 at the glass. Angles are 1/65,536 of a turn.
 *
 * Keys: Space (or Tab) next scene, 1-6 a scene, arrows turn the model (in
 * the cockpit: bank and pitch the craft), A stops and starts the turning,
 * R resets the view, P switches between the clean and the phosphor display
 * style, H makes the wireframes solid (their faces hide what is behind
 * them) and wire again. In the cockpit a click on a row of the right-hand
 * console's list selects it: the host delivers a pointer over a Surface as
 * a pointer inside the rectangle it shows, so the program does no geometry.
 *
 * Solids: faces (v3_face3, v3_face4) give no light but hide the lines
 * behind them, as the host's eye sees them; a solid is its edges and its
 * faces, given with the same corners. */

#include <stdint.h>
#include "terse.h"

#define U 16 /* sixteenths per screen unit */

#define KEY_A 0x04
#define KEY_H 0x0B
#define KEY_P 0x13
#define KEY_R 0x15
#define KEY_1 0x1E

enum { WIREFRAMES, RINGS, STARFIELD, ORRERY, CITY, COCKPIT, SCENES };

static const char *const names[SCENES] = { "WIREFRAMES", "RINGS", "STARFIELD", "ORRERY", "CITY", "COCKPIT" };

static uint8_t list[8192];
static uint8_t scene, phosphor, still, solid, dirty = 1;
static uint8_t buttons;      /* of the last pointer event */
static uint8_t selected = 2; /* the cockpit list's selected row */
static uint8_t key_left, key_right, key_up, key_down;
static uint32_t t;       /* frames the models have turned */
static uint16_t yaw, pitch; /* the user's turn of the model */

static void consoles_text(void);

/* --- Text on the glass ---------------------------------------------------- */

static void put_at(uint8_t col, uint8_t row, uint8_t fg, const char *s)
{
    term_goto(col, row);
    term_color(fg, 0);
    term_puts(s);
}

static uint8_t len(const char *s)
{
    uint8_t n = 0;
    while (s[n])
        n++;
    return n;
}

/* `s` centred on screen x = `x` units. */
static void label(uint16_t x, uint8_t row, uint8_t fg, const char *s)
{
    put_at(x / 8 - len(s) / 2, row, fg, s);
}

static void text(void)
{
    term_color(7, 0);
    term_cmd(TERM_CLEAR);
    put_at(1, 0, 15, "HOLO");
    uint8_t col = 7;
    for (uint8_t i = 0; i < SCENES; i++) {
        char n[3] = { '1' + i, ' ', 0 };
        put_at(col, 0, i == scene ? 11 : 8, n);
        put_at(col + 2, 0, i == scene ? 15 : 8, names[i]);
        col += 4 + len(names[i]);
    }
    put_at(1, 29, 8, "Space next  1-6 scene  arrows turn  A stop/turn  R reset  P phosphor  H solid");
    if (phosphor)
        put_at(71, 0, 10, "PHOSPHOR");
    if (scene == WIREFRAMES) {
        label(190, 24, 14, "CUBE");
        label(190, 25, 8, "8 corners, 12 edges");
        label(450, 24, 11, "ICOSAHEDRON");
        label(450, 25, 8, "12 corners, 30 edges");
        label(320, 27, solid ? 10 : 8, solid ? "solid: the faces hide what is behind them" : "wire");
    } else if (scene == RINGS) {
        label(320, 27, 13, "six rings, each turning in the one outside it");
    } else if (scene == STARFIELD) {
        label(320, 26, 12, "384 points receding from the glass");
    } else if (scene == ORRERY) {
        label(320, 27, 14, "an orrery: four planets on tilted orbits, two moons");
    } else if (scene == CITY) {
        label(320, 27, 10, "solid blocks hiding one another and the ground");
    } else {
        consoles_text();
    }
}

/* --- Models ---------------------------------------------------------------- */

/* The glass: a dim frame at z = 0, where the terminal's text is. */
static void glass(void)
{
    v3_colour(40, 70, 110, 255);
    v3_width(12);
    v3_move(12 * U, 24 * U, 0);
    v3_line(628 * U, 24 * U, 0);
    v3_line(628 * U, 456 * U, 0);
    v3_line(12 * U, 456 * U, 0);
    v3_line(12 * U, 24 * U, 0);
}

/* A cube 150 units wide about the origin: the top, the bottom, the sides;
 * solid, its six faces first. Corner i is at x = ±h (bit 0), y (bit 1), z
 * (bit 2). */
static const uint8_t cube_faces[6][4] = {
    { 0, 1, 3, 2 }, { 4, 5, 7, 6 }, { 0, 1, 5, 4 }, { 0, 2, 6, 4 }, { 1, 3, 7, 5 }, { 2, 3, 7, 6 },
};

/* The first `faces` faces of a box from `lo` to `hi` (x, y, z), its corners
 * numbered as the cube's: z = lo, z = hi, y = lo (the top), x = lo, x = hi,
 * y = hi (the bottom). */
static void box_faces(const int16_t *lo, const int16_t *hi, uint8_t faces)
{
    int16_t c[8][3];
    for (uint8_t i = 0; i < 8; i++) {
        c[i][0] = i & 1 ? hi[0] : lo[0];
        c[i][1] = i & 2 ? hi[1] : lo[1];
        c[i][2] = i & 4 ? hi[2] : lo[2];
    }
    for (uint8_t f = 0; f < faces; f++) {
        const uint8_t *k = cube_faces[f];
        v3_face4(c[k[0]], c[k[1]], c[k[2]], c[k[3]]);
    }
}

/* The edges of a box from `lo` to `hi`: the y = lo and y = hi squares and
 * the four edges between. */
static void box_edges(const int16_t *lo, const int16_t *hi)
{
    for (uint8_t k = 0; k < 2; k++) {
        int16_t y = k ? hi[1] : lo[1];
        v3_move(lo[0], y, lo[2]);
        v3_line(hi[0], y, lo[2]);
        v3_line(hi[0], y, hi[2]);
        v3_line(lo[0], y, hi[2]);
        v3_line(lo[0], y, lo[2]);
    }
    for (uint8_t i = 0; i < 4; i++) {
        int16_t x = i & 1 ? hi[0] : lo[0], z = i & 2 ? hi[2] : lo[2];
        v3_move(x, lo[1], z);
        v3_line(x, hi[1], z);
    }
}

static void cube(void)
{
    static const int16_t lo[3] = { -75 * U, -75 * U, -75 * U }, hi[3] = { 75 * U, 75 * U, 75 * U };
    if (solid)
        box_faces(lo, hi, 6);
    box_edges(lo, hi);
}

/* An icosahedron with corners 95 units from the origin: (0, ±1, ±φ) and
 * its cyclic shifts; the edges join corners 2 apart. */
#define IA 799
#define IB 1293
static const int16_t ico[12][3] = {
    { 0, IA, IB }, { 0, IA, -IB }, { 0, -IA, IB }, { 0, -IA, -IB },
    { IA, IB, 0 }, { IA, -IB, 0 }, { -IA, IB, 0 }, { -IA, -IB, 0 },
    { IB, 0, IA }, { IB, 0, -IA }, { -IB, 0, IA }, { -IB, 0, -IA },
};
static const uint8_t ico_faces[20][3] = {
    { 0, 2, 8 }, { 0, 2, 10 }, { 0, 4, 6 }, { 0, 4, 8 }, { 0, 6, 10 }, { 1, 3, 9 }, { 1, 3, 11 },
    { 1, 4, 6 }, { 1, 4, 9 }, { 1, 6, 11 }, { 2, 5, 7 }, { 2, 5, 8 }, { 2, 7, 10 }, { 3, 5, 7 },
    { 3, 5, 9 }, { 3, 7, 11 }, { 4, 8, 9 }, { 5, 8, 9 }, { 6, 10, 11 }, { 7, 10, 11 },
};
static const uint8_t ico_edges[30][2] = {
    { 0, 2 }, { 0, 4 }, { 0, 6 }, { 0, 8 }, { 0, 10 }, { 1, 3 }, { 1, 4 }, { 1, 6 }, { 1, 9 }, { 1, 11 },
    { 2, 5 }, { 2, 7 }, { 2, 8 }, { 2, 10 }, { 3, 5 }, { 3, 7 }, { 3, 9 }, { 3, 11 }, { 4, 6 }, { 4, 8 },
    { 4, 9 }, { 5, 7 }, { 5, 8 }, { 5, 9 }, { 6, 10 }, { 6, 11 }, { 7, 10 }, { 7, 11 }, { 8, 9 }, { 10, 11 },
};

static void icosahedron(void)
{
    if (solid)
        for (uint8_t i = 0; i < 20; i++)
            v3_face3(ico[ico_faces[i][0]], ico[ico_faces[i][1]], ico[ico_faces[i][2]]);
    for (uint8_t i = 0; i < 30; i++) {
        const int16_t *a = ico[ico_edges[i][0]], *b = ico[ico_edges[i][1]];
        v3_move(a[0], a[1], a[2]);
        v3_line(b[0], b[1], b[2]);
    }
}

/* A small cross on the glass at (x, y) units: where a model's centre is. */
static void mark(int16_t x, int16_t y)
{
    v3_colour(90, 100, 120, 255);
    v3_width(10);
    v3_move((x - 6) * U, y * U, 0);
    v3_line((x + 6) * U, y * U, 0);
    v3_move(x * U, (y - 6) * U, 0);
    v3_line(x * U, (y + 6) * U, 0);
}

static void wireframes(void)
{
    uint16_t spin = t * 150;
    mark(190, 230);
    v3_push();
    v3_translate(190 * U, 230 * U, 0);
    v3_rotate(V3_AXIS_Y, yaw + spin);
    v3_rotate(V3_AXIS_X, pitch + 0x0C00 + t * 60);
    v3_colour(80, 220, 255, 255);
    v3_width(20);
    cube();
    v3_pop();

    mark(450, 230);
    v3_push();
    v3_translate(450 * U, 230 * U, 0);
    v3_rotate(V3_AXIS_X, pitch + t * 90);
    v3_rotate(V3_AXIS_Y, yaw - spin);
    v3_colour(255, 190, 70, 255);
    v3_width(20);
    icosahedron();
    v3_pop();
}

/* Six rings, each turning about an axis of the ring outside it. */
static void rings(void)
{
    static const uint8_t colours[6][3] = {
        { 90, 150, 255 }, { 120, 120, 255 }, { 170, 100, 255 }, { 220, 90, 230 }, { 255, 100, 170 }, { 255, 140, 110 },
    };
    v3_push();
    v3_translate(320 * U, 240 * U, 40 * U);
    v3_rotate(V3_AXIS_Y, yaw);
    v3_rotate(V3_AXIS_X, pitch);
    v3_width(24);
    v3_move(0, 0, 0); /* the centre: circles are drawn about the current point */
    for (uint8_t i = 0; i < 6; i++) {
        v3_rotate(i & 1 ? V3_AXIS_Y : V3_AXIS_X, (uint16_t)(t * (90 + 40 * i)) + 0x1000 * i);
        v3_colour(colours[i][0], colours[i][1], colours[i][2], 255);
        v3_circle((176 - 26 * i) * U);
    }
    v3_colour(255, 255, 255, 255);
    v3_width(48);
    v3_point();
    v3_pop();
}

/* --- Stars ----------------------------------------------------------------- */

#define STARS 384
#define STAR_FAR 2000  /* units */
#define STAR_NEAR -240
#define COCKPIT_STARS 160 /* the cockpit shows the first 160 */
static int16_t star[STARS][3]; /* x, y in units, z in units */
static uint32_t seed = 0x2545F491;

static uint16_t rnd(uint16_t n)
{
    seed = seed * 1664525 + 1013904223;
    return (seed >> 16) % n;
}

/* Spread over a field wide enough that the edges stay full as they recede. */
static void new_star(uint16_t i, int16_t z)
{
    star[i][0] = 320 + (int16_t)rnd(1760) - 880;
    star[i][1] = 240 + (int16_t)rnd(1320) - 660;
    star[i][2] = z;
}

static void stars_init(void)
{
    for (uint16_t i = 0; i < STARS; i++)
        new_star(i, STAR_NEAR + rnd(STAR_FAR - STAR_NEAR));
}

/* The first `n` stars move on. */
static void stars_step(uint16_t n)
{
    for (uint16_t i = 0; i < n; i++) {
        star[i][2] += 4;
        if (star[i][2] >= STAR_FAR)
            new_star(i, STAR_NEAR);
    }
}

static void starfield(void)
{
    v3_push();
    v3_translate(320 * U, 240 * U, 800 * U);
    v3_rotate(V3_AXIS_Y, yaw);
    v3_rotate(V3_AXIS_X, pitch);
    v3_translate(-320 * U, -240 * U, -800 * U);
    v3_width(40);
    for (uint8_t k = 0; k < 4; k++) {
        v3_colour(k == 1 ? 255 : 200, 220, k == 2 ? 160 : 255, 255);
        for (uint16_t i = k; i < STARS; i += 4) {
            v3_move(star[i][0] * U, star[i][1] * U, star[i][2] * U);
            v3_point();
        }
    }
    v3_pop();
}

/* --- The orrery -------------------------------------------------------------- */

#define QUARTER 0x4000

/* Undoes the world's turn and the orbit's, so a body's circle faces the
 * glass: the inverse rotations, in reverse order. */
static void face_glass(uint16_t node, uint16_t tilt, uint16_t angle)
{
    v3_rotate(V3_AXIS_Z, -angle);
    v3_rotate(V3_AXIS_X, -(uint16_t)(QUARTER + tilt));
    v3_rotate(V3_AXIS_Y, -node);
    v3_rotate(V3_AXIS_Y, -yaw);
    v3_rotate(V3_AXIS_X, -(uint16_t)(pitch + 0x1200));
}

static void orrery(void)
{
    static const struct {
        uint16_t radius, size, node, tilt, speed;
        uint8_t r, g, b, moon;
    } planets[4] = {
        { 70, 7, 0x0000, 0x0000, 700, 200, 200, 210, 0 },
        { 120, 11, 0x2000, 0x0500, 420, 255, 170, 90, 1 },
        { 180, 13, 0x6000, 0xFA00, 260, 90, 170, 255, 1 },
        { 245, 9, 0xA000, 0x0400, 160, 255, 110, 90, 0 },
    };
    v3_push();
    v3_translate(320 * U, 250 * U, 160 * U);
    v3_rotate(V3_AXIS_X, pitch + 0x1200);
    v3_rotate(V3_AXIS_Y, yaw);
    v3_move(0, 0, 0); /* the sun */
    /* The sun: rings facing the glass. */
    v3_push();
    v3_rotate(V3_AXIS_Y, -yaw);
    v3_rotate(V3_AXIS_X, -(uint16_t)(pitch + 0x1200));
    v3_width(24);
    for (uint8_t i = 0; i < 3; i++) {
        v3_colour(255, 230 - 40 * i, 80, 255);
        v3_circle((14 + 6 * i) * U);
    }
    v3_pop();
    for (uint8_t i = 0; i < 4; i++) {
        uint16_t angle = t * planets[i].speed + 0x3000 * i;
        v3_push();
        v3_move(0, 0, 0); /* the orbit's centre, the sun */
        v3_rotate(V3_AXIS_Y, planets[i].node);
        v3_rotate(V3_AXIS_X, QUARTER + planets[i].tilt);
        /* The orbit, then the planet on it. */
        v3_colour(planets[i].r / 3, planets[i].g / 3, planets[i].b / 3, 255);
        v3_width(12);
        v3_circle(planets[i].radius * U);
        v3_rotate(V3_AXIS_Z, angle);
        v3_move(planets[i].radius * U, 0, 0);
        v3_translate(planets[i].radius * U, 0, 0);
        face_glass(planets[i].node, planets[i].tilt, angle);
        v3_colour(planets[i].r, planets[i].g, planets[i].b, 255);
        v3_width(20);
        v3_circle(planets[i].size * U);
        if (planets[i].moon) {
            /* A moon on a ring tilted towards the viewer. */
            v3_rotate(V3_AXIS_X, 0x3000);
            v3_colour(110, 110, 120, 255);
            v3_width(8);
            v3_circle((planets[i].size + 14) * U);
            v3_rotate(V3_AXIS_Z, t * 1500);
            v3_move((planets[i].size + 14) * U, 0, 0);
            v3_colour(230, 230, 240, 255);
            v3_width(40);
            v3_point();
        }
        v3_pop();
    }
    v3_pop();
}

/* --- The city ---------------------------------------------------------------- */

/* Blocks on a ground 400 units square: x0, z0, x1, z1 and the height, in 2
 * units about the centre; the ground is 90 units below it. */
#define GROUND 90
static const int8_t blocks[][5] = {
    { -10, -10, 10, 10, 85 }, { -75, -75, -50, -45, 20 }, { -30, -80, -5, -60, 15 }, { 30, -80, 70, -60, 13 },
    { 50, -40, 75, -10, 30 }, { -80, -20, -55, 15, 28 }, { -40, 30, -15, 60, 18 }, { 15, 35, 45, 55, 23 },
    { 55, 30, 80, 75, 10 }, { -75, 55, -50, 80, 15 }, { 15, -40, 30, -25, 25 },
};
#define BLOCKS (sizeof blocks / sizeof blocks[0])

static void city(void)
{
    v3_push();
    v3_translate(320 * U, 250 * U, 200 * U);
    v3_rotate(V3_AXIS_X, pitch + 0x0900);
    v3_rotate(V3_AXIS_Y, yaw + t * 70);
    /* The ground: a grid of 9 by 9 lines, 50 units apart. */
    v3_colour(40, 150, 80, 255);
    v3_width(12);
    for (int16_t v = -200 * U; v <= 200 * U; v += 50 * U) {
        v3_move(v, GROUND * U, -200 * U);
        v3_line(v, GROUND * U, 200 * U);
        v3_move(-200 * U, GROUND * U, v);
        v3_line(200 * U, GROUND * U, v);
    }
    /* The blocks: five faces each (the bottom lies on the ground), then
     * their edges, the tower in the middle gold. */
    v3_width(18);
    for (uint8_t i = 0; i < BLOCKS; i++) {
        const int8_t *b = blocks[i];
        int16_t lo[3] = { b[0] * 2 * U, (GROUND - 2 * b[4]) * U, b[1] * 2 * U };
        int16_t hi[3] = { b[2] * 2 * U, GROUND * U, b[3] * 2 * U };
        box_faces(lo, hi, 5);
        if (i == 0)
            v3_colour(255, 200, 90, 255);
        else
            v3_colour(80, 200 - 8 * i, 255, 255);
        box_edges(lo, hi);
    }
    v3_pop();
}

/* --- The cockpit ---------------------------------------------------------------- */

/* Three regions of the terminal, each shown on a console by a Surface and
 * so not on the glass: the instruments (x, y, w, h in units), the log and
 * the list. Rows 4-11; the status line (row 1) stays on the glass. */
#define TOP 4
#define ROWS_SHOWN 8
#define INSTR_COL 1
#define LOG_COL 30
#define LIST_COL 59
#define INSTR_W 28
#define LOG_W 28
#define LIST_W 20
#define LIST_ROWS 6
#define LOG_ROWS 7
#define BG_PANEL 236 /* the consoles' dark grey */

static const char *const targets[LIST_ROWS] = { "ALPHA    0.4 AU", "BETA     1.1 AU", "GAMMA    2.7 AU",
                                                "DELTA    3.0 AU", "EPSILON  5.2 AU", "ZETA     9.5 AU" };
static const char *const events[6] = { "nav fix", "comms ping", "hull temp ok", "course trim", "beacon heard", "sensor sweep" };

/* `n` as decimal, at least `digits` digits (zero-padded), into `s`;
 * returns the end. */
static char *num(char *s, uint32_t n, uint8_t digits)
{
    char d[10];
    uint8_t k = 0;
    do {
        d[k++] = '0' + n % 10;
        n /= 10;
    } while (n || k < digits);
    while (k)
        *s++ = d[--k];
    return s;
}

static char *str(char *s, const char *t)
{
    while (*t)
        *s++ = *t++;
    return s;
}

/* A signed angle in 1/65,536 of a turn as whole degrees: "+012". */
static char *degrees(char *s, uint16_t a)
{
    int32_t d = (int32_t)(int16_t)a * 360 / 65536;
    *s++ = d < 0 ? '-' : '+';
    return num(s, (uint32_t)(d < 0 ? -d : d), 3);
}

/* Row `row` of a region at `col`, `w` cells: `s` (to its end), padded. */
static void region_row(uint8_t col, uint8_t row, uint8_t w, uint8_t fg, uint8_t bg, char *s, char *end)
{
    /* Padded where it is: every buffer holds at least `w` characters. */
    while (end < s + w)
        *end++ = ' ';
    term_goto(col, row);
    term_color(fg, bg);
    term_write(s, w);
}

/* The instruments: the readouts that change every frame, and with `all`
 * the attitude and the target too. */
static void instruments(uint8_t all)
{
    char b[32], *e;
    uint32_t secs = t / 60;
    e = str(b, " INSTRUMENTS        T+");
    e = num(e, secs / 60 % 100, 2);
    *e++ = ':';
    e = num(e, secs % 60, 2);
    region_row(INSTR_COL, TOP, INSTR_W, 14, BG_PANEL, b, e);
    uint32_t h = t * 13 % 3600;
    e = str(b, " HEADING   ");
    e = num(e, h / 10, 3);
    *e++ = '.';
    e = num(e, h % 10, 1);
    region_row(INSTR_COL, TOP + 1, INSTR_W, 10, BG_PANEL, b, e);
    e = str(b, " SPEED     0.");
    e = num(e, 300 + t * 7 % 200, 3);
    e = str(e, " c");
    region_row(INSTR_COL, TOP + 2, INSTR_W, 10, BG_PANEL, b, e);
    /* A bar gauge going up and down. */
    uint8_t phase = t % 64, level = (phase < 32 ? phase : 63 - phase) / 2;
    e = str(b, " THRUST [");
    for (uint8_t i = 0; i < 16; i++)
        *e++ = i < level ? '#' : '.';
    *e++ = ']';
    region_row(INSTR_COL, TOP + 3, INSTR_W, 11, BG_PANEL, b, e);
    if (t % 8 == 0 || all) {
        uint32_t fuel = 1000 - t / 8 % 1000;
        e = str(b, " FUEL      ");
        e = num(e, fuel / 10, 2);
        *e++ = '.';
        e = num(e, fuel % 10, 1);
        e = str(e, " %");
        region_row(INSTR_COL, TOP + 6, INSTR_W, 7, BG_PANEL, b, e);
    }
    if (!all)
        return;
    e = str(b, " BANK ");
    e = degrees(e, yaw);
    e = str(e, "    PITCH ");
    e = degrees(e, pitch);
    region_row(INSTR_COL, TOP + 4, INSTR_W, 7, BG_PANEL, b, e);
    e = str(b, " TARGET  ");
    e = str(e, targets[selected]);
    region_row(INSTR_COL, TOP + 5, INSTR_W, 12, BG_PANEL, b, e);
    region_row(INSTR_COL, TOP + 7, INSTR_W, 7, BG_PANEL, b, b);
}

/* The log: a line every 40 frames, the newest at the bottom. */
static void log_lines(void)
{
    char b[32], *e;
    e = str(b, " LOG");
    region_row(LOG_COL, TOP, LOG_W, 14, BG_PANEL, b, e);
    uint32_t n = t / 40 + 4;
    for (uint8_t i = 0; i < LOG_ROWS; i++) {
        e = b;
        if (n + i >= LOG_ROWS) {
            uint32_t k = n + i - LOG_ROWS;
            e = str(b, " T+");
            e = num(e, k * 40 / 60, 4);
            *e++ = ' ';
            e = str(e, events[k % 6]);
        }
        region_row(LOG_COL, TOP + 1 + i, LOG_W, i == LOG_ROWS - 1 ? 15 : 8, BG_PANEL, b, e);
    }
}

/* The list: the selected row highlighted. */
static void list_rows(void)
{
    char b[32], *e;
    e = str(b, " TARGETS");
    region_row(LIST_COL, TOP, LIST_W, 14, BG_PANEL, b, e);
    for (uint8_t i = 0; i < LIST_ROWS; i++) {
        e = str(b, i == selected ? ">" : " ");
        e = str(e, targets[i]);
        region_row(LIST_COL, TOP + 1 + i, LIST_W, i != selected ? 7 : phosphor ? 11 : 0, i == selected && !phosphor ? 11 : BG_PANEL, b, e);
    }
    e = str(b, " click a row");
    region_row(LIST_COL, TOP + 1 + LIST_ROWS, LIST_W, 8, BG_PANEL, b, e);
}

/* The status line, on the glass. */
static void status(void)
{
    char b[80], *e;
    e = str(b, "COCKPIT  bank ");
    e = degrees(e, yaw);
    e = str(e, "  pitch ");
    e = degrees(e, pitch);
    e = str(e, "  target ");
    const char *n = targets[selected];
    while (*n != ' ')
        *e++ = *n++;
    e = str(e, "   the consoles show the terminal");
    region_row(1, 1, 78, 10, 0, b, e);
}

/* The attitude the status line and the instruments show. */
static uint16_t shown_yaw, shown_pitch;

static void consoles_text(void)
{
    status();
    instruments(1);
    log_lines();
    list_rows();
    shown_yaw = yaw;
    shown_pitch = pitch;
}

/* A frame of the cockpit's text: the readouts, the attitude when it
 * changed, the log now and then. */
static void consoles_step(void)
{
    uint8_t turned = yaw != shown_yaw || pitch != shown_pitch;
    instruments(turned);
    if (turned)
        status();
    if (t % 40 == 0)
        log_lines();
    shown_yaw = yaw;
    shown_pitch = pitch;
}

/* A press at (x, y) on the screen: a row of the list (it arrives here also
 * when it was made on the console that shows the list). */
static void press(uint16_t x, uint16_t y)
{
    uint8_t col = x / 8, row = y / 16;
    if (col >= LIST_COL && col < LIST_COL + LIST_W && row >= TOP + 1 && row < TOP + 1 + LIST_ROWS) {
        selected = row - TOP - 1;
        list_rows();
        status();
        instruments(1);
    }
}

/* An octahedron `r` units across its corners, about the current origin. */
static void octahedron(int16_t r)
{
    static const int8_t c[6][3] = { { 1, 0, 0 }, { -1, 0, 0 }, { 0, 1, 0 }, { 0, -1, 0 }, { 0, 0, 1 }, { 0, 0, -1 } };
    for (uint8_t i = 0; i < 6; i++)
        for (uint8_t j = i + 1; j < 6; j++)
            if (i / 2 != j / 2) {
                v3_move(c[i][0] * r, c[i][1] * r, c[i][2] * r);
                v3_line(c[j][0] * r, c[j][1] * r, c[j][2] * r);
            }
}

/* A console: the Surface showing `x`..`x + w` of rows TOP.. of the
 * terminal, `width` units wide, and a frame just in front of it. */
static void console(uint16_t x, uint16_t w, uint16_t width)
{
    uint16_t h = ROWS_SHOWN * 16;
    v3_surface(x, TOP * 16, w, h, width * U);
    int16_t hx = width * U / 2, hy = (int32_t)width * h * U / w / 2;
    v3_colour(120, 140, 170, 255);
    v3_width(20);
    v3_move(-hx - 3 * U, -hy - 3 * U, -U);
    v3_line(hx + 3 * U, -hy - 3 * U, -U);
    v3_line(hx + 3 * U, hy + 3 * U, -U);
    v3_line(-hx - 3 * U, hy + 3 * U, -U);
    v3_line(-hx - 3 * U, -hy - 3 * U, -U);
}

#define TILT 59164 /* -35 degrees about x: a console's top leans away */
#define ANGLE 4005 /* 22 degrees about y */

static void cockpit(void)
{
    /* The world: stars streaming towards the craft, and three wire shapes
     * receding ahead. */
    v3_width(36);
    v3_colour(220, 230, 255, 255);
    for (uint16_t i = 0; i < COCKPIT_STARS; i++) {
        v3_move(star[i][0] * U, (star[i][1] - 60) * U, (STAR_FAR + STAR_NEAR - star[i][2]) * U);
        v3_point();
    }
    v3_width(16);
    for (uint8_t k = 0; k < 3; k++) {
        uint16_t z = (t * 3 + 600 * k) % 1800 + 150;
        v3_push();
        v3_translate((200 + 120 * k) * U, (150 + 40 * (k & 1)) * U, z * U);
        v3_rotate(V3_AXIS_Y, t * 300 + 0x5000 * k);
        v3_rotate(V3_AXIS_X, t * 170);
        v3_colour(k == 0 ? 255 : 120, k == 1 ? 255 : 160, k == 2 ? 255 : 120, 255);
        octahedron(40 * U);
        v3_pop();
    }
    /* The craft: canopy, dashboard and consoles, one model turned about a
     * point between them. */
    v3_push();
    v3_translate(320 * U, 330 * U, 0);
    v3_rotate(V3_AXIS_Z, yaw);
    v3_rotate(V3_AXIS_X, pitch);
    /* The dashboard: a solid face behind the consoles, its top edge lit. */
    static const int16_t dash[4][3] = {
        { -310 * U, -12 * U, 70 * U }, { 310 * U, -12 * U, 70 * U }, { 310 * U, 150 * U, 70 * U }, { -310 * U, 150 * U, 70 * U },
    };
    v3_face4(dash[0], dash[1], dash[2], dash[3]);
    v3_colour(90, 110, 140, 255);
    v3_width(24);
    v3_move(dash[0][0], dash[0][1], dash[0][2]);
    v3_line(dash[1][0], dash[1][1], dash[1][2]);
    /* The canopy: the window's frame near and far, and the struts. */
    static const int16_t near[4][3] = {
        { -330 * U, -330 * U, -160 * U }, { 330 * U, -330 * U, -160 * U }, { 330 * U, -12 * U, -160 * U }, { -330 * U, -12 * U, -160 * U },
    };
    static const int16_t far[4][3] = {
        { -240 * U, -250 * U, 260 * U }, { 240 * U, -250 * U, 260 * U }, { 260 * U, -12 * U, 260 * U }, { -260 * U, -12 * U, 260 * U },
    };
    v3_colour(150, 200, 255, 255);
    v3_width(28);
    for (uint8_t i = 0; i < 4; i++) {
        uint8_t j = (i + 1) % 4;
        v3_move(far[i][0], far[i][1], far[i][2]);
        v3_line(far[j][0], far[j][1], far[j][2]);
        v3_move(near[i][0], near[i][1], near[i][2]);
        v3_line(far[i][0], far[i][1], far[i][2]);
    }
    /* The centre strut, and two struts down past the side consoles. */
    v3_move(0, -330 * U, -160 * U);
    v3_line(0, -250 * U, 260 * U);
    v3_move(-330 * U, -330 * U, -160 * U);
    v3_line(-200 * U, 150 * U, -110 * U);
    v3_move(330 * U, -330 * U, -160 * U);
    v3_line(200 * U, 150 * U, -110 * U);
    /* A sight in the middle of the window. */
    v3_colour(255, 160, 80, 255);
    v3_width(16);
    v3_move(0, -170 * U, 120 * U);
    v3_circle(18 * U);
    /* The consoles: the instruments in the middle, the log on the left,
     * the list on the right, each turned towards the pilot. */
    v3_push();
    v3_translate(0, 50 * U, 0);
    v3_rotate(V3_AXIS_X, TILT);
    console(INSTR_COL * 8, INSTR_W * 8, 220);
    v3_pop();
    v3_push();
    v3_translate(-195 * U, 55 * U, 10 * U);
    v3_rotate(V3_AXIS_Y, -ANGLE);
    v3_rotate(V3_AXIS_X, TILT);
    console(LOG_COL * 8, LOG_W * 8, 170);
    v3_pop();
    v3_push();
    v3_translate(190 * U, 55 * U, 10 * U);
    v3_rotate(V3_AXIS_Y, ANGLE);
    v3_rotate(V3_AXIS_X, TILT);
    console(LIST_COL * 8, LIST_W * 8, 140);
    v3_pop();
    v3_pop();
}

/* --- The frame ---------------------------------------------------------------- */

static void input(void)
{
    terse_event ev;
    while (terse_event_pop(&ev) != EV_NONE) {
        if (ev.type == EV_POINTER) {
            uint8_t b = EV_POINTER_BUTTONS(ev);
            if (b & ~buttons & 1 && scene == COCKPIT)
                press(EV_POINTER_X(ev), EV_POINTER_Y(ev));
            buttons = b;
            continue;
        }
        if (ev.type != EV_KEY_DOWN && ev.type != EV_KEY_UP)
            continue;
        uint8_t down = ev.type == EV_KEY_DOWN;
        switch (ev.a) {
        case KEY_LEFT: key_left = down; break;
        case KEY_RIGHT: key_right = down; break;
        case KEY_UP: key_up = down; break;
        case KEY_DOWN: key_down = down; break;
        }
        if (!down)
            continue;
        if (ev.a == KEY_SPACE || ev.a == KEY_TAB) {
            scene = (scene + 1) % SCENES;
            dirty = 1;
        } else if (ev.a >= KEY_1 && ev.a < KEY_1 + SCENES) {
            scene = ev.a - KEY_1;
            dirty = 1;
        } else if (ev.a == KEY_P) {
            phosphor ^= 1;
            v3_style(phosphor ? VEC_STYLE_PHOSPHOR : VEC_STYLE_CLEAN);
            dirty = 1;
        } else if (ev.a == KEY_A) {
            still ^= 1;
        } else if (ev.a == KEY_H) {
            solid ^= 1;
            dirty = 1;
        } else if (ev.a == KEY_R) {
            yaw = pitch = 0;
            t = 0;
            dirty = 1;
        }
    }
    yaw += (key_right - key_left) * 400;
    pitch += (key_down - key_up) * 400;
}

static void draw(void)
{
    v3_begin(list, sizeof list);
    if (scene != COCKPIT)
        glass();
    switch (scene) {
    case WIREFRAMES: wireframes(); break;
    case RINGS: rings(); break;
    case STARFIELD: starfield(); break;
    case ORRERY: orrery(); break;
    case CITY: city(); break;
    default: cockpit(); break;
    }
    v3_end();
    v3_submit_list(list);
}

int main(void)
{
    term_cmd(TERM_CURSOR_OFF);
    v3_persistence(200);
    stars_init();
    for (;;) {
        input();
        if (!still) {
            t++;
            stars_step(scene == COCKPIT ? COCKPIT_STARS : STARS);
        }
        if (dirty) {
            text();
            dirty = 0;
        } else if (scene == COCKPIT) {
            consoles_step();
        }
        draw();
        terse_wait_frame();
    }
}
