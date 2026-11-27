// tersereader is a native Terse reader with no virtual machine: it
// follows feeds over the Terse network (NETWORK.md) and shows their
// articles (ARTICLE.md) in a terminal. It was written from the
// specifications alone.
//
// Package main is the reading half; it talks to the network half only
// through nodeapi.Node (see wire.go).
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"tersereader/internal/article"
	"tersereader/internal/local"
	"tersereader/internal/nodeapi"
	"tersereader/internal/render"
	"tersereader/internal/terse"
)

const usage = `tersereader: a native Terse reader (no virtual machine)

usage:
  tersereader [options]                  the interactive reader (a terminal)
  tersereader subscribe <address>        follow a feed: a key, maybe with @hints
  tersereader unsubscribe <key>
  tersereader sync [--timeout 20s]       fetch new entries and small articles, then exit
  tersereader list                       the timeline, newest first
  tersereader feeds                      the subscribed feeds
  tersereader feed <address>             what is held of one feed
  tersereader show <blob-id>             an article, fetched if needed, as text
  tersereader show <key> <seq>           the article of a feed's entry
  tersereader --dump <blob-id|file>      an article as text (from the store, the relays or a file)
  tersereader relay add|remove <wss://host | ws://host:port | host> | relay list
  tersereader nickname <key> [<name>]    name a feed for yourself (no name: clear it)
  tersereader node                       run only the network node, serving the local
                                         socket (LOCAL.md) until interrupted
  tersereader --version                  its name, version and build identifier

options:
  --dir <dir>          the reader's store (default $XDG_DATA_HOME/tersereader,
                       or ~/.local/share/tersereader; also $TERSEREADER_DIR)
  --relay <ws://...>   a relay for this run instead of the configured ones (repeatable)
  --width <n>          the text column for show and --dump (default: the terminal, at most 80)
  --plain              text for screen readers and pipes: no box drawing, figures as their text
  --tokens             --dump only: the parsed blocks in the test vectors' dump format
  --color              ANSI colours and styles in show and --dump output (the default
                       when the output is a terminal, unless --plain, NO_COLOR or TERM=dumb)
  --no-color           no ANSI colours or styles in show and --dump output
  --no-link-numbers    do not write [n] after links
  --auto-download <n>  articles up to n bytes are fetched as entries arrive (default 16384; 0: none)
  --no-prefetch        do not fetch the articles that small articles link to
  --no-feed-relays     do not follow feeds on the relays their profiles name, nor on hints
  --socket <path>      the local node's socket (default $TERSE_NODE_SOCKET, else
                       $XDG_RUNTIME_DIR/terse/node.sock)
  --node <mode>        auto (default): use a running node if its socket answers, else
                       run the node in this process; socket: only a running node;
                       in-process: never a running node

Interactive keys: arrows or j/k move, Enter opens, Tab/Shift-Tab select a link,
a number then Enter follows link n, Esc or Backspace goes back, s subscribes,
u unsubscribes, F shows an entry's feed, f lists feeds, q quits, ? helps.
`

type config struct {
	dir         string
	relays      []string
	width       int
	plain       bool
	tokens      bool
	color       bool
	noColor     bool
	linkNumbers bool
	timeout     time.Duration
	dump        string
	autoLimit   int
	prefetch    bool
	feedRelays  bool
	args        []string
	socket      string // the local node's socket (LOCAL.md)
	nodeMode    string // auto, socket or in-process
	version     bool   // --version
}

// The reader's version, and its build identifier: "s2:" and the first 8
// bytes of the SHA-256 hash of its own executable, in hex ("unknown" if it
// cannot be read). Self-reported, for accountability, not security.
const programVersion = "0.1.0"

func buildID() string {
	exe, err := os.Executable()
	if err != nil {
		return "unknown"
	}
	data, err := os.ReadFile(exe)
	if err != nil {
		return "unknown"
	}
	sum := sha256.Sum256(data)
	return "s2:" + hex.EncodeToString(sum[:8])
}

