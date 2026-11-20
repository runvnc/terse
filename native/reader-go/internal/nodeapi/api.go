// Package nodeapi is the seam between the two halves of tersereader:
//
//   - the network node (package node, over client, store, ws, blake3):
//     relay connections, verification of entries and blobs, the local
//     store, subscriptions, automatic download and prefetch;
//   - the reader (package main, article, render): parsing and showing
//     articles, the terminal interface, the commands.
//
// The reader sees the node only through the Node interface and the plain
// data types below (strings, numbers, byte slices), so that the two could
// later run as two programs talking over a local socket. Both halves also
// use package terse, which is pure data formats (addresses, names) with
// no I/O.
package nodeapi

import "time"

// Entry kinds (NETWORK.md 4.2).
const (
	KindArticle  = 1
	KindProfile  = 2
	KindProgram  = 3
	KindDocument = 4
)

// Entry is a verified feed entry the node holds.
type Entry struct {
	Feed  string // feed key, 64 hex digits
	Label string // the feed's name with the short form of its key
	Seq   uint32
	Time  uint32 // Unix seconds, as the author claims
	Kind  byte
	Size  uint32
	Blob  string // blob ID, 64 hex digits
	Title string
	Held  bool // the blob is held and its length is Size
}

// Feed describes a feed and what the node holds of it.
type Feed struct {
	Key           string
	Address       string // the key and the hints it was subscribed with
	Label         string // name and short key (NETWORK.md 4.6)
	Subscribed    bool
	Hints         []string // relay URLs of the address's hints
	HintRelays    []string // hinted relays the feed is followed on now
	ProfileRelays []string // relay lines of the profile in force
	Description   string   // of the profile in force
	Entries       []Entry  // increasing seq
}

// Relay is one connection.
type Relay struct {
	URL       string
	Own       bool
	Hint      bool
	Connected bool
}

// Node is everything the reader asks of the network side.
type Node interface {
	// Connect starts the relay connections (idempotent). Offline
	// commands never call it; Blob calls it when it must fetch.
	Connect() error
	// Sync connects, waits until every subscription has caught up and
	// the automatic downloads are done (or the timeout), and reports
	// whether everything arrived.
	Sync(timeout time.Duration) (bool, error)
	// Changed is signalled whenever the node holds something new.
	Changed() <-chan struct{}
	// Log delivers the node's messages for people.
	Log() <-chan string

	// Timeline: article and document entries of the subscribed feeds,
	// newest first in arrival order.
	Timeline() []Entry
	// Subscriptions lists the subscribed feeds (without entries).
	Subscriptions() []Feed
	// Feed describes a feed given by address (hints kept).
	Feed(address string) (Feed, error)
	// Label names a feed: nickname or display name with the short key.
	Label(key string) string

	Subscribe(address string) (isNew bool, err error)
	Unsubscribe(key string) error
	SetNickname(key, name string) error

	// HeldBlob returns a blob from the store only.
	HeldBlob(id string) ([]byte, bool)
	// Blob returns a blob from the store or the relays, verified.
	Blob(id string) ([]byte, error)
	// ArticleTitle is the title of an article entry held for a blob of
	// that size, or "".
	ArticleTitle(id string, size int) string

	Relays() []Relay
	ConfiguredRelays() []string
	SetRelays(urls []string) error

	Close()
}
