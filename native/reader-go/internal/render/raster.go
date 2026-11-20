package render

import (
	"math"
	"sort"
	"strings"

	"tersereader/internal/article"
)

// Figures are drawn with braille characters: each character cell holds
// 2 x 4 dots, about square on a terminal whose cells are twice as high as
// wide. Each dot is sampled at its centre; a dot is set where the paths
// drawn there cover it at least a quarter opaque. A cell takes the colour
// of the dots set in it (the body text colour where `fg` dominates).

type pt struct{ x, y float64 }

type subpath struct {
	pts    []pt
	closed bool
}

type pixel struct {
	r, g, b, a, fg float64
}

// maxFigureSegments caps the work of drawing one figure: paths are drawn
// in order up to the one that would pass it (ARTICLE.md 6.3).
const maxFigureSegments = 200000

func (rd *renderer) figure(f *article.Figure) {
	col := rd.opt.Width
	scaleCol := col
	if scaleCol > RecommendedColumn {
		scaleCol = RecommendedColumn
	}
	// 512 units across a column of the recommended width; two dots a
	// character. A narrower column scales the figure down; a figure wider
	// than the column is scaled down to fit.
	dpu := float64(scaleCol*2) / 512
	if float64(f.W)*dpu > float64(col*2) {
		dpu = float64(col*2) / float64(f.W)
	}
	dw := int(math.Ceil(float64(f.W) * dpu))
	dh := int(math.Ceil(float64(f.H) * dpu))
	if dw < 1 {
		dw = 1
	}
	if dh < 1 {
		dh = 1
	}
	cw, ch := (dw+1)/2, (dh+3)/4
	canvas := make([]pixel, dw*dh)
	segments := 0
	note := ""
	for _, p := range f.Paths {
		subs := flatten(p.Ops, dpu)
		n := 0
		for _, s := range subs {
			n += len(s.pts)
		}
		segments += n
		if segments > maxFigureSegments {
			note = "(the rest of this figure is not drawn: it is too costly)"
			break
		}
		if !p.Fill.None {
			mask := fillMask(subs, dw, dh, dpu)
			paint(canvas, mask, p.Fill)
		}
		if p.StrokeVisible() {
			hw := float64(p.Width) / 16 / 2 * dpu // half width in dots
			if hw < 0.6 {
				hw = 0.6
			}
			mask := strokeMask(subs, dw, dh, dpu, hw)
			paint(canvas, mask, p.Stroke)
		}
	}
	// Each dot's colour, quantised: fg, or an RGB value; covered dots only.
	dots := make([]dotColour, dw*dh)
	counts := map[dotColour]int{}
	for i, px := range canvas {
		if px.a < 0.25 {
			continue
		}
		d := dotColour{set: true}
		if px.fg >= 0.5 {
			d.fg = true
		} else {
			d.rgb = [3]uint8{clamp8(px.r), clamp8(px.g), clamp8(px.b)}
		}
		dots[i] = d
		counts[d]++
	}
	// Without colours, a colour that covers much of the box is taken as
	// a panel behind the drawing: its dots are left blank, so that what is
	// drawn over it shows.
	var panel dotColour
	if !rd.opt.Color {
		for d, n := range counts {
			if !d.fg && n*5 >= dw*dh*2 {
				panel = d
			}
		}
	}
	left := (col - cw) / 2
	if left < 0 {
		left = 0
	}
	for cy := 0; cy < ch; cy++ {
		line := Line{}
		if left > 0 {
			line = append(line, Span{Text: strings.Repeat(" ", left)})
		}
		for cx := 0; cx < cw; cx++ {
			var cellDots []int
			var cellBits []int
			inBox := 0
			for dy := 0; dy < 4; dy++ {
				for dx := 0; dx < 2; dx++ {
					x, y := cx*2+dx, cy*4+dy
					if x >= dw || y >= dh {
						continue
					}
					inBox++
					if dots[y*dw+x].set {
						cellDots = append(cellDots, y*dw+x)
						cellBits = append(cellBits, dotBit(dx, dy))
					}
				}
			}
			sp := Span{Text: " ", Style: FigureStyle}
			bg := panel
			if rd.opt.Color {
				// A cell all covered takes its most common colour as its
				// background; the other dots are drawn over it.
				bg = dotColour{}
				if len(cellDots) == inBox && inBox > 0 {
					cc := map[dotColour]int{}
					for _, i := range cellDots {
						cc[dots[i]]++
					}
					best := 0
					for d, n := range cc {
						if !d.fg && (n > best || n == best && less(d, bg)) {
							bg, best = d, n
						}
					}
				}
				if bg.set {
					rgb := bg.rgb
					sp.BG = &rgb
				}
			}
			bits := 0
			var r, g, b, n, fgn float64
			for k, i := range cellDots {
				d := dots[i]
				if bg.set && d == bg {
					continue
				}
				bits |= cellBits[k]
				n++
				if d.fg {
					fgn++
				} else {
					r += float64(d.rgb[0])
					g += float64(d.rgb[1])
					b += float64(d.rgb[2])
				}
			}
			if bits != 0 {
				sp.Text = string(rune(0x2800 + bits))
				if fgn*2 < n {
					m := n - fgn
					sp.RGB = &[3]uint8{clamp8(r / m), clamp8(g / m), clamp8(b / m)}
				}
			}
			if k := len(line); k > 0 && line[k-1].Style == FigureStyle && sameRGB(line[k-1].RGB, sp.RGB) && sameRGB(line[k-1].BG, sp.BG) {
				line[k-1].Text += sp.Text
			} else {
				line = append(line, sp)
			}
		}
		rd.add(line)
	}
	if note != "" {
		rd.add(Line{{Text: note, Style: Mark}})
	}
}

