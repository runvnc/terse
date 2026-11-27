package main

import (
	"strings"
	"testing"

	"tersereader/internal/article"
	"tersereader/internal/render"
)

// show and --dump: ANSI (and so coloured code) only with --color, or on a
// terminal without NO_COLOR, TERM=dumb or --plain; --no-color wins.
func TestColorChoice(t *testing.T) {
	t.Setenv("NO_COLOR", "")
	t.Setenv("TERM", "xterm-256color")
	if (&config{}).ansi() {
		t.Error("ANSI on a pipe without --color")
	}
	if !(&config{color: true}).ansi() || (&config{color: true, noColor: true}).ansi() {
		t.Error("--color, --no-color")
	}
	if !colorTerminal() {
		t.Error("colours not allowed with TERM=xterm-256color")
	}
	t.Setenv("NO_COLOR", "1")
	if colorTerminal() {
		t.Error("colours allowed with NO_COLOR")
	}
	t.Setenv("NO_COLOR", "")
	t.Setenv("TERM", "dumb")
	if colorTerminal() {
		t.Error("colours allowed with TERM=dumb")
	}

	d := article.Parse([]byte("```go\nfunc f() {}\n```\n"))
	cfg := &config{width: 40, color: true}
	if txt := render.Render(d, cfg.renderOptions()).Text(true); !strings.Contains(txt, "\x1b[35mfunc") {
		t.Errorf("--color: the code is not coloured: %q", txt)
	}
	cfg = &config{width: 40, color: true, plain: true}
	if txt := render.Render(d, cfg.renderOptions()).Text(true); strings.Contains(txt, "\x1b[35m") {
		t.Errorf("--plain --color: the code is coloured: %q", txt)
	}
}
