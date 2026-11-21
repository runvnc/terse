package localnode

import (
	"bytes"
	"testing"

	"tersereader/internal/local"
)

// A reply that would pass the limit carries as many records as fit
// (LOCAL.md 4.3): Subscriptions the first, Feed the latest, in order.
func TestWithRecordsKeepsWhatFits(t *testing.T) {
	const size = 300_000 // six fit in 2 MiB, not seven
	var parts [][]byte
	for i := 0; i < 10; i++ {
		parts = append(parts, bytes.Repeat([]byte{byte(i)}, size))
	}
	for _, latest := range []bool{false, true} {
		m := withRecords(local.New(0xa3, 1).U8(local.Done), parts, latest)
		if len(m) > local.MaxMessage {
			t.Fatalf("%d bytes", len(m))
		}
		in := local.NewR(m)
		in.Head()
		in.U8()
		n := int(in.U16())
		if n != 6 {
			t.Fatalf("latest %v: %d records", latest, n)
		}
		first := 0
		if latest {
			first = 10 - n
		}
		for i := 0; i < n; i++ {
			if p := in.N(size); p[0] != byte(first+i) {
				t.Errorf("latest %v: record %d is %d", latest, i, p[0])
			}
		}
		if !in.Done() {
			t.Error("bytes left")
		}
	}
	// Under the limit, all of them.
	m := withRecords(local.New(0xa3, 1).U8(local.Done), parts[:3], false)
	if len(m) != 8+3*size {
		t.Errorf("%d bytes", len(m))
	}
}
