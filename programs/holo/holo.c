/* holo: a few scenes on the 3D vector display (device 9, SPEC 13): a
 * rotating cube and icosahedron labelled on the glass, nested rotating
 * rings, a starfield receding in depth and a small orrery. The host does
 * every rotation and the projection (it chooses the eye: move the pointer
 * to look around the models); the program only changes angles, so it does
 * no trigonometry and stays tiny. The terminal (device 2) has the labels
 * and the key hints, on the glass in front of the models.
 *
 * Coordinates are sixteenths of a screen unit: x right, y down, z away
 * from the viewer, 0 at the glass. Angles are 1/65,536 of a turn.
 *
 * Keys: Space (or Tab) next scene, 1-4 a scene, arrows turn the model, A
 * stops and starts the turning, R resets the view, P switches between the
 * clean and the phosphor display style. */

#include <stdint.h>
#include "terse.h"

#define U 16 /* sixteenths per screen unit */

#define KEY_A 0x04
#define KEY_P 0x13
#define KEY_R 0x15
#define KEY_1 0x1E

enum { WIREFRAMES, RINGS, STARFIELD, ORRERY, SCENES };

static const char *const names[SCENES] = { "WIREFRAMES", "RINGS", "STARFIELD", "ORRERY" };

static uint8_t list[8192];
static uint8_t scene, phosphor, still, dirty = 1;
static uint8_t key_left, key_right, key_up, key_down;
static uint32_t t;       /* frames the models have turned */
static uint16_t yaw, pitch; /* the user's turn of the model */

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
    uint8_t col = 8;
    for (uint8_t i = 0; i < SCENES; i++) {
        char n[3] = { '1' + i, ' ', 0 };
        put_at(col, 0, i == scene ? 11 : 8, n);
        put_at(col + 2, 0, i == scene ? 15 : 8, names[i]);
        col += 5 + len(names[i]);
    }
    put_at(1, 29, 8, "Space next scene  1-4 scene  arrows turn  A stop/turn  R reset  P phosphor");
    if (phosphor)
        put_at(71, 0, 10, "PHOSPHOR");
    if (scene == WIREFRAMES) {
        label(190, 24, 14, "CUBE");
        label(190, 25, 8, "8 corners, 12 edges");
        label(450, 24, 11, "ICOSAHEDRON");
        label(450, 25, 8, "12 corners, 30 edges");
    } else if (scene == RINGS) {
        label(320, 27, 13, "six rings, each turning in the one outside it");
    } else if (scene == STARFIELD) {
        label(320, 26, 12, "384 points receding from the glass");
    } else {
        label(320, 27, 14, "an orrery: four planets on tilted orbits, two moons");
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

/* A cube 150 units wide about the origin: the top, the bottom, the sides. */
static void cube(void)
{
    const int16_t h = 75 * U;
    for (int16_t y = -h; y <= h; y += 2 * h) {
        v3_move(-h, y, -h);
        v3_line(h, y, -h);
        v3_line(h, y, h);
        v3_line(-h, y, h);
        v3_line(-h, y, -h);
    }
    for (uint8_t i = 0; i < 4; i++) {
        int16_t x = i & 1 ? h : -h, z = i & 2 ? h : -h;
        v3_move(x, -h, z);
        v3_line(x, h, z);
    }
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
static const uint8_t ico_edges[30][2] = {
    { 0, 2 }, { 0, 4 }, { 0, 6 }, { 0, 8 }, { 0, 10 }, { 1, 3 }, { 1, 4 }, { 1, 6 }, { 1, 9 }, { 1, 11 },
    { 2, 5 }, { 2, 7 }, { 2, 8 }, { 2, 10 }, { 3, 5 }, { 3, 7 }, { 3, 9 }, { 3, 11 }, { 4, 6 }, { 4, 8 },
    { 4, 9 }, { 5, 7 }, { 5, 8 }, { 5, 9 }, { 6, 10 }, { 6, 11 }, { 7, 10 }, { 7, 11 }, { 8, 9 }, { 10, 11 },
};

static void icosahedron(void)
{
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

static void stars_step(void)
{
    for (uint16_t i = 0; i < STARS; i++) {
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

/* --- The frame ---------------------------------------------------------------- */

static void input(void)
{
    terse_event ev;
    while (terse_event_pop(&ev) != EV_NONE) {
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
        } else if (ev.a == KEY_R) {
            yaw = pitch = 0;
            t = 0;
        }
    }
    yaw += (key_right - key_left) * 400;
    pitch += (key_down - key_up) * 400;
}

static void draw(void)
{
    v3_begin(list);
    glass();
    switch (scene) {
    case WIREFRAMES: wireframes(); break;
    case RINGS: rings(); break;
    case STARFIELD: starfield(); break;
    default: orrery(); break;
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
        if (dirty) {
            text();
            dirty = 0;
        }
        if (!still) {
            t++;
            stars_step();
        }
        draw();
        terse_wait_frame();
    }
}
