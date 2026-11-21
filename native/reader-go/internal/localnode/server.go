// Package localnode serves a node (any nodeapi.Node) to readers over the
// local node protocol of LOCAL.md.
package localnode

import (
	"bufio"
	"encoding/hex"
	"net"
	"sync"
	"time"

	"tersereader/internal/local"
	"tersereader/internal/nodeapi"
)

// Server serves one node on one listener.
type Server struct {
	n     nodeapi.Node
	ln    net.Listener
	mu    sync.Mutex
	conns map[*conn]bool
	done  chan struct{}
	wg    sync.WaitGroup
}

type conn struct {
	c     net.Conn
	wmu   sync.Mutex
	hello bool
}

// Serve starts serving n on ln; it returns at once.
func Serve(n nodeapi.Node, ln net.Listener) *Server {
	s := &Server{n: n, ln: ln, conns: map[*conn]bool{}, done: make(chan struct{})}
	s.wg.Add(2)
	go s.accept()
	go s.pushes()
	return s
}

// Close stops listening and closes every connection.
func (s *Server) Close() {
	close(s.done)
	s.ln.Close()
	s.mu.Lock()
	for c := range s.conns {
		c.c.Close()
	}
	s.mu.Unlock()
	s.wg.Wait()
}

func (s *Server) accept() {
	defer s.wg.Done()
	for {
		c, err := s.ln.Accept()
		if err != nil {
			select {
			case <-s.done:
				return
			default:
			}
			time.Sleep(10 * time.Millisecond)
			continue
		}
		cn := &conn{c: c}
		s.mu.Lock()
		s.conns[cn] = true
		s.mu.Unlock()
		go s.serve(cn)
	}
}

// pushes passes the node's changes and log messages to every reader that
// has said Hello (LOCAL.md 4.4).
func (s *Server) pushes() {
	defer s.wg.Done()
	for {
		select {
		case <-s.done:
			return
		case <-s.n.Changed():
			s.broadcast([]byte{local.Changed})
		case m := <-s.n.Log():
			s.broadcast((&local.B{}).U8(local.Log).Long(m).Bytes())
		}
	}
}

func (s *Server) broadcast(msg []byte) {
	s.mu.Lock()
	var to []*conn
	for c := range s.conns {
		if c.hello {
			to = append(to, c)
		}
	}
	s.mu.Unlock()
	for _, c := range to {
		c.send(msg)
	}
}

func (c *conn) send(msg []byte) {
	c.wmu.Lock()
	defer c.wmu.Unlock()
	c.c.SetWriteDeadline(time.Now().Add(10 * time.Second))
	if local.WriteFrame(c.c, msg) != nil {
		c.c.Close()
	}
}

func (s *Server) serve(c *conn) {
	defer func() {
		c.c.Close()
		s.mu.Lock()
		delete(s.conns, c)
		s.mu.Unlock()
	}()
	r := bufio.NewReader(c.c)
	for {
		msg, err := local.ReadFrame(r)
		if err != nil || len(msg) == 0 {
			return
		}
		if !c.hello {
			// The first message must be a well-formed Hello (LOCAL.md 4.1).
			in := local.NewR(msg)
			t, req := in.Head()
			v := in.U8()
			in.Short()
			if t != local.Hello || !in.Done() {
				return
			}
			if v != local.Version {
				c.send(refused(local.HelloReply, req, "version not spoken"))
				continue
			}
			s.mu.Lock()
			c.hello = true
			s.mu.Unlock()
			c.send(local.New(local.HelloReply, req).U8(local.Done).U8(local.Version).U8(0).Bytes())
			continue
		}
		reply, changed, ok := s.answer(msg)
		if !ok {
			return // a known type it cannot parse: close (LOCAL.md 3)
		}
		if reply == nil {
			continue
		}
		// Slow requests (Get, Sync) must not hold up the others.
		go func() {
			out := reply()
			if out != nil {
				c.send(out)
			}
			if changed {
				s.broadcast([]byte{local.Changed})
			}
		}()
	}
}

// withRecords ends a reply with `count` (2) and the records (or feed
// records) in parts: as many as fit in a message (LOCAL.md 4.3, a reply's
// size), the first ones, or with latest the last ones (still in order).
func withRecords(m *local.B, parts [][]byte, latest bool) []byte {
	size := len(m.Bytes()) + 2
	n := 0
	for n < len(parts) && n < 65535 {
		p := parts[n]
		if latest {
			p = parts[len(parts)-1-n]
		}
		if size+len(p) > local.MaxMessage {
			break
		}
		size += len(p)
		n++
	}
	if latest {
		parts = parts[len(parts)-n:]
	} else {
		parts = parts[:n]
	}
	m.U16(uint16(n))
	for _, p := range parts {
		m.Raw(p)
	}
	return m.Bytes()
}

func refused(t byte, req uint32, why string) []byte {
	return local.New(t, req).U8(local.Refused).Short(why).Bytes()
}

