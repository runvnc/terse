//go:build thin

package main

// The thin reader (go build -tags thin): only a client of a running node
// (LOCAL.md). It links no cryptography, no hashing, no store and no
// WebSocket; TestThinReader checks that.

import (
	"errors"
	"fmt"

	"tersereader/internal/localclient"
	"tersereader/internal/nodeapi"
)

func openNode(cfg *config) (nodeapi.Node, error) {
	if cfg.nodeMode == "in-process" {
		return nil, errors.New("this is a thin reader: it has no node of its own")
	}
	c, err := localclient.Dial(cfg.socket)
	if err != nil {
		return nil, fmt.Errorf("no node at %s: %v (start one with `tersereader node`)", cfg.socket, err)
	}
	return c, nil
}

func runNode(cfg *config) error {
	return errors.New("this is a thin reader: it cannot run a node (build without -tags thin)")
}
