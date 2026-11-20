// Package local is the local node protocol of LOCAL.md: the messages a
// reader and a node exchange over a local socket, their framing, and
// where the socket is. It holds no cryptography and does no I/O beyond
// the socket, so that a reader built on it alone (a thin reader) needs
// none of the network half.
package local

import (
	"bufio"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"syscall"

	"tersereader/internal/nodeapi"
)

// Message types (LOCAL.md 4). A reply's type is its request's | 80h.
const (
	Get           = 0x01
	GetReply      = 0x81
	Hello         = 0x20
	HelloReply    = 0xa0
	Held          = 0x21
	Timeline      = 0x22
	Subscriptions = 0x23
	Feed          = 0x24
	Subscribe     = 0x25
	Unsubscribe   = 0x26
	Nickname      = 0x27
	Relays        = 0x28
	SetRelays     = 0x29
	Sync          = 0x2a
	Title         = 0x2b
	Changed       = 0xc0
	Log           = 0xc1
)

// Statuses, as on a relay.
const (
	Done     = 1
	NotFound = 2
	Refused  = 3
)

const (
	Version     = 0
	MaxMessage  = 2097152
	MaxTimeline = 1024
)

// Record flags and feed record flags (LOCAL.md 4.3).
const (
	RecHeld     = 1
	RecFullText = 2

	FeedSubscribed = 1
	FeedNickname   = 2

	RelayConfigured = 1
	RelayHint       = 2
	RelayConnected  = 4
)

// IsLocalReply reports whether a reply type carries a reason after
// status 3 (A0h to ABh).
func IsLocalReply(t byte) bool { return t >= HelloReply && t <= Title|0x80 }

// --- Framing (LOCAL.md 2) ---

// WriteFrame writes one message with its 4-byte length.
func WriteFrame(w io.Writer, msg []byte) error {
	if len(msg) > MaxMessage {
		return errors.New("message too long")
	}
	b := make([]byte, 4, 4+len(msg))
	binary.LittleEndian.PutUint32(b, uint32(len(msg)))
	_, err := w.Write(append(b, msg...))
	return err
}

// ReadFrame reads one message.
func ReadFrame(r *bufio.Reader) ([]byte, error) {
	var n [4]byte
	if _, err := io.ReadFull(r, n[:]); err != nil {
		return nil, err
	}
	l := binary.LittleEndian.Uint32(n[:])
	if l > MaxMessage {
		return nil, fmt.Errorf("message of %d bytes is over the limit", l)
	}
	msg := make([]byte, l)
	_, err := io.ReadFull(r, msg)
	return msg, err
}

// --- Building and reading messages ---

// B builds a message.
type B struct{ b []byte }

// New starts a message of type t with req.
func New(t byte, req uint32) *B { return (&B{b: []byte{t}}).U32(req) }

func (m *B) U8(v byte) *B    { m.b = append(m.b, v); return m }
func (m *B) U16(v uint16) *B { m.b = binary.LittleEndian.AppendUint16(m.b, v); return m }
func (m *B) U32(v uint32) *B { m.b = binary.LittleEndian.AppendUint32(m.b, v); return m }
func (m *B) Raw(v []byte) *B { m.b = append(m.b, v...); return m }

// Short appends `n` (1) and at most 255 bytes of s.
func (m *B) Short(s string) *B {
	if len(s) > 255 {
		s = s[:255]
	}
	return m.U8(byte(len(s))).Raw([]byte(s))
}

// Long appends `n2` (2) and at most 65,535 bytes of s.
func (m *B) Long(s string) *B {
	if len(s) > 65535 {
		s = s[:65535]
	}
	return m.U16(uint16(len(s))).Raw([]byte(s))
}

func (m *B) Bytes() []byte { return m.b }

// R reads a message; any read past its end makes it bad.
type R struct {
	b   []byte
	bad bool
}

func NewR(b []byte) *R { return &R{b: b} }

func (r *R) take(n int) []byte {
	if r.bad || n > len(r.b) {
		r.bad = true
		return make([]byte, n)
	}
	v := r.b[:n]
	r.b = r.b[n:]
	return v
}

func (r *R) U8() byte       { return r.take(1)[0] }
func (r *R) U16() uint16    { return binary.LittleEndian.Uint16(r.take(2)) }
func (r *R) U32() uint32    { return binary.LittleEndian.Uint32(r.take(4)) }
func (r *R) N(n int) []byte { return r.take(n) }
func (r *R) Short() string  { return string(r.take(int(r.U8()))) }
func (r *R) Long() string   { return string(r.take(int(r.U16()))) }
func (r *R) Rest() []byte   { v := r.b; r.b = nil; return v }

// Bad reports a read past the end; Done that everything was read, well.
func (r *R) Bad() bool  { return r.bad }
func (r *R) Done() bool { return !r.bad && len(r.b) == 0 }

// Head reads a message's type and req.
func (r *R) Head() (byte, uint32) { return r.U8(), r.U32() }

// --- Records (LOCAL.md 4.3) ---

// PutRecord appends a record for an entry.
func (m *B) PutRecord(e nodeapi.Entry) *B {
	var f byte
	if e.Held {
		f |= RecHeld
	}
	if e.FullText {
		f |= RecFullText
	}
	return m.U32(e.Arrival).U8(f).Short(e.Name).U16(uint16(len(e.Raw))).Raw(e.Raw)
}