// answer parses a request and returns a function that carries it out and
// builds the reply; nil for a type it ignores; ok false if malformed.
func (s *Server) answer(msg []byte) (reply func() []byte, changed bool, ok bool) {
	n := s.n
	in := local.NewR(msg)
	t, req := in.Head()
	rt := t | 0x80
	status := func(err error, okStatus byte) []byte {
		if err != nil {
			return refused(rt, req, err.Error())
		}
		return local.New(rt, req).U8(okStatus).Bytes()
	}
	switch t {
	case local.Get, local.Held:
		id := hex.EncodeToString(in.N(32))
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte {
			var b []byte
			var have bool
			if t == local.Held {
				b, have = n.HeldBlob(id)
			} else {
				var err error
				b, err = n.Blob(id)
				have = err == nil
			}
			if !have {
				return local.New(rt, req).U8(local.NotFound).Bytes()
			}
			return local.New(rt, req).U8(local.Done).Raw(b).Bytes()
		}, false, true
	case local.Timeline:
		before, max := in.U32(), int(in.U16())
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte {
			if max > local.MaxTimeline {
				max = local.MaxTimeline
			}
			var recs []nodeapi.Entry
			for _, e := range n.Timeline() {
				if len(recs) >= max {
					break
				}
				if before == 0 || e.Arrival < before {
					recs = append(recs, e)
				}
			}
			var parts [][]byte
			for _, e := range recs {
				parts = append(parts, (&local.B{}).PutRecord(e).Bytes())
			}
			return withRecords(local.New(rt, req).U8(local.Done), parts, false)
		}, false, true
	case local.Subscriptions:
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte {
			var parts [][]byte
			for _, f := range n.Subscriptions() {
				parts = append(parts, (&local.B{}).PutFeed(f).Bytes())
			}
			return withRecords(local.New(rt, req).U8(local.Done), parts, false)
		}, false, true
	case local.Feed:
		flags := in.U8()
		addr := in.Short()
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte {
			f, err := n.Feed(addr)
			if err != nil {
				return refused(rt, req, err.Error())
			}
			m := local.New(rt, req).U8(local.Done).PutFeed(f)
			if flags&1 == 0 {
				return m.Bytes()
			}
			var parts [][]byte
			for _, e := range f.Entries {
				parts = append(parts, (&local.B{}).PutRecord(e).Bytes())
			}
			return withRecords(m, parts, true)
		}, false, true
	case local.Subscribe:
		addr := in.Short()
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte {
			isNew, err := n.Subscribe(addr)
			if err != nil {
				return refused(rt, req, err.Error())
			}
			var v byte
			if isNew {
				v = 1
			}
			return local.New(rt, req).U8(local.Done).U8(v).Bytes()
		}, true, true
	case local.Unsubscribe:
		key := hex.EncodeToString(in.N(32))
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte {
			// Unsubscribing from a feed not subscribed succeeds, whatever
			// the key (LOCAL.md 4.3).
			for _, f := range n.Subscriptions() {
				if f.Key == key {
					return status(n.Unsubscribe(key), local.Done)
				}
			}
			return status(nil, local.Done)
		}, true, true
	case local.Nickname:
		key := hex.EncodeToString(in.N(32))
		name := in.Short()
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte { return status(n.SetNickname(key, name), local.Done) }, true, true
	case local.Relays:
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte {
			type rel struct {
				url string
				f   byte
			}
			var list []rel
			conns := n.Relays()
			flagsOf := func(r nodeapi.Relay) byte {
				var f byte
				if r.Own {
					f |= local.RelayConfigured
				}
				if r.Hint {
					f |= local.RelayHint
				}
				if r.Connected {
					f |= local.RelayConnected
				}
				return f
			}
			seen := map[string]bool{}
			for _, u := range n.ConfiguredRelays() {
				f := byte(local.RelayConfigured)
				for _, r := range conns {
					if r.URL == u && r.Own && r.Connected {
						f |= local.RelayConnected
					}
				}
				list = append(list, rel{u, f})
				seen[u] = true
			}
			for _, r := range conns {
				if !seen[r.URL] {
					list = append(list, rel{r.URL, flagsOf(r)})
				}
			}
			if len(list) > 255 {
				list = list[:255]
			}
			m := local.New(rt, req).U8(local.Done).U8(byte(len(list)))
			for _, r := range list {
				m.U8(r.f).Short(r.url)
			}
			return m.Bytes()
		}, false, true
	case local.SetRelays:
		count := int(in.U8())
		var urls []string
		for i := 0; i < count; i++ {
			urls = append(urls, in.Short())
		}
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte { return status(n.SetRelays(urls), local.Done) }, true, true
	case local.Sync:
		ms := in.U32()
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte {
			all, err := n.Sync(time.Duration(ms) * time.Millisecond)
			if err != nil {
				return refused(rt, req, err.Error())
			}
			if all {
				return status(nil, local.Done)
			}
			return status(nil, local.NotFound)
		}, false, true
	case local.Title:
		id := hex.EncodeToString(in.N(32))
		size := in.U32()
		if !in.Done() {
			return nil, false, false
		}
		return func() []byte {
			t, ok := n.ArticleTitle(id, int(size))
			if !ok {
				return status(nil, local.NotFound)
			}
			return local.New(rt, req).U8(local.Done).Short(t).Bytes()
		}, false, true
	case local.Hello:
		return nil, false, false // only once
	}
	return nil, false, true // a type it does not know: ignored
}
