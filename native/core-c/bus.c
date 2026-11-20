/* The core side of the device bus (DEVICES.md), socket transport only:
   start drivers, HELLO, OPEN, CALL/RESULT (+READ/DATA), BATCH, BEGIN/INPUTS,
   END, STATE, SHUTDOWN. No shared memory, presentation, properties,
   services, recording. */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include "core.h"

typedef struct Entry {
    int valid;
    uint8_t flags, baddr, blen, access, carg, rule, rarg, cflags, charg;
    uint32_t scale, least, greatest, cmax, rlen, rplace, val, val2, cconst, clow, chigh, cadd;
    uint16_t cperbyte;
} Entry;

typedef struct { uint8_t *p; size_t n, cap; } Buf;

typedef struct {
    char *cmd;
    uint8_t want[8]; int nwant;      /* slots of this spec */
    pid_t pid; int fd;
    char name[64], sock[256];
    int nslots; uint8_t slots[16], vers[16];
    Buf batch; uint32_t nbatch;
    Buf in;                          /* last message read */
} Driver;

#define MAXD 16
static Driver drv[MAXD];
static int ndrv;
static Driver *drv_of[256];
static Entry *table[256];
static char session[200];
int bus_nobatch;
const char *bus_driver_dir;
int bus_withheld[256];

/* ---- byte buffers ---- */
static void bput(Buf *b, const void *d, size_t n)
{
    if (b->n + n > b->cap) { b->cap = (b->n + n) * 2 + 64; b->p = realloc(b->p, b->cap); }
    if (n) memcpy(b->p + b->n, d, n);
    b->n += n;
}
static void b8(Buf *b, uint32_t v) { uint8_t c = v; bput(b, &c, 1); }
static void b16(Buf *b, uint32_t v) { uint8_t c[2]; wr16(c, v); bput(b, c, 2); }
static void b32(Buf *b, uint32_t v) { uint8_t c[4]; wr32(c, v); bput(b, c, 4); }
static void bstr(Buf *b, const char *s) { b16(b, strlen(s)); bput(b, s, strlen(s)); }
/* A message: length, instance, slot, 3 reserved, then the body. */
static void msg(Buf *b, uint32_t inst, int slot, int type)
{
    b->n = 0; b32(b, 0); b32(b, inst); b8(b, slot); b8(b, 0); b16(b, 0); b8(b, type);
}
static unsigned long sent[256], received[256];   /* by type, for --bench */
static void msg_end(Buf *b) { wr32(b->p, b->n - 4); sent[b->p[12]]++; }

void bus_stats(void)
{
    static const char *name[256] = {[1] = "HELLO", [3] = "OPEN", [0x10] = "CALL", [0x11] = "BATCH",
        [0x20] = "READ", [0x30] = "BEGIN", [0x31] = "END", [0x40] = "STATE", [0x60] = "SHUTDOWN",
        [0x81] = "HELLO", [0x83] = "OPEN", [0x90] = "RESULT", [0xA0] = "DATA", [0xB0] = "INPUTS", [0xC0] = "STATE"};
    fputs("tcore: messages sent:", stderr);
    for (int i = 0; i < 256; i++) if (sent[i]) fprintf(stderr, " %s %lu", name[i] ? name[i] : "?", sent[i]);
    fputs("; received:", stderr);
    for (int i = 0; i < 256; i++) if (received[i]) fprintf(stderr, " %s %lu", name[i] ? name[i] : "?", received[i]);
    fputc('\n', stderr);
}

/* ---- failure and cleanup ---- */
void bus_shutdown(void)
{
    Buf b = {0};
    for (int i = 0; i < ndrv; i++) {
        Driver *d = &drv[i];
        if (d->fd >= 0) {
            msg(&b, 0, 0, 0x60); msg_end(&b);
            if (write(d->fd, b.p, b.n) < 0) { /* gone already */ }
            close(d->fd); d->fd = -1;
        }
        if (d->pid > 0) {
            int st;
            for (int t = 0; t < 200 && waitpid(d->pid, &st, WNOHANG) == 0; t++) usleep(10000);
            if (waitpid(d->pid, &st, WNOHANG) == 0) { kill(d->pid, SIGKILL); waitpid(d->pid, &st, 0); }
            d->pid = 0;
        }
        if (d->sock[0]) { unlink(d->sock); d->sock[0] = 0; }
    }
    free(b.p);
    if (session[0]) { rmdir(session); session[0] = 0; }
}

