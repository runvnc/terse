package terse

import (
	"encoding/binary"
	"errors"
)

// Message types of the relay protocol (NETWORK.md 3, 4.3).
const (
	MsgGet              = 0x01
	MsgSubscribe        = 0x04
	MsgUnsubscribe      = 0x05
	MsgGetReply         = 0x81
	MsgSubscribeReply   = 0x84
	MsgUnsubscribeReply = 0x85
	MsgEntry            = 0x90
)

// Reply statuses.
const (
	StatusDone     = 1
	StatusNotFound = 2
	StatusRefused  = 3
)

// EncodeGet builds a Get message.
func EncodeGet(req uint32, id [32]byte) []byte {
	b := []byte{MsgGet}
	b = binary.LittleEndian.AppendUint32(b, req)
	return append(b, id[:]...)
}

// EncodeSubscribe builds a Subscribe message.
func EncodeSubscribe(req uint32, key [32]byte, after uint32) []byte {
	b := []byte{MsgSubscribe}
	b = binary.LittleEndian.AppendUint32(b, req)
	b = append(b, key[:]...)
	return binary.LittleEndian.AppendUint32(b, after)
}

// EncodeUnsubscribe builds an Unsubscribe message.
func EncodeUnsubscribe(req uint32, key [32]byte) []byte {
	b := []byte{MsgUnsubscribe}
	b = binary.LittleEndian.AppendUint32(b, req)
	return append(b, key[:]...)
}

// Reply is a decoded message from a relay. Type is the message type;
// Ignored is set for a type a reader does not use.
type Reply struct {
	Type    byte
	Req     uint32
	Status  byte
	Blob    []byte // get reply, status 1
	Highest uint32 // subscribe reply, status 1
	Entry   []byte // pushed entry (not yet verified)
	Ignored bool
}

var errShort = errors.New("message too short")

// DecodeReply decodes a message from a relay. A reader uses get,
// subscribe and unsubscribe replies and pushed entries; every other type
// is returned with Ignored set (NETWORK.md 6: ignore reply types it does
// not use).
func DecodeReply(m []byte) (*Reply, error) {
	if len(m) < 1 {
		return nil, errShort
	}
	r := &Reply{Type: m[0]}
	if m[0] == MsgEntry {
		r.Entry = m[1:]
		return r, nil
	}
	switch m[0] {
	case MsgGetReply, MsgSubscribeReply, MsgUnsubscribeReply:
	default:
		r.Ignored = true
		return r, nil
	}
	if len(m) < 6 {
		return nil, errShort
	}
	r.Req = binary.LittleEndian.Uint32(m[1:])
	r.Status = m[5]
	rest := m[6:]
	switch m[0] {
	case MsgGetReply:
		if r.Status == StatusDone {
			r.Blob = rest
		}
	case MsgSubscribeReply:
		if r.Status == StatusDone {
			if len(rest) < 4 {
				return nil, errShort
			}
			r.Highest = binary.LittleEndian.Uint32(rest)
		}
	}
	return r, nil
}

// EntrySeq reads the seq of an entry's bytes without verifying it (for
// catching up, which counts invalid entries too); ok is false if the
// bytes are too short to hold one.
func EntrySeq(b []byte) (key [32]byte, seq uint32, ok bool) {
	if len(b) < 36 {
		return key, 0, false
	}
	copy(key[:], b)
	return key, binary.LittleEndian.Uint32(b[32:]), true
}
