/* lowbell (Low Bell): an engine for illustrated text adventures, in the manner of
 * the early 1980s. The top of the screen is a vector drawing of the place,
 * the bottom is text: the story's description, what the player types and
 * the replies.
 *
 * The story is data. story.py compiles the story files (the .story files in story/,
 * described in STORY.md) into story_data.h: the rooms, objects, words,
 * rules, events, texts (compressed), art and sounds, all in ROM. Nothing in
 * this file is specific to one story.
 *
 * The screen (80 x 30 cells of 8 x 16 units on the 640 x 480 screen):
 *   rows 0-15   the scene, drawn on the 2D vector display (640 x 256 units)
 *   row 16      the status line (place, time, score: the story's template)
 *   rows 17-28  the last 12 lines of text (Page Up/Down scroll back)
 *   row 29      the input line, or "-- more --" when a reply is longer
 * The display is in the phosphor style: the look of a vector monitor, with
 * the terminal's text drawn by the beam too.
 *
 * Devices: terminal (2) and vector display (3) required; content (4, for
 * saved games) and sound (6) optional: without them the game is the same,
 * silent and without SAVE/RESTORE.
 *
 * Built with clang (see build.sh), which runs story.py first. */

#include "terse.h"

struct word {
    uint16_t off;
    uint8_t len;
    uint16_t kinds;
    uint8_t verb, dir, prep;
};
struct room {
    uint16_t name, desc;
    uint32_t art;
    uint16_t exits, notes, spots;
    uint8_t dark, amb;
    uint16_t amb_every;
};
struct object {
    uint16_t name, desc, initial, read, here, talk, unknown, arrives, leaves;
    uint32_t art;
    uint16_t kinds, nouns, adjs, topics, idle;
    int16_t x, y, tx, ty;
    uint8_t loc, also, state, key, article, pad;
};
struct rule {
    uint16_t cond, eff;
    uint8_t kind, action, o1, o2;
};
struct event {
    uint16_t cond, eff, a, b;
    uint8_t kind;
};
struct hint {
    uint16_t cond, first;
    uint8_t n;
};

#include "story_data.h"

/* --- Fixed numbers (story.py has the same) ---------------------------------- */

#define SCENE_H 256
#define COLS 78             /* text width: columns 1 to 78 */
#define TEXT_TOP 17
#define TEXT_ROWS 12
#define STATUS_ROW 16
#define INPUT_ROW 29
#define RING 128            /* lines of text kept for scrolling back */
#define INPUT_MAX 74
#define UNDO_N 8

/* Word kinds. */
#define WK_VERB 1
#define WK_DIR 2
#define WK_PREP 4
#define WK_NOUN 8
#define WK_ADJ 16
#define WK_ART 32
#define WK_IT 64
#define WK_ALL 128
#define WK_SEP 256
/* Action flags. */
#define AF_NOUN 1
#define AF_SECOND 2
#define AF_META 4
#define AF_TOPIC 8
#define AF_OPTNOUN 16
#define AF_SIGHT 32
/* Object kinds and states. */
#define K_PORTABLE 1
#define K_CONTAINER 2
#define K_SUPPORTER 4
#define K_OPENABLE 8
#define K_LOCKABLE 16
#define K_WEARABLE 32
#define K_LIGHT 64
#define K_SWITCHABLE 128
#define K_READABLE 256
#define K_NPC 512
#define K_SCENERY 1024
#define K_BEAM 2048
#define K_PLURAL 4096
#define K_PROPER 8192
#define K_TRANSPARENT 16384
#define K_DOOR 32768
#define S_OPEN 1
#define S_LOCKED 2
#define S_ON 4
#define S_MOVED 8
/* Places: 0 nowhere, 1 held, 2 worn, then rooms, then inside objects. */
#define L_NOWHERE 0
#define L_HELD 1
#define L_WORN 2
#define L_ROOM(r) (3 + (r))
#define L_OBJ(o) (3 + N_ROOMS + (o))
#define IS_ROOM(l) ((l) >= 3 && (l) < 3 + N_ROOMS)
#define IS_OBJ(l) ((l) >= 3 + N_ROOMS)
/* Prepositions (story.py PREPS). */
#define P_IN 1
#define P_ON 2
#define P_AT 3
#define P_TO 4
#define P_WITH 5
#define P_ABOUT 6
#define P_FROM 7
/* Placeholders. */
enum { PH_THE = 1, PH_THE_C, PH_A, PH_A_C, PH_THE2, PH_THE2_C, PH_A2, PH_WORD, PH_VERB, PH_LIST, PH_IT, PH_IT_C, PH_TIME,
       PH_SCORE, PH_MAX, PH_MOVES, PH_ROOM, PH_BELL, PH_CHOICES, PH_NAME, PH_IS, PH_PREP, PH_IS_C };
/* Conditions and effects. */
enum { C_END, C_FLAG, C_IN, C_CARRYING, C_HERE, C_OBJIN, C_OPEN, C_LOCKED, C_ON, C_WORN, C_VISITED, C_DARK, C_HEARS,
       C_RANDOM, C_BEAM, C_CMP };
enum { E_END, E_SAY, E_SET, E_CLEAR, E_MOVE, E_GO, E_STATE, E_EXIT, E_SOUND, E_ADD, E_LET, E_SCHEDULE, E_CANCEL,
       E_END_GAME, E_LOUD, E_CONTINUE, E_LOOK, E_ACHIEVE, E_AIM, E_TOGGLE, E_MINUTES, E_TRY };
/* Events. */
enum { T_AT = 1, T_EVERY, T_WHEN, T_WHENEVER, T_TIMER, T_NOISE, T_ENTER, T_ATTIME, T_EVERYTIME };
/* Art. */
#define LT_HIDDEN 1
#define LT_FAINT 4
enum { AN_NONE, AN_FLICKER, AN_DRIFT, AN_BLINK };
#define LAYER_HEAD 22

/* Terminal colours (palette entries this program sets). */
#define C_TEXT 1
#define C_ECHO 2
#define C_TITLE 3
#define C_STATUS 4
#define C_DIM 5
#define C_MORE 6
static const uint8_t palette[] = {
    0, 0, 0,          /* 0 */
    200, 205, 190,    /* text */
    255, 190, 90,     /* the player's commands */
    255, 255, 240,    /* titles */
    110, 200, 225,    /* status */
    120, 120, 120,    /* dim */
    255, 220, 120,    /* more */
};

static uint32_t rd16(const uint8_t *p) { return p[0] | (uint32_t)p[1] << 8; }
static uint32_t rd24(const uint8_t *p) { return p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16; }

/* --- The game's state: everything a save, an undo or a restart covers -------------- */

#define FLAG_BYTES ((N_FLAGS + 7) / 8)
struct game {
    uint32_t magic, story;
    uint16_t moves, minutes;  /* moves made; minutes since the story's clock started */
    uint8_t room, ended, picture, it, xover_n, pad;
    int16_t counters[N_COUNTERS];
    uint16_t timers[N_TIMERS];
    int16_t beam_x, beam_y;     /* half units */
    uint32_t rng;
    uint8_t flags[FLAG_BYTES];
    uint8_t loc[N_OBJS], st[N_OBJS];
    uint8_t visited[(N_ROOMS + 7) / 8];
    uint8_t fired[(N_EVENTS + 7) / 8];
    uint8_t hint_level[N_HINTS];
    uint8_t seen[(N_HIDDEN + 7) / 8]; /* each hidden line the beam has fallen on */
    uint8_t ach[(N_ACHIEVEMENTS + 7) / 8];
    uint8_t xover[8][3];
};
#define GAME_MAGIC 0x504D414Cu /* "LAMP" */
static struct game G;
static struct game undo_ring[UNDO_N];
static uint8_t undo_n, undo_head;

static int bit(const uint8_t *b, uint32_t i) { return b[i >> 3] >> (i & 7) & 1; }
static void bit_set(uint8_t *b, uint32_t i, int v)
{
    if (v)
        b[i >> 3] |= 1 << (i & 7);
    else
        b[i >> 3] &= ~(1 << (i & 7));
}

/* --- Interface state (not saved) ------------------------------------------------------ */

static uint8_t has_content, has_sound;
static uint32_t tick;
static uint8_t input[INPUT_MAX + 1], input_len;
static uint8_t last_line[INPUT_MAX + 1], last_len;
static uint8_t history[4][INPUT_MAX + 1], hist_len[4], hist_n, hist_pos;
/* Text. */
static uint8_t ring[RING][COLS + 2];   /* length, colour, characters */
static uint32_t nlines;                /* lines ever pushed */
static uint8_t cur[COLS], curlen, curcol = C_TEXT, wrapped, cap_next;
static uint32_t page_top, view_end;    /* the reply being shown, and its last line shown + 1 */
static uint8_t more;                   /* 1 while a reply waits for a key */
static uint32_t scroll;                /* lines scrolled back */
static uint8_t dirty_text, dirty_status, dirty_input, dirty_scene;
/* Writing into a buffer instead of the text (status line, names). */
static uint8_t to_buf, sbuf[160], sbuf_len;
/* Questions. */
enum { Q_NONE, Q_WHICH, Q_WHAT, Q_RESTART, Q_QUIT };
static uint8_t question;
/* The scene. */
static uint8_t vlist[12288];
static uint32_t anim_key, room_anim;
static int16_t shown_bx, shown_by;   /* the beam as drawn (it glides to G.beam_x, y) */
static uint8_t keys_held;            /* 1 left, 2 right, 4 up, 8 down */
static uint8_t beam_on_shown;
/* Saving. */
static content_req save_req;
static uint8_t save_busy;            /* 1 saving, 2 restoring */
static char save_name[20];
static struct game load_buf;

/* --- Random numbers (in the state, so a replay is exact) -------------------------------- */

static uint32_t rnd(uint32_t n)
{
    G.rng = G.rng * 1103515245u + 12345u;
    return ((G.rng >> 16) & 0x7FFF) % n;
}

/* --- Text output ------------------------------------------------------------------------- */

static void push_line(const uint8_t *s, uint32_t n, uint8_t colour)
{
    uint8_t *l = ring[nlines % RING];
    l[0] = n;
    l[1] = colour;
    memcpy(l + 2, s, n);
    nlines++;
    dirty_text = 1;
}

static void line_end(void)
{
    push_line(cur, curlen, curcol);
    curlen = 0;
    wrapped = 0;
}

static void out_ch(uint8_t c)
{
    if (cap_next && c >= 'a' && c <= 'z') {
        c -= 32;
        cap_next = 0;
    } else if (cap_next && c != ' ')
        cap_next = 0;
    if (to_buf) {
        if (c != '\n' && sbuf_len < sizeof sbuf)
            sbuf[sbuf_len++] = c;
        return;
    }
    if (c == '\n') {
        line_end();
        return;
    }
    if (c == ' ' && curlen == 0 && wrapped)
        return;
    if (curlen == COLS) {
        if (c == ' ') {
            push_line(cur, curlen, curcol);
            curlen = 0;
            wrapped = 1;
            return;
        }
        int sp = curlen - 1;
        while (sp > 0 && cur[sp] != ' ')
            sp--;
        if (sp > 0) {
            push_line(cur, sp, curcol);
            uint8_t rest = curlen - sp - 1;
            memmove(cur, cur + sp + 1, rest);
            curlen = rest;
        } else {
            push_line(cur, curlen, curcol);
            curlen = 0;
        }
        wrapped = 1;
    }
    cur[curlen++] = c;
}

static void out_str(const char *s)
{
    while (*s)
        out_ch((uint8_t)*s++);
}

static void out_num(int32_t n)
{
    char b[12];
    int i = 0;
    if (n < 0) {
        out_ch('-');
        n = -n;
    }
    do
        b[i++] = '0' + n % 10;
    while ((n /= 10) && i < 11);
    while (i)
        out_ch(b[--i]);
}

/* Ends the current line, if anything is on it. */
static void out_nl(void)
{
    if (curlen || to_buf)
        out_ch('\n');
}

/* A blank line between paragraphs (never two). */
static void out_blank(void)
{
    out_nl();
    if (nlines && ring[(nlines - 1) % RING][0] != 0)
        push_line(cur, 0, curcol);
}

static void text(uint16_t id);

/* The context of a message's placeholders. */
static uint8_t ctx1 = 0xFF, ctx2 = 0xFF, ctx_name_set;
static const uint8_t *ctx_word;
static uint8_t ctx_word_len;
static uint16_t ctx_name;
static uint8_t ctx_list[N_OBJS + 1], ctx_list_n, ctx_list_mode;
static uint8_t ctx_prep;
static uint8_t verb_tok, part_tok;   /* the verb's word in the input, and its particle (0xFF: none) */
/* A question asked: the command waits for a noun phrase. */
static uint8_t q_action, q_o1, q_o2, q_dir, q_prep, q_slot, q_verb_len, q_part_len;
static uint8_t q_cands[N_OBJS], q_ncands;
static uint8_t q_verb[16];

/* Input words (the parser fills these). */
#define MAXT 28
static uint16_t tw[MAXT];
static uint8_t tpos[MAXT], tlen[MAXT], nt;
static uint8_t lower[INPUT_MAX + 1];

static void out_word_at(uint8_t t)
{
    for (uint8_t i = 0; i < tlen[t]; i++)
        out_ch(lower[tpos[t] + i]);
}

static uint8_t is_plural(uint8_t o) { return o < N_OBJS && (objs[o].kinds & K_PLURAL); }

static void name_def(uint8_t o, uint8_t cap)
{
    if (o >= N_OBJS)
        return;
    cap_next = cap;
    if (objs[o].article != 4)
        out_str("the ");
    text(objs[o].name);
}

static void name_indef(uint8_t o, uint8_t cap)
{
    if (o >= N_OBJS)
        return;
    static const char *const art[] = { "a ", "an ", "some ", "the ", "" };
    cap_next = cap;
    out_str(art[objs[o].article]);
    text(objs[o].name);
}

