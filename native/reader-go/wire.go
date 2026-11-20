package main

// The one place the reader binds to the network node's implementation.
// Everything else in package main sees only nodeapi.Node; replacing this
// file with a socket client would split tersereader into two programs.

import (
	"fmt"
	"os"

	"tersereader/internal/client"
	"tersereader/internal/node"
	"tersereader/internal/nodeapi"
)

func openNode(cfg *config) (nodeapi.Node, error) {
	opt := client.DefaultOptions()
	opt.Relays = cfg.relays
	opt.AutoLimit = cfg.autoLimit
	opt.Prefetch = cfg.prefetch
	opt.FeedRelays = cfg.feedRelays
	opt.AllowPrivate = os.Getenv("TERSEREADER_ALLOW_PRIVATE") == "1" // tests: hints on loopback
	if os.Getenv("TERSEREADER_VERBOSE") == "1" {
		opt.Logf = func(f string, a ...any) { fmt.Fprintf(os.Stderr, f+"\n", a...) }
	}
	return node.Open(cfg.dir, opt)
}
