/* Lumen Reef: a first-person wireframe shooter on the 3D vector display
 * (device 9, SPEC §vector3d), its readouts drawn in a stroke font on the 2D
 * vector display (device 3, SPEC §vector).
 *
 * You fly the last lamp skiff at the edge of the Reef, a drift of lights
 * in the dark, and hold it against what comes out of the deep: kites that
 * weave in and spit sparks, wheels that roll across your bow, prongs that
 * dive straight at you and, every fourth stage, a Warden: a solid cube in a
 * ring of four nodes. Between the swarms you thread the Reef itself, slabs
 * and pillars to dodge and gates of light to fly through.
 *
 * Aiming is flying: the sight is where the skiff heads, so the world
 * slides the other way (aim right and everything drifts left), which is
 * also how you dodge the sparks fired at you. The twin bolts cross at the
 * sight's depth (AIM_DEPTH). Firing heats the guns; past the top of the
 * gauge they lock until they cool.
 *
 * Stages go round SWARM, RUN, SWARM, WARDEN, each round faster. A stage's
 * hits on target (per cent of bolts) pay a bonus and a cleared stage
 * gives back one shield cell. The game ends when the shields are gone.
 *
 * Controls: arrows (or W A S D) aim, Space, Z or Ctrl fire, Enter starts, P
 * or Esc pauses, V switches the clean and phosphor styles. The pointer aims
 * (where it is) and its button fires. Controller 0: the left stick or the
 * d-pad aims, A, R1 or R2 fires, Start starts and pauses, Y the style.
 *
 * The world's coordinates are sixteenths of a screen unit about the
 * reference eye's axis: x right, y down, z away, 0 the glass; on drawing
 * they are moved to the screen's centre (CX, CY). The host does all the
 * projection and the turning (models are drawn under Rotate commands); the
 * program only moves points, so its geometry is additions, a sine table
 * and a few divisions. All randomness is a xorshift seeded from the
 * System device's random bytes at start, so a recorded input replays.
 *
 * The readout panels and all text are the 2D list, rebuilt only when what
 * it shows changes; the world is the 3D list, rebuilt every frame. A face
 * on the glass over the panels hides the world behind them (SPEC
 * §vector3d.list), and the radar's blips, on the glass in the face's
 * plane, are not hidden.
 *
 * Optional devices: sound (6) for the effects, content (4) for the high
 * score, kept as the named record "high-score" (four bytes, little-endian)
 * as sparks keeps its own. Without them the game is the same, silent, and
 * the high score lasts for the session.
 *
 * The game's state starts with the eight bytes "REEFSTAT" (struct game,
 * `g`), so that tests can find it in RAM. */

#include <stdint.h>
#include "terse.h"
#include "tables.h"

#define U 16
#define CX (320 * U)
#define CY (240 * U)
#define FAR 2000       /* units: where things appear */
#define AIM_DEPTH 1200 /* units: where the bolts cross */
#define PANEL_Y 400    /* the panels' top edge, screen units */
#define MAXOBJ 28
#define MAXBOLT 16
#define MAXDEBRIS 56
#define NSTARS 72
#define SHIELDS 8
#define HEAT_MAX 400
#define AIM_W 230
#define AIM_H 150

#define KEY_A 0x04
#define KEY_D 0x07
#define KEY_P 0x13
#define KEY_S 0x16
#define KEY_V 0x19
#define KEY_W 0x1A
#define KEY_Z 0x1D
#define KEY_LCTRL 0xE0
#define KEY_RCTRL 0xE4

#define SIN(a) (sintab[(uint8_t)(a)])
#define COS(a) (sintab[(uint8_t)((a) + 64)])

enum { NONE, KITE, WHEEL, PRONG, SPARK, PILLAR, SLAB, GATE, NODE, CORE };
enum { TITLE, INTRO, FIGHT, CLEAR, DYING, OVER };
enum { SWARM, RUN, WARDEN };

static const char *const kind_names[3] = { "SWARM", "REEF RUN", "WARDEN" };
static const char *const kind_hints[3] = { "BREAK THE SWARM", "THREAD THE GATES", "SHOOT THE NODES, THEN THE CUBE" };

typedef struct {
    uint8_t type, hp, phase, flag;
    int32_t x, y, z;
    int16_t vx, vy, vz;
    uint16_t ang, timer, age;
} obj_t;

static struct game {
    char magic[8];
    uint8_t state, stage, shields, paused;
    uint32_t score, high;
    uint16_t kills, need, shots, hits;
    int16_t aim_x, aim_y;
    uint16_t heat, timer;
    uint8_t kind, gates, overheat, nodes;
    uint32_t tick;
    obj_t o[MAXOBJ];
} g = { { 'R', 'E', 'E', 'F', 'S', 'T', 'A', 'T' } };

typedef struct {
    int32_t x, y, z;
    int16_t vx, vy, vz;
    uint8_t life, pad;
} bolt_t;

typedef struct {
    int32_t x, y, z;
    int16_t vx, vy, vz;
    int8_t dx, dy, dz;
    uint8_t life, col;
} debris_t;

static bolt_t bolts[MAXBOLT];
static debris_t debris[MAXDEBRIS];
static int32_t star_x[NSTARS], star_y[NSTARS];
static int16_t star_z[NSTARS];

static uint8_t list2[6144];
static uint8_t list3[12288];
static uint8_t hud_dirty = 1, phosphor = 1;
static uint16_t stage_shots, stage_hits;
static uint32_t bonus;
static uint8_t shake, flash, fire_wait, muzzle;
static int32_t drift_x, drift_y;
static int16_t shown_heat = -1;
static uint8_t shown_blink;

/* --- Numbers ---------------------------------------------------------------- */

static uint32_t seed;
static uint32_t rnd(void)
{
    seed ^= seed << 13;
    seed ^= seed >> 17;
    seed ^= seed << 5;
    return seed;
}

static int32_t rrange(int32_t lo, int32_t hi)
{
    return lo + (int32_t)(rnd() % (uint32_t)(hi - lo + 1));
}

static int32_t iabs(int32_t v) { return v < 0 ? -v : v; }

static int32_t clamp(int32_t v, int32_t lo, int32_t hi) { return v < lo ? lo : v > hi ? hi : v; }

/* `n` in decimal, `digits` wide with leading zeros, into s (ends it). */
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
    *s = 0;
    return s;
}

static char *str(char *s, const char *t)
{
    while (*t)
        *s++ = *t++;
    *s = 0;
    return s;
}

static uint8_t len(const char *s)
{
    uint8_t n = 0;
    while (s[n])
        n++;
    return n;
}

/* --- Sound (device 6, optional) ---------------------------------------------- */

enum { V_LASER, V_BOOM, V_SPARK, V_HIT, V_CHIME };
static uint8_t has_sound, snd_buf[256];
static uint8_t laser_t, boom_t, chime_t, want_laser, want_boom, want_spark, want_hit, want_chime;

static uint32_t hz(uint32_t f) { return f * 89478u; }

static void sound_init(void)
{
    static int8_t wave[64];
    has_sound = terse_has_device(6);
    if (!has_sound)
        return;
    for (uint8_t i = 0; i < 64; i++)
        wave[i] = i < 32 ? 100 : -100;
    snd_load_wave(0, wave);
    for (uint8_t i = 0; i < 64; i++)
        wave[i] = (int8_t)(i * 4 - 128);
    snd_load_wave(1, wave);
    for (uint8_t i = 0; i < 64; i++)
        wave[i] = (int8_t)(i < 32 ? i * 8 - 128 : 383 - i * 8);
    snd_load_wave(2, wave);
    snd_begin(snd_buf, sizeof snd_buf);
    snd_voice(V_LASER);
    snd_wave(1);
    snd_volume(70);
    snd_envelope(0, 1900, 0, 1900);
    snd_voice(V_BOOM);
    snd_noise();
    snd_volume(200);
    snd_envelope(0, 520, 0, 520);
    snd_voice(V_SPARK);
    snd_wave(0);
    snd_volume(50);
    snd_envelope(0, 3000, 0, 3000);
    snd_voice(V_HIT);
    snd_noise();
    snd_volume(255);
    snd_envelope(0, 700, 0, 700);
    snd_voice(V_CHIME);
    snd_wave(2);
    snd_volume(140);
    snd_envelope(0, 600, 0, 600);
    snd_submit_list(snd_buf);
}