static void list_print(void);

static uint32_t now_minutes(void) { return (CLOCK_START + (uint32_t)G.minutes) % 1440; }

/* Minutes to the next hour; 0 on the hour. */
static int bell_moves(void)
{
    uint32_t m = now_minutes() % 60;
    return m ? (int)(60 - m) : 0;
}

static void out_time(void)
{
    uint32_t t = now_minutes(), h = t / 60, m = t % 60;
    if (!CLOCK_24H) {
        uint32_t h12 = h % 12 ? h % 12 : 12;
        out_num(h12);
    } else {
        if (h < 10)
            out_ch('0');
        out_num(h);
    }
    out_ch(':');
    out_ch('0' + m / 10);
    out_ch('0' + m % 10);
    if (!CLOCK_24H)
        out_str(h < 12 ? " am" : " pm");
}

static int lit(void);

static void placeholder(uint8_t code)
{
    switch (code) {
    case PH_THE: name_def(ctx1, 0); break;
    case PH_THE_C: name_def(ctx1, 1); break;
    case PH_A: name_indef(ctx1, 0); break;
    case PH_A_C: name_indef(ctx1, 1); break;
    case PH_THE2: name_def(ctx2, 0); break;
    case PH_THE2_C: name_def(ctx2, 1); break;
    case PH_A2: name_indef(ctx2, 0); break;
    case PH_WORD:
        for (uint8_t i = 0; i < ctx_word_len; i++)
            out_ch(ctx_word[i]);
        break;
    case PH_VERB: /* as saved by save_question, before any question */
        for (uint8_t i = 0; i < q_verb_len; i++)
            out_ch(q_verb[i]);
        if (q_part_len)
            out_ch(' ');
        for (uint8_t i = 0; i < q_part_len; i++)
            out_ch(q_verb[8 + i]);
        break;
    case PH_LIST: list_print(); break;
    case PH_IT: out_str(is_plural(ctx1) ? "them" : "it"); break;
    case PH_IT_C: out_str(is_plural(ctx1) ? "Them" : "It"); break;
    case PH_TIME: out_time(); break;
    case PH_SCORE: out_num(G.counters[1]); break;
    case PH_MAX: out_num(MAX_SCORE); break;
    case PH_MOVES: out_num(G.moves); break;
    case PH_ROOM: text(lit() ? rooms[G.room].name : M_DARK_NAME); break;
    case PH_BELL: {
        int b = bell_moves();
        if (b == 0)
            out_str("now");
        else {
            out_str("in ");
            out_num(b);
            out_str(" min");
        }
        break;
    }
    case PH_CHOICES:
        for (uint8_t i = 0; i < ctx_list_n; i++) {
            if (i)
                text(i + 1 == ctx_list_n ? M_OR : M_COMMA);
            name_def(ctx_list[i], 0);
        }
        break;
    case PH_NAME: text(ctx_name); break;
    case PH_IS: out_str(is_plural(ctx1) ? "are" : "is"); break;
    case PH_IS_C: out_str(is_plural(ctx1) ? "Are" : "Is"); break;
    case PH_PREP:
        if (ctx_prep == 0xFF)
            break;
        {
            static const char *const preps[] = { "", "in", "on", "at", "to", "with", "about", "from", "under", "off" };
            out_str(preps[ctx_prep < 10 ? ctx_prep : 0]);
        }
        break;
    }
}

/* The characters a byte stands for: ASCII, or Windows-1252 / Latin-1 above. */
static const uint16_t cp1252[32] = {
    0x20AC, 0xFFFD, 0x201A, 0xFFFD, 0xFFFD, 0x2026, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD,
    0xFFFD, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD, 0xFFFD, 0x2212,
};
static uint16_t code_point(uint8_t b) { return b < 0x80 ? b : b < 0xA0 ? cp1252[b - 0x80] : b; }

/* Writes text `id`: pairs expanded, escapes read raw. */
static void text(uint16_t id)
{
    if (!id)
        return;
    const uint8_t *p = text_data + text_off[id];
    uint8_t stack[48];
    for (;;) {
        uint8_t c = *p++;
        if (c == 0)
            break;
        if (c == 1) {
            out_ch(*p++);
            continue;
        }
        if (c == 2) {
            placeholder(*p++);
            continue;
        }
        uint8_t sp = 0;
        stack[sp++] = c;
        while (sp) {
            uint8_t b = stack[--sp];
            uint8_t k = token_of[b];
            if (k == 255)
                out_ch(b);
            else if (sp < sizeof stack - 2) {
                stack[sp++] = text_tokens[k][1];
                stack[sp++] = text_tokens[k][0];
            }
        }
    }
}

/* A message or text on its own line(s). */
static void say(uint16_t id)
{
    out_nl();
    text(id);
    out_nl();
}

static void say1(uint16_t id, uint8_t o)
{
    ctx1 = o;
    say(id);
}

static void say2(uint16_t id, uint8_t o, uint8_t o2)
{
    ctx1 = o;
    ctx2 = o2;
    say(id);
}

/* --- The world ------------------------------------------------------------------------------ */

static uint8_t flag(uint8_t f) { return bit(G.flags, f); }

/* The place an object is in, through open containers and supporters: held,
 * worn, a room, nowhere, or 0xFE if a closed container (or a person) holds
 * it out of reach. */
static uint8_t reach(uint8_t o)
{
    uint8_t l = G.loc[o];
    for (uint8_t guard = 0; IS_OBJ(l) && guard < 12; guard++) {
        uint8_t c = l - L_OBJ(0);
        if (objs[c].kinds & K_NPC)
            return 0xFE;
        if ((objs[c].kinds & K_CONTAINER) && !(G.st[c] & S_OPEN) && !(objs[c].kinds & K_TRANSPARENT))
            return 0xFE;
        l = G.loc[c];
    }
    return l;
}

static uint8_t carried(uint8_t o)
{
    uint8_t l = reach(o);
    return l == L_HELD || l == L_WORN;
}

/* Whether the place can be seen: it is not dark, or a light is here (with
 * `beams` 0, a light other than the lantern's beam). */
static int lit_by(uint8_t beams)
{
    if (!rooms[G.room].dark)
        return 1;
    for (uint8_t o = 0; o < N_OBJS; o++)
        if ((objs[o].kinds & K_LIGHT) && (G.st[o] & S_ON) && (beams || !(objs[o].kinds & K_BEAM))) {
            uint8_t l = reach(o);
            if (l == L_HELD || l == L_WORN || l == L_ROOM(G.room))
                return 1;
        }
    return 0;
}

static int lit(void) { return lit_by(1); }

static uint8_t here(uint8_t o)
{
    uint8_t l = reach(o);
    if (l == L_HELD || l == L_WORN)
        return 1;
    if (!lit())
        return 0;
    if (l == L_ROOM(G.room))
        return 1;
    return G.loc[o] != L_NOWHERE && objs[o].also == L_ROOM(G.room);
}

/* The lit beam the player holds, or 0xFF. */
static uint8_t beam_obj(void)
{
    for (uint8_t o = 0; o < N_OBJS; o++)
        if ((objs[o].kinds & K_BEAM) && (G.st[o] & S_ON) && carried(o))
            return o;
    return 0xFF;
}

static uint8_t room_of(uint8_t o)
{
    uint8_t l = G.loc[o];
    for (uint8_t guard = 0; IS_OBJ(l) && guard < 12; guard++)
        l = G.loc[l - L_OBJ(0)];
    if (l == L_HELD || l == L_WORN)
        return G.room;
    return IS_ROOM(l) ? l - 3 : 0xFF;
}

/* Whether an NPC is in the player's room or one an exit leads to. */
static int hears(uint8_t o)
{
    uint8_t r = room_of(o);
    if (r == 0xFF)
        return 0;
    if (r == G.room)
        return 1;
    for (const uint8_t *e = exits_data + rooms[G.room].exits; *e != 0xFF; e += 8)
        if (e[1] == r)
            return 1;
    for (const uint8_t *e = exits_data + rooms[r].exits; *e != 0xFF; e += 8)
        if (e[1] == G.room)
            return 1;
    return 0;
}

static int16_t counter(uint8_t i)
{
    switch (i) {
    case 0: return G.moves;
    case 2: return now_minutes();
    case 3: return bell_moves();
    case 4: return now_minutes() % 60;
    default: return G.counters[i];
    }
}

static int cond(uint16_t off)
{
    if (!off)
        return 1;
    const uint8_t *p = code + off;
    for (;;) {
        uint8_t op = *p++, neg = op & 0x80, v;
        op &= 0x7F;
        switch (op) {
        case C_END: return 1;
        case C_FLAG: v = flag(*p++); break;
        case C_IN: v = G.room == *p++; break;
        case C_CARRYING: v = carried(*p++); break;
        case C_HERE: v = here(*p++); break;
        case C_OBJIN: v = G.loc[p[0]] == p[1]; p += 2; break;
        case C_OPEN: v = (G.st[*p++] & S_OPEN) != 0; break;
        case C_LOCKED: v = (G.st[*p++] & S_LOCKED) != 0; break;
        case C_ON: v = (G.st[*p++] & S_ON) != 0; break;
        case C_WORN: v = G.loc[*p++] == L_WORN; break;
        case C_VISITED: v = bit(G.visited, *p++); break;
        case C_DARK: v = !lit(); break;
        case C_HEARS: v = hears(*p++); break;
        case C_RANDOM: v = rnd(100) < *p++; break;
        case C_BEAM: v = beam_obj() != 0xFF; break;
        case C_CMP: {
            int16_t a = counter(p[0]), b = (int16_t)rd16(p + 2);
            switch (p[1]) {
            case 0: v = a == b; break;
            case 1: v = a != b; break;
            case 2: v = a < b; break;
            case 3: v = a <= b; break;
            case 4: v = a > b; break;
            default: v = a >= b; break;
            }
            p += 4;
            break;
        }
        default: return 0;
        }
        if (!v != !neg)
            continue;
        return 0;
    }
}

/* --- Sound (device 6, optional) -------------------------------------------------------------- */

enum { I_BELL, I_CHIME, I_TICK, I_SWISH, I_HORN, I_THUD, I_TONE, N_INST };
/* Each instrument's voice (the bell also uses voice 1 to move its phase). */
static const uint8_t inst_voice[N_INST] = { 0, 2, 3, 4, 5, 6, 7 };
static uint8_t snd_buf[512];
static struct {
    const uint8_t *note;
    uint8_t left, frames;
} player[N_INST];
static uint32_t amb_next;

static void sound_init(void)
{
    static int8_t wave[64];
    has_sound = terse_has_device(6);
    if (!has_sound)
        return;
    for (int i = 0; i < 64; i++) /* triangle */
        wave[i] = (int8_t)(i < 32 ? i * 8 - 128 : 383 - i * 8);
    snd_load_wave(0, wave);
    for (int i = 0; i < 64; i++) /* a softened square */
        wave[i] = i < 32 ? 90 : -90;
    snd_load_wave(1, wave);
    for (int i = 0; i < 64; i++) { /* a sine, from the triangle, rounded */
        int t = i < 32 ? i * 8 - 128 : 383 - i * 8;
        wave[i] = (int8_t)(t - t * t * t / 50000);
    }
    snd_load_wave(2, wave);
    snd_begin(snd_buf, sizeof snd_buf);
    /* Bell: voice 0 (heard) moved by voice 1 (muted), long ring. */
    snd_voice(0), snd_wave(2), snd_volume(200), snd_pan(220, 220), snd_envelope(0xFFFF, 50, 0, 300), snd_modulator(1, 80);
    snd_voice(1), snd_wave(2), snd_volume(255), snd_mix(0), snd_envelope(0xFFFF, 110, 0, 600);
    snd_voice(2), snd_wave(0), snd_volume(130), snd_pan(200, 240), snd_envelope(0xFFFF, 500, 0, 700);
    snd_voice(3), snd_noise(), snd_volume(70), snd_pan(220, 220), snd_envelope(0xFFFF, 3000, 0, 3000);
    snd_voice(4), snd_noise(), snd_volume(40), snd_pan(240, 200), snd_envelope(120, 40, 0, 60);
    snd_voice(5), snd_wave(1), snd_volume(90), snd_pan(220, 220), snd_envelope(0x0600, 30, 200, 500);
    snd_voice(6), snd_noise(), snd_volume(170), snd_pan(230, 230), snd_envelope(0xFFFF, 900, 0, 900);
    snd_voice(7), snd_wave(1), snd_volume(80), snd_pan(220, 220), snd_envelope(0x4000, 400, 120, 900);
    snd_submit_list(snd_buf);
}

static void sound_play(uint8_t cue)
{
    if (!has_sound || cue >= N_SOUNDS)
        return;
    const uint8_t *p = sound_data + sound_off[cue];
    uint8_t inst = p[0];
    if (inst >= N_INST)
        return;
    player[inst].note = p + 2;
    player[inst].left = p[1];
    player[inst].frames = 0;
}

static void sound_frame(void)
{
    if (!has_sound)
        return;
    snd_begin(snd_buf, sizeof snd_buf);
    for (uint8_t i = 0; i < N_INST; i++) {
        if (!player[i].note)
            continue;
        if (player[i].frames) {
            player[i].frames--;
            continue;
        }
        uint8_t v = inst_voice[i];
        if (!player[i].left) {
            snd_voice(v);
            snd_key_off();
            if (i == I_BELL)
                snd_voice(1), snd_key_off();
            player[i].note = 0;
            continue;
        }
        uint32_t inc = rd16(player[i].note) | rd16(player[i].note + 2) << 16;
        player[i].frames = player[i].note[4] - 1;
        player[i].note += 5;
        player[i].left--;
        snd_voice(v);
        if (inc == 0) {
            snd_key_off();
            continue;
        }
        if (i == I_TICK || i == I_SWISH || i == I_THUD) {
            uint32_t x = inc << 5;
            inc = x < inc ? 0xF0000000u : x;
        }
        snd_frequency(inc);
        snd_key_on();
        if (i == I_BELL) {
            snd_voice(1);
            snd_frequency(inc + (inc >> 1) + (inc >> 3) * 7); /* about 2.4 times: a bell's clang */
            snd_key_on();
        }
    }
    /* The room's ambient sound. */
    const struct room *r = &rooms[G.room];
    if (r->amb != 0xFF && r->amb_every && !G.ended) {
        if (tick >= amb_next) {
            if (amb_next)
                sound_play(r->amb);
            amb_next = tick + r->amb_every;
        }
    }
    if (snd_length(snd_buf))
        snd_submit_list(snd_buf);
}

