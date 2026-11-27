// Package article parses Terse articles (ARTICLE.md) into blocks and
// inline pieces, and writes the dump format of vectors/README.
package article

import (
	"bytes"
)

// BlockKind is the kind of a block as shown.
type BlockKind int

const (
	Heading BlockKind = iota
	Subheading
	Paragraph
	Item
	Pre
	TableBlock
	FigureBlock
)

// Align is a table column's alignment.
type Align int

const (
	AlignStart Align = iota
	AlignCentre
	AlignEnd
)

// Table is a table as shown (ARTICLE.md 2.5).
type Table struct {
	Columns int
	Header  bool
	Align   []Align
	Rows    [][][]Inline // rows, each with exactly Columns cells
}

// Block is one block of an article.
type Block struct {
	Kind   BlockKind
	Inline []Inline   // heading, subheading, paragraph, item
	Lines  [][]Inline // pre: one per line
	Table  *Table
	Figure *Figure
	Source []string // a figure's fence lines (its text, for a reader that does not draw it)
	Info   string   // pre: a fence's info string, blanks trimmed (the format ignores it; code is coloured by it)
}

// Doc is a parsed article.
type Doc struct {
	NotArticle string // "program-image" or "document" for a blob that is not an article
	Blocks     []Block
}

// Kind of a blob reached without an entry (NETWORK.md 1.1).
func BlobKind(b []byte) string {
	switch {
	case bytes.HasPrefix(b, []byte("TERS")):
		return "program-image"
	case bytes.HasPrefix(b, []byte("TDOC")):
		return "document"
	}
	return ""
}

type lineKind int

const (
	lBlank lineKind = iota
	lHeading
	lSubheading
	lItem
	lPre
	lFence
	lTable
	lOrdinary
)

func classify(l []byte) lineKind {
	switch {
	case blankLine(l):
		return lBlank
	case bytes.HasPrefix(l, []byte("## ")):
		return lSubheading
	case bytes.HasPrefix(l, []byte("# ")):
		return lHeading
	case bytes.HasPrefix(l, []byte("- ")), bytes.HasPrefix(l, []byte("* ")):
		return lItem
	case bytes.HasPrefix(l, []byte("    ")), l[0] == '\t':
		return lPre
	case bytes.HasPrefix(l, []byte("```")):
		return lFence
	case l[0] == '|':
		return lTable
	}
	return lOrdinary
}

// Lines splits an article into lines: at LF, a CR before the LF dropped,
// a byte order mark at the start ignored.
func Lines(b []byte) [][]byte {
	b = bytes.TrimPrefix(b, []byte{0xef, 0xbb, 0xbf})
	parts := bytes.Split(b, []byte{'\n'})
	for i, p := range parts {
		if i < len(parts)-1 && len(p) > 0 && p[len(p)-1] == '\r' {
			parts[i] = p[:len(p)-1]
		}
	}
	// A final LF ends the last line; it does not start an empty one.
	if len(parts) > 0 && len(parts[len(parts)-1]) == 0 {
		parts = parts[:len(parts)-1]
	}
	return parts
}

// Parse parses an article blob.
func Parse(b []byte) *Doc {
	if k := BlobKind(b); k != "" {
		return &Doc{NotArticle: k}
	}
	lines := Lines(b)
	d := &Doc{}
	for i := 0; i < len(lines); {
		l := lines[i]
		switch classify(l) {
		case lBlank:
			i++
		case lHeading, lSubheading:
			k := Heading
			skip := 2
			if classify(l) == lSubheading {
				k, skip = Subheading, 3
			}
			d.Blocks = append(d.Blocks, Block{Kind: k, Inline: joinLines([][]byte{trimLeftBlanks(l[skip:])})})
			i++
		case lItem, lOrdinary:
			k := Paragraph
			first := l
			if classify(l) == lItem {
				k, first = Item, l[2:]
			}
			group := [][]byte{first}
			i++
			for i < len(lines) && classify(lines[i]) == lOrdinary {
				group = append(group, lines[i])
				i++
			}
			d.Blocks = append(d.Blocks, Block{Kind: k, Inline: joinLines(group)})
		case lPre:
			var pl [][]Inline
			for i < len(lines) && classify(lines[i]) == lPre {
				pl = append(pl, parseInline(lines[i], false, false))
				i++
			}
			d.Blocks = append(d.Blocks, Block{Kind: Pre, Lines: pl})
		case lTable:
			var rows [][]byte
			for i < len(lines) && classify(lines[i]) == lTable {
				rows = append(rows, lines[i])
				i++
			}
			d.Blocks = append(d.Blocks, Block{Kind: TableBlock, Table: parseTable(rows)})
		case lFence:
			open := l
			i++
			var body [][]byte
			closed := false
			for i < len(lines) {
				if isClosing(lines[i]) {
					closed = true
					i++
					break
				}
				body = append(body, lines[i])
				i++
			}
			d.Blocks = append(d.Blocks, fenceBlocks(open, body, closed)...)
		}
	}
	return d
}

