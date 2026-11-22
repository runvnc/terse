/*
 * An invented device, as an experimental driver (spec/experimental.md,
 * DEVICES.md 15): `example.dice`, a dice roller with a memory. One file,
 * C11 and POSIX, no dependencies; the socket transport only.
 *
 *     cc -std=c11 -O2 -Wall -Wextra -o dice dice.c
 *     terse build demo.c --optional 240:0=example.dice
 *     terse-bus demo.terse --driver example.dice=$PWD/dice --frames 120 --dump
 *
 * The device. Its state is a 32-bit generator, the number of rolls so far
 * and the last 16 rolls. Three calls:
 *
 *   function 0  seed(a0 = seed)        sets the generator; returns 0.
 *                                      Deferred: the core answers it.
 *   function 1  roll(a0 = sides)       1 to sides (1-255); 0 for 0 sides,
 *                                      FFFF_FFFEh above 255. Answered.
 *   function 2  history(a0 = buffer,   writes the last a1 rolls (0-16,
 *               a1 = count)            oldest first, 0 for none) and
 *                                      returns how many rolls there were.
 *
 * Determinism (spec/experimental.md §experimental.determinism): nothing
 * here reads a clock or a random source. A program that wants different
 * dice each run seeds the device from the machine's own `random` call,
 * which the host records; seeded the same, the rolls are the same on
 * every run, after a snapshot is restored, and in a replay.
 *
 * Compared with ../driver-example/terminal.c (a numbered device), the bus
 * half differs in four places, each marked EXPERIMENTAL: HELLO carries
 * the device's name both ways and the size of its state, and no slot;
 * each machine's slot for the device is in its OPEN (one process serves
 * every machine of a shell, each calling the device on the slot its own
 * image gives it); every call's charge is declared in the table, an
 * answered one's too; and the state section is restored (RESTORE), so
 * snapshots work.
 *
 * For the core's tests, TERSE_DICE_BAD=<what> makes it break one rule on
 * purpose: `table` (a buffer over 4,096 bytes), `name` (answers as another
 * device), `charge` (an answered call charged other than declared),
 * `state` (a section longer than declared), `inputs` (a charge at BEGIN).
 * TERSE_DICE_SLOW=sleep:<ms>:<from> makes it slow on purpose: it sleeps
 * <ms> in the END of every machine's frame from that machine's frame
 * <from> on (for the host's rule for slow devices, PERFORMANCE.md R15).
 */
#define _POSIX_C_SOURCE 200809L                /* nanosleep */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

#define NAME "example.dice"
#define ERR_BUFFER 0xFFFFFFFEu

/* ======================================================= the device ===== */

/* The state: x, rolls, history. */
enum { HISTORY = 16, SECTION = 4 + 4 + HISTORY };

typedef struct {
    uint32_t x;             /* xorshift32, never 0 */
    uint32_t rolls;         /* rolls so far */
    uint8_t last[HISTORY];  /* roll n at last[n % 16] */
} Dice;

static void reset(Dice *d)
{
    memset(d, 0, sizeof *d);
    d->x = 0x2545F491;
}
static void seed(Dice *d, uint32_t s)
{
    d->x = s ? s : 0x2545F491;
}
static uint32_t roll(Dice *d, uint32_t sides)
{
    if (!sides) return 0;
    d->x ^= d->x << 13; d->x ^= d->x >> 17; d->x ^= d->x << 5;
    uint32_t r = 1 + d->x % sides;
    d->last[d->rolls++ % HISTORY] = r;
    return r;
}
/* The last n rolls, oldest first; 0 where there were fewer. */
static void history(const Dice *d, uint32_t n, uint8_t *out)
{
    for (uint32_t i = 0; i < n; i++) {
        uint32_t k = d->rolls - n + i;  /* which roll, if any */
        out[i] = d->rolls >= n - i ? d->last[k % HISTORY] : 0;
    }
}

/* ========================================================== the bus ===== */

typedef struct { uint8_t *p; size_t n, cap; } Buf;
static void put_bytes(Buf *b, const void *d, size_t n)
{
    if (b->n + n > b->cap) { b->cap = (b->n + n) * 2; if (!(b->p = realloc(b->p, b->cap))) exit(1); }
    if (d && n) memcpy(b->p + b->n, d, n);
    b->n += n;
}
static void u8(Buf *b, uint32_t v) { uint8_t c = v; put_bytes(b, &c, 1); }
static void u16(Buf *b, uint32_t v) { u8(b, v); u8(b, v >> 8); }
static void u32(Buf *b, uint32_t v) { u16(b, v); u16(b, v >> 16); }
static void str(Buf *b, const char *s) { u16(b, strlen(s)); put_bytes(b, s, strlen(s)); }
static uint32_t rd32(const uint8_t *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }

