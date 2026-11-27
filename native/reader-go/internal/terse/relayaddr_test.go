package terse

import "testing"

// NETWORK.md 3, "Encrypted by default": a relay typed without a scheme is
// wss:// whatever its port, except on localhost or a loopback address.
func TestTypedRelay(t *testing.T) {
	for in, want := range map[string]string{
		"relay.tersenet.org":      "wss://relay.tersenet.org",
		" relay.example:8443 ":    "wss://relay.example:8443",
		"relay.example:7878":      "wss://relay.example:7878",
		"relay.example:80":        "wss://relay.example:80",
		"203.0.113.5:7878":        "wss://203.0.113.5:7878",
		"192.168.1.5:7878":        "wss://192.168.1.5:7878",
		"a.localhost:7878":        "wss://a.localhost:7878",
		"[2001:db8::1]:9":         "wss://[2001:db8::1]:9",
		"ws://relay.example:7878": "ws://relay.example:7878",
		"wss://127.0.0.1:7878":    "wss://127.0.0.1:7878",
		"127.0.0.1:7878":          "ws://127.0.0.1:7878",
		"127.1.2.3:9":             "ws://127.1.2.3:9",
		"localhost:7878":          "ws://localhost:7878",
		"LocalHost:7878":          "ws://LocalHost:7878",
		"[::1]:7878":              "ws://[::1]:7878",
		"http://x":                "",
		"":                        "",
		"a b":                     "",
		"relay.example/x":         "",
		"[::1":                    "",
	} {
		if got := TypedRelay(in); got != want {
			t.Errorf("TypedRelay(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestHintSchemes(t *testing.T) {
	for in, want := range map[string]string{
		"relay.example":      "wss://relay.example",
		"relay.example:443":  "wss://relay.example",
		"relay.example:7878": "wss://relay.example:7878",
		"[2001:db8::1]":      "wss://[2001:db8::1]",
		"127.0.0.1:7878":     "ws://127.0.0.1:7878",
		"localhost:7878":     "ws://localhost:7878",
		"[::1]:9":            "ws://[::1]:9",
		"a.localhost":        "wss://a.localhost",
	} {
		hs, err := ParseHintList(in)
		if err != nil {
			t.Errorf("%s: %v", in, err)
			continue
		}
		if got := hs[0].URL(); got != want {
			t.Errorf("hint %s: %s, want %s", in, got, want)
		}
	}
	for in, reason := range map[string]string{
		"ws:":                   "bad-port",
		"ws://a.example":        "bad-port",
		"ws:relay.example:7878": "bad-port",
		"ws:127.0.0.1:7878":     "bad-port",
		"wss:a.example":         "bad-port",
		"wss://a.example":       "bad-port",
	} {
		if _, err := ParseHintList(in); err == nil || string(err.(AddressError)) != reason {
			t.Errorf("hint %s: %v, want %s", in, err, reason)
		}
	}
	// A relay named by the network is used only encrypted.
	if NetworkRelayUsable("ws://relay.example:7878", "relay.example") || !NetworkRelayUsable("wss://relay.example", "relay.example") ||
		NetworkRelayUsable("wss://127.0.0.1:1", "127.0.0.1") {
		t.Error("NetworkRelayUsable")
	}
}
