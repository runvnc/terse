package article

import "unicode/utf8"

// BasicRanges is the basic text repertoire (ARTICLE.md 1.1, the same set
// as the terminal's basic repertoire, SPEC.md §terminal.text), as
// inclusive ranges of code points: printable ASCII, the Latin-1
// Supplement, dashes, quotation marks, bullet, ellipsis, minus sign, box
// drawing and block elements, and U+FFFD. vectors/basic-text.txt lists
// the same ranges; a test checks they agree.
var BasicRanges = [][2]rune{
	{0x0020, 0x007e},
	{0x00a0, 0x00ff},
	{0x2013, 0x2014},
	{0x2018, 0x2019},
	{0x201c, 0x201d},
	{0x2022, 0x2022},
	{0x2026, 0x2026},
	{0x2212, 0x2212},
	{0x2500, 0x259f},
	{0xfffd, 0xfffd},
}

// BasicRune reports whether a character is in the basic text repertoire,
// or is a control character (which ARTICLE.md 1 does not show, or shows
// as a blank).
func BasicRune(r rune) bool {
	if r < 0x20 || r >= 0x7f && r <= 0x9f {
		return true
	}
	for _, x := range BasicRanges {
		if r >= x[0] && r <= x[1] {
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
