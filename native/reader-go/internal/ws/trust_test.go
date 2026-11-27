package ws

import (
	"encoding/hex"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// typed is the test's relay-as-typed: URLs only.
func typed(s string) string {
	if strings.HasPrefix(s, "ws://") || strings.HasPrefix(s, "wss://") {
		return s
	}
	return ""
}

func keyPin(t *testing.T, cert string) string {
	t.Helper()
	cs, err := ReadCerts(filepath.Join(certs(), cert+".pem"))
	if err != nil {
		t.Fatal(err)
	}
	_, key := Fingerprints(cs[0])
	return "pin=sha256:" + hex.EncodeToString(key[:])
}

func certPin(t *testing.T, cert string) string {
	t.Helper()
	cs, err := ReadCerts(filepath.Join(certs(), cert+".pem"))
	if err != nil {
		t.Fatal(err)
	}
	whole, _ := Fingerprints(cs[0])
	return "pin=sha256:" + hex.EncodeToString(whole[:])
}

func dial(url string) error {
	c, err := Dial(url, "", 5*time.Second)
	if err == nil {
		c.Close()
	}
	return err
}

// The private authority of the test certificates (crates/terse-testkit/tls:
// ca.pem signed localhost.pem; untrusted.pem is self-signed), trusted
// through the user's settings: the authorities folder, one host, a pinned
// key or authority.
func TestPrivateAuthoritiesAndPins(t *testing.T) {
	t.Setenv(CAEnv, "")
	t.Cleanup(func() { SetTrust(nil) })
	secure := server(t, "localhost")     // wss://127.0.0.1:port
	selfSigned := server(t, "untrusted") // wss://127.0.0.1:port
	other := strings.Replace(secure, "127.0.0.1", "localhost", 1)
	load := func(dir string, lines ...string) {
		t.Helper()
		tr, err := LoadTrust(dir, lines, nil, typed)
		if err != nil {
			t.Fatal(err)
		}
		SetTrust(tr)
	}
	expect := func(what, url, want string) {
		t.Helper()
		err := dial(url)
		switch {
		case want == "" && err != nil:
			t.Errorf("%s: %s: %v", what, url, err)
		case want != "" && (err == nil || !strings.Contains(err.Error(), want)):
			t.Errorf("%s: %s: want %q, got %v", what, url, want, err)
		}
	}

	// Nothing set: the private authority is not trusted.
	load("")
	expect("no settings", secure, "certificate is not trusted")

	// The folder: every relay.
	dir := t.TempDir()
	os.Mkdir(filepath.Join(dir, AuthoritiesDir), 0o755)
	ca, _ := os.ReadFile(filepath.Join(certs(), "ca.pem"))
	os.WriteFile(filepath.Join(dir, AuthoritiesDir, "org.pem"), ca, 0o644)
	load(dir)
	expect("folder", secure, "")
	expect("folder", other, "")
	expect("folder, another authority", selfSigned, "certificate is not trusted")

	// One host only: not another name of the same relay.
	empty := t.TempDir()
	load(empty, secure+" authority="+filepath.Join(certs(), "ca.pem"))
	expect("per host", secure, "")
	expect("per host, other name", other, "certificate is not trusted")

	// A pinned key: that key only, without any authority, and the
	// authority trusted for every relay no longer counts for the host.
	load(empty, selfSigned+" "+keyPin(t, "untrusted"))
	expect("pinned key", selfSigned, "")
	load(empty, secure+" "+certPin(t, "localhost"))
	expect("pinned certificate", secure, "")
	load(dir, secure+" "+keyPin(t, "untrusted"))
	expect("pinned key mismatch", secure, "does not match the key pinned for it")
	expect("pin is per host", other, "")

	// A pinned authority: that one only.
	load(empty, selfSigned+" only-authority="+filepath.Join(certs(), "ca.pem"))
	expect("pinned authority mismatch", selfSigned, "not from the authority pinned for it")
	load(empty, secure+" only-authority="+filepath.Join(certs(), "ca.pem"))
	expect("pinned authority", secure, "")
	load(empty, secure+" only-authority="+filepath.Join(certs(), "untrusted.pem")+" "+keyPin(t, "wrong"))
	expect("neither", secure, "matches neither the key nor the authority")
}

func TestTrustSettingsRefused(t *testing.T) {
	ca := filepath.Join(certs(), "ca.pem")
	for _, line := range []string{
		"ws://127.0.0.1:7878 authority=" + ca, // no override on a plain relay
		"wss://a.example pin=sha256:abcd",
		"wss://a.example pin=md5:00",
		"wss://a.example insecure",
		"wss://a.example accept-any=yes",
		"wss://a.example authority=",
		"wss://a.example authority=" + filepath.Join(certs(), "localhost.key"),
	} {
		if _, err := LoadTrust("", []string{line}, nil, typed); err == nil {
			t.Errorf("%q taken", line)
		}
	}
	tr, err := LoadTrust("", []string{"# a comment", "", "wss://[2001:DB8::1]:9 pin=sha256:" + strings.Repeat("AB:", 31) + "AB"}, nil, typed)
	if err != nil {
		t.Fatal(err)
	}
	if h := tr.Hosts["2001:db8::1"]; h == nil || len(h.Pins) != 1 || h.Pins[0][0] != 0xab {
		t.Errorf("hosts %v", tr.Hosts)
	}
}