func versionLine() string {
	return "tersereader " + programVersion + " " + buildID()
}

func defaultDir() string {
	if d := os.Getenv("TERSEREADER_DIR"); d != "" {
		return d
	}
	if d := os.Getenv("XDG_DATA_HOME"); d != "" {
		return filepath.Join(d, "tersereader")
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".local", "share", "tersereader")
}

func parseArgs(args []string) (*config, error) {
	c := &config{dir: defaultDir(), linkNumbers: true, timeout: 20 * time.Second,
		autoLimit: 16384, prefetch: true, feedRelays: true, socket: local.DefaultPath(), nodeMode: "auto"}
	for i := 0; i < len(args); i++ {
		a := args[i]
		name, inline, hasInline := strings.Cut(a, "=")
		val := func() (string, error) {
			if hasInline {
				return inline, nil
			}
			if i+1 >= len(args) {
				return "", fmt.Errorf("%s needs a value", a)
			}
			i++
			return args[i], nil
		}
		var err error
		var v string
		switch name {
		case "--version":
			c.version = true
		case "-h", "--help", "help":
			fmt.Print(usage)
			os.Exit(0)
		case "--dir":
			c.dir, err = val()
		case "--relay":
			v, err = val()
			if err == nil && !strings.HasPrefix(v, "ws://") && !strings.HasPrefix(v, "wss://") {
				err = fmt.Errorf("a relay is a ws:// or wss:// URL: %s", v)
			}
			c.relays = append(c.relays, v)
		case "--width":
			v, err = val()
			c.width, _ = strconv.Atoi(v)
		case "--plain":
			c.plain = true
		case "--tokens":
			c.tokens = true
		case "--color", "--colour":
			c.color = true
		case "--no-color", "--no-colour":
			c.noColor = true
		case "--no-link-numbers":
			c.linkNumbers = false
		case "--timeout":
			if v, err = val(); err == nil {
				c.timeout, err = time.ParseDuration(v)
			}
		case "--dump":
			c.dump, err = val()
		case "--auto-download":
			v, err = val()
			c.autoLimit, _ = strconv.Atoi(v)
		case "--no-prefetch":
			c.prefetch = false
		case "--no-feed-relays":
			c.feedRelays = false
		case "--socket":
			c.socket, err = val()
		case "--node":
			c.nodeMode, err = val()
			if err == nil && c.nodeMode != "auto" && c.nodeMode != "socket" && c.nodeMode != "in-process" {
				err = fmt.Errorf("--node is auto, socket or in-process, not %q", c.nodeMode)
			}
		default:
			if strings.HasPrefix(a, "--") {
				return nil, fmt.Errorf("unknown option %s", a)
			}
			c.args = append(c.args, a)
		}
		if err != nil {
			return nil, err
		}
	}
	return c, nil
}

func main() {
	cfg, err := parseArgs(os.Args[1:])
	if err != nil {
		fatal(err)
	}
	if cfg.version {
		fmt.Println(versionLine())
		return
	}
	if err := run(cfg); err != nil {
		fatal(err)
	}
}

func fatal(err error) {
	fmt.Fprintln(os.Stderr, "tersereader:", err)
	os.Exit(1)
}

