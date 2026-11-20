//! The RV32IMC interpreter of machine profile 1 (SPEC §cpu).
//!
//! Exactly RV32I + M + C, user mode only: no CSRs, no A/F/D, no FENCE.I.
//! Every reserved, illegal or unsupported encoding (including all of C's
//! floating-point and RV64-only forms) faults. Memory is two flat regions,
//! ROM (read and execute) and RAM (read, write and execute); every other
//! address faults, as do misaligned loads and stores and stores to ROM.
//!
//! Instructions are first decoded into an [`Inst`] (compressed forms become
//! their base-ISA equivalents), then executed. ROM cannot change, so a
//! machine decodes it once ([`Code`]) and [`Cpu::run_cached`] executes from
//! that; [`Cpu::run`] decodes as it goes. Both share `decode` and the
//! execution loop. An instruction fetched from ROM costs 1 budget unit; one
//! fetched from RAM is decoded afresh every time (so code written to RAM
//! takes effect at once) and costs [`RAM_INSTRUCTION_COST`] (SPEC §frames).

use std::fmt;

/// Guest address of ROM byte 0 (the entry point).
pub const ROM_BASE: u32 = 0x0001_0000;
/// Guest address of RAM byte 0.
pub const RAM_BASE: u32 = 0x0010_0000;

/// Budget units an instruction fetched from RAM costs (one from ROM costs 1).
/// Provisional (SPEC "Provisional values").
pub const RAM_INSTRUCTION_COST: i64 = 4;

/// Why the machine stopped with a fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// An illegal, reserved or unsupported instruction; the value is its
    /// encoding (16 or 32 bits).
    Illegal(u32),
    /// `ebreak` or `c.ebreak`.
    Ebreak,
    /// The instruction at `pc` (or its second half) is not in ROM or RAM.
    Fetch,
    /// A load from outside ROM and RAM.
    Load,
    /// A store outside RAM (ROM included).
    Store,
    /// A load or store whose address is not a multiple of its size.
    Misaligned,
}

/// A fault: what happened, the address of the instruction and the address
/// it touched (the instruction's own address for `Illegal` and `Ebreak`,
/// the address that could not be fetched for `Fetch`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub kind: FaultKind,
    pub pc: u32,
    pub addr: u32,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            FaultKind::Illegal(i) if i & 3 != 3 => format!("illegal instruction {i:04x}"),
            FaultKind::Illegal(i) => format!("illegal instruction {i:08x}"),
            FaultKind::Ebreak => "ebreak".into(),
            FaultKind::Fetch => "instruction fetch outside ROM and RAM".into(),
            FaultKind::Load => "load outside ROM and RAM".into(),
            FaultKind::Store => "store outside RAM".into(),
            FaultKind::Misaligned => "misaligned load or store".into(),
        };
        write!(f, "{what} at pc {:08x}, address {:08x}", self.pc, self.addr)
    }
}

/// Why [`Cpu::run`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The budget reached zero; `pc` is the next instruction.
    Budget,
    /// An `ecall` executed (and was counted); `pc` is already past it.
    Ecall,
    Fault(Fault),
}

/// The memory the CPU sees.
pub struct Bus<'a> {
    pub rom: &'a [u8],
    pub ram: &'a mut [u8],
}

