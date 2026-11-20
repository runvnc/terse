// Package render lays an article (ARTICLE.md) out for a terminal: text
// wrapped to a column, tables as grids, figures drawn with braille dots.
// The result is a list of styled lines that the interactive reader draws
// with ANSI escapes and the dump writes as text.
package render

import (
	"fmt"
	"strings"

	"tersereader/internal/article"
	"tersereader/internal/terse"
)

// Style is how a span is shown.
type Style int

const (
	Normal Style = iota
	Heading1
	Heading2
	CodeStyle
	PreStyle
	LinkStyle
	Mark // link numbers, bullets, notes
	Border
	HeaderCell
	FigureStyle
)

// Span is a run of text in one style. Link is the 1-based number of the
// link it belongs to, or 0. RGB is a figure dot colour (nil: the body
// text colour).
type Span struct {
	Text  string
	Style Style
	Link  int
	RGB   *[3]uint8
	BG    *[3]uint8 // a figure cell's background colour (colour output only)
}

// Line is one line of the page.
type Line []Span

// Width is the line's width in columns.
func (l Line) Width() int {
	n := 0
	for _, s := range l {
		n += StringWidth(s.Text)
	}
	return n
}

// Options control the layout.
type Options struct {
	Width       int  // the column, in terminal columns
	Plain       bool // for screen readers and plain text: no box drawing, figures as their text
	LinkNumbers bool // write [n] after each link
	Color       bool // figures for a colour terminal (else monochrome dots)
}

// Page is a laid-out article.
type Page struct {
	Lines []Line
	Links []*article.Link // link n is Links[n-1]
}

// RecommendedColumn is the column of the recommended width, in
// characters of body text: 512 units (ARTICLE.md 6.2).
const RecommendedColumn = 68

type cell struct {
	r    rune
	st   Style
	link int
	w    int
}

type renderer struct {
	opt  Options
	page *Page
}

// Render lays out a parsed article.
func Render(d *article.Doc, opt Options) *Page {
	if opt.Width < 20 {
		opt.Width = 20
	}
	rd := &renderer{opt: opt, page: &Page{}}
	if d.NotArticle != "" {
		what := "a program image"
		if d.NotArticle == "document" {
			what = "a document"
		}
		rd.add(Line{{Text: "This blob is " + what + ", not an article: this reader cannot open it.", Style: Mark}})
		return rd.page
	}
	prevItem := false
	for i, b := range d.Blocks {
		if i > 0 && !(b.Kind == article.Item && prevItem) {
			rd.add(Line{})
		}
		prevItem = b.Kind == article.Item
		switch b.Kind {
		case article.Heading, article.Subheading:
			st := Heading1
			if b.Kind == article.Subheading {
				st = Heading2
			}
			lines := rd.wrap(b.Inline, opt.Width, "", "", st)
			if len(lines) == 0 {
				lines = []Line{{}}
			}
			for _, l := range lines {
				rd.add(l)
			}
			if !opt.Plain {
				w := 0
				for _, l := range lines {
					if lw := l.Width(); lw > w {
						w = lw
					}
				}
				ch := "━"
				if st == Heading2 {
					ch = "─"
				}
				if w > 0 {
					rd.add(Line{{Text: strings.Repeat(ch, w), Style: st}})
				}
			}
		case article.Paragraph:
			for _, l := range rd.wrap(b.Inline, opt.Width, "", "", Normal) {
				rd.add(l)
			}
		case article.Item:
			bullet := "• "
			if opt.Plain {
				bullet = "- "
			}
			for _, l := range rd.wrapPrefixed(b.Inline, opt.Width, bullet, "  ", Normal) {
				rd.add(l)
			}
		case article.Pre:
			for _, l := range b.Lines {
				rd.add(rd.preLine(l))
			}
		case article.TableBlock:
			rd.table(b.Table)
		case article.FigureBlock:
			if opt.Plain {
				for _, s := range b.Source {
					rd.add(Line{{Text: expandTabs(s), Style: PreStyle}})
				}
			} else {
				rd.figure(b.Figure)
			}
		}
	}
	return rd.page
}

