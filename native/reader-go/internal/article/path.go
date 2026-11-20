package article

import (
	"errors"
	"fmt"
	"math/big"
)

// Colour is a figure colour (ARTICLE.md 6.1).
type Colour struct {
	None       bool // nothing drawn (`none`, or opacity 0)
	FG         bool // the reader's body text colour
	R, G, B, A uint8
}

func (c Colour) String() string {
	switch {
	case c.None:
		return "none"
	case c.FG:
		return "fg"
	}
	return fmt.Sprintf("#%02x%02x%02x%02x", c.R, c.G, c.B, c.A)
}

// Op is one drawing command, absolute, in sixteenths of a unit.
type Op struct {
	Cmd byte // 'M', 'L', 'Q', 'C' or 'Z'
	P   []int64
}

// Path is the subpaths between two words, with the fill and stroke in
// force (ARTICLE.md 6.3).
type Path struct {
	Fill, Stroke Colour
	Width        int64 // sixteenths of a unit
	Ops          []Op
}

// Draws reports whether the path draws anything: a segment, and a fill or
// a stroke.
func (p *Path) Draws() bool {
	seg := false
	for _, o := range p.Ops {
		if o.Cmd != 'M' {
			seg = true
		}
	}
	return seg && (!p.Fill.None || p.StrokeVisible())
}

// StrokeVisible reports whether the path is outlined.
func (p *Path) StrokeVisible() bool { return !p.Stroke.None && p.Width > 0 }

// Figure is a figure that is drawn.
type Figure struct {
	W, H  int
	Paths []Path // only the paths that draw something
}

var errMalformed = errors.New("malformed path string")

const maxCoord = 2047

func isSep(c byte) bool { return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == ',' }

func isNumStart(c byte) bool { return c >= '0' && c <= '9' || c == '+' || c == '-' || c == '.' }

type pathParser struct {
	s   []byte
	pos int

	fig  *Figure
	cur  Path
	x, y int64 // current point, sixteenths
	open bool
	sx   int64
	sy   int64
	// reflection state: the last control point of a C/S or a Q/T
	lastC, lastQ bool
	cx, cy       int64
}

// ParsePath parses a path string into a figure of W x H units.
func ParsePath(s []byte, w, h int) (*Figure, error) {
	p := &pathParser{s: s, fig: &Figure{W: w, H: h}}
	p.cur = Path{Fill: Colour{None: true}, Stroke: Colour{FG: true}, Width: 16}
	if err := p.run(); err != nil {
		return nil, err
	}
	p.endPath()
	return p.fig, nil
}

func (p *pathParser) skipSeps() bool {
	any := false
	for p.pos < len(p.s) && isSep(p.s[p.pos]) {
		p.pos++
		any = true
	}
	return any
}

// number reads a number at the current position (no separators skipped).
func (p *pathParser) number() (*big.Rat, error) {
	start := p.pos
	i := p.pos
	if i < len(p.s) && (p.s[i] == '+' || p.s[i] == '-') {
		i++
	}
	digits := 0
	for i < len(p.s) && p.s[i] >= '0' && p.s[i] <= '9' {
		i++
		digits++
	}
	if i < len(p.s) && p.s[i] == '.' {
		i++
		for i < len(p.s) && p.s[i] >= '0' && p.s[i] <= '9' {
			i++
			digits++
		}
	}
	if digits == 0 || i-start > 32 {
		return nil, errMalformed
	}
	txt := string(p.s[start:i])
	if txt[0] == '+' {
		txt = txt[1:]
	}
	if txt[len(txt)-1] == '.' {
		txt += "0"
	}
	if txt[0] == '.' {
		txt = "0" + txt
	} else if len(txt) > 1 && txt[0] == '-' && txt[1] == '.' {
		txt = "-0" + txt[1:]
	}
	r, ok := new(big.Rat).SetString(txt)
	if !ok {
		return nil, errMalformed
	}
	p.pos = i
	return r, nil
}

// arg reads the next number of a command, after optional separators.
func (p *pathParser) arg() (*big.Rat, error) {
	p.skipSeps()
	if p.pos >= len(p.s) || !isNumStart(p.s[p.pos]) {
		return nil, errMalformed
	}
	return p.number()
}

var sixteen = big.NewRat(16, 1)

