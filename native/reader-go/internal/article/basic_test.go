package article

import "testing"

func TestBasicText(t *testing.T) {
	cases := []struct {
		text  string
		basic bool
	}{
		{"Plain ASCII, tabs\tand\r\nlines. 3 × 4 − 1 — “yes”…", true},
		{"\xef\xbb\xbfmalformed \xff is U+FFFD", true},
		{"caf\u00e9", false},
		{"x\u00a0y", false},
		{"a\ufeffb", false},
	}
	for _, c := range cases {
		if got := BasicText([]byte(c.text)); got != c.basic {
			t.Errorf("%q: %v", c.text, got)
		}
	}
	n := 0
	for r := rune(0); r <= 0x10ffff; r++ {
		if BasicRune(r) && !(r < 0x20 || r >= 0x7f && r <= 0x9f) {
			n++
		}
	}
	if n != 106 {
		t.Errorf("%d characters in the repertoire, want 106", n)
	}
}
