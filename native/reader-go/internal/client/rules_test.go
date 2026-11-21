package client

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/binary"
	"errors"
	"fmt"
	"net"
	"net/http"
	"strings"
	"sync"
	"testing"
	"time"

	"tersereader/internal/blake3"
	"tersereader/internal/store"
	"tersereader/internal/terse"
	"tersereader/internal/ws"
)

// --- a configurable in-process relay ---

type testRelay struct {
	t   *testing.T
	url string
	srv *http.Server

	mu        sync.Mutex
	entries   map[[32]byte][][]byte // pushed on subscribe, by feed
	highest   map[[32]byte]uint32   // highest reported, by feed (default: of entries)
	refuse    map[[32]byte]bool     // subscribe refused (status 3)
	blobs     map[[32]byte][]byte
	silentGet bool          // never answers a get
	getDelay  time.Duration // answers each get after this, without holding up the next request
	gets      int
	inFlight  int // gets received and not answered yet
	maxFlight int // the most at once
	subs      map[[32]byte]int
	unsubs    map[[32]byte]int
	conns     map[*ws.Conn]bool
}

func newTestRelay(t *testing.T) *testRelay {
	f := &testRelay{
		t:       t,
		entries: map[[32]byte][][]byte{},
		highest: map[[32]byte]uint32{},
		refuse:  map[[32]byte]bool{},
		blobs:   map[[32]byte][]byte{},
		subs:    map[[32]byte]int{},
		unsubs:  map[[32]byte]int{},
		conns:   map[*ws.Conn]bool{},
	}
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	f.url = "ws://" + ln.Addr().String()
	f.srv = &http.Server{Handler: f}
	go f.srv.Serve(ln)
	t.Cleanup(func() { f.srv.Close() })
	return f
}

func (f *testRelay) port() string { return f.url[strings.LastIndexByte(f.url, ':')+1:] }

func (f *testRelay) addEntry(key [32]byte, e []byte) {
	f.mu.Lock()
	f.entries[key] = append(f.entries[key], e)
	f.mu.Unlock()
}

func (f *testRelay) push(e []byte) {
	f.mu.Lock()
	var cs []*ws.Conn
	for c := range f.conns {
		cs = append(cs, c)
	}
	f.mu.Unlock()
	for _, c := range cs {
		c.WriteMessage(append([]byte{terse.MsgEntry}, e...))
	}
}

func (f *testRelay) count(m map[[32]byte]int, key [32]byte) int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return m[key]
}

func (f *testRelay) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	c, err := ws.Accept(w, r)
	if err != nil {
		return
	}
	f.mu.Lock()
	f.conns[c] = true
	f.mu.Unlock()
	defer func() {
		f.mu.Lock()
		delete(f.conns, c)
		f.mu.Unlock()
		c.Close()
	}()
	for {
		m, err := c.ReadMessage()
		if err != nil || len(m) < 5 {
			return
		}
		req := append([]byte(nil), m[1:5]...)
		var key [32]byte
		if len(m) >= 37 {
			copy(key[:], m[5:37])
		}
		switch m[0] {
		case terse.MsgSubscribe:
			after := binary.LittleEndian.Uint32(m[37:])
			f.mu.Lock()
			f.subs[key]++
			refused := f.refuse[key]
			es := append([][]byte(nil), f.entries[key]...)
			high, set := f.highest[key]
			f.mu.Unlock()
			if refused {
				c.WriteMessage(append(append([]byte{terse.MsgSubscribeReply}, req...), terse.StatusRefused))
				continue
			}
			if !set {
				for _, e := range es {
					_, seq, _ := terse.EntrySeq(e)
					high = max(high, seq)
				}
			}
			c.WriteMessage(binary.LittleEndian.AppendUint32(append(append([]byte{terse.MsgSubscribeReply}, req...), terse.StatusDone), high))
			for _, e := range es {
				if _, seq, _ := terse.EntrySeq(e); seq > after {
					c.WriteMessage(append([]byte{terse.MsgEntry}, e...))
				}
			}
		case terse.MsgUnsubscribe:
			f.mu.Lock()
			f.unsubs[key]++
			f.mu.Unlock()
			c.WriteMessage(append(append([]byte{terse.MsgUnsubscribeReply}, req...), terse.StatusDone))
		case terse.MsgGet:
			f.mu.Lock()
			f.gets++
			f.inFlight++
			f.maxFlight = max(f.maxFlight, f.inFlight)
			b, ok := f.blobs[key]
			silent, delay := f.silentGet, f.getDelay
			f.mu.Unlock()
			answer := func() {
				if delay > 0 {
					time.Sleep(delay)
				}
				f.mu.Lock()
				f.inFlight--
				f.mu.Unlock()
				switch {
				case silent:
				case ok:
					c.WriteMessage(append(append(append([]byte{terse.MsgGetReply}, req...), terse.StatusDone), b...))
				default:
					c.WriteMessage(append(append([]byte{terse.MsgGetReply}, req...), terse.StatusNotFound))
				}
			}
			if delay > 0 {
				go answer()
			} else {
				answer()
			}
		}
	}
}

