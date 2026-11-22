package main

import (
	"bufio"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"tersereader/internal/testenv"
)

// TestInteropTLS: the reader against reference relays over TLS (wss://),
// with the test certificates of crates/terse-testkit/tls and their CA
// trusted through TERSE_RELAY_CA; a certificate for another name, one
// from no trusted authority, and each side speaking the other way are
// refused in words.
func TestInteropTLS(t *testing.T) {
	if testing.Short() {
		t.Skip("short")
	}
	e := &env{t: t, tmp: t.TempDir(), terse: findBin(t, "terse"), relayB: findBin(t, "terse-relay")}
	e.reader = filepath.Join(e.tmp, "tersereader")
	build := exec.Command("go", "build", "-p", "4", "-o", e.reader, ".")
	build.Env = append(os.Environ(), "CGO_ENABLED=0")
	if out, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build: %v\n%s", err, out)
	}
	certs := filepath.Join(testenv.Root(), "crates", "terse-testkit", "tls")
	ca := "TERSE_RELAY_CA=" + filepath.Join(certs, "ca.pem")
	start := func(name, cert string) string {
		args := []string{"--listen", "127.0.0.1:0", "--store", filepath.Join(e.tmp, name)}
		if cert != "" {
			args = append(args, "--tls-cert", filepath.Join(certs, cert+".pem"), "--tls-key", filepath.Join(certs, cert+".key"))
		}
		cmd := exec.Command(e.relayB, args...)
		cmd.Env = e.isolated()
		out, err := cmd.StdoutPipe()
		if err != nil {
			t.Fatal(err)
		}
		if err := cmd.Start(); err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
		line, err := bufio.NewReader(out).ReadString('\n')
		if err != nil {
			t.Fatalf("relay %s: %v", name, err)
		}
		return strings.TrimPrefix(strings.TrimSpace(line), "listening on ")
	}
	secure := start("secure", "localhost")
	plain := start("plain", "")
	wrong := start("wrong", "wrong")
	untrusted := start("untrusted", "untrusted")
	if !strings.HasPrefix(secure, "wss://127.0.0.1:") {
		t.Fatalf("TLS relay: %s", secure)
	}

	// Alice publishes on the TLS relay with the reference tool.
	pub := func(args ...string) {
		all := append(args, "--relay", secure, "--store", filepath.Join(e.tmp, "alice", "store"),
			"--config", filepath.Join(e.tmp, "alice", "config"))
		cmd := exec.Command(e.terse, all...)
		cmd.Env = e.isolated(ca)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("terse %v: %v\n%s", args, err, out)
		}
	}
	pub("key", "new")
	alice := e.key("alice", plain)
	pub("feed", "publish", e.file("tls.txt", "# Over TLS\n\nRead over wss://.\n"), "--title", "Over TLS")

	// The reader syncs it over wss://, trusting the test CA.
	verbose := []string{ca, "TERSEREADER_VERBOSE=1"}
	run := func(dir string, env []string, args ...string) string {
		out, err := e.read(dir, env, args...)
		if err != nil {
			t.Fatalf("tersereader %v: %v\n%s", args, err, out)
		}
		return out
	}
	run("r", verbose, "relay", "add", secure)
	run("r", verbose, "subscribe", alice)
	out := run("r", verbose, "sync", "--timeout", "20s")
	if !strings.Contains(out, "1 new in the timeline") {
		t.Fatalf("sync over TLS: %s", out)
	}
	if list := run("r", verbose, "list"); !strings.Contains(list, "Over TLS") {
		t.Errorf("list: %s", list)
	}

	// Refusals, each with its reason in the log.
	cases := []struct{ url, env, want string }{
		{wrong, ca, "certificate is for another name"},
		{untrusted, ca, "certificate is not trusted"},
		{secure, "", "certificate is not trusted"},
		{strings.Replace(plain, "ws://", "wss://", 1), ca, "this relay does not speak TLS (try ws://)"},
		{strings.Replace(secure, "wss://", "ws://", 1), ca, "this relay requires TLS (try wss://)"},
	}
	for i, c := range cases {
		dir := "refused" + string(rune('a'+i))
		env := []string{"TERSEREADER_VERBOSE=1"}
		if c.env != "" {
			env = append(env, c.env)
		}
		run(dir, env, "relay", "add", c.url)
		run(dir, env, "subscribe", alice)
		started := time.Now()
		out, _ := e.read(dir, env, "sync", "--timeout", "3s")
		if !strings.Contains(out, c.want) {
			t.Errorf("%s: want %q in:\n%s", c.url, c.want, out)
		}
		if time.Since(started) > 15*time.Second {
			t.Errorf("%s: sync took %v", c.url, time.Since(started))
		}
	}

	// A bare host name is taken as wss://.
	if out := run("names", nil, "relay", "add", "relay.example"); !strings.Contains(out, "taken as wss://relay.example") {
		t.Errorf("relay add: %s", out)
	}
	if list := run("names", nil, "relay", "list"); list != "wss://relay.example\n" {
		t.Errorf("relay list: %q", list)
	}
}