static void key(uint8_t v, uint32_t f)
{
    snd_voice(v);
    snd_frequency(hz(f));
    snd_key_on();
}

/* The frame's sound: the effects asked for, and the sweeps going on. */
static void sound_frame(void)
{
    if (!has_sound)
        return;
    snd_begin(snd_buf, sizeof snd_buf);
    if (want_laser)
        laser_t = 8, key(V_LASER, 1700);
    else if (laser_t) {
        laser_t--;
        snd_voice(V_LASER);
        snd_frequency(hz(500 + 150 * laser_t));
    }
    if (want_boom)
        boom_t = 30, key(V_BOOM, 2600);
    else if (boom_t) {
        boom_t--;
        snd_voice(V_BOOM);
        snd_frequency(hz(300 + 80 * boom_t));
    }
    if (want_spark)
        key(V_SPARK, 520);
    if (want_hit)
        key(V_HIT, 700);
    if (want_chime)
        chime_t = 7, key(V_CHIME, 880);
    else if (chime_t && --chime_t == 0)
        key(V_CHIME, 1320);
    want_laser = want_boom = want_spark = want_hit = want_chime = 0;
    if (snd_length(snd_buf))
        snd_submit_list(snd_buf);
}

/* --- High score (device 4, optional) ------------------------------------------ */

static content_req load_req, save_req;
static uint32_t load_handle;
static uint8_t high_buf[4], new_high;

static void save_high(void)
{
    if (g.score <= g.high)
        return;
    g.high = g.score;
    new_high = 1;
    for (uint8_t i = 0; i < 4; i++)
        high_buf[i] = (uint8_t)(g.high >> (8 * i));
    record_save(&save_req, "high-score", high_buf, 4);
}

/* --- Objects -------------------------------------------------------------------- */

static obj_t *spawn(uint8_t type, int32_t x, int32_t y, int32_t z)
{
    for (uint8_t i = 0; i < MAXOBJ; i++) {
        obj_t *o = &g.o[i];
        if (o->type == NONE) {
            *o = (obj_t){ 0 };
            o->type = type;
            o->x = x;
            o->y = y;
            o->z = z;
            return o;
        }
    }
    return 0;
}

static uint8_t count(uint8_t type)
{
    uint8_t n = 0;
    for (uint8_t i = 0; i < MAXOBJ; i++)
        n += g.o[i].type == type;
    return n;
}

static uint8_t hostiles(void)
{
    uint8_t n = 0;
    for (uint8_t i = 0; i < MAXOBJ; i++)
        n += g.o[i].type >= KITE && g.o[i].type <= PRONG;
    return n;
}

/* Colours by object type, and for the debris. */
static const uint8_t colours[][3] = {
    { 255, 255, 255 }, /* NONE: the skiff's own debris */
    { 255, 190, 40 },  /* KITE */
    { 230, 80, 255 },  /* WHEEL */
    { 255, 70, 50 },   /* PRONG */
    { 255, 150, 210 }, /* SPARK */
    { 70, 140, 255 },  /* PILLAR */
    { 70, 140, 255 },  /* SLAB */
    { 60, 255, 120 },  /* GATE */
    { 90, 255, 200 },  /* NODE */
    { 150, 170, 255 }, /* CORE */
};

static void burst(int32_t x, int32_t y, int32_t z, uint8_t col, uint8_t n, uint8_t speed)
{
    for (uint8_t i = 0, k = 0; i < MAXDEBRIS && k < n; i++) {
        debris_t *d = &debris[i];
        if (d->life)
            continue;
        k++;
        d->x = x;
        d->y = y;
        d->z = z;
        d->vx = (int16_t)rrange(-speed, speed) * U / 4;
        d->vy = (int16_t)rrange(-speed, speed) * U / 4;
        d->vz = (int16_t)rrange(-speed, speed) * U / 4;
        d->dx = (int8_t)rrange(-12, 12);
        d->dy = (int8_t)rrange(-12, 12);
        d->dz = (int8_t)rrange(-12, 12);
        d->life = (uint8_t)rrange(24, 40);
        d->col = col;
    }
}

static void explode(obj_t *o)
{
    burst(o->x, o->y, o->z, o->type, o->type == CORE ? 24 : 10, o->type == CORE ? 40 : 24);
    o->type = NONE;
    want_boom = 1;
}

static void add_score(uint32_t n)
{
    g.score += n;
    if (g.score > 999999)
        g.score = 999999;
    hud_dirty = 1;
}

static void damage(uint8_t n)
{
    if (g.state != FIGHT && g.state != INTRO && g.state != CLEAR)
        return;
    g.shields = g.shields > n ? g.shields - n : 0;
    shake = 14;
    flash = 6;
    want_hit = 1;
    hud_dirty = 1;
    if (g.shields == 0) {
        g.state = DYING;
        g.timer = 0;
    }
}

/* A spark from `o` towards the skiff, `speed` units a frame. */
static void fire_spark(obj_t *o, int32_t speed)
{
    obj_t *s = spawn(SPARK, o->x, o->y, o->z);
    if (!s)
        return;
    int32_t n = o->z / (speed * U);
    if (n < 1)
        n = 1;
    s->vx = (int16_t)(-o->x / n);
    s->vy = (int16_t)(-o->y / n);
    s->vz = (int16_t)(-speed * U);
    want_spark = 1;
}

/* Whether the skiff, at (0, 0), is within (w, h) units of (x, y). */
static uint8_t near_skiff(int32_t x, int32_t y, int32_t w, int32_t h)
{
    return iabs(x) < w * U && iabs(y) < h * U;
}

/* The stage's pace: 0 in the first round, rising. */
static int32_t pace(void) { return g.stage; }

static void spawn_enemy(void)
{
    int32_t p = pace();
    uint8_t r = (uint8_t)rnd();
    uint8_t type = KITE;
    if (g.stage >= 2 && r < 70)
        type = WHEEL;
    else if (g.stage >= 4 && r > 200)
        type = PRONG;
    if (type == WHEEL) {
        int32_t side = rnd() & 1 ? 1 : -1;
        obj_t *o = spawn(WHEEL, -side * 1150 * U, rrange(-200, 120) * U, rrange(700, 1400) * U);
        if (o) {
            o->vx = (int16_t)(side * (5 + p / 2) * U);
            o->vz = -U;
            o->timer = (uint16_t)rrange(40, 80);
        }
        return;
    }
    obj_t *o = spawn(type, rrange(-700, 700) * U, rrange(-350, 250) * U, FAR * U);
    if (!o)
        return;
    o->phase = (uint8_t)rnd();
    if (type == KITE) {
        o->vz = (int16_t)(-(9 + p) * U);
        o->timer = (uint16_t)rrange(60, 140);
    } else {
        o->vz = (int16_t)(-(13 + p) * U);
    }
}

static void spawn_reef(void)
{
    int32_t speed = 14 + pace();
    uint8_t r = (uint8_t)rnd();
    obj_t *o;
    if (r < 60)
        o = spawn(GATE, rrange(-250, 250) * U, rrange(-150, 120) * U, FAR * U);
    else if (r < 160)
        o = spawn(PILLAR, rrange(-450, 450) * U, 0, FAR * U);
    else
        o = spawn(SLAB, 0, rrange(-260, 220) * U, FAR * U);
    if (o)
        o->vz = (int16_t)(-speed * U);
}

