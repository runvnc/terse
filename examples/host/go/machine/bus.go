package machine

import (
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"

	"lukechampine.com/blake3"
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
		if d.Slot == 2 || d.Slot == 10 {
			ask = append(ask, d.Slot)
		}
	}
	hello := []byte{0x01, 1, 0, 0, 0, 0, 0, 0, byte(len(ask))}
	hello = append(hello, ask...)
	hello = le.AppendUint16(hello, uint16(len(dir)))
	hello = append(append(hello, dir...), 0, 0)
	reply := b.request(0, 0x81, hello)
	at := 10 + int(le.Uint16(reply[4:]))
	versions := map[byte]byte{}
	for range reply[at] {
		slot, version := reply[at+1], reply[at+2]
		at += 4 + 2*int(reply[at+3])
		table := map[uint32][]byte{}
		for range reply[at] {
			table[uint32(reply[at+1])] = reply[at+1 : at+65]
			at += 64
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
	open := append([]byte{0x03, 0, 0, 0}, id[:]...)
	open = le.AppendUint32(open, uint32(len(m.ROM)))
	open = le.AppendUint32(open, uint32(len(m.RAM)))
	open = append(open, 0, 0, byte(len(has)/2))
	open = append(append(open, has...), 0)
	reply = b.request(0, 0x83, append(open, m.ROM...))
	if reply[1] != 1 {
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

func (b *Bus) send(slot byte, body []byte) {
	message := le.AppendUint32(nil, uint32(8+len(body)))
	message = append(message, 0, 0, 0, 0, slot, 0, 0, 0)
	_, err := b.conn.Write(append(message, body...))
	check(err)
}

func (b *Bus) request(slot, want byte, body []byte) []byte {
	b.send(slot, body)
	var length [4]byte
	_, err := io.ReadFull(b.conn, length[:])
	check(err)
	reply := make([]byte, le.Uint32(length[:]))
	_, err = io.ReadFull(b.conn, reply)
	check(err)
	if len(reply) < 9 || reply[8] != want {
		check(fmt.Errorf("the driver said %q", reply[8:]))
	}
	return reply[8:]
}

func (b *Bus) Begin(frame uint32) int64 {
	begin := le.AppendUint32([]byte{0x30, 0, 0, 0}, frame)
	begin = append(begin, make([]byte, 12)...)
	return int64(le.Uint32(b.request(0, 0xb0, begin)[4:]))
}

func (b *Bus) Call(slot, fn uint32) (uint32, int64) {
	m := b.m
	entry := b.tables[slot][fn]
	if entry == nil {
		return 0xffffffff, 4
	}
	charge := int64(64)
	if entry[1]&2 != 0 {
		charge = 4
	}
	args, access := m.X[10:16], entry[16]
	addr := args[entry[2]]
	var buffer []byte
	if entry[1]&4 != 0 {
		size := uint64(le.Uint32(entry[4:]))
		if entry[3] != 0xff {
			size *= uint64(args[entry[3]])
		}
		if size < uint64(le.Uint32(entry[8:])) ||
			size > uint64(le.Uint32(entry[12:])) {
			return le.Uint32(entry[24:]), charge
		}
		ramOnly := access&4 == 0 || access&2 != 0
		buffer = m.Memory(addr, uint32(size), ramOnly)
		if buffer == nil && size > 0 {
			return le.Uint32(entry[28:]), charge
		}
	}
	checked, most := entry[17], le.Uint32(entry[20:])
	if checked != 0xff && args[checked] > most {
		return le.Uint32(entry[28:]), charge
	}

	call := []byte{0x10, byte(fn), 0, 0}
	for _, a := range args {
		call = le.AppendUint32(call, a)
	}
	call = le.AppendUint64(call, uint64(m.Budget-4))
	if access&1 == 0 {
		call = le.AppendUint32(call, 0)
	} else {
		call = le.AppendUint32(call, uint32(len(buffer)))
		call = append(call, buffer...)
	}
	reply := b.request(byte(slot), 0x90, call)
	if reply[1]&1 != 0 {
		m.X[11] = le.Uint32(reply[8:])
	}
	at := 20
	for range le.Uint32(reply[16:]) {
		offset := uint64(le.Uint32(reply[at:]) - addr)
		size := uint64(le.Uint32(reply[at+4:]))
		if access&2 == 0 || offset+size > uint64(len(buffer)) {
			check(fmt.Errorf("a write outside the buffer"))
		}
		copy(buffer[offset:], reply[at+8:at+8+int(size)])
		at += 8 + int(size)
	}
	extra := int64(le.Uint32(reply[12:]))
	return le.Uint32(reply[4:]), charge + extra
}

func (b *Bus) End() {
	stopped := byte(flag(b.m.Stopped != Running))
	end := []byte{0x31, stopped, 0, 0}
	b.send(0, le.AppendUint32(end, b.m.Frame))
}

func (b *Bus) State(slot byte) []byte {
	return b.request(slot, 0xc0, []byte{0x40})[8:]
}

func (b *Bus) Close() {
	b.send(0, []byte{0x60})
	b.conn.Close()
	b.driver.Wait()
	os.RemoveAll(b.dir)
}
