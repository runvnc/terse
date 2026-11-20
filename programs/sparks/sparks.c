/* sparks: steer a small craft around an arena, collect drifting sparks and
 * keep clear of the rotating sweepers. Uses the terminal for the HUD and the
 * 2D vector display (device 3) for everything else; built with clang (see
 * build.sh).
 *
 * Coordinates are sixteenths of a screen unit (SPEC 10.1). Angles are 0-255
 * (a full turn; 0 points right, 64 down), sines come from a table, and
 * fmul() scales by them. All randomness mixes in the host's random bytes,
 * so a run is reproducible from its recorded inputs.
 *
 * Controls: Left/Right turn, Up thrusts, Enter starts, Esc quits, P switches
 * between the clean and the phosphor display style (SPEC 10.6), F turns the
 * fill of filled shapes off and on (outlines; SPEC 10.2). Controller 0:
 * d-pad or left stick turns, A, d-pad up or stick up thrusts, A or Start
 * starts, Y switches the style, X the fill.
 *
 * The high score is a named record, "high-score" (SPEC 9.6): four bytes,
 * little-endian. It is asked for in the first frame and shown once it
 * arrives; a game that beats it saves the new one.
 *
 * Content (device 4) is optional (SPEC 2; build.sh packs it with
 * --optional): sparks runs on a host of the program profile, devices 1, 2
 * and 3 only. Without device 4 the record calls return TERSE_ERR_UNKNOWN,
 * which is no handle (load_handle keeps its low byte, FFh, and no content
 * event ever names it), so the high score lasts for the session only and
 * the game is otherwise the same. */

#include <stdint.h>
#include "terse.h"

#define AX0 64
#define AX1 10176
#define AY0 64
#define AY1 7616
#define CRAFT_R 128      /* craft bounces this far from a wall */
#define SPARK_EDGE 96
#define NSPARKS 8
#define MAX_SWEEPERS 6
#define HIT_LEN 64       /* craft reach along a sweeper, past its end */
#define HIT_WIDE 88      /* craft reach across a sweeper (half width 24 + 64) */
#define INVULNERABLE 120 /* frames after a hit, and at the start */

enum { TITLE, PLAY, OVER };

/* 127 sin(2 pi i / 256). */
static const int8_t sintab[256] = {
       0,    3,    6,    9,   12,   16,   19,   22,   25,   28,   31,   34,   37,   40,   43,   46,
      49,   51,   54,   57,   60,   63,   65,   68,   71,   73,   76,   78,   81,   83,   85,   88,
      90,   92,   94,   96,   98,  100,  102,  104,  106,  107,  109,  111,  112,  113,  115,  116,
     117,  118,  120,  121,  122,  122,  123,  124,  125,  125,  126,  126,  126,  127,  127,  127,
     127,  127,  127,  127,  126,  126,  126,  125,  125,  124,  123,  122,  122,  121,  120,  118,
     117,  116,  115,  113,  112,  111,  109,  107,  106,  104,  102,  100,   98,   96,   94,   92,
      90,   88,   85,   83,   81,   78,   76,   73,   71,   68,   65,   63,   60,   57,   54,   51,
      49,   46,   43,   40,   37,   34,   31,   28,   25,   22,   19,   16,   12,    9,    6,    3,
       0,   -3,   -6,   -9,  -12,  -16,  -19,  -22,  -25,  -28,  -31,  -34,  -37,  -40,  -43,  -46,
     -49,  -51,  -54,  -57,  -60,  -63,  -65,  -68,  -71,  -73,  -76,  -78,  -81,  -83,  -85,  -88,
     -90,  -92,  -94,  -96,  -98, -100, -102, -104, -106, -107, -109, -111, -112, -113, -115, -116,
    -117, -118, -120, -121, -122, -122, -123, -124, -125, -125, -126, -126, -126, -127, -127, -127,
    -127, -127, -127, -127, -126, -126, -126, -125, -125, -124, -123, -122, -122, -121, -120, -118,
    -117, -116, -115, -113, -112, -111, -109, -107, -106, -104, -102, -100,  -98,  -96,  -94,  -92,
     -90,  -88,  -85,  -83,  -81,  -78,  -76,  -73,  -71,  -68,  -65,  -63,  -60,  -57,  -54,  -51,
     -49,  -46,  -43,  -40,  -37,  -34,  -31,  -28,  -25,  -22,  -19,  -16,  -12,   -9,   -6,   -3,
};
#define SIN(a) (sintab[(uint8_t)(a)])
#define COS(a) (sintab[(uint8_t)((a) + 64)])

