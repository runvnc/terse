package localnode_test

// The rules LOCAL.md states for a node, one test each, against the Go
// node on a socket in a temporary directory: a raw client that frames
// messages itself, relays that are TCP listeners on ephemeral ports.

import (
	"bufio"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"tersereader/internal/blake3"
	"tersereader/internal/client"
	"tersereader/internal/local"
	"tersereader/internal/localnode"
	"tersereader/internal/node"
	"tersereader/internal/store"
	"tersereader/internal/terse"
)

// --- a raw client ---

type raw struct {
	t *testing.T
	c net.Conn
	r *bufio.Reader
}

func dialRaw(t *testing.T, sock string) *raw {
	t.Helper()
	c, err := local.Dial(sock)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { c.Close() })
	return &raw{t: t, c: c, r: bufio.NewReader(c)}
}

func (x *raw) send(m []byte) {
	x.t.Helper()
	if err := local.WriteFrame(x.c, m); err != nil {
		x.t.Fatal(err)
	}
}

// next is the next reply (pushes skipped), or nil if the node closed the
// connection.
func (x *raw) next() []byte {
	x.t.Helper()
	for {
		x.c.SetReadDeadline(time.Now().Add(10 * time.Second))
		m, err := local.ReadFrame(x.r)
		if err != nil {
			if ne, ok := err.(net.Error); ok && ne.Timeout() {
				x.t.Fatal("no reply")
			}
			return nil
		}
		if m[0] != local.Changed && m[0] != local.Log {
			return m
		}
	}
}

// call sends m and returns the reply's status and what follows it.
func (x *raw) call(m []byte) (byte, *local.R) {
	x.t.Helper()
	x.send(m)
	r := x.next()
	if r == nil {
		x.t.Fatalf("closed after %x", m[:5])
	}
	in := local.NewR(r)
	t, req := in.Head()
	if t != m[0]|0x80 || req != binary.LittleEndian.Uint32(m[1:]) {
		x.t.Fatalf("reply %x to %x", r[:min(6, len(r))], m[:5])
	}
	return in.U8(), in
}

func hello(req uint32, version byte) []byte {
	return local.New(local.Hello, req).U8(version).U8(0).Bytes()
}

func (x *raw) hello() {
	x.t.Helper()
	if st, in := x.call(hello(1, 0)); st != local.Done || in.U8() != 0 {
		x.t.Fatalf("hello: %d", st)
	}
}

// --- a node ---

// ruleNode serves a node over dir (made beforehand, or empty) with opt.
func ruleNode(t *testing.T, dir string, opt client.Options) (string, *node.Node) {
	t.Helper()
	opt.Logf = t.Logf
	n, err := node.Open(dir, opt)
	if err != nil {
		t.Fatal(err)
	}
	sock := filepath.Join(socketDir(t), "run", "node.sock")
	ln, err := local.Listen(sock)
	if err != nil {
		t.Fatal(err)
	}
	n.Connect() // as `tersereader node` does; no relay is not an error here
	srv := localnode.Serve(n, ln)
	t.Cleanup(func() { srv.Close(); n.Close() })
	return sock, n
}

func noRelays() client.Options {
	opt := client.DefaultOptions()
	opt.Relays = []string{}
	return opt
}

// --- feeds ---

type author struct {
	priv ed25519.PrivateKey
	key  [32]byte
}

func newAuthor(t *testing.T) author {
	for {
		pub, priv, err := ed25519.GenerateKey(rand.Reader)
		if err != nil {
			t.Fatal(err)
		}
		if terse.ValidFeedKey(pub) {
			a := author{priv: priv}
			copy(a.key[:], pub)
			return a
		}
	}
}

func (a author) hex() string { return terse.KeyHex(a.key[:]) }

func (a author) entry(t *testing.T, seq uint32, kind byte, blob []byte, title string) *terse.Entry {
	id := blake3.Sum256(blob)
	b := append([]byte(nil), a.key[:]...)
	b = binary.LittleEndian.AppendUint32(b, seq)
	b = binary.LittleEndian.AppendUint32(b, 1700000000)
	b = append(b, kind, byte(len(blob)), byte(len(blob)>>8), byte(len(blob)>>16))
	b = append(b, id[:]...)
	b = append(b, make([]byte, 32)...)
	b = append(b, byte(len(title)))
	b = append(b, title...)
	b = append(b, ed25519.Sign(a.priv, append([]byte("terse-entry-0"), b...))...)
	e, err := terse.ParseEntry(b)
	if err != nil {
		t.Fatal(err)
	}
	return e
}

