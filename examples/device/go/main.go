package main

import (
	"encoding/binary"
	"io"
	"net"
	"os"
)

var le = binary.LittleEndian

const (
	columns  = 80
	rows     = 30
	none     = 0xff
	deferred = 2
	buffered = 4
	reads    = 1
	inROM    = 4
)

type entry struct {
	fn, flags, buffer, length byte
	scale, most               uint32
	access, checked           byte
	checkedMost               uint32
	rule, ruleArg             byte
	perFour, chargeArg        byte
	perByte                   uint16
	low, high, add            uint32
}

var table = []entry{
	{fn: 0, flags: deferred | buffered, length: 1, scale: 1,
		most: 4096, access: reads | inROM, checked: none,
		rule: 2, ruleArg: 1, chargeArg: none, perByte: 2},
	{fn: 1, flags: deferred, length: none, checked: none,
		rule: 1, chargeArg: 0, low: 1, high: 3, add: 150},
	{fn: 2, flags: deferred, length: none, checked: none,
		rule: 1, chargeArg: none},
	{fn: 4, flags: deferred, length: none, checked: none,
		rule: 1, chargeArg: none},
	{fn: 7, flags: deferred | buffered, buffer: 1, length: 2,
		scale: 4, most: 9600, access: reads | inROM, checked: 0,
		checkedMost: 2399, rule: 2, ruleArg: 2, perFour: 1,
		chargeArg: none},
}

func words(b []byte, values ...uint32) []byte {
	for _, v := range values {
		b = le.AppendUint32(b, v)
	}
	return b
}

func (e entry) encode() []byte {
	b := []byte{e.fn, e.flags, e.buffer, e.length}
	b = words(b, e.scale, 0, e.most)
	b = append(b, e.access, e.checked, 0, 0)
	b = words(b, e.checkedMost, 0xfffffffe, 0xfffffffe)
	b = append(b, e.rule, e.ruleArg, 0, 0)
	b = words(b, 0, 0, 0)
	b = append(b, e.perFour, e.chargeArg)
	b = le.AppendUint16(b, e.perByte)
	return words(b, e.low, e.high, e.add)
}

var xterm = []byte{
	0, 0, 0, 205, 0, 0, 0, 205, 0, 205, 205, 0,
	0, 0, 238, 205, 0, 205, 0, 205, 205, 229, 229, 229,
	127, 127, 127, 255, 0, 0, 0, 255, 0, 255, 255, 0,
	92, 92, 255, 255, 0, 255, 0, 255, 255, 255, 255, 255,
}

type terminal struct {
	grid                          [columns * rows * 4]byte
	col, row, fg, bg, shown, wrap byte
	palette                       [768]byte
}

func newTerminal() *terminal {
	t := &terminal{fg: 7, shown: 1}
	for cell := range columns * rows {
		t.put(cell, ' ')
	}
	copy(t.palette[:], xterm)
	levels := []byte{0, 95, 135, 175, 215, 255}
	for i := range 216 {
		r, g, b := levels[i/36], levels[i/6%6], levels[i%6]
		copy(t.palette[48+3*i:], []byte{r, g, b})
	}
	for i := range 24 {
		gray := byte(8 + 10*i)
		copy(t.palette[696+3*i:], []byte{gray, gray, gray})
	}
	return t
}

func (t *terminal) put(cell int, code uint16) {
	le.PutUint16(t.grid[4*cell:], code)
	t.grid[4*cell+2], t.grid[4*cell+3] = t.fg, t.bg
}

func (t *terminal) newline() {
	t.col, t.wrap = 0, 0
	if t.row < rows-1 {
		t.row++
		return
	}
	copy(t.grid[:], t.grid[columns*4:])
	for col := range columns {
		t.put((rows-1)*columns+col, ' ')
	}
}

