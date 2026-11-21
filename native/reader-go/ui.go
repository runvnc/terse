package main

import (
	"fmt"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"syscall"

	"tersereader/internal/article"
	"tersereader/internal/nodeapi"
	"tersereader/internal/render"
	"tersereader/internal/terse"
)

// The interactive reader: a stack of screens (the timeline, an article,
// a feed, the feed list, help) drawn with ANSI escapes in raw mode.

type screenKind int

const (
	scTimeline screenKind = iota
	scArticle
	scFeed
	scFeeds
	scHelp
)

type item struct {
	text  string
	note  string
	entry *nodeapi.Entry
	addr  string // a feed address
}

type screen struct {
	kind  screenKind
	head  []string // fixed lines above a list
	items []item
	sel   int
	top   int

	art  *loaded
	page *render.Page
	link int // selected link, 1-based; 0: none
	left int
	num  string // digits typed to follow a link by number

	addr string // feed screen: the feed's address
	key  string // feed screen: its key
}

type prompt struct {
	label string
	buf   []rune
	done  func(string)
	yesNo bool
}

type asyncResult struct {
	art *loaded
	err error
}

type ui struct {
	cfg    *config
	n      nodeapi.Node
	stack  []*screen
	status string
	prompt *prompt
	w, h   int
	async  chan asyncResult
	quit   bool
}

func runInteractive(cfg *config, n nodeapi.Node) error {
	if !isTerminal(0) || !isTerminal(1) {
		return fmt.Errorf("the interactive reader needs a terminal; see --help for the commands")
	}
	u := &ui{cfg: cfg, n: n, async: make(chan asyncResult, 4)}
	if err := n.Connect(); err != nil {
		return err
	}
	state, err := makeRaw(0)
	if err != nil {
		return err
	}
	os.Stdout.WriteString("\x1b[?1049h\x1b[?25l")
	defer func() {
		os.Stdout.WriteString("\x1b[0m\x1b[?25h\x1b[?1049l")
		restore(0, state)
	}()

	keys := make(chan []string, 16)
	go func() {
		buf := make([]byte, 256)
		for {
			k, err := os.Stdin.Read(buf)
			if err != nil || k == 0 {
				close(keys)
				return
			}
			keys <- parseKeys(buf[:k])
		}
	}()
	winch := make(chan os.Signal, 1)
	signal.Notify(winch, syscall.SIGWINCH)
	defer signal.Stop(winch)
	term := make(chan os.Signal, 1)
	signal.Notify(term, syscall.SIGTERM, syscall.SIGHUP)
	defer signal.Stop(term)

	u.w, u.h = termSize(1)
	u.stack = []*screen{u.timeline()}
	u.status = "? for help"
	for !u.quit {
		u.draw()
		select {
		case ks, ok := <-keys:
			if !ok {
				return nil
			}
			for _, k := range ks {
				u.key(k)
			}
		case <-n.Changed():
			u.refresh()
		case m := <-n.Log():
			u.status = m
		case r := <-u.async:
			if r.err != nil {
				u.status = r.err.Error()
			} else {
				u.openLoaded(r.art)
			}
		case <-winch:
			u.w, u.h = termSize(1)
			for _, s := range u.stack {
				if s.kind == scArticle {
					s.page = render.Render(s.art.doc, u.renderOpts())
				}
			}
		case <-term:
			return nil
		}
	}
	return nil
}

var keySeqs = []struct{ s, name string }{
	{"\x1b[A", "up"}, {"\x1b[B", "down"}, {"\x1b[C", "right"}, {"\x1b[D", "left"},
	{"\x1bOA", "up"}, {"\x1bOB", "down"}, {"\x1bOC", "right"}, {"\x1bOD", "left"},
	{"\x1b[5~", "pgup"}, {"\x1b[6~", "pgdn"}, {"\x1b[H", "home"}, {"\x1b[F", "end"},
	{"\x1b[1~", "home"}, {"\x1b[4~", "end"}, {"\x1b[Z", "btab"},
}