// round16 rounds to the nearest sixteenth, halves away from zero, and
// returns sixteenths.
func round16(r *big.Rat) int64 {
	v := new(big.Rat).Mul(r, sixteen)
	neg := v.Sign() < 0
	v.Abs(v)
	v.Add(v, big.NewRat(1, 2))
	q := new(big.Int).Quo(v.Num(), v.Denom())
	n := q.Int64()
	if neg {
		n = -n
	}
	return n
}

var limit = big.NewRat(maxCoord, 1)

func inRange(r *big.Rat) bool {
	a := new(big.Rat).Abs(r)
	return a.Cmp(limit) <= 0
}

// coord resolves one coordinate as written: absolute, or relative to base
// (sixteenths). It checks the range before rounding.
func coord(r *big.Rat, rel bool, base int64) (int64, error) {
	if rel {
		sum := new(big.Rat).Add(big.NewRat(base, 16), r)
		if !inRange(sum) {
			return 0, errMalformed
		}
		return base + round16(r), nil
	}
	if !inRange(r) {
		return 0, errMalformed
	}
	return round16(r), nil
}

func (p *pathParser) endPath() {
	if p.cur.Draws() {
		p.fig.Paths = append(p.fig.Paths, p.cur)
	}
	p.cur = Path{Fill: p.cur.Fill, Stroke: p.cur.Stroke, Width: p.cur.Width}
	p.open = false
}

func (p *pathParser) emit(cmd byte, pts ...int64) {
	if cmd != 'M' && !p.open {
		p.cur.Ops = append(p.cur.Ops, Op{Cmd: 'M', P: []int64{p.x, p.y}})
		p.open = true
		p.sx, p.sy = p.x, p.y
	}
	p.cur.Ops = append(p.cur.Ops, Op{Cmd: cmd, P: pts})
}

func (p *pathParser) wordAt(w string) bool {
	if p.pos+len(w) > len(p.s) || string(p.s[p.pos:p.pos+len(w)]) != w {
		return false
	}
	e := p.pos + len(w)
	return e == len(p.s) || isSep(p.s[e])
}

func (p *pathParser) colour() (Colour, error) {
	if !p.skipSeps() || p.pos >= len(p.s) {
		return Colour{}, errMalformed
	}
	start := p.pos
	for p.pos < len(p.s) && !isSep(p.s[p.pos]) {
		p.pos++
	}
	return parseColour(string(p.s[start:p.pos]))
}

func parseColour(t string) (Colour, error) {
	switch t {
	case "none":
		return Colour{None: true}, nil
	case "fg":
		return Colour{FG: true}, nil
	}
	if len(t) < 2 || t[0] != '#' {
		return Colour{}, errMalformed
	}
	h := t[1:]
	for i := 0; i < len(h); i++ {
		if !isHex(h[i]) {
			return Colour{}, errMalformed
		}
	}
	nib := func(i int) uint8 { return unhex(h[i]) }
	var c Colour
	switch len(h) {
	case 3:
		c = Colour{R: nib(0) * 17, G: nib(1) * 17, B: nib(2) * 17, A: 255}
	case 6, 8:
		c = Colour{R: nib(0)<<4 | nib(1), G: nib(2)<<4 | nib(3), B: nib(4)<<4 | nib(5), A: 255}
		if len(h) == 8 {
			c.A = nib(6)<<4 | nib(7)
		}
	default:
		return Colour{}, errMalformed
	}
	if c.A == 0 {
		return Colour{None: true}, nil
	}
	return c, nil
}

var argCount = map[byte]int{'M': 2, 'L': 2, 'H': 1, 'V': 1, 'C': 6, 'S': 4, 'Q': 4, 'T': 2, 'Z': 0}

func upper(c byte) byte {
	if c >= 'a' && c <= 'z' {
		return c - 32
	}
	return c
}

func (p *pathParser) run() error {
	first := true
	afterSep := true
	for {
		if p.skipSeps() {
			afterSep = true
		}
		if p.pos >= len(p.s) {
			return nil
		}
		c := p.s[p.pos]
		if afterSep && (p.wordAt("fill") || p.wordAt("stroke")) {
			if err := p.word(); err != nil {
				return err
			}
			afterSep = false
			continue
		}
		if _, ok := argCount[upper(c)]; !ok {
			return errMalformed
		}
		if first && upper(c) != 'M' {
			return errMalformed
		}
		first = false
		p.pos++
		cmd := c
		for {
			if err := p.command(cmd); err != nil {
				return err
			}
			// More numbers repeat the command (after M, as L).
			save := p.pos
			p.skipSeps()
			if argCount[upper(cmd)] > 0 && p.pos < len(p.s) && isNumStart(p.s[p.pos]) {
				switch cmd {
				case 'M':
					cmd = 'L'
				case 'm':
					cmd = 'l'
				}
				continue
			}
			p.pos = save
			break
		}
		afterSep = false
	}
}