impl Bus<'_> {
    /// The 16-bit parcel at `addr` (even), from ROM or RAM.
    #[inline(always)]
    fn parcel(&self, addr: u32) -> Option<u16> {
        let r = addr.wrapping_sub(ROM_BASE) as usize;
        if r < self.rom.len().saturating_sub(1) {
            return Some(u16::from_le_bytes([self.rom[r], self.rom[r + 1]]));
        }
        let m = addr.wrapping_sub(RAM_BASE) as usize;
        if m < self.ram.len().saturating_sub(1) {
            return Some(u16::from_le_bytes([self.ram[m], self.ram[m + 1]]));
        }
        None
    }

    /// `N` bytes at `addr` (aligned to `N`), from RAM or ROM.
    #[inline(always)]
    fn read<const N: usize>(&self, addr: u32) -> Result<[u8; N], FaultKind> {
        if addr as usize & (N - 1) != 0 {
            return Err(FaultKind::Misaligned);
        }
        let m = addr.wrapping_sub(RAM_BASE) as usize;
        if m < self.ram.len() && self.ram.len() - m >= N {
            return Ok(self.ram[m..m + N].try_into().unwrap());
        }
        let r = addr.wrapping_sub(ROM_BASE) as usize;
        if r < self.rom.len() && self.rom.len() - r >= N {
            return Ok(self.rom[r..r + N].try_into().unwrap());
        }
        Err(FaultKind::Load)
    }

    /// Stores `N` bytes at `addr` (aligned to `N`) in RAM.
    #[inline(always)]
    fn write<const N: usize>(&mut self, addr: u32, bytes: [u8; N]) -> Result<(), FaultKind> {
        if addr as usize & (N - 1) != 0 {
            return Err(FaultKind::Misaligned);
        }
        let m = addr.wrapping_sub(RAM_BASE) as usize;
        if m < self.ram.len() && self.ram.len() - m >= N {
            self.ram[m..m + N].copy_from_slice(&bytes);
            return Ok(());
        }
        Err(FaultKind::Store)
    }

    /// Decodes the instruction at `pc`.
    fn decode_at(&self, pc: u32) -> Inst {
        match self.parcel(pc) {
            None => Inst::FETCH_FAULT,
            Some(lo) if lo & 3 != 3 => decode16(lo),
            Some(lo) => match self.parcel(pc.wrapping_add(2)) {
                None => Inst { op: Op::FetchFault2, ..Inst::FETCH_FAULT },
                Some(hi) => decode32(lo as u32 | (hi as u32) << 16),
            },
        }
    }
}

/// Operations after decoding. Compressed instructions become the base
/// operation they expand to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    Lui,
    Auipc,
    Jal,
    Jalr,
    Beq,
    Bne,
    Blt,
    Bge,
    Bltu,
    Bgeu,
    Lb,
    Lh,
    Lw,
    Lbu,
    Lhu,
    Sb,
    Sh,
    Sw,
    Addi,
    Slti,
    Sltiu,
    Xori,
    Ori,
    Andi,
    Slli,
    Srli,
    Srai,
    Add,
    Sub,
    Sll,
    Slt,
    Sltu,
    Xor,
    Srl,
    Sra,
    Or,
    And,
    Mul,
    Mulh,
    Mulhsu,
    Mulhu,
    Div,
    Divu,
    Rem,
    Remu,
    Fence,
    Ecall,
    Ebreak,
    /// `imm` holds the encoding.
    Illegal,
    /// The instruction's first parcel is not in ROM or RAM.
    FetchFault,
    /// Its second parcel is not (a 32-bit instruction at the very end).
    FetchFault2,
}

/// Every `Op`, indexed by its value (to unpack `Inst::pack`).
const OPS: [Op; 256] = {
    use Op::*;
    let all = [
        Lui, Auipc, Jal, Jalr, Beq, Bne, Blt, Bge, Bltu, Bgeu, Lb, Lh, Lw, Lbu, Lhu, Sb, Sh, Sw, Addi, Slti, Sltiu, Xori,
        Ori, Andi, Slli, Srli, Srai, Add, Sub, Sll, Slt, Sltu, Xor, Srl, Sra, Or, And, Mul, Mulh, Mulhsu, Mulhu, Div,
        Divu, Rem, Remu, Fence, Ecall, Ebreak, Illegal, FetchFault, FetchFault2,
    ];
    let mut t = [Illegal; 256];
    let mut k = 0;
    while k < all.len() {
        t[all[k] as usize] = all[k];
        k += 1;
    }
    t
};

/// A decoded instruction. `imm` is the immediate (already shifted for
/// LUI/AUIPC, the offset for jumps and branches, the shift amount for
/// shifts); `len` is 2 or 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inst {
    pub op: Op,
    pub rd: u8,
    pub rs1: u8,
    pub rs2: u8,
    pub len: u8,
    pub imm: u32,
}

impl Inst {
    /// Packs into 64 bits (op, rd, rs1, rs2, len from bit 0; imm in the
    /// high half), so that decoded instructions live in registers.
    #[inline(always)]
    fn pack(self) -> u64 {
        self.op as u64
            // rd 0 becomes 32: writes to x0 land in a scratch register.
            | (if self.rd == 0 { 32 } else { self.rd as u64 }) << 8
            | (self.rs1 as u64) << 14
            | (self.rs2 as u64) << 19
            | ((self.len == 4) as u64) << 24
            | (self.imm as u64) << 32
    }

    const FETCH_FAULT: Inst = Inst { op: Op::FetchFault, rd: 0, rs1: 0, rs2: 0, len: 2, imm: 0 };

