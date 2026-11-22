package ws

import (
	"crypto/tls"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"tersereader/internal/testenv"
)

func certs() string { return filepath.Join(testenv.Root(), "crates", "terse-testkit", "tls") }

// echo is a WebSocket server answering each message with itself.
func echo(w http.ResponseWriter, r *http.Request) {
	c, err := Accept(w, r)
	if err != nil {
		return
	}
	defer c.Close()
	for {
		m, err := c.ReadMessage()
		if err != nil {
			return
		}
		c.WriteMessage(m)
	}
}

// server serves echo over TLS with a test certificate ("localhost",
// "wrong", "untrusted"), or plain with "".
func server(t *testing.T, cert string) string {
	t.Helper()
	s := httptest.NewUnstartedServer(http.HandlerFunc(echo))
	if cert == "" {
		s.Start()
		t.Cleanup(s.Close)
		return "ws://" + s.Listener.Addr().String()
	}
	pair, err := tls.LoadX509KeyPair(filepath.Join(certs(), cert+".pem"), filepath.Join(certs(), cert+".key"))
	if err != nil {
		t.Fatal(err)
	}
	s.TLS = &tls.Config{Certificates: []tls.Certificate{pair}}
	s.StartTLS()
	t.Cleanup(s.Close)
	return "wss://" + s.Listener.Addr().String()
}

func TestTLS(t *testing.T) {
	t.Setenv(CAEnv, filepath.Join(certs(), "ca.pem"))
	secure := server(t, "localhost")
	c, err := Dial(secure, "", 5*time.Second)
	if err != nil {
		t.Fatalf("%s: %v", secure, err)
	}
	if err := c.WriteMessage([]byte("over TLS")); err != nil {
		t.Fatal(err)
	}
	if m, err := c.ReadMessage(); err != nil || string(m) != "over TLS" {
		t.Fatalf("echo: %q %v", m, err)
	}
	c.Close()

	plain := server(t, "")
	refused := []struct{ url, want string }{
		{server(t, "wrong"), "certificate is for another name"},
		{server(t, "untrusted"), "certificate is not trusted"},
		{strings.Replace(plain, "ws://", "wss://", 1), "does not speak TLS (try ws://)"},
		{strings.Replace(secure, "wss://", "ws://", 1), "requires TLS (try wss://)"},
	}
	for _, r := range refused {
		_, err := Dial(r.url, "", 5*time.Second)
		if err == nil || !strings.Contains(err.Error(), r.want) {
			t.Errorf("%s: want %q, got %v", r.url, r.want, err)
		}
	}
	// Without the CA, the test certificate is not trusted.
	t.Setenv(CAEnv, "")
	if _, err := Dial(secure, "", 5*time.Second); err == nil || !strings.Contains(err.Error(), "not trusted") {
		t.Errorf("without the CA: %v", err)
	}
}