void die(int status, const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    fputs("tcore: ", stderr); vfprintf(stderr, fmt, ap); fputc('\n', stderr);
    va_end(ap);
    bus_shutdown();
    exit(status);
}

/* ---- socket I/O ---- */
static void send_all(Driver *d, const Buf *b)
{
    size_t off = 0;
    while (off < b->n) {
        ssize_t k = write(d->fd, b->p + off, b->n - off);
        if (k < 0 && errno == EINTR) continue;
        if (k <= 0) die(3, "driver %s: write failed (it has gone)", d->name);
        off += k;
    }
}
static void recv_all(Driver *d, uint8_t *p, size_t n, int timeout_ms)
{
    while (n) {
        struct pollfd pf = {d->fd, POLLIN, 0};
        int r = poll(&pf, 1, timeout_ms);
        if (r < 0 && errno == EINTR) continue;
        if (r == 0) die(3, "driver %s: no answer within %d s", d->name, timeout_ms / 1000);
        ssize_t k = read(d->fd, p, n);
        if (k < 0 && errno == EINTR) continue;
        if (k <= 0) die(3, "driver %s: closed its socket", d->name);
        p += k; n -= k;
    }
}
/* Reads one message into d->in; returns the body (after the address). */
static uint8_t *recv_msg(Driver *d, uint32_t *len, int timeout_ms)
{
    uint8_t h[4];
    recv_all(d, h, 4, timeout_ms);
    uint32_t n = rd32(h);
    if (n < 9 || n > 16777216) die(3, "driver %s: bad message length %u", d->name, n);
    if (d->in.cap < n) { d->in.cap = n; d->in.p = realloc(d->in.p, n); }
    recv_all(d, d->in.p, n, timeout_ms);
    d->in.n = n;
    *len = n - 8;
    received[d->in.p[8]]++;
    return d->in.p + 8;
}

static M *cur;  /* the machine, for READ */

static void flush_batch(Driver *d)
{
    if (!d->nbatch) return;
    Buf b = {0};
    msg(&b, 0, 0, 0x11); b8(&b, 0); b16(&b, 0); b32(&b, d->nbatch);
    bput(&b, d->batch.p, d->batch.n);
    msg_end(&b);
    send_all(d, &b);
    free(b.p);
    d->batch.n = 0; d->nbatch = 0;
}

/* Sends a request and waits for its reply of type `want`, answering READs. */
static uint8_t *request(Driver *d, Buf *b, int want, uint32_t *len, int timeout_ms)
{
    flush_batch(d);
    msg_end(b);
    send_all(d, b);
    for (;;) {
        uint8_t *r = recv_msg(d, len, timeout_ms);
        if (r[0] == want) return r;
        if (r[0] == 0x7F) die(3, "driver %s: ERROR %.*s", d->name, (int)*len - 1, r + 1);
        if (r[0] != 0x20 || *len < 12) die(3, "driver %s: unexpected message %02xh", d->name, r[0]);
        uint32_t a = rd32(r + 4), n = rd32(r + 8);
        Buf o = {0};
        msg(&o, rd32(d->in.p), d->in.p[4], 0xA0);
        const uint8_t *src = 0;
        if (in_ram(cur, a, n)) src = cur->ram + (a - RAM_BASE);
        else if (in_rom(cur, a, n)) src = cur->rom + (a - ROM_BASE);
        b8(&o, src != 0); b16(&o, 0);
        if (src) bput(&o, src, n);
        msg_end(&o);
        send_all(d, &o);
        free(o.p);
    }
}

