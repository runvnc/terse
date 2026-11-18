/* softfloat.c: IEEE 754 double add, subtract, multiply and divide in
 * integer C (round to nearest, ties to even; subnormals; infinities; NaN
 * results are the default NaN 7FF8...h). Structure after Berkeley SoftFloat
 * 3; division is plain restoring division, one quotient bit a step. For
 * measuring what doubles cost on the machine, which has no FPU and no
 * compiler-rt. crates/terse-core/tests/softfloat.rs checks it against the
 * host's FPU (natively) and runs it on the machine. */

#include <stdint.h>

typedef uint64_t f64;

#define SIGN(a) ((uint32_t)((a) >> 63))
#define EXP(a) ((int32_t)((a) >> 52) & 0x7FF)
#define FRAC(a) ((a) & 0x000FFFFFFFFFFFFFull)
#define PACK(s, e, m) (((uint64_t)(s) << 63) + ((uint64_t)(e) << 52) + (m))
#define NAN64 0x7FF8000000000000ull

static int clz64(uint64_t v)
{
    int n = 0;
    if (!(v >> 32)) {
        n += 32;
        v <<= 32;
    }
    if (!(v >> 48)) {
        n += 16;
        v <<= 16;
    }
    if (!(v >> 56)) {
        n += 8;
        v <<= 8;
    }
    if (!(v >> 60)) {
        n += 4;
        v <<= 4;
    }
    if (!(v >> 62)) {
        n += 2;
        v <<= 2;
    }
    if (!(v >> 63))
        n += 1;
    return n;
}

/* Shifts right by n, ORing any bits shifted out into bit 0. */
static uint64_t jam(uint64_t v, uint32_t n)
{
    return n < 63 ? v >> n | ((v << (-n & 63)) != 0) : (v != 0);
}

/* sig has its leading 1 at bit 62 (or is smaller for subnormals); e is the
 * biased exponent minus 1. */
static f64 round_pack(uint32_t s, int32_t e, uint64_t sig)
{
    uint32_t rb = sig & 0x3FF;
    if ((uint32_t)e >= 0x7FD) {
        if (e < 0) {
            sig = jam(sig, -e);
            e = 0;
            rb = sig & 0x3FF;
        } else if (e > 0x7FD || (int64_t)(sig + 0x200) < 0) {
            return PACK(s, 0x7FF, 0);
        }
    }
    sig = (sig + 0x200) >> 10;
    sig &= ~(uint64_t)(rb == 0x200);
    if (!sig)
        e = 0;
    return PACK(s, e, sig);
}

static f64 norm_round_pack(uint32_t s, int32_t e, uint64_t sig)
{
    int sh = clz64(sig) - 1;
    e -= sh;
    if (sh >= 10 && (uint32_t)e < 0x7FD)
        return PACK(s, sig ? e : 0, sig << (sh - 10));
    return round_pack(s, e, sig << sh);
}

static f64 add_mags(f64 a, f64 b, uint32_t s)
{
    int32_t ea = EXP(a), eb = EXP(b), d = ea - eb, ez;
    uint64_t ma = FRAC(a), mb = FRAC(b), mz;
    if (!d) {
        if (!ea)
            return a + mb;
        if (ea == 0x7FF)
            return ma | mb ? NAN64 : a;
        ez = ea;
        mz = (0x0020000000000000ull + ma + mb) << 9;
    } else {
        ma <<= 9;
        mb <<= 9;
        if (d < 0) {
            if (eb == 0x7FF)
                return mb ? NAN64 : PACK(s, 0x7FF, 0);
            ez = eb;
            ma = ea ? ma + 0x2000000000000000ull : ma << 1;
            ma = jam(ma, -d);
        } else {
            if (ea == 0x7FF)
                return ma ? NAN64 : a;
            ez = ea;
            mb = eb ? mb + 0x2000000000000000ull : mb << 1;
            mb = jam(mb, d);
        }
        mz = 0x2000000000000000ull + ma + mb;
        if (mz < 0x4000000000000000ull) {
            ez--;
            mz <<= 1;
        }
    }
    return round_pack(s, ez, mz);
}