static int sock;
static uint32_t instance;
static uint8_t to_slot;                 /* the address of the message being handled */
static const char *bad = "";            /* TERSE_DICE_BAD */
static unsigned slow_ms, slow_from;     /* TERSE_DICE_SLOW */

static void reply(Buf *b, uint8_t type)
{
    b->n = 0;
    u32(b, 0); u32(b, instance); u8(b, to_slot); u8(b, 0); u16(b, 0); u8(b, type);
}
static void send_reply(Buf *b)
{
    uint32_t n = b->n - 4;
    memcpy(b->p, &(uint8_t[4]){n, n >> 8, n >> 16, n >> 24}, 4);
    for (size_t off = 0; off < b->n;) {
        ssize_t k = write(sock, b->p + off, b->n - off);
        if (k <= 0) exit(0);
        off += k;
    }
}
static int read_full(uint8_t *p, size_t n)
{
    while (n) {
        ssize_t k = read(sock, p, n);
        if (k <= 0) return 0;
        p += k; n -= k;
    }
    return 1;
}
static void fail(const char *why)
{
    Buf b = {0};
    reply(&b, 0x7F); put_bytes(&b, why, strlen(why));
    send_reply(&b);
    fprintf(stderr, "dice: %s\n", why);
    exit(1);
}

/* The call table (DEVICES.md 5): one 64-byte entry per call. EXPERIMENTAL:
   the charge rule (offsets 44-63) counts for every call, the answered ones
   too, within DEVICES.md 15.2's limits; the core charges by it. */
static void entry(Buf *b, uint8_t fn, uint8_t flags, uint8_t len_arg, uint32_t most, uint8_t access,
                  uint8_t checked, uint32_t checked_max, uint8_t rule, uint32_t charge, uint8_t per4)
{
    u8(b, fn); u8(b, flags); u8(b, 0); u8(b, len_arg);       /* buffer at a0, length argument */
    u32(b, 1); u32(b, 0); u32(b, most);                       /* 1 byte per unit, least, greatest */
    u8(b, access); u8(b, checked); u16(b, 0); u32(b, checked_max);
    u32(b, ERR_BUFFER); u32(b, ERR_BUFFER);                   /* results: bad length, bad place */
    u8(b, rule); u8(b, 0); u16(b, 0); u32(b, 0); u32(b, 0);  /* result rule: constant 0 */
    u32(b, charge);                                           /* constant charge */
    u8(b, per4); u8(b, 0xFF); u16(b, 0);                      /* 1 per 4 bytes; no ranged charge */
    u32(b, 0); u32(b, 0); u32(b, 0);
}
enum { DEFER = 2, BUF = 4, WRITES = 2, NONE = 0xFF };
enum { ROLL_CHARGE = 16 };              /* on top of an answered call's 64 */
static void table(Buf *b)
{
    u8(b, 3);
    entry(b, 0, DEFER, NONE, 0, 0, NONE, 0, 1, 0, 0);                  /* seed: 0; deferred, 4 */
    entry(b, 1, 0, NONE, 0, 0, 0, 255, 0, ROLL_CHARGE, 0);             /* roll: sides <= 255; 64 + 16 */
    entry(b, 2, BUF, 1, !strcmp(bad, "table") ? 8192 : HISTORY, WRITES, /* history: a1 bytes at a0, */
          NONE, 0, 0, 0, 1);                                           /* written; 64 + 1 per 4 bytes */
}

/* The machines served, by instance. EXPERIMENTAL: each with the slot its
   image calls the device on (OPEN); two machines may use two slots. */
static struct { uint32_t id; uint8_t slot; Dice d; } *machines;
static size_t nmachines;
static Dice *find(uint32_t id, uint8_t slot)
{
    for (size_t i = 0; i < nmachines; i++)
        if (machines[i].id == id) {
            if (slot && slot != machines[i].slot) fail("a message on a slot this machine does not call the device on");
            return &machines[i].d;
        }
    return 0;
}

