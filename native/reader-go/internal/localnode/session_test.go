package localnode_test

import (
	"bufio"
	"bytes"
	"encoding/hex"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"tersereader/internal/blake3"
	"tersereader/internal/client"
	"tersereader/internal/local"
	"tersereader/internal/localclient"
	"tersereader/internal/localnode"
	"tersereader/internal/node"
	"tersereader/internal/store"
	"tersereader/internal/terse"
	"tersereader/internal/vecfile"
)

type msg struct {
	dir, note string
	b         []byte
}

func loadLocalSession(t *testing.T) []msg {
	dir, err := vecfile.Dir()
	if err != nil {
		t.Fatal(err)
	}
	f, err := os.Open(filepath.Join(dir, "local-session.txt"))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	var out []msg
	note := ""
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 1<<20), 1<<22)
	for sc.Scan() {
		l := sc.Text()
		if strings.HasPrefix(l, "# ") {
			note = l[2:]
			continue
		}
		d, h, ok := strings.Cut(l, " ")
		if !ok || (d != "c>" && d != "n>") {
			continue
		}
		b, err := hex.DecodeString(h)
		if err != nil {
			t.Fatal(err)
		}
		out = append(out, msg{d, note, b})
	}
	if len(out) < 30 {
		t.Fatalf("only %d messages", len(out))
	}
	return out
}

func entryVector(t *testing.T, name string) *terse.Entry {
	dir, _ := vecfile.Dir()
	recs, err := vecfile.Parse(filepath.Join(dir, "entries.txt"))
	if err != nil {
		t.Fatal(err)
	}
	for _, r := range recs {
		if r.Type == "entry" && r.Name == name {
			h, _ := r.Get("bytes")
			b, _ := vecfile.Hex(h)
			e, err := terse.ParseEntry(b)
			if err != nil {
				t.Fatal(err)
			}
			return e
		}
	}
	t.Fatalf("no entry %s", name)
	return nil
}

// socketDir is a temporary directory for a socket, in /tmp: a socket's
// path holds at most 103 bytes, and t.TempDir() on macOS (under $TMPDIR,
// in /var/folders) makes it longer.
func socketDir(t *testing.T) string {
	t.Helper()
	d, err := os.MkdirTemp("/tmp", "tr")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(d) })
	return d
}

// sessionNode is a node in the state local-session.txt starts from,
// serving a socket in a temporary directory.
func sessionNode(t *testing.T) (string, *node.Node) {
	tmp := t.TempDir()
	dir := filepath.Join(tmp, "store")
	st, err := store.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	for _, x := range []struct{ name, blob string }{{"article", "Hello, Terse!"}, {"profile", "listed: yes\n"}} {
		e := entryVector(t, x.name)
		if _, err := st.AddEntry(e); err != nil {
			t.Fatal(err)
		}
		if err := st.PutBlob(blake3.Sum256([]byte(x.blob)), []byte(x.blob)); err != nil {
			t.Fatal(err)
		}
	}
	a, err := terse.ParseAddress(terse.KeyHex(entryVector(t, "article").Key[:]))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := st.Subscribe(a); err != nil {
		t.Fatal(err)
	}
	st.Close()
	n, err := node.Open(dir, client.DefaultOptions())
	if err != nil {
		t.Fatal(err)
	}
	sock := filepath.Join(socketDir(t), "run", "terse", "node.sock")
	ln, err := local.Listen(sock)
	if err != nil {
		t.Fatal(err)
	}
	srv := localnode.Serve(n, ln)
	t.Cleanup(func() { srv.Close(); n.Close() })
	return sock, n
}

// local-session.txt: a node in the stated state answers the reader's
// messages with the node's messages (pushes skipped, reasons not compared).
func TestLocalSessionVectors(t *testing.T) {
	sock, _ := sessionNode(t)
	c, err := local.Dial(sock)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	r := bufio.NewReader(c)
	next := func() []byte {
		for {
			c.SetReadDeadline(time.Now().Add(5 * time.Second))
			m, err := local.ReadFrame(r)
			if err != nil {
				t.Fatalf("read: %v", err)
			}
			if m[0] != local.Changed && m[0] != local.Log {
				return m
			}
		}
	}
	replies := 0
	for _, m := range loadLocalSession(t) {
		switch {
		case m.dir == "c>":
			if err := local.WriteFrame(c, m.b); err != nil {
				t.Fatal(err)
			}
		case m.b[0] == local.Changed || m.b[0] == local.Log:
			// a push: may come anywhere, skipped
		default:
			got := next()
			want := m.b
			if local.IsLocalReply(want[0]) && len(want) > 5 && want[5] == local.Refused {
				want, got = want[:6], got[:min(6, len(got))]
			}
			if !bytes.Equal(got, want) {
				t.Errorf("%s:\n got %x\nwant %x", m.note, got, want)
			}
			replies++
		}
	}
	if replies < 15 {
		t.Fatalf("%d replies", replies)
	}
}