func run(cfg *config) error {
	// An article file can be dumped without any store.
	if cfg.dump != "" {
		if _, err := os.Stat(cfg.dump); err == nil {
			return dumpFile(cfg, cfg.dump)
		}
	}
	if len(cfg.args) > 0 && cfg.args[0] == "node" {
		return runNode(cfg)
	}
	n, err := openNode(cfg)
	if err != nil {
		return err
	}
	defer n.Close()
	if cfg.dump != "" {
		return showID(cfg, n, cfg.dump)
	}
	if len(cfg.args) == 0 {
		return runInteractive(cfg, n)
	}
	cmd, rest := cfg.args[0], cfg.args[1:]
	need := func(k int, u string) error {
		if len(rest) != k {
			return errors.New("usage: tersereader " + u)
		}
		return nil
	}
	switch cmd {
	case "subscribe":
		if err := need(1, "subscribe <address>"); err != nil {
			return err
		}
		return cmdSubscribe(n, rest[0])
	case "unsubscribe":
		if err := need(1, "unsubscribe <key>"); err != nil {
			return err
		}
		label := n.Label(rest[0])
		if err := n.Unsubscribe(rest[0]); err != nil {
			return err
		}
		fmt.Println("unsubscribed from", label)
		return nil
	case "sync":
		return cmdSync(cfg, n)
	case "list":
		for _, e := range n.Timeline() {
			mark := " "
			if e.Held {
				mark = "*"
			}
			title := shownTitle(e.Title)
			if e.Kind == nodeapi.KindDocument {
				title = "[document, cannot open] " + title
			}
			if e.FullText {
				title += "  [needs full text]" // ARTICLE.md 1.1
			}
			fmt.Printf("%s %s %s  %s  %s\n", mark, e.Blob, when(e.Time), e.Label, title)
		}
		return nil
	case "feeds":
		for _, f := range n.Subscriptions() {
			fmt.Printf("%s  %s\n", f.Address, f.Label)
		}
		return nil
	case "feed":
		if err := need(1, "feed <address>"); err != nil {
			return err
		}
		f, err := n.Feed(rest[0])
		if err != nil {
			return err
		}
		fmt.Print(strings.Join(feedHead(f), "\n") + "\n")
		for _, e := range f.Entries {
			fmt.Printf("  seq %d  %s  %s\n", e.Seq, when(e.Time), entryText(e))
		}
		return nil
	case "show":
		return cmdShow(cfg, n, rest)
	case "relay":
		return cmdRelay(n, rest)
	case "nickname":
		if len(rest) < 1 || len(rest) > 2 {
			return errors.New("usage: tersereader nickname <key> [<name>]")
		}
		name := ""
		if len(rest) == 2 {
			name = rest[1]
		}
		return n.SetNickname(rest[0], name)
	}
	return fmt.Errorf("unknown command %q (tersereader --help)", cmd)
}

// ansi tells whether show and --dump write ANSI colours and styles:
// --color, --no-color, else when the output is a terminal that takes
// them (not TERM=dumb) and neither --plain nor NO_COLOR (no-color.org) is
// set.
func (cfg *config) ansi() bool {
	switch {
	case cfg.noColor:
		return false
	case cfg.color:
		return true
	}
	return !cfg.plain && colorTerminal() && isTerminal(1)
}

// colorTerminal tells whether the environment allows colours: NO_COLOR
// is not set (or empty) and TERM is not dumb.
func colorTerminal() bool {
	return os.Getenv("NO_COLOR") == "" && os.Getenv("TERM") != "dumb"
}

func (cfg *config) column() int {
	if cfg.width > 0 {
		return cfg.width
	}
	if isTerminal(1) {
		if w, _ := termSize(1); w-1 < 80 {
			return w - 1
		}
	}
	return 80
}

func cmdSubscribe(n nodeapi.Node, address string) error {
	isNew, err := n.Subscribe(address)
	if err != nil {
		return err
	}
	f, _ := n.Feed(address)
	if isNew {
		fmt.Println("subscribed to", f.Address)
	} else {
		fmt.Println("already subscribed; address updated:", f.Address)
	}
	if len(f.Hints) > 0 {
		fmt.Printf("if your relays hold nothing of it, the reader will connect to %s\n", strings.Join(f.Hints, " and "))
	}
	return nil
}