type dotColour struct {
	set bool
	fg  bool
	rgb [3]uint8
}

func less(a, b dotColour) bool {
	if !b.set {
		return true
	}
	return string(a.rgb[:]) < string(b.rgb[:])
}

func clamp8(v float64) uint8 {
	if v < 0 {
		return 0
	}
	if v > 255 {
		return 255
	}
	return uint8(v + 0.5)
}

func sameRGB(a, b *[3]uint8) bool {
	if a == nil || b == nil {
		return a == nil && b == nil
	}
	return *a == *b
}

func dotBit(dx, dy int) int {
	if dy == 3 {
		return []int{0x40, 0x80}[dx]
	}
	return 1 << (dx*3 + dy)
}

// flatten turns a path's commands into polylines in dots.
func flatten(ops []article.Op, dpu float64) []subpath {
	var subs []subpath
	var cur *subpath
	var last pt
	at := func(v int64) float64 { return float64(v) / 16 * dpu }
	for _, o := range ops {
		switch o.Cmd {
		case 'M':
			last = pt{at(o.P[0]), at(o.P[1])}
			subs = append(subs, subpath{pts: []pt{last}})
			cur = &subs[len(subs)-1]
		case 'L':
			last = pt{at(o.P[0]), at(o.P[1])}
			cur.pts = append(cur.pts, last)
		case 'Q':
			c := pt{at(o.P[0]), at(o.P[1])}
			e := pt{at(o.P[2]), at(o.P[3])}
			n := steps(dist(last, c) + dist(c, e))
			for k := 1; k <= n; k++ {
				t := float64(k) / float64(n)
				u := 1 - t
				cur.pts = append(cur.pts, pt{
					u*u*last.x + 2*u*t*c.x + t*t*e.x,
					u*u*last.y + 2*u*t*c.y + t*t*e.y,
				})
			}
			last = e
		case 'C':
			c1 := pt{at(o.P[0]), at(o.P[1])}
			c2 := pt{at(o.P[2]), at(o.P[3])}
			e := pt{at(o.P[4]), at(o.P[5])}
			n := steps(dist(last, c1) + dist(c1, c2) + dist(c2, e))
			for k := 1; k <= n; k++ {
				t := float64(k) / float64(n)
				u := 1 - t
				cur.pts = append(cur.pts, pt{
					u*u*u*last.x + 3*u*u*t*c1.x + 3*u*t*t*c2.x + t*t*t*e.x,
					u*u*u*last.y + 3*u*u*t*c1.y + 3*u*t*t*c2.y + t*t*t*e.y,
				})
			}
			last = e
		case 'Z':
			cur.closed = true
			if len(cur.pts) > 0 {
				last = cur.pts[0]
			}
		}
	}
	return subs
}