/* ---- driver specs and start ---- */
static struct { uint8_t slots[8]; int n; char *cmd; } spec[MAXD] = {
    {{2, 10}, 2, "terse-dev-terminal"}, {{3, 9}, 2, "terse-dev-vector"},
    {{4}, 1, "terse-dev-content"},      {{6}, 1, "terse-dev-sound"},
    {{7}, 1, "terse-dev-page"},
};
static int nspec = 5;

void bus_set_driver(const char *arg)
{
    const char *eq = strchr(arg, '=');
    if (!eq) die(2, "--driver wants <slots>=<command line>");
    uint8_t s[8]; int n = 0;
    for (const char *p = arg; p < eq && n < 8; p++) {
        s[n++] = strtoul(p, (char **)&p, 10);
        if (*p != ',') break;
    }
    for (int i = 0; i < nspec; i++)       /* those slots leave their old driver */
        for (int j = 0; j < spec[i].n; j++)
            for (int k = 0; k < n; k++)
                if (spec[i].slots[j] == s[k]) { spec[i].slots[j--] = spec[i].slots[--spec[i].n]; break; }
    if (n == 1 && s[0] == 2) s[n++] = 10;   /* full text goes with the terminal */
    memcpy(spec[nspec].slots, s, n); spec[nspec].n = n; spec[nspec].cmd = strdup(eq + 1);
    nspec++;
}

/* Splits at spaces; resolves the program against --drivers; NULL if not installed. */
static char **split(const char *cmd)
{
    char *c = strdup(cmd), **v = calloc(64, sizeof *v);
    int n = 0;
    for (char *t = strtok(c, " "); t && n < 63; t = strtok(0, " ")) v[n++] = t;
    if (!n) return 0;
    if (!strchr(v[0], '/') && bus_driver_dir) {
        char *p = malloc(strlen(bus_driver_dir) + strlen(v[0]) + 2);
        sprintf(p, "%s/%s", bus_driver_dir, v[0]);
        v[0] = p;
    }
    if (strchr(v[0], '/')) return access(v[0], X_OK) ? 0 : v;
    const char *path = getenv("PATH");
    char buf[4096];
    for (const char *p = path ? path : ""; *p;) {
        const char *e = strchr(p, ':');
        size_t l = e ? (size_t)(e - p) : strlen(p);
        snprintf(buf, sizeof buf, "%.*s/%s", (int)l, p, v[0]);
        if (!access(buf, X_OK)) return v;
        p += l + (e != 0);
    }
    return 0;
}

static void parse_entry(Entry *e, const uint8_t *p)
{
    e->valid = 1;
    e->flags = p[1]; e->baddr = p[2]; e->blen = p[3];
    e->scale = rd32(p + 4); e->least = rd32(p + 8); e->greatest = rd32(p + 12);
    e->access = p[16]; e->carg = p[17]; e->cmax = rd32(p + 20);
    e->rlen = rd32(p + 24); e->rplace = rd32(p + 28);
    e->rule = p[32]; e->rarg = p[33]; e->val = rd32(p + 36); e->val2 = rd32(p + 40);
    e->cconst = rd32(p + 44); e->cflags = p[48]; e->charg = p[49]; e->cperbyte = rd16(p + 50);
    e->clow = rd32(p + 52); e->chigh = rd32(p + 56); e->cadd = rd32(p + 60);
}

