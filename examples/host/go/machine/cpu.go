package machine

import (
	"encoding/binary"
	"fmt"
)

var le = binary.LittleEndian

const (
	ROMBase = 0x10000
	RAMBase = 0x100000
)

const (
	Illegal = 1 + iota
	Breakpoint
	FetchFault
	LoadFault
	StoreFault
	Misaligned
)

type Fault struct {
	Kind              byte
	Address, Encoding uint32
}

func (f *Fault) Error() string {
	kind := []string{"", "illegal instruction", "breakpoint",
		"fetch", "load", "store", "misaligned"}[f.Kind]
	return fmt.Sprintf("fault: %s at %08x", kind, f.Address)
}

type CPU struct {
	X   [32]uint32
	PC  uint32
	ROM []byte
	RAM []byte
}

func within(mem []byte, base, addr, size uint32) []byte {
	offset := uint64(addr) - uint64(base)
	if addr < base || offset+uint64(size) > uint64(len(mem)) {
		return nil
	}
	return mem[addr-base : addr-base+size]
}

func (c *CPU) Memory(addr, size uint32, write bool) []byte {
	b := within(c.RAM, RAMBase, addr, size)
	if b == nil && !write {
		b = within(c.ROM, ROMBase, addr, size)
	}
	return b
}

func (c *CPU) access(addr, size uint32, write bool) (
	[]byte, *Fault) {
	b := c.Memory(addr, size, write)
	switch {
	case addr%size != 0:
		return nil, &Fault{Kind: Misaligned, Address: addr}
	case b == nil && write:
		return nil, &Fault{Kind: StoreFault, Address: addr}
	case b == nil:
		return nil, &Fault{Kind: LoadFault, Address: addr}
	}
	return b, nil
}

func (c *CPU) fetch() (uint32, *Fault) {
	lo := c.Memory(c.PC, 2, false)
	if lo == nil {
		return 0, &Fault{Kind: FetchFault, Address: c.PC}
	}
	parcel := uint32(le.Uint16(lo))
	if parcel&3 != 3 {
		return parcel, nil
	}
	hi := c.Memory(c.PC+2, 2, false)
	if hi == nil {
		return 0, &Fault{Kind: FetchFault, Address: c.PC + 2}
	}
	return parcel | uint32(le.Uint16(hi))<<16, nil
}

func (c *CPU) Step() (cost int64, ecall bool, fault *Fault) {
	raw, fault := c.fetch()
	if fault != nil {
		return 0, false, fault
	}
	in, next := decode(raw), c.PC+4
	if raw&3 != 3 {
		in, next = expand(raw), c.PC+2
	}
	cost = 1
	if c.PC >= RAMBase {
		cost = 4
	}
	ecall, fault = c.execute(in, next)
	if fault != nil && fault.Kind == Illegal {
		fault.Address, fault.Encoding = c.PC, raw
	}
	return cost, ecall, fault
}

func (c *CPU) set(rd, v uint32) {
	if rd != 0 {
		c.X[rd] = v
	}
}

func (c *CPU) execute(in instruction, next uint32) (
	bool, *Fault) {
	a, b, pc := c.X[in.rs1], c.X[in.rs2], c.PC
	illegal := &Fault{Kind: Illegal}
	switch in.op {
	case 0x37:
		c.set(in.rd, in.imm)
	case 0x17:
		c.set(in.rd, pc+in.imm)
	case 0x6f:
		c.set(in.rd, next)
		next = pc + in.imm
	case 0x67:
		if in.f3 != 0 {
			return false, illegal
		}
		c.set(in.rd, next)
		next = (a + in.imm) &^ 1
	case 0x63:
		if in.f3 == 2 || in.f3 == 3 {
			return false, illegal
		}
		if branches(in.f3, a, b) {
			next = pc + in.imm
		}
	case 0x03:
		if in.f3 == 3 || in.f3 > 5 {
			return false, illegal
		}
		size := uint32(1) << (in.f3 & 3)
		mem, fault := c.access(a+in.imm, size, false)
		if fault != nil {
			return false, fault
		}
		var word [4]byte
		copy(word[:], mem)
		v := le.Uint32(word[:])
		if in.f3 < 2 {
			v = sext(v, 8*size)
		}
		c.set(in.rd, v)
	case 0x23:
		if in.f3 > 2 {
			return false, illegal
		}
		mem, fault := c.access(a+in.imm, 1<<in.f3, true)
		if fault != nil {
			return false, fault
		}
		var word [4]byte
		le.PutUint32(word[:], b)
		copy(mem, word[:])
	case 0x13:
		arithmetic := in.f3 == 5 && in.f7 == 32
		if in.f3&3 == 1 && in.f7 != 0 && !arithmetic {
			return false, illegal
		}
		c.set(in.rd, alu(in.f3, arithmetic, a, in.imm))
	case 0x33:
		alternate := in.f7 == 32 && (in.f3 == 0 || in.f3 == 5)
		switch {
		case in.f7 == 1:
			c.set(in.rd, muldiv(in.f3, a, b))
		case in.f7 == 0 || alternate:
			c.set(in.rd, alu(in.f3, alternate, a, b))
		default:
			return false, illegal
		}
	case 0x0f:
		if in.f3 != 0 {
			return false, illegal
		}
	case 0x73:
		if in.f3|in.rd|in.rs1 != 0 || in.imm > 1 {
			return false, illegal
		}
		if in.imm == 1 {
			return false, &Fault{Kind: Breakpoint, Address: pc}
		}
		c.PC = next
		return true, nil
	default:
		return false, illegal
	}
	c.PC = next
	return false, nil
}

func branches(f3, a, b uint32) bool {
	var taken bool
	switch f3 >> 1 {
	case 0:
		taken = a == b
	case 2:
		taken = int32(a) < int32(b)
	case 3:
		taken = a < b
	}
	return taken != (f3&1 == 1)
}

func alu(f3 uint32, alternate bool, a, b uint32) uint32 {
	switch f3 {
	case 0:
		if alternate {
			return a - b
		}
		return a + b
	case 1:
		return a << (b & 31)
	case 2:
		return flag(int32(a) < int32(b))
	case 3:
		return flag(a < b)
	case 4:
		return a ^ b
	case 5:
		if alternate {
			return uint32(int32(a) >> (b & 31))
		}
		return a >> (b & 31)
	case 6:
		return a | b
	}
	return a & b
}

func flag(b bool) uint32 {
	if b {
		return 1
	}
	return 0
}

func muldiv(f3, a, b uint32) uint32 {
	sa, sb := int64(int32(a)), int64(int32(b))
	switch {
	case f3 == 0:
		return a * b
	case f3 == 1:
		return uint32(sa * sb >> 32)
	case f3 == 2:
		return uint32(sa * int64(b) >> 32)
	case f3 == 3:
		return uint32(uint64(a) * uint64(b) >> 32)
	case b == 0 && f3 < 6:
		return 0xffffffff
	case b == 0:
		return a
	case f3 == 4:
		return uint32(int32(a) / int32(b))
	case f3 == 5:
		return a / b
	case f3 == 6:
		return uint32(int32(a) % int32(b))
	}
	return a % b
}
