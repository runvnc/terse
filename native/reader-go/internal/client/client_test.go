package client

import (
	"encoding/binary"
	"net"
	"net/http"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"tersereader/internal/blake3"
	"tersereader/internal/store"
	"tersereader/internal/terse"
	"tersereader/internal/vecfile"
	"tersereader/internal/ws"
)

// fakeRelay is a relay that pushes the vector entries, an invalid one
// among them, and lies about one blob.
type fakeRelay struct {
	t       *testing.T
	entries [][]byte
	blobs   map[[32]byte][]byte
	lie     [32]byte // the get of this ID returns other bytes
	mu      sync.Mutex
	gets    map[[32]byte]int
	subs    []uint32 // `after` of each subscribe
}

func (f *fakeRelay) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	c, err := ws.Accept(w, r)
	if err != nil {
		return
	}
	defer c.Close()
	for {
		m, err := c.ReadMessage()
		if err != nil {
			return
		}
		req := m[1:5]
		switch m[0] {
		case terse.MsgSubscribe:
			after := binary.LittleEndian.Uint32(m[37:])
			f.mu.Lock()
			f.subs = append(f.subs, after)
			f.mu.Unlock()
			c.WriteMessage(append(append([]byte{0x84}, req...), 1, 9, 0, 0, 0))
			for _, e := range f.entries {
				_, seq, _ := terse.EntrySeq(e)
				if seq > after {
					c.WriteMessage(append([]byte{0x90}, e...))
				}
			}
		case terse.MsgGet:
			var id [32]byte
			copy(id[:], m[5:])
			f.mu.Lock()
			f.gets[id]++
			f.mu.Unlock()
			b, ok := f.blobs[id]
			switch {
			case id == f.lie:
				c.WriteMessage(append(append([]byte{0x81}, req...), append([]byte{1}, []byte("not the blob")...)...))
			case ok:
				c.WriteMessage(append(append([]byte{0x81}, req...), append([]byte{1}, b...)...))
			default:
				c.WriteMessage(append(append([]byte{0x81}, req...), 2))
			}
		case 0x77:
			// an unknown type must be harmless both ways
		}
		// an unknown reply type pushed to the client is ignored by it
		c.WriteMessage([]byte{0x99, 1, 2, 3})
	}
}

func TestClientAgainstFakeRelay(t *testing.T) {
	dir, err := vecfile.Dir()
	if err != nil {
		t.Fatal(err)
	}
	recs, err := vecfile.Parse(filepath.Join(dir, "entries.txt"))
	if err != nil {
		t.Fatal(err)
	}
	byName := map[string]*vecfile.Record{}
	for _, r := range recs {
		byName[r.Name] = r
	}
	raw := func(name string) []byte {
		v, _ := byName[name].Get("bytes")
		b, _ := vecfile.Hex(v)
		return b
	}
	f := &fakeRelay{t: t, blobs: map[[32]byte][]byte{}, gets: map[[32]byte]int{}}
	for _, n := range []string{"bad-signature", "article", "profile", "program", "document", "article-reply", "unknown-kind-9", "changed-title"} {
		f.entries = append(f.entries, raw(n))
		if h, ok := byName[n].Get("blob-hex"); ok {
			b, _ := vecfile.Hex(h)
			f.blobs[blake3.Sum256(b)] = b
		}
	}
	prof, _ := terse.ParseEntry(raw("profile"))
	f.lie = prof.Blob

	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	srv := &http.Server{Handler: f}
	go srv.Serve(ln)
	defer srv.Close()

	st, err := store.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()
	key, _ := terse.ParseAddress(prof.KeyHex())
	if _, err := st.Subscribe(key); err != nil {
		t.Fatal(err)
	}
	opt := DefaultOptions()
	opt.Relays = []string{"ws://" + ln.Addr().String()}
	cl := New(st, opt)
	cl.Start()
	if !cl.WaitIdle(10 * time.Second) {
		t.Fatal("not idle")
	}
	cl.Stop()

	es := st.FeedEntries(prof.Key)
	var seqs []uint32
	for _, e := range es {
		seqs = append(seqs, e.Seq)
	}
	if want := []uint32{1, 2, 3, 4, 5, 9}; len(seqs) != len(want) {
		t.Fatalf("seqs %v, want %v", seqs, want)
	}
	if string(es[0].Raw) != string(raw("article")) {
		t.Error("seq 1 is not the valid article: an invalid entry was kept")
	}
	var tl []uint32
	for _, e := range st.Timeline() {
		tl = append(tl, e.Seq)
	}
	if len(tl) != 3 || tl[0] != 5 || tl[1] != 4 || tl[2] != 1 {
		t.Errorf("timeline %v, want [5 4 1]", tl)
	}
	byKind := map[byte]*terse.Entry{}
	for _, e := range es {
		byKind[e.Kind] = e
	}
	if _, ok := st.BlobForEntry(es[0]); !ok {
		t.Error("the article's blob was not downloaded")
	}
	if _, ok := st.BlobForEntry(byKind[terse.KindDocument]); !ok {
		t.Error("the document's blob was not downloaded")
	}
	if st.HasBlob(prof.Blob) {
		t.Error("a blob that does not match its ID was stored")
	}
	if f.gets[byKind[terse.KindProgram].Blob] != 0 {
		t.Error("a reader downloaded a program image")
	}
	if f.gets[byKind[9].Blob] != 0 {
		t.Error("the blob of an undefined kind was downloaded")
	}

	// A second run subscribes with `after` the highest seq held.
	cl = New(st, opt)
	cl.Start()
	cl.WaitIdle(10 * time.Second)
	cl.Stop()
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.subs) != 2 || f.subs[0] != 0 || f.subs[1] != 9 {
		t.Errorf("subscribe afters %v, want [0 9]", f.subs)
	}
}

func TestScanArticleLinks(t *testing.T) {
	h := "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
	g := "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
	text := "a terse:" + h + ". xterse:" + g + " terse:feed/" + g + " [t](terse:" + g + ") `terse:" + h + "` terse:" + h + "0"
	got := ScanArticleLinks([]byte(text))
	if len(got) != 2 || terse.KeyHex(got[0][:]) != h || terse.KeyHex(got[1][:]) != g {
		t.Errorf("scan: %d links", len(got))
	}
}