/* (v * s) / 128, rounded towards zero: v times a table sine. */
static int16_t fmul(int s, int v)
{
    return (int16_t)(v * s / 128);
}

static int abs_(int v) { return v < 0 ? -v : v; }

/* xorshift16, stirred with the host's random bytes. */
static uint16_t seed;
static uint8_t rnd(void)
{
    seed ^= seed << 7;
    seed ^= seed >> 9;
    seed ^= seed << 8;
    seed ^= terse_random_byte();
    if (!seed)
        seed = 0xACE1;
    return (uint8_t)seed;
}

/* The craft. Velocity is in 1/256 sixteenths per frame; cxf, cyf hold the
 * position's fraction. */
static int16_t cx, cy, cvx, cvy;
static uint8_t cxf, cyf, cang;
static int turn;
static uint8_t thrust;

typedef struct {
    int16_t x, y;
    int8_t vx, vy;
    uint8_t phase;
} spark_t;
static spark_t sparks[NSPARKS];

typedef struct {
    int16_t px, py;  /* pivot */
    int16_t len;     /* half length */
    uint16_t ang;    /* angle in 1/256 steps */
    int16_t speed;
    uint8_t a;       /* this frame's angle */
    int16_t ex, ey;  /* pivot to one end */
    uint8_t slot;    /* see new_sweeper */
} sweeper_t;
static sweeper_t sweepers[MAX_SWEEPERS];
static int nsweepers;

static uint8_t state, lives, inv, flash, wait, tick, hud_dirty, until_next;
static uint8_t digits[4];

/* The high score and its record requests. */
static uint32_t high;
static uint8_t high_buf[4], load_handle, new_high;
static content_req load_req, save_req;

static terse_pad pad;
static uint8_t key_left, key_right, key_up, key_start, pad_prev, xy_prev, phosphor, fill_off;
#define KEY_F 0x09
#define KEY_P 0x13

static uint8_t list[1024];

static void new_spark(spark_t *s)
{
    s->x = AX0 + 256 + rnd() * 38;
    s->y = AY0 + 256 + rnd() * 28;
    /* Not on top of the craft. */
    if (abs_(s->x - cx) < 960 && abs_(s->y - cy) < 960)
        s->x += s->x > 5120 ? -4800 : 4800;
    s->vx = (int8_t)(rnd() & 7) - 4;
    s->vy = (int8_t)(rnd() & 7) - 4;
    if (!s->vx && !s->vy)
        s->vx = 3;
    s->phase = rnd();
}

/* Pivots sit in a 4 x 3 grid of slots 2550 apart, jittered, so sweepers
 * never overlap and always stay inside the arena. */
#define SLOTS 12
static const int16_t slot_x[4] = { 1264, 3814, 6364, 8914 };
static const int16_t slot_y[3] = { 1264, 3814, 6364 };

static int slot_taken(int slot)
{
    for (int j = 0; j < nsweepers; j++)
        if (sweepers[j].slot == slot)
            return 1;
    return 0;
}

static int jitter(void) { return (rnd() & 127) * 3 - 192; }

/* Sets up sweepers[nsweepers]; the caller counts it. */
static void new_sweeper(sweeper_t *w)
{
    int slot, tries;
    w->len = 640 + (rnd() & 31) * 12;
    /* From a random slot, the next free one away from the craft (or, if
     * there is none, any free one). */
    slot = rnd() & 15;
    if (slot >= SLOTS)
        slot -= 8;
    for (tries = 0; tries < 2 * SLOTS; tries++) {
        if (++slot == SLOTS)
            slot = 0;
        if (slot_taken(slot))
            continue;
        w->px = slot_x[slot & 3] + jitter();
        w->py = slot_y[slot >> 2] + jitter();
        if (tries >= SLOTS || abs_(w->px - cx) + abs_(w->py - cy) > w->len + 1600)
            break;
    }
    w->slot = slot;
    w->ang = (uint16_t)rnd() << 8;
    w->speed = 128 + rnd();
    if (rnd() & 1)
        w->speed = -w->speed;
}