// parseKeys turns terminal input into key names.
func parseKeys(b []byte) []string {
	var out []string
	for len(b) > 0 {
		if b[0] == 0x1b {
			matched := false
			for _, q := range keySeqs {
				if strings.HasPrefix(string(b), q.s) {
					out = append(out, q.name)
					b = b[len(q.s):]
					matched = true
					break
				}
			}
			if !matched {
				out = append(out, "esc")
				b = b[1:]
			}
			continue
		}
		switch b[0] {
		case '\r', '\n':
			out = append(out, "enter")
		case '\t':
			out = append(out, "tab")
		case 0x7f, 0x08:
			out = append(out, "bs")
		case 0x03:
			out = append(out, "ctrl-c")
		default:
			r := []rune(string(b))
			if len(r) > 0 {
				out = append(out, string(r[0]))
				b = b[len(string(r[0])):]
				continue
			}
		}
		b = b[1:]
	}
	return out
}

func (u *ui) renderOpts() render.Options {
	col := u.w - 2
	if col > 80 {
		col = 80
	}
	return render.Options{Width: col, LinkNumbers: u.cfg.linkNumbers, Color: true}
}

func (u *ui) cur() *screen { return u.stack[len(u.stack)-1] }

func (u *ui) push(s *screen) { u.stack = append(u.stack, s) }

func (u *ui) pop() {
	if len(u.stack) > 1 {
		u.stack = u.stack[:len(u.stack)-1]
		u.refresh()
	}
}

// --- screens ---

func (u *ui) timeline() *screen {
	s := &screen{kind: scTimeline}
	u.fillTimeline(s)
	return s
}

func (u *ui) fillTimeline(s *screen) {
	s.items = nil
	for _, e := range u.n.Timeline() {
		e := e
		it := item{entry: &e, text: shownTitle(e.Title)}
		if it.text == "" {
			it.text = "(no title)"
		}
		note := e.Label + " · " + when(e.Time)
		if e.Kind == nodeapi.KindDocument {
			note = "document, cannot open · " + note
		} else if !e.Held {
			note = fmt.Sprintf("%s, fetched when opened · %s", size(e.Size), note)
		}
		it.note = note
		s.items = append(s.items, it)
	}
	s.head = nil
	if len(u.n.Subscriptions()) == 0 {
		s.head = []string{"You follow no feed yet: press s and give a feed address."}
	} else if len(s.items) == 0 {
		s.head = []string{"Nothing in the timeline yet."}
	}
	if s.sel >= len(s.items) {
		s.sel = max(0, len(s.items)-1)
	}
}

func size(n uint32) string {
	if n >= 1024 {
		return fmt.Sprintf("%d KiB", (n+1023)/1024)
	}
	return fmt.Sprintf("%d bytes", n)
}

func (u *ui) feedScreen(addr string) *screen {
	s := &screen{kind: scFeed, addr: addr}
	u.fillFeed(s)
	return s
}

func (u *ui) fillFeed(s *screen) {
	f, err := u.n.Feed(s.addr)
	if err != nil {
		s.head = []string{err.Error()}
		return
	}
	s.key = f.Key
	s.head = feedHead(f)
	if f.Subscribed {
		s.head = append(s.head, "u: unsubscribe   n: nickname   Enter: open")
	} else {
		s.head = append(s.head, "s: subscribe   Enter: open")
	}
	s.items = nil
	for i := len(f.Entries) - 1; i >= 0; i-- {
		e := f.Entries[i]
		s.items = append(s.items, item{entry: &e, text: entryText(e), note: fmt.Sprintf("seq %d · %s", e.Seq, when(e.Time))})
	}
	if s.sel >= len(s.items) {
		s.sel = max(0, len(s.items)-1)
	}
}

func (u *ui) feedsScreen() *screen {
	s := &screen{kind: scFeeds}
	u.fillFeeds(s)
	return s
}

func (u *ui) fillFeeds(s *screen) {
	s.items = nil
	s.head = []string{"Feeds you follow (Enter: the feed, s: subscribe to another)"}
	for _, f := range u.n.Subscriptions() {
		s.items = append(s.items, item{text: f.Label, note: f.Address, addr: f.Address})
	}
	if s.sel >= len(s.items) {
		s.sel = max(0, len(s.items)-1)
	}
}

func (u *ui) refresh() {
	for _, s := range u.stack {
		switch s.kind {
		case scTimeline:
			u.fillTimeline(s)
		case scFeed:
			u.fillFeed(s)
		case scFeeds:
			u.fillFeeds(s)
		}
	}
}

