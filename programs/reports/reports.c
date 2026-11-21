/* reports: gathers the performance reports (RECORD.md 5) published by the
 * feeds the user follows and, when asked, a sample found on the relays,
 * and shows a verdict for each host, device and terse perf identity in
 * both samples; built with clang (see build.sh).
 *
 * Gathering. At start the program reads the names of the feeds the host
 * knows (content op 09h: who is followed, and their display names), then
 * the timeline (op 04h), 16 KiB of records at a time, again after every
 * feeds-updated event. Each document entry (kind 4) of at most 8,192
 * bytes that it has not taken before (by feed key and seq) is fetched
 * (at most 4 gets at once) and, if it is a document of the `reports`
 * format (owner key zero, name "reports"), parsed by every rule of
 * RECORD.md 5.2: a report that breaks one is counted as malformed
 * and otherwise ignored; a document of another format is ignored. F asks
 * the relays (find, op 0Ah, scope 2: article and document titles) for
 * "performance report", then for "performance report: <host name>" for
 * each of up to three host names already seen, one search every 75 frames
 * (relays limit how often a client searches; each answers at most 16
 * results); what they find is taken like the timeline's entries. A feed
 * is "followed" if the host is subscribed to it.
 *
 * Counting (RECORD.md 5.5). A report speaks about its host, each
 * device identity (not the host's own) and, if it has a perf line, the
 * terse perf that ran. For each (reporter key, identity) only the fact of
 * the entry with the highest seq is kept: whether R15 halted it, whether
 * a perf requirement or ceiling failed, whether it was blamed or warned
 * about, the computer's class, and one measure (a host's median
 * start-up, a device's blamed frames per 10,000, a perf run's targets
 * missed). Nothing else of a report is kept, and no count in a report is
 * trusted beyond its own reporter. The verdicts are computed from the
 * facts, at most two groups a frame, in the words of RECORD.md 5.5.
 *
 * Budget: a frame parses at most PARSE_BYTES of reports (line by line;
 * a quarter of that in a frame that may draw the screen),
 * scans at most SCAN_RECORDS timeline records and computes at most two
 * groups' tallies; the screen is drawn at most every 6 frames while
 * reports arrive. Everything is static RAM.
 *
 * Screens. The list: three rows a group (its kind and identity, then the
 * verdict in the feeds the user follows and in all found); Up and Down
 * choose, Enter shows the group: its numbers in both samples and its
 * reporters (display name or nickname and short key, followed first, the
 * computer's class, what failed). Esc goes back (on the list it does
 * nothing: the host's home key leaves). P publishes the aggregate as an
 * article (a table of every group's two verdicts, the sample sizes, the
 * rule and the date), which the host asks the user about.
 *
 * Devices: Terminal (2) and Content (4) are required; Full text (10) is
 * optional (display names in any script). The page is not used. */

#include <stdint.h>
#include "terse.h"

#define DOC_MAX 8192
#define HEAD 45 /* TDOC, 0, 32 zero bytes, 7, "reports" */
#define PARSE_BYTES 2048
#define SCAN_RECORDS 64
#define MAX_GROUPS 48
#define MAX_REPORTERS 256
#define MAX_FACTS 1024
#define FACT_SLOTS 2048
#define MAX_DONE 1536
#define DONE_SLOTS 2048
#define QUEUE 64
#define GETS 4
#define LABEL_MAX 72
#define NAME_MAX 48
#define FIND_GAP 75
#define MIN_REPORTERS 5
#define ONE_IN 3

#define KEY_F 0x09
#define KEY_P 0x13

enum { K_HOST, K_DEVICE, K_PERF };
enum { C_SMALL, C_MEDIUM, C_LARGE };
#define F_HALTED 0x01
#define F_REQ 0x02
#define F_CEIL 0x04
#define F_BLAMED 0x08
#define F_WARNED 0x10
#define F_MEASURE 0x20

typedef struct {
    uint32_t h1, h2;
    uint8_t kind, len;
    uint8_t label[LABEL_MAX];
    int16_t head; /* the first of its facts, -1: none */
} group;

typedef struct {
    uint8_t key[32];
    uint8_t followed, nlen;
    uint8_t name[NAME_MAX];
} reporter;

typedef struct {
    uint16_t rep;
    uint8_t grp, flags, cls;
    int16_t next; /* the group's next fact */
    uint32_t seq, measure;
} fact;

typedef struct {
    uint32_t reporters, below, judged, failing, halted, req, ceil, blamed, warned, median;
    uint8_t has_median;
} tally;

static group groups[MAX_GROUPS];
static uint32_t ngroups;
static reporter reps[MAX_REPORTERS];
static uint32_t nreps;
static fact facts[MAX_FACTS];
static uint32_t nfacts;
static int16_t fact_slot[FACT_SLOTS];
static tally tallies[MAX_GROUPS][2];
static uint8_t tally_dirty[MAX_GROUPS];

/* Entries taken: the first 8 bytes of the key and the seq. */
static uint32_t done_k0[DONE_SLOTS], done_k1[DONE_SLOTS], done_seq[DONE_SLOTS];
static uint8_t done_used[DONE_SLOTS];
static uint32_t ndone;

/* Entries waiting to be fetched. */
typedef struct {
    uint8_t key[32], blob[32];
    uint32_t seq, size;
    uint8_t followed;
} cand;
static cand queue[QUEUE];
static uint32_t q_head, q_n;

/* The gets: each its own buffer; 0 free, 1 asked, 2 arrived. */
static content_req get_req[GETS];
static uint8_t get_h[GETS], get_state[GETS];
static cand get_cand[GETS];
static uint8_t get_buf[GETS][DOC_MAX];

/* The names of the feeds the host knows. */
static uint8_t names_buf[16384];
static content_req names_req;
static uint8_t names_h, names_done;
static uint32_t names_next;
typedef struct {
    uint8_t key[32];
    uint8_t flags, len;
    uint8_t name[NAME_MAX];
} known;
#define MAX_KNOWN 256
static known knowns[MAX_KNOWN];
static uint32_t nknown;

/* The timeline. */
static uint8_t tl_buf[16384];
static content_req tl_req;
static uint8_t tl_h, tl_again = 1, tl_have;
static uint32_t tl_next, tl_pos, tl_len, tl_count, tl_total;

/* Searching the relays. */
static uint8_t find_buf[16384];
static content_req find_req;
static uint8_t find_h, find_on, find_n, find_have, find_queries;
static uint32_t find_at, find_pos, find_len, find_count, find_results;
static char find_q[4][49];
static uint8_t find_qlen[4];

/* Publishing. */
static uint8_t art[16384];
static uint32_t art_len;
static content_req put_req, pub_req;
static uint8_t put_h, pub_h;

static uint32_t reports_in, malformed, ignored, from_found;
static uint32_t frame_n, drawn_at;
static uint8_t dirty = 1, view, has_full;
static uint32_t sel, top, dsel_top;
static char msg[80];
static uint8_t msg_len;

/* --- Small helpers -------------------------------------------------------- */

