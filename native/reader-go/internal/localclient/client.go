// Package localclient is a nodeapi.Node that is a running node reached
// over its local socket (LOCAL.md). It is all a thin reader links of the
// network side: no cryptography, no hashing, no store, no WebSocket.
package localclient

import (
	"bufio"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"net"
	"sync"
	"time"

	"tersereader/internal/local"
	"tersereader/internal/nodeapi"
)

// Client is a connection to a node.
type Client struct {
	c       net.Conn
	wmu     sync.Mutex
	mu      sync.Mutex
	next    uint32
	wait    map[uint32]chan []byte
	changed chan struct{}
	logs    chan string
	closed  chan struct{}
	once    sync.Once
}

var _ nodeapi.Node = (*Client)(nil)

// ErrClosed is returned once the connection to the node is gone.
var ErrClosed = errors.New("the connection to the node was closed")

// Dial connects to the node at the socket path and says Hello.
func Dial(path string) (*Client, error) {
	c, err := local.Dial(path)
	if err != nil {
		return nil, err
	}
	return New(c)
}

// New says Hello on an open connection.
func New(c net.Conn) (*Client, error) {
	cl := &Client{c: c, wait: map[uint32]chan []byte{}, changed: make(chan struct{}, 1),
		logs: make(chan string, 64), closed: make(chan struct{})}
	go cl.read()
	in, err := cl.call(local.New(local.Hello, 0).U8(local.Version).Short(""), 10*time.Second)
	if err != nil {
		cl.Close()
		return nil, err
	}
	if err := cl.status(in); err != nil {
		cl.Close()
		return nil, err
	}
	return cl, nil
}

func (cl *Client) read() {
	r := bufio.NewReader(cl.c)
	defer cl.Close()
	for {
		msg, err := local.ReadFrame(r)
		if err != nil {
			return
		}
		if len(msg) == 0 {
			continue
		}
		switch msg[0] {
		case local.Changed:
			select {
			case cl.changed <- struct{}{}:
			default:
			}
			continue
		case local.Log:
			in := local.NewR(msg[1:])
			if s := in.Long(); !in.Bad() {
				select {
				case cl.logs <- s:
				default:
				}
			}
			continue
		}
		if len(msg) < 5 {
			continue
		}
		req := binary.LittleEndian.Uint32(msg[1:])
		cl.mu.Lock()
		ch := cl.wait[req]
		delete(cl.wait, req)
		cl.mu.Unlock()
		if ch != nil {
			ch <- msg
		}
	}
}

// call sends a request (its req is set here) and waits for the reply;
// it returns the reply after its type and req.
func (cl *Client) call(m *local.B, timeout time.Duration) (*local.R, error) {
	cl.mu.Lock()
	cl.next++
	req := cl.next
	ch := make(chan []byte, 1)
	cl.wait[req] = ch
	cl.mu.Unlock()
	msg := m.Bytes()
	binary.LittleEndian.PutUint32(msg[1:], req)
	cl.wmu.Lock()
	err := local.WriteFrame(cl.c, msg)
	cl.wmu.Unlock()
	if err != nil {
		cl.Close()
		return nil, ErrClosed
	}
	var t <-chan time.Time
	if timeout > 0 {
		t = time.After(timeout)
	}
	select {
	case reply := <-ch:
		in := local.NewR(reply)
		if rt, _ := in.Head(); rt != msg[0]|0x80 {
			return nil, errors.New("the node answered with the wrong type")
		}
		return in, nil
	case <-cl.closed:
		return nil, ErrClosed
	case <-t:
		cl.mu.Lock()
		delete(cl.wait, req)
		cl.mu.Unlock()
		return nil, errors.New("the node did not answer")
	}
}

// reqTimeout bounds every request but Get and Sync.
const reqTimeout = 30 * time.Second

// status reads a reply's status: nil for done, ErrNotFound for not
// found, the node's reason for a refusal.
func (cl *Client) status(in *local.R) error {
	switch in.U8() {
	case local.Done:
		return nil
	case local.NotFound:
		return ErrNotFound
	case local.Refused:
		if why := in.Short(); why != "" && !in.Bad() {
			return errors.New(why)
		}
		return errors.New("refused by the node")
	}
	return errors.New("a malformed reply from the node")
}

// ErrNotFound is a reply with status 2.
var ErrNotFound = errors.New("not found")

func (cl *Client) simple(m *local.B, timeout time.Duration) (*local.R, error) {
	in, err := cl.call(m, timeout)
	if err != nil {
		return nil, err
	}
	return in, cl.status(in)
}

// Connect does nothing: the node connects to its relays itself.
func (cl *Client) Connect() error { return nil }

func (cl *Client) Sync(timeout time.Duration) (bool, error) {
	ms := timeout.Milliseconds()
	if ms > 1<<32-1 {
		ms = 1<<32 - 1
	}
	_, err := cl.simple(local.New(local.Sync, 0).U32(uint32(ms)), timeout+reqTimeout)
	if errors.Is(err, ErrNotFound) {
		return false, nil
	}
	return err == nil, err
}

func (cl *Client) Changed() <-chan struct{} { return cl.changed }
func (cl *Client) Log() <-chan string       { return cl.logs }

