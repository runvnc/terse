package render

import (
	"strings"
	"testing"

	"tersereader/internal/article"
)

// show writes each run of a fence's lines as class[text].
func show(lang Lang, src string) string {
	lines := strings.Split(src, "\n")
	names := []string{"", "c", "s", "n", "k", "t", "m"}
	var out []string
	for i, rs := range Highlight(lang, lines) {
		l, at := lines[i], 0
		var b strings.Builder
		for _, r := range rs {
			b.WriteString(l[at:r.Start] + names[r.Class] + "[" + l[r.Start:r.End] + "]")
			at = r.End
		}
		b.WriteString(l[at:])
		out = append(out, b.String())
	}
	return strings.Join(out, "\n")
}

func TestLangOf(t *testing.T) {
	for info, want := range map[string]Lang{
		"c": LangC, "rust from examples/x.rs": LangRust, "go": LangGo, "examples/program/c/hello.c": LangC,
		"sh": LangShell, "drive examples/program/line.drive": LangScript, "text": LangNone,
		"text examples/x.c": LangNone, "": LangNone, "toml": LangGeneric, "Rust": LangRust, "path 10 10": LangNone,
	} {
		if got := LangOf(info); got != want {
			t.Errorf("LangOf(%q) = %d, want %d", info, got, want)
		}
	}
}

// The same cases as crates/terse-net/src/highlight.rs.
func TestHighlight(t *testing.T) {
	for _, c := range []struct {
		lang      Lang
		src, want string
	}{
		{LangC, "#include \"terse.h\"\n#include <stdint.h>\nstatic uint8_t x = 0x1F; /* a\nb */ int y; // c\nchar c = '\\n';",
			"m[#include] s[\"terse.h\"]\nm[#include] s[<stdint.h>]\nk[static] t[uint8_t] x = n[0x1F]; c[/* a]\nc[b */] t[int] y; c[// c]\nt[char] c = s['\\n'];"},
		{LangRust, "#![no_std]\nfn main() -> Result<(), String> {\n    println!(\"{}\", 'a', x as u8, 1.5);\n    let s = r#\"q\"#; f::<'a>()",
			"m[#![no_std]]\nk[fn] main() -> t[Result]<(), t[String]> {\n    m[println!](s[\"{}\"], s['a'], x k[as] t[u8], n[1.5]);\n    k[let] s = s[r#\"q\"#]; f::<'a>()"},
		{LangGo, "package main\nfunc f(b []byte) error { return nil } // x\ns := `a\nb`",
			"k[package] main\nk[func] f(b []t[byte]) t[error] { k[return] k[nil] } c[// x]\ns := s[`a]\ns[b`]"},
		{LangShell, "cargo build --release \\\n    --manifest-path x\nX=1 terse run a | grep \"b\" 2>&1 && ls $HOME # c",
			"k[cargo] build --release \\\n    --manifest-path x\nm[X]=1 k[terse] run a | k[grep] s[\"b\"] 2>&1 && k[ls] m[$HOME] c[# c]"},
		{LangShell, "xxd -r -p > f <<EOF\n5445\nEOF\nfor f in a; do\n    a=$(bin/x $f)\ndone",
			"k[xxd] -r -p > f m[<<EOF]\ns[5445]\ns[EOF]\nk[for] f k[in] a; k[do]\n    m[a]=m[$(]k[bin/x] m[$f])\nk[done]"},
		{LangArticle, "# Hello\n\nA `code` span.\n- item\n```c\nint x;\n```",
			"k[# Hello]\n\nA s[`code`] span.\nm[-] item\nm[```c]\ns[int x;]\nm[```]"},
		{LangScript, "# a\n10 press enter\nexpect idle 120", "c[# a]\nn[10] k[press] enter\nk[expect] idle n[120]"},
		{LangGeneric, "[build]\nname = \"x\" # y\n_start: li t0, 100000", "k[[build]]\nname = s[\"x\"] c[# y]\n_start: li t0, n[100000]"},
	} {
		if got := show(c.lang, c.src); got != c.want {
			t.Errorf("lang %d:\n got %q\nwant %q", c.lang, got, c.want)
		}
	}
}

// Coloured output colours a named fence's code; plain output, output
// without ANSI and a fence that names no language have no colour codes.
func TestFenceColours(t *testing.T) {
	src := "# T\n\n```c\nint x = 1; // one\n```\n\n```\nint y;\n```\n"
	d := article.Parse([]byte(src))
	page := Render(d, Options{Width: 60, Color: true, Highlight: true})
	txt := page.Text(true)
	for _, want := range []string{"\x1b[36mint\x1b[0m", "\x1b[33m1\x1b[0m", "\x1b[2;3m// one\x1b[0m"} {
		if !strings.Contains(txt, want) {
			t.Errorf("no %q in %q", want, txt)
		}
	}
	if strings.Contains(txt, "\x1b[36mint\x1b[0m y") {
		t.Errorf("the fence with no language is coloured: %q", txt)
	}
	for _, opt := range []Options{{Width: 60, Plain: true}, {Width: 60}} {
		if txt := Render(d, opt).Text(false); strings.Contains(txt, "\x1b") {
			t.Errorf("escape codes in plain output: %q", txt)
		}
	}
	if txt := Render(d, Options{Width: 60, Color: true}).Text(true); strings.Contains(txt, "\x1b[36m") {
		t.Errorf("coloured without Highlight: %q", txt)
	}
}
