package terse

import (
	"strings"
	"unicode/utf8"
)

func isControl(r rune) bool { return r <= 0x1f || r >= 0x7f && r <= 0x9f }

func isWhiteSpace(r rune) bool {
	switch {
	case r >= 0x09 && r <= 0x0d, r == 0x20, r == 0x85, r == 0xa0, r == 0x1680,
		r >= 0x2000 && r <= 0x200a, r == 0x2028, r == 0x2029, r == 0x202f,
		r == 0x205f, r == 0x3000:
		return true
	}
	return false
}

// ValidDisplayName reports whether b is a display name (NETWORK.md 4.6):
// 1 to 48 bytes of UTF-8, no control characters, no whitespace first or
// last.
func ValidDisplayName(b []byte) bool {
	if len(b) < 1 || len(b) > 48 || !utf8.Valid(b) {
		return false
	}
	s := string(b)
	for _, r := range s {
		if isControl(r) {
			return false
		}
	}
	first, _ := utf8.DecodeRuneInString(s)
	last, _ := utf8.DecodeLastRuneInString(s)
	return !isWhiteSpace(first) && !isWhiteSpace(last)
}

// Profile is a parsed profile document.
type Profile struct {
	Listed      bool
	Relays      []string // at most four ws:// or wss:// URLs, in order
	Description string
}

// ParseProfile parses a profile document (NETWORK.md 4.6); ok is false if
// the document is invalid.
func ParseProfile(b []byte) (p Profile, ok bool) {
	if len(b) > MaxProfile || !utf8.Valid(b) {
		return p, false
	}
	s := string(b)
	for len(s) > 0 {
		line, rest, found := strings.Cut(s, "\n")
		if line == "" && found {
			p.Description = rest
			break
		}
		s = rest
		if line == "" {
			break
		}
		field, value, has := strings.Cut(line, ": ")
		if !has {
			return Profile{}, false
		}
		switch field {
		case "listed":
			p.Listed = value == "yes"
		case "relay":
			if len(p.Relays) < 4 && isRelayURL(value) {
				p.Relays = append(p.Relays, value)
			}
		}
	}
	return p, true
}

// isRelayURL reports whether a relay line's value counts as a relay URL
// (NETWORK.md 4.6): it starts with `ws://` or `wss://` (lowercase), what
// follows is not empty and does not start with `/`, and it holds no
// control character and no whitespace character.
func isRelayURL(v string) bool {
	var rest string
	switch {
	case strings.HasPrefix(v, "ws://"):
		rest = v[5:]
	case strings.HasPrefix(v, "wss://"):
		rest = v[6:]
	default:
		return false
	}
	if rest == "" || rest[0] == '/' {
		return false
	}
	for _, r := range v {
		if isControl(r) || isWhiteSpace(r) {
			return false
		}
	}
	return true
}
