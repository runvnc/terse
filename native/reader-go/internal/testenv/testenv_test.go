package testenv

import (
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"
)

func TestRefuses(t *testing.T) {
	for _, c := range []struct {
		no, allow, need string
		want            bool
	}{
		{"", "", "pty", false},
		{"0", "", "pty", false},
		{"1", "", "pty", true},
		{"1", "go", "pty", true},
		{"1", "go, pty", "pty", false},
	} {
		if got := Refuses(c.no, c.allow, c.need); got != c.want {
			t.Errorf("Refuses(%q, %q, %q) = %v", c.no, c.allow, c.need, got)
		}
	}
}

func TestDepFiles(t *testing.T) {
	got := depFiles("/t/terse: /a/b.rs /c\\ d/e.rs  /f.rs\n")
	if want := []string{"/a/b.rs", "/c d/e.rs", "/f.rs"}; !reflect.DeepEqual(got, want) {
		t.Fatalf("%q", got)
	}
}

// A program whose source changed after it was built is stale; one built
// after it is not; one without a dependency file cannot be told.
func TestStale(t *testing.T) {
	dir := t.TempDir()
	src, exe := filepath.Join(dir, "main.rs"), filepath.Join(dir, "prog")
	os.WriteFile(src, []byte("fn main() {}"), 0o644)
	os.WriteFile(exe, []byte("binary"), 0o755)
	if why, noted := Stale(exe); why != "" || !noted {
		t.Fatalf("no .d: %q %v", why, noted)
	}
	os.WriteFile(exe+".d", []byte(exe+": "+src+"\n"), 0o644)
	old := time.Now().Add(-time.Hour)
	os.Chtimes(src, old, old)
	if why, _ := Stale(exe); why != "" {
		t.Fatalf("fresh: %q", why)
	}
	later := time.Now().Add(time.Hour)
	os.Chtimes(src, later, later)
	if why, _ := Stale(exe); why == "" {
		t.Fatal("a source changed after the build: not seen")
	}
}
