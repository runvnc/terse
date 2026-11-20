// Package store is the reader's local state: the blob store, every valid
// entry received for subscribed feeds in arrival order, the subscriptions,
// the relays and the nicknames. It lives in one directory:
//
//	blobs/<id>       one file per blob, named by its BLAKE3-256 ID
//	entries          every entry kept, in the order first stored (the
//	                 arrival order of NETWORK.md 4.4): a 2-byte
//	                 little-endian length, then the entry
//	subscriptions    one feed address per line
//	relays           one ws:// URL per line
//	nicknames        `<key> <name>` per line
package store

import (
	"bufio"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"syscall"

	"tersereader/internal/blake3"
	"tersereader/internal/terse"
)

// Sub is a subscription: a feed key and the hints it was subscribed with.
type Sub struct {
	Addr terse.Address
}

// Store is the reader's local state. It is safe for concurrent use.
type Store struct {
	dir  string
	lock *os.File

	mu      sync.Mutex
	log     *os.File
	entries []*terse.Entry // arrival order
	byFeed  map[[32]byte]map[uint32]*terse.Entry
	byBlob  map[[32]byte][]*terse.Entry
	subs    []Sub
	relays  []string
	nick    map[[32]byte]string
}

// Open opens (creating) a store directory and takes its lock, so that two
// readers do not write it at once.
func Open(dir string) (*Store, error) {
	if err := os.MkdirAll(filepath.Join(dir, "blobs"), 0o755); err != nil {
		return nil, err
	}
	lf, err := os.OpenFile(filepath.Join(dir, "lock"), os.O_CREATE|os.O_RDWR, 0o644)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(lf.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		lf.Close()
		return nil, fmt.Errorf("%s is in use by another reader", dir)
	}
	s := &Store{
		dir: dir, lock: lf,
		byFeed: map[[32]byte]map[uint32]*terse.Entry{},
		byBlob: map[[32]byte][]*terse.Entry{},
		nick:   map[[32]byte]string{},
	}
	if err := s.loadEntries(); err != nil {
		lf.Close()
		return nil, err
	}
	s.loadSubs()
	s.relays = readLines(filepath.Join(dir, "relays"))
	for _, l := range readLines(filepath.Join(dir, "nicknames")) {
		k, n, ok := strings.Cut(l, " ")
		kb, err := hex.DecodeString(k)
		if ok && err == nil && len(kb) == 32 && terse.ValidDisplayName([]byte(n)) {
			var key [32]byte
			copy(key[:], kb)
			s.nick[key] = n
		}
	}
	return s, nil
}

// Close releases the store.
func (s *Store) Close() {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.log != nil {
		s.log.Close()
	}
	s.lock.Close()
}

// Dir is the store's directory.
func (s *Store) Dir() string { return s.dir }

func readLines(path string) []string {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	var out []string
	for _, l := range strings.Split(string(b), "\n") {
		if l = strings.TrimSpace(l); l != "" && !strings.HasPrefix(l, "#") {
			out = append(out, l)
		}
	}
	return out
}