    fn new(op: Op, rd: u32, rs1: u32, rs2: u32, len: u8, imm: u32) -> Self {
        Inst { op, rd: rd as u8, rs1: rs1 as u8, rs2: rs2 as u8, len, imm }
    }

    fn illegal(enc: u32, len: u8) -> Self {
        Inst { op: Op::Illegal, rd: 0, rs1: 0, rs2: 0, len, imm: enc }
    }
}

/// Sign-extends the low `bits` bits of `v`.
#[inline(always)]
fn sext(v: u32, bits: u32) -> u32 {
    let s = 32 - bits;
    (((v << s) as i32) >> s) as u32
}

/// The offset of C.JAL and C.J.
fn cj_offset(i: u32) -> u32 {
    let off = ((i >> 1) & 0x800)
        | ((i >> 7) & 0x10)
        | ((i >> 1) & 0x300)
        | ((i << 2) & 0x400)
        | ((i >> 1) & 0x40)
        | ((i << 1) & 0x80)
        | ((i >> 2) & 0xE)
        | ((i << 3) & 0x20);
    sext(off, 12)
}

/// Decodes a 16-bit (compressed) instruction.
pub fn decode16(lo: u16) -> Inst {
    use Op::*;
    let i = lo as u32;
    let rd = (i >> 7) & 31; // also rs1 of most quadrant 1 and 2 forms
    let rs2 = (i >> 2) & 31;
    let rdp = ((i >> 2) & 7) + 8; // rd' / rs2'
    let rs1p = ((i >> 7) & 7) + 8; // rs1' / rd'
    let imm6 = sext(((i >> 7) & 0x20) | ((i >> 2) & 0x1F), 6);
    let c = |op, rd, rs1, rs2, imm| Inst::new(op, rd, rs1, rs2, 2, imm);
    let bad = Inst::illegal(i, 2);
    let lw_imm = ((i >> 7) & 0x38) | ((i >> 4) & 0x4) | ((i << 1) & 0x40);
    match (i & 3, i >> 13) {
        // C.ADDI4SPN (nzuimm 0 is reserved: this includes the all-zero parcel)
        (0, 0b000) => {
            let imm = ((i >> 7) & 0x30) | ((i >> 1) & 0x3C0) | ((i >> 4) & 0x4) | ((i >> 2) & 0x8);
            if imm == 0 { bad } else { c(Addi, rdp, 2, 0, imm) }
        }
        (0, 0b010) => c(Lw, rdp, rs1p, 0, lw_imm),
        (0, 0b110) => c(Sw, 0, rs1p, rdp, lw_imm),
        // C.FLD, C.FLW, reserved, C.FSD, C.FSW
        (0, _) => bad,
        // C.ADDI (rd 0 or imm 0: hints)
        (1, 0b000) => c(Addi, rd, rd, 0, imm6),
        // C.JAL (RV32)
        (1, 0b001) => c(Jal, 1, 0, 0, cj_offset(i)),
        // C.LI (rd 0: hint)
        (1, 0b010) => c(Addi, rd, 0, 0, imm6),
        // C.ADDI16SP / C.LUI (nzimm 0 reserved; C.LUI rd 0: hint)
        (1, 0b011) if rd == 2 => {
            let imm =
                ((i >> 3) & 0x200) | ((i >> 2) & 0x10) | ((i << 1) & 0x40) | ((i << 4) & 0x180) | ((i << 3) & 0x20);
            if imm == 0 { bad } else { c(Addi, 2, 2, 0, sext(imm, 10)) }
        }
        (1, 0b011) => {
            if imm6 == 0 { bad } else { c(Lui, rd, 0, 0, imm6 << 12) }
        }
        // MISC-ALU
        (1, 0b100) => match (i >> 10) & 3 {
            // C.SRLI / C.SRAI: shamt[5] = 1 is reserved on RV32; shamt 0 is a hint.
            0b00 | 0b01 if i & 0x1000 != 0 => bad,
            0b00 => c(Srli, rs1p, rs1p, 0, rs2),
            0b01 => c(Srai, rs1p, rs1p, 0, rs2),
            0b10 => c(Andi, rs1p, rs1p, 0, imm6),
            // C.SUBW, C.ADDW (RV64) and reserved
            _ if i & 0x1000 != 0 => bad,
            _ => c([Sub, Xor, Or, And][((i >> 5) & 3) as usize], rs1p, rs1p, rdp, 0),
        },
        // C.J
        (1, 0b101) => c(Jal, 0, 0, 0, cj_offset(i)),
        // C.BEQZ / C.BNEZ
        (1, 0b110) | (1, 0b111) => {
            let off = ((i >> 4) & 0x100) | ((i >> 7) & 0x18) | ((i << 1) & 0xC0) | ((i >> 2) & 0x6) | ((i << 3) & 0x20);
            c(if i >> 13 == 0b110 { Beq } else { Bne }, 0, rs1p, 0, sext(off, 9))
        }
        // C.SLLI (shamt[5] = 1 reserved on RV32; rd 0 or shamt 0: hints)
        (2, 0b000) => {
            if i & 0x1000 != 0 { bad } else { c(Slli, rd, rd, 0, rs2) }
        }
        // C.LWSP (rd 0 reserved)
        (2, 0b010) => {
            if rd == 0 { bad } else { c(Lw, rd, 2, 0, ((i >> 7) & 0x20) | ((i >> 2) & 0x1C) | ((i << 4) & 0xC0)) }
        }
        (2, 0b100) => match (i & 0x1000 != 0, rd, rs2) {
            (false, 0, 0) => bad,                     // C.JR with rs1 0: reserved
            (false, _, 0) => c(Jalr, 0, rd, 0, 0),    // C.JR
            (false, _, _) => c(Add, rd, 0, rs2, 0),   // C.MV (rd 0: hint)
            (true, 0, 0) => c(Ebreak, 0, 0, 0, 0),    // C.EBREAK
            (true, _, 0) => c(Jalr, 1, rd, 0, 0),     // C.JALR
            (true, _, _) => c(Add, rd, rd, rs2, 0),   // C.ADD (rd 0: hint)
        },
        // C.SWSP
        (2, 0b110) => c(Sw, 0, 2, rs2, ((i >> 7) & 0x3C) | ((i >> 1) & 0xC0)),
        // C.FLDSP, C.FLWSP, C.FSDSP, C.FSWSP
        _ => bad,
    }
}

