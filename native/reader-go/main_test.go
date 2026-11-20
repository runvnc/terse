package main

import (
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
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
			if opt.Plain && strings.ContainsAny(txt, "┌│═⣿") {
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
	for _, d := range []string{"internal/article", "internal/render"} {
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
