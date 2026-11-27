package render

// Colouring code: the lines of a fence whose info string names a language
// are coloured by a small tokenizer. This is a choice of this reader, not
// part of the article format (ARTICLE.md 2.4 ignores the info string); a
// reader that shows code in one colour is just as right. The rules and
// classes are those of the website (crates/terse-net/src/highlight.rs)
// and the reference reader (programs/reader/reader.c):
//
//   - The info string's first word: c or h, rust or rs, go, sh (shell,
//     bash, fish, zsh, console), article, drive or events; text, txt,
//     plain, output and path are not coloured; else a word that is a file
//     name gives the language by its extension; else any word gets the
//     generic colouring (comments, strings, numbers). No info string: not
//     coloured.
//   - Classes: comment, string, number, keyword (also a command line's
//     command word, a script's command, an article's heading), type, meta
//     (a C preprocessor word, a Rust attribute or macro, a shell variable).

import "strings"

// Lang is a language a fence is coloured as.
type Lang int

const (
	LangNone Lang = iota
	LangC
	LangRust
	LangGo
	LangShell
	LangArticle
	LangScript
	LangGeneric
)

// Class is what a run of a code line is.
type Class uint8

const (
	ClassPlain Class = iota
	ClassComment
	ClassString
	ClassNumber
	ClassKeyword
	ClassType
	ClassMeta
)

// Run is a coloured run of a line: bytes [Start, End).
type Run struct {
	Class      Class
	Start, End int
}

func byWord(w string) (Lang, bool) {
	switch w {
	case "c", "h":
		return LangC, true
	case "rust", "rs":
		return LangRust, true
	case "go":
		return LangGo, true
	case "sh", "shell", "bash", "fish", "zsh", "console":
		return LangShell, true
	case "article":
		return LangArticle, true
	case "drive", "events":
		return LangScript, true
	case "text", "txt", "plain", "output", "path":
		return LangNone, true
	}
	return LangNone, false
}

// LangOf is the language a fence's info string names.
func LangOf(info string) Lang {
	words := strings.Fields(info)
	if len(words) == 0 {
		return LangNone
	}
	if l, ok := byWord(strings.ToLower(words[0])); ok {
		return l
	}
	for _, w := range words {
		dot := strings.LastIndexByte(w, '.')
		if dot > 0 && w[dot-1] != '/' && !strings.Contains(w[dot:], "/") {
			if l, ok := byWord(strings.ToLower(w[dot+1:])); ok {
				return l
			}
		}
	}
	return LangGeneric
}

var (
	cKeywords  = words("auto break case const continue default do else enum extern for goto if inline register restrict return sizeof static struct switch typedef union volatile while true false NULL")
	cTypes     = words("char double float int long short signed unsigned void bool _Bool")
	rsKeywords = words("as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self static struct super trait true type unsafe use where while")
	rsTypes    = words("bool char str u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize f32 f64")
	goKeywords = words("break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var true false nil iota")
	goTypes    = words("bool byte complex64 complex128 error float32 float64 int int8 int16 int32 int64 rune string uint uint8 uint16 uint32 uint64 uintptr any")
	shKeywords = words("if then else elif fi for in do done while until case esac function end begin set export")
	shCommand  = words("if then else elif do while until begin")
)

func words(s string) map[string]bool {
	m := map[string]bool{}
	for _, w := range strings.Fields(s) {
		m[w] = true
	}
	return m
}

type hlState struct {
	kind int // 0 normal, 1 block comment, 2 string (quote q), 3 shell line continued, 4 here-document (word), 5 article fence
	q    byte
	word string
}

// Highlight returns the coloured runs of each line of a fence.
func Highlight(lang Lang, lines []string) [][]Run {
	var st hlState
	out := make([][]Run, len(lines))
	for i, l := range lines {
		o := &runs{}
		s := []byte(l)
		switch lang {
		case LangC, LangRust, LangGo:
			hlCode(lang, s, &st, o)
		case LangShell:
			hlShell(s, &st, o)
		case LangArticle:
			hlArticle(s, &st, o)
		case LangScript:
			hlScript(s, o)
		case LangGeneric:
			hlGeneric(s, o)
		}
		out[i] = o.r
	}
	return out
}

type runs struct{ r []Run }

func (o *runs) push(c Class, a, b int) {
	if c == ClassPlain || a >= b {
		return
	}
	if n := len(o.r); n > 0 && o.r[n-1].Class == c && o.r[n-1].End == a {
		o.r[n-1].End = b
		return
	}
	o.r = append(o.r, Run{c, a, b})
}

