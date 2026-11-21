/* tcore's command line: run an image headless, scripted input, hashes. */
#include <ctype.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include "core.h"

static const char *usage =
    "usage: tcore <image.terse> [--frames N] [--events <file>] [--hash] [--hash-every N]\n"
    "             [--dump] [--state <file>] [--drivers <dir>] [--driver <slots>=<command line>]...\n"
    "             [--without <slot>]... [--no-batch] [--bench] [--identities]\n"
    "       tcore --version\n";

/* ---- scripted input: the events file of STATE.md 6.2 ---- */
typedef struct { uint32_t frame; size_t seq; int pad; uint8_t ev[4]; uint8_t ps[8]; } In;
static In *in; static size_t nin, incap;
static unsigned long lineno;

static void add(uint32_t f, int pad, const uint8_t *ev, const uint8_t *ps)
{
    if (nin == incap) { incap = incap * 2 + 64; in = realloc(in, incap * sizeof *in); }
    In *e = &in[nin];
    memset(e, 0, sizeof *e);
    e->frame = f; e->seq = nin++; e->pad = pad;
    if (ev) memcpy(e->ev, ev, 4);
    if (ps) memcpy(e->ps, ps, 8);
}

#define BAD(...) do { fprintf(stderr, "tcore: events line %lu: ", lineno); die(2, __VA_ARGS__); } while (0)

/* A whole word as a number in [lo, hi] (base 10 or 16; a sign only if lo < 0). */
static long num(const char *w, int base, long lo, long hi, const char *what)
{
    const char *p = w + (*w == '-' && lo < 0);
    if (!*p || !(base == 16 ? isxdigit((unsigned char)*p) : isdigit((unsigned char)*p))) BAD("bad %s %s", what, w);
    char *e;
    errno = 0;
    long v = strtol(w, &e, base);
    if (*e || errno || v < lo || v > hi) BAD("bad %s %s", what, w);
    return v;
}

/* A key: a name or a USB HID usage ID, case-insensitive (STATE.md 6.2). */
static int key(const char *w)
{
    static const struct { const char *n; int k; } names[] = {
        {"enter", 0x28}, {"esc", 0x29}, {"escape", 0x29}, {"backspace", 0x2A}, {"tab", 0x2B},
        {"space", 0x2C}, {"insert", 0x49}, {"home", 0x4A}, {"pageup", 0x4B}, {"delete", 0x4C},
        {"end", 0x4D}, {"pagedown", 0x4E}, {"right", 0x4F}, {"left", 0x50}, {"down", 0x51}, {"up", 0x52}};
    char s[32];
    size_t n = strlen(w);
    if (n >= sizeof s) BAD("unknown key %s", w);
    for (size_t i = 0; i <= n; i++) s[i] = tolower((unsigned char)w[i]);
    if (s[0] == '0' && s[1] == 'x') return num(s + 2, 16, 0, 255, "key");
    if (n == 1 && s[0] >= 'a' && s[0] <= 'z') return 0x04 + s[0] - 'a';
    if (n == 1 && s[0] >= '1' && s[0] <= '9') return 0x1E + s[0] - '1';
    if (n == 1 && s[0] == '0') return 0x27;
    if (isdigit((unsigned char)s[0])) return num(s, 10, 0, 255, "key");
    if (s[0] == 'f' && isdigit((unsigned char)s[1])) {
        long f = strtol(s + 1, 0, 10);
        if (strspn(s + 1, "0123456789") == n - 1 && f >= 1 && f <= 12) return 0x39 + f;
    }
    for (size_t i = 0; i < sizeof names / sizeof *names; i++) if (!strcmp(s, names[i].n)) return names[i].k;
    BAD("unknown key %s", w);
    return 0;
}

static int mods(char **t, int n)
{
    int m = 0;
    for (int i = 0; i < n; i++) {
        if (!strcmp(t[i], "shift")) m |= 1;
        else if (!strcmp(t[i], "ctrl") || !strcmp(t[i], "control")) m |= 2;
        else if (!strcmp(t[i], "alt")) m |= 4;
        else if (!strcmp(t[i], "meta")) m |= 8;
        else BAD("unknown modifier %s", t[i]);
    }
    return m;
}