static void start(Driver *d, char **argv, const uint8_t *ask, int nask)
{
    static int serial;
    snprintf(d->sock, sizeof d->sock, "%s/d%d.sock", session, serial++);
    int ls = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    struct sockaddr_un sa = {.sun_family = AF_UNIX};
    snprintf(sa.sun_path, sizeof sa.sun_path, "%s", d->sock);
    if (ls < 0 || bind(ls, (struct sockaddr *)&sa, sizeof sa) || listen(ls, 1))
        die(2, "cannot listen on %s: %s", d->sock, strerror(errno));
    fflush(0);
    d->pid = fork();
    if (d->pid == 0) {
        setenv("TERSE_BUS_SOCKET", d->sock, 1);
        int nul = open("/dev/null", O_RDONLY);
        dup2(nul, 0); dup2(2, 1);
        execv(argv[0], argv);
        execvp(argv[0], argv);
        _exit(127);
    }
    /* accept within 10 s, unless the driver exits first */
    for (int t = 0;; t++) {
        struct pollfd pf = {ls, POLLIN, 0};
        if (poll(&pf, 1, 100) > 0) break;
        int st;
        if (waitpid(d->pid, &st, WNOHANG) == d->pid) { d->pid = 0; die(2, "driver %s exited before it connected", argv[0]); }
        if (t >= 100) die(2, "driver %s did not connect within 10 s", argv[0]);
    }
    d->fd = accept4(ls, 0, 0, SOCK_CLOEXEC);
    close(ls);
    if (d->fd < 0) die(2, "accept: %s", strerror(errno));

    Buf b = {0};
    msg(&b, 0, 0, 0x01); b8(&b, 1); b16(&b, 0); b32(&b, 0); /* no shm, no presenting, no replay */
    b8(&b, nask); bput(&b, ask, nask);
    bstr(&b, session); bstr(&b, "");
    uint32_t len;
    uint8_t *r = request(d, &b, 0x81, &len, 10000), *e = r + len;
    free(b.p);
#define NEED(k) do { if (r + (k) > e) die(3, "driver %s: short HELLO reply", argv[0]); } while (0)
    if (r[1] != 1 || r[2] != 0) die(3, "driver %s: protocol %d, transport %d", argv[0], r[1], r[2]);
    r += 4; NEED(2);
    uint32_t nl = rd16(r); NEED(2 + nl);
    snprintf(d->name, sizeof d->name, "%.*s", (int)nl, r + 2);
    r += 2 + nl + 4; NEED(1);
    int s = *r++;
    for (int i = 0; i < s; i++) {
        NEED(3);
        int slot = r[0], ver = r[1], k = r[2], ok = 0;
        r += 3 + 2 * k; NEED(1);
        int nm = *r++; NEED(64 * nm);
        for (int j = 0; j < nask; j++) ok |= ask[j] == slot;
        if (!ok) die(3, "driver %s answers for slot %d, not asked for", d->name, slot);
        d->slots[d->nslots] = slot; d->vers[d->nslots++] = ver;
        drv_of[slot] = d;
        table[slot] = calloc(256, sizeof(Entry));
        for (int j = 0; j < nm; j++, r += 64) parse_entry(&table[slot][r[0]], r);
    }
    /* properties and services follow; a headless core uses none of them */
}

static int lowest(const Driver *d)
{
    int s = 256;
    for (int j = 0; j < d->nslots; j++) if (d->slots[j] < s) s = d->slots[j];
    return s;
}