// fill opens a store in dir and calls f on it.
func fill(t *testing.T, dir string, f func(st *store.Store)) {
	t.Helper()
	st, err := store.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	f(st)
	st.Close()
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func addr(t *testing.T, s string) terse.Address {
	a, err := terse.ParseAddress(s)
	must(t, err)
	return a
}

// --- LOCAL.md 3 and 4.1: Hello ---

// A second Hello closes the connection.
func TestRuleSecondHelloCloses(t *testing.T) {
	sock, _ := ruleNode(t, t.TempDir(), noRelays())
	x := dialRaw(t, sock)
	x.hello()
	x.send(hello(2, 0))
	if r := x.next(); r != nil {
		t.Fatalf("a reply to a second Hello: %x", r)
	}
}

// A Hello of a version the node does not speak is refused (status 3), and
// the node waits for another Hello.
func TestRuleWrongVersionHello(t *testing.T) {
	sock, _ := ruleNode(t, t.TempDir(), noRelays())
	x := dialRaw(t, sock)
	if st, _ := x.call(hello(1, 1)); st != local.Refused {
		t.Fatalf("version 1: status %d", st)
	}
	st, in := x.call(hello(2, 0))
	if st != local.Done || in.U8() != 0 {
		t.Fatalf("then version 0: status %d", st)
	}
	// Flags: this node does not answer Find and Lookup (bit 0 clear).
	if f := in.U8(); f != 0 || !in.Done() {
		t.Errorf("Hello flags %d", f)
	}
}

// A node that does not offer Find and Lookup ignores them like any type
// it does not know: no reply, the connection stays open.
func TestRuleFindAndLookupNotOffered(t *testing.T) {
	sock, _ := ruleNode(t, t.TempDir(), noRelays())
	x := dialRaw(t, sock)
	x.hello()
	x.send(local.New(0x06, 2).U8(1).Short("alice").Bytes())
	var key [32]byte
	x.send(local.New(0x08, 3).Raw(key[:]).U8(3).Short("hello").Bytes())
	if st, _ := x.call(local.New(local.Relays, 4).Bytes()); st != local.Done {
		t.Fatalf("relays: %d", st)
	}
}

// --- LOCAL.md 4.3 ---

// An invalid key: all 0xff is not the encoding of a point (y >= p).
var invalidKey = strings.Repeat("\xff", 32)

// Nickname for an invalid key is refused; Unsubscribe of one succeeds.
func TestRuleInvalidKey(t *testing.T) {
	sock, _ := ruleNode(t, t.TempDir(), noRelays())
	x := dialRaw(t, sock)
	x.hello()
	if st, _ := x.call(local.New(local.Nickname, 2).Raw([]byte(invalidKey)).Short("Al").Bytes()); st != local.Refused {
		t.Errorf("nickname for an invalid key: status %d", st)
	}
	if st, _ := x.call(local.New(local.Nickname, 3).Raw([]byte(invalidKey)).Short("").Bytes()); st != local.Refused {
		t.Errorf("no nickname for an invalid key: status %d", st)
	}
	if st, _ := x.call(local.New(local.Unsubscribe, 4).Raw([]byte(invalidKey)).Bytes()); st != local.Done {
		t.Errorf("unsubscribe from an invalid key: status %d", st)
	}
	a := newAuthor(t)
	if st, _ := x.call(local.New(local.Unsubscribe, 5).Raw(a.key[:]).Bytes()); st != local.Done {
		t.Errorf("unsubscribe from a feed not subscribed: status %d", st)
	}
}

// Feed reports the hints the feed was subscribed with (none if it is not
// subscribed), not those of the address asked about.
func TestRuleFeedHintsAreTheSubscriptions(t *testing.T) {
	dir := t.TempDir()
	sub, other := newAuthor(t), newAuthor(t)
	fill(t, dir, func(st *store.Store) {
		_, err := st.Subscribe(addr(t, sub.hex()+"@relay.example:1"))
		must(t, err)
	})
	opt := noRelays()
	opt.FeedRelays = false // no connection to the hint
	sock, _ := ruleNode(t, dir, opt)
	x := dialRaw(t, sock)
	x.hello()
	for _, c := range []struct{ address, hints, urls string }{
		{sub.hex(), "relay.example:1", "[wss://relay.example:1]"},
		{sub.hex() + "@other.example", "relay.example:1", "[wss://relay.example:1]"},
		{other.hex(), "", "[]"},
		{other.hex() + "@other.example:2", "", "[]"},
	} {
		st, in := x.call(local.New(local.Feed, 2).U8(0).Short(c.address).Bytes())
		if st != local.Done {
			t.Fatalf("%s: status %d", c.address, st)
		}
		f := in.Feed()
		h, _, _ := strings.Cut(f.Address, "@")
		hints := strings.TrimPrefix(f.Address, h)
		hints = strings.TrimPrefix(hints, "@")
		if hints != c.hints || fmt.Sprint(f.Hints) != c.urls || !in.Done() {
			t.Errorf("%s: hints %q %v, want %q %s", c.address, hints, f.Hints, c.hints, c.urls)
		}
	}
}

// Title: of several article entries for one blob and size, the one stored
// first; an empty title is status 1 with n 0; none is status 2.
func TestRuleTitle(t *testing.T) {
	dir := t.TempDir()
	a, b := newAuthor(t), newAuthor(t)
	blob, blank := []byte("one article"), []byte("an untitled one")
	fill(t, dir, func(st *store.Store) {
		for _, e := range []*terse.Entry{
			b.entry(t, 1, terse.KindProgram, blob, "program"),
			b.entry(t, 2, terse.KindArticle, blob, "First stored"),
			a.entry(t, 1, terse.KindArticle, blob, "Second stored"),
			a.entry(t, 2, terse.KindArticle, blank, ""),
		} {
			_, err := st.AddEntry(e)
			must(t, err)
		}
	})
	sock, _ := ruleNode(t, dir, noRelays())
	x := dialRaw(t, sock)
	x.hello()
	title := func(req uint32, b []byte, size int) (byte, string, bool) {
		id := blake3.Sum256(b)
		st, in := x.call(local.New(local.Title, req).Raw(id[:]).U32(uint32(size)).Bytes())
		if st != local.Done {
			return st, "", in.Done()
		}
		s := in.Short()
		return st, s, in.Done()
	}
	if st, s, end := title(2, blob, len(blob)); st != local.Done || s != "First stored" || !end {
		t.Errorf("several: %d %q", st, s)
	}
	if st, s, end := title(3, blank, len(blank)); st != local.Done || s != "" || !end {
		t.Errorf("empty title: %d %q", st, s)
	}
	if st, _, end := title(4, blob, 1); st != local.NotFound || !end {
		t.Errorf("wrong size: %d", st)
	}
}

// A Feed reply that would pass the 2 MiB limit carries the latest records
// that fit, in increasing seq, and says how many.
func TestRuleFeedReplyOverTheLimit(t *testing.T) {
	if testing.Short() {
		t.Skip("short")
	}
	dir := t.TempDir()
	a := newAuthor(t)
	const n = 7500 // records of 301 bytes: about 2.2 MB
	fill(t, dir, func(st *store.Store) {
		title := strings.Repeat("t", 120)
		for i := 1; i <= n; i++ {
			_, err := st.AddEntry(a.entry(t, uint32(i), terse.KindArticle, []byte(fmt.Sprint("article ", i)), title))
			must(t, err)
		}
	})
	sock, _ := ruleNode(t, dir, noRelays())
	x := dialRaw(t, sock)
	x.hello()
	start := time.Now()
	x.send(local.New(local.Feed, 2).U8(1).Short(a.hex()).Bytes())
	m := x.next()
	t.Logf("a Feed of %d entries answered in %v", n, time.Since(start))
	if len(m) > local.MaxMessage {
		t.Fatalf("%d bytes", len(m))
	}
	in := local.NewR(m)
	in.Head()
	if st := in.U8(); st != local.Done {
		t.Fatalf("status %d", st)
	}
	in.Feed()
	count := int(in.U16())
	var seqs []uint32
	for i := 0; i < count; i++ {
		seqs = append(seqs, in.Record().Seq)
	}
	if !in.Done() || count == 0 || count >= n {
		t.Fatalf("%d records of %d, well formed %v", count, n, in.Done())
	}
	for i, s := range seqs {
		if want := uint32(n - count + 1 + i); s != want {
			t.Fatalf("record %d: seq %d, want %d", i, s, want)
		}
	}
	if len(m)+301 <= local.MaxMessage {
		t.Errorf("%d records in %d bytes: one more would fit", count, len(m))
	}
}

// --- relays ---

// tcpRelay is a relay URL whose listener counts connections and lets one
// see them close; it never completes a WebSocket handshake.
type tcpRelay struct {
	url   string
	mu    sync.Mutex
	open  int
	total int
}

func newTCPRelay(t *testing.T) *tcpRelay {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	must(t, err)
	r := &tcpRelay{url: "ws://" + ln.Addr().String()}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			r.mu.Lock()
			r.open++
			r.total++
			r.mu.Unlock()
			go func() {
				io.Copy(io.Discard, c) // until the client closes it
				c.Close()
				r.mu.Lock()
				r.open--
				r.mu.Unlock()
			}()
		}
	}()
	return r
}

