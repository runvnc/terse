package terse

import "testing"

func TestIsRelayURL(t *testing.T) {
	for _, v := range []string{
		"ws://relay.example",
		"wss://relay.example:443/path",
		"ws://a",
		"ws://[::1]:7000",
		"wss://例え.jp",
	} {
		if !isRelayURL(v) {
			t.Errorf("%q: not a relay URL, want one", v)
		}
	}
	for _, v := range []string{
		"",
		"ws://",
		"wss://",
		"ws:///path",
		"WS://relay.example",
		"Wss://relay.example",
		"http://relay.example",
		"ws:relay.example",
		"ws://relay example",
		"ws://relay.example\t",
		"ws://relay\x00.example",
		"ws://relay\x7f.example",
		"ws://relay\u0085.example",
		"ws://relay\u0090.example",
		"ws://relay .example",
		"ws://relay .example",
		"ws://relay .example",
		"ws://relay .example",
		"ws://relay .example",
		"ws://relay .example",
		"ws://relay　.example",
	} {
		if isRelayURL(v) {
			t.Errorf("%q: a relay URL, want not", v)
		}
	}
}

func TestProfileTakesFirstFourRelays(t *testing.T) {
	// A plain (ws://) line is invalid and skipped (NETWORK.md 4.6).
	doc := "listed: yes\n" +
		"relay: http://no\n" +
		"relay: wss://a\n" +
		"relay: wss:///no\n" +
		"relay: ws://plain.example:7878\n" +
		"relay: wss://b\n" +
		"relay: wss://c d\n" +
		"relay: wss://c\n" +
		"relay: wss://d\n" +
		"relay: wss://e\n"
	p, ok := ParseProfile([]byte(doc))
	if !ok || !p.Listed {
		t.Fatal("profile does not parse, or the plain line spoilt the rest")
	}
	if ProfileRelay("ws://relay.example") || !ProfileRelay("wss://relay.example") {
		t.Error("ProfileRelay")
	}
	want := []string{"wss://a", "wss://b", "wss://c", "wss://d"}
	if len(p.Relays) != len(want) {
		t.Fatalf("relays %q, want %q", p.Relays, want)
	}
	for i := range want {
		if p.Relays[i] != want[i] {
			t.Fatalf("relays %q, want %q", p.Relays, want)
		}
	}
}
