//go:build !thin

// The cryptography of feed keys and entries. A thin reader (built with
// -tags thin, LOCAL.md) leaves all of it to its node: key_thin.go.

package terse

import (
	"crypto/ed25519"
	"math/big"
	"sync"
)

// The curve edwards25519: -x^2 + y^2 = 1 + d x^2 y^2 over GF(p).
var (
	fieldP = func() *big.Int {
		p := new(big.Int).Lsh(big.NewInt(1), 255)
		return p.Sub(p, big.NewInt(19))
	}()
	curveD = func() *big.Int {
		// d = -121665 / 121666
		n := big.NewInt(-121665)
		inv := new(big.Int).ModInverse(big.NewInt(121666), fieldP)
		n.Mul(n, inv)
		return n.Mod(n, fieldP)
	}()
)

type point struct{ x, y *big.Int }

func mod(a *big.Int) *big.Int { return a.Mod(a, fieldP) }

// decodePoint decodes a 32-byte point encoding (RFC 8032 section 5.1.3):
// y must be below p (canonical), x must exist, and x = 0 with the sign
// bit set is refused.
func decodePoint(b []byte) (point, bool) {
	if len(b) != 32 {
		return point{}, false
	}
	le := make([]byte, 32)
	copy(le, b)
	sign := le[31] >> 7
	le[31] &= 0x7f
	be := make([]byte, 32)
	for i := range le {
		be[31-i] = le[i]
	}
	y := new(big.Int).SetBytes(be)
	if y.Cmp(fieldP) >= 0 {
		return point{}, false
	}
	y2 := mod(new(big.Int).Mul(y, y))
	u := mod(new(big.Int).Sub(y2, big.NewInt(1)))
	v := mod(new(big.Int).Add(mod(new(big.Int).Mul(curveD, y2)), big.NewInt(1)))
	vinv := new(big.Int).ModInverse(v, fieldP)
	if vinv == nil {
		return point{}, false
	}
	x2 := mod(new(big.Int).Mul(u, vinv))
	x := new(big.Int).ModSqrt(x2, fieldP)
	if x == nil {
		return point{}, false
	}
	if x.Sign() == 0 && sign == 1 {
		return point{}, false
	}
	if uint(x.Bit(0)) != uint(sign) {
		x = mod(new(big.Int).Sub(fieldP, x))
	}
	return point{x, y}, true
}

// add adds two affine points of the twisted Edwards curve (a = -1).
func add(p, q point) point {
	x1y2 := mod(new(big.Int).Mul(p.x, q.y))
	y1x2 := mod(new(big.Int).Mul(p.y, q.x))
	y1y2 := mod(new(big.Int).Mul(p.y, q.y))
	x1x2 := mod(new(big.Int).Mul(p.x, q.x))
	t := mod(new(big.Int).Mul(curveD, mod(new(big.Int).Mul(x1x2, y1y2))))
	one := big.NewInt(1)
	xd := new(big.Int).ModInverse(mod(new(big.Int).Add(one, t)), fieldP)
	yd := new(big.Int).ModInverse(mod(new(big.Int).Sub(one, t)), fieldP)
	x3 := mod(new(big.Int).Mul(mod(new(big.Int).Add(x1y2, y1x2)), xd))
	y3 := mod(new(big.Int).Mul(mod(new(big.Int).Add(y1y2, x1x2)), yd))
	return point{x3, y3}
}

// smallOrder reports whether [8]P is the neutral point.
func smallOrder(p point) bool {
	for i := 0; i < 3; i++ {
		p = add(p, p)
	}
	return p.x.Sign() == 0 && p.y.Cmp(big.NewInt(1)) == 0
}

var keyCache sync.Map // string(key) -> bool

// ValidFeedKey reports whether 32 bytes are a valid feed key (NETWORK.md
// 4.1): they decode to an Ed25519 point that is not of small order.
func ValidFeedKey(k []byte) bool {
	if len(k) != 32 {
		return false
	}
	if v, ok := keyCache.Load(string(k)); ok {
		return v.(bool)
	}
	p, ok := decodePoint(k)
	valid := ok && !smallOrder(p)
	keyCache.Store(string(k), valid)
	return valid
}

// smallOrderEncoding reports whether 32 bytes decode to a point of small
// order (used for a signature's R).
func smallOrderEncoding(b []byte) bool {
	p, ok := decodePoint(b)
	return ok && smallOrder(p)
}

// VerifyStrict is Ed25519 verification that refuses small-order or
// undecodable keys, small-order R and non-canonical S (S >= L). The
// standard library already refuses S >= L.
func VerifyStrict(key, msg, sig []byte) bool {
	if len(sig) != 64 || !ValidFeedKey(key) {
		return false
	}
	if smallOrderEncoding(sig[:32]) {
		return false
	}
	return ed25519.Verify(ed25519.PublicKey(key), msg, sig)
}
