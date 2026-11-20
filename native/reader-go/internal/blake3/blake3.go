// Package blake3 is a small, single-threaded BLAKE3-256 hash (hashing
// only: no keyed mode, no key derivation, no extendable output). It is
// written from the BLAKE3 paper; Terse blobs are at most 1 MiB, so the
// whole input is hashed at once.
package blake3

import (
	"encoding/binary"
	"encoding/hex"
	"math/bits"
)

const (
	chunkLen   = 1024
	blockLen   = 64
	chunkStart = 1
	chunkEnd   = 2
	parent     = 4
	root       = 8
)

var iv = [8]uint32{
	0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
	0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
}

var perm = [16]int{2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8}

func g(s *[16]uint32, a, b, c, d int, x, y uint32) {
	s[a] = s[a] + s[b] + x
	s[d] = bits.RotateLeft32(s[d]^s[a], -16)
	s[c] = s[c] + s[d]
	s[b] = bits.RotateLeft32(s[b]^s[c], -12)
	s[a] = s[a] + s[b] + y
	s[d] = bits.RotateLeft32(s[d]^s[a], -8)
	s[c] = s[c] + s[d]
	s[b] = bits.RotateLeft32(s[b]^s[c], -7)
}

func compress(cv [8]uint32, m [16]uint32, counter uint64, blen uint32, flags uint32) [16]uint32 {
	s := [16]uint32{
		cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7],
		iv[0], iv[1], iv[2], iv[3],
		uint32(counter), uint32(counter >> 32), blen, flags,
	}
	for r := 0; r < 7; r++ {
		g(&s, 0, 4, 8, 12, m[0], m[1])
		g(&s, 1, 5, 9, 13, m[2], m[3])
		g(&s, 2, 6, 10, 14, m[4], m[5])
		g(&s, 3, 7, 11, 15, m[6], m[7])
		g(&s, 0, 5, 10, 15, m[8], m[9])
		g(&s, 1, 6, 11, 12, m[10], m[11])
		g(&s, 2, 7, 8, 13, m[12], m[13])
		g(&s, 3, 4, 9, 14, m[14], m[15])
		if r < 6 {
			var n [16]uint32
			for i := range n {
				n[i] = m[perm[i]]
			}
			m = n
		}
	}
	for i := 0; i < 8; i++ {
		s[i] ^= s[i+8]
		s[i+8] ^= cv[i]
	}
	return s
}

func words(b []byte) [16]uint32 {
	var buf [blockLen]byte
	copy(buf[:], b)
	var m [16]uint32
	for i := range m {
		m[i] = binary.LittleEndian.Uint32(buf[4*i:])
	}
	return m
}

func first8(s [16]uint32) [8]uint32 {
	var cv [8]uint32
	copy(cv[:], s[:8])
	return cv
}

// chunkCV hashes one chunk (at most 1024 bytes, possibly empty).
func chunkCV(data []byte, counter uint64, isRoot bool) [8]uint32 {
	cv := iv
	nblocks := (len(data) + blockLen - 1) / blockLen
	if nblocks == 0 {
		nblocks = 1
	}
	for i := 0; i < nblocks; i++ {
		lo := i * blockLen
		hi := lo + blockLen
		if hi > len(data) {
			hi = len(data)
		}
		var flags uint32
		if i == 0 {
			flags |= chunkStart
		}
		if i == nblocks-1 {
			flags |= chunkEnd
			if isRoot {
				flags |= root
			}
		}
		cv = first8(compress(cv, words(data[lo:hi]), counter, uint32(hi-lo), flags))
	}
	return cv
}

func node(data []byte, counter uint64, isRoot bool) [8]uint32 {
	if len(data) <= chunkLen {
		return chunkCV(data, counter, isRoot)
	}
	chunks := (len(data) + chunkLen - 1) / chunkLen
	leftChunks := 1 << (bits.Len(uint(chunks-1)) - 1)
	split := leftChunks * chunkLen
	l := node(data[:split], counter, false)
	r := node(data[split:], counter+uint64(leftChunks), false)
	var m [16]uint32
	copy(m[:8], l[:])
	copy(m[8:], r[:])
	flags := uint32(parent)
	if isRoot {
		flags |= root
	}
	return first8(compress(iv, m, 0, blockLen, flags))
}

// Sum256 returns the BLAKE3-256 hash of data.
func Sum256(data []byte) [32]byte {
	cv := node(data, 0, true)
	var out [32]byte
	for i, w := range cv {
		binary.LittleEndian.PutUint32(out[4*i:], w)
	}
	return out
}

// Hex returns the hash as 64 lowercase hexadecimal digits.
func Hex(data []byte) string {
	h := Sum256(data)
	return hex.EncodeToString(h[:])
}
