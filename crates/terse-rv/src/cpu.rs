//! An RV32IMC interpreter for profile 2 (RV-PROFILE.md section 1).
//!
//! Exactly RV32I + M + C, user mode only: no CSRs, no A/F/D, no FENCE.I.
//! Every reserved, illegal or unsupported encoding (including all of C's
//! floating-point and RV64-only forms) faults. Memory is two flat regions,
//! ROM (read and execute) and RAM (read, write and execute); every other
//! address faults, as do misaligned loads and stores and stores to ROM.

use std::fmt;

/// Guest address of ROM byte 0 (the entry point).
pub const ROM_BASE: u32 = 0x0001_0000;
/// Guest address of RAM byte 0.
pub const RAM_BASE: u32 = 0x0010_0000;

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
/// it touched (the instruction's own address for `Fetch`, `Illegal` and
/// `Ebreak`).
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

/// Sign-extends the low `bits` bits of `v`.
#[inline(always)]
fn sext(v: u32, bits: u32) -> u32 {
    let s = 32 - bits;
    (((v << s) as i32) >> s) as u32
}

impl Cpu {
    pub fn new(pc: u32, sp: u32) -> Self {
        let mut x = [0; 32];
        x[2] = sp;
        Cpu { x, pc, min_sp: sp }
    }

