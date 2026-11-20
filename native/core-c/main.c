/* tcore's command line: run an image headless, scripted input, hashes. */
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include "core.h"

static const char *usage =
    "usage: tcore <image.terse> [--frames N] [--events <file>] [--hash] [--hash-every N]\n"
    "             [--dump] [--state <file>] [--drivers <dir>] [--driver <slots>=<command line>]...\n"
    "             [--without <slot>]... [--no-batch] [--bench]\n";

/* ---- scripted input (the format of `terse run --events`) ---- */
typedef struct { uint32_t frame; int pad; uint8_t ev[4]; uint8_t ps[8]; } In;
static In *in; static size_t nin, incap;

static void add(uint32_t f, int pad, const uint8_t *ev, const uint8_t *ps)
{
    if (nin == incap) { incap = incap * 2 + 64; in = realloc(in, incap * sizeof *in); }
    In *e = &in[nin++];
    memset(e, 0, sizeof *e);
    e->frame = f; e->pad = pad;
    if (ev) memcpy(e->ev, ev, 4);
    if (ps) memcpy(e->ps, ps, 8);
}

static int key(const char *s)
{
    static const struct { const char *n; int k; } names[] = {
        {"enter", 0x28}, {"esc", 0x29}, {"backspace", 0x2A}, {"tab", 0x2B}, {"space", 0x2C},
        {"right", 0x4F}, {"left", 0x50}, {"down", 0x51}, {"up", 0x52}, {"home", 0x4A},
        {"end", 0x4D}, {"pageup", 0x4B}, {"pagedown", 0x4E}, {"insert", 0x49}, {"delete", 0x4C}};
    if (s[0] >= 'a' && s[0] <= 'z' && !s[1]) return 0x04 + s[0] - 'a';
    if (s[0] >= '0' && s[0] <= '9' && !s[1]) return s[0] == '0' ? 0x27 : 0x1E + s[0] - '1';
    if (s[0] == 'f' && s[1] >= '1' && s[1] <= '9') { int n = atoi(s + 1); if (n >= 1 && n <= 12) return 0x39 + n; }
    for (size_t i = 0; i < sizeof names / sizeof *names; i++) if (!strcmp(s, names[i].n)) return names[i].k;
    char *e;
    long v = strtol(s, &e, 0);
    if (*e || v < 0 || v > 255) die(2, "events: unknown key %s", s);
    return v;
}

static int mods(char **t, int n)
{
    int m = 0;
    for (int i = 0; i < n; i++) {
        if (!strcmp(t[i], "shift")) m |= 1;
        else if (!strcmp(t[i], "ctrl")) m |= 2;
        else if (!strcmp(t[i], "alt")) m |= 4;
        else if (!strcmp(t[i], "meta")) m |= 8;
        else die(2, "events: unknown modifier %s", t[i]);
    }
    return m;
}

static void pointer(uint32_t f, long x, long y, long btn)
{
    x = x < 0 ? 0 : x > 639 ? 639 : x;
    y = y < 0 ? 0 : y > 479 ? 479 : y;
    uint8_t ev[4] = {4, x & 0xFF, y & 0xFF, (x >> 8 & 3) | (y >> 8 & 1) << 2 | (btn & 7) << 3};
    add(f, 0, ev, 0);
}

