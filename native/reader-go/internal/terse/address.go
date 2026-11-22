package terse

import (
	"encoding/hex"
	"fmt"
	"net/netip"
	"strconv"
	"strings"
)

// Hint is one relay hint of a feed address (NETWORK.md 4.9).
type Hint struct {
	Host  string // as a URL writes it: an IPv6 address in brackets, shortest form
	Port  int    // the default port of the scheme (443 or 80) if the hint names none
	Plain bool   // written with "ws:"
	Text  string // as written
}

// PlainPrefix starts a hint that names a plain relay.
const PlainPrefix = "ws:"

// Encrypted reports whether the hint names a relay over TLS: not written
// "ws:" and not on localhost or a loopback address (NETWORK.md 3, 4.9).
func (h Hint) Encrypted() bool { return !h.Plain && !LoopbackHost(h.Host) }

// URL is the relay the hint names: wss://host (with :port unless 443),
// or ws://host:port for a plain one (NETWORK.md 4.9).
func (h Hint) URL() string {
	switch {
	case !h.Encrypted():
		return fmt.Sprintf("ws://%s:%d", h.Host, h.Port)
	case h.Port == 443:
		return "wss://" + h.Host
	}
	return fmt.Sprintf("wss://%s:%d", h.Host, h.Port)
}

// LoopbackHost reports whether a host as a URL writes it is localhost or
// a loopback IP address: a relay there is plain when no scheme is written
// (NETWORK.md 3). Names under .localhost are not.
func LoopbackHost(host string) bool {
	if strings.EqualFold(host, "localhost") {
		return true
	}
	ip, err := netip.ParseAddr(strings.TrimSuffix(strings.TrimPrefix(host, "["), "]"))
	return err == nil && ip.Unmap().IsLoopback()
}

// Address is a parsed feed address.
type Address struct {
	Key   [32]byte
	Hints []Hint
}

func (a Address) KeyHex() string { return KeyHex(a.Key[:]) }

// String writes the address back: the key, then `@` and the hints as written.
func (a Address) String() string {
	s := a.KeyHex()
	if len(a.Hints) > 0 {
		var parts []string
		for _, h := range a.Hints {
			parts = append(parts, h.Text)
		}
		s += "@" + strings.Join(parts, ",")
	}
	return s
}

// AddressError names why an address is malformed, by the reasons of
// vectors/addresses.txt.
type AddressError string

func (e AddressError) Error() string { return "malformed feed address: " + string(e) }

// ParseAddress parses a feed address exactly as NETWORK.md 4.9 writes it.
func ParseAddress(s string) (Address, error) {
	var a Address
	keyPart, hintPart, hasHints := strings.Cut(s, "@")
	if !IsLowerHex64(keyPart) {
		return a, AddressError("bad-key")
	}
	k, _ := hex.DecodeString(keyPart)
	if !ValidFeedKey(k) {
		return a, AddressError("invalid-key")
	}
	copy(a.Key[:], k)
	if !hasHints {
		return a, nil
	}
	hints, err := ParseHintList(hintPart)
	if err != nil {
		return a, err
	}
	a.Hints = hints
	return a, nil
}

// ParseUserAddress is ParseAddress with the typing aids a client may
// offer: blanks around the address and the link form terse:feed/<address>.
func ParseUserAddress(s string) (Address, error) {
	s = strings.TrimSpace(s)
	s = strings.TrimPrefix(s, "terse:feed/")
	return ParseAddress(s)
}

// ParseHintList parses one or two hints separated by a comma.
func ParseHintList(s string) ([]Hint, error) {
	if len(s) > 161 {
		// Two hints of at most 80 bytes and a comma; let the parts say
		// which rule is broken when they can.
		if strings.Count(s, ",") <= 1 {
			return nil, AddressError("hint-too-long")
		}
	}
	parts := strings.Split(s, ",")
	if len(parts) > 2 {
		return nil, AddressError("too-many-hints")
	}
	var hints []Hint
	for _, p := range parts {
		h, err := parseHint(p)
		if err != nil {
			return nil, err
		}
		hints = append(hints, h)
	}
	return hints, nil
}

func parseHint(s string) (Hint, error) {
	if s == "" {
		return Hint{}, AddressError("empty-hint")
	}
	if len(s) > 80 {
		return Hint{}, AddressError("hint-too-long")
	}
	h := Hint{Text: s}
	if strings.HasPrefix(s, PlainPrefix) {
		h.Plain = true
		s = s[len(PlainPrefix):]
		if s == "" {
			return Hint{}, AddressError("bad-name")
		}
	}
	var host, port string
	hasPort := false
	if s[0] == '[' {
		end := strings.IndexByte(s, ']')
		if end < 0 {
			return Hint{}, AddressError("bad-ipv6")
		}
		inner := s[1:end]
		if !validIPv6(inner) {
			return Hint{}, AddressError("bad-ipv6")
		}
		ip, err := netip.ParseAddr(inner)
		if err != nil {
			return Hint{}, AddressError("bad-ipv6")
		}
		host = "[" + ip.String() + "]"
		rest := s[end+1:]
		if rest != "" {
			if rest[0] != ':' {
				return Hint{}, AddressError("bad-port")
			}
			port, hasPort = rest[1:], true
		}
	} else {
		host, port, hasPort = strings.Cut(s, ":")
		if err := checkHost(host); err != nil {
			return Hint{}, err
		}
	}
	if hasPort {
		p, ok := parsePort(port)
		if !ok {
			return Hint{}, AddressError("bad-port")
		}
		h.Port = p
	}
	h.Host = host
	if !hasPort {
		h.Port = 80
		if h.Encrypted() {
			h.Port = 443
		}
	}
	return h, nil
}