func writeLines(path string, lines []string) error {
	tmp := path + ".tmp"
	data := strings.Join(lines, "\n")
	if len(lines) > 0 {
		data += "\n"
	}
	if err := os.WriteFile(tmp, []byte(data), 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

func (s *Store) loadEntries() error {
	path := filepath.Join(s.dir, "entries")
	f, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0o644)
	if err != nil {
		return err
	}
	r := bufio.NewReader(f)
	good := int64(0)
	for {
		var lb [2]byte
		if _, err := io.ReadFull(r, lb[:]); err != nil {
			break
		}
		n := int(binary.LittleEndian.Uint16(lb[:]))
		buf := make([]byte, n)
		if _, err := io.ReadFull(r, buf); err != nil {
			break
		}
		good += int64(2 + n)
		// Entries were verified when stored; verify again, it is cheap.
		e, err := terse.ParseEntry(buf)
		if err != nil {
			continue
		}
		s.index(e)
	}
	// Drop a torn record at the end.
	if err := f.Truncate(good); err != nil {
		f.Close()
		return err
	}
	if _, err := f.Seek(good, io.SeekStart); err != nil {
		f.Close()
		return err
	}
	s.log = f
	return nil
}

func (s *Store) index(e *terse.Entry) bool {
	m := s.byFeed[e.Key]
	if m == nil {
		m = map[uint32]*terse.Entry{}
		s.byFeed[e.Key] = m
	}
	if _, ok := m[e.Seq]; ok {
		return false
	}
	m[e.Seq] = e
	s.entries = append(s.entries, e)
	s.byBlob[e.Blob] = append(s.byBlob[e.Blob], e)
	return true
}

// AddEntry stores a verified entry unless one with its feed and seq is
// held (the first received is kept). It reports whether it was stored.
func (s *Store) AddEntry(e *terse.Entry) (bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if m := s.byFeed[e.Key]; m != nil {
		if _, ok := m[e.Seq]; ok {
			return false, nil
		}
	}
	rec := binary.LittleEndian.AppendUint16(nil, uint16(len(e.Raw)))
	rec = append(rec, e.Raw...)
	if _, err := s.log.Write(rec); err != nil {
		return false, err
	}
	s.index(e)
	return true, nil
}

// HighestSeq is the highest seq held for a feed, or 0.
func (s *Store) HighestSeq(key [32]byte) uint32 {
	s.mu.Lock()
	defer s.mu.Unlock()
	var h uint32
	for seq := range s.byFeed[key] {
		if seq > h {
			h = seq
		}
	}
	return h
}

// FeedEntries returns a feed's entries in increasing seq.
func (s *Store) FeedEntries(key [32]byte) []*terse.Entry {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []*terse.Entry
	for _, e := range s.byFeed[key] {
		out = append(out, e)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Seq < out[j].Seq })
	return out
}

// EntriesForBlob returns the entries held that point at a blob.
func (s *Store) EntriesForBlob(id [32]byte) []*terse.Entry {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]*terse.Entry(nil), s.byBlob[id]...)
}

// Feeds returns every feed key the store holds entries of.
func (s *Store) Feeds() [][32]byte {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out [][32]byte
	for k := range s.byFeed {
		out = append(out, k)
	}
	sort.Slice(out, func(i, j int) bool { return string(out[i][:]) < string(out[j][:]) })
	return out
}

// Timeline returns the article and document entries (kinds 1 and 4) of
// the subscribed feeds, newest first in arrival order (NETWORK.md 4.4).
func (s *Store) Timeline() []*terse.Entry {
	s.mu.Lock()
	defer s.mu.Unlock()
	subscribed := map[[32]byte]bool{}
	for _, sb := range s.subs {
		subscribed[sb.Addr.Key] = true
	}
	var out []*terse.Entry
	for i := len(s.entries) - 1; i >= 0; i-- {
		e := s.entries[i]
		if subscribed[e.Key] && (e.Kind == terse.KindArticle || e.Kind == terse.KindDocument) {
			out = append(out, e)
		}
	}
	return out
}

// ProfileInForce is the feed's valid profile entry with the highest seq.
func (s *Store) ProfileInForce(key [32]byte) *terse.Entry {
	s.mu.Lock()
	defer s.mu.Unlock()
	var best *terse.Entry
	for _, e := range s.byFeed[key] {
		if e.Kind == terse.KindProfile && (best == nil || e.Seq > best.Seq) {
			best = e
		}
	}
	return best
}

// Profile returns the parsed profile document in force, if its blob is
// held, matches the entry's size and parses.
func (s *Store) Profile(key [32]byte) (terse.Profile, bool) {
	e := s.ProfileInForce(key)
	if e == nil {
		return terse.Profile{}, false
	}
	b, ok := s.BlobForEntry(e)
	if !ok {
		return terse.Profile{}, false
	}
	return terse.ParseProfile(b)
}

// Name is the name shown for a feed: the user's nickname, else the
// display name of the profile in force, else "".
func (s *Store) Name(key [32]byte) string {
	s.mu.Lock()
	n := s.nick[key]
	s.mu.Unlock()
	if n != "" {
		return n
	}
	if e := s.ProfileInForce(key); e != nil {
		return e.Title
	}
	return ""
}

// Label names a feed for people: its name, if any, always with the short
// form of its key beside it (NETWORK.md 4.6).
func (s *Store) Label(key [32]byte) string {
	short := terse.ShortKey(terse.KeyHex(key[:]))
	if n := s.Name(key); n != "" {
		return n + " (" + short + ")"
	}
	return short
}