// --- feeds and entries ---

type feed struct {
	priv ed25519.PrivateKey
	key  [32]byte
}

func newFeed(t *testing.T) feed {
	for {
		pub, priv, err := ed25519.GenerateKey(rand.Reader)
		if err != nil {
			t.Fatal(err)
		}
		if terse.ValidFeedKey(pub) {
			var f feed
			f.priv = priv
			copy(f.key[:], pub)
			return f
		}
	}
}

func (f feed) addr(t *testing.T, hints ...*testRelay) terse.Address {
	s := terse.KeyHex(f.key[:])
	for i, h := range hints {
		if i == 0 {
			s += "@"
		} else {
			s += ","
		}
		s += "127.0.0.1:" + h.port()
	}
	a, err := terse.ParseAddress(s)
	if err != nil {
		t.Fatal(err)
	}
	return a
}

// entry signs an entry of the feed for a blob of size bytes with ID blob.
func (f feed) entry(seq uint32, kind byte, size int, blob [32]byte, title string) []byte {
	b := append([]byte(nil), f.key[:]...)
	b = binary.LittleEndian.AppendUint32(b, seq)
	b = binary.LittleEndian.AppendUint32(b, 1700000000)
	b = append(b, kind, byte(size), byte(size>>8), byte(size>>16))
	b = append(b, blob[:]...)
	b = append(b, make([]byte, 32)...)
	b = append(b, byte(len(title)))
	b = append(b, title...)
	msg := append([]byte("terse-entry-0"), b...)
	return append(b, ed25519.Sign(f.priv, msg)...)
}

func (f feed) entryFor(seq uint32, kind byte, blob []byte, title string) []byte {
	return f.entry(seq, kind, len(blob), blake3.Sum256(blob), title)
}

func openStore(t *testing.T) *store.Store {
	st, err := store.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(st.Close)
	return st
}

func testOptions(own ...*testRelay) Options {
	opt := DefaultOptions()
	opt.AllowPrivate = true
	opt.AutoLimit = 0
	opt.Relays = []string{}
	for _, r := range own {
		opt.Relays = append(opt.Relays, r.url)
	}
	return opt
}

func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func mustAdd(t *testing.T, st *store.Store, raw []byte) *terse.Entry {
	t.Helper()
	e, err := terse.ParseEntry(raw)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := st.AddEntry(e); err != nil {
		t.Fatal(err)
	}
	return e
}

// --- hints (NETWORK.md 4.9) ---

// An own relay that refuses the feed holds none of it: the hints are used.
func TestHintsWhenOwnRelayRefuses(t *testing.T) {
	own, hint := newTestRelay(t), newTestRelay(t)
	f := newFeed(t)
	own.refuse[f.key] = true
	hint.addEntry(f.key, f.entryFor(1, terse.KindArticle, []byte("hello"), "hi"))

	st := openStore(t)
	cl := New(st, testOptions(own))
	cl.Start()
	defer cl.Stop()
	if err := cl.Subscribe(f.addr(t, hint)); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "the entry from the hinted relay", func() bool { return len(st.FeedEntries(f.key)) == 1 })
	if hr := cl.HintRelays(f.key); len(hr) != 1 || hr[0] != hint.url {
		t.Errorf("hint relays %v, want [%s]", hr, hint.url)
	}
}