static void put_at(uint8_t col, uint8_t row, uint8_t fg, uint8_t bg, const char *text)
{
    term_goto(col, row);
    term_color(fg, bg);
    term_puts(text);
}

static uint32_t score_value(void)
{
    return digits[0] * 1000 + digits[1] * 100 + digits[2] * 10 + digits[3];
}

/* Four decimal digits of v. */
static void put_4(uint32_t v)
{
    char s[4];
    for (int i = 3; i >= 0; i--) {
        s[i] = '0' + v % 10;
        v /= 10;
    }
    term_write(s, 4);
}

#define HUD_BG 17
#define BOX_BG 236

/* Row 0: title, score and lives on a coloured background. */
static void hud(void)
{
    char score[5];
    hud_dirty = 0;
    term_fill(0, 0, TERM_COLS, ' ', 7, HUD_BG);
    put_at(2, 0, 226, HUD_BG, "SPARKS");
    put_at(12, 0, 250, HUD_BG, "HIGH ");
    put_4(high);
    if (state == TITLE) {
        put_at(30, 0, 250, HUD_BG, "demo - Enter or Start to play");
        return;
    }
    put_at(30, 0, 15, HUD_BG, "SCORE ");
    for (int i = 0; i < 4; i++)
        score[i] = '0' + digits[i];
    term_write(score, 4);
    put_at(62, 0, 15, HUD_BG, "LIVES ");
    term_fg(196);
    for (int i = 0; i < lives; i++)
        term_puts("♥ ");
}

/* A text box in the middle of the screen, rows 11-17. */
static void box(void)
{
    for (int row = 11; row < 18; row++)
        term_fill(18, row, 44, ' ', 7, BOX_BG);
}

static void reset_field(int n)
{
    cx = 5120;
    cy = 3840;
    cvx = cvy = 0;
    cxf = cyf = 0;
    cang = 192;
    for (nsweepers = 0; nsweepers < n; nsweepers++)
        new_sweeper(&sweepers[nsweepers]);
    for (int i = 0; i < NSPARKS; i++)
        new_spark(&sparks[i]);
}

static void start_title(void)
{
    state = TITLE;
    reset_field(MAX_SWEEPERS);
    term_color(7, 0);
    term_cmd(TERM_CLEAR);
    box();
    put_at(35, 12, 226, BOX_BG, "S P A R K S");
    put_at(21, 14, 15, BOX_BG, "Collect the sparks, avoid the sweepers.");
    put_at(21, 15, 250, BOX_BG, "Arrows or stick: turn. Up or A: thrust.");
    put_at(21, 16, 250, BOX_BG, "Enter or Start: play.        Esc: quit.");
    hud();
}

static void start_game(void)
{
    state = PLAY;
    lives = 3;
    inv = INVULNERABLE;
    flash = 0;
    until_next = 10;
    for (int i = 0; i < 4; i++)
        digits[i] = 0;
    reset_field(1);
    term_color(7, 0);
    term_cmd(TERM_CLEAR);
    hud();
}

static void game_over(void)
{
    char score[5];
    state = OVER;
    wait = 45;
    box();
    put_at(35, 12, 196, BOX_BG, "G A M E   O V E R");
    put_at(36, 14, 15, BOX_BG, "Score ");
    for (int i = 0; i < 4; i++)
        score[i] = '0' + digits[i];
    term_write(score, 4);
    /* A new high score is kept on this machine. */
    if (score_value() > high) {
        high = score_value();
        for (int i = 0; i < 4; i++)
            high_buf[i] = (uint8_t)(high >> (8 * i));
        record_save(&save_req, "high-score", high_buf, 4);
        new_high = 1;
        hud_dirty = 1;
        put_at(33, 15, 226, BOX_BG, "New high score!");
    }
    put_at(21, 16, 250, BOX_BG, "Enter or Start: play again.  Esc: quit.");
}