/// Decodes a 32-bit instruction.
pub fn decode32(i: u32) -> Inst {
    use Op::*;
    let rd = (i >> 7) & 31;
    let rs1 = (i >> 15) & 31;
    let rs2 = (i >> 20) & 31;
    let f3 = (i >> 12) & 7;
    let f7 = i >> 25;
    let imm_i = ((i as i32) >> 20) as u32;
    let w = |op, imm| Inst::new(op, rd, rs1, rs2, 4, imm);
    let bad = Inst::illegal(i, 4);
    match (i >> 2) & 31 {
        0b01101 => w(Lui, i & 0xFFFF_F000),
        0b00101 => w(Auipc, i & 0xFFFF_F000),
        0b11011 => {
            let off = ((((i as i32) >> 31) as u32) << 20) | (i & 0xF_F000) | ((i >> 9) & 0x800) | ((i >> 20) & 0x7FE);
            w(Jal, off)
        }
        0b11001 if f3 == 0 => w(Jalr, imm_i),
        0b11000 => {
            let off = ((((i as i32) >> 31) as u32) << 12) | ((i << 4) & 0x800) | ((i >> 20) & 0x7E0) | ((i >> 7) & 0x1E);
            match f3 {
                0b000 => w(Beq, off),
                0b001 => w(Bne, off),
                0b100 => w(Blt, off),
                0b101 => w(Bge, off),
                0b110 => w(Bltu, off),
                0b111 => w(Bgeu, off),
                _ => bad,
            }
        }
        0b00000 => match f3 {
            0b000 => w(Lb, imm_i),
            0b001 => w(Lh, imm_i),
            0b010 => w(Lw, imm_i),
            0b100 => w(Lbu, imm_i),
            0b101 => w(Lhu, imm_i),
            _ => bad,
        },
        0b01000 => {
            let imm = (imm_i & !0x1F) | ((i >> 7) & 0x1F);
            match f3 {
                0b000 => w(Sb, imm),
                0b001 => w(Sh, imm),
                0b010 => w(Sw, imm),
                _ => bad,
            }
        }
        0b00100 => match f3 {
            0b000 => w(Addi, imm_i),
            0b010 => w(Slti, imm_i),
            0b011 => w(Sltiu, imm_i),
            0b100 => w(Xori, imm_i),
            0b110 => w(Ori, imm_i),
            0b111 => w(Andi, imm_i),
            0b001 if f7 == 0 => w(Slli, rs2),
            0b101 if f7 == 0 => w(Srli, rs2),
            0b101 if f7 == 0b0100000 => w(Srai, rs2),
            _ => bad,
        },
        0b01100 => match (f7, f3) {
            (0, 0b000) => w(Add, 0),
            (0b0100000, 0b000) => w(Sub, 0),
            (0, 0b001) => w(Sll, 0),
            (0, 0b010) => w(Slt, 0),
            (0, 0b011) => w(Sltu, 0),
            (0, 0b100) => w(Xor, 0),
            (0, 0b101) => w(Srl, 0),
            (0b0100000, 0b101) => w(Sra, 0),
            (0, 0b110) => w(Or, 0),
            (0, 0b111) => w(And, 0),
            (1, f3) => w([Mul, Mulh, Mulhsu, Mulhu, Div, Divu, Rem, Remu][f3 as usize], 0),
            _ => bad,
        },
        // MISC-MEM: FENCE (and FENCE.TSO, PAUSE, the hint forms) is a no-op;
        // FENCE.I is not in RV32I.
        0b00011 if f3 == 0 => w(Fence, 0),
        // SYSTEM: only ECALL and EBREAK; no CSRs.
        0b11100 if i == 0x0000_0073 => w(Ecall, 0),
        0b11100 if i == 0x0010_0073 => w(Ebreak, 0),
        _ => bad,
    }
}

