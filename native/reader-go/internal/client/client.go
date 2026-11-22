// Package client speaks the Terse relay protocol for a reading client
// (NETWORK.md sections 2 to 6): it keeps connections to the user's
// relays and to feed relays, subscribes to the subscribed feeds, verifies
// and stores entries and blobs, downloads small articles automatically and
// prefetches the articles they link to.
package client

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/url"
	"sort"
	"strings"
	"sync"
	"time"

	"tersereader/internal/blake3"
	"tersereader/internal/store"
	"tersereader/internal/terse"
	"tersereader/internal/ws"
)

// Options are the user's choices.
type Options struct {
	Relays       []string // the user's relays for this run; nil: the store's list
	Authorities  []string // --relay-authority: PEM files of authorities trusted for every relay
	AutoLimit    int      // automatic download limit in bytes (NETWORK.md 4.4); 0: none
	Prefetch     bool     // prefetch the articles that automatically downloaded articles link to
	FeedRelays   bool     // follow feeds on the relays their profiles name, and on their addresses' hints
	AllowPrivate bool     // tests only: accept loopback and private hosts in relay lines and hints
	Logf         func(format string, args ...any)
}

// DefaultOptions are the defaults of NETWORK.md.
func DefaultOptions() Options {
	return Options{AutoLimit: 16384, Prefetch: true, FeedRelays: true}
}

const (
	maxConns    = 16
	downloaders = 8 // blobs fetched at once (requests are pipelined, NETWORK.md 3)
	maxOwn      = 8
	prefetchMax = 8
	getGrace    = 200 * time.Millisecond // how long a request waits for more relays once one is up
	maxBackoff  = 30 * time.Second
	idlePoll    = 2 * time.Millisecond  // how often WaitIdle looks
	idleSettle  = 10 * time.Millisecond // how long Idle must hold for WaitIdle
)

// The timers a test lengthens, to show that something finished without
// waiting for one of them (set before a client starts; never changed
// otherwise).
var (
	getTimeout   = 10 * time.Second // a relay that has not answered a get by then has failed
	dialTimeout  = 10 * time.Second
	firstBackoff = time.Second
)

// ErrNotFound is returned by Get when no relay has the blob.
var ErrNotFound = errors.New("not found on any relay")

type subState struct {
	after    uint32
	replied  bool
	refused  bool
	highest  uint32
	caughtUp bool
}

// Why a feed relay serves a feed: its profile in force names the relay,
// or the relay is a hint of the feed's address (or both).
const (
	viaProfile uint8 = 1 << iota
	viaHint
)

type conn struct {
	url string
	own bool
	cl  *Client

	// guarded by cl.mu
	feeds      map[[32]byte]uint8 // feeds a feed relay serves, and why (own relays serve all)
	ws         *ws.Conn
	connected  bool
	connecting bool // a connection attempt is under way
	tried      bool // a connection attempt has finished, one way or the other
	refused    bool // its host is refused (NETWORK.md 5.1): never connected
	nextReq    uint32
	pending    map[uint32]chan *terse.Reply
	subReq     map[uint32][32]byte
	subs       map[[32]byte]*subState
	stop       chan struct{}
	stopped    bool
}

// Client is a reading client's network side.
type Client struct {
	st  *store.Store
	opt Options

	ctx    context.Context
	cancel context.CancelFunc
	wg     sync.WaitGroup

	mu       sync.Mutex
	conns    []*conn
	fetching map[[32]byte]bool
	work     int // download jobs queued or running
	jobs     chan job
	changed  chan struct{}
	wake     chan struct{} // closed (and replaced) whenever a connection's state changes

	// NETWORK.md 4.9, per subscribed feed (guarded by mu): hintsOn, the
	// hints are in use (they stay so); heldOnOwn, an own relay answered a
	// subscribe with highest > 0, so the hints are not used again until
	// the feed is unsubscribed.
	hintsOn   map[[32]byte]bool
	heldOnOwn map[[32]byte]bool

	own []string // the user's relays now (guarded by mu); SetOwnRelays replaces them
}

type job struct {
	id       [32]byte
	entry    *terse.Entry // the entry the blob is downloaded for, if any
	prefetch bool
}

// New makes a client over a store. Start connects it.
func New(st *store.Store, opt Options) *Client {
	ctx, cancel := context.WithCancel(context.Background())
	if opt.Logf == nil {
		opt.Logf = func(string, ...any) {}
	}
	return &Client{
		st: st, opt: opt, ctx: ctx, cancel: cancel,
		fetching:  map[[32]byte]bool{},
		jobs:      make(chan job, 4096),
		changed:   make(chan struct{}, 1),
		wake:      make(chan struct{}),
		hintsOn:   map[[32]byte]bool{},
		heldOnOwn: map[[32]byte]bool{},
	}
}

