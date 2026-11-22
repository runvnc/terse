package main

import (
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"tersereader/internal/article"
	"tersereader/internal/blake3"
	"tersereader/internal/testenv"
)

// The interoperability test runs the reference relay and command line
// tool as black boxes (never on port 7878, never with the real
// configuration or store) and checks that this reader syncs, verifies and
// shows what they publish. TERSE_BIN_DIR names the reference binaries'
// directory, else target/release of the repository is used. It is skipped
// (a recorded skip, testenv) if they are not built, and fails if one is
// older than its sources: build them first with
// scripts/cargo build --release --workspace --bins.

func findBin(t *testing.T, name string) string {
	t.Helper()
	dir := os.Getenv("TERSE_BIN_DIR")
	if dir == "" {
		dir = filepath.Join(testenv.Root(), "target", "release")
	}
	p := filepath.Join(dir, name)
	if _, err := os.Stat(p); err != nil {
		testenv.Skip(t, "programs", "%s not built (scripts/cargo build --release --workspace --bins, or set TERSE_BIN_DIR)", p)
	}
	why, noted := testenv.Stale(p)
	if why != "" {
		t.Fatalf("%s is older than its sources (%s): run scripts/cargo build --release --workspace --bins", p, why)
	}
	if noted {
		t.Logf("%s: no build record beside it, so not checked against the sources", p)
	}
	return p
}

type env struct {
	t      *testing.T
	tmp    string
	terse  string
	relayB string
	reader string
}

func freePort(t *testing.T) int {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	return l.Addr().(*net.TCPAddr).Port
}

// isolated is an environment with every XDG directory in the test's temp dir.
func (e *env) isolated(extra ...string) []string {
	base := []string{"HOME=" + e.tmp, "XDG_CONFIG_HOME=" + e.tmp + "/xdg-config",
		"XDG_CACHE_HOME=" + e.tmp + "/xdg-cache", "XDG_DATA_HOME=" + e.tmp + "/xdg-data", "PATH=" + os.Getenv("PATH"),
		"XDG_RUNTIME_DIR=" + e.tmp + "/xdg-run"}
	return append(base, extra...)
}

func (e *env) startRelay(name string) string {
	port := freePort(e.t)
	if port == 7878 {
		port = freePort(e.t)
	}
	addr := fmt.Sprintf("127.0.0.1:%d", port)
	cmd := exec.Command(e.relayB, "--listen", addr, "--store", filepath.Join(e.tmp, name))
	cmd.Env = e.isolated()
	if err := cmd.Start(); err != nil {
		e.t.Fatal(err)
	}
	e.t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
	for i := 0; i < 100; i++ {
		if c, err := net.Dial("tcp", addr); err == nil {
			c.Close()
			return "ws://" + addr
		}
		time.Sleep(50 * time.Millisecond)
	}
	e.t.Fatalf("relay %s did not start", name)
	return ""
}

// publisher runs the reference CLI with its own identity.
func (e *env) publisher(who, relay string, args ...string) string {
	all := append(args, "--relay", relay, "--store", filepath.Join(e.tmp, who, "store"),
		"--config", filepath.Join(e.tmp, who, "config"))
	cmd := exec.Command(e.terse, all...)
	cmd.Env = e.isolated()
	out, err := cmd.CombinedOutput()
	if err != nil {
		e.t.Fatalf("terse %v: %v\n%s", args, err, out)
	}
	return string(out)
}

func (e *env) key(who, relay string) string {
	out := e.publisher(who, relay, "key", "show")
	for _, f := range strings.Fields(out) {
		if len(f) == 64 {
			return f
		}
	}
	e.t.Fatalf("no key in %q", out)
	return ""
}

func (e *env) read(dir string, extraEnv []string, args ...string) (string, error) {
	cmd := exec.Command(e.reader, append([]string{"--dir", filepath.Join(e.tmp, dir)}, args...)...)
	cmd.Env = e.isolated(extraEnv...)
	out, err := cmd.CombinedOutput()
	return string(out), err
}

func (e *env) mustRead(dir string, args ...string) string {
	out, err := e.read(dir, nil, args...)
	if err != nil {
		e.t.Fatalf("tersereader %v: %v\n%s", args, err, out)
	}
	return out
}

func (e *env) file(name, content string) string {
	p := filepath.Join(e.tmp, name)
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		e.t.Fatal(err)
	}
	return p
}

func idOf(s string) string { return blake3.Hex([]byte(s)) }