func (t *terminal) write(text []byte) {
	for _, r := range string(text) {
		if r == '\n' {
			t.newline()
		}
		if r < ' ' || r == 0x7f {
			continue
		}
		if t.wrap == 1 {
			t.newline()
		}
		code := uint16(r)
		if r >= 0xffff {
			code = 0xfffd
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
	a := func(i int) uint32 { return le.Uint32(body[4+4*i:]) }
	data := body[40 : 40+le.Uint32(body[36:])]
	switch body[1] {
	case 0:
		t.write(data)
		return a(1), 2 * a(1)
	case 1:
		if a(0) == 4 {
			for col := int(t.col); col < columns; col++ {
				t.put(int(t.row)*columns+col, ' ')
			}
		}
		if a(0) >= 1 && a(0) <= 3 {
			return 0, 150
		}
	case 2:
		t.col = byte(min(a(0), columns-1))
		t.row = byte(min(a(1), rows-1))
		t.wrap = 0
	case 4:
		if a(0) < 256 {
			t.fg = byte(a(0))
		}
		if a(1) < 256 {
			t.bg = byte(a(1))
		}
	case 7:
		for i, b := range data {
			t.grid[(4*int(a(0))+i)%len(t.grid)] = b
		}
		return a(2), uint32(len(data)+3) / 4
	}
	return 0, 0
}

func (t *terminal) state() []byte {
	state := append(t.grid[:], t.col, t.row, t.fg, t.bg)
	state = append(state, t.shown, t.wrap)
	state = append(state, t.palette[:]...)
	return append(state, 0, 0, 0, 0)
}

func hello(body []byte) []byte {
	name := "terminal (Go example)"
	reply := []byte{0x81, 1, 0, 0, byte(len(name)), 0}
	reply = append(append(reply, name...), 0, 0, 0, 0, body[8])
	for _, slot := range body[9 : 9+body[8]] {
		if slot != 2 {
			reply = append(reply, slot, 0, 0, 0)
			continue
		}
		reply = append(reply, 2, 0, 0, byte(len(table)))
		for _, e := range table {
			reply = append(reply, e.encode()...)
		}
	}
	return append(reply, 0, 0, 0)
}

func main() {
	conn, err := net.Dial("unix", os.Getenv("TERSE_BUS_SOCKET"))
	if err != nil {
		panic(err)
	}
	send := func(address, body []byte) {
		message := words(nil, uint32(8+len(body)))
		conn.Write(append(append(message, address...), body...))
	}
	machines := map[uint32]*terminal{}
	for {
		var length [4]byte
		if _, err := io.ReadFull(conn, length[:]); err != nil {
			return
		}
		message := make([]byte, le.Uint32(length[:]))
		if _, err := io.ReadFull(conn, message); err != nil {
			return
		}
		address, body := message[:8], message[8:]
		t := machines[le.Uint32(address)]
		switch body[0] {
		case 0x01:
			send(address, hello(body))
		case 0x03:
			machines[le.Uint32(address)] = newTerminal()
			send(address, []byte{0x83, 1, 0, 0, 0, 0})
		case 0x04:
			delete(machines, le.Uint32(address))
		case 0x10:
			result, charge := t.call(body)
			reply := []byte{0x90, 0, 0, 0}
			send(address, words(reply, result, 0, charge, 0))
		case 0x11:
			at := 8
			for range le.Uint32(body[4:]) {
				size := int(le.Uint32(body[at:]))
				call := body[at+4 : at+4+size]
				machines[le.Uint32(call)].call(call[8:])
				at += 4 + size
			}
		case 0x30:
			inputs := [20]byte{0xb0}
			send(address, inputs[:])
		case 0x31:
		case 0x40:
			state := t.state()
			size := uint32(len(state))
			reply := words([]byte{0xc0, 0, 0, 0}, size)
			send(address, append(reply, state...))
		case 0x60:
			return
		default:
			send(address, []byte("\x7fnot handled"))
			return
		}
	}
}
