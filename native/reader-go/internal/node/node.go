// Package node is the network half of tersereader: it implements
// nodeapi.Node over the local store and the relay client.
package node

import (
	"encoding/hex"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"tersereader/internal/article"
	"tersereader/internal/client"
	"tersereader/internal/nodeapi"
	"tersereader/internal/store"
	"tersereader/internal/terse"
	"tersereader/internal/ws"
)

// Node is a network node over a store directory.
type Node struct {
	st      *store.Store
	opt     client.Options
	mu      sync.Mutex
	cl      *client.Client
	changed chan struct{}
	logs    chan string
	stop    chan struct{}
	live    bool // Connect has been called: relays set later are connected at once
}

var _ nodeapi.Node = (*Node)(nil)

// Open opens the store in dir. opt.Relays, if not nil, replaces the
// configured relays for this run.
func Open(dir string, opt client.Options) (*Node, error) {
	st, err := store.Open(dir)
	if err != nil {
		return nil, err
	}
	n := &Node{st: st, opt: opt, changed: make(chan struct{}, 1), logs: make(chan string, 64), stop: make(chan struct{})}
	user := opt.Logf
	n.opt.Logf = func(f string, a ...any) {
		if user != nil {
			user(f, a...)
		}
		select {
		case n.logs <- fmt.Sprintf(f, a...):
		default:
		}
	}
	return n, nil
}

// relayLines is the user's relay list in use as written: relays and their
// options (n.mu held).
func (n *Node) relayLines() []string {
	if n.opt.Relays != nil {
		return n.opt.Relays
	}
	return n.st.Relays()
}

// relays is the user's relay list in use, as URLs (n.mu held).
func (n *Node) relays() []string {
	return lineURLs(n.relayLines())
}

// lineURLs are the relays of relay lines, as URLs.
func lineURLs(lines []string) []string {
	var out []string
	for _, l := range lines {
		r, _, ok := ws.SplitLine(l)
		if !ok {
			continue
		}
		if u := terse.TypedRelay(r); u != "" {
			out = append(out, u)
		}
	}
	return out
}

// applyTrust trusts what the user's settings say for wss:// relays: the
// reader directory's authorities, the options of the stored relay list and
// of --relay, and --relay-authority (n.mu held).
func (n *Node) applyTrust() error {
	lines := append(append([]string(nil), n.st.Relays()...), n.opt.Relays...)
	t, err := ws.LoadTrust(n.st.Dir(), lines, n.opt.Authorities, terse.TypedRelay)
	if err != nil {
		return err
	}
	ws.SetTrust(t)
	return nil
}

// Connect starts the client.
func (n *Node) Connect() error {
	n.mu.Lock()
	defer n.mu.Unlock()
	n.live = true
	if n.cl != nil {
		return nil
	}
	if len(n.relays()) == 0 {
		return errors.New("no relay: add one with `tersereader relay add <host>` (wss://, encrypted) or `... ws://host:port` (plain), or give --relay")
	}
	if err := n.applyTrust(); err != nil {
		return err
	}
	opt := n.opt
	opt.Relays = n.relays()
	n.cl = client.New(n.st, opt)
	n.cl.Start()
	cl := n.cl
	go func() {
		for {
			select {
			case <-n.stop:
				return
			case <-cl.Changed():
				select {
				case n.changed <- struct{}{}:
				default:
				}
			}
		}
	}()
	return nil
}

func (n *Node) client() *client.Client {
	n.mu.Lock()
	defer n.mu.Unlock()
	return n.cl
}

func (n *Node) Sync(timeout time.Duration) (bool, error) {
	if err := n.Connect(); err != nil {
		return false, err
	}
	n.mu.Lock()
	none := len(n.relays()) == 0
	n.mu.Unlock()
	if none {
		return false, errors.New("no relay")
	}
	return n.client().WaitIdle(timeout), nil
}