// --- opening things ---

func (u *ui) openEntry(e *nodeapi.Entry) {
	switch e.Kind {
	case nodeapi.KindArticle:
	case nodeapi.KindProfile:
		u.push(u.feedScreen(e.Feed))
		return
	case nodeapi.KindProgram:
		u.status = "a program: this reader does not run programs"
		return
	default:
		u.status = "this reader cannot open " + strings.Trim(entryText(*e), "[]")
		return
	}
	if b, ok := u.n.HeldBlob(e.Blob); ok && uint32(len(b)) == e.Size {
		u.openLoaded(prepare(u.n, e.Blob, b))
		return
	}
	u.fetch(e.Blob)
}

func (u *ui) fetch(id string) {
	u.status = "fetching terse:" + terse.ShortKey(id) + " …"
	go func() {
		b, err := u.n.Blob(id)
		if err != nil {
			u.async <- asyncResult{nil, fmt.Errorf("terse:%s: %v", terse.ShortKey(id), err)}
			return
		}
		u.async <- asyncResult{prepare(u.n, id, b), nil}
	}()
}

func (u *ui) openLoaded(l *loaded) {
	if l.doc.NotArticle != "" {
		what := "a program image"
		if l.doc.NotArticle == "document" {
			what = "a document"
		}
		u.status = "that blob is " + what + ": this reader cannot open it"
		return
	}
	s := &screen{kind: scArticle, art: l}
	s.page = render.Render(l.doc, u.renderOpts())
	u.push(s)
	u.status = fmt.Sprintf("%d links · Tab selects, Enter follows, Esc goes back", len(s.page.Links))
}

// feedAddress is a feed link's address: the key and its hints.
func feedAddress(l *article.Link) string {
	if l.Hints != "" {
		return l.ID + "@" + l.Hints
	}
	return l.ID
}

func (u *ui) follow(l *article.Link) {
	switch l.Kind {
	case article.LinkArticle:
		if b, ok := u.n.HeldBlob(l.ID); ok {
			u.openLoaded(prepare(u.n, l.ID, b))
			return
		}
		u.fetch(l.ID)
	case article.LinkFeed:
		// Showing the feed never connects to its hints; subscribing may.
		u.push(u.feedScreen(feedAddress(l)))
	case article.LinkProgram:
		u.status = fmt.Sprintf("program %q of %s: this reader does not run programs", l.Name, terse.ShortKey(l.ID))
	}
}

func (u *ui) askSubscribe(addr string) {
	f, err := u.n.Feed(addr)
	if err != nil {
		u.status = err.Error()
		return
	}
	do := func() {
		if _, err := u.n.Subscribe(addr); err != nil {
			u.status = err.Error()
			return
		}
		u.status = "subscribed to " + u.n.Label(f.Key)
		u.refresh()
	}
	if len(f.Hints) > 0 {
		u.prompt = &prompt{
			label: fmt.Sprintf("If your relays hold nothing of it, this connects to %s. Subscribe? (y/n) ",
				strings.Join(f.Hints, " and ")),
			yesNo: true,
			done: func(s string) {
				if s == "y" {
					do()
				}
			},
		}
		return
	}
	do()
}

func (u *ui) askUnsubscribe(key string) {
	u.prompt = &prompt{
		label: "Unsubscribe from " + u.n.Label(key) + "? (y/n) ",
		yesNo: true,
		done: func(s string) {
			if s == "y" {
				if err := u.n.Unsubscribe(key); err != nil {
					u.status = err.Error()
				} else {
					u.status = "unsubscribed"
				}
				u.refresh()
			}
		},
	}
}

func (u *ui) askAddress() {
	u.prompt = &prompt{
		label: "Feed address: ",
		done: func(s string) {
			if strings.TrimSpace(s) != "" {
				u.askSubscribe(s)
			}
		},
	}
}

// --- keys ---