/* The Warden: the cube, and four nodes on its ring. */
#define RING 200
static void spawn_warden(void)
{
    obj_t *c = spawn(CORE, 0, -40 * U, 1500 * U);
    if (!c)
        return;
    c->hp = 10 + 2 * (g.stage / 4);
    for (uint8_t i = 0; i < 4; i++) {
        obj_t *n = spawn(NODE, 0, 0, 1500 * U);
        if (n) {
            n->phase = i * 64;
            n->hp = 3;
            n->timer = 60 + 30 * i;
        }
    }
    g.nodes = 4;
}

/* --- Stages ------------------------------------------------------------------------ */

static void clear_world(void)
{
    for (uint8_t i = 0; i < MAXOBJ; i++)
        g.o[i].type = NONE;
    for (uint8_t i = 0; i < MAXBOLT; i++)
        bolts[i].life = 0;
}

static void start_stage(void)
{
    static const uint8_t kinds[4] = { SWARM, RUN, SWARM, WARDEN };
    g.kind = kinds[g.stage % 4];
    g.kills = 0;
    g.need = 8 + 2 * g.stage;
    if (g.need > 24)
        g.need = 24;
    g.gates = 0;
    g.state = INTRO;
    g.timer = 0;
    stage_shots = g.shots;
    stage_hits = g.hits;
    clear_world();
    hud_dirty = 1;
}

static void start_game(void)
{
    g.score = 0;
    g.stage = 0;
    g.shields = SHIELDS;
    g.shots = g.hits = 0;
    g.heat = 0;
    g.overheat = 0;
    g.paused = 0;
    new_high = 0;
    for (uint8_t i = 0; i < MAXDEBRIS; i++)
        debris[i].life = 0;
    start_stage();
}

static uint8_t stage_accuracy(void)
{
    uint16_t shots = g.shots - stage_shots, hits = g.hits - stage_hits;
    if (!shots)
        return 0;
    uint32_t a = (uint32_t)hits * 100 / shots;
    return a > 100 ? 100 : (uint8_t)a;
}

static void stage_clear(void)
{
    for (uint8_t i = 0; i < MAXOBJ; i++) {
        obj_t *o = &g.o[i];
        if (o->type != NONE && o->type != GATE && o->type != PILLAR && o->type != SLAB)
            explode(o);
    }
    bonus = (uint32_t)stage_accuracy() * 20;
    add_score(bonus);
    if (g.shields < SHIELDS)
        g.shields++;
    g.state = CLEAR;
    g.timer = 0;
    want_chime = 1;
    hud_dirty = 1;
}

/* --- Input ---------------------------------------------------------------------------- */

static uint8_t k_left, k_right, k_up, k_down, k_fire, m_fire, p_fire, use_pointer;
static uint8_t press_start, press_pause, press_style, aim_held;
static uint16_t ptr_x = 320, ptr_y = 240;
static terse_pad pad;
static uint8_t pad_prev, pad_prev2;

static void input(void)
{
    terse_event ev;
    press_start = press_pause = press_style = 0;
    while (terse_event_pop(&ev) != EV_NONE) {
        if (ev.type == EV_CONTENT_DONE && ev.a == (load_handle & 0xFF)) {
            load_handle = 0;
            if (load_req.status == STATUS_DONE && load_req.length == 4) {
                uint32_t v = high_buf[0] | high_buf[1] << 8 | (uint32_t)high_buf[2] << 16 | (uint32_t)high_buf[3] << 24;
                if (v > g.high && !new_high)
                    g.high = v > 999999 ? 999999 : v;
                hud_dirty = 1;
            }
            continue;
        }
        if (ev.type == EV_POINTER) {
            uint16_t x = EV_POINTER_X(ev), y = EV_POINTER_Y(ev);
            uint8_t b = EV_POINTER_BUTTONS(ev) & 1;
            if (x != ptr_x || y != ptr_y)
                use_pointer = 1;
            ptr_x = x;
            ptr_y = y;
            if (b && !m_fire)
                press_start = 1;
            m_fire = b;
            continue;
        }
        if (ev.type != EV_KEY_DOWN && ev.type != EV_KEY_UP)
            continue;
        uint8_t down = ev.type == EV_KEY_DOWN;
        switch (ev.a) {
        case KEY_LEFT: case KEY_A: k_left = down; break;
        case KEY_RIGHT: case KEY_D: k_right = down; break;
        case KEY_UP: case KEY_W: k_up = down; break;
        case KEY_DOWN: case KEY_S: k_down = down; break;
        case KEY_SPACE: case KEY_Z: case KEY_LCTRL: case KEY_RCTRL:
            if (down && !k_fire)
                press_start = 1;
            k_fire = down;
            break;
        case KEY_ENTER: press_start |= down; break;
        case KEY_P: case KEY_ESCAPE: press_pause |= down; break;
        case KEY_V: press_style |= down; break;
        }
    }
    terse_pad_read(0, &pad);
    uint8_t b = pad.buttons, b2 = pad.buttons2;
    uint8_t edge = b & ~pad_prev, edge2 = b2 & ~pad_prev2;
    pad_prev = b;
    pad_prev2 = b2;
    p_fire = (b & PAD_A) || (b2 & (PAD_R1 | PAD_R2));
    if (edge & PAD_A)
        press_start = 1;
    if (edge2 & PAD_START) {
        if (g.state == TITLE || g.state == OVER)
            press_start = 1;
        else
            press_pause = 1;
    }
    if (edge & PAD_Y)
        press_style = 1;

    /* Aim: the keys and the d-pad speed up as they are held; the stick
     * moves the sight as far as it is pushed; the pointer puts it where it
     * is. Without any, the sight eases back to the middle. */
    int8_t dx = (k_right || (b & PAD_RIGHT)) - (k_left || (b & PAD_LEFT));
    int8_t dy = (k_down || (b & PAD_DOWN)) - (k_up || (b & PAD_UP));
    int8_t sx = pad.lx > 24 || pad.lx < -24 ? pad.lx : 0, sy = pad.ly > 24 || pad.ly < -24 ? pad.ly : 0;
    if (dx || dy || sx || sy) {
        use_pointer = 0;
        if (aim_held < 24)
            aim_held++;
        int16_t speed = 2 + aim_held / 4;
        g.aim_x += dx * speed + sx * 7 / 127;
        g.aim_y += dy * speed + sy * 7 / 127;
    } else {
        aim_held = 0;
        if (use_pointer) {
            g.aim_x = (int16_t)ptr_x - 320;
            g.aim_y = (int16_t)ptr_y - 240;
        } else {
            g.aim_x -= g.aim_x / 24 + (g.aim_x > 0) - (g.aim_x < 0);
            g.aim_y -= g.aim_y / 24 + (g.aim_y > 0) - (g.aim_y < 0);
        }
    }
    g.aim_x = (int16_t)clamp(g.aim_x, -AIM_W, AIM_W);
    g.aim_y = (int16_t)clamp(g.aim_y, -AIM_H, AIM_H);
    if (press_style) {
        phosphor ^= 1;
        v3_style(phosphor ? VEC_STYLE_PHOSPHOR : VEC_STYLE_CLEAN);
    }
}

/* --- The world moves --------------------------------------------------------------------- */

static void stars_step(int32_t speed)
{
    for (uint8_t i = 0; i < NSTARS; i++) {
        star_x[i] += drift_x / 2;
        star_y[i] += drift_y / 2;
        star_z[i] -= (int16_t)speed;
        if (star_z[i] < 60 || iabs(star_x[i]) > 1200 * U || iabs(star_y[i]) > 900 * U) {
            star_x[i] = rrange(-1100, 1100) * U;
            star_y[i] = rrange(-800, 800) * U;
            star_z[i] = star_z[i] < 60 ? FAR : (int16_t)rrange(200, FAR);
        }
    }
}

static int32_t hit_radius(uint8_t type)
{
    switch (type) {
    case KITE: return 46;
    case WHEEL: return 40;
    case PRONG: return 36;
    case NODE: return 28;
    case CORE: return 85;
    }
    return 0;
}

/* The ranging: the nearest target under the sight (its outline, seen from
 * the reference eye, within 14 units of the sight), or none. The guns
 * converge on where it will be when the bolts reach it. */