int bus_setup(M *m, char *err, size_t el)
{
    cur = m;
    int listed[256] = {0}, opt[256] = {0};
    for (int i = 0; i < m->ndev; i++) {
        listed[m->dslot[i]] = 1; opt[m->dslot[i]] = m->dopt[i];
    }
    const char *tmp = getenv("TMPDIR");
    snprintf(session, sizeof session, "%s/tcore-XXXXXX", tmp && *tmp ? tmp : "/tmp");
    if (!mkdtemp(session)) { snprintf(err, el, "mkdtemp: %s", strerror(errno)); session[0] = 0; return 0; }
    for (int i = 0; i < nspec; i++) {
        uint8_t ask[8]; int nask = 0, required = 0;
        for (int j = 0; j < spec[i].n; j++) {
            int s = spec[i].slots[j];
            if (!listed[s] || (opt[s] && bus_withheld[s])) continue;
            if (s == 10) continue;
            ask[nask++] = s;
            required |= !opt[s];
        }
        if (!nask) continue;
        /* Device 10 goes with the terminal, and device 9 needs its driver to
           serve device 3 as well, whose state section holds 9's style
           (DEVICES.md 1.1; the driver says so if it is not asked). */
        int has3 = 0, has9 = 0;
        for (int j = 0; j < nask; j++) { has3 |= ask[j] == 3; has9 |= ask[j] == 9; }
        for (int j = 0; j < spec[i].n; j++) {
            if (spec[i].slots[j] == 10 && listed[10] && !(opt[10] && bus_withheld[10])) ask[nask++] = 10;
            if (spec[i].slots[j] == 3 && has9 && !has3) ask[nask++] = 3;
        }
        char **argv = split(spec[i].cmd);
        if (!argv) {
            if (required) { snprintf(err, el, "needs a device: no driver %s installed", spec[i].cmd); return 0; }
            continue;     /* only optional devices: not granted */
        }
        Driver *d = &drv[ndrv++];
        d->fd = -1;
        start(d, argv, ask, nask);
    }
    /* Drivers in order of their lowest slot (DEVICES.md 1.5) */
    for (int i = 1; i < ndrv; i++)
        for (int j = i; j > 0 && lowest(&drv[j]) < lowest(&drv[j - 1]); j--) {
            Driver t = drv[j]; drv[j] = drv[j - 1]; drv[j - 1] = t;
        }
    for (int i = 0; i < ndrv; i++)
        for (int j = 0; j < drv[i].nslots; j++) drv_of[drv[i].slots[j]] = &drv[i];
    /* Grants, SPEC §image: what the drivers serve, in the version listed. */
    for (int i = 0; i < m->ndev; i++) {
        int s = m->dslot[i], ok = 0;
        Driver *d = drv_of[s];
        if (s == 1) continue;
        if (s == 8) continue;          /* the shell's; absent on a machine run alone */
        if (d && !(m->dopt[i] && bus_withheld[s]))
            for (int j = 0; j < d->nslots; j++) ok |= d->slots[j] == s && d->vers[j] == m->dver[i];
        if (s == 10 && !m->has[2] && !(listed[2] && drv_of[2])) ok = 0;
        if (!ok && !m->dopt[i]) { snprintf(err, el, "needs a device: slot %d version %d", s, m->dver[i]); return 0; }
        m->granted[i] = ok;
        if (ok) { m->has[s] = 1; m->ver[s] = m->dver[i]; }
    }
    /* OPEN instance 0 in every driver */
    for (int i = 0; i < ndrv; i++) {
        Driver *d = &drv[i];
        Buf b = {0};
        msg(&b, 0, 0, 0x03); b8(&b, 0); b16(&b, 0);
        bput(&b, m->id, 32); b32(&b, m->rom_len); b32(&b, m->ram_len);
        bstr(&b, "");
        int k = 0;
        for (int j = 0; j < d->nslots; j++) k += m->has[d->slots[j]];
        b8(&b, k);
        for (int j = 0; j < d->nslots; j++)
            if (m->has[d->slots[j]]) { b8(&b, d->slots[j]); b8(&b, m->ver[d->slots[j]]); }
        b8(&b, 0);
        bput(&b, m->rom, m->rom_len);
        uint32_t len;
        uint8_t *r = request(d, &b, 0x83, &len, 30000);
        free(b.p);
        if (len < 2 || r[1] != 1) {
            int rn = len >= 6 ? (int)rd16(r + 4) : 0;
            snprintf(err, el, "needs a device: driver %s will not take the machine: %.*s", d->name,
                     6 + rn <= (int)len ? rn : 0, (const char *)r + 6);
            return 0;
        }
    }
    return 1;
}

/* ---- calls, DEVICES.md 5 and 6 ---- */
static const Entry *bus_entry(int slot, int fn) { return table[slot] && table[slot][fn].valid ? &table[slot][fn] : 0; }