// An own relay still connecting does not hold the decision up when a
// connected one has answered highest 0.
func TestHintsNotHeldUpByConnectingRelay(t *testing.T) {
	empty, hint := newTestRelay(t), newTestRelay(t)
	// A relay that accepts TCP but never completes the WebSocket handshake.
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	var hmu sync.Mutex
	var held []net.Conn
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			hmu.Lock()
			held = append(held, c)
			hmu.Unlock()
		}
	}()
	f := newFeed(t)
	hint.addEntry(f.key, f.entryFor(1, terse.KindArticle, []byte("hello"), "hi"))
	st := openStore(t)
	opt := testOptions(empty)
	opt.Relays = append(opt.Relays, "ws://"+ln.Addr().String())
	cl := New(st, opt)
	cl.Start()
	defer cl.Stop()
	// Before stopping, end the handshake that hangs.
	defer func() {
		ln.Close()
		hmu.Lock()
		for _, c := range held {
			c.Close()
		}
		hmu.Unlock()
	}()
	if err := cl.Subscribe(f.addr(t, hint)); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "the entry from the hinted relay", func() bool { return len(st.FeedEntries(f.key)) == 1 })
}

// Once an own relay answers with highest > 0, the hinted relays are no
// longer used for the feed, until it is unsubscribed and subscribed again.
func TestHintsDroppedWhenOwnRelayHolds(t *testing.T) {
	own, hint := newTestRelay(t), newTestRelay(t)
	f := newFeed(t)
	e1 := f.entryFor(1, terse.KindArticle, []byte("one"), "one")
	hint.addEntry(f.key, e1)

	st := openStore(t)
	cl := New(st, testOptions(own))
	cl.Start()
	defer cl.Stop()
	addr := f.addr(t, hint)
	if err := cl.Subscribe(addr); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "hints in use", func() bool { return len(st.FeedEntries(f.key)) == 1 && len(cl.HintRelays(f.key)) == 1 })

	// The own relay now holds the feed; a new subscribe learns so.
	own.addEntry(f.key, e1)
	own.addEntry(f.key, f.entryFor(2, terse.KindArticle, []byte("two"), "two"))
	if err := cl.Subscribe(addr); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "hints dropped", func() bool { return len(cl.HintRelays(f.key)) == 0 })
	waitFor(t, "the hint connection closed", func() bool {
		for _, r := range cl.Relays() {
			if r.URL == hint.url {
				return false
			}
		}
		return true
	})

	// The own relay loses the feed again: the hints are not used again.
	own.mu.Lock()
	own.highest[f.key] = 0
	own.entries[f.key] = nil
	own.mu.Unlock()
	if err := cl.Subscribe(addr); err != nil {
		t.Fatal(err)
	}
	time.Sleep(300 * time.Millisecond)
	if hr := cl.HintRelays(f.key); len(hr) != 0 {
		t.Errorf("hints used again before unsubscribing: %v", hr)
	}

	// Unsubscribed and subscribed again, they are.
	if err := cl.Unsubscribe(f.key); err != nil {
		t.Fatal(err)
	}
	if err := cl.Subscribe(addr); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "hints in use again", func() bool { return len(cl.HintRelays(f.key)) == 1 })
}

func addProfile(t *testing.T, st *store.Store, f feed, seq uint32, doc string) {
	t.Helper()
	e := mustAdd(t, st, f.entryFor(seq, terse.KindProfile, []byte(doc), "Name"))
	if err := st.PutBlob(e.Blob, []byte(doc)); err != nil {
		t.Fatal(err)
	}
}