func dist(a, b pt) float64 { return math.Hypot(a.x-b.x, a.y-b.y) }

func steps(length float64) int {
	n := int(math.Ceil(length / 1.5))
	if n < 2 {
		n = 2
	}
	if n > 64 {
		n = 64
	}
	return n
}

type crossing struct {
	x   float64
	dir int
}

// fillMask fills with the non-zero winding rule, every subpath counted
// as closed.
func fillMask(subs []subpath, w, h int, _ float64) []bool {
	mask := make([]bool, w*h)
	for j := 0; j < h; j++ {
		y := float64(j) + 0.5
		var xs []crossing
		for _, s := range subs {
			n := len(s.pts)
			if n < 2 {
				continue
			}
			for k := 0; k < n; k++ {
				p, q := s.pts[k], s.pts[(k+1)%n]
				switch {
				case p.y <= y && q.y > y:
					xs = append(xs, crossing{p.x + (y-p.y)*(q.x-p.x)/(q.y-p.y), 1})
				case q.y <= y && p.y > y:
					xs = append(xs, crossing{p.x + (y-p.y)*(q.x-p.x)/(q.y-p.y), -1})
				}
			}
		}
		if len(xs) == 0 {
			continue
		}
		sort.Slice(xs, func(a, b int) bool { return xs[a].x < xs[b].x })
		wind, k := 0, 0
		for i := 0; i < w; i++ {
			x := float64(i) + 0.5
			for k < len(xs) && xs[k].x < x {
				wind += xs[k].dir
				k++
			}
			if wind != 0 {
				mask[j*w+i] = true
			}
		}
	}
	return mask
}

// strokeMask sets the dots within hw of a segment: an outline centred on
// the segments with round joins and ends.
func strokeMask(subs []subpath, w, h int, _ float64, hw float64) []bool {
	mask := make([]bool, w*h)
	seg := func(a, b pt) {
		x0 := int(math.Floor(math.Min(a.x, b.x) - hw))
		x1 := int(math.Ceil(math.Max(a.x, b.x) + hw))
		y0 := int(math.Floor(math.Min(a.y, b.y) - hw))
		y1 := int(math.Ceil(math.Max(a.y, b.y) + hw))
		if x0 < 0 {
			x0 = 0
		}
		if y0 < 0 {
			y0 = 0
		}
		if x1 > w-1 {
			x1 = w - 1
		}
		if y1 > h-1 {
			y1 = h - 1
		}
		dx, dy := b.x-a.x, b.y-a.y
		l2 := dx*dx + dy*dy
		for j := y0; j <= y1; j++ {
			for i := x0; i <= x1; i++ {
				px, py := float64(i)+0.5, float64(j)+0.5
				t := 0.0
				if l2 > 0 {
					t = ((px-a.x)*dx + (py-a.y)*dy) / l2
					t = math.Max(0, math.Min(1, t))
				}
				ex, ey := a.x+t*dx-px, a.y+t*dy-py
				if ex*ex+ey*ey <= hw*hw {
					mask[j*w+i] = true
				}
			}
		}
	}
	for _, s := range subs {
		if len(s.pts) == 1 && s.closed {
			seg(s.pts[0], s.pts[0])
		}
		for k := 0; k+1 < len(s.pts); k++ {
			seg(s.pts[k], s.pts[k+1])
		}
		if s.closed && len(s.pts) > 1 {
			seg(s.pts[len(s.pts)-1], s.pts[0])
		}
	}
	return mask
}

// paint composites a colour over the canvas where the mask is set
// (source over).
func paint(canvas []pixel, mask []bool, c article.Colour) {
	a := float64(c.A) / 255
	r, g, b, fg := float64(c.R), float64(c.G), float64(c.B), 0.0
	if c.FG {
		a, fg = 1, 1
	}
	for i, on := range mask {
		if !on {
			continue
		}
		d := &canvas[i]
		oa := a + d.a*(1-a)
		if oa <= 0 {
			continue
		}
		k := d.a * (1 - a)
		d.r = (r*a + d.r*k) / oa
		d.g = (g*a + d.g*k) / oa
		d.b = (b*a + d.b*k) / oa
		d.fg = (fg*a + d.fg*k) / oa
		d.a = oa
	}
}