func sprint(v any) string { return fmt.Sprintf("%+v", v) }

// The socket client decodes what the node sends into the same values the
// node gives in process.
func TestClientAgreesWithNode(t *testing.T) {
	sock, n := sessionNode(t)
	cl, err := localclient.Dial(sock)
	if err != nil {
		t.Fatal(err)
	}
	defer cl.Close()
	same := func(what string, got, want any) {
		t.Helper()
		if g, w := strings.TrimSpace(sprint(got)), strings.TrimSpace(sprint(want)); g != w {
			t.Errorf("%s:\n got %s\nwant %s", what, g, w)
		}
	}
	same("timeline", cl.Timeline(), n.Timeline())
	same("subscriptions", cl.Subscriptions(), n.Subscriptions())
	key := terse.KeyHex(entryVector(t, "article").Key[:])
	f1, err1 := cl.Feed(key)
	f2, err2 := n.Feed(key)
	same("feed", f1, f2)
	same("feed error", err1, err2)
	same("label", cl.Label(key), n.Label(key))
	id := entryVector(t, "article").BlobHex()
	t1, ok1 := cl.ArticleTitle(id, 13)
	t2, ok2 := n.ArticleTitle(id, 13)
	same("title", []any{t1, ok1}, []any{t2, ok2})
	b1, ok1 := cl.HeldBlob(id)
	b2, ok2 := n.HeldBlob(id)
	same("held", []any{b1, ok1}, []any{b2, ok2})
	if tl := cl.Timeline(); len(tl) != 1 || tl[0].Title != "Hello" || !tl[0].Held || tl[0].Label != "Alice ("+terse.ShortKey(key)+")" {
		t.Errorf("timeline: %+v", tl)
	}
	if _, err := cl.Subscribe("not an address"); err == nil {
		t.Error("a malformed address was accepted")
	}
	if err := cl.SetRelays([]string{"http://x"}); err == nil {
		t.Error("a bad relay URL was accepted")
	}
}

// Two readers on one node: a change made by one is pushed to both.
func TestTwoReaders(t *testing.T) {
	sock, _ := sessionNode(t)
	a, err := localclient.Dial(sock)
	if err != nil {
		t.Fatal(err)
	}
	defer a.Close()
	b, err := localclient.Dial(sock)
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	key := terse.KeyHex(entryVector(t, "article").Key[:])
	if err := a.SetNickname(key, "Ally"); err != nil {
		t.Fatal(err)
	}
	for name, c := range map[string]*localclient.Client{"a": a, "b": b} {
		select {
		case <-c.Changed():
		case <-time.After(5 * time.Second):
			t.Errorf("reader %s: no Changed", name)
		}
	}
	if got := b.Label(key); !strings.HasPrefix(got, "Ally (") {
		t.Errorf("reader b sees %q", got)
	}
}

// The socket's directory is the user's alone and the socket 0600; a
// directory open to others is refused; a second node does not start.
func TestSocketPermissions(t *testing.T) {
	tmp := socketDir(t)
	sock := filepath.Join(tmp, "run", "terse", "node.sock")
	ln, err := local.Listen(sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	for path, want := range map[string]os.FileMode{filepath.Dir(sock): 0o700, sock: 0o600} {
		fi, err := os.Stat(path)
		if err != nil {
			t.Fatal(err)
		}
		if fi.Mode().Perm() != want {
			t.Errorf("%s: mode %o, want %o", path, fi.Mode().Perm(), want)
		}
	}
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			c.Close()
		}
	}()
	if _, err := local.Listen(sock); err != local.ErrRunning {
		t.Errorf("a second node on the same socket: %v", err)
	}
	open := filepath.Join(tmp, "open")
	os.MkdirAll(open, 0o755)
	os.Chmod(open, 0o755)
	if _, err := local.Listen(filepath.Join(open, "node.sock")); err == nil {
		t.Error("listened in a directory open to others")
	}
	if _, err := local.Dial(filepath.Join(open, "node.sock")); err == nil {
		t.Error("dialled in a directory open to others")
	}
	// Only unix sockets: the node never listens on the network.
	if _, ok := ln.Addr().(*net.UnixAddr); !ok {
		t.Errorf("listening on %v", ln.Addr())
	}
}