// Hints are suppressed only while the profile in force names a relay the
// feed can be followed on: a relay line whose host is refused does not
// suppress them.
func TestHintsNotSuppressedByRefusedProfileRelay(t *testing.T) {
	f := newFeed(t)
	// The hint's host is refused too, so nothing is ever connected: the
	// test looks only at whether the hints are taken into use.
	a, err := terse.ParseAddress(terse.KeyHex(f.key[:]) + "@10.0.0.1:7000")
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		doc      string
		suppress bool
	}{
		{"relay: ws://127.0.0.1:1\nrelay: ws://localhost:2\n", false},
		{"relay: ws://127.0.0.1:1\nrelay: ws://relay.example:7001\n", true},
	} {
		st := openStore(t)
		opt := testOptions()
		opt.AllowPrivate = false
		cl := New(st, opt) // no own relays: none holds the feed
		if _, err := st.Subscribe(a); err != nil {
			t.Fatal(err)
		}
		addProfile(t, st, f, 1, tc.doc)
		if got := len(cl.profileRelays(f.key)) > 0; got != tc.suppress {
			t.Errorf("%q: profile relays usable %v, want %v", tc.doc, got, tc.suppress)
		}
		cl.evaluateHints(f.key)
		cl.mu.Lock()
		on := cl.hintsOn[f.key]
		n := len(cl.conns)
		cl.mu.Unlock()
		cl.Stop()
		if on == tc.suppress {
			t.Errorf("%q: hints in use %v, want %v", tc.doc, on, !tc.suppress)
		}
		if n != 0 {
			t.Errorf("%q: %d connections made", tc.doc, n)
		}
	}
}

// A profile whose only relay line is one of the user's own relays does not
// suppress the hints.
func TestHintsNotSuppressedByOwnProfileRelay(t *testing.T) {
	own, hint := newTestRelay(t), newTestRelay(t)
	f := newFeed(t)
	hint.addEntry(f.key, f.entryFor(2, terse.KindArticle, []byte("hello"), "hi"))
	st := openStore(t)
	addProfile(t, st, f, 1, "relay: "+own.url+"\n")
	cl := New(st, testOptions(own))
	cl.Start()
	defer cl.Stop()
	if err := cl.Subscribe(f.addr(t, hint)); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "the entry from the hinted relay", func() bool { return len(st.FeedEntries(f.key)) == 2 })
	if hr := cl.HintRelays(f.key); len(hr) != 1 || hr[0] != hint.url {
		t.Errorf("hint relays %v, want [%s]", hr, hint.url)
	}
}

// --- pushed entries ---

// A valid entry of a subscribed feed is kept whichever connection pushed
// it; an entry of a feed not subscribed is ignored.
func TestPushedEntryFromAnotherConnection(t *testing.T) {
	hint := newTestRelay(t)
	f1, f2, f3 := newFeed(t), newFeed(t), newFeed(t)
	st := openStore(t)
	cl := New(st, testOptions()) // no own relays: hints are used at once
	cl.Start()
	defer cl.Stop()
	if err := cl.Subscribe(f1.addr(t, hint)); err != nil {
		t.Fatal(err)
	}
	if err := cl.Subscribe(f2.addr(t)); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "the hinted relay's subscribe", func() bool { return hint.count(hint.subs, f1.key) > 0 })
	hint.push(f3.entryFor(1, terse.KindArticle, []byte("x"), "not subscribed"))
	hint.push(f2.entryFor(1, terse.KindArticle, []byte("y"), "subscribed"))
	waitFor(t, "the pushed entry of the other feed", func() bool { return len(st.FeedEntries(f2.key)) == 1 })
	if n := len(st.FeedEntries(f3.key)); n != 0 {
		t.Errorf("%d entries of a feed not subscribed kept", n)
	}
}

// --- feed relays from the profile in force ---

func TestFeedRelaysFollowProfileInForce(t *testing.T) {
	p1, p2 := newTestRelay(t), newTestRelay(t)
	f := newFeed(t)
	st := openStore(t)
	cl := New(st, testOptions())
	defer cl.Stop()
	if _, err := st.Subscribe(f.addr(t)); err != nil {
		t.Fatal(err)
	}
	setProfile := func(seq uint32, doc string) {
		e := mustAdd(t, st, f.entryFor(seq, terse.KindProfile, []byte(doc), "Name"))
		if err := st.PutBlob(e.Blob, []byte(doc)); err != nil {
			t.Fatal(err)
		}
		cl.updateFeedRelays(f.key)
	}
	feedRelays := func() []string {
		var out []string
		for _, r := range cl.Relays() {
			if !r.Own {
				out = append(out, r.URL)
			}
		}
		return out
	}
	setProfile(1, "relay: "+p1.url+"\n")
	if got := feedRelays(); len(got) != 1 || got[0] != p1.url {
		t.Fatalf("feed relays %v, want [%s]", got, p1.url)
	}
	setProfile(2, "relay: "+p2.url+"\n")
	if got := feedRelays(); len(got) != 1 || got[0] != p2.url {
		t.Fatalf("feed relays %v, want [%s]", got, p2.url)
	}
	// A profile in force whose document is not held names no relays.
	mustAdd(t, st, f.entryFor(3, terse.KindProfile, []byte("listed: yes\n"), "Name"))
	cl.updateFeedRelays(f.key)
	if got := feedRelays(); len(got) != 0 {
		t.Fatalf("feed relays %v, want none", got)
	}
}