static obj_t *lock;
static obj_t *find_lock(void)
{
    obj_t *best = 0;
    for (uint8_t i = 0; i < MAXOBJ; i++) {
        obj_t *o = &g.o[i];
        int32_t r = hit_radius(o->type), z = o->z / U;
        if (!r || z < 150 || z > FAR)
            continue;
        int32_t px = o->x / U * 960 / (960 + z), py = o->y / U * 960 / (960 + z);
        r = r * 960 / (960 + z) + 14;
        if (iabs(px - g.aim_x) < r && iabs(py - g.aim_y) < r && (!best || o->z < best->z))
            best = o;
    }
    return best;
}

/* The twin bolts: from the muzzles to the point on the sight's line at
 * AIM_DEPTH, AIM_DEPTH / 80 frames away, and on beyond; or, with a target
 * under the sight, to where it will be. */
#define MUZZLE_X 170
#define MUZZLE_Y 135
#define MUZZLE_Z 60
#define BOLT_SPEED 80
static void fire(void)
{
    int32_t tx = (int32_t)g.aim_x * U * (960 + AIM_DEPTH) / 960, ty = (int32_t)g.aim_y * U * (960 + AIM_DEPTH) / 960;
    int32_t tz = AIM_DEPTH * U;
    int32_t n = (AIM_DEPTH - MUZZLE_Z) / BOLT_SPEED;
    if (lock) {
        obj_t *o = lock;
        n = (o->z - MUZZLE_Z * U) / (BOLT_SPEED * U - o->vz);
        if (n < 1)
            n = 1;
        tx = o->x + (o->vx + drift_x) * n;
        ty = o->y + (o->vy + drift_y) * n;
        tz = o->z + o->vz * n;
    }
    for (int8_t side = -1; side <= 1; side += 2) {
        for (uint8_t i = 0; i < MAXBOLT; i++) {
            bolt_t *b = &bolts[i];
            if (b->life)
                continue;
            b->x = side * MUZZLE_X * U;
            b->y = MUZZLE_Y * U;
            b->z = MUZZLE_Z * U;
            b->vx = (int16_t)clamp((tx - b->x) / n, -2000, 2000);
            b->vy = (int16_t)clamp((ty - b->y) / n, -2000, 2000);
            b->vz = (int16_t)clamp((tz - b->z) / n, 16 * U, 2000);
            b->life = (uint8_t)clamp((FAR * U - MUZZLE_Z * U) / b->vz, 1, 120);
            g.shots++;
            break;
        }
    }
    g.heat += 36;
    if (g.heat >= HEAT_MAX) {
        g.heat = HEAT_MAX;
        g.overheat = 1;
    }
    fire_wait = 8;
    muzzle = 3;
    want_laser = 1;
}

static void hit(obj_t *o)
{
    g.hits++;
    if (o->type == CORE && g.nodes) {
        o->flag = 6; /* shielded: it flashes and takes nothing */
        return;
    }
    if (o->hp > 1) {
        o->hp--;
        o->flag = 6;
        want_spark = 1;
        return;
    }
    switch (o->type) {
    case KITE: add_score(100); g.kills++; break;
    case WHEEL: add_score(150); g.kills++; break;
    case PRONG: add_score(200); g.kills++; break;
    case NODE: add_score(500); g.nodes--; break;
    case CORE: add_score(2000 + 1000 * (g.stage / 4)); break;
    }
    if (o->type == CORE) {
        burst(o->x, o->y, o->z, CORE, 24, 48);
        burst(o->x, o->y, o->z, KITE, 16, 30);
    }
    explode(o);
}

static void bolts_step(void)
{
    for (uint8_t i = 0; i < MAXBOLT; i++) {
        bolt_t *b = &bolts[i];
        if (!b->life)
            continue;
        b->life--;
        b->x += b->vx;
        b->y += b->vy;
        b->z += b->vz;
        for (uint8_t k = 0; k < MAXOBJ; k++) {
            obj_t *o = &g.o[k];
            if (o->type == PILLAR || o->type == SLAB) {
                int32_t hw = o->type == PILLAR ? 40 : 700, hh = o->type == PILLAR ? 700 : 30;
                if (iabs(b->x - o->x) < hw * U && iabs(b->y - o->y) < hh * U && iabs(b->z - o->z) < 80 * U) {
                    b->life = 0;
                    break;
                }
                continue;
            }
            int32_t r = hit_radius(o->type);
            if (!r)
                continue;
            if (iabs(b->x - o->x) < r * U && iabs(b->y - o->y) < r * U && iabs(b->z - o->z) < (r + 40) * U) {
                b->life = 0;
                hit(o);
                break;
            }
        }
    }
}

static void objects_step(void)
{
    int32_t p = pace();
    obj_t *core = 0;
    for (uint8_t i = 0; i < MAXOBJ; i++)
        if (g.o[i].type == CORE)
            core = &g.o[i];
    for (uint8_t i = 0; i < MAXOBJ; i++) {
        obj_t *o = &g.o[i];
        if (o->type == NONE)
            continue;
        o->age++;
        if (o->flag)
            o->flag--;
        o->x += drift_x;
        o->y += drift_y;
        o->x += o->vx;
        o->y += o->vy;
        o->z += o->vz;
        switch (o->type) {
        case KITE:
            o->phase += 3;
            o->x += SIN(o->phase) * (3 + p / 2) * U / 127 - o->x / 128;
            o->y -= o->y / 160;
            o->ang = (uint16_t)(SIN(o->phase) * 70);
            if (o->timer && --o->timer == 0) {
                if (o->z > 300 * U && o->z < 1700 * U && g.state == FIGHT)
                    fire_spark(o, 20 + p);
                o->timer = (uint16_t)rrange(80, 150) - (p > 10 ? 50 : 5 * p);
            }
            if (o->z < 140 * U) {
                if (near_skiff(o->x, o->y, 70, 55)) {
                    damage(2);
                    explode(o);
                } else {
                    o->type = NONE;
                }
            }
            break;
        case WHEEL:
            o->ang += 1300;
            if (o->timer && --o->timer == 0) {
                if (g.state == FIGHT && iabs(o->x) < 700 * U)
                    fire_spark(o, 18 + p);
                o->timer = (uint16_t)rrange(60, 100);
            }
            if ((o->vx > 0 && o->x > 1250 * U) || (o->vx < 0 && o->x < -1250 * U) || o->z < 200 * U)
                o->type = NONE;
            break;
        case PRONG:
            o->ang += 2200;
            if (o->z < 1300 * U) {
                o->x -= o->x / 20;
                o->y -= o->y / 20;
            }
            if (o->z < 100 * U) {
                if (near_skiff(o->x, o->y, 70, 60)) {
                    damage(2);
                    explode(o);
                } else {
                    o->type = NONE;
                }
            }
            break;
        case SPARK:
            o->ang += 3000;
            if (o->z < 40 * U) {
                if (near_skiff(o->x, o->y, 46, 40))
                    damage(1);
                o->type = NONE;
            }
            break;
        case PILLAR:
        case SLAB:
            if (o->z < 60 * U) {
                if (o->type == PILLAR ? near_skiff(o->x, o->y, 40 + 30, 2000) : near_skiff(o->x, o->y, 2000, 30 + 24)) {
                    damage(2);
                    burst(o->x / 2, o->y / 2, 200 * U, o->type, 12, 24);
                }
                o->type = NONE;
            }
            break;
        case GATE:
            if (o->z < 0) {
                int32_t x = o->x / U, y = o->y / U;
                if (x * x + y * y < 82 * 82) {
                    g.gates++;
                    add_score(250);
                    want_chime = 1;
                    burst(o->x, o->y, 100 * U, GATE, 8, 20);
                }
                o->type = NONE;
            }
            break;
        case CORE: {
            /* It sweeps from side to side and comes back to its line after
             * the skiff's drift; with its nodes gone it fires a fan. */
            int32_t tx = SIN(o->age / 3) * 380 * U / 127, ty = SIN(o->age / 5 + 64) * 120 * U / 127 - 40 * U;
            o->x += (tx - o->x) / 40;
            o->y += (ty - o->y) / 40;
            o->ang += g.nodes ? 300 : 900;
            if (!g.nodes && g.state == FIGHT && o->age % (70 - (p > 20 ? 40 : 2 * p)) == 0) {
                fire_spark(o, 16 + p);
                obj_t a = *o;
                a.x -= 160 * U;
                fire_spark(&a, 16 + p);
                a.x += 320 * U;
                fire_spark(&a, 16 + p);
            }
            break;
        }
        case NODE:
            break;
        }
        if (o->type != NONE && o->type != CORE && o->type != NODE && (iabs(o->x) > 1500 * U || iabs(o->y) > 1200 * U))
            o->type = NONE;
    }
    /* The nodes ride the core's ring: tilted 20 degrees towards the eye
     * (sin 44/127, cos 119/127), turning. */
    for (uint8_t i = 0; i < MAXOBJ; i++) {
        obj_t *o = &g.o[i];
        if (o->type != NODE)
            continue;
        if (!core) {
            o->type = NONE;
            continue;
        }
        uint8_t a = (uint8_t)(o->phase + core->age);
        int32_t lx = COS(a) * RING / 127, lz0 = SIN(a) * RING / 127;
        o->x = core->x + lx * U;
        o->y = core->y - lz0 * 44 / 127 * U;
        o->z = core->z + lz0 * 119 / 127 * U;
        o->ang += 2500;
        if (o->timer && --o->timer == 0) {
            if (g.state == FIGHT)
                fire_spark(o, 18 + p);
            o->timer = (uint16_t)rrange(100, 160) - (p > 20 ? 60 : 3 * p);
        }
    }
}