static void scored(void)
{
    int j = 3;
    while (++digits[j] > 9) {
        digits[j] = 0;
        if (!j)
            break;
        j--;
    }
    hud_dirty = 1;
    if (--until_next == 0) {
        until_next = 10;
        if (nsweepers < MAX_SWEEPERS) {
            new_sweeper(&sweepers[nsweepers]);
            nsweepers++;
        }
    }
}

static void input(void)
{
    terse_event ev;
    uint8_t down, b, edge;
    key_start = 0;
    while (terse_event_pop(&ev)) {
        /* The saved high score arrived (a save's completion needs nothing).
         * A higher score made meanwhile wins. */
        if (ev.type == EV_CONTENT_DONE && ev.a == load_handle) {
            load_handle = 0;
            if (load_req.status == STATUS_DONE && load_req.length == 4) {
                uint32_t v = high_buf[0] | high_buf[1] << 8 | (uint32_t)high_buf[2] << 16 | (uint32_t)high_buf[3] << 24;
                if (v > high && !new_high)
                    high = v > 9999 ? 9999 : v;
                hud_dirty = 1;
            }
            continue;
        }
        if (ev.type != EV_KEY_DOWN && ev.type != EV_KEY_UP)
            continue;
        down = ev.type == EV_KEY_DOWN;
        switch (ev.a) {
        case KEY_LEFT: key_left = down; break;
        case KEY_RIGHT: key_right = down; break;
        case KEY_UP: key_up = down; break;
        case KEY_ENTER: key_start |= down; break;
        case KEY_P: phosphor ^= down; break;
        case KEY_F: fill_off ^= down; break;
        case KEY_ESCAPE:
            if (down)
                terse_exit(0);
            break;
        }
    }
    terse_pad_read(0, &pad);
    b = pad.buttons & (PAD_UP | PAD_LEFT | PAD_RIGHT | PAD_A);
    if (pad.buttons2 & PAD_START)
        b |= 0x80;
    edge = b & ~pad_prev;
    pad_prev = b;
    if (edge & (PAD_A | 0x80))
        key_start = 1;
    edge = pad.buttons & ~xy_prev;
    xy_prev = pad.buttons;
    if (edge & PAD_Y)
        phosphor ^= 1;
    if (edge & PAD_X)
        fill_off ^= 1;
    /* In the phosphor style the shapes are outlined and add light; the
     * persistence stays at its default (160). With the fill off, filled
     * shapes are outlines in the clean style too. */
    vec_style(phosphor ? VEC_STYLE_PHOSPHOR : VEC_STYLE_CLEAN);
    vec_fill(fill_off ? VEC_FILL_OFF : VEC_FILL_ON);

    turn = 0;
    if (key_left || (b & PAD_LEFT))
        turn = -4;
    else if (key_right || (b & PAD_RIGHT))
        turn = 4;
    else if (pad.lx < -24 || pad.lx > 24)
        turn = pad.lx >> 5;
    thrust = key_up || (b & (PAD_UP | PAD_A)) || pad.ly < -64;
}

/* The demo flies itself: turn towards the nearest spark, thrust when facing it. */
static void autopilot(void)
{
    int best = 0x7FFF, tx = 0, ty = 0, c = COS(cang), sn = SIN(cang);
    for (int i = 0; i < NSPARKS; i++) {
        int16_t dx = sparks[i].x - cx, dy = sparks[i].y - cy;
        int16_t d = abs_(dx) + abs_(dy);
        if (d < best) {
            best = d;
            tx = dx;
            ty = dy;
        }
    }
    int16_t cross = fmul(c, ty) - fmul(sn, tx);
    int16_t along = fmul(c, tx) + fmul(sn, ty);
    turn = cross > 64 ? 4 : cross < -64 ? -4 : 0;
    thrust = along > abs_(cross);
}

