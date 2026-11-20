package article

import (
	"fmt"
	"path/filepath"
	"strconv"
	"testing"

	"tersereader/internal/vecfile"
)

func TestBasicText(t *testing.T) {
	cases := []struct {
		text  string
		basic bool
	}{
		{"Plain ASCII, tabs\tand\r\nlines. 3 × 4 − 1 — “yes”…", true},
		{"\xef\xbb\xbfmalformed \xff is U+FFFD", true},
		{"café, x y, ─█", true},
		{"a\ufeffb", false},
		{"5 €", false},
	}
	for _, c := range cases {
		if got := BasicText([]byte(c.text)); got != c.basic {
			t.Errorf("%q: %v", c.text, got)
		}
	}
}

// TestBasicTextVectors checks the repertoire against vectors/basic-text.txt
// (the list ARTICLE.md 1.1 and SPEC.md §terminal.text share), and each of
// its texts.
func TestBasicTextVectors(t *testing.T) {
	dir, err := vecfile.Dir()
	if err != nil {
		t.Fatal(err)
	}
	recs, err := vecfile.Parse(filepath.Join(dir, "basic-text.txt"))
	if err != nil {
		t.Fatal(err)
	}
	var ranges [][2]rune
	count, texts := -1, 0
	for _, r := range recs {
		switch r.Type {
		case "repertoire":
			for _, v := range r.All("range") {
				var lo, hi rune
				if _, err := fmt.Sscanf(v, "%x %x", &lo, &hi); err != nil {
					t.Fatalf("range %q: %v", v, err)
				}
				ranges = append(ranges, [2]rune{lo, hi})
			}
			c, _ := r.Get("count")
			count, _ = strconv.Atoi(c)
		case "text":
			h, _ := r.Get("hex")
			b, err := vecfile.Hex(h)
			if err != nil {
				t.Fatal(err)
			}
			want, _ := r.Get("basic")
			if got := BasicText(b); got != (want == "yes") {
				t.Errorf("%s: BasicText %v, want %s", r.Name, got, want)
			}
			texts++
		}
	}
	if fmt.Sprint(ranges) != fmt.Sprint(BasicRanges) {
		t.Errorf("ranges %x, vectors say %x", BasicRanges, ranges)
	}
	if texts < 10 {
		t.Errorf("only %d texts", texts)
	}
	n := 0
	for r := rune(0); r <= 0x10ffff; r++ {
		if BasicRune(r) && !(r < 0x20 || r >= 0x7f && r <= 0x9f) {
			n++
		}
	}
	if n != count || n != 361 {
		t.Errorf("%d characters in the repertoire, vectors say %d", n, count)
	}
}