static f64 sub_mags(f64 a, f64 b, uint32_t s)
{
    int32_t ea = EXP(a), eb = EXP(b), d = ea - eb, ez;
    uint64_t ma = FRAC(a), mb = FRAC(b), mz;
    if (!d) {
        int64_t diff;
        int sh;
        if (ea == 0x7FF)
            return NAN64;
        diff = (int64_t)(ma - mb);
        if (!diff)
            return 0;
        if (ea)
            ea--;
        if (diff < 0) {
            s ^= 1;
            diff = -diff;
        }
        sh = clz64((uint64_t)diff) - 11;
        ez = ea - sh;
        if (ez < 0) {
            sh = ea;
            ez = 0;
        }
        return PACK(s, ez, (uint64_t)diff << sh);
    }
    ma <<= 10;
    mb <<= 10;
    if (d < 0) {
        s ^= 1;
        if (eb == 0x7FF)
            return mb ? NAN64 : PACK(s, 0x7FF, 0);
        ma += ea ? 0x4000000000000000ull : ma;
        ma = jam(ma, -d);
        mb |= 0x4000000000000000ull;
        ez = eb;
        mz = mb - ma;
    } else {
        if (ea == 0x7FF)
            return ma ? NAN64 : a;
        mb += eb ? 0x4000000000000000ull : mb;
        mb = jam(mb, d);
        ma |= 0x4000000000000000ull;
        ez = ea;
        mz = ma - mb;
    }
    return norm_round_pack(s, ez - 1, mz);
}

f64 f64_add(f64 a, f64 b)
{
    return SIGN(a) == SIGN(b) ? add_mags(a, b, SIGN(a)) : sub_mags(a, b, SIGN(a));
}

f64 f64_sub(f64 a, f64 b)
{
    return SIGN(a) == SIGN(b) ? sub_mags(a, b, SIGN(a)) : add_mags(a, b, SIGN(a));
}

/* Makes a subnormal significand normal: leading 1 at bit 52. */
static void normalize(int32_t *e, uint64_t *m)
{
    int sh = clz64(*m) - 11;
    *e = 1 - sh;
    *m <<= sh;
}

static uint64_t mul_hi(uint64_t a, uint64_t b, uint64_t *lo)
{
    uint64_t a0 = (uint32_t)a, a1 = a >> 32, b0 = (uint32_t)b, b1 = b >> 32;
    uint64_t p00 = a0 * b0, p01 = a0 * b1, p10 = a1 * b0, p11 = a1 * b1;
    uint64_t mid = (p00 >> 32) + (uint32_t)p01 + (uint32_t)p10;
    *lo = (mid << 32) | (uint32_t)p00;
    return p11 + (p01 >> 32) + (p10 >> 32) + (mid >> 32);
}

f64 f64_mul(f64 a, f64 b)
{
    uint32_t s = SIGN(a) ^ SIGN(b);
    int32_t ea = EXP(a), eb = EXP(b), ez;
    uint64_t ma = FRAC(a), mb = FRAC(b), hi, lo;
    if (ea == 0x7FF || eb == 0x7FF) {
        if ((ea == 0x7FF && ma) || (eb == 0x7FF && mb))
            return NAN64;
        if ((!ea && !ma) || (!eb && !mb))
            return NAN64; /* infinity times zero */
        return PACK(s, 0x7FF, 0);
    }
    if (!ea) {
        if (!ma)
            return PACK(s, 0, 0);
        normalize(&ea, &ma);
    }
    if (!eb) {
        if (!mb)
            return PACK(s, 0, 0);
        normalize(&eb, &mb);
    }
    ez = ea + eb - 0x3FF;
    ma = (ma | 0x0010000000000000ull) << 10;
    mb = (mb | 0x0010000000000000ull) << 11;
    hi = mul_hi(ma, mb, &lo);
    hi |= lo != 0;
    if (hi < 0x4000000000000000ull) {
        ez--;
        hi <<= 1;
    }
    return round_pack(s, ez, hi);
}

f64 f64_div(f64 a, f64 b)
{
    uint32_t s = SIGN(a) ^ SIGN(b);
    int32_t ea = EXP(a), eb = EXP(b), ez, i;
    uint64_t ma = FRAC(a), mb = FRAC(b), q = 0;
    if (ea == 0x7FF) {
        if (ma || eb == 0x7FF)
            return NAN64;
        return PACK(s, 0x7FF, 0);
    }
    if (eb == 0x7FF)
        return mb ? NAN64 : PACK(s, 0, 0);
    if (!eb) {
        if (!mb)
            return !ea && !ma ? NAN64 : PACK(s, 0x7FF, 0);
        normalize(&eb, &mb);
    }
    if (!ea) {
        if (!ma)
            return PACK(s, 0, 0);
        normalize(&ea, &ma);
    }
    ez = ea - eb + 0x3FE;
    ma |= 0x0010000000000000ull;
    mb |= 0x0010000000000000ull;
    if (ma < mb) {
        ez--;
        ma <<= 1;
    }
    for (i = 0; i < 63; i++) {
        q <<= 1;
        if (ma >= mb) {
            ma -= mb;
            q |= 1;
        }
        ma <<= 1;
    }
    return round_pack(s, ez, q | (ma != 0));
}