static void load_events(const char *path)
{
    FILE *fp = fopen(path, "r");
    if (!fp) die(2, "cannot open %s", path);
    char line[8192];
    while (fgets(line, sizeof line, fp)) {
        line[strcspn(line, "\r\n")] = 0;
        char *p = line;
        while (*p == ' ' || *p == '\t') p++;
        char *e;
        unsigned long f = strtoul(p, &e, 10);
        if (e == p) {
            if (*p && *p != '#') die(2, "events: bad line: %s", line);
            continue;
        }
        p = e + strspn(e, " \t");
        if (!strncmp(p, "text ", 5)) {            /* the rest of the line, one event a character */
            const uint8_t *s = (const uint8_t *)p + 5;
            while (*s) {
                uint32_t c = *s++, k = c >= 0xF0 ? 3 : c >= 0xE0 ? 2 : c >= 0xC0 ? 1 : 0;
                if (k) c &= 0x3F >> k;
                while (k-- && *s) c = c << 6 | (*s++ & 0x3F);
                uint8_t ev[4] = {3, c & 0xFF, c >> 8 & 0xFF, c >> 16 & 0x1F};
                add(f, 0, ev, 0);
            }
            continue;
        }
        char *h = strchr(p, '#');
        if (h) *h = 0;
        char *t[16]; int n = 0;
        for (char *w = strtok(p, " \t"); w && n < 16; w = strtok(0, " \t")) t[n++] = w;
        if (!n) continue;
        if (!strcmp(t[0], "down") || !strcmp(t[0], "up") || !strcmp(t[0], "press")) {
            if (n < 2) die(2, "events: %s needs a key", t[0]);
            uint8_t k = key(t[1]), md = mods(t + 2, n - 2);
            uint8_t dn[4] = {1, k, md, 0}, up[4] = {2, k, md, 0};
            if (t[0][0] != 'u') add(f, 0, dn, 0);
            if (t[0][0] != 'd') add(f, 0, up, 0);
        } else if (!strcmp(t[0], "pointer") && n >= 3) {
            pointer(f, atol(t[1]), atol(t[2]), n > 3 ? atol(t[3]) : 0);
        } else if (!strcmp(t[0], "click") && n >= 3) {
            pointer(f, atol(t[1]), atol(t[2]), 1);
            pointer(f, atol(t[1]), atol(t[2]), 0);
        } else if (!strcmp(t[0], "wheel") && n >= 2) {
            uint8_t ev[4] = {6, (uint8_t)atol(t[1]), (uint8_t)(n > 2 ? atol(t[2]) : 0), 0};
            add(f, 0, ev, 0);
        } else if (!strcmp(t[0], "feeds-updated")) {
            uint8_t ev[4] = {7, 0, 0, 0};
            add(f, 0, ev, 0);
        } else if (!strcmp(t[0], "pad") && n >= 3) {
            uint8_t ps[8] = {0};
            if (strcmp(t[2], "off")) {
                unsigned long b = strtoul(t[2], 0, 16);
                ps[0] = 1; ps[1] = b; ps[2] = b >> 8;
                for (int i = 0; i < 4; i++) {
                    long v = 3 + i < n ? atol(t[3 + i]) : 0;
                    ps[3 + i] = (uint8_t)(v < -127 ? -127 : v > 127 ? 127 : v);
                }
            }
            add(f, 1 + atoi(t[1]), 0, ps);
        } else if (!strcmp(t[0], "home")) {
            /* a shell's only: nothing for a machine run alone */
        } else
            die(2, "events: unknown command %s", t[0]);
    }
    fclose(fp);
}

static void hex(const uint8_t *h) { for (int i = 0; i < 32; i++) printf("%02x", h[i]); }

static void state_hash(M *m, uint8_t h[32], const char *dump)
{
    uint8_t *s;
    size_t n = serialize(m, &s);
    blake3(s, n, h);
    if (dump) { FILE *f = fopen(dump, "wb"); if (f) { fwrite(s, 1, n, f); fclose(f); } }
    free(s);
}

static void put_utf8(uint32_t c)
{
    if (c < 0x80) putchar(c);
    else if (c < 0x800) { putchar(0xC0 | c >> 6); putchar(0x80 | (c & 0x3F)); }
    else { putchar(0xE0 | c >> 12); putchar(0x80 | (c >> 6 & 0x3F)); putchar(0x80 | (c & 0x3F)); }
}

/* The terminal's grid, from its STATE section (DEVICES.md 1.7, STATE.md 1.6). */
static void dump(M *m)
{
    uint8_t *s; uint32_t n;
    if (!m->has[2] || !bus_state(m, 2, &s, &n) || n < 9600) return;
    for (int row = 0; row < 30; row++) {
        int last = -1;
        for (int c = 0; c < 80; c++) {
            uint32_t ch = rd16(s + 4 * (row * 80 + c));
            if (ch != 0x20 && ch != 0xFFFF) last = c;
        }
        for (int c = 0; c <= last; c++) {
            uint32_t ch = rd16(s + 4 * (row * 80 + c));
            if (ch != 0xFFFF) put_utf8(ch < 0x20 ? ' ' : ch);
        }
        putchar('\n');
    }
    free(s);
}

static volatile sig_atomic_t quit;
static void on_signal(int s) { (void)s; quit = 1; }

static const char *fault_name[] = {"", "illegal instruction", "breakpoint", "fetch", "load", "store", "misaligned"};