func TestInterop(t *testing.T) {
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
	relayA := e.startRelay("relayA")
	relayB := e.startRelay("relayB")

	// Alice publishes on relay A: a small article linking to a blob that
	// no entry names (prefetched), a large one (fetched on request).
	e.publisher("alice", relayA, "key", "new")
	alice := e.key("alice", relayA)
	e.publisher("alice", relayA, "profile", "set", "--name", "Alice")
	linked := "# Linked\n\nOnly reached by a link.\n"
	e.publisher("alice", relayA, "put", e.file("linked.txt", linked))
	small := fmt.Sprintf("# Small\n\nSee terse:%s and [Bob](terse:feed/%s@relay.example).\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n", idOf(linked), alice)
	e.publisher("alice", relayA, "feed", "publish", e.file("small.txt", small), "--title", "Small")
	large := "# Large\n\n" + strings.Repeat("A long paragraph of the large article. ", 600) + "\nTHE-END\n"
	e.publisher("alice", relayA, "feed", "publish", e.file("large.txt", large), "--title", "Large")

	r := "reader"
	e.mustRead(r, "relay", "add", relayA)
	if _, err := e.read(r, nil, "subscribe", alice+"@bad..host"); err == nil {
		t.Error("a malformed address was accepted")
	}
	e.mustRead(r, "subscribe", alice)
	out := e.mustRead(r, "sync", "--timeout", "20s")
	if !strings.Contains(out, "2 new in the timeline") {
		t.Errorf("sync: %s", out)
	}
	list := e.mustRead(r, "list")
	lines := strings.Split(strings.TrimRight(list, "\n"), "\n")
	if len(lines) != 2 || !strings.Contains(lines[0], "Large") || !strings.Contains(lines[1], "Small") {
		t.Fatalf("list:\n%s", list)
	}
	if !strings.Contains(lines[0], "Alice (") || !strings.HasPrefix(lines[0], " ") || !strings.HasPrefix(lines[1], "*") {
		t.Errorf("names or automatic download wrong:\n%s", list)
	}
	if _, err := os.Stat(filepath.Join(e.tmp, r, "blobs", idOf(linked))); err != nil {
		t.Error("the linked article was not prefetched")
	}
	if _, err := os.Stat(filepath.Join(e.tmp, r, "blobs", idOf(large))); err == nil {
		t.Error("the large article was downloaded automatically")
	}
	// Show: from the store, and the large one from the relay on request.
	if got := e.mustRead(r, "--dump", idOf(small), "--tokens"); got != article.Dump(article.Parse([]byte(small))) {
		t.Errorf("dump of the synced article differs:\n%s", got)
	}
	if got := e.mustRead(r, "show", idOf(large), "--plain"); !strings.Contains(got, "THE-END") || !strings.HasPrefix(got, "Large\n") {
		t.Errorf("show large: %.200s", got)
	}
	shown := e.mustRead(r, "show", alice, "2")
	if !strings.HasPrefix(shown, "Small\n") {
		t.Errorf("show by seq: %.100s", shown)
	}
	feed := e.mustRead(r, "feed", alice)
	if !strings.Contains(feed, "Feed Alice (") || !strings.Contains(feed, "profile: Alice") {
		t.Errorf("feed:\n%s", feed)
	}

	// Late entries: only what is new comes (after = highest held).
	e.publisher("alice", relayA, "feed", "publish", e.file("third.txt", "Third.\n"), "--title", "Third")
	if out := e.mustRead(r, "sync"); !strings.Contains(out, "1 new in the timeline") {
		t.Errorf("second sync: %s", out)
	}
	before := e.mustRead(r, "list")
	e.mustRead(r, "unsubscribe", alice)
	if l := e.mustRead(r, "list"); strings.TrimSpace(l) != "" {
		t.Errorf("timeline after unsubscribing:\n%s", l)
	}
	e.mustRead(r, "subscribe", alice)
	if after := e.mustRead(r, "list"); after != before {
		t.Errorf("subscribing again changed the timeline:\n%s\n---\n%s", before, after)
	}

	// Hints: Carol publishes only on relay B. With hints allowed on
	// loopback (a test switch), the reader follows her there.
	e.publisher("carol", relayB, "key", "new")
	carol := e.key("carol", relayB)
	e.publisher("carol", relayB, "feed", "publish", e.file("c.txt", "From Carol.\n"), "--title", "Carol's")
	hint := strings.TrimPrefix(relayB, "ws://")
	for _, c := range []struct {
		dir   string
		env   []string
		found bool
	}{{"hinted", []string{"TERSEREADER_ALLOW_PRIVATE=1"}, true}, {"refused", nil, false}} {
		e.mustRead(c.dir, "relay", "add", relayA)
		e.mustRead(c.dir, "subscribe", "terse:feed/"+carol+"@"+hint)
		out, _ := e.read(c.dir, c.env, "sync", "--timeout", "8s")
		l := e.mustRead(c.dir, "list")
		if got := strings.Contains(l, "Carol's"); got != c.found {
			t.Errorf("%s: found %v, want %v\n%s\n%s", c.dir, got, c.found, out, l)
		}
		if c.found && !strings.Contains(out, "followed through hinted relays") {
			t.Errorf("the reader did not say it follows through a hinted relay: %s", out)
		}
	}

	// The manual, as the reference CLI publishes it.
	e.publisher("manual", relayA, "key", "new")
	e.publisher("manual", relayA, "manual", "publish")
	m := e.key("manual", relayA)
	e.mustRead("man", "relay", "add", relayA)
	e.mustRead("man", "subscribe", m)
	e.mustRead("man", "sync", "--timeout", "30s")
	ml := strings.Split(strings.TrimSpace(e.mustRead("man", "list")), "\n")
	if len(ml) < 20 {
		t.Fatalf("manual: %d articles", len(ml))
	}
	for _, l := range ml {
		f := strings.Fields(l)
		if f[0] != "*" {
			t.Errorf("not downloaded: %s", l)
			continue
		}
		out := e.mustRead("man", "--dump", f[1], "--plain")
		if len(out) < 200 {
			t.Errorf("%s: dump too short", l)
		}
	}
	t.Logf("manual: %d articles synced and dumped", len(ml))
}
