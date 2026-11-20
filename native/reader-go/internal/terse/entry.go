// Package terse holds the data formats of the Terse network (NETWORK.md):
// entries, feed keys, display names, profile documents and feed addresses.
package terse

import (
	"encoding/binary"
	"errors"
	"fmt"
	"unicode/utf8"
)

// Entry kinds (NETWORK.md 4.2).
const (
	KindArticle  = 1
	KindProfile  = 2
	KindProgram  = 3
	KindDocument = 4
)

// MaxBlob is the largest blob, MaxProfile the largest profile document,
// and MaxEntry the largest entry.
const (
	MaxBlob    = 1 << 20
	MaxProfile = 4096
	MaxEntry   = 293
	fixedLen   = 109
	sigLen     = 64
)

var entryDomain = []byte("terse-entry-0")

// Entry is a parsed feed entry.
type Entry struct {
	Key     [32]byte
	Seq     uint32
	Time    uint32
	Kind    byte
	Size    uint32
	Blob    [32]byte
	ReplyTo [32]byte
	Title   string
	Raw     []byte // the whole entry as received
}

func (e *Entry) KeyHex() string  { return KeyHex(e.Key[:]) }
func (e *Entry) BlobHex() string { return KeyHex(e.Blob[:]) }

// KindName names a kind for people.
func KindName(k byte) string {
	switch k {
	case KindArticle:
		return "article"
	case KindProfile:
		return "profile"
	case KindProgram:
		return "program"
	case KindDocument:
		return "document"
	}
	return fmt.Sprintf("kind %d", k)
}

// ParseEntry parses and verifies an entry by every rule of NETWORK.md 4.2.
// It returns an error naming the first rule broken.
func ParseEntry(b []byte) (*Entry, error) {
	if len(b) < fixedLen+sigLen {
		return nil, errors.New("shorter than the fixed part")
	}
	n := int(b[108])
	if n > 120 {
		return nil, errors.New("title longer than 120 bytes")
	}
	if len(b) != fixedLen+n+sigLen {
		return nil, errors.New("length does not match the title length")
	}
	e := &Entry{Raw: append([]byte(nil), b...)}
	copy(e.Key[:], b[0:32])
	e.Seq = binary.LittleEndian.Uint32(b[32:])
	e.Time = binary.LittleEndian.Uint32(b[36:])
	e.Kind = b[40]
	e.Size = uint32(b[41]) | uint32(b[42])<<8 | uint32(b[43])<<16
	copy(e.Blob[:], b[44:76])
	copy(e.ReplyTo[:], b[76:108])
	title := b[109 : 109+n]
	sig := b[109+n:]
	msg := make([]byte, 0, len(entryDomain)+fixedLen+n)
	msg = append(msg, entryDomain...)
	msg = append(msg, b[:109+n]...)
	if !VerifyStrict(e.Key[:], msg, sig) {
		return nil, errors.New("signature does not verify")
	}
	if e.Seq < 1 {
		return nil, errors.New("seq is 0")
	}
	if e.Kind == 0 || e.Kind == 255 {
		return nil, errors.New("reserved kind")
	}
	maxSize := uint32(MaxBlob)
	if e.Kind == KindProfile {
		maxSize = MaxProfile
	}
	if e.Size < 1 || e.Size > maxSize {
		return nil, errors.New("blob size out of range")
	}
	if !utf8.Valid(title) {
		return nil, errors.New("title is not UTF-8")
	}
	if (e.Kind == KindProfile || e.Kind == KindProgram) && !ValidDisplayName(title) {
		return nil, errors.New("title is not a display name")
	}
	e.Title = string(title)
	return e, nil
}