/// The decoded ROM: one entry per 2-byte position (so both halves of a
/// 32-bit instruction have one).
pub struct Code {
    insts: Vec<u64>,
}

impl Code {
    pub fn new(rom: &[u8]) -> Self {
        let mut ram = [];
        let bus = Bus { rom, ram: &mut ram };
        Code { insts: (0..rom.len() as u32 / 2).map(|k| bus.decode_at(ROM_BASE + 2 * k).pack()).collect() }
    }
}

/// The register file and program counter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpu {
    /// x0-x31; x0 is always zero between instructions.
    pub x: [u32; 32],
    pub pc: u32,
    /// The lowest value `sp` (x2) has had at the start of an instruction,
    /// for measuring stack depth. Not part of the machine state.
    pub min_sp: u32,
}

impl Cpu {
    pub fn new(pc: u32, sp: u32) -> Self {
        let mut x = [0; 32];
        x[2] = sp;
        Cpu { x, pc, min_sp: sp }
    }

    /// Runs until `budget` is used up, an `ecall`, or a fault, decoding each
    /// instruction as it goes. An instruction in ROM costs 1, one in RAM
    /// [`RAM_INSTRUCTION_COST`]; an instruction that starts with a positive
    /// budget completes, so the budget can end below zero. On a fault `pc` is
    /// left at the faulting instruction, which is not counted and changes
    /// nothing.
    pub fn run(&mut self, bus: &mut Bus, budget: &mut i64) -> Stop {
        self.run_with(bus, None, budget)
    }

    /// As [`Cpu::run`], with ROM instructions taken from `code`, which must
    /// have been made from `bus.rom`.
    pub fn run_cached(&mut self, code: &Code, bus: &mut Bus, budget: &mut i64) -> Stop {
        self.run_with(bus, Some(code), budget)
    }