// Changed is signalled (without blocking) whenever something new is held.
func (c *Client) Changed() <-chan struct{} { return c.changed }

func (c *Client) signal() {
	select {
	case c.changed <- struct{}{}:
	default:
	}
}

// ownRelays is the user's relay list in use.
func (c *Client) ownRelays() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]string(nil), c.own...)
}

// Start connects to the configured relays and starts the downloaders.
func (c *Client) Start() {
	relays := c.opt.Relays
	if relays == nil {
		relays = c.st.Relays()
	}
	if len(relays) > maxOwn {
		relays = relays[:maxOwn]
	}
	c.mu.Lock()
	c.own = append([]string(nil), relays...)
	for _, u := range relays {
		c.addConnLocked(u, true)
	}
	c.mu.Unlock()
	for i := 0; i < downloaders; i++ {
		c.wg.Add(1)
		go c.downloader()
	}
	// Feeds whose profiles name relays are followed there too; feeds with
	// hints and no own relay are followed on the hints.
	for _, sb := range c.st.Subscriptions() {
		c.updateFeedRelays(sb.Addr.Key)
		c.evaluateHints(sb.Addr.Key)
	}
}

// SetOwnRelays makes urls the user's relays from now on (LOCAL.md 4.3,
// Set relays): connections to own relays no longer listed are closed, new
// ones are connected (a feed relay now listed is connected again as an own
// relay), and the feeds' profile relays and hints are worked out again
// against the new list.
func (c *Client) SetOwnRelays(urls []string) {
	if len(urls) > maxOwn {
		urls = urls[:maxOwn]
	}
	listed := map[string]bool{}
	for _, u := range urls {
		listed[u] = true
	}
	c.mu.Lock()
	c.own = append([]string(nil), urls...)
	for _, cn := range append([]*conn(nil), c.conns...) {
		if cn.own != listed[cn.url] {
			c.removeConnLocked(cn)
		}
	}
	for _, u := range urls {
		c.addConnLocked(u, true)
	}
	c.mu.Unlock()
	for _, sb := range c.st.Subscriptions() {
		c.updateFeedRelays(sb.Addr.Key)
		c.evaluateHints(sb.Addr.Key)
	}
	c.signal()
}

// Stop closes every connection and waits for the client's goroutines.
func (c *Client) Stop() {
	c.cancel()
	c.mu.Lock()
	for _, cn := range c.conns {
		c.stopConnLocked(cn)
	}
	c.mu.Unlock()
	c.wg.Wait()
}

func (c *Client) stopConnLocked(cn *conn) {
	if cn.stopped {
		return
	}
	cn.stopped = true
	close(cn.stop)
	if cn.ws != nil {
		cn.ws.Close()
	}
}

func (c *Client) findConnLocked(u string) *conn {
	for _, cn := range c.conns {
		if cn.url == u {
			return cn
		}
	}
	return nil
}

// connStateLocked wakes the requests waiting for a connection (cl.mu held).
func (c *Client) connStateLocked() {
	close(c.wake)
	c.wake = make(chan struct{})
}

func (c *Client) addConnLocked(u string, own bool) *conn {
	if cn := c.findConnLocked(u); cn != nil {
		return cn
	}
	if len(c.conns) >= maxConns {
		c.opt.Logf("not connecting to %s: %d connections already", u, maxConns)
		return nil
	}
	cn := &conn{
		url: u, own: own, cl: c, connecting: true,
		feeds:   map[[32]byte]uint8{},
		pending: map[uint32]chan *terse.Reply{},
		subReq:  map[uint32][32]byte{},
		subs:    map[[32]byte]*subState{},
		stop:    make(chan struct{}),
	}
	c.conns = append(c.conns, cn)
	c.wg.Add(1)
	go cn.run()
	return cn
}

func (c *Client) removeConnLocked(cn *conn) {
	c.stopConnLocked(cn)
	cn.connecting = false
	c.connStateLocked()
	for i, x := range c.conns {
		if x == cn {
			c.conns = append(c.conns[:i], c.conns[i+1:]...)
			return
		}
	}
}

// serves reports whether a connection serves a feed (cl.mu held).
func (cn *conn) serves(key [32]byte) bool {
	if cn.own {
		_, ok := cn.cl.st.Subscribed(key)
		return ok
	}
	return cn.feeds[key] != 0
}

// hint reports whether a feed relay serves some feed because of a hint
// (cl.mu held).
func (cn *conn) hint() bool {
	for _, v := range cn.feeds {
		if v&viaHint != 0 {
			return true
		}
	}
	return false
}

