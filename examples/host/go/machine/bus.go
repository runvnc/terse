package machine

import (
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"

	"lukechampine.com/blake3"
)

const (
	Terminal = 2
	FullText = 10
)

const (
	HELLO       = 0x01
	OPEN        = 0x03
	CALL        = 0x10
	BEGIN       = 0x30
	END         = 0x31
	STATE       = 0x40
	SHUTDOWN    = 0x60
	REPLY       = 0x80
	protocol    = 1
	addressSize = 8
	noSlot      = 0
)

const (
	nameAt     = 4
	lengthSize = 2
	nativeSize = 4
	layerSize  = 2
	tookAt     = 1
	took       = 1
)

const (
	entrySize      = 64
	functionAt     = 0
	flagsAt        = 1
	bufferAt       = 2
	lengthAt       = 3
	scaleAt        = 4
	leastAt        = 8
	mostAt         = 12
	accessAt       = 16
	checkedAt      = 17
	checkedMostAt  = 20
	lengthResultAt = 24
	placeResultAt  = 28
)

const (
	deferred       = 2
	buffered       = 4
	reads          = 1
	writes         = 2
	inROM          = 4
	none           = 0xff
	answeredCharge = 64
)

const (
	setsA1        = 1
	a0At          = 4
	a1At          = 8
	chargeAt      = 12
	writeCountAt  = 16
	writesAt      = 20
	writeLengthAt = 4
	writeDataAt   = 8
)

const (
	inputsChargeAt = 4
	sectionAt      = 8
)

type Bus struct {
	m      *Machine
	conn   net.Conn
	driver *exec.Cmd
	dir    string
	tables map[uint32]map[uint32][]byte
	Grants []byte
}

func Start(command string, m *Machine, file []byte,
	devices []Device) *Bus {
	dir, err := os.MkdirTemp("", "terse-bus")
	check(err)
	path := filepath.Join(dir, "bus.sock")
	listener, err := net.Listen("unix", path)
	check(err)
	driver := exec.Command(command)
	driver.Env = append(os.Environ(), "TERSE_BUS_SOCKET="+path)
	driver.Stdout, driver.Stderr = os.Stderr, os.Stderr
	check(driver.Start())
	conn, err := listener.Accept()
	check(err)
	listener.Close()
	b := &Bus{m: m, conn: conn, driver: driver, dir: dir,
		tables: map[uint32]map[uint32][]byte{}}

	var ask []byte
	for _, d := range devices {
		if d.Slot == Terminal || d.Slot == FullText {
			ask = append(ask, d.Slot)
		}
	}
	hello := []byte{HELLO, protocol, 0, 0, 0, 0, 0, 0}
	hello = append(append(hello, byte(len(ask))), ask...)
	hello = le.AppendUint16(hello, uint16(len(dir)))
	hello = append(append(hello, dir...), 0, 0)
	rest := b.request(noSlot, hello)[nameAt:]
	take := func(n int) []byte {
		taken := rest[:n]
		rest = rest[n:]
		return taken
	}
	length := le.Uint16(take(lengthSize))
	take(int(length) + nativeSize)
	versions := map[byte]byte{}
	for range take(1)[0] {
		slot, version := take(1)[0], take(1)[0]
		layers := int(take(1)[0])
		take(layerSize * layers)
		table := map[uint32][]byte{}
		for range take(1)[0] {
			entry := take(entrySize)
			table[uint32(entry[functionAt])] = entry
		}
		b.tables[uint32(slot)], versions[slot] = table, version
	}

	var has []byte
	for _, d := range devices {
		version, served := versions[d.Slot]
		granted := served && version == d.Version
		if !granted && !d.Optional {
			check(fmt.Errorf("no driver for device %d", d.Slot))
		}
		if d.Optional {
			b.Grants = append(b.Grants, byte(flag(granted)))
		}
		if granted {
			has = append(has, d.Slot, d.Version)
		}
	}
	id := blake3.Sum256(file)
	open := append([]byte{OPEN, 0, 0, 0}, id[:]...)
	open = words(open, uint32(len(m.ROM)), uint32(len(m.RAM)))
	open = append(open, 0, 0, byte(len(has)/deviceSize))
	open = append(append(open, has...), 0)
	reply := b.request(noSlot, append(open, m.ROM...))
	if reply[tookAt] != took {
		check(fmt.Errorf("the driver will not run the program"))
	}
	return b
}