int main(int argc, char **argv)
{
    const char *image = 0, *events = 0, *state_file = 0;
    long frames = 30, every = 0;
    int want_hash = 0, want_dump = 0, bench = 0;
    signal(SIGPIPE, SIG_IGN);
    signal(SIGINT, on_signal); signal(SIGTERM, on_signal); signal(SIGHUP, on_signal);
    bus_driver_dir = getenv("TERSE_DRIVERS");
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
#define ARG() (i + 1 < argc ? argv[++i] : (die(2, "%s needs a value", a), ""))
        if (!strcmp(a, "--frames")) frames = atol(ARG());
        else if (!strcmp(a, "--events")) events = ARG();
        else if (!strcmp(a, "--hash")) want_hash = 1;
        else if (!strcmp(a, "--hash-every")) every = atol(ARG());
        else if (!strcmp(a, "--dump")) want_dump = 1;
        else if (!strcmp(a, "--state")) state_file = ARG();
        else if (!strcmp(a, "--drivers")) bus_driver_dir = ARG();
        else if (!strcmp(a, "--driver")) bus_set_driver(ARG());
        else if (!strcmp(a, "--without")) bus_withheld[atoi(ARG()) & 255] = 1;
        else if (!strcmp(a, "--no-batch")) bus_nobatch = 1;
        else if (!strcmp(a, "--bench")) bench = 1;
        else if (a[0] == '-' || image) { fputs(usage, stderr); return 2; }
        else image = a;
    }
    if (!image) { fputs(usage, stderr); return 2; }
    FILE *f = fopen(image, "rb");
    if (!f) die(2, "cannot open %s", image);
    uint8_t *file = malloc(131072 + 64 + 1);
    size_t len = fread(file, 1, 131072 + 64 + 1, f);
    fclose(f);
    static M m;
    char err[256];
    if (!image_load(&m, file, len, err, sizeof err)) die(2, "%s: %s", image, err);
    if (events) load_events(events);
    if (!bus_setup(&m, err, sizeof err)) die(2, "%s: %s", image, err);

    uint8_t pads[4][8] = {{0}}, (*ev)[4] = malloc(4 * (nin + 1)), h[32];
    size_t next = 0;
    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    long ran = 0;
    int64_t used = 0;
    for (; ran < frames && m.stopped == RUNNING && !quit; ran++) {
        int nev = 0;
        while (next < nin && in[next].frame < m.frame) next++;
        for (; next < nin && in[next].frame == m.frame; next++) {
            if (in[next].pad) { if (in[next].pad <= 4) memcpy(pads[in[next].pad - 1], in[next].ps, 8); }
            else memcpy(ev[nev++], in[next].ev, 4);
        }
        /* SPEC §frames: step 1 (inputs), step 2 (run), step 3 (end) */
        int64_t charge;
        bus_begin(&m, (const uint8_t (*)[4])ev, nev, &charge);
        m.time = 0;
        memcpy(m.pads, pads, 32);
        m.budget += BUDGET - charge;
        int64_t before = m.budget;
        cpu_run(&m);
        used += before - m.budget;
        if (m.budget > 0) m.budget = 0;
        m.frame++;
        bus_end(&m);
        if (every && m.frame % every == 0) {
            state_hash(&m, h, 0);
            printf("%u ", m.frame); hex(h); putchar('\n');
        }
    }
    clock_gettime(CLOCK_MONOTONIC, &t1);
    if (bench) {
        double s = (t1.tv_sec - t0.tv_sec) + (t1.tv_nsec - t0.tv_nsec) / 1e9;
        fprintf(stderr, "tcore: %ld frames in %.3f s, %.0f frames/s, %lld units, %.1f M units/s\n",
                ran, s, ran / s, (long long)used, used / s / 1e6);
        bus_stats();
    }
    if (want_hash || state_file) { state_hash(&m, h, state_file); if (want_hash) { hex(h); putchar('\n'); } }
    if (want_dump) dump(&m);
    fflush(stdout);
    bus_shutdown();
    if (m.stopped == EXITED) return m.exit_code;
    if (m.stopped == FAULTED) {
        fprintf(stderr, "tcore: fault: %s at pc %08x, address %08x", fault_name[m.fault_kind], m.fault_pc, m.fault_addr);
        if (m.fault_kind == F_ILLEGAL) fprintf(stderr, ", encoding %08x", m.fault_enc);
        fputc('\n', stderr);
        return 128 + SIGTERM;
    }
    return 0;
}