func (p *pathParser) word() error {
	isFill := p.wordAt("fill")
	if isFill {
		p.pos += 4
	} else {
		p.pos += 6
	}
	col, err := p.colour()
	if err != nil {
		return err
	}
	p.endPath()
	if isFill {
		p.cur.Fill = col
		return nil
	}
	p.cur.Stroke = col
	p.cur.Width = 16
	save := p.pos
	p.skipSeps()
	if p.pos < len(p.s) && isNumStart(p.s[p.pos]) {
		w, err := p.number()
		if err != nil {
			return err
		}
		if w.Sign() < 0 || w.Cmp(limit) > 0 {
			return errMalformed
		}
		p.cur.Width = round16(w)
		return nil
	}
	p.pos = save
	return nil
}

func (p *pathParser) command(c byte) error {
	n := argCount[upper(c)]
	rel := c >= 'a'
	args := make([]*big.Rat, n)
	for i := range args {
		a, err := p.arg()
		if err != nil {
			return err
		}
		args[i] = a
	}
	bx, by := p.x, p.y
	pt := func(i int) (int64, int64, error) {
		x, err := coord(args[i], rel, bx)
		if err != nil {
			return 0, 0, err
		}
		y, err := coord(args[i+1], rel, by)
		return x, y, err
	}
	reflect := func(use bool) (int64, int64, error) {
		if !use {
			return p.x, p.y, nil
		}
		rx, ry := 2*p.x-p.cx, 2*p.y-p.cy
		if abs(rx) > maxCoord*16 || abs(ry) > maxCoord*16 {
			return 0, 0, errMalformed
		}
		return rx, ry, nil
	}
	lastC, lastQ := false, false
	switch upper(c) {
	case 'M':
		x, y, err := pt(0)
		if err != nil {
			return err
		}
		p.cur.Ops = append(p.cur.Ops, Op{Cmd: 'M', P: []int64{x, y}})
		p.open, p.sx, p.sy = true, x, y
		p.x, p.y = x, y
	case 'L':
		x, y, err := pt(0)
		if err != nil {
			return err
		}
		p.emit('L', x, y)
		p.x, p.y = x, y
	case 'H':
		x, err := coord(args[0], rel, bx)
		if err != nil {
			return err
		}
		p.emit('L', x, p.y)
		p.x = x
	case 'V':
		y, err := coord(args[0], rel, by)
		if err != nil {
			return err
		}
		p.emit('L', p.x, y)
		p.y = y
	case 'C', 'S':
		var x1, y1 int64
		var err error
		k := 0
		if upper(c) == 'C' {
			if x1, y1, err = pt(0); err != nil {
				return err
			}
			k = 2
		} else if x1, y1, err = reflect(p.lastC); err != nil {
			return err
		}
		x2, y2, err := pt(k)
		if err != nil {
			return err
		}
		x, y, err := pt(k + 2)
		if err != nil {
			return err
		}
		p.emit('C', x1, y1, x2, y2, x, y)
		p.x, p.y, p.cx, p.cy = x, y, x2, y2
		lastC = true
	case 'Q', 'T':
		var x1, y1 int64
		var err error
		k := 0
		if upper(c) == 'Q' {
			if x1, y1, err = pt(0); err != nil {
				return err
			}
			k = 2
		} else if x1, y1, err = reflect(p.lastQ); err != nil {
			return err
		}
		x, y, err := pt(k)
		if err != nil {
			return err
		}
		p.emit('Q', x1, y1, x, y)
		p.x, p.y, p.cx, p.cy = x, y, x1, y1
		lastQ = true
	case 'Z':
		if p.open {
			p.cur.Ops = append(p.cur.Ops, Op{Cmd: 'Z'})
			p.open = false
			p.x, p.y = p.sx, p.sy
		}
	}
	p.lastC, p.lastQ = lastC, lastQ
	return nil
}

func abs(v int64) int64 {
	if v < 0 {
		return -v
	}
	return v
}