// --- connections ---

func (cn *conn) run() {
	defer cn.cl.wg.Done()
	backoff := firstBackoff
	for {
		select {
		case <-cn.stop:
			return
		default:
		}
		cl := cn.cl
		cl.mu.Lock()
		cn.connecting = !cn.stopped
		cl.connStateLocked()
		cl.mu.Unlock()
		w, err := cn.dial()
		if err != nil {
			select {
			case <-cn.stop: // dropped while connecting (removeConnLocked)
				return
			default:
			}
			cl.mu.Lock()
			cn.tried = true
			cn.connecting = false
			cl.work++ // until the hints are worked out again (Idle)
			cl.connStateLocked()
			cl.mu.Unlock()
			cl.opt.Logf("%s: %v", cn.url, err)
			cl.afterConnChange()
			cl.doneWork()
			if cn.refusedNow() {
				return
			}
			select {
			case <-cn.stop:
				return
			case <-time.After(backoff):
			}
			backoff *= 2
			if backoff > maxBackoff {
				backoff = maxBackoff
			}
			continue
		}
		backoff = firstBackoff
		cl.mu.Lock()
		cn.connecting = false
		if cn.stopped {
			cl.connStateLocked()
			cl.mu.Unlock()
			w.Close()
			return
		}
		cn.ws, cn.connected, cn.tried = w, true, true
		cl.connStateLocked()
		cl.mu.Unlock()
		cl.opt.Logf("connected to %s", cn.url)
		cn.subscribeAll()
		cn.readLoop(w)
		cl.mu.Lock()
		cn.connected = false
		cn.ws = nil
		cl.connStateLocked()
		for req, ch := range cn.pending {
			close(ch)
			delete(cn.pending, req)
		}
		cn.subs = map[[32]byte]*subState{}
		cn.subReq = map[uint32][32]byte{}
		cl.mu.Unlock()
		cl.afterConnChange()
	}
}

func (cn *conn) refusedNow() bool {
	cn.cl.mu.Lock()
	defer cn.cl.mu.Unlock()
	return cn.refused
}

// dial connects. A feed relay's host is checked first (NETWORK.md 5.1):
// a name is resolved once, refused if any address is loopback, private
// or link-local, and the connection goes to an address that was checked.
func (cn *conn) dial() (*ws.Conn, error) {
	if cn.own {
		return ws.DialStop(cn.url, "", dialTimeout, cn.stop)
	}
	u, err := url.Parse(cn.url)
	if err != nil {
		return nil, err
	}
	host := u.Hostname()
	port := u.Port()
	if port == "" {
		port = "80"
		if u.Scheme == "wss" {
			port = "443"
		}
	}
	refuse := func() (*ws.Conn, error) {
		cn.cl.mu.Lock()
		cn.refused = true
		cn.cl.mu.Unlock()
		return nil, fmt.Errorf("refused: a feed relay may not be a local or private address")
	}
	allow := cn.cl.opt.AllowPrivate
	if !allow && terse.RefusedHost(host) {
		return refuse()
	}
	ctx, cancel := context.WithTimeout(cn.cl.ctx, dialTimeout)
	defer cancel()
	ips, err := net.DefaultResolver.LookupNetIP(ctx, "ip", host)
	if err != nil {
		return nil, err
	}
	if len(ips) == 0 {
		return nil, fmt.Errorf("%s has no address", host)
	}
	for _, ip := range ips {
		if !allow && terse.RefusedIP(ip) {
			return refuse()
		}
	}
	addr := ips[0].Unmap().String()
	return ws.DialStop(cn.url, net.JoinHostPort(addr, port), dialTimeout, cn.stop)
}

func (cn *conn) send(m []byte) error {
	cn.cl.mu.Lock()
	w := cn.ws
	cn.cl.mu.Unlock()
	if w == nil {
		return errors.New("not connected")
	}
	return w.WriteMessage(m)
}

// subscribeAll subscribes, on a new connection, to every feed it serves.
func (cn *conn) subscribeAll() {
	for _, sb := range cn.cl.st.Subscriptions() {
		cn.cl.mu.Lock()
		ok := cn.serves(sb.Addr.Key)
		cn.cl.mu.Unlock()
		if ok {
			cn.subscribe(sb.Addr.Key)
		}
	}
}