func (rd *renderer) add(l Line) { rd.page.Lines = append(rd.page.Lines, l) }

func (rd *renderer) addLink(l *article.Link) int {
	rd.page.Links = append(rd.page.Links, l)
	return len(rd.page.Links)
}

// ShortLink shows a bare link shortened as the reference reader does:
// the first and last six digits, a program's name, a feed's hints.
func ShortLink(l *article.Link) string {
	short := terse.ShortKey(l.ID)
	switch l.Kind {
	case article.LinkFeed:
		s := "terse:feed/" + short
		if l.Hints != "" {
			s += "@" + l.Hints
		}
		return s
	case article.LinkProgram:
		return "terse:program/" + short + "/" + l.Name
	}
	return "terse:" + short
}

// cells flattens inline pieces into styled characters.
func (rd *renderer) cells(pieces []article.Inline, base Style, inPre bool) []cell {
	var out []cell
	push := func(s string, st Style, link int) {
		for _, r := range s {
			if r == '\t' && !inPre {
				r = ' '
			}
			out = append(out, cell{r: r, st: st, link: link, w: RuneWidth(r)})
		}
	}
	for _, p := range pieces {
		switch p.Kind {
		case article.Text:
			push(p.Text, base, 0)
		case article.Code:
			push(p.Text, CodeStyle, 0)
		case article.LinkPiece:
			n := rd.addLink(p.Link)
			text := p.Text
			if !p.WithText && !inPre {
				text = ShortLink(p.Link)
			}
			push(text, LinkStyle, n)
			if rd.opt.LinkNumbers {
				push(fmt.Sprintf("[%d]", n), Mark, n)
			}
		}
	}
	return out
}

func isSpace(r rune) bool { return r == ' ' || r == '\t' }

// wrap wraps inline content to width columns, breaking between words and
// inside a word only when it is wider than the whole line.
func (rd *renderer) wrap(pieces []article.Inline, width int, first, rest string, base Style) []Line {
	return rd.wrapCells(rd.cells(pieces, base, false), width, first, rest)
}

func (rd *renderer) wrapPrefixed(pieces []article.Inline, width int, first, rest string, base Style) []Line {
	lines := rd.wrapCells(rd.cells(pieces, base, false), width, first, rest)
	if len(lines) == 0 {
		lines = []Line{{{Text: first, Style: Mark}}}
	}
	return lines
}

func (rd *renderer) wrapCells(cs []cell, width int, first, rest string) []Line {
	// split into words: runs of non-blank cells
	var words [][]cell
	for i := 0; i < len(cs); {
		if isSpace(cs[i].r) {
			i++
			continue
		}
		j := i
		for j < len(cs) && !isSpace(cs[j].r) {
			j++
		}
		words = append(words, cs[i:j])
		i = j
	}
	var lines [][]cell
	var cur []cell
	curW := 0
	prefixW := StringWidth(first)
	avail := func() int { return width - prefixW }
	flush := func() {
		lines = append(lines, cur)
		cur, curW = nil, 0
		prefixW = StringWidth(rest)
	}
	for _, w := range words {
		ww := cellsWidth(w)
		need := ww
		if len(cur) > 0 {
			need++
		}
		if curW+need <= avail() {
			if len(cur) > 0 {
				cur = append(cur, cell{r: ' ', st: w[0].st, link: joinLink(cur[len(cur)-1], w[0]), w: 1})
				curW++
			}
			cur = append(cur, w...)
			curW += ww
			continue
		}
		if len(cur) > 0 {
			flush()
		}
		// a word wider than the whole line is broken anywhere
		for cellsWidth(w) > avail() && avail() > 0 {
			k, kw := 0, 0
			for k < len(w) && kw+w[k].w <= avail() {
				kw += w[k].w
				k++
			}
			if k == 0 {
				k = 1
			}
			cur = append(cur, w[:k]...)
			w = w[k:]
			flush()
		}
		cur = append(cur, w...)
		curW = cellsWidth(w)
	}
	if len(cur) > 0 {
		lines = append(lines, cur)
	}
	out := make([]Line, 0, len(lines))
	for i, l := range lines {
		p := rest
		if i == 0 {
			p = first
		}
		var line Line
		if p != "" {
			line = append(line, Span{Text: p, Style: Mark})
		}
		out = append(out, append(line, spans(l)...))
	}
	return out
}