// Record reads a record as an entry; the entry's fields come from its
// bytes, read by their layout (NETWORK.md 4.2) and not verified.
func (r *R) Record() nodeapi.Entry {
	arrival := r.U32()
	f := r.U8()
	name := r.Short()
	raw := append([]byte(nil), r.N(int(r.U16()))...)
	e, ok := EntryFields(raw)
	if !ok {
		r.bad = true
	}
	e.Arrival, e.Name, e.Raw = arrival, name, raw
	e.Held, e.FullText = f&RecHeld != 0, f&RecFullText != 0
	e.Label = nodeapi.LabelOf(name, e.Feed)
	return e
}

// EntryFields reads an entry's fields by its layout (NETWORK.md 4.2),
// without verifying it: a reader trusts its node (LOCAL.md 3).
func EntryFields(b []byte) (nodeapi.Entry, bool) {
	if len(b) < 173 {
		return nodeapi.Entry{}, false
	}
	n := int(b[108])
	if n > 120 || len(b) != 173+n {
		return nodeapi.Entry{}, false
	}
	return nodeapi.Entry{
		Feed:  hex.EncodeToString(b[:32]),
		Seq:   binary.LittleEndian.Uint32(b[32:]),
		Time:  binary.LittleEndian.Uint32(b[36:]),
		Kind:  b[40],
		Size:  uint32(b[41]) | uint32(b[42])<<8 | uint32(b[43])<<16,
		Blob:  hex.EncodeToString(b[44:76]),
		Title: string(b[109 : 109+n]),
	}, true
}

// PutFeed appends a feed record.
func (m *B) PutFeed(f nodeapi.Feed) *B {
	k, _ := hex.DecodeString(f.Key)
	var key [32]byte
	copy(key[:], k)
	var fl byte
	if f.Subscribed {
		fl |= FeedSubscribed
	}
	if f.Nickname {
		fl |= FeedNickname
	}
	hints := ""
	if len(f.Address) > 64 && f.Address[64] == '@' {
		hints = f.Address[65:]
	}
	m.Raw(key[:]).U8(fl).Short(f.Name).Short(hints)
	for _, l := range [][]string{f.Hints, f.HintRelays, f.ProfileRelays} {
		if len(l) > 255 {
			l = l[:255]
		}
		m.U8(byte(len(l)))
		for _, u := range l {
			m.Short(u)
		}
	}
	return m.Long(f.Description)
}

// Feed reads a feed record.
func (r *R) Feed() nodeapi.Feed {
	var f nodeapi.Feed
	f.Key = hex.EncodeToString(r.N(32))
	fl := r.U8()
	f.Subscribed, f.Nickname = fl&FeedSubscribed != 0, fl&FeedNickname != 0
	f.Name = r.Short()
	f.Address = f.Key
	if h := r.Short(); h != "" {
		f.Address += "@" + h
	}
	for _, l := range []*[]string{&f.Hints, &f.HintRelays, &f.ProfileRelays} {
		for i, n := 0, int(r.U8()); i < n; i++ {
			*l = append(*l, r.Short())
		}
	}
	f.Description = r.Long()
	f.Label = nodeapi.LabelOf(f.Name, f.Key)
	return f
}

// --- The socket (LOCAL.md 2) ---

// DefaultPath is the node's socket: $TERSE_NODE_SOCKET, else
// $XDG_RUNTIME_DIR/terse/node.sock, else /tmp/terse-<uid>/node.sock.
func DefaultPath() string {
	if p := os.Getenv("TERSE_NODE_SOCKET"); p != "" {
		return p
	}
	if d := os.Getenv("XDG_RUNTIME_DIR"); d != "" {
		return filepath.Join(d, "terse", "node.sock")
	}
	return filepath.Join(os.TempDir(), "terse-"+strconv.Itoa(os.Getuid()), "node.sock")
}

// checkDir checks that the socket's directory belongs to the user and is
// not open to others (LOCAL.md 2); create makes it first if it is missing.
func checkDir(dir string, create bool) error {
	if create {
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return err
		}
	}
	fi, err := os.Stat(dir)
	if err != nil {
		return err
	}
	if fi.Mode().Perm()&0o077 != 0 {
		return fmt.Errorf("%s is open to other users (mode %o): refusing to use it", dir, fi.Mode().Perm())
	}
	if st, ok := fi.Sys().(*syscall.Stat_t); ok && int(st.Uid) != os.Getuid() {
		return fmt.Errorf("%s belongs to another user: refusing to use it", dir)
	}
	return nil
}

// ErrRunning is returned by Listen when a node already serves the path.
var ErrRunning = errors.New("a node is already running on this socket")

// Listen opens the node's socket at path: the directory 0700 and the
// user's, a stale socket file removed, the socket 0600.
func Listen(path string) (net.Listener, error) {
	if err := checkDir(filepath.Dir(path), true); err != nil {
		return nil, err
	}
	if _, err := os.Lstat(path); err == nil {
		if c, err := net.Dial("unix", path); err == nil {
			c.Close()
			return nil, ErrRunning
		}
		if err := os.Remove(path); err != nil {
			return nil, err
		}
	}
	ln, err := net.Listen("unix", path)
	if err != nil {
		return nil, err
	}
	if err := os.Chmod(path, 0o600); err != nil {
		ln.Close()
		return nil, err
	}
	return ln, nil
}

// Dial connects to the node's socket at path, after checking its
// directory.
func Dial(path string) (net.Conn, error) {
	if err := checkDir(filepath.Dir(path), false); err != nil {
		return nil, err
	}
	return net.Dial("unix", path)
}