func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, "bus:", err)
		os.Exit(3)
	}
}

func words(b []byte, values ...uint32) []byte {
	for _, v := range values {
		b = le.AppendUint32(b, v)
	}
	return b
}

func (b *Bus) send(slot byte, body []byte) {
	message := words(nil, uint32(addressSize+len(body)))
	message = append(message, 0, 0, 0, 0, slot, 0, 0, 0)
	_, err := b.conn.Write(append(message, body...))
	check(err)
}

func (b *Bus) request(slot byte, body []byte) []byte {
	b.send(slot, body)
	var length uint32
	check(binary.Read(b.conn, le, &length))
	reply := make([]byte, length)
	_, err := io.ReadFull(b.conn, reply)
	check(err)
	reply = reply[addressSize:]
	if len(reply) == 0 || reply[0] != body[0]|REPLY {
		check(fmt.Errorf("the driver said %q", reply))
	}
	return reply
}

func (b *Bus) Call(slot, fn uint32) (uint32, int64) {
	m := b.m
	entry := b.tables[slot][fn]
	if entry == nil {
		return UnknownCall, BaseCharge
	}
	field := func(at int) uint32 {
		return le.Uint32(entry[at:])
	}
	flags, access := entry[flagsAt], entry[accessAt]
	charge := int64(answeredCharge)
	if flags&deferred != 0 {
		charge = BaseCharge
	}
	args := m.X[A0 : A5+1]
	addr := args[entry[bufferAt]]
	var buffer []byte
	if flags&buffered != 0 {
		size := uint64(field(scaleAt))
		if entry[lengthAt] != none {
			size *= uint64(args[entry[lengthAt]])
		}
		if size < uint64(field(leastAt)) ||
			size > uint64(field(mostAt)) {
			return field(lengthResultAt), charge
		}
		ramOnly := access&inROM == 0 || access&writes != 0
		buffer = m.Memory(addr, uint32(size), ramOnly)
		if buffer == nil && size > 0 {
			return field(placeResultAt), charge
		}
	}
	checked := entry[checkedAt]
	if checked != none && args[checked] > field(checkedMostAt) {
		return field(placeResultAt), charge
	}

	call := words([]byte{CALL, byte(fn), 0, 0}, args...)
	call = le.AppendUint64(call, uint64(m.Budget-BaseCharge))
	if access&reads == 0 {
		call = words(call, 0)
	} else {
		call = words(call, uint32(len(buffer)))
		call = append(call, buffer...)
	}
	reply := b.request(byte(slot), call)
	if reply[flagsAt]&setsA1 != 0 {
		m.X[A1] = le.Uint32(reply[a1At:])
	}
	at := writesAt
	for range le.Uint32(reply[writeCountAt:]) {
		write := reply[at:]
		offset := uint64(le.Uint32(write) - addr)
		size := le.Uint32(write[writeLengthAt:])
		end := offset + uint64(size)
		if access&writes == 0 || end > uint64(len(buffer)) {
			check(fmt.Errorf("a write outside the buffer"))
		}
		copy(buffer[offset:], write[writeDataAt:][:size])
		at += writeDataAt + int(size)
	}
	extra := int64(le.Uint32(reply[chargeAt:]))
	return le.Uint32(reply[a0At:]), charge + extra
}

func (b *Bus) Begin(frame uint32) int64 {
	begin := words([]byte{BEGIN, 0, 0, 0}, frame, 0, 0, 0)
	reply := b.request(noSlot, begin)
	return int64(le.Uint32(reply[inputsChargeAt:]))
}

func (b *Bus) End() {
	stopped := byte(flag(b.m.Stopped != Running))
	end := words([]byte{END, stopped, 0, 0}, b.m.Frame)
	b.send(noSlot, end)
}

func (b *Bus) State(slot byte) []byte {
	return b.request(slot, []byte{STATE})[sectionAt:]
}

func (b *Bus) Close() {
	b.send(noSlot, []byte{SHUTDOWN})
	b.conn.Close()
	b.driver.Wait()
	os.RemoveAll(b.dir)
}