func isIdentStart(b byte) bool { return b >= 'a' && b <= 'z' || b >= 'A' && b <= 'Z' || b == '_' }
func isIdent(b byte) bool      { return isIdentStart(b) || b >= '0' && b <= '9' }
func isDigit(b byte) bool      { return b >= '0' && b <= '9' }
func isBlank(b byte) bool      { return b == ' ' || b == '\t' }

func at(s []byte, i int) byte {
	if i >= 0 && i < len(s) {
		return s[i]
	}
	return 0
}

// stringEnd is the index after the closing q of a string whose text starts
// at i, or -1 if the line ends first.
func stringEnd(s []byte, i int, q byte, raw bool) int {
	for i < len(s) {
		if s[i] == '\\' && !raw {
			i += 2
		} else if s[i] == q {
			return i + 1
		} else {
			i++
		}
	}
	return -1
}

func numberEnd(s []byte, i int) int {
	for i < len(s) && (isIdent(s[i]) || s[i] == '.' && isDigit(at(s, i+1))) {
		i++
	}
	return i
}

func find(s []byte, from int, what string) int {
	if from > len(s) {
		return -1
	}
	k := strings.Index(string(s[from:]), what)
	if k < 0 {
		return -1
	}
	return k + from
}

func hlCode(lang Lang, s []byte, st *hlState, o *runs) {
	kw, ty := goKeywords, goTypes
	switch lang {
	case LangC:
		kw, ty = cKeywords, cTypes
	case LangRust:
		kw, ty = rsKeywords, rsTypes
	}
	i := 0
	switch st.kind {
	case 1:
		e := find(s, 0, "*/")
		if e < 0 {
			o.push(ClassComment, 0, len(s))
			return
		}
		o.push(ClassComment, 0, e+2)
		i = e + 2
	case 2:
		e := stringEnd(s, 0, st.q, st.q == '`')
		if e < 0 {
			o.push(ClassString, 0, len(s))
			return
		}
		o.push(ClassString, 0, e)
		i = e
	}
	st.kind = 0
	lead := 0
	for lead < len(s) && (s[lead] == ' ' || s[lead] == '\t') {
		lead++
	}
	if lang == LangC && i == 0 && at(s, lead) == '#' {
		e := lead + 1
		for e < len(s) && (s[e] == ' ' || isIdent(s[e])) && !(s[e] == ' ' && e > lead+1 && isIdent(s[e-1])) {
			e++
		}
		o.push(ClassMeta, lead, e)
		i = e
		skip := e
		for skip < len(s) && s[skip] == ' ' {
			skip++
		}
		if at(s, skip) == '<' {
			if k := strings.IndexByte(string(s[e:]), '>'); k >= 0 {
				o.push(ClassString, skip, e+k+1)
				i = e + k + 1
			}
		}
	}
	for i < len(s) {
		b, next := s[i], at(s, i+1)
		switch {
		case b == '/' && next == '/':
			o.push(ClassComment, i, len(s))
			return
		case b == '/' && next == '*':
			e := find(s, i+2, "*/")
			if e < 0 {
				st.kind = 1
				o.push(ClassComment, i, len(s))
				return
			}
			o.push(ClassComment, i, e+2)
			i = e + 2
		case lang == LangRust && b == '#' && (next == '[' || next == '!' && at(s, i+2) == '['):
			e := find(s, i, "]")
			if e < 0 {
				e = len(s)
			} else {
				e++
			}
			o.push(ClassMeta, i, e)
			i = e
		case b == '"' || b == '`' && lang == LangGo:
			e := stringEnd(s, i+1, b, b == '`')
			if e < 0 {
				if lang != LangC {
					st.kind, st.q = 2, b
				}
				o.push(ClassString, i, len(s))
				return
			}
			o.push(ClassString, i, e)
			i = e
		case b == '\'':
			e := -1
			if next == '\\' {
				e = stringEnd(s, i+1, '\'', false)
			} else {
				e = charEnd(s, i+1)
			}
			switch {
			case e >= 0:
				o.push(ClassString, i, e)
				i = e
			case lang == LangRust:
				i++
			default:
				o.push(ClassString, i, len(s))
				return
			}
		case isDigit(b):
			e := numberEnd(s, i)
			o.push(ClassNumber, i, e)
			i = e
		case isIdentStart(b):
			e := i
			for e < len(s) && isIdent(s[e]) {
				e++
			}
			w := string(s[i:e])
			if lang == LangRust && (w == "r" || w == "b" || w == "br") && (at(s, e) == '"' || at(s, e) == '#') {
				h := 0
				for at(s, e+h) == '#' {
					h++
				}
				if at(s, e+h) == '"' {
					end := -1
					if w != "b" {
						if k := find(s, e+h+1, "\""+strings.Repeat("#", h)); k >= 0 {
							end = k + 1 + h
						}
					} else {
						end = stringEnd(s, e+1, '"', false)
					}
					if end < 0 {
						end = len(s)
					}
					o.push(ClassString, i, end)
					i = end
					continue
				}
			}
			c := ClassPlain
			switch {
			case kw[w]:
				c = ClassKeyword
			case ty[w] || lang == LangC && strings.HasSuffix(w, "_t") ||
				lang == LangRust && b >= 'A' && b <= 'Z' && strings.ContainsAny(w, "abcdefghijklmnopqrstuvwxyz"):
				c = ClassType
			case lang == LangRust && at(s, e) == '!' && at(s, e+1) != '=':
				e++
				c = ClassMeta
			}
			o.push(c, i, e)
			i = e
		default:
			i++
			for i < len(s) && s[i]&0xC0 == 0x80 {
				i++
			}
		}
	}
}

