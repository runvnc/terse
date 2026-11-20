package article

import (
	"strings"
	"unicode/utf8"
)

// Clean turns bytes of an article into the text shown (ARTICLE.md 1):
// malformed UTF-8 as U+FFFD by the maximal subpart rule, a CR (one that
// is still in a line is a lone CR) as a space, other control characters
// but tab removed.
func Clean(b []byte) string {
	var sb strings.Builder
	sb.Grow(len(b))
	for i := 0; i < len(b); {
		c := b[i]
		if c < 0x80 {
			switch {
			case c == '\r':
				sb.WriteByte(' ')
			case c == '\t':
				sb.WriteByte('\t')
			case c < 0x20 || c == 0x7f:
			default:
				sb.WriteByte(c)
			}
			i++
			continue
		}
		r, n := decodeMaximal(b[i:])
		i += n
		if r >= 0x80 && r <= 0x9f {
			continue
		}
		sb.WriteRune(r)
	}
	return sb.String()
}

// decodeMaximal decodes one character starting with a non-ASCII byte. On
// a malformed sequence it returns U+FFFD and the length of the maximal
// subpart (Unicode 3.9, "U+FFFD substitution of maximal subparts"): the
// longest prefix that could start a well-formed sequence, at least 1.
func decodeMaximal(b []byte) (rune, int) {
	c := b[0]
	var need int
	lo, hi := byte(0x80), byte(0xbf)
	switch {
	case c >= 0xc2 && c <= 0xdf:
		need = 1
	case c == 0xe0:
		need, lo = 2, 0xa0
	case c >= 0xe1 && c <= 0xec, c == 0xee, c == 0xef:
		need = 2
	case c == 0xed:
		need, hi = 2, 0x9f
	case c == 0xf0:
		need, lo = 3, 0x90
	case c >= 0xf1 && c <= 0xf3:
		need = 3
	case c == 0xf4:
		need, hi = 3, 0x8f
	default:
		return utf8.RuneError, 1
	}
	for k := 1; k <= need; k++ {
		if k >= len(b) {
			return utf8.RuneError, k
		}
		x := b[k]
		if k == 1 {
			if x < lo || x > hi {
				return utf8.RuneError, 1
			}
		} else if x < 0x80 || x > 0xbf {
			return utf8.RuneError, k
		}
	}
	r, n := utf8.DecodeRune(b[:need+1])
	return r, n
}

func isBlank(c byte) bool { return c == ' ' || c == '\t' || c == '\r' }

func blankLine(l []byte) bool {
	for _, c := range l {
		if !isBlank(c) {
			return false
		}
	}
	return true
}

func trimLeftBlanks(l []byte) []byte {
	for len(l) > 0 && isBlank(l[0]) {
		l = l[1:]
	}
	return l
}

func trimRightBlanks(l []byte) []byte {
	for len(l) > 0 && isBlank(l[len(l)-1]) {
		l = l[:len(l)-1]
	}
	return l
}

func isAlnum(c byte) bool {
	return c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9'
}

func isLowerHex(c byte) bool { return c >= '0' && c <= '9' || c >= 'a' && c <= 'f' }

func isHex(c byte) bool { return isLowerHex(c) || c >= 'A' && c <= 'F' }

func hex64At(b []byte, i int) bool {
	if i+64 > len(b) {
		return false
	}
	for k := i; k < i+64; k++ {
		if !isLowerHex(b[k]) {
			return false
		}
	}
	return true
}