static void debris_step(void)
{
    for (uint8_t i = 0; i < MAXDEBRIS; i++) {
        debris_t *d = &debris[i];
        if (!d->life)
            continue;
        d->life--;
        d->x += d->vx + drift_x;
        d->y += d->vy + drift_y;
        d->z += d->vz;
        if (d->z < 40 * U)
            d->life = 0;
    }
}

static void fight_step(void)
{
    uint16_t t = g.timer;
    if (g.kind == SWARM) {
        uint8_t most = 3 + g.stage / 2;
        if (most > 7)
            most = 7;
        int32_t every = 40 - 3 * g.stage;
        if (every < 18)
            every = 18;
        if (t % every == 0 && hostiles() < most && g.kills + hostiles() < g.need)
            spawn_enemy();
        if (g.kills >= g.need)
            stage_clear();
    } else if (g.kind == RUN) {
        int32_t every = 34 - g.stage;
        if (every < 16)
            every = 16;
        if (t < 1380 && t % every == 0)
            spawn_reef();
        if (g.stage >= 5 && t % 150 == 75 && t < 1380)
            spawn_enemy();
        if (t >= 1500)
            stage_clear();
    } else {
        if (t == 1)
            spawn_warden();
        if (g.stage >= 7 && t % 240 == 120 && hostiles() < 2) {
            obj_t *o = spawn(PRONG, rrange(-600, 600) * U, rrange(-300, 200) * U, FAR * U);
            if (o)
                o->vz = (int16_t)(-(13 + pace()) * U);
        }
        if (t > 1 && count(CORE) == 0)
            stage_clear();
    }
}

static void step(void)
{
    if (g.state == TITLE) {
        drift_x = drift_y = 0;
        stars_step(6 * U);
        if (press_start)
            start_game();
        return;
    }
    if (g.state == OVER) {
        g.timer++;
        drift_x = drift_y = 0;
        stars_step(3 * U);
        debris_step();
        if (press_start && g.timer > 60)
            start_game();
        return;
    }
    if (press_pause) {
        g.paused ^= 1;
        hud_dirty = 1;
    }
    if (g.paused)
        return;
    g.timer++;
    if (g.state == DYING) {
        drift_x = drift_y = 0;
        if (g.timer % 12 == 1 && g.timer < 80) {
            burst(rrange(-200, 200) * U, rrange(-150, 150) * U, rrange(150, 400) * U, NONE, 12, 30);
            want_boom = 1;
        }
        objects_step();
        bolts_step();
        debris_step();
        stars_step(2 * U);
        if (g.timer >= 150) {
            g.state = OVER;
            g.timer = 0;
            save_high();
            hud_dirty = 1;
        }
        return;
    }
    drift_x = -g.aim_x * U / 48;
    drift_y = -g.aim_y * U / 48;
    if (g.heat > 3)
        g.heat -= 3;
    else
        g.heat = 0;
    if (g.overheat && g.heat < 120) {
        g.overheat = 0;
        hud_dirty = 1;
    }
    if (fire_wait)
        fire_wait--;
    if (muzzle)
        muzzle--;
    lock = find_lock();
    if ((k_fire || m_fire || p_fire) && !fire_wait && !g.overheat && g.state != CLEAR)
        fire();
    stars_step((g.kind == RUN ? 14 + pace() : 6) * U);
    if (g.state == FIGHT)
        fight_step();
    objects_step();
    bolts_step();
    debris_step();
    if (g.state == INTRO && g.timer >= 120) {
        g.state = FIGHT;
        g.timer = 0;
        hud_dirty = 1;
    } else if (g.state == CLEAR && g.timer >= 180) {
        g.stage++;
        start_stage();
    }
}

/* --- The 3D list ------------------------------------------------------------------------------ */

static void line3(int16_t x0, int16_t y0, int16_t z0, int16_t x1, int16_t y1, int16_t z1)
{
    v3_move(x0, y0, z0);
    v3_line(x1, y1, z1);
}

/* A model: corners in units, edges as pairs of corner numbers. */
static void model(const int8_t (*v)[3], const uint8_t (*e)[2], uint8_t edges)
{
    for (uint8_t i = 0; i < edges; i++) {
        const int8_t *a = v[e[i][0]], *b = v[e[i][1]];
        line3(a[0] * U, a[1] * U, a[2] * U, b[0] * U, b[1] * U, b[2] * U);
    }
}

/* A kite: two swept wings about a long spine, a fin above and below. */
static const int8_t kite_v[6][3] = { { 0, 0, -36 }, { -46, 0, 6 }, { 46, 0, 6 }, { 0, -16, 10 }, { 0, 16, 10 }, { 0, 0, 30 } };
static const uint8_t kite_e[12][2] = { { 0, 1 }, { 0, 2 }, { 0, 3 }, { 0, 4 }, { 1, 3 }, { 3, 2 }, { 2, 4 }, { 4, 1 }, { 1, 5 }, { 2, 5 }, { 3, 5 }, { 4, 5 } };
/* A prong: a long three-sided spike, point first. */
static const int8_t prong_v[4][3] = { { 0, 0, -44 }, { 0, -26, 20 }, { 23, 13, 20 }, { -23, 13, 20 } };
static const uint8_t prong_e[6][2] = { { 0, 1 }, { 0, 2 }, { 0, 3 }, { 1, 2 }, { 2, 3 }, { 3, 1 } };
/* An octahedron of radius 1 (scaled with v3_scale). */
static const int8_t octa_v[6][3] = { { 64, 0, 0 }, { -64, 0, 0 }, { 0, 64, 0 }, { 0, -64, 0 }, { 0, 0, 64 }, { 0, 0, -64 } };
static const uint8_t octa_e[12][2] = { { 0, 2 }, { 0, 3 }, { 0, 4 }, { 0, 5 }, { 1, 2 }, { 1, 3 }, { 1, 4 }, { 1, 5 }, { 2, 4 }, { 4, 3 }, { 3, 5 }, { 5, 2 } };
/* A box's corners: bit 0 x, bit 1 y, bit 2 z; its faces and edges. */
static const uint8_t box_f[6][4] = { { 0, 1, 3, 2 }, { 4, 5, 7, 6 }, { 0, 1, 5, 4 }, { 2, 3, 7, 6 }, { 0, 2, 6, 4 }, { 1, 3, 7, 5 } };
static const uint8_t box_e[12][2] = { { 0, 1 }, { 2, 3 }, { 4, 5 }, { 6, 7 }, { 0, 2 }, { 1, 3 }, { 4, 6 }, { 5, 7 }, { 0, 4 }, { 1, 5 }, { 2, 6 }, { 3, 7 } };

