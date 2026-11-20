package terse

import (
	"crypto/ed25519"
	"encoding/hex"
	"path/filepath"
	"strconv"
	"testing"

	"tersereader/internal/blake3"
	"tersereader/internal/vecfile"
)

func load(t *testing.T, name string) []*vecfile.Record {
	t.Helper()
	dir, err := vecfile.Dir()
	if err != nil {
		t.Fatal(err)
	}
	recs, err := vecfile.Parse(filepath.Join(dir, name))
	if err != nil {
		t.Fatal(err)
	}
	return recs
}

func field(t *testing.T, r *vecfile.Record, f string) string {
	t.Helper()
	v, ok := r.Get(f)
	if !ok {
		t.Fatalf("%s %s: no field %s", r.Type, r.Name, f)
	}
	return v
}

func unhex(t *testing.T, s string) []byte {
	t.Helper()
	b, err := vecfile.Hex(s)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func TestEntryVectors(t *testing.T) {
	recs := load(t, "entries.txt")
	keys := map[string]ed25519.PrivateKey{}
	valid, invalid := 0, 0
	for _, r := range recs {
		switch r.Type {
		case "key":
			seed := unhex(t, field(t, r, "secret-seed"))
			priv := ed25519.NewKeyFromSeed(seed)
			pub := priv.Public().(ed25519.PublicKey)
			if got := hex.EncodeToString(pub); got != field(t, r, "feed-key") {
				t.Errorf("key %s: derived %s", r.Name, got)
			}
			if !ValidFeedKey(pub) {
				t.Errorf("key %s: not a valid feed key", r.Name)
			}
			keys[r.Name] = priv
		case "entry":
			raw := unhex(t, field(t, r, "bytes"))
			e, err := ParseEntry(raw)
			if field(t, r, "valid") == "no" {
				invalid++
				if err == nil {
					t.Errorf("entry %s: accepted, but invalid: %s", r.Name, field(t, r, "reason"))
				}
				continue
			}
			valid++
			if err != nil {
				t.Errorf("entry %s: refused: %v", r.Name, err)
				continue
			}
			check := func(name, got string) {
				if want := field(t, r, name); got != want {
					t.Errorf("entry %s: %s %s, want %s", r.Name, name, got, want)
				}
			}
			check("feed-key", e.KeyHex())
			check("seq", strconv.Itoa(int(e.Seq)))
			check("time", strconv.Itoa(int(e.Time)))
			check("kind", strconv.Itoa(int(e.Kind)))
			check("size", strconv.Itoa(int(e.Size)))
			check("blob-id", e.BlobHex())
			check("reply-to", hex.EncodeToString(e.ReplyTo[:]))
			title, _ := vecfile.Unquote(field(t, r, "title"))
			if e.Title != title {
				t.Errorf("entry %s: title %q, want %q", r.Name, e.Title, title)
			}
			var blob []byte
			if h, ok := r.Get("blob-hex"); ok {
				blob = unhex(t, h)
			} else {
				blob = vecfile.Pattern(int(e.Size))
			}
			if blake3.Hex(blob) != e.BlobHex() {
				t.Errorf("entry %s: blob does not hash to its ID", r.Name)
			}
			msg := unhex(t, field(t, r, "signed-message"))
			n := len(raw) - 64
			if string(msg) != "terse-entry-0"+string(raw[:n]) {
				t.Errorf("entry %s: signed message differs", r.Name)
			}
			// Deterministic signatures: signing the same bytes gives the entry.
			if sig := ed25519.Sign(keys["test"], msg); hex.EncodeToString(sig) != field(t, r, "signature") {
				t.Errorf("entry %s: our signature differs", r.Name)
			}
		}
	}
	if valid < 5 || invalid < 10 {
		t.Fatalf("too few entries: %d valid, %d invalid", valid, invalid)
	}
}

func TestDisplayNames(t *testing.T) {
	for _, r := range load(t, "profiles.txt") {
		if r.Type != "name" {
			continue
		}
		b := unhex(t, r.Name)
		want := field(t, r, "valid") == "yes"
		if got := ValidDisplayName(b); got != want {
			t.Errorf("name %s (%s): valid %v, want %v", r.Name, r.All("note"), got, want)
		}
	}
}

func TestProfileDocuments(t *testing.T) {
	for _, r := range load(t, "profiles.txt") {
		if r.Type != "document" {
			continue
		}
		var b []byte
		if h, ok := r.Get("hex"); ok {
			b = unhex(t, h)
		} else {
			v := field(t, r, "hex-then-x")
			var hx string
			var n int
			for i := range v {
				if v[i] == ' ' {
					hx = v[:i]
					n, _ = strconv.Atoi(v[i+1:])
				}
			}
			b = unhex(t, hx)
			for i := 0; i < n; i++ {
				b = append(b, 'x')
			}
		}
		p, ok := ParseProfile(b)
		if want := field(t, r, "valid") == "yes"; ok != want {
			t.Errorf("document %s: valid %v, want %v", r.Name, ok, want)
			continue
		}
		if !ok {
			continue
		}
		if want := field(t, r, "listed") == "yes"; p.Listed != want {
			t.Errorf("document %s: listed %v", r.Name, p.Listed)
		}
		relays := r.All("relay")
		if len(relays) != len(p.Relays) {
			t.Errorf("document %s: relays %q, want %q", r.Name, p.Relays, relays)
		} else {
			for i := range relays {
				if relays[i] != p.Relays[i] {
					t.Errorf("document %s: relays %q, want %q", r.Name, p.Relays, relays)
				}
			}
		}
		if d, ok := r.Get("description"); ok {
			want, _ := vecfile.Unquote(d)
			if p.Description != want {
				t.Errorf("document %s: description %q, want %q", r.Name, p.Description, want)
			}
		} else if n := field(t, r, "description-bytes"); strconv.Itoa(len(p.Description)) != n {
			t.Errorf("document %s: description %d bytes, want %s", r.Name, len(p.Description), n)
		}
	}
}

func TestAddresses(t *testing.T) {
	n := 0
	for _, r := range load(t, "addresses.txt") {
		if r.Type != "address" {
			continue
		}
		n++
		text, _ := vecfile.Unquote(field(t, r, "text"))
		a, err := ParseAddress(text)
		if field(t, r, "valid") == "no" {
			want := field(t, r, "reason")
			if err == nil {
				t.Errorf("address %s: accepted, want %s", r.Name, want)
			} else if string(err.(AddressError)) != want {
				t.Errorf("address %s: reason %s, want %s", r.Name, err.(AddressError), want)
			}
			continue
		}
		if err != nil {
			t.Errorf("address %s: refused: %v", r.Name, err)
			continue
		}
		if a.KeyHex() != field(t, r, "key") {
			t.Errorf("address %s: key %s", r.Name, a.KeyHex())
		}
		hints := r.All("hint")
		if len(hints) != len(a.Hints) {
			t.Errorf("address %s: %d hints, want %d", r.Name, len(a.Hints), len(hints))
			continue
		}
		for i, h := range a.Hints {
			got := h.Host + " " + strconv.Itoa(h.Port) + " " + h.URL()
			if got != hints[i] {
				t.Errorf("address %s: hint %q, want %q", r.Name, got, hints[i])
			}
		}
		if a.String() != text {
			t.Errorf("address %s: written back as %q", r.Name, a.String())
		}
	}
	if n < 20 {
		t.Fatalf("only %d addresses", n)
	}
}

func TestRefusedHosts(t *testing.T) {
	for h, want := range map[string]bool{
		"localhost": true, "a.localhost": true, "127.0.0.1": true, "10.1.2.3": true,
		"172.16.0.1": true, "192.168.1.1": true, "100.64.0.1": true, "169.254.1.1": true,
		"[::1]": true, "[fe80::1]": true, "[fd00::1]": true, "0.0.0.0": true, "[::]": true,
		"relay.example": false, "203.0.113.7": false, "[2001:db8::1]": false,
	} {
		if RefusedHost(h) != want {
			t.Errorf("RefusedHost(%s) != %v", h, want)
		}
	}
}
