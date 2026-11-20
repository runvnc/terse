//go:build thin

// A thin reader (LOCAL.md) has no cryptography: its node verifies every
// entry and refuses invalid feed keys, so these never decide anything that
// matters. Nothing in a thin build parses entries from the network.

package terse

// ValidFeedKey, in a thin build, checks only the length: the node checks
// the point (NETWORK.md 4.1) when the key reaches it.
func ValidFeedKey(k []byte) bool { return len(k) == 32 }

// VerifyStrict, in a thin build, verifies nothing and refuses everything.
func VerifyStrict(key, msg, sig []byte) bool { return false }
