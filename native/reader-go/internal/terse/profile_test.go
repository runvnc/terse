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
	doc := "relay: http://no\n" +
		"relay: ws://a\n" +
		"relay: ws:///no\n" +
		"relay: wss://b\n" +
		"relay: ws://c d\n" +
		"relay: ws://c\n" +
		"relay: ws://d\n" +
		"relay: ws://e\n"
	p, ok := ParseProfile([]byte(doc))
	if !ok {
		t.Fatal("profile does not parse")
	}
	want := []string{"ws://a", "wss://b", "ws://c", "ws://d"}
	if len(p.Relays) != len(want) {
		t.Fatalf("relays %q, want %q", p.Relays, want)
	}
	for i := range want {
		if p.Relays[i] != want[i] {
			t.Fatalf("relays %q, want %q", p.Relays, want)
		}
	}
}