func (u *ui) key(k string) {
	if p := u.prompt; p != nil {
		switch {
		case p.yesNo:
			u.prompt = nil
			if k == "y" || k == "Y" {
				p.done("y")
			} else {
				p.done("n")
			}
		case k == "enter":
			u.prompt = nil
			p.done(string(p.buf))
		case k == "esc" || k == "ctrl-c":
			u.prompt = nil
		case k == "bs":
			if len(p.buf) > 0 {
				p.buf = p.buf[:len(p.buf)-1]
			}
		case len([]rune(k)) == 1:
			p.buf = append(p.buf, []rune(k)...)
		}
		return
	}
	s := u.cur()
	switch k {
	case "q", "ctrl-c":
		if k == "ctrl-c" || len(u.stack) == 1 {
			u.quit = true
			return
		}
		u.pop()
		return
	case "esc", "bs", "h":
		if s.kind == scArticle && s.num != "" {
			s.num = ""
			return
		}
		u.pop()
		return
	case "?":
		u.push(&screen{kind: scHelp, head: strings.Split(strings.TrimRight(usage, "\n"), "\n")})
		return
	case "f":
		if s.kind != scFeeds {
			u.push(u.feedsScreen())
		}
		return
	}
	if s.kind == scArticle {
		u.articleKey(s, k)
		return
	}
	rows := max(1, u.h-2-len(s.head))
	switch k {
	case "up", "k":
		if s.sel > 0 {
			s.sel--
		}
	case "down", "j":
		if s.sel < len(s.items)-1 {
			s.sel++
		}
	case "pgup":
		s.sel = max(0, s.sel-rows)
	case "pgdn", " ":
		s.sel = min(max(0, len(s.items)-1), s.sel+rows)
	case "home", "g":
		s.sel = 0
	case "end", "G":
		s.sel = max(0, len(s.items)-1)
	case "enter", "right", "l":
		if s.sel < len(s.items) {
			it := s.items[s.sel]
			switch {
			case it.addr != "":
				u.push(u.feedScreen(it.addr))
			case it.entry != nil:
				u.openEntry(it.entry)
			}
		}
	case "s":
		if s.kind == scFeed {
			u.askSubscribe(s.addr)
		} else {
			u.askAddress()
		}
	case "u":
		switch {
		case s.kind == scFeed:
			u.askUnsubscribe(s.key)
		case s.sel < len(s.items) && s.items[s.sel].entry != nil:
			u.askUnsubscribe(s.items[s.sel].entry.Feed)
		case s.sel < len(s.items) && s.items[s.sel].addr != "":
			u.askUnsubscribe(s.items[s.sel].addr)
		}
	case "F":
		if s.sel < len(s.items) && s.items[s.sel].entry != nil {
			u.push(u.feedScreen(s.items[s.sel].entry.Feed))
		}
	case "n":
		if s.kind == scFeed {
			key := s.key
			u.prompt = &prompt{label: "Nickname (empty: none): ", done: func(name string) {
				if err := u.n.SetNickname(key, strings.TrimSpace(name)); err != nil {
					u.status = err.Error()
				}
				u.refresh()
			}}
		}
	}
}

func linkLine(p *render.Page, n int) int {
	for i, l := range p.Lines {
		for _, sp := range l {
			if sp.Link == n {
				return i
			}
		}
	}
	return -1
}

func (u *ui) articleKey(s *screen, k string) {
	rows := u.h - 2
	maxTop := max(0, len(s.page.Lines)-rows)
	selectLink := func(n int) {
		if n < 1 || n > len(s.page.Links) {
			return
		}
		s.link = n
		if i := linkLine(s.page, n); i >= 0 && (i < s.top || i >= s.top+rows) {
			s.top = min(maxTop, max(0, i-rows/3))
		}
		u.status = fmt.Sprintf("[%d] %s%s", n, s.page.Links[n-1].Written, linkNote(s.page.Links[n-1]))
	}
	switch k {
	case "up", "k":
		s.top = max(0, s.top-1)
	case "down", "j":
		s.top = min(maxTop, s.top+1)
	case "pgup", "b":
		s.top = max(0, s.top-rows+1)
	case "pgdn", " ":
		s.top = min(maxTop, s.top+rows-1)
	case "home", "g":
		s.top = 0
	case "end", "G":
		s.top = maxTop
	case "left":
		s.left = max(0, s.left-8)
	case "right":
		s.left += 8
	case "tab":
		n := s.link + 1
		if n > len(s.page.Links) {
			n = 1
		}
		selectLink(n)
	case "btab":
		n := s.link - 1
		if n < 1 {
			n = len(s.page.Links)
		}
		selectLink(n)
	case "enter":
		if s.num != "" {
			n, _ := strconv.Atoi(s.num)
			s.num = ""
			if n < 1 || n > len(s.page.Links) {
				u.status = fmt.Sprintf("no link %d", n)
				return
			}
			selectLink(n)
		}
		if s.link >= 1 && s.link <= len(s.page.Links) {
			u.follow(s.page.Links[s.link-1])
		}
	case "s":
		if s.link >= 1 && s.page.Links[s.link-1].Kind == article.LinkFeed {
			u.askSubscribe(feedAddress(s.page.Links[s.link-1]))
		}
	default:
		if len(k) == 1 && k[0] >= '0' && k[0] <= '9' {
			s.num += k
			u.status = "follow link " + s.num + " (Enter)"
		}
	}
}

