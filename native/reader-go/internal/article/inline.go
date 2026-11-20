package article

import (
	"bytes"
	"strings"

	"tersereader/internal/terse"
)

// InlineKind is the kind of an inline piece.
type InlineKind int

const (
	Text InlineKind = iota
	Code
	LinkPiece
)

// LinkKind is the kind of a link's target.
type LinkKind int

const (
	LinkArticle LinkKind = iota
	LinkFeed
	LinkProgram
)

// Link is a link's target (ARTICLE.md 4).
type Link struct {
	Kind    LinkKind
	ID      string       // 64 hex digits: the article's blob ID, the feed's or publisher's key
	Hints   string       // a feed link's hint list as written, without the `@`
	Name    string       // a program link's name, decoded
	Written string       // the bare link as written
	HintSet []terse.Hint // the parsed hints
}

// Target writes the link's target as a terse: link.
func (l *Link) Target() string { return l.Written }

// Inline is one piece of inline content.
type Inline struct {
	Kind     InlineKind
	Text     string // the text shown (for a link: its text, or the bare link as written)
	Link     *Link
	WithText bool // a link with text
}

const (
	prefixArticle = "terse:"
	prefixFeed    = "terse:feed/"
	prefixProgram = "terse:program/"
)

// bareLink recognises a bare link starting at b[i] (ARTICLE.md 4.1). The
// caller checks that it is not preceded by a letter or digit.
func bareLink(b []byte, i int) (*Link, int, bool) {
	rest := b[i:]
	switch {
	case bytes.HasPrefix(rest, []byte(prefixFeed)):
		p := i + len(prefixFeed)
		if !hex64At(b, p) {
			return nil, 0, false
		}
		end := p + 64
		if end < len(b) && isAlnum(b[end]) {
			return nil, 0, false
		}
		l := &Link{Kind: LinkFeed, ID: string(b[p:end])}
		if end < len(b) && b[end] == '@' {
			cand := hintCandidate(b[end+1:])
			if cand != "" {
				if hs, err := terse.ParseHintList(cand); err == nil {
					l.Hints = cand
					l.HintSet = hs
					end = end + 1 + len(cand)
				}
			}
		}
		l.Written = string(b[i:end])
		return l, end, true
	case bytes.HasPrefix(rest, []byte(prefixProgram)):
		p := i + len(prefixProgram)
		if !hex64At(b, p) || p+64 >= len(b) || b[p+64] != '/' {
			return nil, 0, false
		}
		q := p + 65
		k := q
		for k < len(b) {
			c := b[k]
			if isAlnum(c) || c == '-' || c == '.' || c == '_' || c == '~' {
				k++
				continue
			}
			if c == '%' && k+2 < len(b) && isHex(b[k+1]) && isHex(b[k+2]) {
				k += 3
				continue
			}
			break
		}
		for k > q && b[k-1] == '.' {
			k--
		}
		name, ok := decodePercent(b[q:k])
		if !ok || !terse.ValidDisplayName(name) {
			return nil, 0, false
		}
		return &Link{Kind: LinkProgram, ID: string(b[p : p+64]), Name: string(name), Written: string(b[i:k])}, k, true
	case bytes.HasPrefix(rest, []byte(prefixArticle)):
		p := i + len(prefixArticle)
		if !hex64At(b, p) {
			return nil, 0, false
		}
		end := p + 64
		if end < len(b) && isAlnum(b[end]) {
			return nil, 0, false
		}
		return &Link{Kind: LinkArticle, ID: string(b[p:end]), Written: string(b[i:end])}, end, true
	}
	return nil, 0, false
}

func decodePercent(b []byte) ([]byte, bool) {
	out := make([]byte, 0, len(b))
	for i := 0; i < len(b); i++ {
		if b[i] == '%' {
			if i+2 >= len(b) {
				return nil, false
			}
			out = append(out, unhex(b[i+1])<<4|unhex(b[i+2]))
			i += 2
			continue
		}
		out = append(out, b[i])
	}
	return out, true
}

func unhex(c byte) byte {
	switch {
	case c >= '0' && c <= '9':
		return c - '0'
	case c >= 'a' && c <= 'f':
		return c - 'a' + 10
	default:
		return c - 'A' + 10
	}
}