/* A solid box of half sizes (hx, hy, hz) units about the origin: its faces
 * hide what is behind it, then its edges. */
static void solid_box(int16_t hx, int16_t hy, int16_t hz, uint8_t faces)
{
    int16_t c[8][3];
    for (uint8_t i = 0; i < 8; i++) {
        c[i][0] = (i & 1 ? hx : -hx) * U;
        c[i][1] = (i & 2 ? hy : -hy) * U;
        c[i][2] = (i & 4 ? hz : -hz) * U;
    }
    if (faces)
        for (uint8_t f = 0; f < 6; f++)
            v3_face4(c[box_f[f][0]], c[box_f[f][1]], c[box_f[f][2]], c[box_f[f][3]]);
    for (uint8_t i = 0; i < 12; i++) {
        const int16_t *a = c[box_e[i][0]], *b = c[box_e[i][1]];
        line3(a[0], a[1], a[2], b[0], b[1], b[2]);
    }
}

static void octa(uint16_t scale)
{
    v3_scale(scale);
    model(octa_v, octa_e, 12);
}

static void colour(uint8_t type, uint8_t bright)
{
    const uint8_t *c = colours[type];
    if (bright)
        v3_colour(255, 255, 255, 255);
    else
        v3_colour(c[0], c[1], c[2], 255);
}

static void at(int32_t x, int32_t y, int32_t z)
{
    v3_push();
    v3_translate((int16_t)(CX + x), (int16_t)(CY + y), (int16_t)z);
}

static void draw_wheel(void)
{
    v3_rotate(V3_AXIS_Y, 0x1400);
    v3_move(0, 0, 0);
    v3_circle(36 * U);
    v3_circle(11 * U);
    line3(0, 0, -14 * U, 0, 0, 14 * U);
    for (uint8_t k = 0; k < 3; k++) {
        uint8_t a = k * 85;
        line3(COS(a) * 11 * U / 127, SIN(a) * 11 * U / 127, 0, COS(a) * 36 * U / 127, SIN(a) * 36 * U / 127, 0);
    }
}

static void draw_object(obj_t *o, uint8_t demo)
{
    uint8_t bright = o->flag & 2;
    if (!demo && (o->z < 20 * U || o->z > (FAR + 10) * U))
        return;
    at(o->x, o->y, o->z);
    colour(o->type, bright);
    switch (o->type) {
    case KITE:
        v3_rotate(V3_AXIS_Z, o->ang);
        model(kite_v, kite_e, 12);
        break;
    case WHEEL:
        v3_rotate(V3_AXIS_Z, o->ang);
        draw_wheel();
        break;
    case PRONG:
        v3_rotate(V3_AXIS_Z, o->ang);
        model(prong_v, prong_e, 6);
        break;
    case SPARK:
        v3_rotate(V3_AXIS_Y, o->ang);
        v3_rotate(V3_AXIS_X, o->ang >> 1);
        v3_width(24);
        line3(-9 * U, 0, 0, 9 * U, 0, 0);
        line3(0, -9 * U, 0, 0, 9 * U, 0);
        line3(0, 0, -9 * U, 0, 0, 9 * U);
        v3_width(16);
        break;
    case PILLAR:
        solid_box(40, 700, 30, 1);
        break;
    case SLAB:
        solid_box(700, 30, 30, 1);
        break;
    case GATE:
        v3_move(0, 0, 0);
        v3_width(24);
        v3_circle(90 * U);
        for (uint8_t k = 0; k < 4; k++) {
            uint8_t a = k * 64 + 32;
            line3(COS(a) * 90 * U / 127, SIN(a) * 90 * U / 127, 0, COS(a) * 110 * U / 127, SIN(a) * 110 * U / 127, 0);
        }
        v3_width(16);
        break;
    case NODE:
        v3_rotate(V3_AXIS_Y, o->ang);
        v3_rotate(V3_AXIS_X, 0x2000);
        octa(26 * 4); /* 64 * 104 / 256 = 26 units */
        break;
    case CORE: {
        /* The ring the nodes ride, then the cube: solid while a node is
         * left, open (no faces) with its heart showing once they are gone. */
        v3_push();
        v3_rotate(V3_AXIS_X, 0x4000 + 3641);
        v3_move(0, 0, 0);
        v3_colour(60, 90, 160, 255);
        v3_circle(RING * U);
        v3_pop();
        v3_rotate(V3_AXIS_Y, o->ang);
        v3_rotate(V3_AXIS_X, o->ang / 3);
        if (g.nodes)
            colour(CORE, bright);
        else
            v3_colour(255, (g.tick & 8) ? 90 : 160, 60, 255);
        solid_box(70, 70, 70, g.nodes != 0);
        if (!g.nodes) {
            colour(bright ? CORE : PRONG, bright);
            v3_rotate(V3_AXIS_Z, o->ang * 3);
            octa(36 * 4);
        }
        break;
    }
    }
    v3_pop();
}

/* The sight: brackets on the line from the reference eye through the aim,
 * where the bolts cross (at AIM_DEPTH, or yellow at the target the guns
 * range on), and a small cross on the glass. */
static void sight(void)
{
    int32_t d = lock && lock->type != NONE ? lock->z / U : AIM_DEPTH;
    int32_t k = (960 + d) * 256 / 960;
    at((int32_t)g.aim_x * U * k / 256, (int32_t)g.aim_y * U * k / 256, d * U);
    v3_scale((uint16_t)k);
    if (g.overheat)
        v3_colour(255, 60, 40, 255);
    else if (d != AIM_DEPTH)
        v3_colour(255, 230, 90, 255);
    else
        v3_colour(120, 255, 150, 255);
    v3_width(20);
    for (uint8_t q = 0; q < 4; q++) {
        int16_t sx = q & 1 ? 1 : -1, sy = q & 2 ? 1 : -1;
        v3_move(sx * 16 * U, sy * 8 * U, 0);
        v3_line(sx * 16 * U, sy * 16 * U, 0);
        v3_line(sx * 8 * U, sy * 16 * U, 0);
    }
    v3_move(0, 0, 0);
    v3_point();
    v3_pop();
    v3_colour(70, 140, 90, 255);
    v3_width(12);
    int16_t x = CX + g.aim_x * U, y = CY + g.aim_y * U;
    line3(x - 4 * U, y, 0, x + 4 * U, y, 0);
    line3(x, y - 4 * U, 0, x, y + 4 * U, 0);
}

/* The guns: a barrel each side, from below the panels' edge to the muzzle. */
static void guns(void)
{
    v3_colour(110, 130, 160, 255);
    v3_width(24);
    for (int8_t s = -1; s <= 1; s += 2) {
        int16_t mx = CX + s * MUZZLE_X * U, my = CY + MUZZLE_Y * U;
        line3(CX + s * 215 * U, CY + 170 * U, 0, mx, my, MUZZLE_Z * U);
        line3(CX + s * 135 * U, CY + 170 * U, 0, mx, my, MUZZLE_Z * U);
        if (muzzle) {
            v3_colour(200, 255, 255, 255);
            v3_move(mx, my, MUZZLE_Z * U);
            v3_circle(7 * U);
            v3_colour(110, 130, 160, 255);
        }
    }
}