func cmdSync(cfg *config, n nodeapi.Node) error {
	before := len(n.Timeline())
	idle, err := n.Sync(cfg.timeout)
	if err != nil {
		return err
	}
	after := len(n.Timeline())
	subs := n.Subscriptions()
	fmt.Printf("%d new in the timeline (%d in all), %d feeds\n", after-before, after, len(subs))
	for _, f := range subs {
		if len(f.HintRelays) > 0 {
			fmt.Printf("%s is followed through hinted relays: %s\n", f.Label, strings.Join(f.HintRelays, ", "))
		}
	}
	if !idle {
		return fmt.Errorf("not everything arrived within %s", cfg.timeout)
	}
	return nil
}

func cmdRelay(n nodeapi.Node, args []string) error {
	if len(args) == 0 || args[0] == "list" {
		for _, u := range n.ConfiguredRelays() {
			fmt.Println(u)
		}
		return nil
	}
	if len(args) != 2 || (args[0] != "add" && args[0] != "remove") {
		return errors.New("usage: tersereader relay add|remove <wss://host | ws://host:port | host>")
	}
	url := typedRelay(args[1])
	if url == "" {
		return fmt.Errorf("not a relay: %s (a ws:// or wss:// URL, or a host name)", args[1])
	}
	if url != args[1] {
		fmt.Fprintf(os.Stderr, "tersereader: taken as %s (over TLS; for a plain relay give ws://%s)\n", url, args[1])
	}
	var out []string
	for _, x := range n.ConfiguredRelays() {
		if x != url {
			out = append(out, x)
		}
	}
	if args[0] == "add" {
		out = append(out, url)
	}
	return n.SetRelays(out)
}

// typedRelay is a relay as a person types it: a ws:// or wss:// URL as
// it is, or a host and maybe a port without a scheme, taken as wss://
// (over TLS, as public relays are reached). Empty if it is neither.
func typedRelay(s string) string {
	s = strings.TrimSpace(s)
	if strings.HasPrefix(s, "ws://") || strings.HasPrefix(s, "wss://") {
		return s
	}
	if s == "" || strings.Contains(s, "://") || strings.ContainsAny(s, "/ \t") {
		return ""
	}
	return "wss://" + s
}

func when(t uint32) string { return time.Unix(int64(t), 0).UTC().Format("2006-01-02 15:04") }

// feedHead describes a feed: its name with its key, its hints and relays.
func feedHead(f nodeapi.Feed) []string {
	out := []string{"Feed " + f.Label, "key " + f.Key}
	if len(f.Hints) > 0 {
		out = append(out, "relay hints: "+strings.Join(f.Hints, ", "))
	}
	if len(f.HintRelays) > 0 {
		out = append(out, "followed through hinted relays: "+strings.Join(f.HintRelays, ", "))
	}
	if len(f.ProfileRelays) > 0 {
		out = append(out, "publishes on: "+strings.Join(f.ProfileRelays, ", "))
	}
	if d := strings.TrimSpace(article.Clean([]byte(f.Description))); d != "" {
		out = append(out, "", d, "")
	}
	if f.Subscribed {
		out = append(out, "subscribed")
	} else {
		out = append(out, "not subscribed")
	}
	if len(f.Entries) == 0 {
		out = append(out, "nothing of this feed is held")
	}
	return out
}

// entryText shows an entry in a list; kinds the reader does not open say so.
func entryText(e nodeapi.Entry) string {
	t := shownTitle(e.Title)
	switch e.Kind {
	case nodeapi.KindArticle:
		return t
	case nodeapi.KindProfile:
		return "profile: " + t
	case nodeapi.KindProgram:
		return "[program " + t + ": this reader does not run programs]"
	case nodeapi.KindDocument:
		return "[document " + t + ": cannot open]"
	}
	return fmt.Sprintf("[kind %d: %s, cannot open]", e.Kind, t)
}

// shownTitle is a title as ARTICLE.md 9 says to show it: the text rules
// of section 1 (control characters not shown, tabs and lone CRs as
// spaces), on one line.
func shownTitle(t string) string {
	return strings.ReplaceAll(article.Clean([]byte(t)), "\n", " ")
}

