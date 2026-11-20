package terse

import (
	"bufio"
	"encoding/hex"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"tersereader/internal/blake3"
	"tersereader/internal/vecfile"
)

type sessionMsg struct {
	dir     string // c>, r>, p>
	bytes   []byte
	comment string
}

func loadSession(t *testing.T) []sessionMsg {
	dir, err := vecfile.Dir()
	if err != nil {
		t.Fatal(err)
	}
	f, err := os.Open(filepath.Join(dir, "relay-session.txt"))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	var out []sessionMsg
	comment := ""
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 1<<20), 1<<22)
	for sc.Scan() {
		l := sc.Text()
		if strings.HasPrefix(l, "# ") {
			comment = l[2:]
			continue
		}
		d, h, ok := strings.Cut(l, " ")
		if !ok || (d != "c>" && d != "r>" && d != "p>") {
			continue
		}
		b, err := hex.DecodeString(h)
		if err != nil {
			t.Fatal(err)
		}
		out = append(out, sessionMsg{d, b, comment})
	}
	return out
}

// TestRelaySession checks the messages a reader sends (get, subscribe,
// unsubscribe) byte for byte, and decodes every relay message the way the
// reader does.
func TestRelaySession(t *testing.T) {
	msgs := loadSession(t)
	if len(msgs) < 20 {
		t.Fatalf("only %d messages", len(msgs))
	}
	var pendingGet [32]byte
	var gets, subs, unsubs, entries, ignored int
	for _, m := range msgs {
		switch m.dir {
		case "c>":
			req := uint32(m.bytes[1]) | uint32(m.bytes[2])<<8 | uint32(m.bytes[3])<<16 | uint32(m.bytes[4])<<24
			var enc []byte
			switch m.bytes[0] {
			case MsgGet:
				copy(pendingGet[:], m.bytes[5:])
				enc = EncodeGet(req, pendingGet)
				gets++
			case MsgSubscribe:
				var k [32]byte
				copy(k[:], m.bytes[5:])
				after := uint32(m.bytes[37]) | uint32(m.bytes[38])<<8 | uint32(m.bytes[39])<<16 | uint32(m.bytes[40])<<24
				enc = EncodeSubscribe(req, k, after)
				subs++
			case MsgUnsubscribe:
				var k [32]byte
				copy(k[:], m.bytes[5:])
				enc = EncodeUnsubscribe(req, k)
				unsubs++
			default:
				continue // put, publish, find, lookup: a reader does not send them
			}
			if hex.EncodeToString(enc) != hex.EncodeToString(m.bytes) {
				t.Errorf("%s: encoded %x, want %x", m.comment, enc, m.bytes)
			}
		case "r>":
			r, err := DecodeReply(m.bytes)
			if err != nil {
				t.Errorf("%s: %v", m.comment, err)
				continue
			}
			switch {
			case r.Ignored:
				ignored++
			case r.Type == MsgEntry:
				entries++
				if _, err := ParseEntry(r.Entry); err != nil {
					t.Errorf("%s: pushed entry invalid: %v", m.comment, err)
				}
			case r.Type == MsgGetReply && r.Status == StatusDone:
				if blake3.Sum256(r.Blob) != pendingGet {
					t.Errorf("%s: blob does not match the ID asked for", m.comment)
				}
			case r.Type == MsgSubscribeReply:
				if r.Status != StatusDone || r.Highest != 2 {
					t.Errorf("%s: status %d highest %d", m.comment, r.Status, r.Highest)
				}
			}
		}
	}
	if gets != 2 || subs != 1 || unsubs != 1 || entries != 2 || ignored < 5 {
		t.Errorf("counts: gets %d subs %d unsubs %d entries %d ignored %d", gets, subs, unsubs, entries, ignored)
	}
}