/* Radar blips on the glass in the middle panel: x across, depth up. */
#define RADAR_X0 256
#define RADAR_X1 384
#define RADAR_Y0 414
#define RADAR_Y1 466
static void radar(void)
{
    v3_width(40);
    uint8_t last = 0xFF;
    for (uint8_t i = 0; i < MAXOBJ; i++) {
        obj_t *o = &g.o[i];
        if (o->type == NONE || o->type == SPARK || o->z < 0)
            continue;
        int32_t x = 320 + o->x / U * (RADAR_X1 - RADAR_X0) / 2 / 1300, y = RADAR_Y1 - 3 - o->z / U * (RADAR_Y1 - RADAR_Y0 - 6) / FAR;
        if (x < RADAR_X0 + 3 || x > RADAR_X1 - 3)
            continue;
        if (o->type != last) {
            colour(o->type, 0);
            last = o->type;
        }
        v3_move((int16_t)(x * U), (int16_t)(y * U), 0);
        v3_point();
    }
    v3_colour(200, 255, 220, 255);
    v3_width(16);
    v3_move(316 * U, (RADAR_Y1 - 1) * U, 0);
    v3_line(320 * U, (RADAR_Y1 - 6) * U, 0);
    v3_line(324 * U, (RADAR_Y1 - 1) * U, 0);
}

static void stars(void)
{
    v3_colour(140, 150, 200, 255);
    v3_width(24);
    for (uint8_t i = 0; i < NSTARS; i++) {
        v3_move((int16_t)(CX + star_x[i]), (int16_t)(CY + star_y[i]), star_z[i] * U);
        v3_point();
    }
}

/* The roster on the title: each shape turning over its name. */
static void roster(void)
{
    static const uint8_t types[4] = { KITE, WHEEL, PRONG, CORE };
    for (uint8_t i = 0; i < 4; i++) {
        obj_t o = { 0 };
        o.type = types[i];
        int32_t z = i == 3 ? 1300 : 250;
        o.x = (int32_t)(110 + 140 * i - 320) * U * (960 + z) / 960;
        o.y = (int32_t)(250 - 240) * U * (960 + z) / 960;
        o.z = z * U;
        o.ang = (uint16_t)(g.tick * 400);
        o.age = (uint16_t)g.tick;
        if (o.type == CORE) {
            uint8_t nodes = g.nodes;
            g.nodes = 4;
            draw_object(&o, 1);
            for (uint8_t k = 0; k < 4; k++) {
                uint8_t a = (uint8_t)(k * 64 + g.tick);
                obj_t n = { 0 };
                n.type = NODE;
                n.x = o.x + COS(a) * RING / 127 * U;
                n.y = o.y - SIN(a) * RING / 127 * 44 / 127 * U;
                n.z = o.z + SIN(a) * RING / 127 * 119 / 127 * U;
                n.ang = o.ang;
                draw_object(&n, 1);
            }
            g.nodes = nodes;
        } else {
            v3_push();
            v3_translate((int16_t)(CX + o.x), (int16_t)(CY + o.y), (int16_t)o.z);
            v3_rotate(V3_AXIS_Y, o.ang);
            v3_translate((int16_t)(-CX - o.x), (int16_t)(-CY - o.y), (int16_t)-o.z);
            o.ang = 0x1000;
            draw_object(&o, 1);
            v3_pop();
        }
    }
}

static void draw3(void)
{
    v3_begin(list3, sizeof list3);
    /* The panels hide the world behind them. */
    static const int16_t panel[4][3] = { { 0, PANEL_Y * U, 0 }, { 640 * U, PANEL_Y * U, 0 }, { 640 * U, 480 * U, 0 }, { 0, 480 * U, 0 } };
    if (g.state != TITLE)
        v3_face4(panel[0], panel[1], panel[2], panel[3]);
    v3_push();
    if (shake) {
        v3_translate((int16_t)rrange(-shake, shake) * U / 2, (int16_t)rrange(-shake, shake) * U / 2, 0);
        if (!g.paused)
            shake--;
    }
    stars();
    v3_width(16);
    if (g.state == TITLE) {
        roster();
    } else {
        for (uint8_t i = 0; i < MAXOBJ; i++)
            if (g.o[i].type != NONE)
                draw_object(&g.o[i], 0);
        v3_colour(170, 255, 255, 255);
        v3_width(28);
        for (uint8_t i = 0; i < MAXBOLT; i++) {
            bolt_t *b = &bolts[i];
            if (b->life)
                line3((int16_t)(CX + b->x), (int16_t)(CY + b->y), (int16_t)b->z, (int16_t)(CX + b->x - b->vx), (int16_t)(CY + b->y - b->vy), (int16_t)(b->z - b->vz));
        }
    }
    v3_width(16);
    uint8_t last = 0xFF;
    for (uint8_t i = 0; i < MAXDEBRIS; i++) {
        debris_t *d = &debris[i];
        if (!d->life)
            continue;
        if (d->col != last) {
            colour(d->col, 0);
            last = d->col;
        }
        int16_t x = (int16_t)(CX + d->x), y = (int16_t)(CY + d->y), z = (int16_t)d->z;
        line3(x - d->dx * U, y - d->dy * U, z - d->dz * U, x + d->dx * U, y + d->dy * U, z + d->dz * U);
    }
    v3_pop();
    if (g.state != TITLE && g.state != OVER) {
        if (g.state != DYING) {
            guns();
            sight();
        }
        radar();
    }
    if (flash) {
        /* A hit: the screen's edge lights red for a moment. */
        v3_colour(255, 40, 30, 255);
        v3_width(48);
        line3(4 * U, 4 * U, 0, 636 * U, 4 * U, 0);
        v3_line(636 * U, 396 * U, 0);
        v3_line(4 * U, 396 * U, 0);
        v3_line(4 * U, 4 * U, 0);
        if (!g.paused)
            flash--;
    }
    v3_end();
    v3_submit_list(list3);
}

/* --- The 2D list: panels and text ------------------------------------------------------------- */

/* `s` at (x, y) units (its top left), each grid step s2 sixteenths: a
 * letter is 10 * s2 sixteenths wide... its advance s2 units. */
static void text(int16_t x, int16_t y, uint8_t s2, const char *s)
{
    vec_width((uint8_t)(s2 < 30 ? 10 + s2 : 40));
    int16_t ox = x * U, oy = y * U;
    for (; *s; s++, ox += s2 * U) {
        uint8_t c = (uint8_t)*s;
        if (c >= 'a' && c <= 'z')
            c -= 32;
        if (c < 32 || c >= 96 || font_ascii[c - 32] == 0xFF)
            continue;
        const uint8_t *p = &font_points[font_index[font_ascii[c - 32]]];
        uint8_t pen = 0, b;
        while ((b = *p++) != 0xFE) {
            if (b == 0xFF) {
                pen = 0;
                continue;
            }
            int16_t gx = ox + (b / 23) * s2, gy = oy + (b % 23) * s2;
            if (pen)
                vec_line(gx, gy);
            else
                vec_move(gx, gy);
            pen = 1;
        }
    }
}

/* Centred on x: n letters are n * s2 units less the last gap (6/16). */
static void ctext(int16_t x, int16_t y, uint8_t s2, const char *s)
{
    text(x - (len(s) * s2 - s2 * 6 / 16) / 2, y, s2, s);
}

static void rtext(int16_t x, int16_t y, uint8_t s2, const char *s)
{
    text(x - (len(s) * s2 - s2 * 6 / 16), y, s2, s);
}

/* A panel's frame: a rectangle with its corners cut. */
static void frame(int16_t x0, int16_t y0, int16_t x1, int16_t y1)
{
    const int16_t c = 6;
    vec_width(16);
    vec_move((x0 + c) * U, y0 * U);
    vec_line((x1 - c) * U, y0 * U);
    vec_line(x1 * U, (y0 + c) * U);
    vec_line(x1 * U, (y1 - c) * U);
    vec_line((x1 - c) * U, y1 * U);
    vec_line((x0 + c) * U, y1 * U);
    vec_line(x0 * U, (y1 - c) * U);
    vec_line(x0 * U, (y0 + c) * U);
    vec_line((x0 + c) * U, y0 * U);
}