uint32_t bus_call(M *m, int slot, int fn, int64_t *charge)
{
    const Entry *e = bus_entry(slot, fn);
    uint32_t *a = m->x + 10, addr = 0, r = 0;
    uint64_t len = 0;
    *charge = 4;
    if (!e) return 0xFFFFFFFF;
    int deferred = e->flags & 2;
    *charge = deferred ? 4 : 64;
    if (e->flags & 4) {
        addr = a[e->baddr];
        len = e->blen == 0xFF ? e->scale : (uint64_t)a[e->blen] * e->scale;
        if (len < e->least || len > e->greatest) return e->rlen;
        if (len && !in_ram(m, addr, len) && !(!(e->access & 2) && (e->access & 4) && in_rom(m, addr, len)))
            return e->rplace;
    }
    if (e->carg != 0xFF && a[e->carg] > e->cmax) return e->rplace;

    Driver *d = drv_of[slot];
    Buf b = {0};
    msg(&b, 0, slot, 0x10); b8(&b, fn); b16(&b, 0);
    for (int i = 0; i < 6; i++) b32(&b, a[i]);
    int64_t left = m->budget - 4;
    b32(&b, (uint32_t)left); b32(&b, (uint32_t)((uint64_t)left >> 32));
    uint32_t n = (e->flags & 4) && (e->access & 1) ? (uint32_t)len : 0;
    b32(&b, n);
    if (n) bput(&b, in_ram(m, addr, n) ? m->ram + (addr - RAM_BASE) : m->rom + (addr - ROM_BASE), n);

    int64_t mine = 0;
    if (deferred) {
        if (e->rule == 1) { r = e->val; if (e->flags & 1) a[1] = e->val2; }
        else if (e->rule == 2) r = a[e->rarg];
        else if (e->rule == 3) r = a[e->rarg] <= e->val ? 0 : 1;
        mine = e->cconst + ((e->cflags & 1) ? (len + 3) / 4 : 0) + (int64_t)e->cperbyte * len;
        if (e->charg != 0xFF && a[e->charg] >= e->clow && a[e->charg] <= e->chigh) mine += e->cadd;
        *charge += mine;
        if (!bus_nobatch) {
            msg_end(&b);
            bput(&d->batch, b.p, 4); bput(&d->batch, b.p + 4, b.n - 4);
            d->nbatch++;
            free(b.p);
            return r;
        }
    }
    uint32_t rl;
    uint8_t *p = request(d, &b, 0x90, &rl, 30000), *end = p + rl;
    free(b.p);
    if (rl < 20) die(3, "driver %s: short RESULT", d->name);
    uint32_t r0 = rd32(p + 4), r1 = rd32(p + 8), c = rd32(p + 12), w = rd32(p + 16), fl = p[1];
    if (deferred) {   /* --no-batch: check the driver agrees with the table */
        if (r0 != r || c != mine || w) die(3, "driver %s: call %d/%d gave %u (+%u), table says %u (+%lld)",
                                          d->name, slot, fn, r0, c, r, (long long)mine);
        return r;
    }
    p += 20;
    for (uint32_t i = 0; i < w; i++) {
        if (p + 8 > end) die(3, "driver %s: short RESULT", d->name);
        uint32_t wa = rd32(p), wn = rd32(p + 4);
        p += 8;
        if (p + wn > end || !(e->access & 2) || wa < addr || (uint64_t)wa + wn > addr + len || !in_ram(m, wa, wn))
            die(3, "driver %s: write outside its buffer", d->name);
        memcpy(m->ram + (wa - RAM_BASE), p, wn);
        p += wn;
    }
    if (fl & 1) a[1] = r1;
    *charge += c;
    return r0;
}

/* ---- frames, DEVICES.md 1.5 and 1.6 ---- */
typedef struct { uint8_t ev[4]; int pos, flags, index, drv; } Ev;

static void queue_list(M *m, Ev *l, int n)
{
    for (int i = 0; i < n; i++) {
        if (l[i].flags & 1) {                       /* a pair: both or neither */
            if (m->qn + 2 <= QMAX) { queue_event(m, l[i].ev); if (i + 1 < n) queue_event(m, l[i + 1].ev); }
            i++;
            continue;
        }
        if (l[i].ev[0] == 7) { if (m->feeds_this_frame) continue; m->feeds_this_frame = 1; }
        queue_event(m, l[i].ev);
    }
}

