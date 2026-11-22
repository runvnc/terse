package ws

import (
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/hex"
	"encoding/pem"
	"errors"
	"fmt"
	"net/netip"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
)

// What the reader trusts for wss:// relays besides the system's
// authorities (NETWORK.md 3, "Overrides come only from the user"), from the
// user's own settings only (README "Private authorities and pins"): every
// *.pem in <dir>/authorities, TERSE_RELAY_CA, --relay-authority, and the
// options of a relay line (authority=, only-authority=, pin=). Nothing
// turns verification off; an option applies only to its host.

// AuthoritiesDir is the folder, in the reader's directory, whose PEM files
// are trusted for every relay.
const AuthoritiesDir = "authorities"

// HostTrust is what is trusted for one host.
type HostTrust struct {
	Also []*x509.Certificate // authority=: trusted for this host besides the others
	Only []*x509.Certificate // only-authority=: the only authorities for this host
	Pins [][32]byte          // pin=: the only certificates or public keys for this host
}

// Pinned reports whether nothing but the pinned authorities and keys is
// accepted for the host.
func (h *HostTrust) Pinned() bool { return len(h.Only) > 0 || len(h.Pins) > 0 }

// Trust is everything trusted besides the system's authorities.
type Trust struct {
	All   []*x509.Certificate // for every relay
	Hosts map[string]*HostTrust
}

var (
	trustMu sync.Mutex
	trust   *Trust
)

// SetTrust replaces what this process trusts (TERSE_RELAY_CA alone until
// it is called).
func SetTrust(t *Trust) {
	trustMu.Lock()
	trust = t
	trustMu.Unlock()
}

func currentTrust() (*Trust, error) {
	trustMu.Lock()
	t := trust
	trustMu.Unlock()
	if t != nil {
		return t, nil
	}
	t = &Trust{Hosts: map[string]*HostTrust{}}
	if path := os.Getenv(CAEnv); path != "" {
		if err := t.AddAll(path); err != nil {
			return nil, fmt.Errorf("%s: %w", CAEnv, err)
		}
	}
	return t, nil
}

// HostKey is a host as the settings key it: lowercase, an IPv6 address
// without brackets in its shortest form.
func HostKey(host string) string {
	h := strings.TrimSuffix(strings.TrimPrefix(host, "["), "]")
	if ip, err := netip.ParseAddr(h); err == nil {
		return ip.String()
	}
	return strings.ToLower(h)
}

// Fingerprints are the SHA-256 of a certificate and of its public key
// (SubjectPublicKeyInfo).
func Fingerprints(c *x509.Certificate) (whole, key [32]byte) {
	return sha256.Sum256(c.Raw), sha256.Sum256(c.RawSubjectPublicKeyInfo)
}

// ReadCerts reads the certificates of a PEM file.
func ReadCerts(path string) ([]*x509.Certificate, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var out []*x509.Certificate
	for {
		var blk *pem.Block
		blk, b = pem.Decode(b)
		if blk == nil {
			break
		}
		if blk.Type != "CERTIFICATE" {
			continue
		}
		c, err := x509.ParseCertificate(blk.Bytes)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", path, err)
		}
		out = append(out, c)
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("%s: no certificate in it (PEM, BEGIN CERTIFICATE)", path)
	}
	return out, nil
}

// AddAll trusts the authorities of a PEM file for every relay.
func (t *Trust) AddAll(path string) error {
	cs, err := ReadCerts(path)
	if err != nil {
		return err
	}
	t.All = append(t.All, cs...)
	return nil
}

// SplitLine splits a relay line at blanks: the relay, then its options.
// ok is false for an empty line or a # comment.
func SplitLine(line string) (relay string, options []string, ok bool) {
	w := strings.Fields(line)
	if len(w) == 0 || strings.HasPrefix(w[0], "#") {
		return "", nil, false
	}
	return w[0], w[1:], true
}

// AddLine takes the options of a relay line for the relay url (already a
// URL), files relative to base (if not empty).
func (t *Trust) AddLine(url string, options []string, base string) error {
	if len(options) == 0 {
		return nil
	}
	if !strings.HasPrefix(url, "wss://") {
		return fmt.Errorf("%s: authority= and pin= apply only to wss:// (encrypted) relays", url)
	}
	host := strings.TrimPrefix(url, "wss://")
	host, _, _ = strings.Cut(host, "/")
	if strings.HasPrefix(host, "[") {
		host = host[:strings.IndexByte(host, ']')+1]
	} else {
		host, _, _ = strings.Cut(host, ":")
	}
	file := func(name string) string {
		if base != "" && !filepath.IsAbs(name) {
			return filepath.Join(base, name)
		}
		return name
	}
	h := t.Hosts[HostKey(host)]
	if h == nil {
		h = &HostTrust{}
	}
	for _, o := range options {
		name, value, ok := strings.Cut(o, "=")
		if !ok {
			return fmt.Errorf("%s: %s: not an option (authority=, only-authority= or pin=)", url, o)
		}
		if value == "" {
			return fmt.Errorf("%s: %s= without a value", url, name)
		}
		switch name {
		case "authority", "only-authority":
			cs, err := ReadCerts(file(value))
			if err != nil {
				return fmt.Errorf("%s: %w", url, err)
			}
			if name == "authority" {
				h.Also = append(h.Also, cs...)
			} else {
				h.Only = append(h.Only, cs...)
			}
		case "pin":
			hx, ok := strings.CutPrefix(value, "sha256:")
			hx = strings.ToLower(strings.ReplaceAll(hx, ":", ""))
			b, err := hex.DecodeString(hx)
			if !ok || err != nil || len(b) != 32 {
				return fmt.Errorf("%s: pin=%s: not a fingerprint (sha256: and 64 hex digits)", url, value)
			}
			h.Pins = append(h.Pins, [32]byte(b))
		default:
			return fmt.Errorf("%s: %s: not an option (authority=, only-authority= or pin=)", url, o)
		}
	}
	if t.Hosts == nil {
		t.Hosts = map[string]*HostTrust{}
	}
	t.Hosts[HostKey(host)] = h
	return nil
}