static void move_craft(void)
{
    int16_t t;
    cang += turn;
    if (thrust) {
        cvx += COS(cang) * 2;
        cvy += SIN(cang) * 2;
    }
    /* Drag, down to rest. */
    if (cvx > 0)
        cvx -= (cvx >> 6) + 1;
    else if (cvx < 0)
        cvx += (-cvx >> 6) + 1;
    if (cvy > 0)
        cvy -= (cvy >> 6) + 1;
    else if (cvy < 0)
        cvy += (-cvy >> 6) + 1;
    t = cxf + cvx;
    cx += t >> 8;
    cxf = (uint8_t)t;
    t = cyf + cvy;
    cy += t >> 8;
    cyf = (uint8_t)t;
    /* Soft bounce: reflect, losing half the speed. */
    if (cx < AX0 + CRAFT_R) {
        cx = AX0 + CRAFT_R;
        if (cvx < 0)
            cvx = -(cvx >> 1);
    } else if (cx > AX1 - CRAFT_R) {
        cx = AX1 - CRAFT_R;
        if (cvx > 0)
            cvx = -(cvx >> 1);
    }
    if (cy < AY0 + CRAFT_R) {
        cy = AY0 + CRAFT_R;
        if (cvy < 0)
            cvy = -(cvy >> 1);
    } else if (cy > AY1 - CRAFT_R) {
        cy = AY1 - CRAFT_R;
        if (cvy > 0)
            cvy = -(cvy >> 1);
    }
}

static void move_sparks(void)
{
    for (int i = 0; i < NSPARKS; i++) {
        spark_t *s = &sparks[i];
        s->x += s->vx;
        s->y += s->vy;
        if (s->x < AX0 + SPARK_EDGE || s->x > AX1 - SPARK_EDGE)
            s->vx = -s->vx;
        if (s->y < AY0 + SPARK_EDGE || s->y > AY1 - SPARK_EDGE)
            s->vy = -s->vy;
        if (state == OVER)
            continue;
        int16_t dx = s->x - cx, dy = s->y - cy;
        if (dx > -224 && dx < 224 && dy > -224 && dy < 224) {
            int8_t a = dx >> 4, b = dy >> 4;
            if (a * a + b * b < 196) {
                new_spark(s);
                if (state == PLAY)
                    scored();
            }
        }
    }
}

/* Rotates the sweepers; returns 1 if one touches the craft. */
static int move_sweepers(void)
{
    int hit = 0;
    for (int i = 0; i < nsweepers; i++) {
        sweeper_t *w = &sweepers[i];
        w->ang += w->speed;
        w->a = w->ang >> 8;
        int c = COS(w->a), sn = SIN(w->a);
        w->ex = fmul(c, w->len);
        w->ey = fmul(sn, w->len);
        if (state == OVER)
            continue;
        int16_t dx = cx - w->px, dy = cy - w->py;
        int lim = w->len + HIT_LEN;
        if (dx > -lim && dx < lim && dy > -lim && dy < lim) {
            /* The craft in the sweeper's frame: u along, t across. */
            int16_t u = fmul(c, dx) + fmul(sn, dy);
            if (u > -lim && u < lim) {
                int16_t t = fmul(c, dy) - fmul(sn, dx);
                if (t > -HIT_WIDE && t < HIT_WIDE)
                    hit = 1;
            }
        }
    }
    return hit;
}

static void step(void)
{
    if (state == TITLE) {
        if (key_start) {
            start_game();
            return;
        }
        autopilot();
    } else if (state == OVER) {
        if (wait)
            wait--;
        else if (key_start) {
            start_game();
            return;
        }
    }
    if (state != OVER)
        move_craft();
    move_sparks();
    if (move_sweepers() && state == PLAY && !inv) {
        flash = 16;
        inv = INVULNERABLE;
        cvx = -cvx;
        cvy = -cvy;
        lives--;
        hud_dirty = 1;
        if (!lives)
            game_over();
    }
    if (inv)
        inv--;
    if (flash)
        flash--;
}

