package main

import (
	"encoding/binary"
	"io"
	"math"
	"net"
	"os"
	"slices"
	"unicode/utf8"
)

var le = binary.LittleEndian

const (
	columns      = 80
	rows         = 30
	cells        = columns * rows
	cellSize     = 4
	grays        = 24
	continuation = 0xffff
)

const (
	write      = 0
	command    = 1
	setCursor  = 2
	setColours = 4
	cellsWrite = 7
)

const (
	clearScreen = 1
	scrollDown  = 3
	clearToEnd  = 4
)

const (
	a0, a1, a2   = 0, 1, 2
	none         = 0xff
	deferred     = 2
	buffered     = 4
	reads        = 1
	inROM        = 4
	failed       = 0xfffffffe
	returnsValue = 1
	returnsArg   = 2
	perFour      = 1
)

type entry struct {
	function, flags, buffer, length byte
	scale, least, most              uint32
	access, checked                 byte
	_                               [2]byte
	checkedMost                     uint32
	lengthResult, placeResult       uint32
	rule, ruleArg                   byte
	_                               [2]byte
	value, value2, charge           uint32
	chargeFlags, chargeArg          byte
	perByte                         uint16
	low, high, add                  uint32
}

var table = []entry{
	{function: write, flags: deferred | buffered, buffer: a0,
		length: a1, scale: 1, most: 4096,
		access: reads | inROM, checked: none,
		lengthResult: failed, placeResult: failed,
		rule: returnsArg, ruleArg: a1, chargeArg: none,
		perByte: 2},
	{function: command, flags: deferred, length: none,
		checked: none, rule: returnsValue, chargeArg: a0,
		low: clearScreen, high: scrollDown, add: 150},
	{function: setCursor, flags: deferred, length: none,
		checked: none, rule: returnsValue, chargeArg: none},
	{function: setColours, flags: deferred, length: none,
		checked: none, rule: returnsValue, chargeArg: none},
	{function: cellsWrite, flags: deferred | buffered,
		buffer: a1, length: a2, scale: cellSize,
		most: cells * cellSize, access: reads | inROM,
		checked: a0, checkedMost: cells - 1,
		lengthResult: failed, placeResult: failed,
		rule: returnsArg, ruleArg: a2, chargeFlags: perFour,
		chargeArg: none},
}

var xterm = []byte{
	0, 0, 0, 205, 0, 0, 0, 205, 0, 205, 205, 0,
	0, 0, 238, 205, 0, 205, 0, 205, 205, 229, 229, 229,
	127, 127, 127, 255, 0, 0, 0, 255, 0, 255, 255, 0,
	92, 92, 255, 255, 0, 255, 0, 255, 255, 255, 255, 255,
}

type terminal struct {
	grid                          [cells * cellSize]byte
	col, row, fg, bg, shown, wrap byte
	palette                       []byte
	partial                       [4]byte
}

func newTerminal() *terminal {
	t := &terminal{fg: 7, shown: 1}
	t.palette = slices.Clone(xterm)
	for cell := range cells {
		t.put(cell, ' ')
	}
	levels := []byte{0, 95, 135, 175, 215, 255}
	for _, r := range levels {
		for _, g := range levels {
			for _, b := range levels {
				t.palette = append(t.palette, r, g, b)
			}
		}
	}
	for i := range byte(grays) {
		gray := 8 + 10*i
		t.palette = append(t.palette, gray, gray, gray)
	}
	return t
}

func (t *terminal) put(cell int, code uint16) {
	bytes := append(le.AppendUint16(nil, code), t.fg, t.bg)
	copy(t.grid[cellSize*cell:], bytes)
}

func (t *terminal) newline() {
	t.col, t.wrap = 0, 0
	if t.row < rows-1 {
		t.row++
		return
	}
	copy(t.grid[:], t.grid[columns*cellSize:])
	for col := range columns {
		t.put((rows-1)*columns+col, ' ')
	}
}

func (t *terminal) write(text []byte) {
	for _, r := range string(text) {
		if r == '\n' {
			t.newline()
		}
		if r < ' ' || r == '\x7f' {
			continue
		}
		if t.wrap == 1 {
			t.newline()
		}
		code := uint16(r)
		if r >= continuation {
			code = utf8.RuneError
		}
		t.put(int(t.row)*columns+int(t.col), code)
		if t.col == columns-1 {
			t.wrap = 1
		} else {
			t.col++
		}
	}
}