// LoadTrust is everything the user's settings say: TERSE_RELAY_CA, the
// PEM files of <dir>/authorities (in name order), the options of the relay
// lines (files relative to dir), and the files of authorities
// (--relay-authority). typed turns a relay as typed into its URL.
func LoadTrust(dir string, lines []string, authorities []string, typed func(string) string) (*Trust, error) {
	t := &Trust{Hosts: map[string]*HostTrust{}}
	if path := os.Getenv(CAEnv); path != "" {
		if err := t.AddAll(path); err != nil {
			return nil, fmt.Errorf("%s: %w", CAEnv, err)
		}
	}
	if dir != "" {
		pems, _ := filepath.Glob(filepath.Join(dir, AuthoritiesDir, "*.pem"))
		sort.Strings(pems)
		for _, p := range pems {
			if err := t.AddAll(p); err != nil {
				return nil, err
			}
		}
	}
	for _, l := range lines {
		relay, opts, ok := SplitLine(l)
		if !ok {
			continue
		}
		u := typed(relay)
		if u == "" {
			return nil, fmt.Errorf("not a relay: %s", relay)
		}
		if err := t.AddLine(u, opts, dir); err != nil {
			return nil, err
		}
	}
	for _, a := range authorities {
		if err := t.AddAll(a); err != nil {
			return nil, fmt.Errorf("--relay-authority: %w", err)
		}
	}
	return t, nil
}

// The ways a pinned host's certificate is refused.
var (
	ErrPinKey          = errors.New("the relay's certificate does not match the key pinned for it (pin= in your relay list): it is not trusted")
	ErrPinAuthority    = errors.New("the relay's certificate is not from the authority pinned for it (only-authority= in your relay list): it is not trusted")
	ErrPinKeyAuthority = errors.New("the relay's certificate matches neither the key nor the authority pinned for it (your relay list): it is not trusted")
)

// pinnedCheck verifies a pinned host's certificate: its certificate or
// public key is pinned, or it chains to a pinned authority and is for the
// host. Nothing else is accepted, a public authority's certificate
// included.
func pinnedCheck(h *HostTrust, host string) func(tls.ConnectionState) error {
	return func(cs tls.ConnectionState) error {
		if len(cs.PeerCertificates) == 0 {
			return ErrPinKey
		}
		leaf := cs.PeerCertificates[0]
		whole, key := Fingerprints(leaf)
		for _, p := range h.Pins {
			if p == whole || p == key {
				return nil
			}
		}
		if len(h.Only) == 0 {
			return ErrPinKey
		}
		roots, inter := x509.NewCertPool(), x509.NewCertPool()
		for _, c := range h.Only {
			roots.AddCert(c)
		}
		for _, c := range cs.PeerCertificates[1:] {
			inter.AddCert(c)
		}
		_, err := leaf.Verify(x509.VerifyOptions{Roots: roots, Intermediates: inter, DNSName: host})
		var unknown x509.UnknownAuthorityError
		switch {
		case err == nil:
			return nil
		case errors.As(err, &unknown) && len(h.Pins) > 0:
			return ErrPinKeyAuthority
		case errors.As(err, &unknown):
			return ErrPinAuthority
		}
		return err
	}
}

// clientTLS is the TLS configuration for a wss:// relay named host: the
// system's roots, those trusted for every relay and for host, or what is
// pinned for host.
func clientTLS(host string) (*tls.Config, error) {
	cfg := &tls.Config{ServerName: host, MinVersion: tls.VersionTLS12}
	t, err := currentTrust()
	if err != nil {
		return nil, err
	}
	h := t.Hosts[HostKey(host)]
	if h != nil && h.Pinned() {
		// The standard verification is replaced, not dropped: VerifyConnection
		// accepts only what is pinned for this host.
		cfg.InsecureSkipVerify = true
		cfg.VerifyConnection = pinnedCheck(h, host)
		return cfg, nil
	}
	extra := t.All
	if h != nil {
		extra = append(append([]*x509.Certificate(nil), extra...), h.Also...)
	}
	if len(extra) == 0 {
		return cfg, nil
	}
	pool, err := x509.SystemCertPool()
	if err != nil || pool == nil {
		pool = x509.NewCertPool()
	}
	for _, c := range extra {
		pool.AddCert(c)
	}
	cfg.RootCAs = pool
	return cfg, nil
}