func (n *Node) Changed() <-chan struct{} { return n.changed }
func (n *Node) Log() <-chan string       { return n.logs }

// names caches the feeds' names while a list of entries is made (finding
// a feed's profile in force looks at all its entries).
type names map[[32]byte]string

func (m names) of(st *store.Store, key [32]byte) string {
	v, ok := m[key]
	if !ok {
		v = st.Name(key)
		m[key] = v
	}
	return v
}

func (n *Node) entry(e *terse.Entry, nm names) nodeapi.Entry {
	b, held := n.st.BlobForEntry(e)
	full := !article.BasicText([]byte(e.Title)) || held && e.Kind == terse.KindArticle && !article.BasicText(b)
	name := nm.of(n.st, e.Key)
	return nodeapi.Entry{
		Feed: e.KeyHex(), Label: nodeapi.LabelOf(name, e.KeyHex()), Seq: e.Seq, Time: e.Time, Kind: e.Kind,
		Size: e.Size, Blob: e.BlobHex(), Title: e.Title, Held: held,
		Arrival: e.Arrival, Name: name, FullText: full, Raw: e.Raw,
	}
}

func (n *Node) Timeline() []nodeapi.Entry {
	var out []nodeapi.Entry
	nm := names{}
	for _, e := range n.st.Timeline() {
		out = append(out, n.entry(e, nm))
	}
	return out
}

func (n *Node) feed(a terse.Address, withEntries bool) nodeapi.Feed {
	// The hints are those the feed was subscribed with, whatever the
	// address asked about carries (LOCAL.md 4.3, a feed record).
	a = terse.Address{Key: a.Key}
	if sb, ok := n.st.Subscribed(a.Key); ok {
		a = sb.Addr
	}
	f := nodeapi.Feed{Key: a.KeyHex(), Address: a.String(), Label: n.st.Label(a.Key), Name: n.st.Name(a.Key)}
	f.Nickname = n.st.Nickname(a.Key) != ""
	_, f.Subscribed = n.st.Subscribed(a.Key)
	for _, h := range a.Hints {
		f.Hints = append(f.Hints, h.URL())
	}
	if cl := n.client(); cl != nil {
		f.HintRelays = cl.HintRelays(a.Key)
	}
	if p, ok := n.st.Profile(a.Key); ok {
		f.ProfileRelays = p.Relays
		f.Description = p.Description
	}
	if withEntries {
		nm := names{a.Key: f.Name}
		for _, e := range n.st.FeedEntries(a.Key) {
			f.Entries = append(f.Entries, n.entry(e, nm))
		}
	}
	return f
}

func (n *Node) Subscriptions() []nodeapi.Feed {
	var out []nodeapi.Feed
	for _, sb := range n.st.Subscriptions() {
		out = append(out, n.feed(sb.Addr, false))
	}
	return out
}

func (n *Node) Feed(address string) (nodeapi.Feed, error) {
	a, err := terse.ParseUserAddress(address)
	if err != nil {
		return nodeapi.Feed{}, err
	}
	return n.feed(a, true), nil
}

func parseKey(s string) ([32]byte, error) {
	a, err := terse.ParseUserAddress(s)
	if err != nil {
		return [32]byte{}, err
	}
	return a.Key, nil
}

func (n *Node) Label(key string) string {
	k, err := parseKey(key)
	if err != nil {
		return terse.ShortKey(key)
	}
	return n.st.Label(k)
}

func (n *Node) Subscribe(address string) (bool, error) {
	a, err := terse.ParseUserAddress(address)
	if err != nil {
		return false, err
	}
	_, had := n.st.Subscribed(a.Key)
	if cl := n.client(); cl != nil {
		return !had, cl.Subscribe(a)
	}
	_, err = n.st.Subscribe(a)
	return !had, err
}