func isClosing(l []byte) bool {
	return bytes.HasPrefix(l, []byte("```")) && blankLine(l[3:])
}

func fenceBlocks(open []byte, body [][]byte, closed bool) []Block {
	if w, h, ok := figureSize(open[3:]); ok {
		var src []string
		for _, l := range body {
			src = append(src, Clean(l))
		}
		if closed {
			if f, err := ParsePath(bytes.Join(body, []byte{'\n'}), w, h); err == nil {
				return []Block{{Kind: FigureBlock, Figure: f, Source: src}}
			}
		}
	}
	if len(body) == 0 {
		return nil
	}
	var pl [][]Inline
	for _, l := range body {
		pl = append(pl, parseInline(l, false, false))
	}
	return []Block{{Kind: Pre, Lines: pl, Info: string(bytes.Trim(open[3:], " \t\r"))}}
}

// figureSize reads a fence's info string as `path W H` (ARTICLE.md 6).
func figureSize(info []byte) (int, int, bool) {
	info = trimLeftBlanks(info)
	if !bytes.HasPrefix(info, []byte("path")) {
		return 0, 0, false
	}
	rest := info[4:]
	if len(rest) == 0 || !isBlank(rest[0]) {
		return 0, 0, false
	}
	rest = trimLeftBlanks(rest)
	w, rest, ok := sizeNumber(rest)
	if !ok || len(rest) == 0 || !isBlank(rest[0]) {
		return 0, 0, false
	}
	rest = trimLeftBlanks(rest)
	h, rest, ok := sizeNumber(rest)
	if !ok || !blankLine(rest) {
		return 0, 0, false
	}
	return w, h, true
}

func sizeNumber(b []byte) (int, []byte, bool) {
	k := 0
	for k < len(b) && b[k] >= '0' && b[k] <= '9' {
		k++
	}
	if k < 1 || k > 4 {
		return 0, b, false
	}
	n := 0
	for _, c := range b[:k] {
		n = n*10 + int(c-'0')
	}
	if n < 1 || n > 1024 {
		return 0, b, false
	}
	return n, b[k:], true
}

// splitCells cuts a table row into its cells as written (ARTICLE.md 2.5).
func splitCells(l []byte) [][]byte {
	l = l[1:]
	l = trimRightBlanks(l)
	if n := len(l); n > 0 && l[n-1] == '|' && (n < 2 || l[n-2] != '\\') {
		l = l[:n-1]
	}
	var cells [][]byte
	start := 0
	for k := 0; k < len(l); k++ {
		if l[k] == '|' && (k == 0 || l[k-1] != '\\') {
			cells = append(cells, l[start:k])
			start = k + 1
		}
	}
	cells = append(cells, l[start:])
	for i, c := range cells {
		cells[i] = trimRightBlanks(trimLeftBlanks(c))
	}
	return cells
}

func isAlignmentRow(l []byte) bool {
	dash := false
	for _, c := range l {
		switch {
		case c == '-':
			dash = true
		case c == '|' || c == ':' || isBlank(c):
		default:
			return false
		}
	}
	return dash
}

func parseTable(rows [][]byte) *Table {
	first := splitCells(rows[0])
	t := &Table{Columns: len(first)}
	if t.Columns > 8 {
		t.Columns = 8
	}
	t.Align = make([]Align, t.Columns)
	body := rows
	if len(rows) > 1 && isAlignmentRow(rows[1]) {
		t.Header = true
		for i, c := range splitCells(rows[1]) {
			if i >= t.Columns {
				break
			}
			n := len(c)
			switch {
			case n >= 2 && c[0] == ':' && c[n-1] == ':':
				t.Align[i] = AlignCentre
			case n >= 1 && c[n-1] == ':' && (c[0] != ':' || n == 1):
				t.Align[i] = AlignEnd
			}
		}
		body = append([][]byte{rows[0]}, rows[2:]...)
	}
	for _, r := range body {
		cells := splitCells(r)
		row := make([][]Inline, t.Columns)
		for i := 0; i < t.Columns && i < len(cells); i++ {
			row[i] = parseInline(cells[i], true, true)
		}
		t.Rows = append(t.Rows, row)
	}
	return t
}