void bus_begin(M *m, const uint8_t (*host)[4], int nhost, int64_t *charge)
{
    static Ev *dev; static size_t cap;
    size_t nd = 0;
    Buf b = {0};
    *charge = 0;
    m->feeds_this_frame = 0;
    /* writes are applied after the events are queued: keep them */
    Buf writes = {0};
    for (int i = 0; i < ndrv; i++) {
        Driver *d = &drv[i];
        msg(&b, 0, 0, 0x30); b8(&b, 0); b16(&b, 0);
        b32(&b, m->frame); b32(&b, m->time); b32(&b, nhost);
        for (int j = 0; j < nhost; j++) bput(&b, host[j], 4);
        b32(&b, 0); /* the replay field, written empty: the driver wants it (SPEC-FINDINGS.md) */
        uint32_t len;
        uint8_t *r = request(d, &b, 0xB0, &len, 30000), *e = r + len;
        if (len < 12) die(3, "driver %s: short INPUTS", d->name);
        *charge += rd32(r + 4);
        uint32_t ne = rd32(r + 8);
        r += 12;
        if (r + 8 * (uint64_t)ne + 4 > e) die(3, "driver %s: short INPUTS", d->name);
        for (uint32_t j = 0; j < ne; j++, r += 8) {
            if (nd == cap) { cap = cap * 2 + 64; dev = realloc(dev, cap * sizeof *dev); }
            Ev *v = &dev[nd++];
            memcpy(v->ev, r, 4); v->pos = r[4]; v->flags = r[5]; v->index = rd16(r + 6); v->drv = i;
        }
        uint32_t nw = rd32(r); r += 4;
        for (uint32_t j = 0; j < nw; j++) {
            if (r + 8 > e || r + 8 + rd32(r + 4) > e) die(3, "driver %s: short INPUTS", d->name);
            uint32_t wa = rd32(r), wn = rd32(r + 4);
            if (!in_ram(m, wa, wn)) die(3, "driver %s: INPUTS writes outside RAM", d->name);
            bput(&writes, r, 8 + wn);
            r += 8 + wn;
        }
        /* the input record (for recordings) is ignored */
    }
    free(b.p);
    /* Order: position 0, host events each followed by its position-1
       events, position 2; drivers already in order of their lowest slot. */
    size_t nl = 0, lcap = nd + nhost + 1;
    Ev *l = malloc(lcap * sizeof *l);
    for (size_t j = 0; j < nd; j++) if (dev[j].pos == 0) l[nl++] = dev[j];
    for (int h = 0; h < nhost; h++) {
        memcpy(l[nl].ev, host[h], 4); l[nl++].flags = 0;
        for (size_t j = 0; j < nd; j++) if (dev[j].pos == 1 && dev[j].index == h) l[nl++] = dev[j];
    }
    for (size_t j = 0; j < nd; j++) if (dev[j].pos == 2) l[nl++] = dev[j];
    queue_list(m, l, nl);
    for (size_t o = 0; o < writes.n;) {
        uint32_t wa = rd32(writes.p + o), wn = rd32(writes.p + o + 4);
        memcpy(m->ram + (wa - RAM_BASE), writes.p + o + 8, wn);
        o += 8 + wn;
    }
    nl = 0;
    for (size_t j = 0; j < nd; j++) if (dev[j].pos == 3) l[nl++] = dev[j];
    queue_list(m, l, nl);
    free(l); free(writes.p);
}

void bus_end(M *m)
{
    Buf b = {0};
    for (int i = 0; i < ndrv; i++) {
        flush_batch(&drv[i]);
        msg(&b, 0, 0, 0x31); b8(&b, m->stopped != RUNNING); b16(&b, 0); b32(&b, m->frame);
        msg_end(&b);
        send_all(&drv[i], &b);
    }
    free(b.p);
}

int bus_state(M *m, int slot, uint8_t **out, uint32_t *n)
{
    (void)m;
    Driver *d = drv_of[slot] ? drv_of[slot] : slot == 3 ? drv_of[9] : 0;
    if (!d) return 0;
    Buf b = {0};
    msg(&b, 0, slot, 0x40);
    uint32_t len;
    uint8_t *r = request(d, &b, 0xC0, &len, 30000);
    free(b.p);
    if (len < 8 || rd32(r + 4) != len - 8) die(3, "driver %s: bad STATE reply", d->name);
    *n = len - 8;
    *out = malloc(*n + 1);
    memcpy(*out, r + 8, *n);
    return 1;
}