// --- drawing ---

func cutLine(l render.Line, left, width int) render.Line {
	var out render.Line
	col := 0
	for _, sp := range l {
		var b strings.Builder
		for _, r := range sp.Text {
			w := render.RuneWidth(r)
			if col >= left && col+w <= left+width {
				b.WriteRune(r)
			}
			col += w
		}
		if b.Len() > 0 {
			sp.Text = b.String()
			out = append(out, sp)
		}
	}
	return out
}

func clip(s string, w int) string {
	out := strings.Builder{}
	col := 0
	for _, r := range s {
		rw := render.RuneWidth(r)
		if col+rw > w {
			break
		}
		out.WriteRune(r)
		col += rw
	}
	return out.String()
}

func row(b *strings.Builder, y int, content string) {
	fmt.Fprintf(b, "\x1b[%d;1H%s\x1b[0m\x1b[K", y, content)
}

func (u *ui) draw() {
	var b strings.Builder
	s := u.cur()
	title := "Tersenet"
	switch s.kind {
	case scTimeline:
		con, total := 0, 0
		for _, r := range u.n.Relays() {
			total++
			if r.Connected {
				con++
			}
		}
		title = fmt.Sprintf("Tersenet — timeline · %d feeds · relays %d/%d connected", len(u.n.Subscriptions()), con, total)
	case scArticle:
		title = s.art.title
	case scFeed:
		title = "Feed " + u.n.Label(s.key)
	case scFeeds:
		title = "Feeds"
	case scHelp:
		title = "Help (Esc to go back)"
	}
	t := clip(title, u.w-2)
	row(&b, 1, "\x1b[7m "+t+strings.Repeat(" ", max(0, u.w-1-render.StringWidth(t)))+"\x1b[0m")
	rows := u.h - 2
	if s.kind == scArticle {
		maxTop := max(0, len(s.page.Lines)-rows)
		if s.top > maxTop {
			s.top = maxTop
		}
		for i := 0; i < rows; i++ {
			line := ""
			if n := s.top + i; n < len(s.page.Lines) {
				line = " " + render.LineText(cutLine(s.page.Lines[n], s.left, u.w-2), true, s.link)
			}
			row(&b, i+2, line)
		}
	} else {
		y := 2
		for _, h := range s.head {
			if y > u.h-1 {
				break
			}
			row(&b, y, " \x1b[2m"+clip(h, u.w-2))
			y++
		}
		listRows := max(1, u.h-y)
		if s.sel < s.top {
			s.top = s.sel
		}
		if s.sel >= s.top+listRows {
			s.top = s.sel - listRows + 1
		}
		for i := 0; y <= u.h-1; i++ {
			n := s.top + i
			line := ""
			if n < len(s.items) {
				it := s.items[n]
				text := clip(it.text, u.w-3)
				rest := u.w - 3 - render.StringWidth(text)
				note := ""
				if rest > 5 && it.note != "" {
					note = "  \x1b[2m" + clip(it.note, rest-2)
				}
				if n == s.sel {
					line = "\x1b[7m " + text + " \x1b[0m" + note
				} else {
					line = " " + text + " " + note
				}
			}
			row(&b, y, line)
			y++
		}
	}
	if p := u.prompt; p != nil {
		row(&b, u.h, clip(p.label+string(p.buf), u.w-1)+"\x1b[?25h")
	} else {
		row(&b, u.h, "\x1b[2m"+clip(u.status, u.w-1)+"\x1b[?25l")
	}
	os.Stdout.WriteString(b.String())
}