// hintCandidate is the longest run of hint bytes (letters, digits, `.`,
// `-`, `:`, `,`, `[`, and `]` while a `[` is open), less every `.`, `,`
// and `:` at its end.
func hintCandidate(b []byte) string {
	open := 0
	k := 0
	for k < len(b) {
		c := b[k]
		switch {
		case isAlnum(c) || c == '.' || c == '-' || c == ':' || c == ',':
		case c == '[':
			open++
		case c == ']' && open > 0:
			open--
		default:
			goto done
		}
		k++
	}
done:
	for k > 0 && (b[k-1] == '.' || b[k-1] == ',' || b[k-1] == ':') {
		k--
	}
	return string(b[:k])
}

// linkAt recognises a bare link at b[i], checking what precedes it.
func linkAt(b []byte, i int) (*Link, int, bool) {
	if b[i] != 't' || (i > 0 && isAlnum(b[i-1])) {
		return nil, 0, false
	}
	return bareLink(b, i)
}

// linkWithText recognises `[text](target)` starting at b[i] == '['.
func linkWithText(b []byte, i int) (*Link, []byte, int, bool) {
	k := i + 1
	for k < len(b) && b[k] != ']' {
		if b[k] == '[' || b[k] == '`' {
			return nil, nil, 0, false
		}
		k++
	}
	if k >= len(b) {
		return nil, nil, 0, false
	}
	text := b[i+1 : k]
	if len(text) < 1 || len(text) > 200 {
		return nil, nil, 0, false
	}
	for j := i + 1; j < k; j++ {
		if _, _, ok := linkAt(b, j); ok {
			return nil, nil, 0, false
		}
	}
	if k+1 >= len(b) || b[k+1] != '(' {
		return nil, nil, 0, false
	}
	start := k + 2
	m := bytes.IndexByte(b[start:], ')')
	if m < 0 {
		return nil, nil, 0, false
	}
	target := b[start : start+m]
	if len(target) == 0 {
		return nil, nil, 0, false
	}
	l, end, ok := bareLink(target, 0)
	if !ok || end != len(target) {
		return nil, nil, 0, false
	}
	return l, text, start + m + 1, true
}

// parseInline finds links (and, if code is set, code spans) in one line
// of a block (ARTICLE.md 3). In a table cell (cell set) `\|` is shown as
// `|`.
func parseInline(b []byte, code, cell bool) []Inline {
	var out []Inline
	show := func(raw []byte) string {
		s := Clean(raw)
		if cell {
			s = strings.ReplaceAll(s, `\|`, "|")
		}
		return s
	}
	textStart := 0
	flush := func(end int) {
		if end > textStart {
			out = appendText(out, show(b[textStart:end]))
		}
	}
	for i := 0; i < len(b); {
		c := b[i]
		if code && c == '`' {
			j := bytes.IndexByte(b[i+1:], '`')
			if j >= 1 && j <= 200 {
				flush(i)
				out = append(out, Inline{Kind: Code, Text: show(b[i+1 : i+1+j])})
				i += j + 2
				textStart = i
				continue
			}
		}
		if c == '[' {
			if l, text, end, ok := linkWithText(b, i); ok {
				flush(i)
				out = append(out, Inline{Kind: LinkPiece, Text: show(text), Link: l, WithText: true})
				i = end
				textStart = i
				continue
			}
		}
		if c == 't' {
			if l, end, ok := linkAt(b, i); ok {
				flush(i)
				out = append(out, Inline{Kind: LinkPiece, Text: Clean(b[i:end]), Link: l})
				i = end
				textStart = i
				continue
			}
		}
		i++
	}
	flush(len(b))
	return out
}

func appendText(out []Inline, s string) []Inline {
	if s == "" {
		return out
	}
	if n := len(out); n > 0 && out[n-1].Kind == Text {
		out[n-1].Text += s
		return out
	}
	return append(out, Inline{Kind: Text, Text: s})
}

// joinLines parses each line of a paragraph or list item and joins them
// with a space.
func joinLines(lines [][]byte) []Inline {
	var out []Inline
	for n, l := range lines {
		l = trimLeftBlanks(l)
		if n > 0 {
			out = appendText(out, " ")
		}
		for _, p := range parseInline(l, true, false) {
			if p.Kind == Text {
				out = appendText(out, p.Text)
			} else {
				out = append(out, p)
			}
		}
	}
	return out
}