// subscribe sends Subscribe with `after` the highest seq held (NETWORK.md 4.4).
func (cn *conn) subscribe(key [32]byte) {
	after := cn.cl.st.HighestSeq(key)
	cn.cl.mu.Lock()
	if !cn.connected {
		cn.cl.mu.Unlock()
		return
	}
	cn.nextReq++
	req := cn.nextReq
	cn.subReq[req] = key
	cn.subs[key] = &subState{after: after}
	cn.cl.mu.Unlock()
	if err := cn.send(terse.EncodeSubscribe(req, key, after)); err != nil {
		cn.cl.opt.Logf("%s: %v", cn.url, err)
	}
}

func (cn *conn) unsubscribe(key [32]byte) {
	cn.cl.mu.Lock()
	if !cn.connected || cn.subs[key] == nil {
		delete(cn.subs, key)
		cn.cl.mu.Unlock()
		return
	}
	delete(cn.subs, key)
	cn.nextReq++
	req := cn.nextReq
	cn.cl.mu.Unlock()
	cn.send(terse.EncodeUnsubscribe(req, key))
}

func (cn *conn) readLoop(w *ws.Conn) {
	for {
		m, err := w.ReadMessage()
		if err != nil {
			select {
			case <-cn.stop:
			default:
				cn.cl.opt.Logf("%s: connection lost: %v", cn.url, err)
			}
			w.Close()
			return
		}
		r, err := terse.DecodeReply(m)
		if err != nil || r.Ignored {
			continue
		}
		switch r.Type {
		case terse.MsgEntry:
			cn.cl.handleEntry(cn, r.Entry)
		case terse.MsgSubscribeReply:
			cn.cl.mu.Lock()
			key, ok := cn.subReq[r.Req]
			delete(cn.subReq, r.Req)
			s := cn.subs[key]
			held := false
			cn.cl.work++ // until the hints are worked out again (Idle)
			if ok && s != nil {
				s.replied = true
				if r.Status != terse.StatusDone {
					s.refused = true
				} else {
					s.highest = r.Highest
					if r.Highest <= s.after {
						s.caughtUp = true
					}
					held = cn.own && r.Highest > 0
				}
			}
			cn.cl.mu.Unlock()
			if held {
				cn.cl.heldOnOwnRelay(key)
			} else if ok {
				cn.cl.evaluateHints(key)
			}
			cn.cl.doneWork()
		case terse.MsgGetReply:
			cn.cl.mu.Lock()
			ch := cn.pending[r.Req]
			delete(cn.pending, r.Req)
			cn.cl.mu.Unlock()
			if ch != nil {
				ch <- r
				close(ch)
			}
		}
	}
}

// --- entries ---

func (c *Client) handleEntry(cn *conn, raw []byte) {
	e, err := terse.ParseEntry(raw)
	if err == nil {
		// A valid entry of a subscribed feed is kept whichever relay
		// pushed it; entries of other feeds are ignored.
		if _, subscribed := c.st.Subscribed(e.Key); subscribed {
			added, err := c.st.AddEntry(e)
			if err != nil {
				c.opt.Logf("store: %v", err)
			}
			if added {
				c.entryAdded(e)
			}
		}
	}
	// Caught up once an entry with seq >= highest arrived, valid or not.
	if key, seq, ok := terse.EntrySeq(raw); ok {
		c.mu.Lock()
		if s := cn.subs[key]; s != nil && s.replied && seq >= s.highest {
			s.caughtUp = true
		}
		c.mu.Unlock()
	}
	c.signal()
}

func (c *Client) autoKind(k byte) bool {
	return k == terse.KindArticle || k == terse.KindProfile || k == terse.KindDocument
}

func (c *Client) entryAdded(e *terse.Entry) {
	if !c.autoKind(e.Kind) || c.opt.AutoLimit <= 0 || int(e.Size) > c.opt.AutoLimit {
		return
	}
	if _, ok := c.st.BlobForEntry(e); ok {
		if e.Kind == terse.KindProfile {
			c.updateFeedRelays(e.Key)
		}
		return
	}
	c.enqueue(job{id: e.Blob, entry: e})
}

func (c *Client) enqueue(j job) bool {
	c.mu.Lock()
	if c.fetching[j.id] {
		c.mu.Unlock()
		return false
	}
	c.fetching[j.id] = true
	c.work++
	c.mu.Unlock()
	select {
	case c.jobs <- j:
		return true
	default:
		c.mu.Lock()
		delete(c.fetching, j.id)
		c.work--
		c.mu.Unlock()
		return false
	}
}

func (c *Client) downloader() {
	defer c.wg.Done()
	for {
		select {
		case <-c.ctx.Done():
			return
		case j := <-c.jobs:
			c.runJob(j)
			c.mu.Lock()
			delete(c.fetching, j.id)
			c.mu.Unlock()
			c.doneWork()
		}
	}
}