func (r *tcpRelay) state() (open, total int) {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.open, r.total
}

func waitUntil(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out: %s", what)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func setRelays(x *raw, req uint32, urls ...string) byte {
	m := local.New(local.SetRelays, req).U8(byte(len(urls)))
	for _, u := range urls {
		m.Short(u)
	}
	st, _ := x.call(m.Bytes())
	return st
}

// Set relays takes effect at once: the node connects to the new relays
// (also when it had none) and disconnects from those no longer listed.
func TestRuleSetRelaysAtOnce(t *testing.T) {
	a, b := newTCPRelay(t), newTCPRelay(t)
	sock, _ := ruleNode(t, t.TempDir(), client.DefaultOptions())
	x := dialRaw(t, sock)
	x.hello()
	if st := setRelays(x, 2, a.url); st != local.Done {
		t.Fatalf("set relays: %d", st)
	}
	waitUntil(t, "a connection to the first relay", func() bool { o, _ := a.state(); return o == 1 })
	if st := setRelays(x, 3, b.url, b.url); st != local.Done {
		t.Fatalf("set relays: %d", st)
	}
	waitUntil(t, "the first relay dropped, the second connected", func() bool {
		oa, _ := a.state()
		ob, _ := b.state()
		return oa == 0 && ob == 1
	})
	st, in := x.call(local.New(local.Relays, 4).Bytes())
	if st != local.Done || in.U8() != 1 || in.U8() != local.RelayConfigured || in.Short() != b.url || !in.Done() {
		t.Errorf("relays after set relays: not just %s, configured", b.url)
	}
	if st := setRelays(x, 5); st != local.Done {
		t.Fatalf("set no relays: %d", st)
	}
	waitUntil(t, "no connection", func() bool { o, _ := b.state(); return o == 0 })
	// With no relay, Sync cannot try.
	if st, _ := x.call(local.New(local.Sync, 6).U32(1000).Bytes()); st != local.Refused {
		t.Errorf("sync with no relay: %d", st)
	}
	// And Get of a blob not held is not found at once.
	var id [32]byte
	if st, _ := x.call(local.New(local.Get, 7).Raw(id[:]).Bytes()); st != local.NotFound {
		t.Errorf("get with no relay: %d", st)
	}
	for _, bad := range [][]string{{"http://relay.example"}, strings.Fields(strings.Repeat("ws://r.example ", 1) + "ws://a ws://b ws://c ws://d ws://e ws://f ws://g ws://h")} {
		if st := setRelays(x, 8, bad...); st != local.Refused {
			t.Errorf("set relays %v: %d", bad, st)
		}
	}
}