/* Pointer positions are clamped to the screen (SPEC §events). */
static void pointer(uint32_t f, long x, long y, long btn)
{
    x = x > 639 ? 639 : x;
    y = y > 479 ? 479 : y;
    uint8_t ev[4] = {4, x & 0xFF, y & 0xFF, (x >> 8 & 3) | (y >> 8 & 1) << 2 | (btn & 7) << 3};
    add(f, 0, ev, 0);
}

static int by_frame(const void *a, const void *b)
{
    const In *x = a, *y = b;
    if (x->frame != y->frame) return x->frame < y->frame ? -1 : 1;
    return x->seq < y->seq ? -1 : x->seq > y->seq;
}

static void load_events(const char *path)
{
    FILE *fp = fopen(path, "r");
    if (!fp) die(2, "cannot open %s", path);
    char *line = 0;
    size_t cap = 0;
    ssize_t len;
    while ((len = getline(&line, &cap, fp)) >= 0) {
        lineno++;
        if (len && line[len - 1] == '\n') line[--len] = 0;
        if (len && line[len - 1] == '\r') line[--len] = 0;
        char *p = line + strspn(line, " \t");
        if (!*p || *p == '#') continue;                 /* blank, or a comment */
        size_t fl = strcspn(p, " \t");
        char fw[16];
        if (fl >= sizeof fw) BAD("bad frame number");
        memcpy(fw, p, fl); fw[fl] = 0;
        uint32_t f = num(fw, 10, 0, 4294967295L, "frame number");
        p += fl; p += strspn(p, " \t");
        if (!strncmp(p, "text", 4) && (p[4] == ' ' || p[4] == '\t' || !p[4])) {
            /* the rest of the line after one whitespace character: one text
               event per Unicode scalar value, '#' included */
            const uint8_t *s = (const uint8_t *)p + 4 + (p[4] != 0);
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
        for (char *w = strtok(p, " \t"); w; w = strtok(0, " \t")) {
            if (n == 16) BAD("too many arguments");
            t[n++] = w;
        }
        if (!n) BAD("no command");
        const char *c = t[0];
        if (!strcmp(c, "down") || !strcmp(c, "up") || !strcmp(c, "press")) {
            if (n < 2) BAD("missing key");
            uint8_t k = key(t[1]), md = mods(t + 2, n - 2);
            uint8_t dn[4] = {1, k, md, 0}, up[4] = {2, k, md, 0};
            if (c[0] != 'u') add(f, 0, dn, 0);          /* press: down, then up */
            if (c[0] != 'd') add(f, 0, up, 0);
        } else if (!strcmp(c, "pointer") || !strcmp(c, "click")) {
            if (n < 3) BAD("missing position");
            long x = num(t[1], 10, 0, 65535, "x"), y = num(t[2], 10, 0, 65535, "y");
            if (c[0] == 'c') { pointer(f, x, y, 1); pointer(f, x, y, 0); }
            else pointer(f, x, y, n > 3 ? num(t[3], 10, 0, 7, "buttons") : 0);
        } else if (!strcmp(c, "wheel")) {
            if (n < 2) BAD("missing vertical steps");
            uint8_t ev[4] = {6, (uint8_t)num(t[1], 10, -127, 127, "vertical steps"),
                             (uint8_t)(n > 2 ? num(t[2], 10, -127, 127, "horizontal steps") : 0), 0};
            add(f, 0, ev, 0);
        } else if (!strcmp(c, "feeds-updated")) {
            uint8_t ev[4] = {7, 0, 0, 0};
            add(f, 0, ev, 0);
        } else if (!strcmp(c, "pad")) {
            if (n < 3) BAD("pad wants <n> <buttons> <lx> <ly> [<rx> <ry>]");
            int pn = num(t[1], 10, 0, 3, "controller");
            uint8_t ps[8] = {0};
            if (n != 3 || strcmp(t[2], "off")) {
                if (n != 5 && n != 7) BAD("pad wants <n> <buttons> <lx> <ly> [<rx> <ry>]");
                long b = num(t[2] + 2 * (t[2][0] == '0' && t[2][1] == 'x'), 16, 0, 0xFFFF, "buttons");
                ps[0] = 1; ps[1] = b; ps[2] = b >> 8;
                for (int i = 0; i + 3 < n; i++) ps[3 + i] = (uint8_t)num(t[3 + i], 10, -127, 127, "stick");
            }
            add(f, 1 + pn, 0, ps);
        } else if (!strcmp(c, "home")) {
            /* a shell's only: dropped for a machine run alone */
        } else
            BAD("unknown command %s", c);
    }
    free(line);
    fclose(fp);
    qsort(in, nin, sizeof *in, by_frame);   /* events of a frame in the order written */
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

/* The terminal's grid, from its STATE section (DEVICES.md 1.7, STATE.md
   1.6), as STATE.md 6.3 prints it. */
static void dump(M *m)
{
    uint8_t *s; uint32_t n;
    if (!m->has[2] || !bus_state(m, 2, &s, &n) || n < 9600) return;
    for (int row = 0; row < 30; row++) {
        char line[80 * 3 + 1];
        size_t len = 0;
        for (int c = 0; c < 80; c++) {
            uint32_t ch = rd16(s + 4 * (row * 80 + c));
            if (ch == 0xFFFF) {
                /* the right half of a wide pair is skipped; any other is a space */
                if (c > 0 && rd16(s + 4 * (row * 80 + c - 1)) != 0xFFFF) continue;
                ch = ' ';
            }
            if (ch >= 0xD800 && ch < 0xE000) ch = 0xFFFD;
            if (ch < 0x80) line[len++] = ch;
            else if (ch < 0x800) { line[len++] = 0xC0 | ch >> 6; line[len++] = 0x80 | (ch & 0x3F); }
            else { line[len++] = 0xE0 | ch >> 12; line[len++] = 0x80 | (ch >> 6 & 0x3F); line[len++] = 0x80 | (ch & 0x3F); }
        }
        while (len && line[len - 1] == ' ') len--;
        fwrite(line, 1, len, stdout);
        putchar('\n');
    }
    free(s);
}

static volatile sig_atomic_t quit;
static void on_signal(int s) { (void)s; quit = 1; }

static const char *fault_name[] = {"", "illegal instruction", "breakpoint", "fetch", "load", "store", "misaligned"};

/* tcore --version: its name, version and build identifier, `b3:` and the
   first 8 bytes of the BLAKE3 hash of its own executable. */
static void print_version(void)
{
    char build[24] = "unknown";
    FILE *f = fopen(self_exe(), "rb");
    if (f) {
        size_t n = 0, cap = 1 << 20, k;
        uint8_t *p = malloc(cap), h[32];
        while (p && (k = fread(p + n, 1, cap - n, f)) > 0)
            if ((n += k) == cap) p = realloc(p, cap *= 2);
        if (p && !ferror(f)) {
            blake3(p, n, h);
            snprintf(build, sizeof build, "b3:%02x%02x%02x%02x%02x%02x%02x%02x", h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        }
        free(p);
        fclose(f);
    }
    printf("tcore 0.1.0 %s\n", build);
}

int main(int argc, char **argv)
{
    const char *image = 0, *events = 0, *state_file = 0;
    long frames = 30, every = 0;
    int want_hash = 0, want_dump = 0, bench = 0;
    signal(SIGPIPE, SIG_IGN);
    signal(SIGINT, on_signal); signal(SIGTERM, on_signal); signal(SIGHUP, on_signal);
    bus_driver_dir = getenv("TERSE_DRIVERS");
    int identities = 0;
    if (argc == 2 && !strcmp(argv[1], "--version")) { print_version(); return 0; }
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
        else if (!strcmp(a, "--identities")) identities = 1;
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
    if (identities) bus_identities();

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
