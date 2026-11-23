// Step 2: fetch an article from a relay by its ID.
package main

import (
	"bytes"
	"encoding/hex"
	"log"
	"os"

	"github.com/gorilla/websocket"

	"reader/article"
	"reader/relay"
)

func main() {
	id, err := hex.DecodeString(os.Args[2])
	if err != nil || len(id) != relay.IDSize {
		log.Fatal("an ID is 64 hexadecimal digits")
	}
	url := os.Args[1]
	conn, _, err := websocket.DefaultDialer.Dial(url, nil)
	if err != nil {
		log.Fatal(err)
	}
	blob, err := relay.Get(conn, relay.ID(id))
	if err != nil {
		log.Fatal(err)
	}
	if bytes.HasPrefix(blob, []byte("TERS")) ||
		bytes.HasPrefix(blob, []byte("TDOC")) {
		log.Fatal("not an article")
	}
	article.Show(blob)
}