func (c *Client) runJob(j job) {
	limit := 0
	if j.prefetch {
		limit = c.opt.AutoLimit
	}
	b, err := c.get(j.id, limit)
	if err != nil {
		c.opt.Logf("blob %s: %v", terse.ShortKey(terse.KeyHex(j.id[:])), err)
		return
	}
	if j.entry == nil {
		return
	}
	if uint32(len(b)) != j.entry.Size {
		c.opt.Logf("blob %s: %d bytes, but its entry says %d: not associated with it",
			terse.ShortKey(j.entry.BlobHex()), len(b), j.entry.Size)
		return
	}
	switch j.entry.Kind {
	case terse.KindProfile:
		c.updateFeedRelays(j.entry.Key)
	case terse.KindArticle:
		if c.opt.Prefetch {
			c.prefetch(b)
		}
	}
}

// prefetch takes the first 8 distinct article links of an automatically
// downloaded article (in text order) whose blobs are neither held nor
// being fetched, and fetches each of them that no entry held puts over the
// automatic download limit or names as a program (NETWORK.md 4.4). Links
// skipped that way count towards the 8.
func (c *Client) prefetch(text []byte) {
	n := 0
	for _, id := range ScanArticleLinks(text) {
		if n >= prefetchMax {
			return
		}
		if c.st.HasBlob(id) {
			continue
		}
		c.mu.Lock()
		busy := c.fetching[id]
		c.mu.Unlock()
		if busy {
			continue
		}
		n++
		skip := false
		for _, e := range c.st.EntriesForBlob(id) {
			if e.Kind == terse.KindProgram || int(e.Size) > c.opt.AutoLimit {
				skip = true
			}
		}
		if !skip {
			c.enqueue(job{id: id, prefetch: true})
		}
	}
}

// ScanArticleLinks is the plain scan of NETWORK.md 4.4: every `terse:`
// followed by 64 lowercase hex digits, not preceded by an ASCII letter or
// digit and not followed by one, distinct, in order.
func ScanArticleLinks(b []byte) [][32]byte {
	var out [][32]byte
	seen := map[[32]byte]bool{}
	const p = "terse:"
	for i := 0; i+len(p)+64 <= len(b); i++ {
		if b[i] != 't' || string(b[i:i+len(p)]) != p {
			continue
		}
		if i > 0 && isAlnum(b[i-1]) {
			continue
		}
		h := b[i+len(p) : i+len(p)+64]
		if !terse.IsLowerHex64(string(h)) {
			continue
		}
		if end := i + len(p) + 64; end < len(b) && isAlnum(b[end]) {
			continue
		}
		var id [32]byte
		for k := 0; k < 32; k++ {
			id[k] = unhex(h[2*k])<<4 | unhex(h[2*k+1])
		}
		if !seen[id] {
			seen[id] = true
			out = append(out, id)
		}
	}
	return out
}

func isAlnum(c byte) bool {
	return c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9'
}

func unhex(c byte) byte {
	if c <= '9' {
		return c - '0'
	}
	return c - 'a' + 10
}

// --- getting blobs ---

// Get returns a blob from the store, or fetches it from the relays (and
// stores it). It waits for a first connection if none is up.
func (c *Client) Get(id [32]byte) ([]byte, error) {
	if b, ok := c.st.Blob(id); ok {
		return b, nil
	}
	return c.get(id, 0)
}

// get asks the connected relays in turn, the relays named by the profile
// in force of a feed with an entry pointing at the blob first, then the
// rest, with at most two requests outstanding: as soon as one answers
// without the blob, the next is asked (NETWORK.md 5.1). The first
// verified blob wins. A blob over limit (if not 0) is discarded.
func (c *Client) get(id [32]byte, limit int) ([]byte, error) {
	if err := c.waitForRelays(); err != nil {
		return nil, err
	}
	order := c.getOrder(id)
	if len(order) == 0 {
		return nil, ErrNotFound
	}
	results := make(chan []byte, len(order))
	next, outstanding := 0, 0
	ask := func() {
		cn := order[next]
		next++
		outstanding++
		go func() { results <- cn.getOne(id) }()
	}
	for next < len(order) && outstanding < 2 {
		ask()
	}
	var found []byte
	for outstanding > 0 {
		b := <-results
		outstanding--
		if b != nil {
			found = b
			break
		}
		if next < len(order) {
			ask()
		}
	}
	if found == nil {
		return nil, ErrNotFound
	}
	if limit > 0 && len(found) > limit {
		return nil, fmt.Errorf("%d bytes, over the automatic download limit", len(found))
	}
	if err := c.st.PutBlob(id, found); err != nil {
		return nil, err
	}
	c.signal()
	return found, nil
}

