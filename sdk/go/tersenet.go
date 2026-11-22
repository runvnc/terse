// Package tersenet checks Tersenet feed entries: every validity rule
// of NETWORK.md 4.2, with the strict Ed25519 verification the network
// asks for (ordinary Ed25519 libraries accept signatures it refuses).
// It has no options, no state and does no I/O.
package tersenet

import (
	"bytes"
	"crypto/sha512"
	"encoding/binary"
	"errors"
	"unicode/utf8"

	"filippo.io/edwards25519"
)

// Entry is a valid entry's fields (NETWORK.md 4.2).
type Entry struct {
	Feed    [32]byte // the feed key
	Seq     uint32   // 1 for the feed's first entry
	Time    uint32   // Unix seconds, as claimed by the author
	Kind    byte     // 1 article, 2 profile, 3 program, 4 document
	Size    uint32   // the blob's size in bytes
	Blob    [32]byte // the blob's ID
	ReplyTo [32]byte // the blob replied to, or all zeros
	Title   string
}

// VerifyEntry returns one of these: the rule the entry breaks.
var (
	ErrLength      = errors.New("tersenet: the entry's length does not match its title length")
	ErrTitleLength = errors.New("tersenet: the title is longer than 120 bytes")
	ErrTitleUTF8   = errors.New("tersenet: the title is not valid UTF-8")
	ErrSeq         = errors.New("tersenet: seq is 0")
	ErrKind        = errors.New("tersenet: the kind is reserved (0 or 255)")
	ErrSize        = errors.New("tersenet: the size is not 1 to 1,048,576 bytes (4,096 for a profile)")
	ErrName        = errors.New("tersenet: a profile's or a program's title is not a display name")
	ErrKey         = errors.New("tersenet: the feed key is not the canonical encoding of a point of large order")
	ErrSignatureS  = errors.New("tersenet: the signature's S is not below the group order")
	ErrSignatureR  = errors.New("tersenet: the signature's R is not a point of large order")
	ErrSignature   = errors.New("tersenet: the signature does not verify")
)

// VerifyEntry checks an entry, as received, by every rule of NETWORK.md
// 4.2 and returns its fields, or the first rule it breaks.
func VerifyEntry(b []byte) (Entry, error) {
	if len(b) < 173 {
		return Entry{}, ErrLength
	}
	n := int(b[108])
	if n > 120 {
		return Entry{}, ErrTitleLength
	}
	if len(b) != 173+n {
		return Entry{}, ErrLength
	}
	e := Entry{
		Seq:   binary.LittleEndian.Uint32(b[32:]),
		Time:  binary.LittleEndian.Uint32(b[36:]),
		Kind:  b[40],
		Size:  uint32(b[41]) | uint32(b[42])<<8 | uint32(b[43])<<16,
		Title: string(b[109 : 109+n]),
	}
	copy(e.Feed[:], b[0:32])
	copy(e.Blob[:], b[44:76])
	copy(e.ReplyTo[:], b[76:108])
	maxSize := uint32(1 << 20)
	if e.Kind == 2 {
		maxSize = 4096
	}
	switch {
	case !utf8.ValidString(e.Title):
		return Entry{}, ErrTitleUTF8
	case e.Seq == 0:
		return Entry{}, ErrSeq
	case e.Kind == 0 || e.Kind == 255:
		return Entry{}, ErrKind
	case e.Size < 1 || e.Size > maxSize:
		return Entry{}, ErrSize
	case (e.Kind == 2 || e.Kind == 3) && !displayName(e.Title):
		return Entry{}, ErrName
	}
	message := append([]byte("terse-entry-0"), b[:109+n]...)
	if err := verify(b[:32], message, b[109+n:]); err != nil {
		return Entry{}, err
	}
	return e, nil
}

// Verify reports whether signature is a valid Ed25519 signature of
// message by key under Tersenet's strict rules (NETWORK.md 4.2).
func Verify(key, message, signature []byte) bool {
	return len(key) == 32 && len(signature) == 64 &&
		verify(key, message, signature) == nil
}

// ValidFeedKey reports whether key is a valid feed key (NETWORK.md
// 4.1): the canonical encoding of a point not of small order.
func ValidFeedKey(key []byte) bool {
	_, ok := feedPoint(key)
	return ok
}

func feedPoint(key []byte) (*edwards25519.Point, bool) {
	a, err := new(edwards25519.Point).SetBytes(key)
	if err != nil || !bytes.Equal(a.Bytes(), key) || smallOrder(a) {
		return nil, false
	}
	return a, true
}

// verify makes the four checks of NETWORK.md 4.2, in order.
func verify(key, message, sig []byte) error {
	a, ok := feedPoint(key)
	if !ok {
		return ErrKey
	}
	s, err := edwards25519.NewScalar().SetCanonicalBytes(sig[32:])
	if err != nil {
		return ErrSignatureS
	}
	r, err := new(edwards25519.Point).SetBytes(sig[:32])
	if err != nil || smallOrder(r) {
		return ErrSignatureR
	}
	h := sha512.New()
	h.Write(sig[:32])
	h.Write(key)
	h.Write(message)
	k, _ := edwards25519.NewScalar().SetUniformBytes(h.Sum(nil))
	minusA := new(edwards25519.Point).Negate(a)
	// [S]B - [k]A, without the factor of 8, compared byte for byte.
	p := new(edwards25519.Point).VarTimeDoubleScalarBaseMult(k, minusA, s)
	if !bytes.Equal(p.Bytes(), sig[:32]) {
		return ErrSignature
	}
	return nil
}

func smallOrder(p *edwards25519.Point) bool {
	p8 := new(edwards25519.Point).MultByCofactor(p)
	return p8.Equal(edwards25519.NewIdentityPoint()) == 1
}

// displayName: 1 to 48 bytes, no control character, no white space
// first or last (NETWORK.md 4.6).
func displayName(s string) bool {
	if len(s) < 1 || len(s) > 48 {
		return false
	}
	for _, c := range s {
		if c < 0x20 || c >= 0x7f && c <= 0x9f {
			return false
		}
	}
	first, _ := utf8.DecodeRuneInString(s)
	last, _ := utf8.DecodeLastRuneInString(s)
	return !space(first) && !space(last)
}

// space is the Unicode White_Space property.
func space(c rune) bool {
	switch c {
	case 0x20, 0x85, 0xa0, 0x1680, 0x2028, 0x2029, 0x202f, 0x205f, 0x3000:
		return true
	}
	return c >= 0x09 && c <= 0x0d || c >= 0x2000 && c <= 0x200a
}