    #[inline(always)]
    fn run_with(&mut self, bus: &mut Bus, code: Option<&Code>, budget: &mut i64) -> Stop {
        use Op::*;
        let mut pc = self.pc;
        let mut left = *budget;
        let mut min_sp = self.min_sp;
        // x0-x31 and scratch (x32 takes writes to x0; the rest pad the
        // array so that a 6-bit index needs no bounds check).
        let mut regs = [0u32; 64];
        regs[..32].copy_from_slice(&self.x);
        let x = &mut regs;
        let stop = loop {
            if left <= 0 {
                break Stop::Budget;
            }
            min_sp = min_sp.min(x[2]);
            let slot = (pc.wrapping_sub(ROM_BASE) / 2) as usize;
            let (inst, cost) = match code {
                Some(c) if slot < c.insts.len() => (c.insts[slot], 1),
                _ if slot < bus.rom.len() / 2 => (bus.decode_at(pc).pack(), 1),
                _ => (bus.decode_at(pc).pack(), RAM_INSTRUCTION_COST),
            };
            left -= cost;
            macro_rules! fault {
                ($kind:expr, $addr:expr) => {{
                    // The faulting instruction was not executed.
                    left += cost;
                    break Stop::Fault(Fault { kind: $kind, pc, addr: $addr });
                }};
            }
            macro_rules! load {
                ($n:literal, $addr:expr) => {
                    match bus.read::<$n>($addr) {
                        Ok(b) => b,
                        Err(k) => fault!(k, $addr),
                    }
                };
            }
            macro_rules! store {
                ($addr:expr, $bytes:expr) => {
                    if let Err(k) = bus.write($addr, $bytes) {
                        fault!(k, $addr)
                    }
                };
            }
            let (rd, a, b, imm) =
                ((inst >> 8) as usize & 63, x[(inst >> 14) as usize & 31], x[(inst >> 19) as usize & 31], (inst >> 32) as u32);
            let next = pc.wrapping_add(2 + ((inst >> 23) as u32 & 2));
            let op = OPS[inst as u8 as usize];
            macro_rules! branch {
                ($cond:expr) => {{
                    pc = if $cond { pc.wrapping_add(imm) } else { next };
                    continue;
                }};
            }
            x[rd] = match op {
                Lui => imm,
                Auipc => pc.wrapping_add(imm),
                Jal => {
                    x[rd] = next;
                    pc = pc.wrapping_add(imm);
                    continue;
                }
                Jalr => {
                    // The target is computed before rd is written (rd may be rs1).
                    let t = a.wrapping_add(imm) & !1;
                    x[rd] = next;
                    pc = t;
                    continue;
                }
                Beq => branch!(a == b),
                Bne => branch!(a != b),
                Blt => branch!((a as i32) < (b as i32)),
                Bge => branch!((a as i32) >= (b as i32)),
                Bltu => branch!(a < b),
                Bgeu => branch!(a >= b),
                Lb => load!(1, a.wrapping_add(imm))[0] as i8 as u32,
                Lh => i16::from_le_bytes(load!(2, a.wrapping_add(imm))) as u32,
                Lw => u32::from_le_bytes(load!(4, a.wrapping_add(imm))),
                Lbu => load!(1, a.wrapping_add(imm))[0] as u32,
                Lhu => u16::from_le_bytes(load!(2, a.wrapping_add(imm))) as u32,
                Sb => {
                    store!(a.wrapping_add(imm), [b as u8]);
                    pc = next;
                    continue;
                }
                Sh => {
                    store!(a.wrapping_add(imm), (b as u16).to_le_bytes());
                    pc = next;
                    continue;
                }
                Sw => {
                    store!(a.wrapping_add(imm), b.to_le_bytes());
                    pc = next;
                    continue;
                }
                Addi => a.wrapping_add(imm),
                Slti => ((a as i32) < (imm as i32)) as u32,
                Sltiu => (a < imm) as u32,
                Xori => a ^ imm,
                Ori => a | imm,
                Andi => a & imm,
                Slli => a << imm,
                Srli => a >> imm,
                Srai => ((a as i32) >> imm) as u32,
                Add => a.wrapping_add(b),
                Sub => a.wrapping_sub(b),
                Sll => a << (b & 31),
                Slt => ((a as i32) < (b as i32)) as u32,
                Sltu => (a < b) as u32,
                Xor => a ^ b,
                Srl => a >> (b & 31),
                Sra => ((a as i32) >> (b & 31)) as u32,
                Or => a | b,
                And => a & b,
                Mul => a.wrapping_mul(b),
                Mulh => ((a as i32 as i64 * b as i32 as i64) >> 32) as u32,
                Mulhsu => ((a as i32 as i64 * b as u64 as i64) >> 32) as u32,
                Mulhu => ((a as u64 * b as u64) >> 32) as u32,
                Div => {
                    if b == 0 {
                        u32::MAX
                    } else {
                        (a as i32).wrapping_div(b as i32) as u32
                    }
                }
                Divu => a.checked_div(b).unwrap_or(u32::MAX),
                Rem => {
                    if b == 0 {
                        a
                    } else {
                        (a as i32).wrapping_rem(b as i32) as u32
                    }
                }
                Remu => a.checked_rem(b).unwrap_or(a),
                Fence => {
                    pc = next;
                    continue;
                }
                Ecall => {
                    pc = next;
                    break Stop::Ecall;
                }
                Ebreak => fault!(FaultKind::Ebreak, pc),
                Illegal => fault!(FaultKind::Illegal(imm), pc),
                FetchFault => fault!(FaultKind::Fetch, pc),
                FetchFault2 => fault!(FaultKind::Fetch, pc.wrapping_add(2)),
            };
            pc = next;
        };
        self.pc = pc;
        self.min_sp = min_sp;
        self.x.copy_from_slice(&regs[..32]);
        *budget = left;
        stop
    }
}
