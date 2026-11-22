#[derive(Clone, Copy, Default)]
pub struct Instruction {
    pub op: u32,
    pub f3: u32,
    pub f7: u32,
    pub rd: u32,
    pub rs1: u32,
    pub rs2: u32,
    pub imm: u32,
}

pub fn sext(v: u32, bits: u32) -> u32 {
    ((v << (32 - bits)) as i32 >> (32 - bits)) as u32
}

pub fn decode(i: u32) -> Instruction {
    let op = i & 0x7f;
    let imm = match op {
        0x37 | 0x17 => i & !0xfff,
        0x6f => sext(
            i >> 11 & 0x100000
                | i & 0xff000
                | i >> 9 & 0x800
                | i >> 20 & 0x7fe,
            21,
        ),
        0x63 => sext(
            i >> 19 & 0x1000
                | i << 4 & 0x800
                | i >> 20 & 0x7e0
                | i >> 7 & 0x1e,
            13,
        ),
        0x23 => sext(i >> 20 & 0xfe0 | i >> 7 & 31, 12),
        _ => sext(i >> 20, 12),
    };
    Instruction {
        op,
        f3: i >> 12 & 7,
        f7: i >> 25,
        rd: i >> 7 & 31,
        rs1: i >> 15 & 31,
        rs2: i >> 20 & 31,
        imm,
    }
}

pub fn expand(c: u32) -> Instruction {
    let (rd, rs2) = (c >> 7 & 31, c >> 2 & 31);
    let (rdc, rs2c) = (8 + (c >> 7 & 7), 8 + (c >> 2 & 7));
    let imm = sext(c >> 7 & 0x20 | c >> 2 & 31, 6);
    let word = c >> 7 & 0x38 | c >> 4 & 4 | c << 1 & 0x40;
    let jump = sext(
        c >> 1 & 0x800
            | c >> 7 & 0x10
            | c >> 1 & 0x300
            | c << 2 & 0x400
            | c >> 1 & 0x40
            | c << 1 & 0x80
            | c >> 2 & 0xe
            | c << 3 & 0x20,
        12,
    );
    let branch = sext(
        c >> 4 & 0x100
            | c >> 7 & 0x18
            | c << 1 & 0xc0
            | c >> 2 & 6
            | c << 3 & 0x20,
        9,
    );
    let sp = sext(
        c >> 3 & 0x200
            | c >> 2 & 0x10
            | c << 1 & 0x40
            | c << 4 & 0x180
            | c << 3 & 0x20,
        10,
    );
    let four = c >> 7 & 0x30
        | c >> 1 & 0x3c0
        | c >> 4 & 4
        | c >> 2 & 8;
    let shift = c >> 7 & 0x20 | rs2;
    let (high, funct2) = (c & 0x1000 != 0, c >> 10 & 3);
    let none = Instruction::default();
    let i = Instruction { rd, rs1: rd, ..none };
    let compact = Instruction { rd: rdc, rs1: rdc, ..none };
    match (c & 3) << 3 | c >> 13 {
        0o00 if four != 0 => Instruction {
            op: 0x13,
            rd: rs2c,
            rs1: 2,
            imm: four,
            ..none
        },
        0o02 => Instruction {
            op: 0x03,
            f3: 2,
            rd: rs2c,
            rs1: rdc,
            imm: word,
            ..none
        },
        0o06 => Instruction {
            op: 0x23,
            f3: 2,
            rs1: rdc,
            rs2: rs2c,
            imm: word,
            ..none
        },
        0o10 => Instruction { op: 0x13, imm, ..i },
        0o11 => {
            Instruction { op: 0x6f, rd: 1, imm: jump, ..none }
        }
        0o12 => Instruction { op: 0x13, rs1: 0, imm, ..i },
        0o13 if rd == 2 && sp != 0 => {
            Instruction { op: 0x13, imm: sp, ..i }
        }
        0o13 if rd != 2 && imm != 0 => {
            Instruction { op: 0x37, imm: imm << 12, ..i }
        }
        0o14 if funct2 < 2 && shift < 32 => Instruction {
            op: 0x13,
            f3: 5,
            f7: funct2 << 5,
            imm: shift,
            ..compact
        },
        0o14 if funct2 == 2 => {
            Instruction { op: 0x13, f3: 7, imm, ..compact }
        }
        0o14 if !high => {
            let f3 = [0, 4, 6, 7][(c >> 5 & 3) as usize];
            let f7 = if f3 == 0 { 32 } else { 0 };
            Instruction {
                op: 0x33,
                f3,
                f7,
                rs2: rs2c,
                ..compact
            }
        }
        0o15 => Instruction { op: 0x6f, imm: jump, ..none },
        0o16 | 0o17 => Instruction {
            op: 0x63,
            f3: c >> 13 & 1,
            rs1: rdc,
            imm: branch,
            ..none
        },
        0o20 if !high => {
            Instruction { op: 0x13, f3: 1, imm: rs2, ..i }
        }
        0o22 if rd != 0 => Instruction {
            op: 0x03,
            f3: 2,
            rs1: 2,
            imm: c >> 7 & 0x20 | c >> 2 & 0x1c | c << 4 & 0xc0,
            ..i
        },
        0o24 if rs2 != 0 && !high => {
            Instruction { op: 0x33, rs1: 0, rs2, ..i }
        }
        0o24 if rs2 != 0 => Instruction { op: 0x33, rs2, ..i },
        0o24 if rd == 0 && high => {
            Instruction { op: 0x73, imm: 1, ..none }
        }
        0o24 if rd != 0 => {
            Instruction { op: 0x67, rd: high as u32, ..i }
        }
        0o26 => Instruction {
            op: 0x23,
            f3: 2,
            rs1: 2,
            rs2,
            imm: c >> 7 & 0x3c | c >> 1 & 0xc0,
            ..none
        },
        _ => none,
    }
}
