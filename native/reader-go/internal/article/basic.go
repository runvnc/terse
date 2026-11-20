package article

import "unicode/utf8"

// BasicExtra are the characters of the basic text repertoire beyond
// printable ASCII (ARTICLE.md 1.1).
var BasicExtra = []rune{
	'×', '–', '—', '‘', '’', '“', '”',
	'•', '…', '−', '�',
}

// BasicRune reports whether a character is in the basic text repertoire,
// or is a control character (which ARTICLE.md 1 does not show, or shows
// as a blank).
func BasicRune(r rune) bool {
	if r >= 0x20 && r <= 0x7e || r < 0x20 || r >= 0x7f && r <= 0x9f {
		return true
	}
	for _, x := range BasicExtra {
		if r == x {
			return true
		}
	}
	return false
}

// BasicText reports whether an article's bytes (or a title) are in basic
// text: every character shown is in the repertoire. Malformed UTF-8 is
// shown as U+FFFD, which is; a byte order mark that starts it is not shown.
func BasicText(b []byte) bool {
	if len(b) >= 3 && b[0] == 0xef && b[1] == 0xbb && b[2] == 0xbf {
		b = b[3:]
	}
	for len(b) > 0 {
		r, n := utf8.DecodeRune(b)
		if !BasicRune(r) {
			return false
		}
		b = b[n:]
	}
	return true
}