/* --- Objects listed --------------------------------------------------------------------------------- */

static void list_print(void)
{
    if (!ctx_list_n) {
        text(M_NOTHING);
        return;
    }
    for (uint8_t i = 0; i < ctx_list_n; i++) {
        uint8_t o = ctx_list[i];
        if (i)
            text(i + 1 == ctx_list_n ? M_AND : M_COMMA);
        name_indef(o, 0);
        if (ctx_list_mode) {
            if (G.loc[o] == L_WORN)
                text(M_WORN_SUFFIX);
            if ((objs[o].kinds & K_LIGHT) && (G.st[o] & S_ON))
                text(M_LIT_SUFFIX);
            if ((objs[o].kinds & K_CONTAINER) && (G.st[o] & S_OPEN) && ctx_list_mode == 2)
                text(M_OPEN_SUFFIX);
        }
    }
}

static uint8_t gather(uint8_t loc, uint8_t skip_scenery)
{
    ctx_list_n = 0;
    for (uint8_t o = 0; o < N_OBJS; o++)
        if (G.loc[o] == loc && !(skip_scenery && (objs[o].kinds & (K_SCENERY | K_NPC))))
            ctx_list[ctx_list_n++] = o;
    return ctx_list_n;
}

/* --- Effects -------------------------------------------------------------------------------- */

static uint8_t noise;
static int16_t move_minutes = -1;   /* set by a rule's `minutes` */
static void look(void);
/* A description of the place left for the next frame (1), and its
 * arrival events (2): after an undo, a restore or a restart, whose text
 * fills a frame already. */
static uint8_t pending_look;
static void game_over(uint8_t how, uint8_t pic, uint16_t id);
static void aim_at(uint8_t o);
static void beam_check(void);
static void events_now(void);
static void enter_events(void);

static void move_obj(uint8_t o, uint8_t to)
{
    uint8_t from_here = here(o), npc = (objs[o].kinds & K_NPC) != 0;
    if (npc && from_here) {
        uint8_t r = IS_ROOM(to) ? to - 3 : 0xFF;
        if (r != G.room) {
            G.loc[o] = to;
            if (objs[o].leaves)
                say1(objs[o].leaves, o);
            else
                say1(M_LEAVES, o);
            dirty_scene = 1;
            return;
        }
    }
    G.loc[o] = to;
    if (npc && !from_here && here(o)) {
        if (objs[o].arrives)
            say1(objs[o].arrives, o);
        else
            say1(M_ARRIVES, o);
    }
    if (to != L_HELD && to != L_WORN && o == G.it && !here(o))
        G.it = 0xFF;
    dirty_scene = 1;
}

static void go_room(uint8_t r)
{
    G.room = r;
    dirty_scene = 1;
    dirty_status = 1;
    amb_next = 0;
    /* The beam starts in the middle of each new place. */
    G.beam_x = 640;
    G.beam_y = 300;
    look();
    enter_events();
}

static void try_action(uint8_t a, uint8_t o1, uint8_t o2);

static int effects(uint16_t off)
{
    int cont = 0;
    if (!off)
        return 0;
    const uint8_t *p = code + off;
    for (;;) {
        if (G.ended)
            return 0;
        switch (*p++) {
        case E_END: return cont;
        case E_SAY: say((uint16_t)rd24(p)); p += 3; break;
        case E_SET: bit_set(G.flags, *p++, 1); dirty_scene = 1; break;
        case E_CLEAR: bit_set(G.flags, *p++, 0); dirty_scene = 1; break;
        case E_TOGGLE: bit_set(G.flags, p[0], !flag(p[0])); p++; dirty_scene = 1; break;
        case E_MOVE: move_obj(p[0], p[1]); p += 2; break;
        case E_GO: go_room(*p++); break;
        case E_STATE: {
            uint8_t o = p[0], s = p[1];
            p += 2;
            static const uint8_t mask[] = { S_OPEN, S_OPEN, S_LOCKED, S_LOCKED, S_ON, S_ON };
            if (s % 2 == 0)
                G.st[o] |= mask[s];
            else
                G.st[o] &= ~mask[s];
            dirty_scene = 1;
            break;
        }
        case E_EXIT: {
            uint8_t i;
            for (i = 0; i < G.xover_n; i++)
                if (G.xover[i][0] == p[0] && G.xover[i][1] == p[1])
                    break;
            if (i < 8) {
                G.xover[i][0] = p[0], G.xover[i][1] = p[1], G.xover[i][2] = p[2];
                if (i == G.xover_n)
                    G.xover_n++;
            }
            p += 3;
            break;
        }
        case E_SOUND: sound_play(*p++); break;
        case E_ADD: G.counters[p[0]] += (int16_t)rd16(p + 1); p += 3; dirty_status = 1; break;
        case E_LET: G.counters[p[0]] = (int16_t)rd16(p + 1); p += 3; dirty_status = 1; break;
        case E_SCHEDULE: {
            const struct event *e = &events[p[0]];
            G.timers[e->a] = (uint16_t)rd16(p + 1);
            p += 3;
            break;
        }
        case E_CANCEL: G.timers[events[*p++].a] = 0; break;
        case E_END_GAME: game_over(p[0], p[1], (uint16_t)rd24(p + 2)); return 0;
        case E_LOUD: noise = 1; break;
        case E_CONTINUE: cont = 1; break;
        case E_LOOK: look(); break;
        case E_ACHIEVE: {
            uint8_t a = *p++;
            if (!bit(G.ach, a)) {
                bit_set(G.ach, a, 1);
                G.counters[1] += achievement_points[a];
                ctx_name = achievement_text[a];
                say(M_ACHIEVEMENT);
                dirty_status = 1;
            }
            break;
        }
        case E_AIM: aim_at(*p++); break;
        case E_MINUTES: move_minutes = (int16_t)rd16(p); p += 2; break;
        case E_TRY: try_action(p[0], p[1], p[2]); p += 3; break;
        default: return cont;
        }
    }
}

/* --- Events ------------------------------------------------------------------------------------ */

static void fire(uint8_t i)
{
    if (cond(events[i].cond))
        effects(events[i].eff);
}

/* After each move: the clock's events, timers, conditions, people. */
static void events_move(uint16_t prev)
{
    for (uint8_t i = 0; i < N_EVENTS_REAL && !G.ended; i++) {
        const struct event *e = &events[i];
        switch (e->kind) {
        case T_ATTIME:
            if (prev < e->a && e->a <= G.minutes && !bit(G.fired, i)) {
                bit_set(G.fired, i, 1);
                fire(i);
            }
            break;
        case T_EVERYTIME:
            if (G.minutes >= e->b && e->b + (G.minutes - e->b) / e->a * e->a > prev)
                fire(i);
            break;
        case T_AT:
            if (G.moves == e->a && !bit(G.fired, i)) {
                bit_set(G.fired, i, 1);
                fire(i);
            }
            break;
        case T_EVERY:
            if (G.moves >= e->b && (G.moves - e->b) % e->a == 0)
                fire(i);
            break;
        case T_TIMER:
            if (G.timers[e->a] && --G.timers[e->a] == 0)
                fire(i);
            break;
        case T_WHENEVER:
            if (cond(e->cond))
                effects(e->eff);
            break;
        }
    }
    events_now();
    /* People's idle lines, in turn. */
    for (uint8_t o = 0; o < N_OBJS && !G.ended; o++) {
        if (!(objs[o].kinds & K_NPC) || !here(o))
            continue;
        const uint8_t *p = idle_data + objs[o].idle;
        if (p[0] && p[1] && G.moves % p[0] == 0)
            say1((uint16_t)rd24(p + 2 + 3 * ((G.moves / p[0]) % p[1])), o);
    }
}

/* `when` events: at once, whenever their condition becomes true. */
static void events_now(void)
{
    for (uint8_t again = 0; again < 4; again++) {
        uint8_t any = 0;
        for (uint8_t i = 0; i < N_EVENTS_REAL && !G.ended; i++)
            if (events[i].kind == T_WHEN && !bit(G.fired, i) && cond(events[i].cond)) {
                bit_set(G.fired, i, 1);
                effects(events[i].eff);
                any = 1;
            }
        if (!any)
            break;
    }
}

static void noise_events(void)
{
    while (noise && !G.ended) {
        noise = 0;
        for (uint8_t i = 0; i < N_EVENTS_REAL && !G.ended; i++)
            if (events[i].kind == T_NOISE)
                fire(i);
    }
}

static void enter_events(void)
{
    for (uint8_t i = 0; i < N_EVENTS_REAL && !G.ended; i++)
        if (events[i].kind == T_ENTER && events[i].a == G.room)
            fire(i);
}

/* --- Describing --------------------------------------------------------------------------------- */

static void contents_lines(uint8_t o)
{
    if (!((objs[o].kinds & K_SUPPORTER) || ((objs[o].kinds & K_CONTAINER) && (G.st[o] & S_OPEN))))
        return;
    if (!gather(L_OBJ(o), 1))
        return;
    ctx1 = o;
    ctx_list_mode = 0;
    say((objs[o].kinds & K_SUPPORTER) ? M_ON_LIST : M_IN_LIST);
}

static void look(void)
{
    const struct room *r = &rooms[G.room];
    out_nl();
    if (!lit()) {
        curcol = C_TITLE;
        text(M_DARK_NAME);
        out_nl();
        curcol = C_TEXT;
        say(M_DARK_ROOM);
        return;
    }
    bit_set(G.visited, G.room, 1);
    curcol = C_TITLE;
    text(r->name);
    out_nl();
    curcol = C_TEXT;
    text(r->desc);
    for (const uint8_t *n = notes_data + r->notes; rd16(n); n += 5)
        if (cond(rd16(n))) {
            out_ch(' ');
            text((uint16_t)rd24(n + 2));
        }
    out_nl();
    /* Objects with their own first description, then the rest. */
    uint8_t rest[N_OBJS], nrest = 0;
    for (uint8_t o = 0; o < N_OBJS; o++) {
        if (G.loc[o] != L_ROOM(G.room) || (objs[o].kinds & K_SCENERY))
            continue;
        if (objs[o].kinds & K_NPC) {
            if (objs[o].here)
                say1(objs[o].here, o);
            else
                say1(M_NPC_HERE, o);
        } else if (objs[o].initial && !(G.st[o] & S_MOVED))
            say(objs[o].initial);
        else
            rest[nrest++] = o;
    }
    if (nrest) {
        memcpy(ctx_list, rest, nrest);
        ctx_list_n = nrest;
        ctx_list_mode = 0;
        say(M_HERE_LIST);
    }
    for (uint8_t o = 0; o < N_OBJS; o++)
        if (G.loc[o] == L_ROOM(G.room) && !(objs[o].kinds & K_NPC))
            contents_lines(o);
}

/* --- The beam ---------------------------------------------------------------------------------- */

/* Where an object is drawn (and the beam aims), in half units: 1 if it is
 * in the scene now. */
static void point_at(const uint8_t *p, int32_t *x, int32_t *y)
{
    uint32_t v = rd24(p);
    *x = (int32_t)(v << 20) >> 20;
    *y = (int32_t)(v << 8) >> 20;
}

/* Until the player moves it, a thing is drawn where the story put it. */
static int at_home(uint8_t o) { return objs[o].x >= 0 && !(G.st[o] & S_MOVED); }

static int obj_pos(uint8_t o, int16_t *x, int16_t *y)
{
    uint8_t l = G.loc[o];
    if (l == L_ROOM(G.room) || (objs[o].also == L_ROOM(G.room) && l != L_NOWHERE)) {
        if (at_home(o)) {
            *x = objs[o].x, *y = objs[o].y;
            return 1;
        }
        /* Otherwise at the room's spots, in turn. */
        const uint8_t *s = spots_data + rooms[G.room].spots;
        if (!s[0])
            return 0;
        uint8_t k = 0;
        for (uint8_t i = 0; i < o; i++)
            if (G.loc[i] == l && objs[i].art && !at_home(i))
                k++;
        int32_t sx, sy;
        point_at(s + 1 + 3 * (k % s[0]), &sx, &sy);
        *x = sx + 24 * (k / s[0]);
        *y = sy;
        return 1;
    }
    if (IS_OBJ(l)) {
        uint8_t c = l - L_OBJ(0);
        if ((objs[c].kinds & K_SUPPORTER) && obj_pos(c, x, y)) {
            uint8_t k = 0;
            for (uint8_t i = 0; i < o; i++)
                if (G.loc[i] == l)
                    k++;
            *x += objs[c].tx + 28 * k;
            *y += objs[c].ty;
            return 1;
        }
    }
    return 0;
}

static void aim_at(uint8_t o)
{
    int16_t x, y;
    if (o < N_OBJS && obj_pos(o, &x, &y)) {
        G.beam_x = x;
        G.beam_y = y;
        beam_check();
    }
}

/* --- Ending ------------------------------------------------------------------------------------ */

static void game_over(uint8_t how, uint8_t pic, uint16_t id)
{
    say(id);
    G.ended = how;
    G.picture = pic;
    out_blank();
    curcol = C_TITLE;
    say(how == 1 ? M_END_WON : M_END_LOST);
    curcol = C_TEXT;
    if (MAX_SCORE)
        say(M_SCORE);
    say(M_END_PROMPT);
    dirty_scene = 1;
    dirty_status = 1;
}

