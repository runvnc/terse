"""BLAKE3-256 in plain Python (the hash that names a blob, SPEC §content):
story.py names each scene's blob with it. Slow but small; a scene is tens
of kilobytes.

    python3 blake3.py <file>...      prints each file's hash, as b3sum does
"""
import struct
import sys

IV = (0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19)
PERM = (2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8)
CHUNK_START, CHUNK_END, PARENT, ROOT = 1, 2, 4, 8
M = 0xFFFFFFFF


def _g(v, a, b, c, d, x, y):
    v[a] = (v[a] + v[b] + x) & M
    v[d] = v[d] ^ v[a]
    v[d] = (v[d] >> 16 | v[d] << 16) & M
    v[c] = (v[c] + v[d]) & M
    v[b] = v[b] ^ v[c]
    v[b] = (v[b] >> 12 | v[b] << 20) & M
    v[a] = (v[a] + v[b] + y) & M
    v[d] = v[d] ^ v[a]
    v[d] = (v[d] >> 8 | v[d] << 24) & M
    v[c] = (v[c] + v[d]) & M
    v[b] = v[b] ^ v[c]
    v[b] = (v[b] >> 7 | v[b] << 25) & M


def _compress(cv, block, counter, blen, flags):
    m = list(struct.unpack('<16I', block))
    v = list(cv) + list(IV[:4]) + [counter & M, counter >> 32 & M, blen, flags]
    for r in range(7):
        _g(v, 0, 4, 8, 12, m[0], m[1])
        _g(v, 1, 5, 9, 13, m[2], m[3])
        _g(v, 2, 6, 10, 14, m[4], m[5])
        _g(v, 3, 7, 11, 15, m[6], m[7])
        _g(v, 0, 5, 10, 15, m[8], m[9])
        _g(v, 1, 6, 11, 12, m[10], m[11])
        _g(v, 2, 7, 8, 13, m[12], m[13])
        _g(v, 3, 4, 9, 14, m[14], m[15])
        if r < 6:
            m = [m[i] for i in PERM]
    return [v[i] ^ v[i + 8] for i in range(8)] + [v[i + 8] ^ cv[i] for i in range(8)]


def _chunk(data, counter, root):
    """A chunk's chaining value (or, as the root, its first output words)."""
    cv = list(IV)
    blocks = [data[i:i + 64] for i in range(0, len(data), 64)] or [b'']
    for k, b in enumerate(blocks):
        flags = (CHUNK_START if k == 0 else 0) | (CHUNK_END if k == len(blocks) - 1 else 0)
        if k == len(blocks) - 1 and root:
            flags |= ROOT
        cv = _compress(cv, b.ljust(64, b'\0'), counter, len(b), flags)[:8]
    return cv


def _parent(left, right, root):
    return _compress(list(IV), struct.pack('<16I', *left, *right), 0, 64, PARENT | (ROOT if root else 0))[:8]


def blake3(data):
    """The 32-byte BLAKE3 hash of `data`."""
    chunks = [data[i:i + 1024] for i in range(0, len(data), 1024)] or [b'']
    if len(chunks) == 1:
        return struct.pack('<8I', *_chunk(chunks[0], 0, True))
    cvs = [_chunk(c, i, False) for i, c in enumerate(chunks)]

    def tree(lo, hi, root):
        # The left subtree holds the largest power of two of chunks less than all.
        n = hi - lo
        if n == 1:
            return cvs[lo]
        left = 1 << ((n - 1).bit_length() - 1)
        return _parent(tree(lo, lo + left, False), tree(lo + left, hi, False), root)

    return struct.pack('<8I', *tree(0, len(cvs), True))


if __name__ == '__main__':
    for f in sys.argv[1:]:
        print(blake3(open(f, 'rb').read()).hex(), f)