/* One display list for the frame. In the phosphor style lines add light,
 * so the arena border and the craft are drawn twice there (passes = 2):
 * they shine brighter than the sparks, as on a vector monitor. */
static void draw(void)
{
    int passes = phosphor ? 2 : 1;
    vec_begin(list, sizeof list);
    /* Arena border. */
    vec_colour(70, 110, 190, 255);
    vec_width(32);
    for (int n = 0; n < passes; n++) {
        vec_move(AX0, AY0);
        vec_line(AX1, AY0);
        vec_line(AX1, AY1);
        vec_line(AX0, AY1);
        vec_line(AX0, AY0);
    }
    if (flash) {
        vec_colour(255, 40, 40, flash * 8);
        vec_rect(AX1, AY1);
    }

    /* Sparks: a disc and a pulsing ring. */
    vec_width(20);
    for (int i = 0; i < NSPARKS; i++) {
        spark_t *s = &sparks[i];
        int p = (tick + s->phase) & 63;
        if (p >= 32)
            p = 63 - p;
        vec_move(s->x, s->y);
        vec_colour(255, 225, 90, 255);
        vec_disc(52);
        vec_colour(255, 190, 60, 230 - p * 6);
        vec_circle(80 + p * 2);
    }

    /* Sweepers and their pivots. */
    vec_width(48);
    vec_colour(255, 80, 50, 255);
    for (int i = 0; i < nsweepers; i++) {
        sweeper_t *w = &sweepers[i];
        vec_move(w->px - w->ex, w->py - w->ey);
        vec_line(w->px + w->ex, w->py + w->ey);
    }
    vec_colour(255, 170, 130, 255);
    for (int i = 0; i < nsweepers; i++) {
        vec_move(sweepers[i].px, sweepers[i].py);
        vec_disc(56);
    }

    /* The craft: an arrowhead, see-through while invulnerable. */
    if (state != OVER) {
        int c = COS(cang), sn = SIN(cang);
        int16_t bx = cx - fmul(c, 64), by = cy - fmul(sn, 64);
        if (thrust) {
            int flame = 120 + (tick & 3) * 32;
            vec_colour(255, 150, 40, 220);
            vec_width(40 + (tick & 3) * 8);
            vec_move(bx, by);
            vec_line(bx - fmul(c, flame), by - fmul(sn, flame));
        }
        int alpha = inv && (tick & 4) ? 90 : 255;
        int16_t nx = cx + fmul(c, 256), ny = cy + fmul(sn, 256);
        int16_t lx = cx + fmul(COS(cang + 102), 200), ly = cy + fmul(SIN(cang + 102), 200);
        int16_t rx = cx + fmul(COS(cang - 102), 200), ry = cy + fmul(SIN(cang - 102), 200);
        vec_colour(60, 200, 255, alpha / 2);
        vec_move(nx, ny);
        vec_tri(lx, ly, bx, by);
        vec_tri(bx, by, rx, ry);
        vec_colour(200, 245, 255, alpha);
        vec_width(20);
        for (int n = 0; n < passes; n++) {
            vec_line(lx, ly);
            vec_line(bx, by);
            vec_line(rx, ry);
            vec_line(nx, ny);
        }
    }
    /* The title and game-over boxes: their cell backgrounds are below every
     * shape (SPEC 10.5), so a near-opaque panel of the same colour, last in
     * the list, keeps the shapes behind the text from showing through. */
    if (state != PLAY) {
        vec_colour(48, 48, 48, 230);
        vec_move(18 * 128, 11 * 256);
        vec_rect(62 * 128, 18 * 256);
    }
    vec_end();
    vec_submit_list(list);
}

int main(void)
{
    seed = 0xACE1;
    term_cmd(TERM_CURSOR_OFF);
    /* Asked for now, shown when it arrives: the first frame does not wait. */
    load_handle = record_load(&load_req, "high-score", 0, 4, high_buf);
    start_title();
    /* Setting up is frame 0; the game's frames start at frame 1. */
    terse_wait_frame();
    for (;;) {
        input();
        step();
        draw();
        if (hud_dirty)
            hud();
        tick++;
        terse_wait_frame();
    }
}