/* --- The scene ------------------------------------------------------------------------------------ */

static uint8_t vcol[3];
static int16_t pen_x = -32000, pen_y;

static void vec_rgb(uint8_t r, uint8_t g, uint8_t b)
{
    if (vcol[0] != r || vcol[1] != g || vcol[2] != b) {
        vec_colour(r, g, b, 255);
        vcol[0] = r, vcol[1] = g, vcol[2] = b;
    }
}

/* A line from a to b in sixteenths, joined to the last if it starts there. */
static void seg_solid(int16_t ax, int16_t ay, int16_t bx, int16_t by)
{
    if (ax != pen_x || ay != pen_y)
        vec_move(ax, ay);
    vec_line(bx, by);
    pen_x = bx, pen_y = by;
}

/* Lines the beam reveals are drawn dashed: 5 units on, 3 off, the dash
 * pattern running on from one segment to the next (dash_at). */
#define DASH_ON (5 * 16)
#define DASH_PERIOD (8 * 16)
static uint8_t dashing;
static int32_t dash_at, dash_x = -1, dash_y = -1;
static uint32_t isqrt32(uint32_t v);

static void seg(int16_t ax, int16_t ay, int16_t bx, int16_t by)
{
    if (!dashing) {
        seg_solid(ax, ay, bx, by);
        return;
    }
    int32_t dx = bx - ax, dy = by - ay;
    int32_t len = (int32_t)isqrt32((uint32_t)(dx * dx + dy * dy));
    if (len == 0) {
        seg_solid(ax, ay, bx, by);
        return;
    }
    if (ax != dash_x || ay != dash_y)
        dash_at = 0;
    dash_x = bx, dash_y = by;
    for (int32_t s = 0; s < len;) {
        int32_t ph = dash_at % DASH_PERIOD, step;
        if (ph < DASH_ON) {
            step = DASH_ON - ph;
            if (s + step > len)
                step = len - s;
            seg_solid(ax + dx * s / len, ay + dy * s / len, ax + dx * (s + step) / len, ay + dy * (s + step) / len);
        } else {
            step = DASH_PERIOD - ph;
            if (s + step > len)
                step = len - s;
        }
        s += step;
        dash_at += step;
    }
}

/* The beam's circle for clipping, in half units. */
static int32_t clip_x, clip_y, clip_r;

/* The part of (x0, y0)-(x1, y1) (half units) inside the beam: 1 and the
 * ends, or 0. */
static uint32_t isqrt32(uint32_t v)
{
    uint32_t r = 0, b = 1u << 30;
    while (b > v)
        b >>= 2;
    while (b) {
        if (v >= r + b) {
            v -= r + b;
            r = (r >> 1) + b;
        } else
            r >>= 1;
        b >>= 2;
    }
    return r;
}

/* The part of (x0, y0)-(x1, y1) (half units) inside the beam: 1 and the
 * ends, or 0. All in 32 bits: the closest point of the line to the
 * centre, then half the chord either side of it, clamped to the segment. */
static int clip_seg(int32_t x0, int32_t y0, int32_t x1, int32_t y1, int32_t *o)
{
    int32_t r = clip_r, cx = clip_x, cy = clip_y;
    if ((x0 < cx - r && x1 < cx - r) || (x0 > cx + r && x1 > cx + r) || (y0 < cy - r && y1 < cy - r) || (y0 > cy + r && y1 > cy + r))
        return 0;
    int32_t dx = x1 - x0, dy = y1 - y0, fx = x0 - cx, fy = y0 - cy, r2 = r * r;
    int32_t gx = x1 - cx, gy = y1 - cy;
    uint8_t in0 = fx * fx + fy * fy <= r2, in1 = gx * gx + gy * gy <= r2;
    o[0] = x0, o[1] = y0, o[2] = x1, o[3] = y1;
    if (in0 && in1)
        return 1;
    int32_t a = dx * dx + dy * dy;
    if (a == 0)
        return in0;
    int32_t len = (int32_t)isqrt32((uint32_t)a);
    if (len == 0)
        return in0;
    /* s: distance along the segment from (x0, y0), in half units. */
    int32_t sc = -(fx * dx + fy * dy) / len;
    int32_t px = x0 + dx * sc / len - cx, py = y0 + dy * sc / len - cy;
    int32_t d2 = px * px + py * py;
    if (d2 >= r2)
        return 0;
    int32_t h = (int32_t)isqrt32((uint32_t)(r2 - d2));
    int32_t s0 = sc - h, s1 = sc + h;
    if (s0 < 0)
        s0 = 0;
    if (s1 > len)
        s1 = len;
    if (s0 >= s1)
        return 0;
    if (!in0)
        o[0] = x0 + dx * s0 / len, o[1] = y0 + dy * s0 / len;
    if (!in1)
        o[2] = x0 + dx * s1 / len, o[3] = y0 + dy * s1 / len;
    return 1;
}

/* While nonzero: draw only the hidden lines already seen, the first of
 * them numbered only_seen - 1. */
static uint16_t only_seen;

/* Draws the shapes of one layer: `ox, oy` added (half units), `dx, dy`
 * the drift (sixteenths), clipped to the beam if `clip`. Returns the
 * pointer after the layer. */
static const uint8_t *draw_shapes(const uint8_t *p, uint16_t n, int32_t ox, int32_t oy, int32_t dx, int32_t dy, uint8_t clip, uint8_t draw)
{
    for (uint16_t s = 0; s < n; s++) {
        uint8_t np = *p++;
        if (!draw || (only_seen && !bit(G.seen, only_seen - 1 + s))) {
            p += 3 * np;
            continue;
        }
        int32_t x0, y0, x1, y1, c[4];
        point_at(p, &x0, &y0);
        x0 += ox, y0 += oy;
        if (np == 1) {
            if (!clip || clip_seg(x0, y0, x0, y0, c))
                seg(x0 * 8 + dx, y0 * 8 + dy, x0 * 8 + dx, y0 * 8 + dy);
            p += 3;
            continue;
        }
        for (uint8_t k = 1; k < np; k++) {
            point_at(p + 3 * k, &x1, &y1);
            x1 += ox, y1 += oy;
            if (!clip)
                seg(x0 * 8 + dx, y0 * 8 + dy, x1 * 8 + dx, y1 * 8 + dy);
            else if (clip_seg(x0, y0, x1, y1, c))
                seg(c[0] * 8 + dx, c[1] * 8 + dy, c[2] * 8 + dx, c[3] * 8 + dy);
            x0 = x1, y0 = y1;
        }
        p += 3 * np;
    }
    return p;
}

static uint32_t hash32(uint32_t x)
{
    x ^= x >> 16;
    x *= 0x7FEB352Du;
    x ^= x >> 15;
    x *= 0x846CA68Bu;
    x ^= x >> 16;
    return x;
}

/* A layer's animation now: its brightness (out of 256) and drift, and
 * whether it shows; mixes its state into anim_key. */
static uint8_t layer_anim(const uint8_t *h, uint16_t *bright, int32_t *dx, int32_t *dy, uint32_t salt)
{
    *bright = 256, *dx = 0, *dy = 0;
    switch (h[4]) {
    case AN_FLICKER: {
        uint32_t step = (tick >> 2) + salt * 7;
        *bright = 150 + (hash32(step) & 105);
        anim_key = anim_key * 31 + *bright;
        room_anim = 1;
        return 1;
    }
    case AN_DRIFT: {
        int32_t period = h[7] * 8, ph = tick % period, half = period / 2;
        int32_t tri = ph < half ? ph : period - ph;
        /* In whole units: the picture changes only when a line moves one. */
        *dx = (int8_t)h[5] * tri / half * 16;
        *dy = (int8_t)h[6] * tri / half * 16;
        anim_key = anim_key * 31 + (uint32_t)(*dx * 977 + *dy);
        room_anim = 1;
        return 1;
    }
    case AN_BLINK: {
        uint8_t on = (tick % (h[7] + h[8])) < h[7];
        anim_key = anim_key * 31 + on;
        room_anim = 1;
        return on;
    }
    }
    return 1;
}

/* Draws (or, with draw 0, only walks for the animation key) an art block
 * at (ox, oy) half units. mode: 0 lit, 1 dark but seen in the beam. */
static void draw_art(uint32_t off, int32_t ox, int32_t oy, uint8_t mode, uint8_t beam, uint8_t draw, uint32_t salt)
{
    if (!off)
        return;
    const uint8_t *p = art_data + off;
    uint8_t nl = *p++;
    for (uint8_t i = 0; i < nl; i++) {
        const uint8_t *h = p;
        uint16_t n = rd16(h + 18);
        p += LAYER_HEAD;
        uint8_t show = cond(rd16(h + 16));
        uint16_t bright;
        int32_t dx, dy;
        if (show && !layer_anim(h, &bright, &dx, &dy, salt + i))
            show = 0;
        if (!show || !draw) {
            p = draw_shapes(p, n, 0, 0, 0, 0, 0, 0);
            continue;
        }
        uint8_t r = h[1], g = h[2], b = h[3];
        if (h[0] & LT_FAINT)
            r /= 3, g /= 3, b /= 3;
        if (bright != 256)
            r = r * bright >> 8, g = g * bright >> 8, b = b * bright >> 8;
        uint8_t hidden = h[0] & LT_HIDDEN;
        uint8_t revealed = hidden && h[9] != 0xFF && flag(h[9]);
        const uint8_t *start = p;
        if (hidden) {
            /* Lines seen once stay, faint; in the beam they are drawn
             * brighter and wider than the rest of the picture. */
            (void)revealed;
            vec_rgb(r / 3, g / 3, b / 3);
            dashing = 1;
            only_seen = rd16(h + 20) + 1;
            draw_shapes(start, n, ox, oy, dx, dy, 0, 1);
            only_seen = 0;
            if (beam) {
                vec_rgb(r, g, b);
                vec_width(32);
                draw_shapes(start, n, ox, oy, dx, dy, 1, 1);
                vec_width(16);
            }
            dashing = 0;
            p = draw_shapes(start, n, 0, 0, 0, 0, 0, 0);
            continue;
        }
        vec_rgb(r, g, b);
        p = draw_shapes(start, n, ox, oy, dx, dy, mode == 1, 1);
    }
}

static void scene_build(uint8_t draw)
{
    uint8_t beam = beam_obj() != 0xFF && !G.ended;
    uint8_t is_lit = lit();
    anim_key = 0;
    room_anim = 0;
    if (draw) {
        vec_begin(vlist, sizeof vlist - 1);
        vcol[0] = vcol[1] = vcol[2] = 1;
        pen_x = -32000;
        /* The frame round the scene. */
        vec_rgb(70, 70, 80);
        seg(2 * 16, 1 * 16, 638 * 16, 1 * 16);
        vec_line(638 * 16, 255 * 16);
        vec_line(2 * 16, 255 * 16);
        vec_line(2 * 16, 1 * 16);
        pen_x = -32000;
    }
    clip_x = shown_bx, clip_y = shown_by, clip_r = BEAM_R * 2;
    if (G.ended && G.picture != 0xFF) {
        draw_art(picture_art[G.picture], 0, 0, 0, 0, draw, 1);
    } else if (is_lit || beam) {
        /* A dark place lit only by the lantern is seen only in its beam. */
        uint8_t mode = lit_by(0) ? 0 : 1;
        draw_art(rooms[G.room].art, 0, 0, mode, beam, draw, 1);
        for (uint8_t o = 0; o < N_OBJS; o++) {
            int16_t x, y;
            if (objs[o].art && obj_pos(o, &x, &y))
                draw_art(objs[o].art, x, y, mode, beam, draw, 17 + o * 5);
        }
    }
    if (draw && beam) {
        /* The beam: its spot, and the edges of its cone from the lantern. */
        int32_t bx = shown_bx, by = shown_by, r = BEAM_R * 2, lx = 640, ly = 508;
        vec_rgb(90, 75, 40);
        pen_x = -32000;
        vec_move(bx * 8, by * 8);
        vec_circle(r * 8);
        int32_t ddx = bx - lx, ddy = by - ly;
        uint32_t len = isqrt32((uint32_t)(ddx * ddx + ddy * ddy));
        if (len > (uint32_t)r) {
            int32_t px = -ddy * r / (int32_t)len, py = ddx * r / (int32_t)len;
            vec_rgb(45, 38, 20);
            seg(lx * 8, ly * 8, (bx + px) * 8, (by + py) * 8);
            seg(lx * 8, ly * 8, (bx - px) * 8, (by - py) * 8);
        }
    }
    if (draw) {
        /* The line under the status row. */
        vec_rgb(70, 70, 80);
        seg(2 * 16, 273 * 16, 638 * 16, 273 * 16);
        vec_end();
        vec_submit_list(vlist);
    }
}

static uint32_t last_key = 0xFFFFFFFFu;

static void scene_frame(void)
{
    /* The beam glides to where it is aimed. */
    uint8_t beam = beam_obj() != 0xFF && !G.ended;
    if (beam != beam_on_shown) {
        beam_on_shown = beam;
        dirty_scene = 1;
        if (beam) {
            shown_bx = G.beam_x, shown_by = G.beam_y;
            beam_check();
        }
    }
    if (beam && (shown_bx != G.beam_x || shown_by != G.beam_y)) {
        /* The beam is where it was aimed at once: what it reveals is
         * drawn with the reply that tells of it. */
        shown_bx = G.beam_x, shown_by = G.beam_y;
        dirty_scene = 1;
    }
    if (!dirty_scene && !room_anim)
        return;
    if (!dirty_scene) {
        scene_build(0);
        if (anim_key == last_key)
            return;
    }
    scene_build(1);
    last_key = anim_key;
    dirty_scene = 0;
}

/* Hidden layers the beam now falls on, newly seen: their flags are set and
 * their text said. */