// waitForRelays holds a request up only while no relay is connected: with
// none connecting either, it fails at once (ErrNotFound); otherwise it
// waits until none is still connecting, or until one is connected and
// getGrace has passed since it started waiting, whichever comes first.
func (c *Client) waitForRelays() error {
	start := time.Now()
	for {
		c.mu.Lock()
		connected, connecting := false, false
		for _, cn := range c.conns {
			connected = connected || cn.connected
			connecting = connecting || cn.connecting
		}
		wake := c.wake
		c.mu.Unlock()
		if !connecting {
			if connected {
				return nil
			}
			return ErrNotFound
		}
		var timer <-chan time.Time
		if connected {
			left := getGrace - time.Since(start)
			if left <= 0 {
				return nil
			}
			timer = time.After(left)
		}
		select {
		case <-wake:
		case <-timer:
			return nil
		case <-c.ctx.Done():
			return c.ctx.Err()
		}
	}
}

// getOrder lists the connected relays for a get of id: the preferred ones
// (named by the relay lines of the profile in force of a feed with an
// entry pointing at the blob) first, then the rest, each in the client's
// order.
func (c *Client) getOrder(id [32]byte) []*conn {
	named := map[string]bool{}
	for _, e := range c.st.EntriesForBlob(id) {
		if p, ok := c.st.Profile(e.Key); ok {
			for _, u := range p.Relays {
				named[normURL(u)] = true
			}
		}
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	var a, b []*conn
	for _, cn := range c.conns {
		if !cn.connected {
			continue
		}
		if named[normURL(cn.url)] {
			a = append(a, cn)
		} else {
			b = append(b, cn)
		}
	}
	return append(a, b...)
}

func normURL(u string) string { return strings.TrimSuffix(u, "/") }

// getOne asks one relay; nil if it has not got the blob, it fails, or the
// bytes do not hash to the ID (then it counts as not found there).
func (cn *conn) getOne(id [32]byte) []byte {
	cn.cl.mu.Lock()
	if !cn.connected {
		cn.cl.mu.Unlock()
		return nil
	}
	cn.nextReq++
	req := cn.nextReq
	ch := make(chan *terse.Reply, 1)
	cn.pending[req] = ch
	cn.cl.mu.Unlock()
	if err := cn.send(terse.EncodeGet(req, id)); err != nil {
		return nil
	}
	select {
	case r, ok := <-ch:
		if !ok || r == nil || r.Status != terse.StatusDone {
			return nil
		}
		if len(r.Blob) < 1 || len(r.Blob) > terse.MaxBlob || blake3.Sum256(r.Blob) != id {
			cn.cl.opt.Logf("%s sent a blob that does not match its ID: discarded", cn.url)
			return nil
		}
		return r.Blob
	case <-time.After(getTimeout):
		cn.cl.mu.Lock()
		delete(cn.pending, req)
		cn.cl.mu.Unlock()
		return nil
	case <-cn.cl.ctx.Done():
		return nil
	}
}

// --- subscribing ---

// Subscribe adds a feed and subscribes to it on every connected relay
// that serves it.
func (c *Client) Subscribe(a terse.Address) error {
	if _, err := c.st.Subscribe(a); err != nil {
		return err
	}
	c.mu.Lock()
	conns := append([]*conn(nil), c.conns...)
	c.mu.Unlock()
	for _, cn := range conns {
		c.mu.Lock()
		ok := cn.serves(a.Key) && cn.connected
		c.mu.Unlock()
		if ok {
			cn.subscribe(a.Key)
		}
	}
	c.updateFeedRelays(a.Key)
	c.evaluateHints(a.Key)
	c.signal()
	return nil
}

// Unsubscribe removes a feed and unsubscribes on every connection.
func (c *Client) Unsubscribe(key [32]byte) error {
	c.mu.Lock()
	conns := append([]*conn(nil), c.conns...)
	c.mu.Unlock()
	if _, err := c.st.Unsubscribe(key); err != nil {
		return err
	}
	c.mu.Lock()
	delete(c.hintsOn, key)
	delete(c.heldOnOwn, key)
	c.mu.Unlock()
	for _, cn := range conns {
		cn.unsubscribe(key)
		c.mu.Lock()
		if !cn.own {
			delete(cn.feeds, key)
			if len(cn.feeds) == 0 {
				c.removeConnLocked(cn)
			}
		}
		c.mu.Unlock()
	}
	c.signal()
	return nil
}

// addFeedRelay follows a feed on a feed relay, for the reason via.
func (c *Client) addFeedRelay(u string, key [32]byte, via uint8) {
	c.mu.Lock()
	if via == viaHint && (c.heldOnOwn[key] || !c.hintsOn[key]) {
		c.mu.Unlock()
		return
	}
	for _, cn := range c.conns {
		if cn.own && normURL(cn.url) == normURL(u) {
			c.mu.Unlock()
			return
		}
	}
	cn := c.addConnLocked(u, false)
	if cn == nil {
		c.mu.Unlock()
		return
	}
	was := cn.feeds[key]
	cn.feeds[key] = was | via
	connected := cn.connected
	c.mu.Unlock()
	if was == 0 && connected {
		cn.subscribe(key)
	}
}

// dropFeedRelays stops following a feed on the feed relays that serve it
// for the reason via, except those keep names: a relay that no longer
// serves the feed for any reason is unsubscribed from it, and closed when
// it serves no feed any more.
func (c *Client) dropFeedRelays(key [32]byte, via uint8, keep map[string]bool) {
	c.mu.Lock()
	var drop []*conn
	for _, cn := range c.conns {
		v := cn.feeds[key]
		if cn.own || v&via == 0 || keep[cn.url] {
			continue
		}
		if v &^= via; v != 0 {
			cn.feeds[key] = v
			continue
		}
		delete(cn.feeds, key)
		drop = append(drop, cn)
	}
	c.mu.Unlock()
	for _, cn := range drop {
		cn.unsubscribe(key)
		c.mu.Lock()
		if len(cn.feeds) == 0 {
			c.removeConnLocked(cn)
		}
		c.mu.Unlock()
	}
}

func hostOf(u string) string {
	p, err := url.Parse(u)
	if err != nil {
		return ""
	}
	return p.Hostname()
}

func (c *Client) ownSet() map[string]bool {
	own := map[string]bool{}
	for _, u := range c.ownRelays() {
		own[normURL(u)] = true
	}
	return own
}

// profileRelays returns the relays the profile in force of a feed names
// that the feed is followed on: the first two relay lines that are not
// among the user's relays and whose host is not refused (NETWORK.md 4.9,
// 5.1). A profile whose document is not held names none. While this list
// is not empty, the feed's hints are not used.
func (c *Client) profileRelays(key [32]byte) (chosen []string) {
	p, ok := c.st.Profile(key)
	if !ok || len(p.Relays) == 0 {
		return nil
	}
	own := c.ownSet()
	for _, u := range p.Relays {
		if len(chosen) >= 2 {
			break
		}
		if own[normURL(u)] {
			continue
		}
		if !c.opt.AllowPrivate && !terse.NetworkRelayUsable(u, hostOf(u)) {
			continue
		}
		chosen = append(chosen, u)
	}
	return chosen
}

// updateFeedRelays recomputes, from scratch, the relays a feed is
// followed on because its profile in force names them; relays no longer
// named stop serving it. While it names relays to follow the feed on, the
// hints are not used; once it names none, they come back if they were in
// use (NETWORK.md 4.9, 5.1).
func (c *Client) updateFeedRelays(key [32]byte) {
	if !c.opt.FeedRelays {
		return
	}
	var chosen []string
	if _, ok := c.st.Subscribed(key); ok {
		chosen = c.profileRelays(key)
	}
	keep := map[string]bool{}
	for _, u := range chosen {
		keep[u] = true
	}
	c.dropFeedRelays(key, viaProfile, keep)
	for _, u := range chosen {
		c.addFeedRelay(u, key, viaProfile)
	}
	if len(chosen) > 0 {
		c.dropFeedRelays(key, viaHint, nil)
	} else {
		c.evaluateHints(key)
	}
}

// ownHoldNoneLocked reports whether none of the user's relays holds an
// entry of a feed (NETWORK.md 4.9): there is no own relay; or every own
// relay's last connection attempt failed; or one is connected and every
// connected one answered the subscribe with highest 0 or refused it.
// Relays still connecting do not hold the decision up (c.mu held).
func (c *Client) ownHoldNoneLocked(key [32]byte) bool {
	anyOwn, anyConnected, allFailed := false, false, true
	for _, cn := range c.conns {
		if !cn.own {
			continue
		}
		anyOwn = true
		if cn.connected {
			anyConnected = true
		}
		if cn.connected || cn.connecting || !cn.tried {
			allFailed = false
		}
	}
	if !anyOwn {
		return true
	}
	if !anyConnected {
		return allFailed
	}
	for _, cn := range c.conns {
		if !cn.own || !cn.connected {
			continue
		}
		s := cn.subs[key]
		if s == nil || !s.replied {
			return false
		}
		if !s.refused && s.highest > 0 {
			return false
		}
	}
	return true
}

// evaluateHints follows a feed on its address's hints (at most two) when
// none of the user's relays holds an entry of it. Once in use they stay
// in use, unless an own relay reports entries of the feed (heldOnOwnRelay);
// while the profile in force names relays to follow the feed on
// (profileRelays) they are not used.
func (c *Client) evaluateHints(key [32]byte) {
	if !c.opt.FeedRelays {
		return
	}
	sb, ok := c.st.Subscribed(key)
	if !ok || len(sb.Addr.Hints) == 0 {
		return
	}
	if len(c.profileRelays(key)) > 0 {
		return
	}
	c.mu.Lock()
	use := !c.heldOnOwn[key] && (c.hintsOn[key] || c.ownHoldNoneLocked(key))
	if use {
		c.hintsOn[key] = true
	}
	c.mu.Unlock()
	if !use {
		return
	}
	own := c.ownSet()
	n := 0
	for _, h := range sb.Addr.Hints {
		if n >= 2 {
			break
		}
		u := h.URL()
		if own[normURL(u)] {
			continue
		}
		if !c.opt.AllowPrivate && !h.Encrypted() && !terse.LoopbackHost(h.Host) {
			c.opt.Logf("hint %s refused: a plain (unencrypted) relay named by the network", h.Text)
			continue
		}
		if !c.opt.AllowPrivate && terse.RefusedHost(h.Host) {
			c.opt.Logf("hint %s refused: a local or private address", h.Text)
			continue
		}
		c.addFeedRelay(u, key, viaHint)
		n++
	}
}

// heldOnOwnRelay records that an own relay holds entries of a feed: the
// feed is no longer followed on its hints, and they are not used again
// until it is unsubscribed (NETWORK.md 4.9).
func (c *Client) heldOnOwnRelay(key [32]byte) {
	if _, ok := c.st.Subscribed(key); !ok {
		return
	}
	c.mu.Lock()
	c.heldOnOwn[key] = true
	delete(c.hintsOn, key)
	c.mu.Unlock()
	c.dropFeedRelays(key, viaHint, nil)
}

func (c *Client) afterConnChange() {
	for _, sb := range c.st.Subscriptions() {
		c.evaluateHints(sb.Addr.Key)
	}
	c.signal()
}

// --- state for people and for sync ---

// RelayStatus describes one connection.
type RelayStatus struct {
	URL       string
	Own, Hint bool
	Connected bool
	Feeds     int // feeds it serves (feed relays)
}

// Relays lists the connections.
func (c *Client) Relays() []RelayStatus {
	c.mu.Lock()
	defer c.mu.Unlock()
	var out []RelayStatus
	for _, cn := range c.conns {
		out = append(out, RelayStatus{cn.url, cn.own, cn.hint(), cn.connected, len(cn.feeds)})
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Own && !out[j].Own })
	return out
}