// SetNickname sets (or with "" clears) a nickname.
func (s *Store) SetNickname(key [32]byte, name string) error {
	if name != "" && !terse.ValidDisplayName([]byte(name)) {
		return errors.New("a nickname must be a display name: 1 to 48 bytes, no control characters, no blank first or last")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if name == "" {
		delete(s.nick, key)
	} else {
		s.nick[key] = name
	}
	var lines []string
	for k, n := range s.nick {
		lines = append(lines, terse.KeyHex(k[:])+" "+n)
	}
	sort.Strings(lines)
	return writeLines(filepath.Join(s.dir, "nicknames"), lines)
}

// --- blobs ---

func (s *Store) blobPath(id [32]byte) string {
	return filepath.Join(s.dir, "blobs", hex.EncodeToString(id[:]))
}

// HasBlob reports whether a blob is held.
func (s *Store) HasBlob(id [32]byte) bool {
	_, err := os.Stat(s.blobPath(id))
	return err == nil
}

// Blob returns a held blob. The bytes are hashed again, so a damaged file
// is never used.
func (s *Store) Blob(id [32]byte) ([]byte, bool) {
	b, err := os.ReadFile(s.blobPath(id))
	if err != nil || len(b) == 0 || blake3.Sum256(b) != id {
		return nil, false
	}
	return b, true
}

// BlobForEntry returns an entry's blob if it is held and its length is
// the entry's size (else the blob is not associated with the entry,
// NETWORK.md 4.4).
func (s *Store) BlobForEntry(e *terse.Entry) ([]byte, bool) {
	b, ok := s.Blob(e.Blob)
	if !ok || uint32(len(b)) != e.Size {
		return nil, false
	}
	return b, true
}

// PutBlob verifies a blob against its ID and stores it.
func (s *Store) PutBlob(id [32]byte, b []byte) error {
	if len(b) < 1 || len(b) > terse.MaxBlob {
		return errors.New("blob size out of range")
	}
	if blake3.Sum256(b) != id {
		return errors.New("blob does not match its ID")
	}
	p := s.blobPath(id)
	tmp := p + ".tmp"
	if err := os.WriteFile(tmp, b, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, p)
}

// --- subscriptions and relays ---

func (s *Store) loadSubs() {
	for _, l := range readLines(filepath.Join(s.dir, "subscriptions")) {
		a, err := terse.ParseAddress(l)
		if err == nil {
			s.subs = append(s.subs, Sub{Addr: a})
		}
	}
}

func (s *Store) saveSubs() error {
	var lines []string
	for _, sb := range s.subs {
		lines = append(lines, sb.Addr.String())
	}
	return writeLines(filepath.Join(s.dir, "subscriptions"), lines)
}

// Subscriptions returns the subscribed feeds in the order subscribed.
func (s *Store) Subscriptions() []Sub {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]Sub(nil), s.subs...)
}

// Subscribed reports whether a feed is subscribed, with its hints.
func (s *Store) Subscribed(key [32]byte) (Sub, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, sb := range s.subs {
		if sb.Addr.Key == key {
			return sb, true
		}
	}
	return Sub{}, false
}

// Subscribe adds a feed (a newer address replaces the hints of one
// already subscribed). It reports whether the feed is new.
func (s *Store) Subscribe(a terse.Address) (bool, error) {
	if !terse.ValidFeedKey(a.Key[:]) {
		return false, errors.New("not a valid feed key")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	for i, sb := range s.subs {
		if sb.Addr.Key == a.Key {
			s.subs[i].Addr = a
			return false, s.saveSubs()
		}
	}
	s.subs = append(s.subs, Sub{Addr: a})
	return true, s.saveSubs()
}

// Unsubscribe removes a feed from the list (its entries stay in the store).
func (s *Store) Unsubscribe(key [32]byte) (bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i, sb := range s.subs {
		if sb.Addr.Key == key {
			s.subs = append(s.subs[:i], s.subs[i+1:]...)
			return true, s.saveSubs()
		}
	}
	return false, nil
}

// Relays returns the configured relays.
func (s *Store) Relays() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]string(nil), s.relays...)
}

// SetRelays replaces the configured relays (at most eight).
func (s *Store) SetRelays(urls []string) error {
	if len(urls) > 8 {
		return errors.New("at most eight relays")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.relays = append([]string(nil), urls...)
	return writeLines(filepath.Join(s.dir, "relays"), s.relays)
}
