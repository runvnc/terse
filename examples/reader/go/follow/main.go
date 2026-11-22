// Step 3: follow a feed and open one of its articles.
package main

import (
	"bytes"
	"crypto/ed25519"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"log"
	"os"
	"strconv"
	"unicode/utf8"

	"filippo.io/edwards25519"
	"github.com/gorilla/websocket"

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
		if valid(entry, key) && entry[40] == 1 {
			title := string(entry[109 : len(entry)-64])
			fmt.Printf("%d  %s\n", seq, article.Visible(title))
			articles[seq] = [32]byte(entry[44:76])
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

func valid(e, key []byte) bool {
	if len(e) < 173 || len(e) != 173+int(e[108]) {
		return false
	}
	signed, signature := e[:len(e)-64], e[len(e)-64:]
	message := append([]byte("terse-entry-0"), signed...)
	seq := binary.LittleEndian.Uint32(e[32:])
	size := int(e[41]) | int(e[42])<<8 | int(e[43])<<16
	return bytes.Equal(e[:32], key) &&
		e[108] <= 120 &&
		seq >= 1 &&
		size >= 1 && size <= 1<<20 &&
		utf8.Valid(signed[109:]) &&
		verify(key, message, signature)
}

func verify(key, message, signature []byte) bool {
	a, err := new(edwards25519.Point).SetBytes(key)
	canonical := err == nil && bytes.Equal(a.Bytes(), key)
	if !canonical || smallOrder(a) {
		return false
	}
	r, err := new(edwards25519.Point).SetBytes(signature[:32])
	if err != nil || smallOrder(r) {
		return false
	}
	return ed25519.Verify(key, message, signature)
}

func smallOrder(p *edwards25519.Point) bool {
	p8 := new(edwards25519.Point).MultByCofactor(p)
	return p8.Equal(edwards25519.NewIdentityPoint()) == 1
}