// --- prefetch (NETWORK.md 4.4) ---

func TestPrefetchCounting(t *testing.T) {
	st := openStore(t)
	cl := New(st, testOptions()) // not started: jobs stay queued
	cl.opt.AutoLimit = 16384
	f := newFeed(t)
	var ids [][32]byte
	var text strings.Builder
	for i := 0; i < 11; i++ {
		b := []byte(fmt.Sprintf("blob %d", i))
		id := blake3.Sum256(b)
		ids = append(ids, id)
		fmt.Fprintf(&text, "see terse:%s\n", terse.KeyHex(id[:]))
		switch i {
		case 0: // held: does not count
			if err := st.PutBlob(id, b); err != nil {
				t.Fatal(err)
			}
		case 1: // being fetched: does not count
			cl.fetching[id] = true
		case 2: // over the limit by an entry held: counts, not fetched
			mustAdd(t, st, f.entry(1, terse.KindArticle, 20000, id, "big"))
		case 3: // a program's blob: counts, not fetched
			mustAdd(t, st, f.entry(2, terse.KindProgram, len(b), id, "Prog"))
		}
	}
	cl.prefetch([]byte(text.String()))
	for i, id := range ids {
		want := i >= 4 && i <= 9 || i == 1
		if cl.fetching[id] != want {
			t.Errorf("link %d: fetching %v, want %v", i, cl.fetching[id], want)
		}
	}
}

// --- get (NETWORK.md 5.1) ---

func TestGetConstants(t *testing.T) {
	if getTimeout != 10*time.Second {
		t.Errorf("getTimeout %v, want 10s", getTimeout)
	}
	if getGrace != 200*time.Millisecond {
		t.Errorf("getGrace %v, want 200ms", getGrace)
	}
}

// With at most two requests outstanding, a relay that never answers does
// not hold the others up: each "not found" lets the next one be asked.
func TestGetSlidingWindow(t *testing.T) {
	silent, nf1, nf2, has := newTestRelay(t), newTestRelay(t), newTestRelay(t), newTestRelay(t)
	silent.silentGet = true
	blob := []byte("the blob")
	id := blake3.Sum256(blob)
	has.blobs[id] = blob
	st := openStore(t)
	cl := New(st, testOptions(silent, nf1, nf2, has))
	cl.Start()
	defer cl.Stop()
	waitFor(t, "all relays connected", func() bool {
		n := 0
		for _, r := range cl.Relays() {
			if r.Connected {
				n++
			}
		}
		return n == 4
	})
	start := time.Now()
	b, err := cl.Get(id)
	if err != nil || string(b) != string(blob) {
		t.Fatalf("get: %q, %v", b, err)
	}
	if d := time.Since(start); d > 3*time.Second {
		t.Errorf("get took %v: held up by the silent relay", d)
	}
	for _, r := range []*testRelay{silent, nf1, nf2, has} {
		r.mu.Lock()
		g := r.gets
		r.mu.Unlock()
		if g != 1 {
			t.Errorf("%s asked %d times, want 1", r.url, g)
		}
	}
}

// With no relay connected and none connecting, a get fails at once; with
// relays connecting it waits only until none is still connecting.
func TestGetWithoutRelays(t *testing.T) {
	st := openStore(t)
	var id [32]byte
	cl := New(st, testOptions())
	cl.Start()
	start := time.Now()
	if _, err := cl.Get(id); !errors.Is(err, ErrNotFound) {
		t.Errorf("no relays: %v, want not found", err)
	}
	cl.Stop()

	// A port nobody listens on: the attempt fails fast.
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	dead := "ws://" + ln.Addr().String()
	ln.Close()
	opt := testOptions()
	opt.Relays = []string{dead}
	cl = New(st, opt)
	cl.Start()
	defer cl.Stop()
	if _, err := cl.Get(id); !errors.Is(err, ErrNotFound) {
		t.Errorf("unreachable relay: %v, want not found", err)
	}
	if d := time.Since(start); d > 5*time.Second {
		t.Errorf("took %v", d)
	}
}

