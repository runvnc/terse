package blake3

import (
	"path/filepath"
	"testing"

	"tersereader/internal/vecfile"
)

func TestBlobVectors(t *testing.T) {
	dir, err := vecfile.Dir()
	if err != nil {
		t.Fatal(err)
	}
	recs, err := vecfile.Parse(filepath.Join(dir, "blobs.txt"))
	if err != nil {
		t.Fatal(err)
	}
	n := 0
	for _, r := range recs {
		if r.Type != "blob" {
			continue
		}
		size := vecfile.Atoi(must(r.Get("size")))
		var data []byte
		if h, ok := r.Get("hex"); ok {
			data, err = vecfile.Hex(h)
			if err != nil {
				t.Fatal(err)
			}
		} else {
			data = vecfile.Pattern(size)
		}
		if len(data) != size {
			t.Fatalf("%s: size %d, have %d bytes", r.Name, size, len(data))
		}
		if got, want := Hex(data), must(r.Get("blake3")); got != want {
			t.Errorf("%s: blake3 %s, want %s", r.Name, got, want)
		}
		n++
	}
	if n < 5 {
		t.Fatalf("only %d blob vectors", n)
	}
}

func must(s string, ok bool) string {
	if !ok {
		panic("missing field")
	}
	return s
}