static uint32_t get32(const uint8_t *p)
{
    return p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

static void say(const char *s)
{
    msg_len = 0;
    while (*s && msg_len < sizeof msg)
        msg[msg_len++] = *s++;
    dirty = 1;
}

/* floor(a * b / c) for c > 0, without a 64-bit division routine. */
static uint32_t muldiv(uint32_t a, uint32_t b, uint32_t c)
{
    uint64_t n = (uint64_t)a * b, q = 0, r = 0;
    for (int i = 63; i >= 0; i--) {
        r = r << 1 | (n >> i & 1);
        if (r >= c) {
            r -= c;
            q |= (uint64_t)1 << i;
        }
    }
    return (uint32_t)q;
}

/* --- Parsing a report (RECORD.md 5.2) ------------------------------- */

/* The known line types, in their order. */
static const char *const types[] = { "terse-report", "period", "host", "env", "device", "runs", "start", "late", "warn",
                                     "halt", "shed", "mem", "check", "perf", "bench", "program", "end" };
#define NTYPES 17
enum { T_REPORT, T_PERIOD, T_HOST, T_ENV, T_DEVICE, T_RUNS, T_START, T_LATE, T_WARN, T_HALT, T_SHED, T_MEM, T_CHECK, T_PERF, T_BENCH, T_PROGRAM, T_END };
static const uint8_t need[NTYPES] = { 1, 2, 3, 7, 5, 4, 5, 5, 2, 2, 2, 3, 3, 4, 6, 2, 1 };

#define MAX_F 8
#define SLOTS 20 /* core, compose, present, outside, then the devices */

static struct {
    const uint8_t *p, *end;
    uint32_t line;
    uint8_t rank, ended, seen[NTYPES];
    uint32_t ih1, ih2; /* the identity just parsed (unkinded) */
    uint8_t ilabel[LABEL_MAX], ilen;
    uint32_t hh1, hh2;
    uint8_t hlabel[LABEL_MAX], hlen;
    uint32_t cores, ram;
    uint8_t ndev;
    uint16_t dev_code[16];
    uint32_t dh1[16], dh2[16];
    uint8_t dlabel[16][LABEL_MAX], dlen[16];
    uint32_t frames, start_med;
    uint8_t has_start;
    uint32_t blamed[SLOTS];
    uint8_t halted[SLOTS], warned[SLOTS];
    uint32_t dup[5]; /* late, warn, halt, mem, check */
    uint8_t n5[5], nbench, nprog;
    uint8_t has_perf, req_fail, ceil_fail;
    uint32_t missed;
    uint32_t ph1, ph2;
    uint8_t plabel[LABEL_MAX], plen;
    uint8_t bench_id[64][24], bench_len[64];
} P;

static const uint8_t *fs[MAX_F + 1];
static uint8_t fl[MAX_F + 1];
static uint32_t nf;

static uint8_t eq(uint32_t i, const char *s)
{
    uint32_t n = 0;
    while (s[n])
        n++;
    return fl[i] == n && !memcmp(fs[i], s, n);
}

/* An integer: 0, or 1 to 10 digits without a leading zero, at most
 * 4294967295. */
static uint8_t num(uint32_t i, uint32_t *v)
{
    const uint8_t *s = fs[i];
    uint32_t n = fl[i], x = 0;
    if (!n || n > 10 || (n > 1 && s[0] == '0'))
        return 0;
    for (uint32_t k = 0; k < n; k++) {
        uint32_t d = s[k] - '0';
        if (d > 9)
            return 0;
        if (x > 429496729u || (x == 429496729u && d > 5))
            return 0;
        x = x * 10 + d;
    }
    *v = x;
    return 1;
}

/* A decimal with exactly `dec` digits after the point and 1 to 7 before
 * it (no leading zero), as an integer of hundredths or tenths; `neg`
 * allows a minus sign. */
static uint8_t fixed(uint32_t i, uint32_t dec, uint8_t neg, uint32_t *v)
{
    const uint8_t *s = fs[i];
    uint32_t n = fl[i], k = 0, x = 0, ip = 0;
    if (n && s[0] == '-') {
        if (!neg)
            return 0;
        k = 1;
    }
    while (k < n && s[k] != '.') {
        if ((uint32_t)(s[k] - '0') > 9)
            return 0;
        x = x * 10 + (s[k] - '0');
        ip++;
        k++;
        if (ip > 7)
            return 0;
    }
    if (!ip || (ip > 1 && s[k - ip] == '0') || k == n || n - k - 1 != dec)
        return 0;
    for (k++; k < n; k++) {
        if ((uint32_t)(s[k] - '0') > 9)
            return 0;
        x = x * 10 + (s[k] - '0');
    }
    *v = x;
    return 1;
}

static uint8_t bench_value(uint32_t i)
{
    const uint8_t *s = fs[i];
    uint32_t n = fl[i], k = 0;
    while (k < n && s[k] != '.') {
        if ((uint32_t)(s[k] - '0') > 9)
            return 0;
        k++;
    }
    if (!k || k > 7 || (k > 1 && s[0] == '0'))
        return 0;
    if (k == n)
        return 1;
    if (n - k - 1 < 1 || n - k - 1 > 3)
        return 0;
    for (k++; k < n; k++)
        if ((uint32_t)(s[k] - '0') > 9)
            return 0;
    return 1;
}

static uint8_t two(const uint8_t *s, uint32_t *v)
{
    if ((uint32_t)(s[0] - '0') > 9 || (uint32_t)(s[1] - '0') > 9)
        return 0;
    *v = (s[0] - '0') * 10 + (s[1] - '0');
    return 1;
}

static uint8_t date(uint32_t i)
{
    const uint8_t *s = fs[i];
    uint32_t y1, y2, m, d;
    static const uint8_t days[12] = { 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31 };
    if (fl[i] != 10 || s[4] != '-' || s[7] != '-' || !two(s, &y1) || !two(s + 2, &y2) || !two(s + 5, &m) || !two(s + 8, &d))
        return 0;
    uint32_t y = y1 * 100 + y2, leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    if (y < 2000 || m < 1 || m > 12 || d < 1)
        return 0;
    return d <= days[m - 1] + (m == 2 && leap);
}

static uint8_t hexup(uint8_t c) { return (uint32_t)(c - '0') < 10 || (uint32_t)(c - 'A') < 6; }
static uint8_t hexval(uint8_t c) { return c <= '9' ? c - '0' : c - 'A' + 10; }

/* A string field: `%` only before two uppercase hex digits; at most `max`
 * bytes unescaped ("-" is empty). Unescapes into out (cut at cap) and
 * hashes every byte; returns the unescaped length + 1, or 0. */
static uint32_t h1, h2;
static void hash_byte(uint8_t b)
{
    h1 = (h1 ^ b) * 16777619u;
    h2 = (h2 ^ b) * 2246822519u + 1;
}
static uint32_t string(uint32_t i, uint32_t max, uint8_t *out, uint32_t *olen, uint32_t cap)
{
    const uint8_t *s = fs[i];
    uint32_t n = fl[i], len = 0;
    if (n == 1 && s[0] == '-')
        return 1;
    for (uint32_t k = 0; k < n;) {
        uint8_t b = s[k];
        if (b == '%') {
            if (k + 2 >= n || !hexup(s[k + 1]) || !hexup(s[k + 2]))
                return 0;
            b = hexval(s[k + 1]) << 4 | hexval(s[k + 2]);
            k += 3;
        } else {
            k++;
        }
        len++;
        hash_byte(b);
        if (out && *olen < cap)
            out[(*olen)++] = b;
    }
    return len <= max ? len + 1 : 0;
}

/* A version or build: "-" or 1 to 32 bytes without `%`. */
static uint8_t version(uint32_t i)
{
    if (fl[i] > 32)
        return 0;
    for (uint32_t k = 0; k < fl[i]; k++)
        if (fs[i][k] == '%')
            return 0;
    return 1;
}

static void label_add(const uint8_t *s, uint32_t n)
{
    for (uint32_t k = 0; k < n && P.ilen < LABEL_MAX; k++)
        P.ilabel[P.ilen++] = s[k];
}

/* An identity from fields i..i+2 into P.ih1/ih2 and P.ilabel. */
static uint8_t identity3(uint32_t i)
{
    h1 = 2166136261u;
    h2 = 0x9747b28cu;
    P.ilen = 0;
    uint32_t lab = 0;
    uint32_t r = string(i, 64, P.ilabel, &lab, LABEL_MAX);
    if (r <= 1 || !version(i + 1) || !version(i + 2))
        return 0;
    P.ilen = lab;
    hash_byte(0);
    for (uint32_t k = 1; k <= 2; k++) {
        label_add((const uint8_t *)" ", 1);
        if (fl[i + k] == 1 && fs[i + k][0] == '-') {
            label_add((const uint8_t *)"-", 1);
        } else {
            for (uint32_t j = 0; j < fl[i + k]; j++)
                hash_byte(fs[i + k][j]);
            label_add(fs[i + k], fl[i + k]);
        }
        hash_byte(0);
    }
    P.ih1 = h1;
    P.ih2 = h2;
    return 1;
}

/* A component code: 0x100 core, 0x101 compose, 0x102 present, 0x103
 * outside, n for dev<n> (written exactly so); 0xFFFF if none. */
static uint16_t component(uint32_t i)
{
    static const char *const host[] = { "core", "compose", "present", "outside" };
    for (uint32_t k = 0; k < 4; k++)
        if (eq(i, host[k]))
            return 0x100 + k;
    const uint8_t *s = fs[i];
    uint32_t n = fl[i], v = 0;
    if (n < 4 || n > 6 || s[0] != 'd' || s[1] != 'e' || s[2] != 'v' || (n > 4 && s[3] == '0'))
        return 0xFFFF;
    for (uint32_t k = 3; k < n; k++) {
        if ((uint32_t)(s[k] - '0') > 9)
            return 0xFFFF;
        v = v * 10 + (s[k] - '0');
    }
    return v > 255 ? 0xFFFF : v;
}

/* The slot of a declared component, or 0xFF. */
static uint8_t slot_of(uint16_t c)
{
    if (c >= 0x100 && c <= 0x103)
        return c - 0x100;
    for (uint8_t k = 0; k < P.ndev; k++)
        if (P.dev_code[k] == c)
            return 4 + k;
    return 0xFF;
}

/* Starts parsing the text [p, end). */
static void parse_start(const uint8_t *p, const uint8_t *end)
{
    /* Only what is read before it is written (the labels and the bench
     * ids are written before they are read). */
    P.p = p;
    P.end = end;
    P.line = P.rank = P.ended = P.ndev = P.nbench = P.nprog = 0;
    P.has_start = P.has_perf = P.req_fail = P.ceil_fail = 0;
    P.frames = P.start_med = P.missed = 0;
    memset(P.seen, 0, sizeof P.seen);
    memset(P.blamed, 0, sizeof P.blamed);
    memset(P.halted, 0, sizeof P.halted);
    memset(P.warned, 0, sizeof P.warned);
    memset(P.dup, 0, sizeof P.dup);
    memset(P.n5, 0, sizeof P.n5);
}

/* One line [s, e); 0 if it breaks a rule. */
static uint8_t parse_line(const uint8_t *s, const uint8_t *e)
{
    uint32_t n = ++P.line, len = e - s;
    if (P.ended || len == 0 || len > 255 || s[0] == ' ' || e[-1] == ' ')
        return 0;
    nf = 0;
    const uint8_t *w = s;
    for (const uint8_t *q = s; q <= e; q++) {
        if (q == e || *q == ' ') {
            if (q == w)
                return 0; /* two spaces */
            if (nf <= MAX_F) {
                fs[nf] = w;
                fl[nf] = q - w > 255 ? 255 : q - w;
            }
            nf++;
            w = q + 1;
        } else if (*q < 0x20 || *q > 0x7E) {
            return 0;
        }
    }
    uint32_t t = NTYPES;
    for (uint32_t k = 0; k < NTYPES; k++)
        if (eq(0, types[k])) {
            t = k;
            break;
        }
    if (t == NTYPES)
        return n != 1; /* a later version's type: skipped, but not first */
    if (n == 1 && t != T_REPORT)
        return 0;
    if (t < P.rank)
        return 0;
    P.rank = t;
    P.seen[t]++;
    if (nf - 1 < need[t])
        return 0;
    if (P.seen[t] > 1 && (t <= T_ENV || t == T_RUNS || t == T_START || t == T_SHED || t == T_PERF || t == T_END))
        return 0;
    /* The fields after the type, from 1. */
    uint32_t v, a, b, c, d;
    switch (t) {
    case T_REPORT:
        return num(1, &v) && v == 1;
    case T_PERIOD:
        if (!date(1) || !date(2) || memcmp(fs[1], fs[2], 10) > 0)
            return 0;
        return 1;
    case T_HOST:
        if (!identity3(1))
            return 0;
        P.hh1 = P.ih1;
        P.hh2 = P.ih2;
        memcpy(P.hlabel, P.ilabel, P.ilen);
        P.hlen = P.ilen;
        return 1;
    case T_ENV: {
        uint32_t z = 0;
        if (!num(4, &P.cores) || P.cores < 1 || P.cores > 4096 || !num(5, &P.ram) || P.ram > 65536)
            return 0;
        return string(1, 64, 0, &z, 0) && string(2, 64, 0, &z, 0) && string(3, 32, 0, &z, 0) && string(6, 32, 0, &z, 0)
               && string(7, 128, 0, &z, 0);
    }
    case T_DEVICE: {
        if (P.ndev >= 16)
            return 0;
        uint16_t code = component(1);
        if (code == 0xFFFF || (code >= 0x100 && code != 0x100))
            return 0;
        for (uint32_t k = 0; k < P.ndev; k++)
            if (P.dev_code[k] == code)
                return 0;
        if (!eq(2, "-")) {
            /* slots: 1 to 255, comma-separated */
            const uint8_t *q = fs[2], *qe = fs[2] + fl[2];
            while (1) {
                const uint8_t *st = q;
                uint32_t x = 0;
                while (q < qe && *q != ',') {
                    if ((uint32_t)(*q - '0') > 9)
                        return 0;
                    x = x * 10 + (*q - '0');
                    if (q - st > 3)
                        return 0;
                    q++;
                }
                if (q == st || (q - st > 1 && *st == '0') || x < 1 || x > 255)
                    return 0;
                if (q == qe)
                    break;
                q++;
            }
        }
        if (!identity3(3))
            return 0;
        uint8_t k = P.ndev++;
        P.dev_code[k] = code;
        P.dh1[k] = P.ih1;
        P.dh2[k] = P.ih2;
        memcpy(P.dlabel[k], P.ilabel, P.ilen);
        P.dlen[k] = P.ilen;
        return 1;
    }
    case T_RUNS:
        if (!num(1, &a) || !num(2, &b) || !num(3, &c) || !num(4, &d) || b > a || d > c)
            return 0;
        P.frames = c;
        return 1;
    case T_START: {
        uint32_t med, worst, cold, cw = 0;
        if (!num(1, &a) || !fixed(2, 1, 0, &med) || !fixed(3, 1, 0, &worst) || !num(4, &cold) || cold > a)
            return 0;
        uint8_t has_cw = !eq(5, "-");
        if (has_cw && !fixed(5, 1, 0, &cw))
            return 0;
        if (a == 0 || med > worst || (cold == 0) != !has_cw)
            return 0;
        P.has_start = 1;
        P.start_med = med;
        return 1;
    }
    case T_LATE:
    case T_WARN:
    case T_HALT:
    case T_MEM:
    case T_CHECK: {
        uint32_t which = t == T_LATE ? 0 : t == T_WARN ? 1 : t == T_HALT ? 2 : t == T_MEM ? 3 : 4;
        if (P.n5[which] >= 20)
            return 0;
        uint16_t code = component(1);
        if (code == 0xFFFF)
            return 0;
        uint8_t sl = slot_of(code);
        if (sl == 0xFF)
            return 0;
        if (P.dup[which] >> sl & 1)
            return 0;
        P.dup[which] |= 1u << sl;
        P.n5[which]++;
        if (t == T_LATE) {
            if (!num(2, &a) || !num(3, &b) || !num(4, &c) || !num(5, &d))
                return 0;
            P.blamed[sl] = a;
            return 1;
        }
        if (t == T_WARN || t == T_HALT) {
            if (!num(2, &a) || a < 1)
                return 0;
            if (t == T_WARN)
                P.warned[sl] = 1;
            else
                P.halted[sl] = 1;
            return 1;
        }
        if (t == T_MEM)
            return fixed(2, 1, 0, &a) && (eq(3, "-") || fixed(3, 2, 1, &b));
        return fixed(2, 1, 0, &a) && (eq(3, "ok") || eq(3, "shed") || eq(3, "slow") || eq(3, "absent"));
    }
    case T_SHED:
        return num(1, &a) && num(2, &b);
    case T_PERF:
        if (!identity3(1) || !date(4))
            return 0;
        P.has_perf = 1;
        P.ph1 = P.ih1;
        P.ph2 = P.ih2;
        memcpy(P.plabel, P.ilabel, P.ilen);
        P.plen = P.ilen;
        return 1;
    case T_BENCH: {
        uint32_t z = 0;
        if (!P.has_perf || P.nbench >= 64 || fl[1] > 24)
            return 0;
        for (uint32_t k = 0; k < fl[1]; k++)
            if (fs[1][k] == '%')
                return 0;
        for (uint32_t k = 0; k < P.nbench; k++)
            if (P.bench_len[k] == fl[1] && !memcmp(P.bench_id[k], fs[1], fl[1]))
                return 0;
        memcpy(P.bench_id[P.nbench], fs[1], fl[1]);
        P.bench_len[P.nbench++] = fl[1];
        if (!eq(2, "-") && !bench_value(2))
            return 0;
        if (!string(3, 16, 0, &z, 0))
            return 0;
        if (!(eq(4, "pass") || eq(4, "fail") || eq(4, "-")) || !(eq(5, "pass") || eq(5, "fail") || eq(5, "-"))
            || !(eq(6, "met") || eq(6, "missed") || eq(6, "-")))
            return 0;
        P.req_fail |= eq(4, "fail");
        P.ceil_fail |= eq(5, "fail");
        P.missed += eq(6, "missed");
        return 1;
    }
    case T_PROGRAM:
        if (P.nprog >= 5 || fl[1] != 16)
            return 0;
        for (uint32_t k = 0; k < 16; k++)
            if ((uint32_t)(fs[1][k] - '0') > 9 && (uint32_t)(fs[1][k] - 'a') > 5)
                return 0;
        P.nprog++;
        return num(2, &a);
    case T_END:
        if (!num(1, &a) || a != n - 1)
            return 0;
        P.ended = 1;
        return 1;
    }
    return 0;
}

/* --- The facts ------------------------------------------------------------- */

static uint32_t group_of(uint8_t kind, uint32_t a, uint32_t b, const uint8_t *label, uint32_t len)
{
    for (uint32_t g = 0; g < ngroups; g++)
        if (groups[g].kind == kind && groups[g].h1 == a && groups[g].h2 == b)
            return g;
    if (ngroups == MAX_GROUPS)
        return MAX_GROUPS;
    group *g = &groups[ngroups];
    g->kind = kind;
    g->h1 = a;
    g->h2 = b;
    g->len = len;
    memcpy(g->label, label, len);
    g->head = -1;
    return ngroups++;
}

static const known *known_of(const uint8_t *key)
{
    for (uint32_t i = 0; i < nknown; i++)
        if (!memcmp(knowns[i].key, key, 32))
            return &knowns[i];
    return 0;
}

static uint32_t reporter_of(const uint8_t *key)
{
    for (uint32_t i = 0; i < nreps; i++)
        if (!memcmp(reps[i].key, key, 32))
            return i;
    if (nreps == MAX_REPORTERS)
        return MAX_REPORTERS;
    reporter *r = &reps[nreps];
    memcpy(r->key, key, 32);
    const known *k = known_of(key);
    r->followed = k && !(k->flags & 0x04);
    r->nlen = k ? k->len : 0;
    if (k)
        memcpy(r->name, k->name, k->len);
    return nreps++;
}

/* Keeps the fact of (rep, g) if its seq is higher than the one kept. */
static void put_fact(uint32_t rep, uint32_t g, uint32_t seq, uint8_t flags, uint8_t cls, uint32_t measure)
{
    if (g >= MAX_GROUPS)
        return;
    uint32_t s = (rep * 2654435761u ^ g * 40503u) & (FACT_SLOTS - 1);
    while (fact_slot[s]) {
        fact *f = &facts[fact_slot[s] - 1];
        if (f->rep == rep && f->grp == g) {
            if (f->seq >= seq)
                return;
            f->seq = seq;
            f->flags = flags;
            f->cls = cls;
            f->measure = measure;
            tally_dirty[g] = 1;
            return;
        }
        s = (s + 1) & (FACT_SLOTS - 1);
    }
    if (nfacts == MAX_FACTS)
        return;
    fact *f = &facts[nfacts];
    f->rep = rep;
    f->grp = g;
    f->seq = seq;
    f->flags = flags;
    f->cls = cls;
    f->measure = measure;
    f->next = groups[g].head;
    groups[g].head = nfacts;
    fact_slot[s] = ++nfacts;
    tally_dirty[g] = 1;
}

/* A parsed report's facts. */
static void commit(const cand *c)
{
    uint32_t rep = reporter_of(c->key);
    if (rep == MAX_REPORTERS)
        return;
    uint8_t cls = P.cores < 4 || P.ram < 4 ? C_SMALL : P.cores >= 8 && P.ram >= 16 ? C_LARGE : C_MEDIUM;
    uint8_t fl = 0;
    uint32_t b = 0;
    for (uint32_t k = 0; k < 4; k++) {
        b += P.blamed[k];
        fl |= (P.halted[k] ? F_HALTED : 0) | (P.warned[k] ? F_WARNED : 0);
    }
    if (b)
        fl |= F_BLAMED;
    if (P.has_start)
        fl |= F_MEASURE;
    put_fact(rep, group_of(K_HOST, P.hh1, P.hh2, P.hlabel, P.hlen), c->seq, fl, cls, P.start_med);
    /* Devices: one group per identity, not the host's own. */
    for (uint32_t i = 0; i < P.ndev; i++) {
        if (P.dev_code[i] >= 0x100 || (P.dh1[i] == P.hh1 && P.dh2[i] == P.hh2))
            continue;
        uint32_t j;
        for (j = 0; j < i; j++)
            if (P.dev_code[j] < 0x100 && P.dh1[j] == P.dh1[i] && P.dh2[j] == P.dh2[i])
                break;
        if (j < i)
            continue; /* counted with the first */
        uint32_t blamed = 0;
        fl = 0;
        for (j = i; j < P.ndev; j++) {
            if (P.dev_code[j] >= 0x100 || P.dh1[j] != P.dh1[i] || P.dh2[j] != P.dh2[i])
                continue;
            blamed += P.blamed[4 + j];
            fl |= (P.halted[4 + j] ? F_HALTED : 0) | (P.warned[4 + j] ? F_WARNED : 0);
        }
        if (blamed)
            fl |= F_BLAMED;
        uint32_t m = 0;
        if (P.frames) {
            fl |= F_MEASURE;
            m = muldiv(blamed < P.frames ? blamed : P.frames, 10000, P.frames);
        }
        put_fact(rep, group_of(K_DEVICE, P.dh1[i], P.dh2[i], P.dlabel[i], P.dlen[i]), c->seq, fl, cls, m);
    }
    if (P.has_perf) {
        fl = F_MEASURE | (P.req_fail ? F_REQ : 0) | (P.ceil_fail ? F_CEIL : 0);
        put_fact(rep, group_of(K_PERF, P.ph1, P.ph2, P.plabel, P.plen), c->seq, fl, cls, P.missed);
    }
    reports_in++;
    if (!c->followed)
        from_found++;
    dirty = 1;
}

/* --- Tallies --------------------------------------------------------------- */

static uint32_t mbuf[MAX_REPORTERS];

static void compute(uint32_t g, uint32_t s)
{
    tally *t = &tallies[g][s];
    uint32_t n = 0;
    memset(t, 0, sizeof *t);
    for (int32_t i = groups[g].head; i >= 0; i = facts[i].next) {
        fact *f = &facts[i];
        if (s == 0 && !reps[f->rep].followed)
            continue;
        t->reporters++;
        if (f->cls == C_SMALL) {
            t->below++;
            continue;
        }
        t->judged++;
        if (f->flags & (F_HALTED | F_REQ | F_CEIL)) {
            t->failing++;
            t->halted += !!(f->flags & F_HALTED);
            t->req += !!(f->flags & F_REQ);
            t->ceil += !!(f->flags & F_CEIL);
        }
        t->blamed += !!(f->flags & F_BLAMED);
        t->warned += !!(f->flags & F_WARNED);
        if ((f->flags & F_MEASURE) && n < MAX_REPORTERS)
            mbuf[n++] = f->measure;
    }
    /* Shell sort, then the upper median. */
    for (uint32_t gap = n / 2; gap; gap /= 2)
        for (uint32_t i = gap; i < n; i++)
            for (uint32_t j = i; j >= gap && mbuf[j - gap] > mbuf[j]; j -= gap) {
                uint32_t x = mbuf[j];
                mbuf[j] = mbuf[j - gap];
                mbuf[j - gap] = x;
            }
    t->has_median = n != 0;
    t->median = n ? mbuf[n / 2] : 0;
}

/* --- Text ------------------------------------------------------------------ */

static char tb[256];
static uint32_t tn;

static void ts(const char *s)
{
    while (*s && tn < sizeof tb)
        tb[tn++] = *s++;
}

static void tu(uint32_t v)
{
    char d[10];
    uint32_t n = 0;
    do {
        d[n++] = '0' + v % 10;
        v /= 10;
    } while (v);
    while (n && tn < sizeof tb)
        tb[tn++] = d[--n];
}

static void tbytes(const uint8_t *s, uint32_t n)
{
    for (uint32_t i = 0; i < n && tn < sizeof tb; i++)
        tb[tn++] = s[i];
}

static void verdict(const tally *t)
{
    if (t->judged < MIN_REPORTERS) {
        ts("TOO FEW REPORTS (");
        tu(t->judged);
        ts(")");
        return;
    }
    if (t->failing * ONE_IN >= t->judged) {
        ts("NON-CONFORMANT IN SAMPLE (");
        tu(t->failing);
        ts(" of ");
        tu(t->judged);
        ts(" fail: ");
        uint8_t any = 0;
        if (t->halted) {
            ts("halted ");
            tu(t->halted);
            any = 1;
        }
        if (t->req) {
            ts(any ? ", requirement " : "requirement ");
            tu(t->req);
            any = 1;
        }
        if (t->ceil) {
            ts(any ? ", ceiling " : "ceiling ");
            tu(t->ceil);
        }
        ts(")");
        return;
    }
    ts("CONFORMANT IN SAMPLE (");
    tu(t->judged);
    ts(" reporters)");
}

static void measure(uint8_t kind, const tally *t)
{
    if (!t->has_median) {
        ts("-");
        return;
    }
    if (kind == K_HOST) {
        ts("start ");
        tu((t->median + 5) / 10);
        ts(" ms");
    } else if (kind == K_DEVICE) {
        ts("blamed ");
        tu(t->median);
        ts("/10k");
    } else {
        ts("targets missed ");
        tu(t->median);
    }
}

static const char *const kinds[] = { "host", "device", "perf" };

/* --- The screen ------------------------------------------------------------ */

/* Writes tb as row r (cut at 80 columns), padded with spaces. */
static void row(uint8_t r, uint8_t fg, uint8_t bg)
{
    term_goto(0, r);
    term_color(fg, bg);
    /* Count the columns: one per character (bytes 80h-BFh continue one). */
    uint32_t cols = 0, i = 0;
    while (i < tn) {
        uint8_t c = tb[i];
        if ((c & 0xC0) != 0x80) {
            if (cols == 80)
                break;
            cols++;
        }
        i++;
    }
    term_write(tb, i);
    if (cols < 80)
        term_fill(cols, r, 80 - cols, ' ', fg, bg);
    tn = 0;
}

/* The order of the groups: kind, then label. */
static uint8_t order[MAX_GROUPS];

static int8_t lcmp(const group *a, const group *b)
{
    if (a->kind != b->kind)
        return a->kind < b->kind ? -1 : 1;
    uint32_t n = a->len < b->len ? a->len : b->len;
    int c = memcmp(a->label, b->label, n);
    if (c)
        return c < 0 ? -1 : 1;
    return a->len == b->len ? 0 : a->len < b->len ? -1 : 1;
}

static void sort_groups(void)
{
    for (uint32_t i = 0; i < ngroups; i++) {
        uint32_t j = i;
        order[i] = i;
        while (j && lcmp(&groups[order[j - 1]], &groups[i]) > 0) {
            order[j] = order[j - 1];
            j--;
        }
        order[j] = i;
    }
}

static void group_label(uint32_t g)
{
    ts(kinds[groups[g].kind]);
    ts(" ");
    tbytes(groups[g].label, groups[g].len);
}

static void short_key(const uint8_t *key)
{
    static const char hex[] = "0123456789abcdef";
    for (uint32_t i = 0; i < 3; i++) {
        tb[tn++] = hex[key[i] >> 4];
        tb[tn++] = hex[key[i] & 15];
    }
    ts("\xE2\x80\xA6");
    for (uint32_t i = 29; i < 32; i++) {
        tb[tn++] = hex[key[i] >> 4];
        tb[tn++] = hex[key[i] & 15];
    }
}

static void header(void)
{
    tn = 0;
    ts(" Performance reports");
    row(0, 15, 4);
    ts("Trust \"feeds you follow\": anyone can make keys, so \"all found\" may hold fakes.");
    row(1, 11, 0);
    uint32_t followed = 0;
    for (uint32_t i = 0; i < nreps; i++)
        followed += reps[i].followed;
    ts("Followed: ");
    tu(followed);
    ts(" reporters.  All found: ");
    tu(nreps);
    ts(" reporters, ");
    tu(reports_in);
    ts(" reports, ");
    tu(malformed);
    ts(" malformed.");
    row(2, 7, 0);
}

static void footer(const char *keys)
{
    tbytes((const uint8_t *)msg, msg_len);
    row(28, 14, 0);
    ts(keys);
    row(29, 0, 7);
}

#define LIST_TOP 4
#define LIST_ROWS 8

static void draw_list(void)
{
    header();
    row(3, 7, 0);
    if (!ngroups) {
        ts(tl_have ? "  No performance reports in the feeds you follow yet. F looks on the relays." : "  Reading the timeline\xE2\x80\xA6");
        row(LIST_TOP, 7, 0);
        for (uint8_t r = LIST_TOP + 1; r < 28; r++)
            row(r, 7, 0);
    }
    for (uint32_t i = 0; ngroups && i < LIST_ROWS; i++) {
        uint32_t k = top + i;
        uint8_t r = LIST_TOP + i * 3;
        if (k >= ngroups) {
            row(r, 7, 0);
            row(r + 1, 7, 0);
            row(r + 2, 7, 0);
            continue;
        }
        uint32_t g = order[k];
        ts(k == sel ? "> " : "  ");
        group_label(g);
        row(r, k == sel ? 15 : 7, k == sel ? 8 : 0);
        for (uint32_t s = 0; s < 2; s++) {
            ts(s ? "    all found    " : "    you follow   ");
            verdict(&tallies[g][s]);
            const tally *t = &tallies[g][s];
            row(r + 1 + s, t->judged < MIN_REPORTERS ? 8 : t->failing * ONE_IN >= t->judged ? 9 : 10, 0);
        }
    }
    footer(" Up Down choose  Enter details  F find on relays  P publish the aggregate");
}

static void stat_row(uint8_t r, const char *what, uint32_t a, uint32_t b)
{
    ts("  ");
    ts(what);
    while (tn < 34)
        tb[tn++] = ' ';
    uint32_t start = tn;
    tu(a);
    while (tn < start + 18)
        tb[tn++] = ' ';
    tu(b);
    row(r, 7, 0);
}

#define REP_TOP 17
#define REP_ROWS 11

static uint16_t rep_list[MAX_REPORTERS];
static uint32_t rep_n;

/* The group's reporters, followed first, then by key. */
static void list_reporters(uint32_t g)
{
    rep_n = 0;
    for (int32_t i = groups[g].head; i >= 0; i = facts[i].next) {
        uint16_t x = i;
        uint32_t j = rep_n++;
        const reporter *a = &reps[facts[x].rep];
        while (j) {
            const reporter *b = &reps[facts[rep_list[j - 1]].rep];
            if (b->followed > a->followed || (b->followed == a->followed && memcmp(b->key, a->key, 32) < 0))
                break;
            rep_list[j] = rep_list[j - 1];
            j--;
        }
        rep_list[j] = x;
    }
}

static void draw_detail(void)
{
    uint32_t g = order[sel];
    const tally *f = &tallies[g][0], *a = &tallies[g][1];
    tn = 0;
    ts(" ");
    group_label(g);
    row(0, 15, 4);
    ts("Trust \"feeds you follow\": anyone can make keys, so \"all found\" may hold fakes.");
    row(1, 11, 0);
    ts("                                  you follow        all found");
    row(2, 7, 0);
    stat_row(3, "reporters", f->reporters, a->reporters);
    stat_row(4, "below the floor (not judged)", f->below, a->below);
    stat_row(5, "judged", f->judged, a->judged);
    stat_row(6, "failing", f->failing, a->failing);
    stat_row(7, "  halted (R15)", f->halted, a->halted);
    stat_row(8, "  a requirement failed", f->req, a->req);
    stat_row(9, "  a ceiling failed", f->ceil, a->ceil);
    stat_row(10, "blamed for a late frame", f->blamed, a->blamed);
    stat_row(11, "warned about (R15)", f->warned, a->warned);
    ts("  median");
    while (tn < 34)
        tb[tn++] = ' ';
    measure(groups[g].kind, f);
    while (tn < 52)
        tb[tn++] = ' ';
    measure(groups[g].kind, a);
    row(12, 7, 0);
    ts("  you follow: ");
    verdict(f);
    row(13, 15, 0);
    ts("  all found:  ");
    verdict(a);
    row(14, 15, 0);
    row(15, 7, 0);
    ts("Reporters (* followed, first). Floor: 4 cores and 4 GiB; small: below it.");
    row(16, 11, 0);
    list_reporters(g);
    for (uint32_t i = 0; i < REP_ROWS; i++) {
        uint32_t k = dsel_top + i;
        if (k >= rep_n) {
            row(REP_TOP + i, 7, 0);
            continue;
        }
        const fact *x = &facts[rep_list[k]];
        const reporter *r = &reps[x->rep];
        ts(r->followed ? "  * " : "    ");
        uint32_t start = tn;
        if (r->nlen) {
            uint32_t n = r->nlen;
            tbytes(r->name, n > 20 ? 20 : n);
        } else {
            ts(r->followed ? "(no name)" : "(not followed)");
        }
        /* Pad by columns, not bytes. */
        uint32_t cols = 0;
        for (uint32_t j = start; j < tn; j++)
            cols += (tb[j] & 0xC0) != 0x80;
        while (cols++ < 22)
            tb[tn++] = ' ';
        short_key(r->key);
        ts("  ");
        ts(x->cls == C_SMALL ? "small " : x->cls == C_LARGE ? "large " : "medium");
        if (x->flags & F_HALTED)
            ts("  halted");
        if (x->flags & F_REQ)
            ts("  requirement");
        if (x->flags & F_CEIL)
            ts("  ceiling");
        if (x->flags & F_BLAMED)
            ts("  blamed");
        if (x->flags & F_WARNED)
            ts("  warned");
        row(REP_TOP + i, r->followed ? 15 : 7, 0);
    }
    footer(" Up Down scroll the reporters  Esc back  * a feed you follow");
}

/* --- The aggregate as an article ------------------------------------------- */

static void aput(const void *s, uint32_t n)
{
    const uint8_t *p = s;
    for (uint32_t i = 0; i < n && art_len < sizeof art; i++)
        art[art_len++] = p[i];
}

static void as(const char *s)
{
    uint32_t n = 0;
    while (s[n])
        n++;
    aput(s, n);
}

static void atb(void)
{
    aput(tb, tn);
    tn = 0;
}

/* The date of Unix time t, YYYY-MM-DD, into tb. */
static void tdate(uint32_t t)
{
    int32_t z = t / 86400 + 719468;
    int32_t era = z / 146097;
    int32_t doe = z - era * 146097;
    int32_t yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    int32_t y = yoe + era * 400;
    int32_t doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    int32_t mp = (5 * doy + 2) / 153;
    int32_t d = doy - (153 * mp + 2) / 5 + 1;
    int32_t m = mp < 10 ? mp + 3 : mp - 9;
    y += m <= 2;
    tu(y);
    ts(m < 10 ? "-0" : "-");
    tu(m);
    ts(d < 10 ? "-0" : "-");
    tu(d);
}

static char title[64];
static uint32_t title_len;

static void build_article(void)
{
    uint32_t followed = 0;
    for (uint32_t i = 0; i < nreps; i++)
        followed += reps[i].followed;
    art_len = 0;
    tn = 0;
    ts("Performance reports: an aggregate, ");
    tdate(terse_time());
    memcpy(title, tb, tn);
    title_len = tn;
    as("# ");
    atb();
    tn = 0;
    as("\n\nMade with the reports program from the performance reports (RECORD.md 5) it found: ");
    tu(reports_in);
    ts(" reports, ");
    tu(followed);
    ts(" reporters from the feeds I follow and ");
    tu(nreps - followed);
    ts(" others found on the relays.");
    atb();
    as(" Anyone can make feed keys, so only the first column is a sample of people I chose; "
       "neither is a global ruling.\n\nThe rule: one report per feed key and identity (the newest); computers "
       "with fewer than 4 cores or 4 GiB are listed but not judged; a verdict needs 5 judged reporters; "
       "NON-CONFORMANT IN SAMPLE when at least 1 in 3 fail (halted by R15, or a terse perf requirement or "
       "ceiling failed); starved and memory verdicts never count.\n\n| Identity | Feeds I follow | All found |\n|---|---|---|\n");
    for (uint32_t k = 0; k < ngroups; k++) {
        uint32_t g = order[k];
        as("| ");
        tn = 0;
        group_label(g);
        /* A | in a cell is written \|. */
        for (uint32_t i = 0; i < tn; i++) {
            if (tb[i] == '|')
                as("\\");
            aput(&tb[i], 1);
        }
        tn = 0;
        as(" | ");
        verdict(&tallies[g][0]);
        atb();
        as(" | ");
        verdict(&tallies[g][1]);
        atb();
        as(" |\n");
    }
}

/* --- Gathering ------------------------------------------------------------- */

static uint8_t taken(const uint8_t *key, uint32_t seq)
{
    uint32_t k0 = get32(key), k1 = get32(key + 4);
    uint32_t s = (k0 ^ k1 * 31 ^ seq * 2654435761u) & (DONE_SLOTS - 1);
    while (done_used[s]) {
        if (done_k0[s] == k0 && done_k1[s] == k1 && done_seq[s] == seq)
            return 1;
        s = (s + 1) & (DONE_SLOTS - 1);
    }
    if (ndone >= MAX_DONE)
        return 1; /* full: no more */
    done_used[s] = 1;
    done_k0[s] = k0;
    done_k1[s] = k1;
    done_seq[s] = seq;
    ndone++;
    return 0;
}

/* Takes timeline or find records from buf[*pos, len): queues the new
 * document entries; stops when the queue is full or after SCAN_RECORDS. */
static void scan(const uint8_t *buf, uint32_t *pos, uint32_t len, uint8_t timeline)
{
    for (uint32_t n = 0; n < SCAN_RECORDS && *pos < len && q_n < QUEUE; n++) {
        const uint8_t *r = buf + *pos;
        uint32_t l = r[0];
        *pos += 1 + l;
        if (l < 110 || *pos > len)
            break;
        const uint8_t *e = r + 2;
        uint32_t size = get24(e + 41);
        if (e[40] != ENTRY_DOCUMENT || size > DOC_MAX || size < HEAD)
            continue;
        if (taken(e, get32(e + 32)))
            continue;
        cand *c = &queue[(q_head + q_n++) % QUEUE];
        memcpy(c->key, e, 32);
        memcpy(c->blob, e + 44, 32);
        c->seq = get32(e + 32);
        c->size = size;
        /* (The timeline holds only followed feeds; a find's 16 results
         * are looked up.) */
        const known *k = timeline ? 0 : known_of(e);
        c->followed = timeline || (k && !(k->flags & 0x04));
    }
}

/* The names of the known feeds, a page at a time. */
static void names_step(void)
{
    if (names_done || names_h)
        return;
    names_h = content_names(&names_req, names_next, sizeof names_buf, names_buf);
}

static void names_arrived(void)
{
    if (names_req.status != STATUS_DONE) {
        names_done = 1;
        return;
    }
    uint32_t p = 0, got = 0;
    while (p < names_req.length) {
        const uint8_t *r = names_buf + p;
        uint32_t l = r[0];
        p += 1 + l;
        got++;
        if (l < 33 || nknown == MAX_KNOWN)
            continue;
        known *k = &knowns[nknown++];
        k->flags = r[1];
        memcpy(k->key, r + 2, 32);
        k->len = l - 33 > NAME_MAX ? NAME_MAX : l - 33;
        memcpy(k->name, r + 34, k->len);
    }
    names_next += got;
    if (!got || names_next >= names_req.size)
        names_done = 1;
}

static void timeline_step(void)
{
    if (!names_done || tl_h)
        return;
    if (tl_pos < tl_len) {
        scan(tl_buf, &tl_pos, tl_len, 1);
        return;
    }
    if (tl_count && tl_next < tl_total) {
        tl_h = content_feed(&tl_req, CONTENT_TIMELINE, tl_next, sizeof tl_buf, tl_buf);
        return;
    }
    if (tl_count || !tl_again)
        return;
    /* From the newest again. */
    tl_again = 0;
    tl_next = 0;
    tl_h = content_feed(&tl_req, CONTENT_TIMELINE, 0, sizeof tl_buf, tl_buf);
}

static void timeline_arrived(void)
{
    if (tl_req.status != STATUS_DONE) {
        tl_count = tl_pos = tl_len = 0;
        tl_have = 1;
        return;
    }
    uint32_t p = 0, got = 0;
    while (p < tl_req.length) {
        p += 1 + tl_buf[p];
        got++;
    }
    tl_pos = 0;
    tl_len = tl_req.length;
    tl_total = tl_req.size;
    tl_next += got;
    tl_count = got && tl_next < tl_total; /* more to ask for */
    tl_have = 1;
    dirty = 1;
}

static void find_step(void)
{
    if (!find_on || find_h)
        return;
    if (find_pos < find_len) {
        scan(find_buf, &find_pos, find_len, 0);
        return;
    }
    if (find_n == find_queries) {
        find_on = 0;
        say(find_results ? "Search done: the relays' results are counted under \"all found\"." : "Search done: the relays found no reports.");
        return;
    }
    if (frame_n < find_at)
        return;
    find_h = content_find(&find_req, FIND_ENTRIES, find_q[find_n], find_qlen[find_n], find_buf, sizeof find_buf);
    find_at = frame_n + FIND_GAP;
    find_n++;
    say("Searching the relays\xE2\x80\xA6");
}

static void find_arrived(void)
{
    find_pos = 0;
    find_len = 0;
    if (find_req.status == STATUS_DONE) {
        find_len = find_req.length;
        find_results += find_req.size;
    } else if (find_req.status == STATUS_REFUSED) {
        find_on = 0;
        say("Search refused (busy relays, or not on this host).");
    } else {
        find_on = 0;
        say("Search failed: no relay answered.");
    }
}

static void start_find(void)
{
    if (find_on)
        return;
    static const char base[] = "performance report";
    find_queries = 0;
    memcpy(find_q[0], base, 18);
    find_qlen[find_queries++] = 18;
    for (uint32_t k = 0; k < ngroups && find_queries < 4; k++) {
        const group *g = &groups[order[k]];
        if (g->kind != K_HOST)
            continue;
        /* The name: the label up to its first space (the version
         * follows), printable ASCII only. */
        uint32_t n = 0;
        while (n < g->len && g->label[n] != ' ' && n < 28)
            n++;
        uint8_t ok = n > 0;
        for (uint32_t i = 0; i < n; i++)
            ok &= g->label[i] > 0x20 && g->label[i] < 0x7F;
        for (uint32_t j = 1; ok && j < find_queries; j++)
            ok = !(find_qlen[j] == 20 + n && !memcmp(find_q[j] + 20, g->label, n)); /* asked already */
        if (!ok)
            continue;
        char *q = find_q[find_queries];
        memcpy(q, "performance report: ", 20);
        memcpy(q + 20, g->label, n);
        find_qlen[find_queries++] = 20 + n;
    }
    find_n = 0;
    find_on = 1;
    find_at = frame_n;
    find_pos = find_len = 0;
}

static void gets_step(void)
{
    for (uint32_t i = 0; i < GETS && q_n; i++) {
        if (get_state[i])
            continue;
        cand *c = &queue[q_head];
        uint8_t h = content_get(&get_req[i], c->blob, 0, c->size, get_buf[i]);
        if (!h)
            return;
        get_cand[i] = *c;
        get_h[i] = h;
        get_state[i] = 1;
        q_head = (q_head + 1) % QUEUE;
        q_n--;
    }
}

/* The report being parsed: which get buffer, or -1. */
static int8_t parsing = -1;

static void parse_step(uint32_t budget)
{
    while (budget) {
        if (parsing < 0) {
            for (uint32_t i = 0; i < GETS; i++)
                if (get_state[i] == 2) {
                    uint32_t n = get_req[i].length;
                    const uint8_t *d = get_buf[i];
                    static const uint8_t zero[32];
                    if (get_req[i].status != STATUS_DONE || n < HEAD || memcmp(d, "TDOC", 4) || d[4] || memcmp(d + 5, zero, 32)
                        || d[37] != 7 || memcmp(d + 38, "reports", 7)) {
                        get_state[i] = 0;
                        ignored++;
                        continue;
                    }
                    parsing = i;
                    parse_start(d + HEAD, d + n);
                    budget = budget > 256 ? budget - 256 : 0; /* starting one costs as much as a line */
                    if (n == HEAD || d[n - 1] != '\n') {
                        get_state[i] = 0;
                        parsing = -1;
                        malformed++;
                        dirty = 1;
                        continue;
                    }
                    break;
                }
            if (parsing < 0)
                return;
        }
        /* One line. */
        const uint8_t *s = P.p, *e = s;
        while (e < P.end && *e != '\n')
            e++;
        uint32_t used = e - s + 1;
        uint8_t ok = parse_line(s, e);
        P.p = e + 1;
        budget = used >= budget ? 0 : budget - used;
        uint8_t i = parsing;
        if (!ok || P.p >= P.end) {
            if (ok) {
                static const uint8_t req[] = { T_REPORT, T_PERIOD, T_HOST, T_ENV, T_RUNS, T_END };
                for (uint32_t k = 0; k < sizeof req; k++)
                    ok &= P.seen[req[k]] != 0;
            }
            if (ok)
                commit(&get_cand[i]);
            else
                malformed++;
            get_state[i] = 0;
            parsing = -1;
            dirty = 1;
        }
    }
}

/* --- Main ------------------------------------------------------------------- */

static void publish_step(void)
{
    if (put_h && put_req.status != STATUS_PENDING) {
        put_h = 0;
        if (put_req.status != STATUS_DONE) {
            say("Could not keep the article (refused on this host?).");
            return;
        }
        pub_h = content_publish_kind(&pub_req, put_req.id, title, title_len, ENTRY_ARTICLE);
        say("The host asks you whether to publish the aggregate\xE2\x80\xA6");
    }
    if (pub_h && pub_req.status != STATUS_PENDING) {
        pub_h = 0;
        if (pub_req.status == STATUS_DONE) {
            tn = 0;
            ts("Published the aggregate as article #");
            tu(pub_req.size);
            ts(" of your feed.");
            tb[tn] = 0;
            say(tb);
            tn = 0;
        } else {
            say(pub_req.status == STATUS_REFUSED ? "Not published (refused)." : "Publishing failed.");
        }
    }
}

int main(void)
{
    terse_event ev;
    has_full = terse_has_full_text();
    term_cmd(TERM_CURSOR_OFF);
    term_color(7, 0);
    term_cmd(TERM_CLEAR);
    for (uint32_t i = 0; i < FACT_SLOTS; i++)
        fact_slot[i] = 0;
    for (;;) {
        terse_wait_frame();
        frame_n++;
        while (terse_event_pop(&ev)) {
            if (ev.type == EV_FEEDS_UPDATED) {
                tl_again = 1;
                continue;
            }
            if (ev.type != EV_KEY_DOWN)
                continue;
            if (view == 0) {
                if (ev.a == KEY_DOWN && sel + 1 < ngroups)
                    sel++;
                else if (ev.a == KEY_UP && sel)
                    sel--;
                else if (ev.a == KEY_ENTER && ngroups) {
                    view = 1;
                    dsel_top = 0;
                } else if (ev.a == KEY_F) {
                    start_find();
                } else if (ev.a == KEY_P && !put_h && !pub_h) {
                    if (!ngroups) {
                        say("Nothing to publish yet.");
                    } else {
                        build_article();
                        put_h = content_put(&put_req, art, art_len);
                        say(put_h ? "Keeping the article\xE2\x80\xA6" : "Could not ask the host.");
                    }
                }
                if (sel < top)
                    top = sel;
                if (sel >= top + LIST_ROWS)
                    top = sel - LIST_ROWS + 1;
            } else {
                if (ev.a == KEY_ESCAPE)
                    view = 0;
                else if (ev.a == KEY_DOWN && dsel_top + REP_ROWS < rep_n)
                    dsel_top++;
                else if (ev.a == KEY_UP && dsel_top)
                    dsel_top--;
            }
            dirty = 1;
        }
        /* Completions. */
        if (names_h && names_req.status != STATUS_PENDING) {
            names_h = 0;
            names_arrived();
        }
        if (tl_h && tl_req.status != STATUS_PENDING) {
            tl_h = 0;
            timeline_arrived();
        }
        if (find_h && find_req.status != STATUS_PENDING) {
            find_h = 0;
            find_arrived();
        }
        for (uint32_t i = 0; i < GETS; i++)
            if (get_state[i] == 1 && get_req[i].status != STATUS_PENDING)
                get_state[i] = 2;
        publish_step();
        /* Work. */
        names_step();
        timeline_step();
        find_step();
        gets_step();
        /* A frame that may draw the screen parses less. */
        parse_step(frame_n - drawn_at >= 6 ? PARSE_BYTES / 4 : PARSE_BYTES);
        /* At most two groups' tallies a frame. */
        uint32_t done = 0;
        for (uint32_t g = 0; g < ngroups && done < 2; g++)
            if (tally_dirty[g]) {
                tally_dirty[g] = 0;
                compute(g, 0);
                compute(g, 1);
                done++;
                dirty = 1;
            }
        uint8_t busy = q_n || parsing >= 0 || get_state[0] || get_state[1] || get_state[2] || get_state[3] || done;
        if (dirty && (!busy || frame_n - drawn_at >= 6)) {
            sort_groups();
            if (view && sel >= ngroups)
                view = 0;
            if (view)
                draw_detail();
            else
                draw_list();
            dirty = 0;
            drawn_at = frame_n;
        }
    }
}