// --- sync ---

// A sync's downloads go out together, pipelined on one connection
// (NETWORK.md 3: replies may come in any order), so a relay 50 ms away
// costs about 50 ms per downloaders blobs, not 50 ms per blob; and the
// sync ends soon after the last one.
func TestSyncFetchesConcurrently(t *testing.T) {
	r := newTestRelay(t)
	r.getDelay = 50 * time.Millisecond
	f := newFeed(t)
	const n = 40
	for i := 1; i <= n; i++ {
		blob := []byte(fmt.Sprintf("article %d", i))
		r.blobs[blake3.Sum256(blob)] = blob
		r.addEntry(f.key, f.entryFor(uint32(i), terse.KindArticle, blob, fmt.Sprint("t", i)))
	}
	st := openStore(t)
	if _, err := st.Subscribe(f.addr(t)); err != nil {
		t.Fatal(err)
	}
	opt := testOptions(r)
	opt.AutoLimit = 16384
	cl := New(st, opt)
	start := time.Now()
	cl.Start()
	defer cl.Stop()
	if !cl.WaitIdle(10 * time.Second) {
		t.Fatal("sync did not finish")
	}
	took := time.Since(start)
	for _, e := range st.Timeline() {
		if _, ok := st.BlobForEntry(e); !ok {
			t.Errorf("seq %d: blob not held after sync", e.Seq)
		}
	}
	r.mu.Lock()
	most, gets := r.maxFlight, r.gets
	r.mu.Unlock()
	t.Logf("%d blobs, 50 ms each: %v, at most %d gets at once", gets, took, most)
	if most < downloaders/2 || most > downloaders {
		t.Errorf("at most %d gets at once, want about %d", most, downloaders)
	}
	if one := time.Duration(n) * r.getDelay; took > one/2 {
		t.Errorf("sync took %v; one at a time takes %v", took, one)
	}
}

// With nothing to fetch a sync ends at once, not after a polling period.
func TestSyncWithNothingNewIsQuick(t *testing.T) {
	r := newTestRelay(t)
	f := newFeed(t)
	st := openStore(t)
	if _, err := st.Subscribe(f.addr(t)); err != nil {
		t.Fatal(err)
	}
	cl := New(st, testOptions(r))
	start := time.Now()
	cl.Start()
	defer cl.Stop()
	if !cl.WaitIdle(10 * time.Second) {
		t.Fatal("not idle")
	}
	if d := time.Since(start); d > 150*time.Millisecond {
		t.Errorf("an empty sync took %v", d)
	}
}

// SetOwnRelays takes effect at once (LOCAL.md 4.3, Set relays): a relay
// dropped is disconnected, a new one connected and subscribed to.
func TestSetOwnRelaysAtOnce(t *testing.T) {
	a, b := newTestRelay(t), newTestRelay(t)
	f := newFeed(t)
	st := openStore(t)
	if _, err := st.Subscribe(f.addr(t)); err != nil {
		t.Fatal(err)
	}
	cl := New(st, testOptions(a))
	cl.Start()
	defer cl.Stop()
	connected := func(want ...string) func() bool {
		return func() bool {
			var got []string
			for _, r := range cl.Relays() {
				if r.Connected && r.Own {
					got = append(got, r.URL)
				}
			}
			return fmt.Sprint(got) == fmt.Sprint(want)
		}
	}
	waitFor(t, "a connected", connected(a.url))
	cl.SetOwnRelays([]string{b.url})
	waitFor(t, "b connected and a dropped", connected(b.url))
	waitFor(t, "a's connection closed", func() bool {
		a.mu.Lock()
		defer a.mu.Unlock()
		return len(a.conns) == 0
	})
	waitFor(t, "the feed subscribed on b", func() bool { return b.count(b.subs, f.key) == 1 })
	cl.SetOwnRelays(nil)
	waitFor(t, "no relay", func() bool { return len(cl.Relays()) == 0 })
}