func (cl *Client) Timeline() []nodeapi.Entry {
	var out []nodeapi.Entry
	var before uint32
	for {
		in, err := cl.simple(local.New(local.Timeline, 0).U32(before).U16(local.MaxTimeline), reqTimeout)
		if err != nil {
			return out
		}
		n := int(in.U16())
		for i := 0; i < n && !in.Bad(); i++ {
			out = append(out, in.Record())
		}
		if in.Bad() || n < local.MaxTimeline || len(out) == 0 {
			return out
		}
		before = out[len(out)-1].Arrival
		if before <= 1 {
			return out
		}
	}
}

func (cl *Client) Subscriptions() []nodeapi.Feed {
	in, err := cl.simple(local.New(local.Subscriptions, 0), reqTimeout)
	if err != nil {
		return nil
	}
	var out []nodeapi.Feed
	for i, n := 0, int(in.U16()); i < n && !in.Bad(); i++ {
		out = append(out, in.Feed())
	}
	return out
}

func (cl *Client) feed(address string, entries bool) (nodeapi.Feed, error) {
	var fl byte
	if entries {
		fl = 1
	}
	in, err := cl.simple(local.New(local.Feed, 0).U8(fl).Short(address), reqTimeout)
	if err != nil {
		return nodeapi.Feed{}, err
	}
	f := in.Feed()
	if entries {
		for i, n := 0, int(in.U16()); i < n && !in.Bad(); i++ {
			f.Entries = append(f.Entries, in.Record())
		}
	}
	if in.Bad() {
		return nodeapi.Feed{}, errors.New("a malformed reply from the node")
	}
	return f, nil
}

func (cl *Client) Feed(address string) (nodeapi.Feed, error) { return cl.feed(address, true) }

func (cl *Client) Label(key string) string {
	f, err := cl.feed(key, false)
	if err != nil {
		return nodeapi.LabelOf("", key)
	}
	return f.Label
}

func (cl *Client) Subscribe(address string) (bool, error) {
	in, err := cl.simple(local.New(local.Subscribe, 0).Short(address), reqTimeout)
	if err != nil {
		return false, err
	}
	return in.U8() == 1, nil
}

func keyBytes(key string) ([]byte, error) {
	k, err := hex.DecodeString(key)
	if err != nil || len(k) != 32 || hex.EncodeToString(k) != key {
		return nil, errors.New("not a feed key: 64 lowercase hex digits")
	}
	return k, nil
}

func (cl *Client) Unsubscribe(key string) error {
	k, err := keyBytes(key)
	if err != nil {
		return err
	}
	_, err = cl.simple(local.New(local.Unsubscribe, 0).Raw(k), reqTimeout)
	return err
}

func (cl *Client) SetNickname(key, name string) error {
	k, err := keyBytes(key)
	if err != nil {
		return err
	}
	if len(name) > 255 {
		return errors.New("a nickname is at most 48 bytes")
	}
	_, err = cl.simple(local.New(local.Nickname, 0).Raw(k).Short(name), reqTimeout)
	return err
}

func (cl *Client) blob(t byte, id string, timeout time.Duration) ([]byte, error) {
	k, err := hex.DecodeString(id)
	if err != nil || len(k) != 32 {
		return nil, errors.New("not a blob ID: 64 lowercase hex digits")
	}
	in, err := cl.simple(local.New(t, 0).Raw(k), timeout)
	if err != nil {
		return nil, err
	}
	return in.Rest(), nil
}

func (cl *Client) HeldBlob(id string) ([]byte, bool) {
	b, err := cl.blob(local.Held, id, reqTimeout)
	return b, err == nil
}

func (cl *Client) Blob(id string) ([]byte, error) {
	b, err := cl.blob(local.Get, id, 0)
	if errors.Is(err, ErrNotFound) {
		return nil, errors.New("not found on any relay")
	}
	return b, err
}

func (cl *Client) ArticleTitle(id string, size int) (string, bool) {
	k, err := hex.DecodeString(id)
	if err != nil || len(k) != 32 || size < 0 {
		return "", false
	}
	in, err := cl.simple(local.New(local.Title, 0).Raw(k).U32(uint32(size)), reqTimeout)
	if err != nil {
		return "", false
	}
	t := in.Short()
	return t, !in.Bad()
}

func (cl *Client) relays() ([]nodeapi.Relay, error) {
	in, err := cl.simple(local.New(local.Relays, 0), reqTimeout)
	if err != nil {
		return nil, err
	}
	var out []nodeapi.Relay
	for i, n := 0, int(in.U8()); i < n && !in.Bad(); i++ {
		f := in.U8()
		u := in.Short()
		out = append(out, nodeapi.Relay{URL: u, Own: f&local.RelayConfigured != 0,
			Hint: f&local.RelayHint != 0, Connected: f&local.RelayConnected != 0})
	}
	return out, nil
}

func (cl *Client) Relays() []nodeapi.Relay {
	r, _ := cl.relays()
	return r
}

func (cl *Client) ConfiguredRelays() []string {
	r, _ := cl.relays()
	var out []string
	for _, x := range r {
		if x.Own {
			out = append(out, x.URL)
		}
	}
	return out
}

func (cl *Client) SetRelays(urls []string) error {
	if len(urls) > 255 {
		return errors.New("at most eight relays")
	}
	m := local.New(local.SetRelays, 0).U8(byte(len(urls)))
	for _, u := range urls {
		m.Short(u)
	}
	_, err := cl.simple(m, reqTimeout)
	return err
}

// Close closes the connection; the node goes on running.
func (cl *Client) Close() {
	cl.once.Do(func() {
		close(cl.closed)
		cl.c.Close()
	})
}
