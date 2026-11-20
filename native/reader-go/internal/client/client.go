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
	maxConns     = 16
	maxOwn       = 8
	prefetchMax  = 8
	getTimeout   = 30 * time.Second
	dialTimeout  = 10 * time.Second
	firstBackoff = time.Second
	maxBackoff   = 30 * time.Second
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

type conn struct {
	url  string
	own  bool
	hint bool // a feed relay taken from an address's hints
	cl   *Client

	// guarded by cl.mu
	feeds     map[[32]byte]bool // feeds a feed relay serves (own relays serve all)
	ws        *ws.Conn
	connected bool
	tried     bool // a connection attempt has finished, one way or the other
	refused   bool // its host is refused (NETWORK.md 5.1): never connected
	nextReq   uint32
	pending   map[uint32]chan *terse.Reply
	subReq    map[uint32][32]byte
	subs      map[[32]byte]*subState
	stop      chan struct{}
	stopped   bool
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
	anyConn  chan struct{} // closed when a first connection is made
	anyOnce  sync.Once
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
		fetching: map[[32]byte]bool{},
		jobs:     make(chan job, 4096),
		changed:  make(chan struct{}, 1),
		anyConn:  make(chan struct{}),
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

// Start connects to the configured relays and starts the downloaders.
func (c *Client) ownRelays() []string {
	if c.opt.Relays != nil {
		return c.opt.Relays
	}
	return c.st.Relays()
}

func (c *Client) Start() {
	relays := c.ownRelays()
	if len(relays) > maxOwn {
		relays = relays[:maxOwn]
	}
	c.mu.Lock()
	for _, u := range relays {
		c.addConnLocked(u, true, false)
	}
	c.mu.Unlock()
	for i := 0; i < 3; i++ {
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

func (c *Client) addConnLocked(u string, own, hint bool) *conn {
	if cn := c.findConnLocked(u); cn != nil {
		return cn
	}
	if len(c.conns) >= maxConns {
		c.opt.Logf("not connecting to %s: %d connections already", u, maxConns)
		return nil
	}
	cn := &conn{
		url: u, own: own, hint: hint, cl: c,
		feeds:   map[[32]byte]bool{},
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
	return cn.feeds[key]
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
		w, err := cn.dial()
		cl := cn.cl
		if err != nil {
			cl.mu.Lock()
			cn.tried = true
			cl.mu.Unlock()
			cl.opt.Logf("%s: %v", cn.url, err)
			cl.afterConnChange()
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
		if cn.stopped {
			cl.mu.Unlock()
			w.Close()
			return
		}
		cn.ws, cn.connected, cn.tried = w, true, true
		cl.mu.Unlock()
		cl.anyOnce.Do(func() { close(cl.anyConn) })
		cl.opt.Logf("connected to %s", cn.url)
		cn.subscribeAll()
		cn.readLoop(w)
		cl.mu.Lock()
		cn.connected = false
		cn.ws = nil
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
		return ws.Dial(cn.url, "", dialTimeout)
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
	return ws.Dial(cn.url, net.JoinHostPort(addr, port), dialTimeout)
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
			if ok && s != nil {
				s.replied = true
				if r.Status != terse.StatusDone {
					s.refused = true
				} else {
					s.highest = r.Highest
					if r.Highest <= s.after {
						s.caughtUp = true
					}
				}
			}
			cn.cl.mu.Unlock()
			if ok {
				cn.cl.evaluateHints(key)
				cn.cl.signal()
			}
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
		c.mu.Lock()
		serves := cn.serves(e.Key)
		c.mu.Unlock()
		if serves {
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
			c.work--
			c.mu.Unlock()
			c.signal()
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

// prefetch gets the first 8 distinct article links of an automatically
// downloaded article that are not held or being fetched (NETWORK.md 4.4).
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
		skip := false
		for _, e := range c.st.EntriesForBlob(id) {
			if e.Kind == terse.KindProgram || int(e.Size) > c.opt.AutoLimit {
				skip = true
			}
		}
		if skip {
			continue
		}
		if c.enqueue(job{id: id, prefetch: true}) {
			n++
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

// get asks the connected relays in turn, feed relays of the feeds with
// an entry pointing at the blob first, then the rest, two at a time
// (NETWORK.md 5.1). A blob over limit (if not 0) is discarded.
func (c *Client) get(id [32]byte, limit int) ([]byte, error) {
	order := c.getOrder(id)
	if len(order) == 0 {
		// Only while none is connected does a request wait, for the first
		// connection and briefly for the others.
		select {
		case <-c.anyConn:
			time.Sleep(300 * time.Millisecond)
		case <-time.After(dialTimeout):
		case <-c.ctx.Done():
			return nil, c.ctx.Err()
		}
		order = c.getOrder(id)
		if len(order) == 0 {
			return nil, errors.New("no relay connected")
		}
	}
	for i := 0; i < len(order); i += 2 {
		batch := order[i:min(i+2, len(order))]
		results := make(chan []byte, len(batch))
		for _, cn := range batch {
			go func(cn *conn) { results <- cn.getOne(id) }(cn)
		}
		var found []byte
		for range batch {
			if b := <-results; b != nil && found == nil {
				found = b
			}
		}
		if found != nil {
			if limit > 0 && len(found) > limit {
				return nil, fmt.Errorf("%d bytes, over the automatic download limit", len(found))
			}
			if err := c.st.PutBlob(id, found); err != nil {
				return nil, err
			}
			c.signal()
			return found, nil
		}
	}
	return nil, ErrNotFound
}

func (c *Client) getOrder(id [32]byte) []*conn {
	c.mu.Lock()
	defer c.mu.Unlock()
	first := map[*conn]bool{}
	for _, e := range c.st.EntriesForBlob(id) {
		for _, cn := range c.conns {
			if !cn.own && cn.feeds[e.Key] {
				first[cn] = true
			}
		}
	}
	var a, b []*conn
	for _, cn := range c.conns {
		if !cn.connected {
			continue
		}
		if first[cn] {
			a = append(a, cn)
		} else {
			b = append(b, cn)
		}
	}
	return append(a, b...)
}

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

// addFeedRelay follows a feed on a feed relay.
func (c *Client) addFeedRelay(u string, key [32]byte, hint bool) {
	c.mu.Lock()
	for _, cn := range c.conns {
		if cn.url == u && cn.own {
			c.mu.Unlock()
			return
		}
	}
	cn := c.addConnLocked(u, false, hint)
	if cn == nil {
		c.mu.Unlock()
		return
	}
	isNew := !cn.feeds[key]
	cn.feeds[key] = true
	connected := cn.connected
	c.mu.Unlock()
	if isNew && connected {
		cn.subscribe(key)
	}
}

func hostOf(u string) string {
	p, err := url.Parse(u)
	if err != nil {
		return ""
	}
	return p.Hostname()
}

// updateFeedRelays follows a feed on up to two relays its profile in
// force names that are not among the user's relays; hints are then no
// longer used for it (NETWORK.md 4.9, 5.1).
func (c *Client) updateFeedRelays(key [32]byte) {
	if !c.opt.FeedRelays {
		return
	}
	if _, ok := c.st.Subscribed(key); !ok {
		return
	}
	p, ok := c.st.Profile(key)
	if !ok || len(p.Relays) == 0 {
		return
	}
	own := map[string]bool{}
	for _, u := range c.ownRelays() {
		own[strings.TrimSuffix(u, "/")] = true
	}
	n := 0
	var chosen []string
	for _, u := range p.Relays {
		if n >= 2 {
			break
		}
		if own[strings.TrimSuffix(u, "/")] {
			continue
		}
		if !c.opt.AllowPrivate && terse.RefusedHost(hostOf(u)) {
			continue
		}
		chosen = append(chosen, u)
		n++
	}
	// Drop the hint relays of this feed.
	c.mu.Lock()
	var drop []*conn
	for _, cn := range c.conns {
		if cn.hint && cn.feeds[key] {
			drop = append(drop, cn)
		}
	}
	c.mu.Unlock()
	for _, cn := range drop {
		cn.unsubscribe(key)
		c.mu.Lock()
		delete(cn.feeds, key)
		if len(cn.feeds) == 0 {
			c.removeConnLocked(cn)
		}
		c.mu.Unlock()
	}
	for _, u := range chosen {
		c.addFeedRelay(u, key, false)
	}
}

// evaluateHints follows a feed on its address's hints when none of the
// user's relays holds an entry of it: every connected one answered the
// subscribe with highest 0, or there is none, or none can be reached.
func (c *Client) evaluateHints(key [32]byte) {
	if !c.opt.FeedRelays {
		return
	}
	sb, ok := c.st.Subscribed(key)
	if !ok || len(sb.Addr.Hints) == 0 {
		return
	}
	if p, ok := c.st.Profile(key); ok && len(p.Relays) > 0 {
		return
	}
	c.mu.Lock()
	use := true
	for _, cn := range c.conns {
		if !cn.own {
			continue
		}
		if !cn.tried {
			use = false
			break
		}
		if cn.connected {
			s := cn.subs[key]
			if s == nil || !s.replied || (s.highest > 0 && !s.refused) {
				use = false
				break
			}
		}
	}
	c.mu.Unlock()
	if !use {
		return
	}
	for i, h := range sb.Addr.Hints {
		if i >= 2 {
			break
		}
		if !c.opt.AllowPrivate && terse.RefusedHost(h.Host) {
			c.opt.Logf("hint %s refused: a local or private address", h.Text)
			continue
		}
		c.addFeedRelay(h.URL(), key, true)
	}
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
		out = append(out, RelayStatus{cn.url, cn.own, cn.hint, cn.connected, len(cn.feeds)})
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
		if cn.hint && cn.feeds[key] {
			out = append(out, cn.url)
		}
	}
	return out
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

// WaitIdle waits until Idle or the timeout; it reports whether it was idle.
func (c *Client) WaitIdle(timeout time.Duration) bool {
	deadline := time.Now().Add(timeout)
	stable := 0
	for time.Now().Before(deadline) {
		if c.Idle() {
			stable++
			if stable >= 3 {
				return true
			}
		} else {
			stable = 0
		}
		time.Sleep(100 * time.Millisecond)
	}
	return false
}
