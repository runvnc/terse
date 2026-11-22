// Step 3: follow a feed and open one of its articles.
package main

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"log"
	"os"
	"strconv"

	"github.com/gorilla/websocket"
	tersenet "github.com/runvnc/terse/sdk/go"

	"reader/article"
	"reader/relay"
)

func main() {
	key, err := hex.DecodeString(os.Args[2])
	if err != nil || len(key) != 32 {
		log.Fatal("a feed key is 64 hexadecimal digits")
	}
	url := os.Args[1]
	conn, _, err := websocket.DefaultDialer.Dial(url, nil)
	if err != nil {
		log.Fatal(err)
	}
	subscribe := append([]byte{0x04, 2, 0, 0, 0}, key...)
	subscribe = append(subscribe, 0, 0, 0, 0)
	err = conn.WriteMessage(websocket.BinaryMessage, subscribe)
	if err != nil {
		log.Fatal(err)
	}
	_, reply, err := conn.ReadMessage()
	if err != nil || len(reply) < 10 || reply[5] != 1 {
		log.Fatal("the relay refused the subscription")
	}
	highest := binary.LittleEndian.Uint32(reply[6:])
	articles := map[uint32][32]byte{}
	for caughtUp := highest == 0; !caughtUp; {
		_, message, err := conn.ReadMessage()
		if err != nil {
			log.Fatal(err)
		}
		if len(message) < 37 || message[0] != 0x90 {
			continue
		}
		entry := message[1:]
		seq := binary.LittleEndian.Uint32(entry[32:])
		caughtUp = seq >= highest
		e, err := tersenet.VerifyEntry(entry)
		mine := err == nil && bytes.Equal(e.Feed[:], key)
		if mine && e.Kind == 1 {
			title := article.Visible(e.Title)
			fmt.Printf("%d  %s\n", seq, title)
			articles[seq] = e.Blob
		}
	}
	if len(os.Args) < 4 {
		return
	}
	seq, _ := strconv.Atoi(os.Args[3])
	id, ok := articles[uint32(seq)]
	if !ok {
		log.Fatal("no such article in the feed")
	}
	blob, err := relay.Get(conn, id)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println()
	article.Show(blob)
}