func (n *Node) Unsubscribe(key string) error {
	k, err := parseKey(key)
	if err != nil {
		return err
	}
	if _, ok := n.st.Subscribed(k); !ok {
		return errors.New("not subscribed to that feed")
	}
	if cl := n.client(); cl != nil {
		return cl.Unsubscribe(k)
	}
	_, err = n.st.Unsubscribe(k)
	return err
}

func (n *Node) SetNickname(key, name string) error {
	k, err := parseKey(key)
	if err != nil {
		return err
	}
	return n.st.SetNickname(k, name)
}

func parseID(s string) ([32]byte, error) {
	var id [32]byte
	if !terse.IsLowerHex64(s) {
		return id, errors.New("not a blob ID: 64 lowercase hex digits")
	}
	b, _ := hex.DecodeString(s)
	copy(id[:], b)
	return id, nil
}

func (n *Node) HeldBlob(id string) ([]byte, bool) {
	k, err := parseID(id)
	if err != nil {
		return nil, false
	}
	return n.st.Blob(k)
}

func (n *Node) Blob(id string) ([]byte, error) {
	k, err := parseID(id)
	if err != nil {
		return nil, err
	}
	if b, ok := n.st.Blob(k); ok {
		return b, nil
	}
	if err := n.Connect(); err != nil {
		return nil, err
	}
	return n.client().Get(k)
}

// ArticleTitle is the title of the first article entry stored for that
// blob and size (LOCAL.md 4.3, Title); ok is false if there is none.
func (n *Node) ArticleTitle(id string, size int) (string, bool) {
	k, err := parseID(id)
	if err != nil {
		return "", false
	}
	for _, e := range n.st.EntriesForBlob(k) {
		if e.Kind == terse.KindArticle && int(e.Size) == size {
			return e.Title, true
		}
	}
	return "", false
}

func (n *Node) Relays() []nodeapi.Relay {
	cl := n.client()
	if cl == nil {
		n.mu.Lock()
		urls := n.relays()
		n.mu.Unlock()
		var out []nodeapi.Relay
		for _, u := range urls {
			out = append(out, nodeapi.Relay{URL: u, Own: true})
		}
		return out
	}
	var out []nodeapi.Relay
	for _, r := range cl.Relays() {
		out = append(out, nodeapi.Relay{URL: r.URL, Own: r.Own, Hint: r.Hint, Connected: r.Connected})
	}
	return out
}

func (n *Node) ConfiguredRelays() []string { return n.st.Relays() }

// SetRelays replaces the user's relay list (a URL given twice counts
// once) and, once the node is connecting, uses it at once: relays no
// longer listed are disconnected and new ones connected (LOCAL.md 4.3).
func (n *Node) SetRelays(lines []string) error {
	var stored, list []string
	seen := map[string]bool{}
	check := &ws.Trust{}
	for _, line := range lines {
		r, opts, ok := ws.SplitLine(line)
		if !ok {
			continue
		}
		u := terse.TypedRelay(r)
		if u == "" {
			return fmt.Errorf("not a relay (a wss:// or ws:// URL, or a host): %s", r)
		}
		if err := check.AddLine(u, opts, n.st.Dir()); err != nil {
			return err
		}
		if !seen[u] {
			seen[u] = true
			list = append(list, u)
			stored = append(stored, strings.Join(append([]string{u}, opts...), " "))
		}
	}
	if len(list) > 8 {
		return errors.New("at most eight relays")
	}
	if err := n.st.SetRelays(stored); err != nil {
		return err
	}
	n.mu.Lock()
	n.opt.Relays = nil // from now on the list just set, not --relay
	cl, live := n.cl, n.live
	err := n.applyTrust()
	n.mu.Unlock()
	if err != nil {
		return err
	}
	if cl != nil {
		cl.SetOwnRelays(list)
	} else if live && len(list) > 0 {
		return n.Connect()
	}
	return nil
}

func (n *Node) Close() {
	n.mu.Lock()
	cl := n.cl
	n.mu.Unlock()
	if cl != nil {
		cl.Stop()
	}
	close(n.stop)
	n.st.Close()
}