// loaded is an article ready to show.
type loaded struct {
	id    string
	title string
	doc   *article.Doc // the blocks shown (a heading taken as the title left out)
	raw   []byte
}

// prepare parses an article. The title is that of an entry held for the
// blob, else the article's first heading (then not shown again), else a
// short form of the ID (ARTICLE.md 4.4, 9).
func prepare(n nodeapi.Node, id string, b []byte) *loaded {
	l := &loaded{id: id, doc: article.Parse(b), raw: b}
	if n != nil {
		l.title, _ = n.ArticleTitle(id, len(b))
	}
	if l.title == "" && len(l.doc.Blocks) > 0 && l.doc.Blocks[0].Kind == article.Heading {
		var t strings.Builder
		for _, p := range l.doc.Blocks[0].Inline {
			t.WriteString(p.Text)
		}
		if s := strings.TrimSpace(t.String()); s != "" {
			l.title = s
			l.doc.Blocks = l.doc.Blocks[1:]
		}
	}
	if l.title == "" && id != "" {
		l.title = "terse:" + terse.ShortKey(id)
	}
	return l
}

func (cfg *config) renderOptions() render.Options {
	ansi := cfg.ansi()
	return render.Options{Width: cfg.column(), Plain: cfg.plain, LinkNumbers: cfg.linkNumbers, Color: ansi, Highlight: ansi && !cfg.plain}
}

// writeArticle prints an article as text: its title, the page, and the
// links' targets by number.
func writeArticle(cfg *config, l *loaded) {
	if cfg.tokens {
		fmt.Print(article.Dump(article.Parse(l.raw)))
		return
	}
	page := render.Render(l.doc, cfg.renderOptions())
	fmt.Println(l.title)
	if !cfg.plain {
		fmt.Println(strings.Repeat("═", min(render.StringWidth(l.title), cfg.column())))
	}
	fmt.Println()
	fmt.Print(page.Text(cfg.ansi()))
	if cfg.linkNumbers && len(page.Links) > 0 {
		fmt.Println()
		fmt.Println("Links:")
		for i, lk := range page.Links {
			fmt.Printf("[%d] %s%s\n", i+1, lk.Written, linkNote(lk))
		}
	}
}

func linkNote(l *article.Link) string {
	switch l.Kind {
	case article.LinkFeed:
		return " (a feed)"
	case article.LinkProgram:
		return " (a program: this reader does not run programs)"
	}
	return ""
}

func cmdShow(cfg *config, n nodeapi.Node, args []string) error {
	switch len(args) {
	case 1:
		return showID(cfg, n, args[0])
	case 2:
		f, err := n.Feed(args[0])
		if err != nil {
			return err
		}
		for _, e := range f.Entries {
			if strconv.Itoa(int(e.Seq)) == args[1] {
				if e.Kind != nodeapi.KindArticle {
					return fmt.Errorf("seq %d is not an article: this reader cannot open it", e.Seq)
				}
				return showID(cfg, n, e.Blob)
			}
		}
		return fmt.Errorf("no entry %s of that feed is held", args[1])
	}
	return errors.New("usage: tersereader show <blob-id> | show <key> <seq>")
}

func showID(cfg *config, n nodeapi.Node, id string) error {
	id = strings.TrimPrefix(strings.TrimSpace(id), "terse:")
	if !terse.IsLowerHex64(id) {
		return errors.New("not a blob ID (64 lowercase hex digits) nor a file")
	}
	b, err := n.Blob(id)
	if err != nil {
		return err
	}
	writeArticle(cfg, prepare(n, id, b))
	return nil
}

func dumpFile(cfg *config, path string) error {
	b, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	l := prepare(nil, "", b)
	if l.title == "" {
		l.title = filepath.Base(path)
	}
	writeArticle(cfg, l)
	return nil
}