func parsePort(s string) (int, bool) {
	if s == "" || len(s) > 5 || s[0] == '0' {
		return 0, false
	}
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return 0, false
		}
	}
	n, _ := strconv.Atoi(s)
	return n, n >= 1 && n <= 65535
}

func isLetter(c byte) bool { return c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' }
func isDigit(c byte) bool  { return c >= '0' && c <= '9' }

// checkHost checks a name or an IPv4 address. A host whose last label is
// not empty and holds no letter must be an IPv4 address.
func checkHost(host string) error {
	labels := strings.Split(host, ".")
	last := labels[len(labels)-1]
	hasLetter := false
	for i := 0; i < len(last); i++ {
		if isLetter(last[i]) {
			hasLetter = true
		}
	}
	if last != "" && !hasLetter {
		if !validIPv4(host) {
			return AddressError("bad-ipv4")
		}
		return nil
	}
	for _, l := range labels {
		if len(l) < 1 || len(l) > 63 || l[0] == '-' || l[len(l)-1] == '-' {
			return AddressError("bad-name")
		}
		for i := 0; i < len(l); i++ {
			c := l[i]
			if !isLetter(c) && !isDigit(c) && c != '-' {
				return AddressError("bad-name")
			}
		}
	}
	return nil
}

func validIPv4(s string) bool {
	parts := strings.Split(s, ".")
	if len(parts) != 4 {
		return false
	}
	for _, p := range parts {
		if p == "" || len(p) > 3 || (len(p) > 1 && p[0] == '0') {
			return false
		}
		for i := 0; i < len(p); i++ {
			if !isDigit(p[i]) {
				return false
			}
		}
		if n, _ := strconv.Atoi(p); n > 255 {
			return false
		}
	}
	return true
}

// validIPv6 checks the grammar of NETWORK.md 4.9: groups of 1 to 4 hex
// digits, 8 of them, or fewer with one "::"; no dotted part, no zone.
func validIPv6(s string) bool {
	isGroup := func(g string) bool {
		if len(g) < 1 || len(g) > 4 {
			return false
		}
		for i := 0; i < len(g); i++ {
			c := g[i]
			if !isDigit(c) && !(c >= 'a' && c <= 'f') && !(c >= 'A' && c <= 'F') {
				return false
			}
		}
		return true
	}
	groups := func(part string) (int, bool) {
		if part == "" {
			return 0, true
		}
		gs := strings.Split(part, ":")
		for _, g := range gs {
			if !isGroup(g) {
				return 0, false
			}
		}
		return len(gs), true
	}
	if i := strings.Index(s, "::"); i >= 0 {
		left, right := s[:i], s[i+2:]
		if strings.Contains(right, "::") {
			return false
		}
		nl, ok1 := groups(left)
		nr, ok2 := groups(right)
		return ok1 && ok2 && nl+nr <= 7
	}
	n, ok := groups(s)
	return ok && n == 8
}

// RefusedHost reports whether a relay line's or hint's host must be
// ignored (NETWORK.md 5.1): localhost, a name under .localhost, or an
// address that is loopback, unspecified, private or link-local.
func RefusedHost(host string) bool {
	h := strings.ToLower(strings.TrimSuffix(strings.TrimPrefix(host, "["), "]"))
	if h == "localhost" || strings.HasSuffix(h, ".localhost") {
		return true
	}
	if ip, err := netip.ParseAddr(h); err == nil {
		return RefusedIP(ip)
	}
	return false
}

// RefusedIP reports whether a relay address is loopback, unspecified,
// private (including 100.64.0.0/10 and fc00::/7) or link-local.
func RefusedIP(ip netip.Addr) bool {
	ip = ip.Unmap()
	if ip.IsLoopback() || ip.IsUnspecified() || ip.IsPrivate() || ip.IsLinkLocalUnicast() {
		return true
	}
	if ip.Is4() && netip.MustParsePrefix("100.64.0.0/10").Contains(ip) {
		return true
	}
	return false
}

// TypedRelay is a relay as a person types it (NETWORK.md 3, "Encrypted by
// default"): a ws:// or wss:// URL as it is, or a host and maybe a port
// without a scheme, taken as wss:// (a port is only a port), except that
// localhost or a loopback address is taken as ws:// (this machine).
// Empty if it is neither.
func TypedRelay(s string) string {
	s = strings.TrimSpace(s)
	if IsRelayURL(s) {
		return s
	}
	if s == "" || strings.Contains(s, "://") || strings.ContainsAny(s, "/ \t") {
		return ""
	}
	host := s
	if strings.HasPrefix(s, "[") {
		end := strings.IndexByte(s, ']')
		if end < 0 {
			return ""
		}
		host = s[:end+1]
		if rest := s[end+1:]; rest != "" && rest[0] != ':' {
			return ""
		}
	} else if h, _, ok := strings.Cut(s, ":"); ok {
		host = h
	}
	if host == "" || host == "[]" {
		return ""
	}
	if LoopbackHost(host) {
		return "ws://" + s
	}
	return "wss://" + s
}

// NetworkRelayUsable reports whether a relay URL named by the network (a
// profile's relay line, a hint) whose host is host may be used: it is
// encrypted (a plain relay named by the network is not used unless it is
// one of the user's own, which serve every feed anyway: NETWORK.md 3, no
// downgrade) and its host is not refused (NETWORK.md 5.1).
func NetworkRelayUsable(u, host string) bool {
	return strings.HasPrefix(u, "wss://") && !RefusedHost(host)
}