func charEnd(s []byte, i int) int {
	if i >= len(s) {
		return -1
	}
	c := s[i]
	n := 1
	switch {
	case c >= 0xF0:
		n = 4
	case c >= 0xE0:
		n = 3
	case c >= 0xC0:
		n = 2
	}
	if c != '\'' && at(s, i+n) == '\'' {
		return i + n + 1
	}
	return -1
}

func hlShell(s []byte, st *hlState, o *runs) {
	command := true
	switch st.kind {
	case 4:
		o.push(ClassString, 0, len(s))
		if strings.TrimSpace(string(s)) == st.word {
			st.kind = 0
		}
		return
	case 3:
		command = false
	}
	st.kind = 0
	i, wordStart, afterFor := 0, true, 0
	for i < len(s) {
		b := s[i]
		if isBlank(b) {
			wordStart = true
			i++
			continue
		}
		if b == '#' && wordStart {
			o.push(ClassComment, i, len(s))
			return
		}
		if b == '\\' && i+1 == len(s) {
			st.kind = 3
			return
		}
		if b == '|' || b == ';' || b == '&' || b == '(' {
			command = b != '&' || at(s, i+1) == '&' || at(s, i-1) == '&'
			if b == '&' && at(s, i-1) == '>' {
				command = false
			}
			wordStart = true
			i++
			continue
		}
		if b == '<' && at(s, i+1) == '<' && at(s, i+2) != '<' {
			k := i + 2
			if at(s, k) == '-' {
				k++
			}
			q := at(s, k)
			from := k
			if q == '\'' || q == '"' {
				from++
			} else {
				q = 0
			}
			e := from
			for e < len(s) && isIdent(s[e]) {
				e++
			}
			if e > from {
				st.kind, st.word = 4, string(s[from:e])
				end := e
				if q != 0 && at(s, e) == q {
					end++
				}
				o.push(ClassMeta, i, end)
				i, wordStart = end, false
				continue
			}
		}
		if b == '\'' || b == '"' {
			e := stringEnd(s, i+1, b, b == '\'')
			if e < 0 {
				e = len(s)
			}
			o.push(ClassString, i, e)
			i, wordStart, command = e, false, false
			continue
		}
		if b == '$' {
			var e int
			switch at(s, i+1) {
			case '{':
				e = find(s, i, "}")
				if e < 0 {
					e = len(s)
				} else {
					e++
				}
			case '(':
				o.push(ClassMeta, i, i+2)
				i += 2
				command, wordStart = true, true
				continue
			default:
				e = i + 1
				for e < len(s) && (isIdent(s[e]) || e == i+1 && strings.IndexByte("?#@*!$", s[e]) >= 0) {
					e++
				}
			}
			o.push(ClassMeta, i, e)
			i, wordStart = e, false
			continue
		}
		e := i
		for e < len(s) && strings.IndexByte(" \t|;&()<>'\"$", s[e]) < 0 {
			e++
		}
		if e == i {
			i++
			wordStart = false
			continue
		}
		w := string(s[i:e])
		if afterFor > 0 {
			afterFor++
			if afterFor == 3 && w == "in" {
				o.push(ClassKeyword, i, e)
			}
		}
		if command && wordStart {
			eq := strings.IndexByte(w, '=')
			assign := eq > 0
			for k := 0; k < eq && assign; k++ {
				assign = isIdent(w[k])
			}
			switch {
			case assign:
				o.push(ClassMeta, i, i+eq)
			case shKeywords[w]:
				o.push(ClassKeyword, i, e)
				command = shCommand[w]
				if w == "for" {
					afterFor = 1
				} else {
					afterFor = 0
				}
			default:
				o.push(ClassKeyword, i, e)
				command = false
			}
		}
		i, wordStart = e, false
	}
}

