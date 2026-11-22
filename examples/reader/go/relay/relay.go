package relay

import (
	"encoding/binary"
	"errors"

	"github.com/gorilla/websocket"
	"lukechampine.com/blake3"
)

type ID = [IDSize]byte

const (
	IDSize         = 32
	typeGet        = 0x01
	typeGetReply   = 0x81
	requestNumber  = 1
	statusAt       = 5
	blobAt         = 6
	statusFound    = 1
	statusNotFound = 2
)

func Get(conn *websocket.Conn, id ID) ([]byte, error) {
	request := []byte{typeGet}
	le := binary.LittleEndian
	request = le.AppendUint32(request, requestNumber)
	request = append(request, id[:]...)
	err := conn.WriteMessage(websocket.BinaryMessage, request)
	if err != nil {
		return nil, err
	}
	for {
		_, reply, err := conn.ReadMessage()
		switch {
		case err != nil:
			return nil, err
		case len(reply) < blobAt || reply[0] != typeGetReply:
			continue
		case reply[statusAt] == statusNotFound:
			return nil, errors.New("the relay does not have it")
		case reply[statusAt] != statusFound:
			return nil, errors.New("the relay refused it")
		case blake3.Sum256(reply[blobAt:]) != id:
			return nil, errors.New("hash check failed")
		}
		return reply[blobAt:], nil
	}
}