/* A gauge of `n` cells from x, `lit` of them lit. */
static void gauge(int16_t x, int16_t y, uint8_t n, uint8_t lit, uint8_t w, uint8_t r, uint8_t gr, uint8_t b)
{
    for (uint8_t i = 0; i < n; i++, x += w + 3) {
        if (i < lit) {
            vec_colour(r, gr, b, 255);
            vec_width(16);
            vec_move(x * U, y * U);
            vec_line((x + w) * U, y * U);
            vec_line((x + w) * U, (y + 12) * U);
            vec_line(x * U, (y + 12) * U);
            vec_line(x * U, y * U);
            vec_move((x + w / 2) * U, (y + 3) * U);
            vec_line((x + w / 2) * U, (y + 9) * U);
        } else {
            vec_colour(r / 3, gr / 3, b / 3, 255);
            vec_width(16);
            vec_move(x * U, (y + 12) * U);
            vec_line((x + w) * U, (y + 12) * U);
        }
    }
}

#define AMBER() vec_colour(255, 170, 50, 255)
#define TEAL() vec_colour(40, 170, 210, 255)
#define WHITE() vec_colour(220, 255, 235, 255)
#define DIM() vec_colour(120, 140, 160, 255)

static void play_hud(void)
{
    char b[40], *e;
    uint8_t blink = (g.tick / 20) & 1;
    TEAL();
    frame(12, 404, 202, 472);
    frame(250, 404, 390, 472);
    frame(438, 404, 628, 472);
    vec_move(0, PANEL_Y * U - 8);
    vec_line(640 * U, PANEL_Y * U - 8);
    /* Left: the score and the high score. */
    AMBER();
    text(22, 411, 8, "SCORE");
    WHITE();
    num(b, g.score, 6);
    text(22, 427, 12, b);
    AMBER();
    text(22, 452, 8, "HI");
    DIM();
    num(b, g.high > g.score ? g.high : g.score, 6);
    text(50, 452, 8, b);
    /* Middle: the radar's name. */
    AMBER();
    text(258, 408, 6, "SCAN");
    /* Right: shields and heat. */
    AMBER();
    text(448, 411, 8, "SHIELD");
    gauge(512, 410, SHIELDS, g.shields, 11, g.shields > 2 ? 90 : 255, g.shields > 2 ? 230 : 70, g.shields > 2 ? 255 : 50);
    if (g.overheat) {
        if (blink) {
            vec_colour(255, 60, 40, 255);
            text(448, 440, 8, "LOCK");
        }
    } else {
        AMBER();
        text(448, 440, 8, "HEAT");
    }
    uint8_t h = (uint8_t)(g.heat / 40);
    gauge(512, 439, 10, h, 8, 255, h > 6 ? 70 : 170, h > 6 ? 40 : 50);
    /* Top: the stage. */
    AMBER();
    e = str(b, "STAGE ");
    num(e, g.stage + 1, 1);
    text(16, 10, 8, b);
    TEAL();
    rtext(624, 10, 8, kind_names[g.kind]);

    if (g.state == INTRO) {
        WHITE();
        ctext(320, 130, 22, b);
        AMBER();
        ctext(320, 175, 14, kind_names[g.kind]);
        TEAL();
        ctext(320, 205, 8, kind_hints[g.kind]);
    } else if (g.state == CLEAR) {
        WHITE();
        ctext(320, 130, 20, g.kind == WARDEN ? "WARDEN DOWN" : "STAGE CLEAR");
        AMBER();
        e = str(b, "ON TARGET ");
        e = num(e, stage_accuracy(), 1);
        str(e, "%");
        ctext(320, 175, 10, b);
        e = str(b, "BONUS ");
        num(e, bonus, 1);
        ctext(320, 195, 10, b);
        if (g.kind == RUN) {
            e = str(b, "GATES ");
            num(e, g.gates, 1);
            ctext(320, 215, 10, b);
        }
    } else if (g.state == DYING) {
        vec_colour(255, 70, 50, 255);
        ctext(320, 150, 18, "SHIELDS GONE");
    }
    if (g.paused) {
        WHITE();
        ctext(320, 150, 22, "PAUSED");
        TEAL();
        ctext(320, 195, 8, "P OR START TO GO ON");
    }
}

static void over_hud(void)
{
    char b[40], *e;
    vec_colour(255, 70, 50, 255);
    ctext(320, 110, 30, "GAME OVER");
    WHITE();
    e = str(b, "SCORE ");
    num(e, g.score, 6);
    ctext(320, 180, 14, b);
    AMBER();
    e = str(b, "REACHED STAGE ");
    num(e, g.stage + 1, 1);
    ctext(320, 215, 9, b);
    e = str(b, "ON TARGET ");
    e = num(e, g.shots ? (uint32_t)g.hits * 100 / g.shots : 0, 1);
    str(e, "%");
    ctext(320, 235, 9, b);
    if (new_high) {
        vec_colour(90, 255, 160, 255);
        ctext(320, 265, 11, "NEW HIGH SCORE");
    }
    if (g.timer > 60 && ((g.tick / 25) & 1)) {
        TEAL();
        ctext(320, 320, 12, "PRESS FIRE");
    }
}

static void title_hud(void)
{
    char b[24];
    vec_colour(70, 220, 255, 255);
    ctext(320, 50, 36, "LUMEN REEF");
    AMBER();
    ctext(320, 112, 9, "HOLD THE LIGHT AT THE EDGE OF THE REEF");
    static const char *const names[4] = { "KITE", "WHEEL", "PRONG", "WARDEN" };
    static const char *const points[4] = { "100", "150", "200", "2000" };
    for (uint8_t i = 0; i < 4; i++) {
        const uint8_t *c = colours[i == 3 ? CORE : KITE + i];
        vec_colour(c[0], c[1], c[2], 255);
        ctext(110 + 140 * i, 320, 9, names[i]);
        DIM();
        ctext(110 + 140 * i, 336, 8, points[i]);
    }
    if ((g.tick / 30) & 1) {
        WHITE();
        ctext(320, 378, 14, "PRESS FIRE");
    }
    DIM();
    ctext(320, 418, 7, "AIM: ARROWS, STICK OR POINTER   FIRE: SPACE, A OR CLICK");
    ctext(320, 432, 7, "P PAUSE   V DISPLAY STYLE");
    AMBER();
    num(str(b, "HI "), g.high, 6);
    ctext(320, 456, 8, b);
}

static void hud(void)
{
    vec_begin(list2, sizeof list2);
    if (g.state == TITLE)
        title_hud();
    else if (g.state == OVER)
        over_hud();
    else
        play_hud();
    vec_end();
    vec_submit_list(list2);
}

/* Whether the 2D list shows something that changed. */
static void hud_check(void)
{
    uint8_t blink;
    if (g.state == TITLE)
        blink = (g.tick / 30) & 1;
    else if (g.state == OVER)
        blink = ((g.tick / 25) & 1) | (g.timer > 60) << 1;
    else
        blink = g.overheat ? (g.tick / 20) & 1 : 0;
    int16_t h = (int16_t)(g.heat / 40);
    if (blink != shown_blink || h != shown_heat)
        hud_dirty = 1;
    shown_blink = blink;
    shown_heat = h;
}

int main(void)
{
    /* A host without the terminal (it is optional) shows none; one with
     * it would show its cursor. */
    if (terse_has_device(2))
        term_cmd(TERM_CURSOR_OFF);
    terse_random(&seed, 4);
    if (!seed)
        seed = 0x2545F491;
    v3_style(VEC_STYLE_PHOSPHOR);
    v3_persistence(120);
    sound_init();
    for (uint8_t i = 0; i < NSTARS; i++) {
        star_x[i] = rrange(-1100, 1100) * U;
        star_y[i] = rrange(-800, 800) * U;
        star_z[i] = (int16_t)rrange(100, FAR);
    }
    /* Asked for now, shown when it arrives: the first frame does not wait. */
    load_handle = record_load(&load_req, "high-score", 0, 4, high_buf);
    g.state = TITLE;
    terse_wait_frame();
    for (;;) {
        input();
        step();
        g.tick++;
        hud_check();
        if (hud_dirty) {
            hud();
            hud_dirty = 0;
        }
        draw3();
        sound_frame();
        terse_wait_frame();
    }
}
