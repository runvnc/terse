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

	"tersereader/internal/client"
	"tersereader/internal/nodeapi"
	"tersereader/internal/store"
	"tersereader/internal/terse"
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

func (n *Node) relays() []string {
	if n.opt.Relays != nil {
		return n.opt.Relays
	}
	return n.st.Relays()
}

// Connect starts the client.
func (n *Node) Connect() error {
	n.mu.Lock()
	defer n.mu.Unlock()
	if n.cl != nil {
		return nil
	}
	if len(n.relays()) == 0 {
		return errors.New("no relay: add one with `tersereader relay add ws://host:port`, or give --relay")
	}
	n.cl = client.New(n.st, n.opt)
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
	return n.client().WaitIdle(timeout), nil
}

func (n *Node) Changed() <-chan struct{} { return n.changed }
func (n *Node) Log() <-chan string       { return n.logs }

func (n *Node) entry(e *terse.Entry) nodeapi.Entry {
	_, held := n.st.BlobForEntry(e)
	return nodeapi.Entry{
		Feed: e.KeyHex(), Label: n.st.Label(e.Key), Seq: e.Seq, Time: e.Time, Kind: e.Kind,
		Size: e.Size, Blob: e.BlobHex(), Title: e.Title, Held: held,
	}
}

func (n *Node) Timeline() []nodeapi.Entry {
	var out []nodeapi.Entry
	for _, e := range n.st.Timeline() {
		out = append(out, n.entry(e))
	}
	return out
}

func (n *Node) feed(a terse.Address, withEntries bool) nodeapi.Feed {
	f := nodeapi.Feed{Key: a.KeyHex(), Address: a.String(), Label: n.st.Label(a.Key)}
	if sb, ok := n.st.Subscribed(a.Key); ok {
		f.Subscribed = true
		if len(a.Hints) == 0 {
			a = sb.Addr
			f.Address = a.String()
		}
	}
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
		for _, e := range n.st.FeedEntries(a.Key) {
			f.Entries = append(f.Entries, n.entry(e))
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

func (n *Node) ArticleTitle(id string, size int) string {
	k, err := parseID(id)
	if err != nil {
		return ""
	}
	for _, e := range n.st.EntriesForBlob(k) {
		if e.Kind == terse.KindArticle && int(e.Size) == size {
			return e.Title
		}
	}
	return ""
}

func (n *Node) Relays() []nodeapi.Relay {
	cl := n.client()
	if cl == nil {
		var out []nodeapi.Relay
		for _, u := range n.relays() {
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

func (n *Node) SetRelays(urls []string) error {
	for _, u := range urls {
		if !strings.HasPrefix(u, "ws://") && !strings.HasPrefix(u, "wss://") {
			return fmt.Errorf("a relay is a ws:// or wss:// URL: %s", u)
		}
	}
	return n.st.SetRelays(urls)
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