/* Marks the hidden lines of an art block that the beam falls on as seen. */
static uint8_t mark_seen(uint32_t off, int32_t ox, int32_t oy)
{
    if (!off)
        return 0;
    const uint8_t *p = art_data + off;
    uint8_t nl = *p++, any = 0;
    clip_x = G.beam_x, clip_y = G.beam_y, clip_r = BEAM_R * 2;
    for (uint8_t i = 0; i < nl; i++) {
        const uint8_t *h = p;
        uint16_t n = rd16(h + 18), first = rd16(h + 20);
        p += LAYER_HEAD;
        uint8_t live = (h[0] & LT_HIDDEN) && cond(rd16(h + 16));
        for (uint16_t s = 0; s < n; s++) {
            uint8_t np = *p++;
            if (live && !bit(G.seen, first + s)) {
                int32_t x0, y0, x1, y1, c[4];
                point_at(p, &x0, &y0);
                for (uint8_t k = 0; k < np; k++) {
                    point_at(p + 3 * (k + 1 < np ? k + 1 : k), &x1, &y1);
                    if (clip_seg(x0 + ox, y0 + oy, x1 + ox, y1 + oy, c)) {
                        bit_set(G.seen, first + s, 1);
                        any = 1;
                        break;
                    }
                    x0 = x1, y0 = y1;
                }
            }
            p += 3 * np;
        }
    }
    return any;
}

static void beam_check(void)
{
    if (beam_obj() == 0xFF || G.ended)
        return;
    uint8_t found = mark_seen(rooms[G.room].art, 0, 0);
    for (uint8_t o = 0; o < N_OBJS; o++) {
        int16_t x, y;
        if (objs[o].art && obj_pos(o, &x, &y))
            found |= mark_seen(objs[o].art, x, y);
    }
    clip_x = shown_bx, clip_y = shown_by;
    uint32_t off = rooms[G.room].art;
    if (!off)
        return;
    const uint8_t *p = art_data + off;
    uint8_t nl = *p++;
    for (uint8_t i = 0; i < nl; i++) {
        const uint8_t *h = p;
        uint16_t n = rd16(h + 18);
        p = draw_shapes(p + LAYER_HEAD, n, 0, 0, 0, 0, 0, 0);
        if (!(h[0] & LT_HIDDEN) || h[9] == 0xFF || flag(h[9]) || !cond(rd16(h + 16)))
            continue;
        int32_t ax, ay;
        point_at(h + 13, &ax, &ay);
        int32_t dx = ax - G.beam_x, dy = ay - G.beam_y, r = BEAM_R * 2;
        if (dx * dx + dy * dy > r * r)
            continue;
        bit_set(G.flags, h[9], 1);
        uint16_t t = (uint16_t)rd24(h + 10);
        say(t ? t : M_REVEAL);
        found = 2;
    }
    if (found == 1)
        dirty_scene = 1;
    if (found >= 2) {
        events_now();
        dirty_scene = 1;
        dirty_status = 1;
    }
}

/* --- The screen's text -------------------------------------------------------------------------------- */

/* A row of cells, each a word: the character, then the colours (SPEC
 * §terminal.cells, little-endian). */
static uint32_t cells[80];

static void cell(uint8_t col, uint16_t cp, uint8_t fg) { cells[col] = cp | (uint32_t)fg << 16; }

static void row_clear(uint8_t fg)
{
    uint32_t v = ' ' | (uint32_t)fg << 16, *c = cells;
    for (uint32_t *e = c + 80; c < e; c++)
        *c = v;
}

static void row_put(uint8_t col, const uint8_t *s, uint8_t n, uint8_t fg)
{
    if (col + n > 80)
        n = 80 - col;
    uint32_t f = (uint32_t)fg << 16, *c = cells + col;
    for (const uint8_t *e = s + n; s < e; s++)
        *c++ = (*s < 0x80 ? *s : code_point(*s)) | f;
}

static void row_write(uint8_t row) { term_cells_write(row * 80, cells, 80); }

static void draw_text(void)
{
    uint32_t bottom = more ? view_end : nlines;
    bottom = bottom > scroll ? bottom - scroll : 0;
    for (uint8_t r = 0; r < TEXT_ROWS; r++) {
        row_clear(C_TEXT);
        int32_t li = (int32_t)bottom - TEXT_ROWS + r;
        if (li >= 0 && (uint32_t)li + RING >= nlines && (uint32_t)li < nlines) {
            const uint8_t *l = ring[li % RING];
            row_put(1, l + 2, l[0], l[1]);
        }
        row_write(TEXT_TOP + r);
    }
}

static void draw_status(void)
{
    row_clear(C_STATUS);
    to_buf = 1;
    sbuf_len = 0;
    text(TEXT_STATUS);
    to_buf = 0;
    /* Left part || right part. */
    uint8_t split = sbuf_len;
    for (uint8_t i = 0; i + 1 < sbuf_len; i++)
        if (sbuf[i] == '|' && sbuf[i + 1] == '|') {
            split = i;
            break;
        }
    row_put(1, sbuf, split < 78 ? split : 78, C_STATUS);
    if (split + 2 <= sbuf_len) {
        uint8_t n = sbuf_len - split - 2;
        if (n > 70)
            n = 70;
        row_put(79 - n, sbuf + split + 2, n, C_STATUS);
    }
    row_write(STATUS_ROW);
}

static void draw_input(void)
{
    row_clear(C_ECHO);
    if (more) {
        to_buf = 1;
        sbuf_len = 0;
        text(M_MORE);
        to_buf = 0;
        row_put(1, sbuf, sbuf_len, C_MORE);
        row_write(INPUT_ROW);
        term_cmd(TERM_CURSOR_OFF);
        return;
    }
    if (scroll) {
        static const uint8_t s[] = "[scrolled back: Page Down returns]";
        row_put(1, s, sizeof s - 1, C_DIM);
        row_write(INPUT_ROW);
        term_cmd(TERM_CURSOR_OFF);
        return;
    }
    cell(1, '>', C_ECHO);
    row_put(3, input, input_len, C_ECHO);
    row_write(INPUT_ROW);
    term_goto(3 + input_len, INPUT_ROW);
    term_cmd(TERM_CURSOR_ON);
}

static void screen_frame(void)
{
    if (dirty_text) {
        draw_text();
        dirty_text = 0;
    }
    if (dirty_status) {
        draw_status();
        dirty_status = 0;
    }
    if (dirty_input) {
        draw_input();
        dirty_input = 0;
    }
}

/* After a reply: show it from its first line, with "more" if it does not fit. */
static void reply_done(void)
{
    out_nl();
    scroll = 0;
    if (nlines - page_top > TEXT_ROWS) {
        more = 1;
        view_end = page_top + TEXT_ROWS;
    } else
        more = 0;
    dirty_text = dirty_input = dirty_status = 1;
}

static void more_key(void)
{
    view_end += TEXT_ROWS - 1;
    if (view_end >= nlines) {
        more = 0;
        view_end = nlines;
    }
    dirty_text = dirty_input = 1;
}

/* --- The parser -------------------------------------------------------------------------------------- */

static int word_cmp(uint16_t w, const uint8_t *s, uint8_t n)
{
    const char *a = word_chars + words[w].off;
    uint8_t m = words[w].len;
    for (uint8_t i = 0; i < n && i < m; i++)
        if ((uint8_t)a[i] != s[i])
            return (uint8_t)a[i] < s[i] ? -1 : 1;
    return m < n ? -1 : m > n;
}

static uint16_t lookup(const uint8_t *s, uint8_t n)
{
    int32_t lo = 0, hi = N_WORDS - 1;
    while (lo <= hi) {
        int32_t mid = (lo + hi) / 2;
        int c = word_cmp(mid, s, n);
        if (!c)
            return mid;
        if (c < 0)
            lo = mid + 1;
        else
            hi = mid - 1;
    }
    return 0xFFFF;
}

#define T_PERIOD 0xFFFE
#define T_UNKNOWN 0xFFFF

