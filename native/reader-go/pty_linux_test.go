//go:build linux

package main

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"
	"unsafe"

	"tersereader/internal/blake3"
	"tersereader/internal/vecfile"
)

// TestInteractive drives the interactive reader in a pseudo-terminal (no
// window): the timeline shows the vector feed's article, Enter opens it,
// q goes back and quits.
func TestInteractive(t *testing.T) {
	if testing.Short() {
		t.Skip("short")
	}
	tmp := t.TempDir()
	bin := filepath.Join(tmp, "tersereader")
	if out, err := exec.Command("go", "build", "-o", bin, ".").CombinedOutput(); err != nil {
		t.Fatalf("build: %v\n%s", err, out)
	}
	// A store holding the vector entry `article` and its blob.
	dir, _ := vecfile.Dir()
	recs, _ := vecfile.Parse(filepath.Join(dir, "entries.txt"))
	var entry []byte
	for _, r := range recs {
		if r.Name == "article" {
			h, _ := r.Get("bytes")
			entry, _ = vecfile.Hex(h)
		}
	}
	store := filepath.Join(tmp, "store")
	os.MkdirAll(filepath.Join(store, "blobs"), 0o755)
	rec := binary.LittleEndian.AppendUint16(nil, uint16(len(entry)))
	os.WriteFile(filepath.Join(store, "entries"), append(rec, entry...), 0o644)
	blob := []byte("Hello, Terse!")
	os.WriteFile(filepath.Join(store, "blobs", blake3.Hex(blob)), blob, 0o644)
	os.WriteFile(filepath.Join(store, "subscriptions"), []byte(fmt.Sprintf("%x\n", entry[:32])), 0o644)
	// A relay nobody listens on: the reader keeps trying in the background.
	os.WriteFile(filepath.Join(store, "relays"), []byte("ws://127.0.0.1:9\n"), 0o644)

	master, err := os.OpenFile("/dev/ptmx", os.O_RDWR, 0)
	if err != nil {
		t.Skip("no pseudo-terminals:", err)
	}
	defer master.Close()
	var unlock int32
	var n uint32
	ioctl(int(master.Fd()), syscall.TIOCSPTLCK, unsafe.Pointer(&unlock))
	if err := ioctl(int(master.Fd()), syscall.TIOCGPTN, unsafe.Pointer(&n)); err != nil {
		t.Skip(err)
	}
	slave, err := os.OpenFile(fmt.Sprintf("/dev/pts/%d", n), os.O_RDWR|syscall.O_NOCTTY, 0)
	if err != nil {
		t.Skip(err)
	}
	ws := struct{ Row, Col, X, Y uint16 }{24, 80, 0, 0}
	ioctl(int(slave.Fd()), syscall.TIOCSWINSZ, unsafe.Pointer(&ws))
	cmd := exec.Command(bin, "--dir", store)
	cmd.Env = []string{"HOME=" + tmp, "TERM=xterm", "TERSE_NODE_SOCKET=" + filepath.Join(tmp, "no-node", "node.sock")}
	cmd.Stdin, cmd.Stdout, cmd.Stderr = slave, slave, slave
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true, Setctty: true}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	slave.Close()
	var mu sync.Mutex
	var screen bytes.Buffer
	go func() {
		buf := make([]byte, 4096)
		for {
			k, err := master.Read(buf)
			if err != nil {
				return
			}
			mu.Lock()
			screen.Write(buf[:k])
			mu.Unlock()
		}
	}()
	waitFor := func(s string) {
		deadline := time.Now().Add(5 * time.Second)
		for time.Now().Before(deadline) {
			mu.Lock()
			ok := strings.Contains(screen.String(), s)
			mu.Unlock()
			if ok {
				return
			}
			time.Sleep(50 * time.Millisecond)
		}
		mu.Lock()
		defer mu.Unlock()
		cmd.Process.Kill()
		t.Fatalf("%q never shown; screen:\n%q", s, screen.String())
	}
	waitFor("Terse — timeline")
	waitFor("Hello")
	mu.Lock()
	screen.Reset()
	mu.Unlock()
	master.Write([]byte("\r"))
	waitFor("Hello, Terse!")
	master.Write([]byte("q"))
	time.Sleep(200 * time.Millisecond)
	master.Write([]byte("q"))
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	select {
	case err := <-done:
		if err != nil {
			t.Errorf("exit: %v", err)
		}
	case <-time.After(5 * time.Second):
		cmd.Process.Kill()
		t.Fatal("the reader did not quit")
	}
}