func hlArticle(s []byte, st *hlState, o *runs) {
	line := string(s)
	fence := strings.HasPrefix(line, "```")
	if st.kind == 5 {
		if fence && strings.Trim(line[3:], " \t") == "" {
			st.kind = 0
			o.push(ClassMeta, 0, len(s))
			return
		}
		o.push(ClassString, 0, len(s))
		return
	}
	switch {
	case fence:
		st.kind = 5
		o.push(ClassMeta, 0, len(s))
		return
	case strings.HasPrefix(line, "# ") || strings.HasPrefix(line, "## "):
		o.push(ClassKeyword, 0, len(s))
		return
	case strings.HasPrefix(line, "    ") || strings.HasPrefix(line, "\t"):
		o.push(ClassString, 0, len(s))
		return
	}
	i := 0
	if strings.HasPrefix(line, "- ") || strings.HasPrefix(line, "* ") {
		o.push(ClassMeta, 0, 1)
		i = 2
	}
	table := strings.HasPrefix(line, "|")
	for i < len(s) {
		if s[i] == '`' {
			if k := strings.IndexByte(line[i+1:], '`'); k > 0 {
				o.push(ClassString, i, i+k+2)
				i += k + 2
				continue
			}
		}
		if table && s[i] == '|' && at(s, i-1) != '\\' {
			o.push(ClassMeta, i, i+1)
		}
		i++
	}
}

func hlScript(s []byte, o *runs) {
	lead := 0
	for lead < len(s) && isBlank(s[lead]) {
		lead++
	}
	if at(s, lead) == '#' {
		o.push(ClassComment, lead, len(s))
		return
	}
	first := true
	for i := lead; i < len(s); {
		if isBlank(s[i]) {
			i++
			continue
		}
		e := i
		for e < len(s) && !isBlank(s[e]) {
			e++
		}
		w := s[i:e]
		number, digit := true, false
		for _, c := range w {
			digit = digit || isDigit(c)
			number = number && (isDigit(c) || c >= 'a' && c <= 'f' || c >= 'A' && c <= 'F' || c == 'x' || c == '-' || c == '.')
		}
		if number && digit {
			o.push(ClassNumber, i, e)
		} else if first {
			o.push(ClassKeyword, i, e)
			first = false
		}
		i = e
	}
}

func hlGeneric(s []byte, o *runs) {
	lead := 0
	for lead < len(s) && isBlank(s[lead]) {
		lead++
	}
	trimmed := strings.TrimRight(string(s), " \t\r")
	if at(s, lead) == '[' && strings.HasSuffix(trimmed, "]") {
		o.push(ClassKeyword, lead, len(trimmed))
		return
	}
	i, wordStart := 0, true
	for i < len(s) {
		b := s[i]
		if wordStart && (b == '#' || b == '/' && at(s, i+1) == '/') {
			o.push(ClassComment, i, len(s))
			return
		}
		if wordStart && (b == '"' || b == '\'') {
			if e := stringEnd(s, i+1, b, false); e >= 0 {
				o.push(ClassString, i, e)
				i, wordStart = e, false
				continue
			}
		}
		if wordStart && isDigit(b) {
			e := numberEnd(s, i)
			o.push(ClassNumber, i, e)
			i, wordStart = e, false
			continue
		}
		wordStart = !isIdent(b) && b != '.' && b < 0x80
		i++
	}
}

// classify gives the cells of a fence's line the class of the run each
// falls in (runs are byte ranges of the cells' text); links and marks
// keep their own style.
func classify(cs []cell, rs []Run) {
	off, k := 0, 0
	for i := range cs {
		for k < len(rs) && rs[k].End <= off {
			k++
		}
		if k < len(rs) && rs[k].Start <= off && cs[i].st == PreStyle {
			cs[i].cls = rs[k].Class
		}
		off += len(string(cs[i].r))
	}
}

// classCode is the ANSI style of each class: colours of the terminal's own
// palette (so they suit its light or dark theme); comments are dim and
// italic, so they differ from code without colour too.
var classCode = [...]string{
	ClassPlain:   "",
	ClassComment: "\x1b[2;3m",
	ClassString:  "\x1b[32m",
	ClassNumber:  "\x1b[33m",
	ClassKeyword: "\x1b[35m",
	ClassType:    "\x1b[36m",
	ClassMeta:    "\x1b[31m",
}
