//go:build !thin

package main

// Where the reader binds to the network node. Everything else in package
// main sees only nodeapi.Node. A running node is reached over its local
// socket (LOCAL.md, package localclient); otherwise this full build runs
// the node in this process. A thin build (-tags thin, wire_thin.go) has
// only the socket.

import (
	"errors"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"tersereader/internal/client"
	"tersereader/internal/local"
	"tersereader/internal/localclient"
	"tersereader/internal/localnode"
	"tersereader/internal/node"
	"tersereader/internal/nodeapi"
)

func openNode(cfg *config) (nodeapi.Node, error) {
	if cfg.nodeMode != "in-process" {
		c, err := localclient.Dial(cfg.socket)
		if err == nil {
			if len(cfg.relays) > 0 {
				fmt.Fprintln(os.Stderr, "tersereader: using the running node; --relay is ignored")
			}
			return c, nil
		}
		if cfg.nodeMode == "socket" {
			return nil, fmt.Errorf("no node at %s: %v (start one with `tersereader node`)", cfg.socket, err)
		}
	}
	return openInProcess(cfg)
}

func openInProcess(cfg *config) (*node.Node, error) {
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

// runNode runs only the node: it opens the store, connects to the relays
// and serves the local socket until interrupted (LOCAL.md 5).
func runNode(cfg *config) error {
	n, err := openInProcess(cfg)
	if err != nil {
		return err
	}
	defer n.Close()
	ln, err := local.Listen(cfg.socket)
	if errors.Is(err, local.ErrRunning) {
		return fmt.Errorf("%s: %w", cfg.socket, err)
	}
	if err != nil {
		return err
	}
	if err := n.Connect(); err != nil {
		fmt.Fprintln(os.Stderr, "tersereader node:", err)
	}
	srv := localnode.Serve(n, ln)
	fmt.Fprintln(os.Stderr, "tersereader node: serving", cfg.socket)
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, os.Interrupt, syscall.SIGTERM)
	<-sig
	srv.Close()
	return nil
}