static uint8_t tokenize(void)
{
    nt = 0;
    for (uint8_t i = 0; i < input_len; i++) {
        uint8_t c = input[i];
        lower[i] = c >= 'A' && c <= 'Z' ? c + 32 : c;
    }
    uint8_t i = 0;
    while (i < input_len && nt < MAXT) {
        uint8_t c = lower[i];
        if ((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '\'' || c == '-' || c >= 0x80) {
            uint8_t s = i;
            while (i < input_len && ((lower[i] >= 'a' && lower[i] <= 'z') || (lower[i] >= '0' && lower[i] <= '9') || lower[i] == '\'' || lower[i] == '-' || lower[i] >= 0x80))
                i++;
            tpos[nt] = s;
            tlen[nt] = i - s;
            tw[nt] = lookup(lower + s, i - s);
            nt++;
        } else if (c == '.' || c == ',' || c == ';' || c == '!' || c == '?') {
            if (c == '.' || c == ';') {
                tpos[nt] = i;
                tlen[nt] = 1;
                tw[nt++] = T_PERIOD;
            }
            i++;
        } else
            i++;
    }
    return nt;
}

static uint16_t kinds(uint8_t t) { return tw[t] >= N_WORDS ? 0 : words[tw[t]].kinds; }

/* The command being carried out. */
static uint8_t c_action, c_o1, c_o2, c_dir, c_prep, c_all;
static uint16_t c_topic;

static int obj_has_word(uint16_t list, uint16_t w)
{
    const uint16_t *p = word_lists + list;
    for (uint16_t i = 0; i < p[0]; i++)
        if (p[1 + i] == w)
            return 1;
    return 0;
}

/* Whether the words t0..t1 (articles left out) all name object o, at least
 * one of them as a noun. */
static int np_matches_(uint8_t o, uint8_t t0, uint8_t t1, uint8_t noun)
{
    for (uint8_t t = t0; t < t1; t++) {
        uint16_t w = tw[t], k = kinds(t);
        if (k & WK_ART)
            continue;
        if (obj_has_word(objs[o].nouns, w))
            noun = 1;
        else if (!obj_has_word(objs[o].adjs, w))
            return 0;
    }
    return noun;
}

static int np_matches(uint8_t o, uint8_t t0, uint8_t t1) { return np_matches_(o, t0, t1, 0); }

/* The last content word of a noun phrase, for messages. */
static void np_word(uint8_t t0, uint8_t t1)
{
    for (int t = t1 - 1; t >= t0; t--)
        if (!(kinds(t) & WK_ART)) {
            ctx_word = lower + tpos[t];
            ctx_word_len = tlen[t];
            return;
        }
    ctx_word = lower + tpos[t0];
    ctx_word_len = tlen[t0];
}

#define R_NONE 0xFF
#define R_FAIL 0xFE
#define R_ALL 0xFD
#define R_ASKED 0xFC

static void ask_which(uint8_t slot, uint8_t *c, uint8_t n);

/* Resolves the noun phrase t0..t1 (slot 1 or 2): an object, or R_*. */
static uint8_t resolve(uint8_t t0, uint8_t t1, uint8_t slot)
{
    if (t0 >= t1)
        return R_NONE;
    uint8_t only_art = 1;
    for (uint8_t t = t0; t < t1; t++) {
        uint16_t k = kinds(t);
        if (k & WK_IT) {
            if (G.it < N_OBJS && here(G.it))
                return G.it;
            ctx_word = lower + tpos[t];
            ctx_word_len = tlen[t];
            say(M_IT_UNKNOWN);
            return R_FAIL;
        }
        if (k & WK_ALL)
            return R_ALL;
        if ((k & WK_SEP) && !(k & (WK_NOUN | WK_ADJ))) {
            say(M_ONE_AT_A_TIME);
            return R_FAIL;
        }
        if (!(k & WK_ART))
            only_art = 0;
    }
    if (only_art)
        return R_NONE;
    uint8_t c[N_OBJS], n = 0, exists = 0;
    for (uint8_t o = 0; o < N_OBJS; o++)
        if (np_matches(o, t0, t1)) {
            exists = 1;
            if (here(o))
                c[n++] = o;
        }
    if (n == 0) {
        np_word(t0, t1);
        say(!lit() && exists ? M_TOO_DARK : M_CANT_SEE);
        return R_FAIL;
    }
    if (n > 1) {
        /* Prefer what the verb wants: held things to drop, others to take. */
        uint8_t d[N_OBJS], m = 0;
        uint8_t want_held = c_action == A_DROP || c_action == A_PUT_IN || c_action == A_PUT_ON || c_action == A_WEAR ||
                            c_action == A_GIVE || c_action == A_SHOW || (slot == 2 && (c_action == A_UNLOCK || c_action == A_LOCK));
        uint8_t want_free = c_action == A_TAKE;
        if (want_held || want_free) {
            for (uint8_t i = 0; i < n; i++)
                if (carried(c[i]) == want_held)
                    d[m++] = c[i];
            if (m == 1)
                return d[0];
            if (m > 1) {
                memcpy(c, d, m);
                n = m;
            }
        }
        np_word(t0, t1);
        ask_which(slot, c, n);
        return R_ASKED;
    }
    return c[0];
}

static void ask_which(uint8_t slot, uint8_t *c, uint8_t n)
{
    memcpy(q_cands, c, n);
    q_ncands = n;
    memcpy(ctx_list, c, n);
    ctx_list_n = n;
    say(M_WHICH);
    question = Q_WHICH;
    q_slot = slot;
}

static void save_question(void)
{
    q_action = c_action, q_o1 = c_o1, q_o2 = c_o2, q_dir = c_dir, q_prep = c_prep;
    q_verb_len = 0;
    if (verb_tok != 0xFF) {
        q_verb_len = tlen[verb_tok] < 7 ? tlen[verb_tok] : 7;
        memcpy(q_verb, lower + tpos[verb_tok], q_verb_len);
    }
    q_part_len = 0;
    if (part_tok != 0xFF) {
        q_part_len = tlen[part_tok] < 7 ? tlen[part_tok] : 7;
        memcpy(q_verb + 8, lower + tpos[part_tok], q_part_len);
    }
}

/* --- Actions ------------------------------------------------------------------------------------------- */

static uint8_t held_count(void)
{
    uint8_t n = 0;
    for (uint8_t o = 0; o < N_OBJS; o++)
        n += G.loc[o] == L_HELD;
    return n;
}

static int inside(uint8_t o, uint8_t c)
{
    uint8_t l = L_OBJ(c);
    for (uint8_t guard = 0; guard < 12; guard++) {
        if (l == L_OBJ(o))
            return 1;
        if (!IS_OBJ(l))
            return 0;
        l = G.loc[l - L_OBJ(0)];
    }
    return 0;
}

static int do_take(uint8_t o)
{
    if (G.loc[o] == L_HELD || G.loc[o] == L_WORN) {
        say1(M_ALREADY_HAVE, o);
        return 0;
    }
    if (objs[o].kinds & K_NPC) {
        say1(M_TAKE_NPC, o);
        return 0;
    }
    if (!(objs[o].kinds & K_PORTABLE)) {
        say1(M_FIXED, o);
        return 0;
    }
    if (CARRY_LIMIT && held_count() >= CARRY_LIMIT && !carried(o)) {
        say(M_CARRY_FULL);
        return 0;
    }
    move_obj(o, L_HELD);
    G.st[o] |= S_MOVED;
    say(M_TAKEN);
    return 1;
}

static int do_drop(uint8_t o)
{
    if (G.loc[o] == L_WORN) {
        say1(M_TAKE_OFF_FIRST, o);
        return 0;
    }
    if (!carried(o)) {
        say1(M_NOT_CARRIED, o);
        return 0;
    }
    move_obj(o, L_ROOM(G.room));
    G.st[o] |= S_MOVED;
    say(M_DROPPED);
    return 1;
}

static void open_reveal(uint8_t o)
{
    if ((objs[o].kinds & K_CONTAINER) && gather(L_OBJ(o), 1)) {
        ctx1 = o;
        ctx_list_mode = 0;
        say(M_OPENED_REVEAL);
    } else
        say1(M_OPENED, o);
}

/* The key the player holds for o, or 0xFF. */
static uint8_t held_key(uint8_t o)
{
    uint8_t k = objs[o].key;
    return k < N_OBJS && carried(k) ? k : 0xFF;
}

static int lock_check(uint8_t o, uint8_t *key)
{
    if (!(objs[o].kinds & K_LOCKABLE)) {
        say1(M_NOT_LOCKABLE, o);
        return 0;
    }
    if (*key == R_NONE) {
        *key = held_key(o);
        if (*key == 0xFF) {
            ctx1 = o;
            ctx_prep = P_WITH;
            c_prep = P_WITH;
            save_question();
            q_slot = 2;
            question = Q_WHAT;
            q_ncands = 0;
            say(M_WHAT2);
            return 0;
        }
        ctx2 = *key;
        say(M_WITH_KEY);
    }
    if (objs[o].key != *key) {
        say2(M_WRONG_KEY, o, *key);
        return 0;
    }
    return 1;
}

static void shine(uint8_t target)
{
    if (beam_obj() == 0xFF) {
        say(M_SHINE_NO_BEAM);
        return;
    }
    int16_t x, y;
    if (target < N_OBJS && obj_pos(target, &x, &y)) {
        say1(M_SHINE, target);
        G.beam_x = x;
        G.beam_y = y;
        beam_check();
    } else if (target < N_OBJS)
        say1(M_SHINE, target);
    else
        say(M_SHINE_WHERE);
    dirty_scene = 1;
}

static int topic_text(uint8_t npc, uint16_t w)
{
    const uint8_t *p = topics_data + objs[npc].topics;
    while (p[0]) {
        uint8_t n = p[0];
        for (uint8_t i = 0; i < n; i++)
            if (rd16(p + 1 + 2 * i) == w) {
                say1((uint16_t)rd24(p + 1 + 2 * n), npc);
                return 1;
            }
        p += 1 + 2 * n + 3;
    }
    return 0;
}

static void go_dir(uint8_t d)
{
    const struct room *r = &rooms[G.room];
    for (uint8_t i = 0; i < G.xover_n; i++)
        if (G.xover[i][0] == G.room && G.xover[i][1] == d) {
            if (G.xover[i][2] == 0xFF)
                say(M_NO_EXIT);
            else
                go_room(G.xover[i][2]);
            return;
        }
    for (const uint8_t *e = exits_data + r->exits; *e != 0xFF; e += 8) {
        if (e[0] != d)
            continue;
        uint16_t refuse = (uint16_t)rd24(e + 5);
        if (e[2] != 0xFF && !(G.st[e[2]] & S_OPEN)) {
            say1(M_DOOR_CLOSED, e[2]);
            return;
        }
        if (!cond(rd16(e + 3)) || e[1] == 0xFF) {
            say(refuse ? refuse : M_NO_EXIT);
            return;
        }
        sound_play(STEP_SOUND);
        go_room(e[1]);
        return;
    }
    say(M_NO_EXIT);
}

static uint16_t default_of(uint8_t a) { return action_default[a]; }

/* The built-in behaviour of an action. 1: it was done. */
static int action_default_run(void)
{
    uint8_t o = c_o1, o2 = c_o2;
    ctx1 = o, ctx2 = o2;
    switch (c_action) {
    case A_LOOK: look(); return 1;
    case A_EXAMINE:
        if (objs[o].desc)
            say(objs[o].desc);
        else
            say1(M_EXAMINE, o);
        if (((objs[o].kinds & K_CONTAINER) && (G.st[o] & S_OPEN)) || (objs[o].kinds & K_SUPPORTER))
            contents_lines(o);
        else if ((objs[o].kinds & K_CONTAINER) && (objs[o].kinds & K_OPENABLE))
            say1(M_CONTAINER_CLOSED, o);
        return 1;
    case A_INVENTORY:
        ctx_list_n = 0;
        for (uint8_t i = 0; i < N_OBJS; i++)
            if (G.loc[i] == L_HELD || G.loc[i] == L_WORN)
                ctx_list[ctx_list_n++] = i;
        ctx_list_mode = 1;
        say(ctx_list_n ? M_INV : M_INV_EMPTY);
        ctx_list_mode = 0;
        for (uint8_t i = 0; i < N_OBJS; i++)
            if ((G.loc[i] == L_HELD || G.loc[i] == L_WORN) && ((objs[i].kinds & K_CONTAINER) && (G.st[i] & S_OPEN)))
                contents_lines(i);
        return 1;
    case A_TAKE: return do_take(o);
    case A_DROP: return do_drop(o);
    case A_PUT_IN:
    case A_PUT_ON: {
        uint8_t on = c_action == A_PUT_ON;
        if (!carried(o)) {
            say1(M_NOT_CARRIED, o);
            return 0;
        }
        if (o == o2 || inside(o, o2)) {
            say(M_PUT_SELF);
            return 0;
        }
        if (on && !(objs[o2].kinds & K_SUPPORTER)) {
            say2(M_NOT_SUPPORTER, o, o2);
            return 0;
        }
        if (!on && !(objs[o2].kinds & K_CONTAINER)) {
            say2(M_NOT_CONTAINER, o, o2);
            return 0;
        }
        if (!on && !(G.st[o2] & S_OPEN)) {
            say1(M_CONTAINER_CLOSED, o2);
            return 0;
        }
        if (G.loc[o] == L_WORN) {
            say1(M_TAKE_OFF_FIRST, o);
            return 0;
        }
        move_obj(o, L_OBJ(o2));
        G.st[o] |= S_MOVED;
        say2(on ? M_PUT_ON : M_PUT_IN, o, o2);
        return 1;
    }
    case A_OPEN:
        if (!(objs[o].kinds & K_OPENABLE)) {
            say1(M_NOT_OPENABLE, o);
            return 0;
        }
        if (G.st[o] & S_OPEN) {
            say1(M_ALREADY_OPEN, o);
            return 0;
        }
        if (G.st[o] & S_LOCKED) {
            say1(M_LOCKED, o);
            return 0;
        }
        G.st[o] |= S_OPEN;
        dirty_scene = 1;
        open_reveal(o);
        return 1;
    case A_CLOSE:
        if (!(objs[o].kinds & K_OPENABLE)) {
            say1(M_NOT_OPENABLE, o);
            return 0;
        }
        if (!(G.st[o] & S_OPEN)) {
            say1(M_ALREADY_CLOSED, o);
            return 0;
        }
        G.st[o] &= ~S_OPEN;
        dirty_scene = 1;
        say1(M_CLOSED, o);
        return 1;
    case A_UNLOCK:
        if (!lock_check(o, &c_o2))
            return 0;
        if (!(G.st[o] & S_LOCKED)) {
            say1(M_ALREADY_UNLOCKED, o);
            return 0;
        }
        G.st[o] &= ~S_LOCKED;
        say1(M_UNLOCKED, o);
        return 1;
    case A_LOCK:
        if (!lock_check(o, &c_o2))
            return 0;
        if (G.st[o] & S_LOCKED) {
            say1(M_ALREADY_LOCKED, o);
            return 0;
        }
        if (G.st[o] & S_OPEN) {
            say1(M_LOCK_OPEN, o);
            return 0;
        }
        G.st[o] |= S_LOCKED;
        say1(M_LOCKED_NOW, o);
        return 1;
    case A_WEAR:
        if (!(objs[o].kinds & K_WEARABLE)) {
            say1(M_NOT_WEARABLE, o);
            return 0;
        }
        if (G.loc[o] == L_WORN) {
            say1(M_ALREADY_WORN, o);
            return 0;
        }
        if (!carried(o) && !do_take(o))
            return 0;
        move_obj(o, L_WORN);
        G.st[o] |= S_MOVED;
        say1(M_WORN, o);
        return 1;
    case A_REMOVE:
        if (G.loc[o] != L_WORN) {
            if (IS_OBJ(G.loc[o]) && !carried(o)) /* remove X: take it out */
                return do_take(o);
            say1(M_NOT_WORN, o);
            return 0;
        }
        move_obj(o, L_HELD);
        say1(M_REMOVED, o);
        return 1;
    case A_TURN_ON:
        if (!(objs[o].kinds & K_SWITCHABLE)) {
            say1(M_NOT_SWITCHABLE, o);
            return 0;
        }
        if (G.st[o] & S_ON) {
            say1(M_ALREADY_ON, o);
            return 0;
        }
        {
            uint8_t was_lit = lit();
            G.st[o] |= S_ON;
            dirty_scene = 1;
            say1(M_SWITCHED_ON, o);
            if (!was_lit && lit())
                look();
        }
        return 1;
    case A_TURN_OFF:
        if (!(objs[o].kinds & K_SWITCHABLE)) {
            say1(M_NOT_SWITCHABLE, o);
            return 0;
        }
        if (!(G.st[o] & S_ON)) {
            say1(M_ALREADY_OFF, o);
            return 0;
        }
        G.st[o] &= ~S_ON;
        dirty_scene = 1;
        say1(M_SWITCHED_OFF, o);
        return 1;
    case A_READ:
        if (objs[o].read)
            say(objs[o].read);
        else
            say1(M_READ_NOTHING, o);
        return 1;
    case A_GO: go_dir(c_dir); return 1;
    case A_TALK:
        if (!(objs[o].kinds & K_NPC)) {
            say1(M_NOT_NPC, o);
            return 0;
        }
        say1(objs[o].talk ? objs[o].talk : M_TALK_DEFAULT, o);
        return 1;
    case A_ASK:
    case A_TELL:
        if (!(objs[o].kinds & K_NPC)) {
            say1(M_NOT_NPC, o);
            return 0;
        }
        if (c_topic != 0xFFFF && topic_text(o, c_topic))
            return 1;
        say1(objs[o].unknown ? objs[o].unknown : M_ASK_UNKNOWN, o);
        return 1;
    case A_GIVE:
    case A_SHOW:
        if (!(objs[o2].kinds & K_NPC)) {
            say2(M_GIVE_NOT_NPC, o, o2);
            return 0;
        }
        if (!carried(o)) {
            say1(M_NOT_CARRIED, o);
            return 0;
        }
        say2(c_action == A_GIVE ? M_GIVE_DEFAULT : M_SHOW_DEFAULT, o, o2);
        return 1;
    case A_SHINE:
        /* shine lantern at X, shine at X, shine X (not a light). */
        if (o2 < N_OBJS)
            shine(o2);
        else if (o < N_OBJS && !(objs[o].kinds & K_BEAM))
            shine(o);
        else
            shine(0xFF);
        return 1;
    case A_SEARCH:
        if (((objs[o].kinds & K_CONTAINER) && !(G.st[o] & S_OPEN)) && (objs[o].kinds & K_OPENABLE)) {
            say1(M_CONTAINER_CLOSED, o);
            return 0;
        }
        if (objs[o].kinds & (K_CONTAINER | K_SUPPORTER)) {
            if (gather(L_OBJ(o), 1)) {
                ctx1 = o;
                say((objs[o].kinds & K_SUPPORTER) ? M_SEARCH_ON : M_SEARCH_IN);
            } else
                say1(M_SEARCH_EMPTY, o);
            return 1;
        }
        say1(default_of(A_SEARCH), o);
        return 1;
    case A_WAIT:
        say(M_WAIT);
        return 1;
    default:
        say1(default_of(c_action), o);
        return 1;
    }
}

/* Rules for the command: 1 if one took its place. */
static int rules_run(uint8_t kind)
{
    uint8_t ran = 0;
    for (uint16_t i = 0; i < N_RULES; i++) {
        const struct rule *r = &rules[i];
        if (r->kind != kind || r->action != c_action)
            continue;
        if (c_action == A_GO) {
            if (r->o1 != 0xFE && r->o1 != 0xFD && r->o1 != c_dir)
                continue;
        } else {
            if (r->o1 < 0xFD && r->o1 != c_o1)
                continue;
            if (r->o2 < 0xFD && r->o2 != c_o2)
                continue;
        }
        if (!cond(r->cond))
            continue;
        ctx1 = c_o1, ctx2 = c_o2;
        int cont = effects(r->eff);
        ran = 1;
        if (kind == 0)
            return cont ? 0 : 1;
    }
    return ran;
}

static uint8_t moved_now;

/* A rule's `try`: another command carried out in this one's place. */
static void try_action(uint8_t a, uint8_t o1, uint8_t o2)
{
    static uint8_t depth;
    if (depth > 3)
        return;
    depth++;
    uint8_t sa = c_action, s1 = c_o1, s2 = c_o2;
    c_action = a, c_o1 = o1, c_o2 = o2;
    if (!rules_run(0) && !G.ended && action_default_run() && !G.ended)
        rules_run(1);
    c_action = sa, c_o1 = s1, c_o2 = s2;
    depth--;
}
static uint16_t spend;              /* the minutes the commands of this line take */

/* The whole command, once its objects are known. */
static void perform(void)
{
    uint8_t f = action_flags[c_action];
    move_minutes = -1;
    if (c_o1 < N_OBJS)
        G.it = c_o1;
    if ((f & AF_SIGHT) && !lit()) {
        say(M_NEED_LIGHT);
    } else if (!rules_run(0)) {
        if (!G.ended && action_default_run() && !G.ended)
            rules_run(1);
    }
    noise_events();
    /* Looking and examining take no time; the rest what the story says. */
    uint16_t m = move_minutes >= 0 ? (uint16_t)move_minutes : action_minutes[c_action];
    if (!(f & AF_META) && m) {
        moved_now = 1;
        spend += m;
    }
}

/* take all / drop all */
static void perform_all(void)
{
    uint8_t list[N_OBJS], n = 0;
    for (uint8_t o = 0; o < N_OBJS; o++) {
        if (c_action == A_TAKE) {
            uint8_t l = G.loc[o];
            if (!(objs[o].kinds & K_PORTABLE) || carried(o) || !here(o))
                continue;
            if (l != L_ROOM(G.room) && !IS_OBJ(l)) /* in the room, or on or in something there */
                continue;
        } else if (G.loc[o] != L_HELD)
            continue;
        list[n++] = o;
    }
    if (!n) {
        say(c_action == A_TAKE ? M_NOTHING_ALL : M_INV_EMPTY);
        return;
    }
    for (uint8_t i = 0; i < n && !G.ended; i++) {
        out_nl();
        name_def(list[i], 1);
        out_str(": ");
        c_o1 = list[i];
        ctx1 = c_o1;
        if (!rules_run(0)) {
            if (c_action == A_TAKE ? do_take(c_o1) : do_drop(c_o1))
                rules_run(1);
        }
    }
    moved_now = 1;
    spend += action_minutes[c_action];
}

static void need_object(uint8_t slot)
{
    save_question();
    q_slot = slot;
    q_ncands = 0;
    question = Q_WHAT;
    ctx1 = c_o1;
    if (slot == 1)
        say(M_WHAT1);
    else {
        static const uint8_t def_prep[N_BUILTIN_ACTIONS] = { [A_PUT_IN] = P_IN, [A_PUT_ON] = P_ON, [A_GIVE] = P_TO, [A_SHOW] = P_TO,
                                                             [A_LOCK] = P_WITH, [A_UNLOCK] = P_WITH, [A_ASK] = P_ABOUT, [A_TELL] = P_ABOUT };
        ctx_prep = c_action < N_BUILTIN_ACTIONS ? def_prep[c_action] : P_WITH;
        if (c_prep)
            ctx_prep = c_prep;
        say(M_WHAT2);
    }
}

/* Checks the objects and carries the command out. */
static void dispatch(void)
{
    uint8_t f = action_flags[c_action];
    if (c_o1 == R_ALL) {
        if (c_action != A_TAKE && c_action != A_DROP) {
            say(M_ALL_BAD);
            return;
        }
        perform_all();
        return;
    }
    if (c_action == A_GO && c_dir == 0xFF) {
        say(M_GO_WHERE);
        return;
    }
    if (c_action == A_SHINE) {
        perform();
        return;
    }
    if ((f & AF_NOUN) && c_o1 == R_NONE) {
        need_object(1);
        return;
    }
    if ((f & AF_SECOND) && c_o2 == R_NONE && c_action != A_LOCK && c_action != A_UNLOCK) {
        need_object(2);
        return;
    }
    if ((f & AF_TOPIC) && c_topic == 0xFFFF) {
        need_object(2);
        return;
    }
    if (c_action == A_ASK || c_action == A_TELL)
        c_o2 = R_NONE;
    perform();
}

/* --- Meta commands ------------------------------------------------------------------------------------- */

static void undo_push(void)
{
    undo_ring[undo_head] = G;
    undo_head = (undo_head + 1) % UNDO_N;
    if (undo_n < UNDO_N)
        undo_n++;
}

static void refresh_all(void)
{
    dirty_scene = dirty_status = dirty_text = dirty_input = 1;
    beam_on_shown = 0xFF;
    shown_bx = G.beam_x, shown_by = G.beam_y;
}

static void do_undo(void)
{
    if (!undo_n) {
        say(M_UNDO_NONE);
        return;
    }
    undo_head = (undo_head + UNDO_N - 1) % UNDO_N;
    undo_n--;
    G = undo_ring[undo_head];
    say(M_UNDONE);
    pending_look = 1;
    refresh_all();
}

static void game_init(void)
{
    memset(&G, 0, sizeof G);
    G.magic = GAME_MAGIC;
    G.story = STORY_ID;
    G.room = START_ROOM;
    G.picture = 0xFF;
    G.it = 0xFF;
    G.rng = 0x2545F491u;
    G.beam_x = 640, G.beam_y = 300;
    memcpy(G.flags, flag_init, sizeof flag_init < FLAG_BYTES ? sizeof flag_init : FLAG_BYTES);
    for (uint8_t i = 0; i < N_COUNTERS; i++)
        G.counters[i] = counter_init[i];
    for (uint8_t o = 0; o < N_OBJS; o++) {
        G.loc[o] = objs[o].loc;
        G.st[o] = objs[o].state;
    }
}

static void intro(void)
{
    curcol = C_TITLE;
    say(TEXT_TITLE);
    curcol = C_TEXT;
    out_blank();
    say(TEXT_INTRO);
    out_blank();
    pending_look = 2; /* the place in the next frame */
}

/* The name after SAVE or RESTORE: letters and digits, or "game". */
static int save_name_from(uint8_t t)
{
    uint8_t n = 0, gap = 0;
    /* The rest of the line: one word of letters and digits. */
    for (uint8_t i = tpos[t] + tlen[t]; i < input_len; i++) {
        uint8_t c = lower[i];
        if (c == ' ') {
            gap = n != 0;
            continue;
        }
        if (gap || !((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')) || n >= 16) {
            say(M_NO_FREE_TEXT);
            return 0;
        }
        save_name[n++] = c;
    }
    if (!n) {
        memcpy(save_name, "game", 4);
        n = 4;
    }
    save_name[n] = 0;
    return 1;
}

static void do_save(uint8_t restore)
{
    if (!has_content) {
        say(M_NO_STORAGE);
        return;
    }
    if (save_busy)
        return;
    if (restore) {
        if (record_load(&save_req, save_name, 0, sizeof load_buf, &load_buf) && save_req.status == STATUS_PENDING)
            save_busy = 2;
    } else {
        if (record_save(&save_req, save_name, &G, sizeof G) && save_req.status == STATUS_PENDING)
            save_busy = 1;
    }
    if (!save_busy) {
        say(M_SAVE_FAILED);
        return;
    }
    say(restore ? M_RESTORING : M_SAVING);
}

static void save_poll(void)
{
    if (!save_busy || save_req.status == STATUS_PENDING)
        return;
    page_top = nlines;
    ctx_word = (const uint8_t *)save_name;
    ctx_word_len = 0;
    while (save_name[ctx_word_len])
        ctx_word_len++;
    if (save_busy == 1)
        say(save_req.status == STATUS_DONE ? M_SAVED : M_SAVE_FAILED);
    else if (save_req.status == STATUS_NOT_FOUND)
        say(M_NO_SAVE);
    else if (save_req.status != STATUS_DONE)
        say(M_SAVE_FAILED);
    else if (save_req.length != sizeof load_buf || load_buf.magic != GAME_MAGIC || load_buf.story != STORY_ID)
        say(M_BAD_SAVE);
    else {
        undo_push();
        G = load_buf;
        say(M_RESTORED);
        pending_look = 1;
        refresh_all();
    }
    save_busy = 0;
    reply_done();
}

static void do_hint(void)
{
    for (uint8_t h = 0; h < N_HINTS_REAL; h++) {
        if (!cond(hints[h].cond))
            continue;
        uint8_t lv = G.hint_level[h];
        if (lv >= hints[h].n)
            lv = hints[h].n - 1;
        say(hint_texts[hints[h].first + lv]);
        if (lv + 1 < hints[h].n) {
            say(M_HINT_MORE);
            G.hint_level[h] = lv + 1;
        } else
            say(M_HINT_LAST);
        return;
    }
    say(M_HINT_NONE);
}

/* --- One command line ------------------------------------------------------------------------------------- */

static int is_verb_start(uint8_t t) { return kinds(t) & (WK_VERB | WK_DIR); }
static uint16_t then_word;

/* Parses and carries out tokens t0..t1 as one command. */
static void command(uint8_t t0, uint8_t t1)
{
    verb_tok = part_tok = 0xFF;
    c_o1 = c_o2 = R_NONE;
    c_dir = 0xFF;
    c_prep = 0;
    c_all = 0;
    c_topic = 0xFFFF;
    ctx1 = ctx2 = 0xFF;
    ctx_prep = 0xFF;
    if (t0 >= t1)
        return;
    /* SAVE and RESTORE take any name after them. */
    if (tw[t0] < N_WORDS && (words[tw[t0]].kinds & WK_VERB) && (words[tw[t0]].verb == A_SAVE || words[tw[t0]].verb == A_RESTORE)) {
        question = Q_NONE;
        if (save_name_from(t0))
            do_save(words[tw[t0]].verb == A_RESTORE);
        return;
    }
    for (uint8_t t = t0; t < t1; t++)
        if (tw[t] == T_UNKNOWN) {
            ctx_word = lower + tpos[t];
            ctx_word_len = tlen[t];
            say(M_UNKNOWN_WORD);
            question = Q_NONE;
            return;
        }
    /* An answer to "which" or "what". */
    if ((question == Q_WHICH || question == Q_WHAT) && !(kinds(t0) & WK_VERB && !(kinds(t0) & (WK_NOUN | WK_ADJ)))) {
        uint8_t qs = question;
        question = Q_NONE;
        c_action = q_action, c_o1 = q_o1, c_o2 = q_o2, c_dir = q_dir, c_prep = q_prep;
        uint8_t got = R_NONE;
        if (qs == Q_WHICH) {
            uint8_t c[N_OBJS], n = 0;
            for (uint8_t i = 0; i < q_ncands; i++)
                if (np_matches_(q_cands[i], t0, t1, 1)) /* "iron" will do */
                    c[n++] = q_cands[i];
            if (n == 1)
                got = c[0];
            else if (n > 1) {
                np_word(t0, t1);
                ask_which(q_slot, c, n);
                return;
            }
        } else if (c_action == A_ASK || c_action == A_TELL) {
            for (int t = t1 - 1; t >= t0; t--)
                if (!(kinds(t) & WK_ART)) {
                    c_topic = tw[t];
                    break;
                }
            got = c_o1;
        } else
            got = resolve(t0, t1, q_slot);
        if (got == R_FAIL || got == R_ASKED)
            return;
        if (got != R_NONE) {
            if (q_slot == 1)
                c_o1 = got;
            else if (!(c_action == A_ASK || c_action == A_TELL))
                c_o2 = got;
            dispatch();
            return;
        }
        /* Not an answer: a new command. */
    }
    question = Q_NONE;
    uint8_t t = t0;
    uint16_t k = kinds(t);
    if ((k & WK_DIR) && !(k & WK_VERB)) {
        c_action = A_GO;
        c_dir = words[tw[t]].dir;
        dispatch();
        return;
    }
    if (!(k & WK_VERB)) {
        say(M_NO_VERB);
        return;
    }
    c_action = words[tw[t]].verb;
    verb_tok = t++;
    /* A particle straight after the verb: look at, pick up, turn on. */
    if (t < t1) {
        for (uint16_t i = 0; i < N_PARTICLES; i++)
            if (particle_tab[i][0] == tw[verb_tok] && particle_tab[i][1] == tw[t]) {
                /* "put on the coat" but not "put on" + nothing */
                c_action = particle_tab[i][2] & 0xFF;
                part_tok = t++;
                break;
            }
    }
    if (c_action == A_AGAIN) {
        if (!last_len) {
            say(M_AGAIN_NONE);
            return;
        }
        memcpy(input, last_line, last_len);
        input_len = last_len;
        tokenize();
        command(0, nt);
        return;
    }
    if (c_action == A_GO) {
        for (; t < t1; t++)
            if (kinds(t) & WK_DIR) {
                c_dir = words[tw[t]].dir;
                break;
            }
        dispatch();
        return;
    }
    if (c_action == A_SAVE || c_action == A_RESTORE) {
        if (save_name_from(verb_tok))
            do_save(c_action == A_RESTORE);
        return;
    }
    /* The noun phrases: up to a preposition followed by more words. */
    uint8_t n1s = t, n1e = t, n2s = t1, n2e = t1;
    while (n1e < t1 && !((kinds(n1e) & WK_PREP) && n1e + 1 < t1 && n1e > n1s))
        n1e++;
    /* "shine at wall", "look under bench": a preposition first. */
    if (n1s < t1 && (kinds(n1s) & WK_PREP) && !(kinds(n1s) & (WK_NOUN | WK_ADJ)) && n1s + 1 < t1) {
        c_prep = words[tw[n1s]].prep;
        n2s = n1s + 1;
        n1e = n1s;
    } else if (n1e < t1) {
        c_prep = words[tw[n1e]].prep;
        n2s = n1e + 1;
    }
    /* A particle at the end: turn lantern on, put coat on. */
    if (n2s >= t1 && n1e > n1s + 1 && part_tok == 0xFF) {
        uint8_t last = n1e - 1;
        for (uint16_t i = 0; i < N_PARTICLES; i++)
            if (particle_tab[i][0] == tw[verb_tok] && particle_tab[i][1] == tw[last] && (particle_tab[i][2] >> 8)) {
                c_action = particle_tab[i][2] & 0xFF;
                part_tok = last;
                n1e = last;
                break;
            }
    }
    /* A preposition that changes the action: put X on Y, remove X from Y. */
    if (c_prep)
        for (uint8_t i = 0; i < N_PSWITCH; i++)
            if (pswitch_tab[i][0] == c_action && pswitch_tab[i][1] == c_prep) {
                c_action = pswitch_tab[i][2];
                break;
            }
    ctx_prep = c_prep ? c_prep : 0xFF;
    uint8_t f = action_flags[c_action];
    if (c_action == A_CLIMB || c_action == A_LISTEN || c_action == A_SMELL) {
        /* climb up / listen to: the direction or particle is not an object */
    }
    /* The second noun phrase first, so a question about the first keeps it. */
    if (n2e > n2s) {
        if (f & AF_TOPIC) {
            for (int t2 = n2e - 1; t2 >= n2s; t2--)
                if (!(kinds(t2) & WK_ART)) {
                    c_topic = tw[t2];
                    break;
                }
        } else {
            c_o2 = resolve(n2s, n2e, 2);
            if (c_o2 == R_FAIL || c_o2 == R_ASKED) {
                if (c_o2 == R_ASKED) {
                    save_question();
                    q_slot = 2;
                }
                return;
            }
        }
    }
    if (n1e > n1s) {
        c_o1 = resolve(n1s, n1e, 1);
        if (c_o1 == R_FAIL || c_o1 == R_ASKED) {
            if (c_o1 == R_ASKED)
                save_question(), q_slot = 1;
            return;
        }
    }
    if ((f & AF_META) && c_action != A_LOOK) {
        switch (c_action) {
        case A_INVENTORY: action_default_run(); return;
        case A_UNDO: do_undo(); return;
        case A_RESTART: question = Q_RESTART; say(M_RESTART_Q); return;
        case A_QUIT: question = Q_QUIT; say(M_QUIT_Q); return;
        case A_HELP: say(M_HELP); return;
        case A_HINT: do_hint(); return;
        case A_SCORE: say(MAX_SCORE ? M_SCORE : M_SCORE_NONE); return;
        default: say(default_of(c_action)); return;
        }
    }
    dispatch();
}

static void line(void)
{
    /* Echo the command. */
    out_blank();
    page_top = nlines;
    curcol = C_ECHO;
    out_ch('>');
    out_ch(' ');
    for (uint8_t i = 0; i < input_len; i++)
        out_ch(input[i]);
    out_nl();
    curcol = C_TEXT;
    tokenize();
    if (question == Q_RESTART || question == Q_QUIT) {
        uint8_t q = question;
        question = Q_NONE;
        if (nt && (lower[tpos[0]] == 'y')) {
            if (q == Q_QUIT) {
                reply_done();
                screen_frame();
                terse_wait_frame();
                terse_exit(0);
            }
            game_init();
            undo_n = 0;
            refresh_all();
            intro();
            return;
        }
        say(M_CANCELLED);
        return;
    }
    if (!nt) {
        say(M_PARDON);
        return;
    }
    /* Remember it for AGAIN (unless it is AGAIN). */
    uint8_t is_again = nt == 1 && tw[0] < N_WORDS && (words[tw[0]].kinds & WK_VERB) && words[tw[0]].verb == A_AGAIN;
    uint8_t meta = nt && tw[0] < N_WORDS && (words[tw[0]].kinds & WK_VERB) && (action_flags[words[tw[0]].verb] & AF_META) &&
                   words[tw[0]].verb != A_AGAIN;
    if (G.ended) {
        uint8_t v = tw[0] < N_WORDS && (words[tw[0]].kinds & WK_VERB) ? words[tw[0]].verb : 0xFF;
        if (v != A_RESTART && v != A_RESTORE && v != A_UNDO && v != A_QUIT && v != A_SCORE) {
            say(M_END_PROMPT);
            return;
        }
    }
    uint8_t pushed = !meta && !G.ended, was_ended = G.ended;
    if (pushed)
        undo_push();
    uint8_t saved_len = input_len, saved[INPUT_MAX + 1];
    memcpy(saved, input, input_len);
    moved_now = 0;
    /* Commands separated by "." or "then" (or "and" before a verb). */
    uint8_t s = 0;
    for (uint8_t t = 0; t <= nt && G.ended == was_ended; t++) {
        uint8_t end = t == nt || tw[t] == T_PERIOD ||
                      (t > s && (kinds(t) & WK_SEP) && t + 1 < nt && (is_verb_start(t + 1) || tw[t] == then_word));
        if (!end)
            continue;
        if (t > s) {
            uint8_t q_before = question;
            command(s, t);
            if (moved_now && !G.ended) {
                uint16_t prev = G.minutes;
                G.minutes += spend;
                G.moves++;
                dirty_status = 1;
                events_move(prev);
            }
            moved_now = 0;
            spend = 0;
            if (question != Q_NONE && q_before == Q_NONE && t < nt)
                break;
        }
        s = t + 1;
    }
    /* A line that changed nothing (only looked, or was not understood) is
     * not a step for UNDO. */
    if (pushed && undo_n) {
        uint8_t last = (undo_head + UNDO_N - 1) % UNDO_N, it = G.it;
        G.it = undo_ring[last].it;
        if (!memcmp(&G, &undo_ring[last], sizeof G)) {
            undo_head = last;
            undo_n--;
        }
        G.it = it;
    }
    if (!is_again) {
        memcpy(last_line, saved, saved_len);
        last_len = saved_len;
    }
    (void)saved_len;
}

/* --- Input -------------------------------------------------------------------------------------------- */

static uint8_t text_busy;

static void submit(void)
{
    text_busy = 1;
    if (input_len) {
        uint8_t h = hist_n % 4;
        memcpy(history[h], input, input_len);
        hist_len[h] = input_len;
        hist_n++;
    }
    hist_pos = 0;
    line();
    /* The beam may have come on, or the place changed under it. */
    if (beam_obj() != 0xFF && !G.ended) {
        if (beam_on_shown != 1)
            shown_bx = G.beam_x, shown_by = G.beam_y;
        beam_on_shown = 1;
        beam_check();
    }
    input_len = 0;
    reply_done();
    dirty_scene = 1;
}

static void beam_nudge(int16_t dx, int16_t dy)
{
    if (beam_obj() == 0xFF)
        return;
    int32_t x = G.beam_x + dx, y = G.beam_y + dy;
    if (x < 0)
        x = 0;
    if (x > 1280)
        x = 1280;
    if (y < 0)
        y = 0;
    if (y > 512)
        y = 512;
    G.beam_x = x, G.beam_y = y;
    shown_bx = x, shown_by = y;
    dirty_scene = 1;
    uint32_t before = nlines;
    page_top = nlines;
    beam_check();
    if (nlines != before)
        reply_done();
}

/* A key that typed its own text this frame (a keyboard's Space gives a key
 * and a text event: the "more" prompt takes only one of them). */
static uint8_t more_took_key, mods;

static void key_down(uint8_t k)
{
    if (more) {
        if (k == KEY_ENTER || k == KEY_SPACE || k == KEY_PAGE_DOWN || k == KEY_ESCAPE) {
            more_key();
            more_took_key = 1;
        }
        return;
    }
    switch (k) {
    case KEY_ENTER:
        if (scroll) {
            scroll = 0;
            dirty_text = dirty_input = 1;
        }
        submit();
        break;
    case KEY_BACKSPACE:
        if (input_len)
            input_len--;
        dirty_input = 1;
        break;
    case KEY_ESCAPE:
        input_len = 0;
        dirty_input = 1;
        break;
    case KEY_PAGE_UP:
        if (nlines > TEXT_ROWS) {
            scroll += TEXT_ROWS / 2;
            uint32_t most = (nlines < RING ? nlines : RING) - TEXT_ROWS;
            if (scroll > most)
                scroll = most;
            dirty_text = dirty_input = 1;
        }
        break;
    case KEY_PAGE_DOWN:
        scroll = scroll > TEXT_ROWS / 2 ? scroll - TEXT_ROWS / 2 : 0;
        dirty_text = dirty_input = 1;
        break;
    case KEY_LEFT: keys_held |= 1; beam_nudge(-12, 0); break;
    case KEY_RIGHT: keys_held |= 2; beam_nudge(12, 0); break;
    case KEY_UP:
        if (beam_obj() == 0xFF) {
            /* Without a beam, Up recalls the commands typed before. */
            if (hist_pos < 4 && hist_pos < hist_n) {
                hist_pos++;
                uint8_t h = (hist_n - hist_pos) % 4;
                memcpy(input, history[h], hist_len[h]);
                input_len = hist_len[h];
                dirty_input = 1;
            }
        } else {
            keys_held |= 4;
            beam_nudge(0, -12);
        }
        break;
    case KEY_DOWN:
        if (beam_obj() == 0xFF) {
            if (hist_pos > 1) {
                hist_pos--;
                uint8_t h = (hist_n - hist_pos) % 4;
                memcpy(input, history[h], hist_len[h]);
                input_len = hist_len[h];
            } else {
                hist_pos = 0;
                input_len = 0;
            }
            dirty_input = 1;
        } else {
            keys_held |= 8;
            beam_nudge(0, 12);
        }
        break;
    }
}

static void key_up(uint8_t k)
{
    switch (k) {
    case KEY_LEFT: keys_held &= ~1; break;
    case KEY_RIGHT: keys_held &= ~2; break;
    case KEY_UP: keys_held &= ~4; break;
    case KEY_DOWN: keys_held &= ~8; break;
    }
}

static void text_in(uint32_t cp)
{
    if (more_took_key && cp == ' ')
        return;
    if (more) {
        more_key();
        return;
    }
    if (cp < 0x20 || cp == 0x7F || cp > 0xFF || input_len >= INPUT_MAX || (mods & MOD_CONTROL))
        return;
    if (scroll) {
        scroll = 0;
        dirty_text = 1;
    }
    input[input_len++] = (uint8_t)cp;
    dirty_input = 1;
}

static uint8_t held_frames;

static void events_frame(void)
{
    terse_event e;
    more_took_key = 0;
    while (terse_event_pop(&e)) {
        switch (e.type) {
        case EV_KEY_DOWN: mods = e.b; key_down(e.a); break;
        case EV_KEY_UP: mods = e.b; key_up(e.a); break;
        case EV_TEXT: text_in(EV_TEXT_CP(e)); break;
        case EV_WHEEL: {
            int8_t w = EV_WHEEL_Y(e);
            if (w < 0)
                key_down(KEY_PAGE_UP);
            else if (w > 0)
                key_down(KEY_PAGE_DOWN);
            break;
        }
        case EV_POINTER: {
            int x = EV_POINTER_X(e), y = EV_POINTER_Y(e);
            if (y < SCENE_H && beam_obj() != 0xFF && !more)
                beam_nudge(x * 2 - G.beam_x, y * 2 - G.beam_y);
            break;
        }
        }
    }
    /* Arrows held: the beam keeps moving (after a short pause, as keys repeat). */
    if (keys_held && beam_obj() != 0xFF) {
        if (++held_frames > 8)
            beam_nudge((keys_held & 2 ? 6 : 0) - (keys_held & 1 ? 6 : 0), (keys_held & 8 ? 6 : 0) - (keys_held & 4 ? 6 : 0));
    } else
        held_frames = 0;
}

/* The text the tests read: a marker, then the count of lines and the ring. */
struct transcript_view {
    char marker[8];
    uint32_t *lines;
    uint8_t (*ring)[COLS + 2];
    uint32_t ring_size, cols;
    struct game *game;
};
volatile struct transcript_view tv = { { 'L', 'A', 'M', 'P', 'T', 'E', 'X', 'T' }, &nlines, ring, RING, COLS + 2, &G };

int main(void)
{
    term_palette_set(0, palette, sizeof palette / 3);
    term_color(C_TEXT, 0);
    term_cmd(TERM_CLEAR);
    vec_style(VEC_STYLE_PHOSPHOR);
    has_content = terse_has_device(4);
    then_word = lookup((const uint8_t *)"then", 4);
    (void)tv.marker[0]; /* keeps the transcript's marker in the image */
    sound_init();
    game_init();
    beam_on_shown = 0;
    dirty_scene = 1; /* the first place is drawn with its text */
    page_top = 0;
    /* The intro in frame 0, the first place in frame 1, its picture in 2. */
    curcol = C_TITLE;
    say(TEXT_TITLE);
    curcol = C_TEXT;
    out_blank();
    say(TEXT_INTRO);
    out_blank();
    terse_wait_frame();
    look();
    enter_events();
    text_busy = 1;
    reply_done();
    if (nlines > TEXT_ROWS) {
        more = 1;
        view_end = TEXT_ROWS;
    }
    for (;;) {
        if (pending_look && !G.ended) {
            uint8_t enter = pending_look == 2;
            pending_look = 0;
            look();
            if (enter)
                enter_events();
            text_busy = 1;
            reply_done();
            dirty_scene = 1;
        }
        events_frame();
        save_poll();
        /* The picture of a new place follows its text by a frame, so that no
         * frame carries both. */
        if (!text_busy)
            scene_frame();
        text_busy = 0;
        sound_frame();
        screen_frame();
        tick++;
        terse_wait_frame();
    }
}
