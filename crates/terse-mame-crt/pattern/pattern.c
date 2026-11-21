/* pattern.c: a test pattern for vector displays in the phosphor style
 * (SPEC §vector.phosphor), to compare renderers.
 *
 *   - lines of widths 8, 16, 32, 64 and 128 (sixteenths of a unit), white;
 *   - lines of opacity (brightness) 16, 32, 64, 128 and 255, width 16;
 *   - red, green, blue and white lines, and a grey one;
 *   - a star of eight lines through one point, and a grid, whose crossings
 *     show how overlapping light adds;
 *   - a polyline and circles (outlines of 0.5 to 40 units);
 *   - a dot moving slowly left to right and a line sweeping round, for the
 *     afterglow;
 *   - labels in the terminal, as text drawn by the beam.
 *
 * The persistence is 255 (the longest afterglow a program can ask for);
 * set PERSIST below to change it. No input; runs until stopped. */

#include "terse.h"

#define U 16 /* one screen unit, in list coordinates */
#define PERSIST 255

static uint8_t list[4096];

/* Sine of a (0-255 a whole turn), times 256. */
static int sine(int a)
{
    static const int16_t q[65] = {
        0, 6, 13, 19, 25, 31, 38, 44, 50, 56, 62, 68, 74, 80, 86, 92,
        98, 104, 109, 115, 121, 126, 132, 137, 142, 147, 152, 157, 162, 167, 172, 177,
        181, 185, 190, 194, 198, 202, 206, 209, 213, 216, 220, 223, 226, 229, 231, 234,
        237, 239, 241, 243, 245, 247, 248, 250, 251, 252, 253, 254, 255, 255, 256, 256, 256,
    };
    a &= 255;
    if (a < 64)
        return q[a];
    if (a < 128)
        return q[128 - a];
    if (a < 192)
        return -q[a - 128];
    return -q[256 - a];
}

static void label(uint8_t col, uint8_t row, uint8_t fg, const char *s)
{
    term_goto(col, row);
    term_color(fg, 0);
    term_puts(s);
}

static void draw(uint32_t frame)
{
    vec_begin(list, sizeof list);

    /* Widths: rows of white lines at y = 40, 52, 64, 76, 92 units. */
    static const uint8_t widths[5] = {8, 16, 32, 64, 128};
    static const uint16_t wy[5] = {40, 52, 64, 76, 92};
    vec_colour(255, 255, 255, 255);
    for (int i = 0; i < 5; i++) {
        vec_width(widths[i]);
        vec_move(40 * U, wy[i] * U);
        vec_line(260 * U, wy[i] * U);
    }

    /* Brightness: opacity 16 to 255 at width 16, y = 136 to 200. */
    static const uint8_t alphas[5] = {16, 32, 64, 128, 255};
    vec_width(16);
    for (int i = 0; i < 5; i++) {
        vec_colour(255, 255, 255, alphas[i]);
        vec_move(40 * U, (136 + 16 * i) * U);
        vec_line(260 * U, (136 + 16 * i) * U);
    }

    /* Colours, y = 248 to 312. */
    static const uint8_t rgb[5][3] = {{255, 40, 40}, {40, 255, 40}, {60, 80, 255}, {255, 255, 255}, {128, 128, 128}};
    for (int i = 0; i < 5; i++) {
        vec_colour(rgb[i][0], rgb[i][1], rgb[i][2], 255);
        vec_move(40 * U, (248 + 16 * i) * U);
        vec_line(260 * U, (248 + 16 * i) * U);
    }

    /* A star of eight lines through (440, 110), radius 80. */
    vec_colour(255, 255, 255, 255);
    vec_width(16);
    for (int a = 0; a < 128; a += 16) {
        int dx = sine(a + 64) * 80 / 256, dy = sine(a) * 80 / 256;
        vec_move((440 - dx) * U, (110 - dy) * U);
        vec_line((440 + dx) * U, (110 + dy) * U);
    }

    /* A grid of 6 by 6 lines, 16 units apart, at (360, 220). */
    vec_colour(120, 255, 140, 255);
    for (int i = 0; i < 6; i++) {
        vec_move((360 + 16 * i) * U, 220 * U);
        vec_line((360 + 16 * i) * U, 300 * U);
        vec_move(360 * U, (220 + 16 * i) * U);
        vec_line(440 * U, (220 + 16 * i) * U);
    }

    /* Circles of radius 0.5, 2, 8, 20 and 40 units at y = 270. */
    static const uint16_t radii[5] = {8, 32, 128, 320, 640};
    static const uint16_t cx[5] = {480, 495, 515, 545, 595};
    vec_colour(255, 220, 120, 255);
    for (int i = 0; i < 5; i++) {
        vec_move(cx[i] * U, 270 * U);
        vec_circle(radii[i]);
    }

    /* A polyline: a zigzag. */
    vec_colour(200, 200, 255, 255);
    vec_move(300 * U, 40 * U);
    for (int i = 1; i <= 8; i++)
        vec_line((300 + 10 * i) * U, (i & 1 ? 90 : 40) * U);

    /* A line sweeping round (560, 110), one turn in 4 seconds. */
    {
        int a = (int)(frame * 256 / 240);
        int dx = sine(a + 64) * 50 / 256, dy = sine(a) * 50 / 256;
        vec_colour(255, 255, 255, 255);
        vec_width(16);
        vec_move(560 * U, 110 * U);
        vec_line((560 + dx) * U, (110 + dy) * U);
    }

    /* A dot moving left to right along y = 380, 1 unit a frame. */
    {
        int x = 40 + (int)(frame % 560);
        vec_colour(255, 255, 255, 255);
        vec_width(48);
        vec_move(x * U, 380 * U);
        vec_line(x * U, 380 * U);
    }

    /* The screen's edge. */
    vec_colour(90, 110, 170, 255);
    vec_width(16);
    vec_move(4 * U, 4 * U);
    vec_line(636 * U, 4 * U);
    vec_line(636 * U, 476 * U);
    vec_line(4 * U, 476 * U);
    vec_line(4 * U, 4 * U);

    vec_end();
    vec_submit_list(list);
}

int main(void)
{
    vec_style(VEC_STYLE_PHOSPHOR);
    vec_persistence(PERSIST);
    term_cmd(TERM_CURSOR_OFF);
    label(2, 1, 15, "VECTOR CRT TEST PATTERN");
    label(34, 1, 7, "widths 8 16 32 64 128 (1/16 unit)");
    label(37, 6, 7, "zigzag");
    label(2, 7, 7, "opacity 16 32 64 128 255");
    label(2, 14, 7, "red green blue white grey");
    label(44, 12, 7, "star: 8 lines, 1 point");
    label(66, 11, 7, "sweep 4 s");
    label(45, 19, 7, "grid 6x6");
    label(58, 13, 7, "circles r .5 2 8 20 40");
    label(2, 22, 7, "dot: 1 unit a frame, width 48");
    label(2, 26, 10, "The quick brown fox jumps over the lazy dog. 0123456789");
    label(2, 27, 14, "THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG! (#$%&*+-/:;<=>?@[]^_{|}~)");
    terse_wait_frame();
    for (uint32_t frame = 1;; frame++) {
        draw(frame);
        terse_wait_frame();
    }
}
