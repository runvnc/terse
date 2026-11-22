// Package ws is a minimal RFC 6455 WebSocket implementation: a client
// (ws:// and wss://) and, for tests, a server side. It carries binary
// messages, answers pings and closes cleanly.
//
// A wss:// relay's certificate is checked by crypto/tls against the
// system's trusted roots and the URL's host; TERSE_RELAY_CA may name a
// PEM file of further CA certificates to trust (a self-hosted relay with a
// private CA), as for the reference clients.
package ws

import (
	"bufio"
	"context"
	"crypto/rand"
	"crypto/sha1"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
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

// ErrRequiresTLS: a plain (ws://) request was answered with TLS, or
// redirected to https://.
var ErrRequiresTLS = errors.New("this relay requires TLS (try wss://)")

// CAEnv names a PEM file of CA certificates trusted for wss:// relays
// besides the system's.
const CAEnv = "TERSE_RELAY_CA"

// clientTLS is the TLS configuration for a wss:// relay named host: the
// system's roots, and those of CAEnv if it is set.
func clientTLS(host string) (*tls.Config, error) {
	cfg := &tls.Config{ServerName: host, MinVersion: tls.VersionTLS12}
	path := os.Getenv(CAEnv)
	if path == "" {
		return cfg, nil
	}
	pem, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("%s: %w", CAEnv, err)
	}
	pool, err := x509.SystemCertPool()
	if err != nil || pool == nil {
		pool = x509.NewCertPool()
	}
	if !pool.AppendCertsFromPEM(pem) {
		return nil, fmt.Errorf("%s: %s: no PEM certificate in it", CAEnv, path)
	}
	cfg.RootCAs = pool
	return cfg, nil
}

// tlsFailure says in words why a TLS handshake with a relay failed.
func tlsFailure(err error) error {
	var hostErr x509.HostnameError
	var authErr x509.UnknownAuthorityError
	var certErr x509.CertificateInvalidError
	var recErr tls.RecordHeaderError
	switch {
	case errors.As(err, &hostErr):
		return fmt.Errorf("the relay's certificate is for another name (%w)", err)
	case errors.As(err, &authErr):
		return fmt.Errorf("the relay's certificate is not trusted (not from a known authority; for a private CA, see %s): %w", CAEnv, err)
	case errors.As(err, &certErr) && certErr.Reason == x509.Expired:
		return fmt.Errorf("the relay's certificate has expired or is not valid yet: %w", err)
	case errors.As(err, &certErr):
		return fmt.Errorf("the relay's certificate is not trusted: %w", err)
	case errors.As(err, &recErr):
		return errors.New("this relay does not speak TLS (try ws://)")
	case errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF):
		return errors.New("the relay closed the connection at the TLS handshake: it may not speak TLS (try ws://)")
	}
	return err
}

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
	return DialStop(rawurl, addr, timeout, nil)
}

// DialStop is Dial that gives up as soon as stop is closed.
func DialStop(rawurl, addr string, timeout time.Duration, stop <-chan struct{}) (*Conn, error) {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	if stop != nil {
		go func() {
			select {
			case <-stop:
				cancel()
			case <-ctx.Done():
			}
		}()
	}
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
	c, err := (&net.Dialer{}).DialContext(ctx, "tcp", addr)
	if err != nil {
		return nil, err
	}
	// Stopped or out of time during the handshake: the connection closes.
	keep := context.AfterFunc(ctx, func() { c.Close() })
	if u.Scheme == "wss" {
		cfg, err := clientTLS(u.Hostname())
		if err != nil {
			c.Close()
			return nil, err
		}
		tc := tls.Client(c, cfg)
		tc.SetDeadline(time.Now().Add(timeout))
		if err := tc.Handshake(); err != nil {
			c.Close()
			return nil, tlsFailure(err)
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
	// A TLS record in answer to a plain request: the relay wants wss://.
	if first, err := r.Peek(1); err == nil && u.Scheme == "ws" && (first[0] == 0x15 || first[0] == 0x16) {
		c.Close()
		return nil, ErrRequiresTLS
	}
	resp, err := http.ReadResponse(r, nil)
	if err != nil {
		c.Close()
		return nil, err
	}
	if u.Scheme == "ws" && resp.StatusCode/100 == 3 {
		if loc := resp.Header.Get("Location"); strings.HasPrefix(loc, "https://") || strings.HasPrefix(loc, "wss://") {
			c.Close()
			return nil, ErrRequiresTLS
		}
	}
	// A TLS server's refusal of plain HTTP (nginx's and Cloudflare's "The
	// plain HTTP request was sent to HTTPS port", Go's "Client sent an HTTP
	// request to an HTTPS server").
	if u.Scheme == "ws" && resp.StatusCode == http.StatusBadRequest {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		b := strings.ToLower(string(body))
		if strings.Contains(b, "https") || strings.Contains(b, "tls") || strings.Contains(b, "ssl") {
			c.Close()
			return nil, ErrRequiresTLS
		}
	}
	if resp.StatusCode != http.StatusSwitchingProtocols ||
		!strings.EqualFold(resp.Header.Get("Upgrade"), "websocket") ||
		resp.Header.Get("Sec-WebSocket-Accept") != acceptKey(key) {
		c.Close()
		return nil, fmt.Errorf("ws: handshake refused (%s)", resp.Status)
	}
	if !keep() {
		return nil, ctx.Err()
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