func (t *terminal) call(body []byte) (result, charge uint32) {
	args := body[argsAt:]
	a := func(i int) uint32 { return le.Uint32(args[4*i:]) }
	data := body[dataAt:][:le.Uint32(body[sizeAt:])]
	switch body[functionAt] {
	case write:
		t.write(data)
		return a(1), 2 * a(1)
	case command:
		if a(0) == clearToEnd {
			for col := int(t.col); col < columns; col++ {
				t.put(int(t.row)*columns+col, ' ')
			}
		}
		if a(0) >= clearScreen && a(0) <= scrollDown {
			return 0, 150
		}
	case setCursor:
		t.col = byte(min(a(0), columns-1))
		t.row = byte(min(a(1), rows-1))
		t.wrap = 0
	case setColours:
		if a(0) <= math.MaxUint8 {
			t.fg = byte(a(0))
		}
		if a(1) <= math.MaxUint8 {
			t.bg = byte(a(1))
		}
	case cellsWrite:
		for i, b := range data {
			at := cellSize*int(a(0)) + i
			t.grid[at%len(t.grid)] = b
		}
		return a(2), uint32(len(data)+3) / 4
	}
	return 0, 0
}

func (t *terminal) state() []byte {
	state := append(t.grid[:], t.col, t.row, t.fg, t.bg)
	state = append(state, t.shown, t.wrap)
	state = append(state, t.palette...)
	return append(state, t.partial[:]...)
}

const (
	HELLO    = 0x01
	OPEN     = 0x03
	CLOSE    = 0x04
	CALL     = 0x10
	BATCH    = 0x11
	BEGIN    = 0x30
	END      = 0x31
	STATE    = 0x40
	SHUTDOWN = 0x60
	ERROR    = 0x7f
	REPLY    = 0x80
)

const (
	addressSize   = 8
	protocol      = 1
	onSocket      = 0
	took          = 1
	terminalSlot  = 2
	deviceVersion = 0
	slotsAt       = 8
	functionAt    = 1
	argsAt        = 4
	sizeAt        = 36
	dataAt        = 40
	batchCountAt  = 4
	batchCallsAt  = 8
)

func words(b []byte, values ...uint32) []byte {
	for _, v := range values {
		b = le.AppendUint32(b, v)
	}
	return b
}

func hello(body []byte) []byte {
	name := "terminal (Go example)"
	reply := []byte{HELLO | REPLY, protocol, onSocket, 0}
	reply = le.AppendUint16(reply, uint16(len(name)))
	reply = append(reply, name...)
	slots := body[slotsAt+1:][:body[slotsAt]]
	reply = append(reply, 0, 0, 0, 0, byte(len(slots)))
	for _, slot := range slots {
		var entries []entry
		if slot == terminalSlot {
			entries = table
		}
		count := byte(len(entries))
		reply = append(reply, slot, deviceVersion, 0, count)
		reply, _ = binary.Append(reply, le, entries)
	}
	return append(reply, 0, 0, 0)
}

func main() {
	conn, err := net.Dial("unix", os.Getenv("TERSE_BUS_SOCKET"))
	if err != nil {
		panic(err)
	}
	send := func(address, body []byte) {
		message := words(nil, uint32(addressSize+len(body)))
		conn.Write(append(append(message, address...), body...))
	}
	machines := map[uint32]*terminal{}
	for {
		var size [4]byte
		if _, err := io.ReadFull(conn, size[:]); err != nil {
			return
		}
		message := make([]byte, le.Uint32(size[:]))
		if _, err := io.ReadFull(conn, message); err != nil {
			return
		}
		address := message[:addressSize]
		body := message[addressSize:]
		instance := le.Uint32(address)
		t := machines[instance]
		switch body[0] {
		case HELLO:
			send(address, hello(body))
		case OPEN:
			machines[instance] = newTerminal()
			reply := []byte{OPEN | REPLY, took, 0, 0, 0, 0}
			send(address, reply)
		case CLOSE:
			delete(machines, instance)
		case CALL:
			result, charge := t.call(body)
			reply := []byte{CALL | REPLY, 0, 0, 0}
			send(address, words(reply, result, 0, charge, 0))
		case BATCH:
			calls := body[batchCallsAt:]
			for range le.Uint32(body[batchCountAt:]) {
				size := le.Uint32(calls)
				call := calls[4:][:size]
				instance := le.Uint32(call)
				machines[instance].call(call[addressSize:])
				calls = calls[4+size:]
			}
		case BEGIN:
			reply := []byte{BEGIN | REPLY, 0, 0, 0}
			send(address, words(reply, 0, 0, 0, 0))
		case END:
		case STATE:
			state := t.state()
			reply := []byte{STATE | REPLY, 0, 0, 0}
			reply = words(reply, uint32(len(state)))
			send(address, append(reply, state...))
		case SHUTDOWN:
			return
		default:
			reply := append([]byte{ERROR}, "not handled"...)
			send(address, reply)
			return
		}
	}
}
