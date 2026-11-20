/* BLAKE3-256, unkeyed, one-shot: enough for image IDs and state hashes.
   Written from the BLAKE3 paper's description (compression function,
   chunks of 1,024 bytes, binary tree of parents). Portable, not fast. */
#include "core.h"

static const uint32_t IV[8] = {0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
                               0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19};
static const uint8_t PERM[16] = {2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8};
enum { CHUNK_START = 1, CHUNK_END = 2, PARENT = 4, ROOT = 8 };

static uint32_t rotr(uint32_t x, int n) { return x >> n | x << (32 - n); }

static void g(uint32_t *s, int a, int b, int c, int d, uint32_t x, uint32_t y)
{
    s[a] += s[b] + x; s[d] = rotr(s[d] ^ s[a], 16);
    s[c] += s[d];     s[b] = rotr(s[b] ^ s[c], 12);
    s[a] += s[b] + y; s[d] = rotr(s[d] ^ s[a], 8);
    s[c] += s[d];     s[b] = rotr(s[b] ^ s[c], 7);
}

/* cv := first 8 words of the compression output */
static void compress(uint32_t cv[8], const uint32_t block[16], uint64_t counter,
                     uint32_t len, uint32_t flags)
{
    uint32_t s[16], m[16], t[16];
    memcpy(s, cv, 32);
    memcpy(s + 8, IV, 16);
    s[12] = (uint32_t)counter; s[13] = (uint32_t)(counter >> 32);
    s[14] = len; s[15] = flags;
    memcpy(m, block, 64);
    for (int r = 0; r < 7; r++) {
        g(s, 0, 4, 8, 12, m[0], m[1]);   g(s, 1, 5, 9, 13, m[2], m[3]);
        g(s, 2, 6, 10, 14, m[4], m[5]);  g(s, 3, 7, 11, 15, m[6], m[7]);
        g(s, 0, 5, 10, 15, m[8], m[9]);  g(s, 1, 6, 11, 12, m[10], m[11]);
        g(s, 2, 7, 8, 13, m[12], m[13]); g(s, 3, 4, 9, 14, m[14], m[15]);
        for (int i = 0; i < 16; i++) t[i] = m[PERM[i]];
        memcpy(m, t, 64);
    }
    for (int i = 0; i < 8; i++) cv[i] = s[i] ^ s[i + 8];
}

static void chunk_cv(const uint8_t *p, size_t len, uint64_t ctr, int root, uint32_t cv[8])
{
    memcpy(cv, IV, 32);
    size_t nblocks = len ? (len + 63) / 64 : 1;
    for (size_t b = 0; b < nblocks; b++) {
        uint8_t buf[64] = {0};
        size_t n = len - b * 64 > 64 ? 64 : len - b * 64;
        memcpy(buf, p + b * 64, n);
        uint32_t w[16];
        for (int i = 0; i < 16; i++) w[i] = rd32(buf + 4 * i);
        uint32_t f = (b == 0 ? CHUNK_START : 0) | (b == nblocks - 1 ? CHUNK_END : 0);
        if (root && b == nblocks - 1) f |= ROOT;
        compress(cv, w, ctr, (uint32_t)n, f);
    }
}

static void subtree(const uint8_t *p, size_t len, uint64_t ctr, int root, uint32_t cv[8])
{
    if (len <= 1024) { chunk_cv(p, len, ctr, root, cv); return; }
    size_t full = (len - 1) / 1024, pow2 = 1;
    while (pow2 * 2 <= full) pow2 *= 2;
    size_t left = pow2 * 1024;
    uint32_t w[16];
    subtree(p, left, ctr, 0, w);
    subtree(p + left, len - left, ctr + pow2, 0, w + 8);
    memcpy(cv, IV, 32);
    compress(cv, w, 0, 64, PARENT | (root ? ROOT : 0));
}

void blake3(const void *data, size_t len, uint8_t out[32])
{
    uint32_t cv[8];
    subtree(data, len, 0, 1, cv);
    for (int i = 0; i < 8; i++) wr32(out + 4 * i, cv[i]);
}
