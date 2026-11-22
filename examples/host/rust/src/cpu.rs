use crate::decode::{Instruction, decode, expand, sext};
use std::fmt;

pub const ROM_BASE: u32 = 0x10000;
pub const RAM_BASE: u32 = 0x100000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Illegal = 1,
    Breakpoint,
    Fetch,
    Load,
    Store,
    Misaligned,
}

#[derive(Clone, Copy, Debug)]
pub struct Fault {
    pub kind: Kind,
    pub address: u32,
    pub encoding: u32,
}

impl Fault {
    fn at(kind: Kind, address: u32) -> Fault {
        Fault { kind, address, encoding: 0 }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (kind, address) = (self.kind, self.address);
        write!(f, "fault: {kind:?} at {address:08x}")
    }
}

pub struct Cpu {
    pub x: [u32; 32],
    pub pc: u32,
    pub rom: Vec<u8>,
    pub ram: Vec<u8>,
}

fn within(mem: &mut [u8], base: u32, addr: u32, size: u32)
-> Option<&mut [u8]> {
    let start = addr.checked_sub(base)? as usize;
    mem.get_mut(start..start + size as usize)
}

impl Cpu {
    pub fn memory(&mut self, addr: u32, size: u32, write: bool)
    -> Option<&mut [u8]> {
        if addr >= RAM_BASE || write {
            return within(&mut self.ram, RAM_BASE, addr, size);
        }
        within(&mut self.rom, ROM_BASE, addr, size)
    }

    fn access(&mut self, addr: u32, size: u32, write: bool)
    -> Result<&mut [u8], Fault> {
        let kind = match (addr % size, write) {
            (0, true) => Kind::Store,
            (0, false) => Kind::Load,
            _ => return Err(Fault::at(Kind::Misaligned, addr)),
        };
        let fault = Fault::at(kind, addr);
        self.memory(addr, size, write).ok_or(fault)
    }

    fn parcel(&mut self, addr: u32) -> Result<u32, Fault> {
        let fault = Fault::at(Kind::Fetch, addr);
        let bytes = self.memory(addr, 2, false).ok_or(fault)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]) as u32)
    }

    pub fn step(&mut self) -> Result<(i64, bool), Fault> {
        let mut raw = self.parcel(self.pc)?;
        let (instruction, next) = if raw & 3 == 3 {
            raw |= self.parcel(self.pc + 2)? << 16;
            (decode(raw), self.pc + 4)
        } else {
            (expand(raw), self.pc + 2)
        };
        let cost = if self.pc >= RAM_BASE { 4 } else { 1 };
        match self.execute(instruction, next) {
            Err(f) if f.kind == Kind::Illegal => Err(Fault {
                kind: Kind::Illegal,
                address: self.pc,
                encoding: raw,
            }),
            result => result.map(|ecall| (cost, ecall)),
        }
    }

    fn load(&mut self, addr: u32, f3: u32)
    -> Result<u32, Fault> {
        let size = 1 << (f3 & 3);
        let mut word = [0; 4];
        word[..size as usize]
            .copy_from_slice(self.access(addr, size, false)?);
        let value = u32::from_le_bytes(word);
        Ok(if f3 < 2 { sext(value, 8 * size) } else { value })
    }

    fn store(&mut self, addr: u32, f3: u32, value: u32)
    -> Result<(), Fault> {
        let size = 1 << f3;
        let bytes = &value.to_le_bytes()[..size as usize];
        self.access(addr, size, true)?.copy_from_slice(bytes);
        Ok(())
    }

    fn execute(&mut self, i: Instruction, mut next: u32)
    -> Result<bool, Fault> {
        let (pc, link) = (self.pc, next);
        let a = self.x[i.rs1 as usize];
        let b = self.x[i.rs2 as usize];
        let shifts = i.f3 & 3 == 1;
        let arithmetic = i.f3 == 5 && i.f7 == 32;
        let alternate = arithmetic || i.f3 == 0 && i.f7 == 32;
        let system = i.f3 | i.rd | i.rs1 == 0;
        let value = match i.op {
            0x37 => Some(i.imm),
            0x17 => Some(pc.wrapping_add(i.imm)),
            0x6f => {
                next = pc.wrapping_add(i.imm);
                Some(link)
            }
            0x67 if i.f3 == 0 => {
                next = a.wrapping_add(i.imm) & !1;
                Some(link)
            }
            0x63 if i.f3 >> 1 != 1 => {
                if branches(i.f3, a, b) {
                    next = pc.wrapping_add(i.imm);
                }
                None
            }
            0x03 if matches!(i.f3, 0 | 1 | 2 | 4 | 5) => {
                Some(self.load(a.wrapping_add(i.imm), i.f3)?)
            }
            0x23 if i.f3 <= 2 => {
                self.store(a.wrapping_add(i.imm), i.f3, b)?;
                None
            }
            0x13 if !shifts || i.f7 == 0 || arithmetic => {
                Some(alu(i.f3, arithmetic, a, i.imm))
            }
            0x33 if i.f7 == 1 => Some(muldiv(i.f3, a, b)),
            0x33 if i.f7 == 0 || alternate => {
                Some(alu(i.f3, alternate, a, b))
            }
            0x0f if i.f3 == 0 => None,
            0x73 if system && i.imm == 0 => None,
            0x73 if system && i.imm == 1 => {
                return Err(Fault::at(Kind::Breakpoint, pc));
            }
            _ => return Err(Fault::at(Kind::Illegal, pc)),
        };
        if let Some(v) = value
            && i.rd != 0
        {
            self.x[i.rd as usize] = v;
        }
        self.pc = next;
        Ok(i.op == 0x73)
    }
}

fn branches(f3: u32, a: u32, b: u32) -> bool {
    let taken = match f3 >> 1 {
        0 => a == b,
        2 => (a as i32) < (b as i32),
        _ => a < b,
    };
    taken != (f3 & 1 == 1)
}

fn alu(f3: u32, alternate: bool, a: u32, b: u32) -> u32 {
    match f3 {
        0 if alternate => a.wrapping_sub(b),
        0 => a.wrapping_add(b),
        1 => a << (b & 31),
        2 => ((a as i32) < (b as i32)) as u32,
        3 => (a < b) as u32,
        4 => a ^ b,
        5 if alternate => ((a as i32) >> (b & 31)) as u32,
        5 => a >> (b & 31),
        6 => a | b,
        _ => a & b,
    }
}

fn muldiv(f3: u32, a: u32, b: u32) -> u32 {
    let (sa, sb) = (a as i32 as i64, b as i32 as i64);
    match f3 {
        0 => a.wrapping_mul(b),
        1 => (sa * sb >> 32) as u32,
        2 => (sa * b as i64 >> 32) as u32,
        3 => (a as u64 * b as u64 >> 32) as u32,
        4 | 5 if b == 0 => u32::MAX,
        6 | 7 if b == 0 => a,
        4 => (a as i32).wrapping_div(b as i32) as u32,
        5 => a / b,
        6 => (a as i32).wrapping_rem(b as i32) as u32,
        _ => a % b,
    }
}
