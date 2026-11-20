package terse

import "encoding/hex"

// KeyHex is a key or ID as 64 lowercase hexadecimal digits.
func KeyHex(k []byte) string { return hex.EncodeToString(k) }

// ShortKey is the first and last six hex digits of a key.
func ShortKey(h string) string {
	if len(h) < 12 {
		return h
	}
	return h[:6] + "…" + h[len(h)-6:]
}

// IsLowerHex64 reports whether s is exactly 64 lowercase hexadecimal digits.
func IsLowerHex64(s string) bool {
	if len(s) != 64 {
		return false
	}
	for i := 0; i < 64; i++ {
		if !isLowerHex(s[i]) {
			return false
		}
	}
	return true
}

func isLowerHex(c byte) bool { return c >= '0' && c <= '9' || c >= 'a' && c <= 'f' }