// joinLink keeps a space inside a link's text part of the link.
func joinLink(a, b cell) int {
	if a.link != 0 && a.link == b.link {
		return a.link
	}
	return 0
}

func cellsWidth(cs []cell) int {
	n := 0
	for _, c := range cs {
		n += c.w
	}
	return n
}

func spans(cs []cell) Line {
	var l Line
	for _, c := range cs {
		st := c.st
		if c.r == ' ' && c.link == 0 && st == LinkStyle {
			st = Normal
		}
		if n := len(l); n > 0 && l[n-1].Style == st && l[n-1].Link == c.link {
			l[n-1].Text += string(c.r)
			continue
		}
		l = append(l, Span{Text: string(c.r), Style: st, Link: c.link})
	}
	return l
}

func expandTabs(s string) string {
	if !strings.ContainsRune(s, '\t') {
		return s
	}
	var b strings.Builder
	col := 0
	for _, r := range s {
		if r == '\t' {
			n := 4 - col%4
			b.WriteString(strings.Repeat(" ", n))
			col += n
			continue
		}
		b.WriteRune(r)
		col += RuneWidth(r)
	}
	return b.String()
}

// preLine shows a preformatted line as written (tabs as spaces to the
// next multiple of four), never wrapped.
func (rd *renderer) preLine(pieces []article.Inline) Line {
	var out []cell
	col := 0
	for _, c := range rd.cells(pieces, PreStyle, true) {
		if c.r == '\t' {
			n := 4 - col%4
			for k := 0; k < n; k++ {
				out = append(out, cell{r: ' ', st: c.st, link: c.link, w: 1})
			}
			col += n
			continue
		}
		out = append(out, c)
		col += c.w
	}
	return spans(out)
}

// Text writes the page as text, with ANSI styles if ansi is set.
func (p *Page) Text(ansi bool) string {
	var b strings.Builder
	for _, l := range p.Lines {
		b.WriteString(LineText(l, ansi, -1))
		b.WriteByte('\n')
	}
	return b.String()
}

// LineText writes one line; selected is the link shown as selected.
func LineText(l Line, ansi bool, selected int) string {
	var b strings.Builder
	for _, s := range l {
		if !ansi {
			b.WriteString(s.Text)
			continue
		}
		b.WriteString(StyleCode(s, selected))
		b.WriteString(s.Text)
		b.WriteString("\x1b[0m")
	}
	if !ansi {
		return strings.TrimRight(b.String(), " ")
	}
	return b.String()
}

// StyleCode is the ANSI escape that starts a span.
func StyleCode(s Span, selected int) string {
	code := ""
	switch s.Style {
	case Heading1:
		code = "\x1b[1;4m"
	case Heading2:
		code = "\x1b[1m"
	case CodeStyle:
		code = "\x1b[36m"
	case PreStyle:
		code = ""
	case LinkStyle:
		code = "\x1b[34;4m"
	case Mark, Border:
		code = "\x1b[2m"
	case HeaderCell:
		code = "\x1b[1m"
	case FigureStyle:
		if s.RGB != nil {
			code = fmt.Sprintf("\x1b[38;2;%d;%d;%dm", s.RGB[0], s.RGB[1], s.RGB[2])
		}
		if s.BG != nil {
			code += fmt.Sprintf("\x1b[48;2;%d;%d;%dm", s.BG[0], s.BG[1], s.BG[2])
		}
	}
	if s.Link != 0 && s.Link == selected {
		code += "\x1b[7m"
	}
	return code
}