    /// Runs until `budget` (instructions) is used up, an `ecall`, or a fault.
    /// Every instruction costs 1. On a fault `pc` is left at the faulting
    /// instruction and nothing it would have written is changed.
    pub fn run(&mut self, bus: &mut Bus, budget: &mut i64) -> Stop {
        let mut pc = self.pc;
        let mut left = *budget;
        let mut min_sp = self.min_sp;
        let x = &mut self.x;
        let stop = loop {
            if left <= 0 {
                break Stop::Budget;
            }
            left -= 1;
            min_sp = min_sp.min(x[2]);
            let Some(lo) = bus.parcel(pc) else {
                break Stop::Fault(Fault { kind: FaultKind::Fetch, pc, addr: pc });
            };
            macro_rules! fault {
                ($kind:expr, $addr:expr) => {
                    break Stop::Fault(Fault { kind: $kind, pc, addr: $addr })
                };
            }
            macro_rules! illegal {
                ($i:expr) => {
                    fault!(FaultKind::Illegal($i), pc)
                };
            }
            // Loads and stores: a fault leaves everything unchanged.
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

            if lo & 3 != 3 {
                // ---- Compressed (RVC) ----
                let i = lo as u32;
                let rd = (i >> 7) & 31; // also rs1 for most quadrant 1/2 forms
                let rs2 = (i >> 2) & 31;
                let rdp = ((i >> 2) & 7) + 8; // rd' / rs2'
                let rs1p = ((i >> 7) & 7) + 8; // rs1' / rd'
                let imm6 = sext(((i >> 7) & 0x20) | ((i >> 2) & 0x1F), 6);
                let next = pc.wrapping_add(2);
                match (i & 3, i >> 13) {
                    // C.ADDI4SPN (nzuimm 0 is reserved: this includes the all-zero parcel)
                    (0, 0b000) => {
                        let imm = ((i >> 7) & 0x30) | ((i >> 1) & 0x3C0) | ((i >> 4) & 0x4) | ((i >> 2) & 0x8);
                        if imm == 0 {
                            illegal!(i);
                        }
                        x[rdp as usize] = x[2].wrapping_add(imm);
                    }
                    // C.LW
                    (0, 0b010) => {
                        let imm = ((i >> 7) & 0x38) | ((i >> 4) & 0x4) | ((i << 1) & 0x40);
                        let a = x[rs1p as usize].wrapping_add(imm);
                        x[rdp as usize] = u32::from_le_bytes(load!(4, a));
                    }
                    // C.SW
                    (0, 0b110) => {
                        let imm = ((i >> 7) & 0x38) | ((i >> 4) & 0x4) | ((i << 1) & 0x40);
                        let a = x[rs1p as usize].wrapping_add(imm);
                        store!(a, x[rdp as usize].to_le_bytes());
                    }
                    // C.FLD, C.FLW, reserved, C.FSD, C.FSW
                    (0, _) => illegal!(i),
                    // C.ADDI (rd 0 or imm 0: hints)
                    (1, 0b000) => x[rd as usize] = x[rd as usize].wrapping_add(imm6),
                    // C.JAL (RV32)
                    (1, 0b001) => {
                        x[1] = next;
                        pc = pc.wrapping_add(cj_offset(i));
                        x[0] = 0;
                        continue;
                    }
                    // C.LI (rd 0: hint)
                    (1, 0b010) => x[rd as usize] = imm6,
                    // C.ADDI16SP / C.LUI (nzimm 0 reserved; C.LUI rd 0: hint)
                    (1, 0b011) => {
                        if rd == 2 {
                            let imm = ((i >> 3) & 0x200)
                                | ((i >> 2) & 0x10)
                                | ((i << 1) & 0x40)
                                | ((i << 4) & 0x180)
                                | ((i << 3) & 0x20);
                            if imm == 0 {
                                illegal!(i);
                            }
                            x[2] = x[2].wrapping_add(sext(imm, 10));
                        } else {
                            if imm6 == 0 {
                                illegal!(i);
                            }
                            x[rd as usize] = imm6 << 12;
                        }
                    }
                    // MISC-ALU
                    (1, 0b100) => {
                        let r = rs1p as usize;
                        match (i >> 10) & 3 {
                            0b00 | 0b01 => {
                                // C.SRLI / C.SRAI: shamt[5] = 1 is reserved on RV32; shamt 0 is a hint.
                                if i & 0x1000 != 0 {
                                    illegal!(i);
                                }
                                let sh = (i >> 2) & 31;
                                x[r] = if (i >> 10) & 1 == 0 { x[r] >> sh } else { ((x[r] as i32) >> sh) as u32 };
                            }
                            0b10 => x[r] &= imm6, // C.ANDI
                            _ => {
                                if i & 0x1000 != 0 {
                                    illegal!(i); // C.SUBW, C.ADDW (RV64) and reserved
                                }
                                let b = x[rdp as usize];
                                x[r] = match (i >> 5) & 3 {
                                    0 => x[r].wrapping_sub(b),
                                    1 => x[r] ^ b,
                                    2 => x[r] | b,
                                    _ => x[r] & b,
                                };
                            }
                        }
                    }
                    // C.J
                    (1, 0b101) => {
                        pc = pc.wrapping_add(cj_offset(i));
                        continue;
                    }
                    // C.BEQZ / C.BNEZ
                    (1, 0b110) | (1, 0b111) => {
                        let zero = x[rs1p as usize] == 0;
                        if zero == (i >> 13 == 0b110) {
                            let off = ((i >> 4) & 0x100)
                                | ((i >> 7) & 0x18)
                                | ((i << 1) & 0xC0)
                                | ((i >> 2) & 0x6)
                                | ((i << 3) & 0x20);
                            pc = pc.wrapping_add(sext(off, 9));
                            continue;
                        }
                    }
                    // C.SLLI (shamt[5] = 1 reserved on RV32; rd 0 or shamt 0: hints)
                    (2, 0b000) => {
                        if i & 0x1000 != 0 {
                            illegal!(i);
                        }
                        x[rd as usize] <<= (i >> 2) & 31;
                    }
                    // C.LWSP (rd 0 reserved)
                    (2, 0b010) => {
                        if rd == 0 {
                            illegal!(i);
                        }
                        let imm = ((i >> 7) & 0x20) | ((i >> 2) & 0x1C) | ((i << 4) & 0xC0);
                        let a = x[2].wrapping_add(imm);
                        x[rd as usize] = u32::from_le_bytes(load!(4, a));
                    }
                    (2, 0b100) => {
                        if i & 0x1000 == 0 {
                            if rs2 == 0 {
                                // C.JR (rs1 0 reserved)
                                if rd == 0 {
                                    illegal!(i);
                                }
                                pc = x[rd as usize] & !1;
                                continue;
                            }
                            x[rd as usize] = x[rs2 as usize]; // C.MV (rd 0: hint)
                        } else if rs2 == 0 {
                            if rd == 0 {
                                fault!(FaultKind::Ebreak, pc); // C.EBREAK
                            }
                            // C.JALR: read rs1 before writing ra.
                            let t = x[rd as usize] & !1;
                            x[1] = next;
                            pc = t;
                            continue;
                        } else {
                            x[rd as usize] = x[rd as usize].wrapping_add(x[rs2 as usize]); // C.ADD (rd 0: hint)
                        }
                    }
                    // C.SWSP
                    (2, 0b110) => {
                        let imm = ((i >> 7) & 0x3C) | ((i >> 1) & 0xC0);
                        let a = x[2].wrapping_add(imm);
                        store!(a, x[rs2 as usize].to_le_bytes());
                    }
                    // C.FLDSP, C.FLWSP, C.FSDSP, C.FSWSP
                    _ => illegal!(i),
                }
                x[0] = 0;
                pc = next;
                continue;
            }

            // ---- 32-bit ----
            let Some(hi) = bus.parcel(pc.wrapping_add(2)) else {
                fault!(FaultKind::Fetch, pc.wrapping_add(2));
            };
            let i = lo as u32 | (hi as u32) << 16;
            let rd = ((i >> 7) & 31) as usize;
            let rs1 = ((i >> 15) & 31) as usize;
            let rs2 = ((i >> 20) & 31) as usize;
            let f3 = (i >> 12) & 7;
            let f7 = i >> 25;
            let imm_i = ((i as i32) >> 20) as u32;
            let next = pc.wrapping_add(4);
            match (i >> 2) & 31 {
                // LUI
                0b01101 => x[rd] = i & 0xFFFF_F000,
                // AUIPC
                0b00101 => x[rd] = pc.wrapping_add(i & 0xFFFF_F000),
                // JAL
                0b11011 => {
                    let off = ((((i as i32) >> 31) as u32) << 20)
                        | (i & 0xF_F000)
                        | ((i >> 9) & 0x800)
                        | ((i >> 20) & 0x7FE);
                    x[rd] = next;
                    x[0] = 0;
                    pc = pc.wrapping_add(off);
                    continue;
                }
                // JALR
                0b11001 => {
                    if f3 != 0 {
                        illegal!(i);
                    }
                    let t = x[rs1].wrapping_add(imm_i) & !1;
                    x[rd] = next;
                    x[0] = 0;
                    pc = t;
                    continue;
                }
                // BRANCH
                0b11000 => {
                    let (a, b) = (x[rs1], x[rs2]);
                    let taken = match f3 {
                        0b000 => a == b,
                        0b001 => a != b,
                        0b100 => (a as i32) < (b as i32),
                        0b101 => (a as i32) >= (b as i32),
                        0b110 => a < b,
                        0b111 => a >= b,
                        _ => illegal!(i),
                    };
                    if taken {
                        let off = ((((i as i32) >> 31) as u32) << 12)
                            | ((i << 4) & 0x800)
                            | ((i >> 20) & 0x7E0)
                            | ((i >> 7) & 0x1E);
                        pc = pc.wrapping_add(off);
                        continue;
                    }
                }
                // LOAD
                0b00000 => {
                    let a = x[rs1].wrapping_add(imm_i);
                    x[rd] = match f3 {
                        0b000 => load!(1, a)[0] as i8 as u32,
                        0b001 => i16::from_le_bytes(load!(2, a)) as u32,
                        0b010 => u32::from_le_bytes(load!(4, a)),
                        0b100 => load!(1, a)[0] as u32,
                        0b101 => u16::from_le_bytes(load!(2, a)) as u32,
                        _ => illegal!(i),
                    };
                }
                // STORE
                0b01000 => {
                    let imm = (((i as i32) >> 20) as u32 & !0x1F) | ((i >> 7) & 0x1F);
                    let a = x[rs1].wrapping_add(imm);
                    let v = x[rs2];
                    match f3 {
                        0b000 => store!(a, [v as u8]),
                        0b001 => store!(a, (v as u16).to_le_bytes()),
                        0b010 => store!(a, v.to_le_bytes()),
                        _ => illegal!(i),
                    }
                }
                // OP-IMM
                0b00100 => {
                    let a = x[rs1];
                    let sh = imm_i & 31;
                    x[rd] = match f3 {
                        0b000 => a.wrapping_add(imm_i),
                        0b010 => ((a as i32) < (imm_i as i32)) as u32,
                        0b011 => (a < imm_i) as u32,
                        0b100 => a ^ imm_i,
                        0b110 => a | imm_i,
                        0b111 => a & imm_i,
                        0b001 if f7 == 0 => a << sh,
                        0b101 if f7 == 0 => a >> sh,
                        0b101 if f7 == 0b0100000 => ((a as i32) >> sh) as u32,
                        _ => illegal!(i),
                    };
                }
                // OP
                0b01100 => {
                    let (a, b) = (x[rs1], x[rs2]);
                    x[rd] = match (f7, f3) {
                        (0, 0b000) => a.wrapping_add(b),
                        (0b0100000, 0b000) => a.wrapping_sub(b),
                        (0, 0b001) => a << (b & 31),
                        (0, 0b010) => ((a as i32) < (b as i32)) as u32,
                        (0, 0b011) => (a < b) as u32,
                        (0, 0b100) => a ^ b,
                        (0, 0b101) => a >> (b & 31),
                        (0b0100000, 0b101) => ((a as i32) >> (b & 31)) as u32,
                        (0, 0b110) => a | b,
                        (0, 0b111) => a & b,
                        (1, 0b000) => a.wrapping_mul(b),
                        (1, 0b001) => ((a as i32 as i64 * b as i32 as i64) >> 32) as u32,
                        (1, 0b010) => ((a as i32 as i64 * b as u64 as i64) >> 32) as u32,
                        (1, 0b011) => ((a as u64 * b as u64) >> 32) as u32,
                        (1, 0b100) => {
                            if b == 0 {
                                u32::MAX
                            } else {
                                (a as i32).wrapping_div(b as i32) as u32
                            }
                        }
                        (1, 0b101) => a.checked_div(b).unwrap_or(u32::MAX),
                        (1, 0b110) => {
                            if b == 0 {
                                a
                            } else {
                                (a as i32).wrapping_rem(b as i32) as u32
                            }
                        }
                        (1, 0b111) => a.checked_rem(b).unwrap_or(a),
                        _ => illegal!(i),
                    };
                }
                // MISC-MEM: FENCE (and FENCE.TSO, PAUSE) is a no-op; FENCE.I is not in RV32I.
                0b00011 => {
                    if f3 != 0 {
                        illegal!(i);
                    }
                }
                // SYSTEM: only ECALL and EBREAK; no CSRs.
                0b11100 => match i {
                    0x0000_0073 => {
                        pc = next;
                        break Stop::Ecall;
                    }
                    0x0010_0073 => fault!(FaultKind::Ebreak, pc),
                    _ => illegal!(i),
                },
                _ => illegal!(i),
            }
            x[0] = 0;
            pc = next;
        };
        if matches!(stop, Stop::Fault(_)) {
            // The faulting instruction was not executed.
            left += 1;
        }
        self.pc = pc;
        self.min_sp = min_sp;
        *budget = left;
        stop
    }
}

/// The offset of C.JAL and C.J.
#[inline(always)]
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
