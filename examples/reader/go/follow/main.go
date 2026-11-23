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

const (
	keySize       = 32
	typeSubscribe = 0x04
	typeEntry     = 0x90
	requestNumber = 2
	after         = 0
	statusAt      = 5
	statusDone    = 1
	highestAt     = 6
	highestEnd    = 10
	seqAt         = 32
	seqEnd        = 36
	kindArticle   = 1
)

var le = binary.LittleEndian

func main() {
	key, err := hex.DecodeString(os.Args[2])
	if err != nil || len(key) != keySize {
		log.Fatal("a feed key is 64 hexadecimal digits")
	}
	url := os.Args[1]
	conn, _, err := websocket.DefaultDialer.Dial(url, nil)
	if err != nil {
		log.Fatal(err)
	}
	request := []byte{typeSubscribe}
	request = le.AppendUint32(request, requestNumber)
	request = append(request, key...)
	request = le.AppendUint32(request, after)
	err = conn.WriteMessage(websocket.BinaryMessage, request)
	if err != nil {
		log.Fatal(err)
	}
	_, reply, err := conn.ReadMessage()
	if err != nil || len(reply) < highestEnd ||
		reply[statusAt] != statusDone {
		log.Fatal("the relay refused the subscription")
	}
	highest := le.Uint32(reply[highestAt:highestEnd])
	articles := map[uint32]relay.ID{}
	for caughtUp := highest == after; !caughtUp; {
		_, msg, err := conn.ReadMessage()
		if err != nil {
			log.Fatal(err)
		}
		entry, ok := bytes.CutPrefix(msg, []byte{typeEntry})
		if !ok || len(entry) < seqEnd {
			continue
		}
		seq := le.Uint32(entry[seqAt:seqEnd])
		caughtUp = seq >= highest
		e, err := tersenet.VerifyEntry(entry)
		mine := err == nil && bytes.Equal(e.Feed[:], key)
		if mine && e.Kind == kindArticle {
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