// Relays: a relay a feed's profile names has neither bit 0 (configured)
// nor bit 1 (hinted); a hint in use has bit 1.
func TestRuleRelaysFlags(t *testing.T) {
	named, hinted := newTCPRelay(t), newTCPRelay(t)
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	must(t, err)
	own := &tcpRelay{url: "ws://" + ln.Addr().String()}
	ln.Close() // nobody listens: the own relay fails at once
	dir := t.TempDir()
	withProfile, withHint := newAuthor(t), newAuthor(t)
	// A profile lists encrypted relays only (NETWORK.md 4.6): named as
	// wss:// (whether it answers does not matter here).
	namedURL := "wss://" + strings.TrimPrefix(named.url, "ws://")
	doc := []byte("listed: yes\nrelay: " + namedURL + "\n")
	fill(t, dir, func(st *store.Store) {
		_, err := st.AddEntry(withProfile.entry(t, 1, terse.KindProfile, doc, "Named"))
		must(t, err)
		must(t, st.PutBlob(blake3.Sum256(doc), doc))
		_, err = st.Subscribe(addr(t, withProfile.hex()))
		must(t, err)
		_, err = st.Subscribe(addr(t, withHint.hex()+"@127.0.0.1:"+hinted.url[strings.LastIndexByte(hinted.url, ':')+1:]))
		must(t, err)
		must(t, st.SetRelays([]string{own.url}))
	})
	opt := client.DefaultOptions()
	opt.AllowPrivate = true
	sock, _ := ruleNode(t, dir, opt)
	x := dialRaw(t, sock)
	x.hello()
	// The own relay cannot be reached, so the hint comes into use (no own
	// relay holds the feed).
	want := map[string]byte{own.url: local.RelayConfigured, namedURL: 0, hinted.url: local.RelayHint}
	var got map[string]byte
	waitUntil(t, "the three relays listed", func() bool {
		st, in := x.call(local.New(local.Relays, 2).Bytes())
		if st != local.Done {
			t.Fatalf("relays: %d", st)
		}
		got = map[string]byte{}
		for i, n := 0, int(in.U8()); i < n; i++ {
			f := in.U8()
			got[in.Short()] = f &^ local.RelayConnected
		}
		return fmt.Sprint(got) == fmt.Sprint(want)
	})
}
