package machine

type instruction struct {
	op, f3, f7, rd, rs1, rs2, imm uint32
}

func sext(v, bits uint32) uint32 {
	return uint32(int32(v<<(32-bits)) >> (32 - bits))
}

func decode(i uint32) instruction {
	in := instruction{op: i & 0x7f, rd: i >> 7 & 31,
		f3: i >> 12 & 7, rs1: i >> 15 & 31, rs2: i >> 20 & 31,
		f7: i >> 25}
	switch in.op {
	case 0x37, 0x17:
		in.imm = i &^ 0xfff
	case 0x6f:
		in.imm = sext(i>>11&0x100000|i&0xff000|i>>9&0x800|
			i>>20&0x7fe, 21)
	case 0x63:
		in.imm = sext(i>>19&0x1000|i<<4&0x800|i>>20&0x7e0|
			i>>7&0x1e, 13)
	case 0x23:
		in.imm = sext(i>>20&0xfe0|i>>7&31, 12)
	default:
		in.imm = sext(i>>20, 12)
	}
	return in
}

func expand(c uint32) instruction {
	rd, rs2 := c>>7&31, c>>2&31
	rdc, rs2c := 8+c>>7&7, 8+c>>2&7
	imm := sext(c>>7&0x20|c>>2&31, 6)
	word := c>>7&0x38 | c>>4&4 | c<<1&0x40
	jump := sext(c>>1&0x800|c>>7&0x10|c>>1&0x300|c<<2&0x400|
		c>>1&0x40|c<<1&0x80|c>>2&0xe|c<<3&0x20, 12)
	branch := sext(c>>4&0x100|c>>7&0x18|c<<1&0xc0|c>>2&6|
		c<<3&0x20, 9)
	switch c&3<<3 | c>>13 {
	case 0o00:
		u := c>>7&0x30 | c>>1&0x3c0 | c>>4&4 | c>>2&8
		if u != 0 {
			return instruction{op: 0x13, rd: rs2c, rs1: 2,
				imm: u}
		}
	case 0o02:
		return instruction{op: 0x03, f3: 2, rd: rs2c, rs1: rdc,
			imm: word}
	case 0o06:
		return instruction{op: 0x23, f3: 2, rs1: rdc, rs2: rs2c,
			imm: word}
	case 0o10:
		return instruction{op: 0x13, rd: rd, rs1: rd, imm: imm}
	case 0o11:
		return instruction{op: 0x6f, rd: 1, imm: jump}
	case 0o12:
		return instruction{op: 0x13, rd: rd, imm: imm}
	case 0o13:
		sp := sext(c>>3&0x200|c>>2&0x10|c<<1&0x40|c<<4&0x180|
			c<<3&0x20, 10)
		if rd == 2 && sp != 0 {
			return instruction{op: 0x13, rd: 2, rs1: 2, imm: sp}
		}
		if rd != 2 && imm != 0 {
			return instruction{op: 0x37, rd: rd, imm: imm << 12}
		}
	case 0o14:
		shift := c>>7&0x20 | rs2
		switch c >> 10 & 3 {
		case 0, 1:
			if shift < 32 {
				return instruction{op: 0x13, f3: 5,
					f7: c >> 10 & 1 << 5, rd: rdc, rs1: rdc,
					imm: shift}
			}
		case 2:
			return instruction{op: 0x13, f3: 7, rd: rdc,
				rs1: rdc, imm: imm}
		case 3:
			if c&0x1000 == 0 {
				f3 := []uint32{0, 4, 6, 7}[c>>5&3]
				return instruction{op: 0x33, f3: f3,
					f7: flag(f3 == 0) << 5, rd: rdc, rs1: rdc,
					rs2: rs2c}
			}
		}
	case 0o15:
		return instruction{op: 0x6f, imm: jump}
	case 0o16, 0o17:
		return instruction{op: 0x63, f3: c >> 13 & 1, rs1: rdc,
			imm: branch}
	case 0o20:
		if c&0x1000 == 0 {
			return instruction{op: 0x13, f3: 1, rd: rd, rs1: rd,
				imm: rs2}
		}
	case 0o22:
		if rd != 0 {
			return instruction{op: 0x03, f3: 2, rd: rd, rs1: 2,
				imm: c>>7&0x20 | c>>2&0x1c | c<<4&0xc0}
		}
	case 0o24:
		jalr := c >> 12 & 1
		switch {
		case rs2 != 0 && jalr == 0:
			return instruction{op: 0x33, rd: rd, rs2: rs2}
		case rs2 != 0:
			return instruction{op: 0x33, rd: rd, rs1: rd,
				rs2: rs2}
		case rd == 0 && jalr == 1:
			return instruction{op: 0x73, imm: 1}
		case rd != 0:
			return instruction{op: 0x67, rd: jalr, rs1: rd}
		}
	case 0o26:
		return instruction{op: 0x23, f3: 2, rs1: 2, rs2: rs2,
			imm: c>>7&0x3c | c>>1&0xc0}
	}
	return instruction{}
}
