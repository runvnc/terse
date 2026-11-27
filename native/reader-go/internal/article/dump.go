package article

import (
	"fmt"
	"strings"
)

// Quote writes a string as the vectors quote it.
func Quote(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range s {
		switch r {
		case '\\':
			b.WriteString(`\\`)
		case '"':
			b.WriteString(`\"`)
		case '\t':
			b.WriteString(`\t`)
		case '\n':
			b.WriteString(`\n`)
		case '\r':
			b.WriteString(`\r`)
		default:
			b.WriteRune(r)
		}
	}
	b.WriteByte('"')
	return b.String()
}

func dumpInline(b *strings.Builder, ind string, pieces []Inline) {
	for _, p := range pieces {
		switch p.Kind {
		case Text:
			fmt.Fprintf(b, "%stext %s\n", ind, Quote(p.Text))
		case Code:
			fmt.Fprintf(b, "%scode %s\n", ind, Quote(p.Text))
		case LinkPiece:
			b.WriteString(ind + "link " + linkDump(p.Link))
			if p.WithText {
				b.WriteString(" as " + Quote(p.Text))
			}
			b.WriteByte('\n')
		}
	}
}

func linkDump(l *Link) string {
	switch l.Kind {
	case LinkFeed:
		if l.Hints != "" {
			return "feed " + l.ID + " @" + l.Hints
		}
		return "feed " + l.ID
	case LinkProgram:
		return "program " + l.ID + " " + Quote(l.Name)
	}
	return "article " + l.ID
}

// Dump writes a parsed article in the dump format of vectors/README.
func Dump(d *Doc) string {
	var b strings.Builder
	if d.NotArticle != "" {
		return "not-an-article " + d.NotArticle + "\n"
	}
	for _, bl := range d.Blocks {
		switch bl.Kind {
		case Heading, Subheading, Paragraph, Item:
			b.WriteString([]string{"heading", "subheading", "paragraph", "item"}[bl.Kind] + "\n")
			dumpInline(&b, "  ", bl.Inline)
		case Pre:
			b.WriteString("pre\n")
			for _, l := range bl.Lines {
				b.WriteString("  line\n")
				dumpInline(&b, "    ", l)
			}
		case TableBlock:
			t := bl.Table
			h := "noheader"
			if t.Header {
				h = "header"
			}
			fmt.Fprintf(&b, "table %d %s\n  align", t.Columns, h)
			for _, a := range t.Align {
				b.WriteString(" " + []string{"start", "centre", "end"}[a])
			}
			b.WriteByte('\n')
			for _, r := range t.Rows {
				b.WriteString("  row\n")
				for _, c := range r {
					b.WriteString("    cell\n")
					dumpInline(&b, "      ", c)
				}
			}
		case FigureBlock:
			f := bl.Figure
			fmt.Fprintf(&b, "figure %d %d\n", f.W, f.H)
			for _, p := range f.Paths {
				stroke := "none"
				if p.StrokeVisible() {
					stroke = fmt.Sprintf("%s %d", p.Stroke, p.Width)
				}
				layer := ""
				if p.Layer >= 0 {
					layer = fmt.Sprintf(" layer %d", p.Layer)
				}
				fmt.Fprintf(&b, "  path fill %s stroke %s%s\n", p.Fill, stroke, layer)
				for _, o := range p.Ops {
					b.WriteString("    " + string(o.Cmd))
					for _, v := range o.P {
						fmt.Fprintf(&b, " %d", v)
					}
					b.WriteByte('\n')
				}
			}
		}
	}
	return b.String()
}
