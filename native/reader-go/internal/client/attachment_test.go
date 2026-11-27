package client

import (
	"testing"
	"time"

	"tersereader/internal/blake3"
	"tersereader/internal/terse"
)

// NETWORK.md 4.2: an attachment entry (kind 5) is a valid entry, kept,
// and not listed in the timeline; its blob is not downloaded
// automatically.
func TestAttachmentNotListed(t *testing.T) {
	r := newTestRelay(t)
	f := newFeed(t)
	article, part := []byte("# A\n\nSee the part.\n"), []byte("the part")
	r.blobs[blake3.Sum256(article)] = article
	r.blobs[blake3.Sum256(part)] = part
	r.addEntry(f.key, f.entryFor(1, terse.KindArticle, article, "A"))
	r.addEntry(f.key, f.entryFor(2, terse.KindAttachment, part, ""))
	st := openStore(t)
	if _, err := st.Subscribe(f.addr(t)); err != nil {
		t.Fatal(err)
	}
	opt := testOptions(r)
	opt.AutoLimit = 16384
	cl := New(st, opt)
	cl.Start()
	defer cl.Stop()
	waitFor(t, "both entries", func() bool { return len(st.FeedEntries(f.key)) == 2 })
	if !cl.WaitIdle(time.Minute) {
		t.Fatal("not idle")
	}
	tl := st.Timeline()
	if len(tl) != 1 || tl[0].Kind != terse.KindArticle {
		t.Errorf("timeline %v: want the article only", tl)
	}
	for _, e := range st.FeedEntries(f.key) {
		if _, ok := st.BlobForEntry(e); ok && e.Kind == terse.KindAttachment {
			t.Error("the attachment's blob was downloaded automatically")
		}
	}
}
