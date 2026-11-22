package main

import (
	"bufio"
	"go/parser"
	"go/token"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"tersereader/internal/article"
	"tersereader/internal/render"
)

func repoRoot(t *testing.T) string {
	d, _ := os.Getwd()
	for {
		if _, err := os.Stat(filepath.Join(d, "ARTICLE.md")); err == nil {
			return d
		}
		p := filepath.Dir(d)
		if p == d {
			t.Fatal("repository root not found")
		}
		d = p
	}
}

// --version: name, version, and "s2:" with 16 hex digits of SHA-256 of
// the executable (here the test binary).
func TestVersion(t *testing.T) {
	c, err := parseArgs([]string{"--version"})
	if err != nil || !c.version {
		t.Fatalf("parseArgs --version: %v %v", c, err)
	}
	line := versionLine()
	if !regexp.MustCompile(`^tersereader 0\.1\.0 s2:[0-9a-f]{16}$`).MatchString(line) {
		t.Errorf("version line %q", line)
	}
}

// Every article of the built manual parses and renders in every mode.
func TestManualArticles(t *testing.T) {
	files, _ := filepath.Glob(filepath.Join(repoRoot(t), "docs", "manual", "build", "*.txt"))
	if len(files) < 20 {
		t.Fatalf("only %d manual articles", len(files))
	}
	figures, tables := 0, 0
	for _, f := range files {
		b, err := os.ReadFile(f)
		if err != nil {
			t.Fatal(err)
		}
		d := article.Parse(b)
		if d.NotArticle != "" || len(d.Blocks) == 0 {
			t.Errorf("%s: no blocks", f)
		}
		for _, bl := range d.Blocks {
			switch bl.Kind {
			case article.FigureBlock:
				figures++
			case article.TableBlock:
				tables++
			}
		}
		for _, opt := range []render.Options{
			{Width: 72, LinkNumbers: true}, {Width: 72, Plain: true, LinkNumbers: true},
			{Width: 30, Color: true}, {Width: 200},
		} {
			p := render.Render(d, opt)
			txt := p.Text(opt.Color)
			if len(txt) < len(b)/3 {
				t.Errorf("%s: rendered text too short (%d bytes from %d)", f, len(txt), len(b))
			}
			if opt.Plain && !strings.ContainsAny(string(b), "┌│═⣿") && strings.ContainsAny(txt, "┌│═⣿") {
				t.Errorf("%s: box drawing or braille in plain output", f)
			}
		}
	}
	t.Logf("%d manual articles, %d figures, %d tables", len(files), figures, tables)
	if figures == 0 || tables == 0 {
		t.Error("the manual should hold figures and tables")
	}
}

// The reading half never imports the network half: package main (but
// wire.go), article and render use only nodeapi and terse.
func TestSeam(t *testing.T) {
	forbidden := []string{"internal/node", "internal/client", "internal/store", "internal/ws", "internal/blake3"}
	check := func(path string) {
		f, err := parser.ParseFile(token.NewFileSet(), path, nil, parser.ImportsOnly)
		if err != nil {
			t.Fatal(err)
		}
		for _, imp := range f.Imports {
			p := strings.Trim(imp.Path.Value, "\"")
			for _, bad := range forbidden {
				if strings.HasSuffix(p, bad) {
					t.Errorf("%s imports %s", path, p)
				}
			}
		}
	}
	files, _ := filepath.Glob("*.go")
	for _, d := range []string{"internal/article", "internal/render", "internal/local", "internal/localclient"} {
		more, _ := filepath.Glob(filepath.Join(d, "*.go"))
		files = append(files, more...)
	}
	for _, f := range files {
		if f == "wire.go" || strings.HasSuffix(f, "_test.go") {
			continue
		}
		check(f)
	}
}

// TestThinReader: the reader built with -tags thin links only the socket
// client of the network side (LOCAL.md 5): no cryptography, hashing,
// store, WebSocket, relay client or node. It logs the binary's size and
// the lines of Go it is built from, against the full build.
func TestThinReader(t *testing.T) {
	if testing.Short() {
		t.Skip("short")
	}
	deps := func(tags ...string) []string {
		args := append([]string{"list"}, tags...)
		args = append(args, "-deps", "-f", "{{.ImportPath}}|{{.Standard}}|{{.Dir}}|{{join .GoFiles \",\"}}", ".")
		out, err := exec.Command("go", args...).CombinedOutput()
		if err != nil {
			t.Fatalf("go list: %v\n%s", err, out)
		}
		return strings.Split(strings.TrimSpace(string(out)), "\n")
	}
	forbidden := []string{"crypto/ed25519", "crypto/sha512", "crypto/tls", "tersereader/internal/blake3",
		"tersereader/internal/store", "tersereader/internal/ws", "tersereader/internal/client",
		"tersereader/internal/node", "tersereader/internal/localnode"}
	lines := func(tags ...string) (int, int) {
		files, n := 0, 0
		for _, d := range deps(tags...) {
			p := strings.Split(d, "|")
			if len(tags) > 0 {
				for _, bad := range forbidden {
					if p[0] == bad {
						t.Errorf("the thin reader links %s", bad)
					}
				}
			}
			if p[1] == "true" || p[3] == "" {
				continue
			}
			for _, f := range strings.Split(p[3], ",") {
				fh, err := os.Open(filepath.Join(p[2], f))
				if err != nil {
					t.Fatal(err)
				}
				sc := bufio.NewScanner(fh)
				for sc.Scan() {
					n++
				}
				fh.Close()
				files++
			}
		}
		return files, n
	}
	tmp := t.TempDir()
	size := func(out string, tags ...string) int64 {
		args := append([]string{"build", "-p", "4", "-trimpath", "-ldflags=-s -w"}, tags...)
		cmd := exec.Command("go", append(args, "-o", out, ".")...)
		cmd.Env = append(os.Environ(), "CGO_ENABLED=0")
		if b, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("go build %v: %v\n%s", tags, err, b)
		}
		fi, err := os.Stat(out)
		if err != nil {
			t.Fatal(err)
		}
		return fi.Size()
	}
	tf, tl := lines("-tags", "thin")
	ff, fl := lines()
	ts := size(filepath.Join(tmp, "thin"), "-tags", "thin")
	fs := size(filepath.Join(tmp, "full"))
	t.Logf("thin reader: %d lines in %d files, %d bytes stripped; full: %d lines in %d files, %d bytes", tl, tf, ts, fl, ff, fs)
	if ts >= fs || tl >= fl {
		t.Errorf("the thin reader is not smaller")
	}
}
