package article

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"tersereader/internal/vecfile"
)

// normalise applies the comparison rule of vectors/README to a dump: in
// every block but `pre`, runs of blanks in text become one space, and each
// block's or cell's text is trimmed at its ends (text pieces left empty go).
func normalise(t *testing.T, dump string) string {
	lines := strings.Split(strings.TrimRight(dump, "\n"), "\n")
	var out []string
	inPre := false
	// group: indices into out of the text lines of the current container
	var group []int
	var groupInd string
	closeGroup := func() {
		if len(group) == 0 {
			return
		}
		// the container's inline lines are the ones after the header at
		// groupInd; trim the first and last if they are text
		var items []int
		for i := group[0]; i < len(out); i++ {
			if strings.HasPrefix(out[i], groupInd) {
				items = append(items, i)
			}
		}
		trim := func(i int, left bool) {
			if !strings.HasPrefix(out[i], groupInd+"text ") {
				return
			}
			s, err := vecfile.Unquote(strings.TrimPrefix(out[i], groupInd+"text "))
			if err != nil {
				t.Fatal(err)
			}
			if left {
				s = strings.TrimLeft(s, " ")
			} else {
				s = strings.TrimRight(s, " ")
			}
			out[i] = groupInd + "text " + Quote(s)
		}
		if len(items) > 0 {
			trim(items[0], true)
			trim(items[len(items)-1], false)
		}
		group = nil
	}
	for _, l := range lines {
		trimmed := strings.TrimLeft(l, " ")
		ind := l[:len(l)-len(trimmed)]
		isInline := strings.HasPrefix(trimmed, "text ") || strings.HasPrefix(trimmed, "code ") || strings.HasPrefix(trimmed, "link ")
		if !isInline {
			closeGroup()
			if ind == "" {
				inPre = trimmed == "pre"
			}
			out = append(out, l)
			continue
		}
		if !inPre && strings.HasPrefix(trimmed, "text ") {
			s, err := vecfile.Unquote(strings.TrimPrefix(trimmed, "text "))
			if err != nil {
				t.Fatal(err)
			}
			s = strings.Join(strings.FieldsFunc(s, func(r rune) bool { return r == ' ' || r == '\t' }), " ") +
				"" // collapse runs; keep a leading/trailing blank as one space
			orig, _ := vecfile.Unquote(strings.TrimPrefix(trimmed, "text "))
			if len(orig) > 0 && (orig[0] == ' ' || orig[0] == '\t') {
				s = " " + s
			}
			if len(orig) > 0 && (orig[len(orig)-1] == ' ' || orig[len(orig)-1] == '\t') && strings.TrimSpace(orig) != "" {
				s += " "
			}
			l = ind + "text " + Quote(s)
		}
		if !inPre {
			if len(group) == 0 {
				groupInd = ind
			}
			group = append(group, len(out))
		}
		out = append(out, l)
	}
	closeGroup()
	var final []string
	for _, l := range out {
		if strings.TrimLeft(l, " ") == `text ""` {
			continue
		}
		final = append(final, l)
	}
	return strings.Join(final, "\n") + "\n"
}

func TestArticleVectors(t *testing.T) {
	dir, err := vecfile.Dir()
	if err != nil {
		t.Fatal(err)
	}
	files, _ := filepath.Glob(filepath.Join(dir, "articles", "*.txt"))
	if len(files) < 40 {
		t.Fatalf("only %d article vectors", len(files))
	}
	for _, f := range files {
		name := strings.TrimSuffix(filepath.Base(f), ".txt")
		src, err := os.ReadFile(f)
		if err != nil {
			t.Fatal(err)
		}
		want, err := os.ReadFile(strings.TrimSuffix(f, ".txt") + ".dump")
		if err != nil {
			t.Fatal(err)
		}
		got := Dump(Parse(src))
		if normalise(t, got) != normalise(t, string(want)) {
			t.Errorf("%s: dump differs\n--- got\n%s--- want\n%s", name, got, want)
		}
	}
}