// HintRelays names the hinted relays a feed is followed on.
func (c *Client) HintRelays(key [32]byte) []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	var out []string
	for _, cn := range c.conns {
		if cn.feeds[key]&viaHint != 0 {
			out = append(out, cn.url)
		}
	}
	return out
}

// doneWork ends a unit of work counted in c.work and wakes WaitIdle.
func (c *Client) doneWork() {
	c.mu.Lock()
	c.work--
	c.mu.Unlock()
	c.signal()
}

// Idle reports whether every relay has been tried, every subscription on
// a connected relay has caught up (or was refused), and no download is
// queued or running.
func (c *Client) Idle() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.work > 0 {
		return false
	}
	for _, cn := range c.conns {
		if cn.refused {
			continue
		}
		if !cn.tried {
			return false
		}
		if !cn.connected {
			continue
		}
		for _, sb := range c.st.Subscriptions() {
			if !cn.serves(sb.Addr.Key) {
				continue
			}
			s := cn.subs[sb.Addr.Key]
			if s == nil || !(s.caughtUp || s.refused) {
				return false
			}
		}
	}
	return true
}

// WaitIdle waits until Idle or the timeout; it reports whether it was
// idle. Every step that can start more work (an entry stored, a
// subscribe reply, a failed connection, a download) counts in c.work
// until it has started it, so Idle is not true in between; it must still
// hold for idleSettle, as a margin.
func (c *Client) WaitIdle(timeout time.Duration) bool {
	deadline := time.Now().Add(timeout)
	var since time.Time
	for {
		now := time.Now()
		if c.Idle() {
			if since.IsZero() {
				since = now
			}
			if now.Sub(since) >= idleSettle {
				return true
			}
		} else {
			since = time.Time{}
		}
		if !now.Before(deadline) {
			return false
		}
		time.Sleep(idlePoll)
	}
}
