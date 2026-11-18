/* Drives softfloat.c: natively (TERSE_NATIVE) it checks every operation
 * against the host's FPU and prints a checksum; on the machine it prints the
 * same checksum and spends one frame on each phase, so that the frame
 * results give the instructions per operation. */

#include <stdint.h>
#include "softfloat.c"

#define OPS 200

static uint64_t rng = 0x9E3779B97F4A7C15ull;
static uint64_t next(void)
{
    rng ^= rng << 13;
    rng ^= rng >> 7;
    rng ^= rng << 17;
    return rng;
}

/* A random double: mostly ordinary magnitudes, sometimes extreme exponents,
 * subnormals, zeros, infinities and NaNs. */
static f64 operand(void)
{
    uint64_t r = next(), m = r & 0x000FFFFFFFFFFFFFull, s = r >> 63;
    uint32_t k = (uint32_t)(r >> 52) & 63;
    int32_t e;
    if (k < 40)
        e = 1023 - 30 + (int32_t)((uint32_t)next() % 61);
    else if (k < 52)
        e = 1 + (int32_t)((uint32_t)next() % 2046);
    else if (k < 56)
        e = 0;
    else if (k < 58)
        return PACK(s, 0, 0);
    else if (k < 60)
        return PACK(s, 0x7FF, 0);
    else if (k < 61)
        return PACK(s, 0x7FF, m | 1);
    else
        e = (k & 1) ? 2046 - (int32_t)((uint32_t)next() % 4) : 1 + (int32_t)((uint32_t)next() % 4);
    return PACK(s, e, m);
}

static uint64_t sum;
static void mix(f64 v)
{
    if (EXP(v) == 0x7FF && FRAC(v))
        v = NAN64;
    sum = (sum ^ v) * 0x100000001B3ull;
}

#ifdef TERSE_NATIVE
#include <stdio.h>
#include <string.h>

static int same(f64 soft, double hard)
{
    f64 h;
    memcpy(&h, &hard, 8);
    if (EXP(h) == 0x7FF && FRAC(h))
        return EXP(soft) == 0x7FF && FRAC(soft);
    return soft == h;
}

int main(void)
{
    long bad = 0, n;
    for (n = 0; n < 2000000; n++) {
        f64 a = operand(), b = operand(), r[4];
        double x, y, h[4];
        int k;
        memcpy(&x, &a, 8);
        memcpy(&y, &b, 8);
        r[0] = f64_add(a, b), h[0] = x + y;
        r[1] = f64_sub(a, b), h[1] = x - y;
        r[2] = f64_mul(a, b), h[2] = x * y;
        r[3] = f64_div(a, b), h[3] = x / y;
        for (k = 0; k < 4; k++)
            if (!same(r[k], h[k]) && bad++ < 10)
                fprintf(stderr, "op %d: %016llx %016llx -> %016llx, FPU %016llx\n", k, (unsigned long long)a,
                        (unsigned long long)b, (unsigned long long)r[k], *(unsigned long long *)&h[k]);
    }
    printf("mismatches %ld\n", bad);
    rng = 0x9E3779B97F4A7C15ull;
    for (n = 0; n < 4 * OPS; n++) {
        f64 a = operand(), b = operand();
        mix(f64_add(a, b));
        mix(f64_mul(a, b));
        mix(f64_div(a, b));
    }
    printf("checksum %016llx\n", (unsigned long long)sum);
    return 0;
}

#else
#include "terse.h"

static f64 xs[OPS], ys[OPS];
static volatile f64 zs[OPS]; /* volatile: the results are kept */

int main(void)
{
    static const char hex[] = "0123456789abcdef";
    char line[32] = "checksum ";
    int n, k;
    for (n = 0; n < 4 * OPS; n++) {
        f64 a = operand(), b = operand();
        mix(f64_add(a, b));
        mix(f64_mul(a, b));
        mix(f64_div(a, b));
    }
    for (k = 0; k < 16; k++)
        line[9 + k] = hex[sum >> (60 - 4 * k) & 15];
    line[25] = 0;
    term_puts(line);
    /* Ordinary operands (k < 40 in operand()) for the timing phases. */
    for (n = 0; n < OPS; n++) {
        xs[n] = PACK(next() >> 63, 1000 + (uint32_t)next() % 40, next() & 0x000FFFFFFFFFFFFFull);
        ys[n] = PACK(next() >> 63, 1000 + (uint32_t)next() % 40, next() & 0x000FFFFFFFFFFFFFull);
    }
    terse_wait_frame();
    for (n = 0; n < OPS; n++) /* phase 1: the loop alone */
        zs[n] = xs[n] ^ ys[n];
    terse_wait_frame();
    for (n = 0; n < OPS; n++)
        zs[n] = f64_add(xs[n], ys[n]);
    terse_wait_frame();
    for (n = 0; n < OPS; n++)
        zs[n] = f64_mul(xs[n], ys[n]);
    terse_wait_frame();
    for (n = 0; n < OPS; n++)
        zs[n] = f64_div(xs[n], ys[n]);
    terse_wait_frame();
    return 0;
}
#endif
