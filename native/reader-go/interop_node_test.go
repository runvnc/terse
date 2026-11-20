package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"tersereader/internal/article"
)

// TestInteropNode: the reader split in two (LOCAL.md). The reference relay
// and CLI as black boxes; `tersereader node` as a process serving a socket
// in the test's temp dir; the thin reader (-tags thin, no cryptography)
// syncs, lists and dumps through it; a full reader on the same node sees a
// change the thin one made.
func TestInteropNode(t *testing.T) {
	if testing.Short() {
		t.Skip("short")
	}
	e := &env{t: t, tmp: t.TempDir(), terse: findBin(t, "terse"), relayB: findBin(t, "terse-relay")}
	e.reader = filepath.Join(e.tmp, "tersereader")
	thin := filepath.Join(e.tmp, "tersereader-thin")
	for _, b := range [][]string{{"-o", e.reader}, {"-tags", "thin", "-o", thin}} {
		build := exec.Command("go", append(append([]string{"build", "-p", "4"}, b...), ".")...)
		build.Env = append(os.Environ(), "CGO_ENABLED=0")
		if out, err := build.CombinedOutput(); err != nil {
			t.Fatalf("build: %v\n%s", err, out)
		}
	}
	relay := e.startRelay("relay")
	e.publisher("alice", relay, "key", "new")
	alice := e.key("alice", relay)
	e.publisher("alice", relay, "profile", "set", "--name", "Alice")
	text := "# Through the node\n\nRead with no cryptography: café, 5 €.\n"
	e.publisher("alice", relay, "feed", "publish", e.file("a.txt", text), "--title", "Through the node")

	dir := "node-store"
	e.mustRead(dir, "--node", "in-process", "relay", "add", relay)
	e.mustRead(dir, "--node", "in-process", "subscribe", alice)

	sock := filepath.Join(e.tmp, "xdg-run", "terse", "node.sock")
	node := exec.Command(e.reader, "--dir", filepath.Join(e.tmp, dir), "node")
	node.Env = e.isolated()
	var nodeOut strings.Builder
	node.Stderr = &nodeOut
	if err := node.Start(); err != nil {
		t.Fatal(err)
	}
	stopped := false
	stop := func() {
		if !stopped {
			stopped = true
			node.Process.Signal(os.Interrupt)
			node.Wait()
		}
	}
	t.Cleanup(stop)
	for i := 0; ; i++ {
		if _, err := os.Stat(sock); err == nil {
			break
		}
		if i == 200 {
			t.Fatalf("the node did not start: %s", nodeOut.String())
		}
		time.Sleep(25 * time.Millisecond)
	}

	runThin := func(args ...string) string {
		cmd := exec.Command(thin, args...)
		cmd.Env = e.isolated()
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("thin %v: %v\n%s", args, err, out)
		}
		return string(out)
	}
	runThin("sync", "--timeout", "20s")
	list := runThin("list")
	if !strings.Contains(list, "Through the node") || !strings.Contains(list, "Alice (") ||
		!strings.Contains(list, "[needs full text]") || !strings.HasPrefix(list, "*") {
		t.Errorf("list through the node: %s", list)
	}
	if got := runThin("--dump", idOf(text), "--tokens"); got != article.Dump(article.Parse([]byte(text))) {
		t.Errorf("dump through the node:\n%s", got)
	}
	if got := runThin("--dump", idOf(text), "--plain"); !strings.Contains(got, "Read with no cryptography") {
		t.Errorf("plain dump: %s", got)
	}
	// Two readers: the thin one names the feed; the full one, in auto mode,
	// uses the running node (whose store is locked) and sees the name.
	runThin("nickname", alice, "Al")
	if got := e.mustRead(dir, "feeds"); !strings.Contains(got, "Al (") {
		t.Errorf("full reader through the node: %s", got)
	}
	// A thin reader with no node says so.
	cmd := exec.Command(thin, "list")
	cmd.Env = e.isolated("TERSE_NODE_SOCKET=" + filepath.Join(e.tmp, "none", "node.sock"))
	if out, err := cmd.CombinedOutput(); err == nil || !strings.Contains(string(out), "no node") {
		t.Errorf("thin reader without a node: %v %s", err, out)
	}
	stop()
	if _, err := os.Stat(sock); err == nil {
		t.Error("the node left its socket behind")
	}
	if t.Failed() {
		t.Logf("node: %s", nodeOut.String())
	}
}
