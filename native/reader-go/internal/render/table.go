package render

import (
	"strings"

	"tersereader/internal/article"
)

// minColumn is how far a column is narrowed at most, in terminal columns
// (about the reference reader's 48 units of a 512-unit column).
const minColumn = 8

// table lays a table out as a grid: columns narrowed and cells wrapped if
// it is too wide for the column, then shown wider than the column (cut
// off or scrolled by the viewer) if it still does not fit.
func (rd *renderer) table(t *article.Table) {
	n := t.Columns
	rows := make([][][]cell, len(t.Rows))
	natural := make([]int, n)
	for i, r := range t.Rows {
		rows[i] = make([][]cell, n)
		for c := 0; c < n; c++ {
			cs := rd.cells(r[c], Normal, false)
			rows[i][c] = cs
			if w := cellsWidth(cs); w > natural[c] {
				natural[c] = w
			}
		}
	}
	widths := make([]int, n)
	for c := range widths {
		widths[c] = natural[c]
		if widths[c] < 1 {
			widths[c] = 1
		}
	}
	overhead := 3*n + 1
	if rd.opt.Plain {
		overhead = 2 * (n - 1)
	}
	total := func() int {
		s := overhead
		for _, w := range widths {
			s += w
		}
		return s
	}
	for total() > rd.opt.Width {
		widest := -1
		for c, w := range widths {
			if w > minColumn && (widest < 0 || w > widths[widest]) {
				widest = c
			}
		}
		if widest < 0 {
			break
		}
		widths[widest]--
	}
	border := func(l, m, r, fill string) Line {
		var b strings.Builder
		b.WriteString(l)
		for c, w := range widths {
			b.WriteString(strings.Repeat(fill, w+2))
			if c < n-1 {
				b.WriteString(m)
			}
		}
		b.WriteString(r)
		return Line{{Text: b.String(), Style: Border}}
	}
	if !rd.opt.Plain {
		rd.add(border("┌", "┬", "┐", "─"))
	}
	for i, r := range rows {
		header := t.Header && i == 0
		cellLines := make([][]Line, n)
		height := 1
		for c := 0; c < n; c++ {
			cellLines[c] = rd.wrapCells(r[c], widths[c], "", "")
			if len(cellLines[c]) > height {
				height = len(cellLines[c])
			}
		}
		for k := 0; k < height; k++ {
			var line Line
			if !rd.opt.Plain {
				line = append(line, Span{Text: "│ ", Style: Border})
			}
			for c := 0; c < n; c++ {
				var cl Line
				if k < len(cellLines[c]) {
					cl = cellLines[c][k]
				}
				if header {
					for j := range cl {
						if cl[j].Style == Normal {
							cl[j].Style = HeaderCell
						}
					}
				}
				pad := widths[c] - cl.Width()
				if pad < 0 {
					pad = 0
				}
				left := 0
				switch t.Align[c] {
				case article.AlignEnd:
					left = pad
				case article.AlignCentre:
					left = pad / 2
				}
				if left > 0 {
					line = append(line, Span{Text: strings.Repeat(" ", left)})
				}
				line = append(line, cl...)
				right := pad - left
				last := c == n-1
				if rd.opt.Plain {
					if !last {
						line = append(line, Span{Text: strings.Repeat(" ", right+2)})
					}
					continue
				}
				line = append(line, Span{Text: strings.Repeat(" ", right)})
				sep := " │ "
				if last {
					sep = " │"
				}
				line = append(line, Span{Text: sep, Style: Border})
			}
			rd.add(line)
		}
		if header && len(rows) > 1 {
			if rd.opt.Plain {
				rd.add(Line{})
			} else {
				rd.add(border("╞", "╪", "╡", "═"))
			}
		}
	}
	if !rd.opt.Plain {
		rd.add(border("└", "┴", "┘", "─"))
	}
}
