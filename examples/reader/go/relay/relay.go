package relay

import (
	"errors"

	"github.com/gorilla/websocket"
	"lukechampine.com/blake3"
)

func Get(conn *websocket.Conn, id [32]byte) ([]byte, error) {
	request := append([]byte{0x01, 1, 0, 0, 0}, id[:]...)
	err := conn.WriteMessage(websocket.BinaryMessage, request)
	if err != nil {
		return nil, err
	}
	for {
		_, reply, err := conn.ReadMessage()
		switch {
		case err != nil:
			return nil, err
		case len(reply) < 6 || reply[0] != 0x81:
			continue
		case reply[5] != 1:
			return nil, errors.New("the relay does not have it")
		case blake3.Sum256(reply[6:]) != id:
			return nil, errors.New("hash check failed")
		}
		return reply[6:], nil
	}
}
