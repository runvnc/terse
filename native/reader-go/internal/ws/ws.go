// Package ws is a minimal RFC 6455 WebSocket implementation: a client
// (ws:// only) and, for tests, a server side. It carries binary messages,
// answers pings and closes cleanly.
package ws

import (
	"bufio"
	"crypto/rand"
	"crypto/sha1"
	"crypto/tls"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

const guid = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

// MaxMessage bounds a received message (a blob of 1 MiB and its header).
const MaxMessage = 1<<20 + 64

const (
	opCont   = 0
	opText   = 1
	opBinary = 2
	opClose  = 8
	opPing   = 9
	opPong   = 10
)

// ErrText is returned when the peer sends a text message.
var ErrText = errors.New("ws: text message")

// Conn is a WebSocket connection.
type Conn struct {
	c      net.Conn
	r      *bufio.Reader
	client bool // client frames are masked
	wmu    sync.Mutex
	closed bool
}

func acceptKey(key string) string {
	h := sha1.Sum([]byte(key + guid))
	return base64.StdEncoding.EncodeToString(h[:])
}

// Dial opens a client connection to a ws:// URL. If addr is not empty it
// is the address (host:port) to connect to, already checked by the
// caller, instead of resolving the URL's host again.
func Dial(rawurl, addr string, timeout time.Duration) (*Conn, error) {
	u, err := url.Parse(rawurl)
	if err != nil {
		return nil, err
	}
	if u.Scheme != "ws" && u.Scheme != "wss" {
		return nil, fmt.Errorf("ws: unsupported scheme %q", u.Scheme)
	}
	host := u.Host
	if u.Port() == "" {
		port := "80"
		if u.Scheme == "wss" {
			port = "443"
		}
		host = net.JoinHostPort(u.Hostname(), port)
	}
	if addr == "" {
		addr = host
	}
	c, err := net.DialTimeout("tcp", addr, timeout)
	if err != nil {
		return nil, err
	}
	if u.Scheme == "wss" {
		tc := tls.Client(c, &tls.Config{ServerName: u.Hostname()})
		tc.SetDeadline(time.Now().Add(timeout))
		if err := tc.Handshake(); err != nil {
			c.Close()
			return nil, err
		}
		c = tc
	}
	c.SetDeadline(time.Now().Add(timeout))
	var kb [16]byte
	rand.Read(kb[:])
	key := base64.StdEncoding.EncodeToString(kb[:])
	path := u.RequestURI()
	if path == "" {
		path = "/"
	}
	req := "GET " + path + " HTTP/1.1\r\nHost: " + u.Host + "\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
		"Sec-WebSocket-Key: " + key + "\r\nSec-WebSocket-Version: 13\r\n\r\n"
	if _, err := io.WriteString(c, req); err != nil {
		c.Close()
		return nil, err
	}
	r := bufio.NewReader(c)
	resp, err := http.ReadResponse(r, nil)
	if err != nil {
		c.Close()
		return nil, err
	}
	if resp.StatusCode != http.StatusSwitchingProtocols ||
		!strings.EqualFold(resp.Header.Get("Upgrade"), "websocket") ||
		resp.Header.Get("Sec-WebSocket-Accept") != acceptKey(key) {
		c.Close()
		return nil, fmt.Errorf("ws: handshake refused (%s)", resp.Status)
	}
	c.SetDeadline(time.Time{})
	return &Conn{c: c, r: r, client: true}, nil
}

// Accept completes a server-side handshake on a hijacked connection whose
// request has been read (used by tests).
func Accept(w http.ResponseWriter, req *http.Request) (*Conn, error) {
	key := req.Header.Get("Sec-WebSocket-Key")
	if key == "" {
		http.Error(w, "not a websocket request", http.StatusBadRequest)
		return nil, errors.New("ws: no key")
	}
	hj, ok := w.(http.Hijacker)
	if !ok {
		return nil, errors.New("ws: cannot hijack")
	}
	c, rw, err := hj.Hijack()
	if err != nil {
		return nil, err
	}
	resp := "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: " +
		acceptKey(key) + "\r\n\r\n"
	if _, err := io.WriteString(c, resp); err != nil {
		c.Close()
		return nil, err
	}
	return &Conn{c: c, r: rw.Reader}, nil
}

func (c *Conn) writeFrame(op byte, payload []byte) error {
	c.wmu.Lock()
	defer c.wmu.Unlock()
	if c.closed && op != opClose {
		return net.ErrClosed
	}
	hdr := make([]byte, 0, 14)
	hdr = append(hdr, 0x80|op)
	mask := byte(0)
	if c.client {
		mask = 0x80
	}
	n := len(payload)
	switch {
	case n < 126:
		hdr = append(hdr, mask|byte(n))
	case n < 1<<16:
		hdr = append(hdr, mask|126, byte(n>>8), byte(n))
	default:
		hdr = append(hdr, mask|127)
		hdr = binary.BigEndian.AppendUint64(hdr, uint64(n))
	}
	buf := payload
	if c.client {
		var mk [4]byte
		rand.Read(mk[:])
		hdr = append(hdr, mk[:]...)
		buf = make([]byte, n)
		for i := range payload {
			buf[i] = payload[i] ^ mk[i&3]
		}
	}
	if _, err := c.c.Write(append(hdr, buf...)); err != nil {
		return err
	}
	return nil
}

// WriteMessage sends one binary message.
func (c *Conn) WriteMessage(b []byte) error { return c.writeFrame(opBinary, b) }

// WriteText sends one text message (tests only).
func (c *Conn) WriteText(b []byte) error { return c.writeFrame(opText, b) }

func (c *Conn) readFrame() (fin bool, op byte, payload []byte, err error) {
	var h [2]byte
	if _, err = io.ReadFull(c.r, h[:]); err != nil {
		return
	}
	fin = h[0]&0x80 != 0
	op = h[0] & 0x0f
	masked := h[1]&0x80 != 0
	n := uint64(h[1] & 0x7f)
	switch n {
	case 126:
		var e [2]byte
		if _, err = io.ReadFull(c.r, e[:]); err != nil {
			return
		}
		n = uint64(binary.BigEndian.Uint16(e[:]))
	case 127:
		var e [8]byte
		if _, err = io.ReadFull(c.r, e[:]); err != nil {
			return
		}
		n = binary.BigEndian.Uint64(e[:])
	}
	if n > MaxMessage {
		err = errors.New("ws: frame too large")
		return
	}
	var mk [4]byte
	if masked {
		if _, err = io.ReadFull(c.r, mk[:]); err != nil {
			return
		}
	}
	payload = make([]byte, n)
	if _, err = io.ReadFull(c.r, payload); err != nil {
		return
	}
	if masked {
		for i := range payload {
			payload[i] ^= mk[i&3]
		}
	}
	return
}

// ReadMessage returns the next binary message. It answers pings, returns
// io.EOF on a close, and ErrText on a text message.
func (c *Conn) ReadMessage() ([]byte, error) {
	var msg []byte
	var msgOp byte
	inMsg := false
	for {
		fin, op, p, err := c.readFrame()
		if err != nil {
			return nil, err
		}
		switch op {
		case opPing:
			c.writeFrame(opPong, p)
			continue
		case opPong:
			continue
		case opClose:
			c.Close()
			return nil, io.EOF
		case opText, opBinary:
			if inMsg {
				return nil, errors.New("ws: new message inside a fragmented one")
			}
			msgOp, msg, inMsg = op, p, true
		case opCont:
			if !inMsg {
				return nil, errors.New("ws: continuation without a message")
			}
			if len(msg)+len(p) > MaxMessage {
				return nil, errors.New("ws: message too large")
			}
			msg = append(msg, p...)
		default:
			return nil, fmt.Errorf("ws: unknown opcode %d", op)
		}
		if fin {
			if msgOp == opText {
				return nil, ErrText
			}
			return msg, nil
		}
	}
}

// Close closes the connection (sending a close frame first).
func (c *Conn) Close() error {
	c.wmu.Lock()
	already := c.closed
	c.closed = true
	c.wmu.Unlock()
	if already {
		return nil
	}
	c.writeFrame(opClose, []byte{0x03, 0xe8})
	return c.c.Close()
}