/* One call: the core has checked the buffer and the sides (the table) and
   sends the call only if they are good. */
static void call(Dice *d, const uint8_t *m, size_t n, Buf *r)
{
    uint8_t out[HISTORY];
    uint32_t a[6], ret = 0, charge = 0, wlen = 0;
    if (n < 40) fail("a malformed CALL");
    for (int i = 0; i < 6; i++) a[i] = rd32(m + 4 + 4 * i);
    switch (m[1]) {
    case 0: seed(d, a[0]); break;
    case 1: ret = roll(d, a[0]); charge = ROLL_CHARGE + !strcmp(bad, "charge"); break;
    case 2: wlen = a[1]; history(d, wlen, out); ret = d->rolls; charge = (wlen + 3) / 4; break;
    default: fail("a function that is not in the call table");
    }
    reply(r, 0x90);
    u8(r, 0); u16(r, 0); u32(r, ret); u32(r, 0); u32(r, charge);
    u32(r, wlen ? 1 : 0);                                      /* the write: address, length, bytes */
    if (wlen) { u32(r, a[0]); u32(r, wlen); put_bytes(r, out, wlen); }
}

int main(int argc, char **argv)
{
    if (argc > 1 && !strcmp(argv[1], "--version")) { puts("dice 0.1.0 unknown"); return 0; }
    if (getenv("TERSE_DICE_BAD")) bad = getenv("TERSE_DICE_BAD");
    if (getenv("TERSE_DICE_SLOW") && sscanf(getenv("TERSE_DICE_SLOW"), "sleep:%u:%u", &slow_ms, &slow_from) != 2) {
        fputs("dice: TERSE_DICE_SLOW is sleep:<ms>:<from>\n", stderr);
        return 2;
    }
    const char *path = getenv("TERSE_BUS_SOCKET");
    struct sockaddr_un sa = {.sun_family = AF_UNIX};
    if (!path || strlen(path) >= sizeof sa.sun_path) { fputs("start me from a Terse core\n", stderr); return 2; }
    strcpy(sa.sun_path, path);
    sock = socket(AF_UNIX, SOCK_STREAM, 0);
    if (sock < 0 || connect(sock, (struct sockaddr *)&sa, sizeof sa)) { perror("connect"); return 1; }

    Buf in = {0}, r = {0}, scratch = {0};
    for (;;) {
        uint8_t h[4];
        if (!read_full(h, 4)) return 0;
        uint32_t len = rd32(h);
        if (len < 9 || len > 16777216) fail("a malformed message");
        in.n = 0; put_bytes(&in, 0, len);
        if (!read_full(in.p, len)) return 0;
        instance = rd32(in.p);
        to_slot = in.p[4];
        const uint8_t *m = in.p + 8;
        size_t n = len - 8;
        Dice *d = find(instance, to_slot);

        switch (m[0]) {
        case 0x01: { /* HELLO. EXPERIMENTAL: no slots, and after the bus
                        file the device's name: ours, or empty when `terse
                        drivers add` asks which device this is. */
            if (n < 9 || m[1] != 1 || m[8] != 0) fail("this driver serves one experimental device, and no slot");
            size_t at = 9;
            for (int i = 0; i < 2; i++) at += 2 + (at + 1 < n ? (m[at] | m[at + 1] << 8) : 0);
            if (at + 3 > n || m[at] != 1) fail("no experimental device asked for");
            size_t nl = m[at + 1] | m[at + 2] << 8;
            if (at + 3 + nl > n || (nl && (nl != strlen(NAME) || memcmp(m + at + 3, NAME, nl))))
                fail("this driver serves " NAME " only");
            reply(&r, 0x81);
            u8(&r, 1); u8(&r, 0); u8(&r, 0);                   /* protocol 1, the socket */
            str(&r, "dice (example)");
            u16(&r, 0); u16(&r, 0);                             /* draws nothing */
            u8(&r, 1); u8(&r, 0); u8(&r, 0); u8(&r, 0);        /* 1 entry: slot 0, version 0, no layers */
            table(&r);
            u8(&r, 0); u8(&r, 0); u8(&r, 0);                    /* no properties, subscriptions, services */
            str(&r, "0.1.0"); str(&r, "unknown");               /* identity */
            u8(&r, 1);                                          /* EXPERIMENTAL: the device, */
            str(&r, !strcmp(bad, "name") ? "other.dice" : NAME);/* its name */
            u32(&r, SECTION);                                   /* and the most its state can be */
            send_reply(&r);
            break;
        }
        case 0x03: { /* OPEN: a machine starts, its dice at reset. EXPERIMENTAL:
                        its one (slot, version) pair is the slot this
                        machine calls the device on, 240-255. */
            size_t at = 44;
            at += 2 + (at + 1 < n ? (m[at] | m[at + 1] << 8) : 0);   /* the memory file */
            uint8_t s = at + 3 <= n && m[at] == 1 ? m[at + 1] : 0, v = at + 3 <= n ? m[at + 2] : 0;
            if (s < 240) fail("OPEN without the machine's slot for the device");
            size_t i = 0;
            while (i < nmachines && machines[i].id != instance) i++;
            if (i == nmachines) {
                machines = realloc(machines, (nmachines + 1) * sizeof *machines);
                machines[nmachines++].id = instance;
            }
            machines[i].slot = s;
            d = &machines[i].d;
            reset(d);
            reply(&r, 0x83); u8(&r, v == 0); u16(&r, 0); str(&r, v == 0 ? "" : "this driver implements version 0");
            send_reply(&r);
            break;
        }
        case 0x04: /* CLOSE */
            for (size_t i = 0; i < nmachines; i++)
                if (machines[i].id == instance) { machines[i] = machines[--nmachines]; break; }
            break;
        case 0x02: /* RESET */
            if (d) reset(d);
            reply(&r, 0x82); send_reply(&r);
            break;
        case 0x10: /* CALL */
            if (!d) fail("a call for a machine that was not opened");
            call(d, m, n, &r);
            send_reply(&r);
            break;
        case 0x11: { /* BATCH: the deferred calls (seed), in order */
            size_t at = 8;
            for (uint32_t i = 0, count = n >= 8 ? rd32(m + 4) : 0; i < count; i++) {
                uint32_t l = at + 4 <= n ? rd32(m + at) : 0;
                if (l < 9 || at + 4 + l > n) fail("a malformed BATCH");
                instance = rd32(m + at + 4);
                if (!(d = find(instance, m[at + 8]))) fail("a batched call for a machine that was not opened");
                call(d, m + at + 12, l - 8, &scratch);
                at += 4 + l;
            }
            break;
        }
        case 0x30: /* BEGIN. EXPERIMENTAL: no inputs, ever. */
            reply(&r, 0xB0);
            u8(&r, 0); u16(&r, 0);
            u32(&r, !strcmp(bad, "inputs")); u32(&r, 0); u32(&r, 0); u32(&r, 0);
            send_reply(&r);
            break;
        case 0x31: /* END: nothing between frames (but TERSE_DICE_SLOW) */
            if (slow_ms && n >= 8 && rd32(m + 4) >= slow_from) {
                struct timespec ts = {slow_ms / 1000, (long)(slow_ms % 1000) * 1000000};
                while (nanosleep(&ts, &ts)) {}
            }
            break;
        case 0x40: /* STATE: x, rolls, the last 16 rolls (24 bytes, as declared) */
            if (!d) fail("STATE of a machine that was not opened");
            reply(&r, 0xC0);
            u8(&r, 0); u16(&r, 0); u32(&r, SECTION + 4 * !strcmp(bad, "state"));
            u32(&r, d->x); u32(&r, d->rolls); put_bytes(&r, d->last, HISTORY);
            if (!strcmp(bad, "state")) u32(&r, 0);
            send_reply(&r);
            break;
        case 0x41: { /* RESTORE. EXPERIMENTAL: the core gives exactly the
                        section; a valid one has the generator not 0. */
            if (!d) fail("RESTORE of a machine that was not opened");
            uint32_t l = n >= 8 ? rd32(m + 4) : 0;
            int ok = l == SECTION && n >= 8 + SECTION && rd32(m + 8) != 0;
            if (ok) { d->x = rd32(m + 8); d->rolls = rd32(m + 12); memcpy(d->last, m + 16, HISTORY); }
            reply(&r, 0xC1); u8(&r, ok); u16(&r, 0); u32(&r, ok ? SECTION : 0);
            send_reply(&r);
            break;
        }
        case 0x60: /* SHUTDOWN */
            return 0;
        default:
            fail("a message this driver does not handle");
        }
    }
}
