//! Instruction semantics against llvm-mc as the encoder: thousands of
//! random instances of every RV32I, M and C instruction form are written as
//! assembly text, encoded by llvm-mc, and each is executed for one
//! instruction from a random register file and RAM. The expected state is
//! computed here from the instruction as written (operands as numbers, not
//! decoded bits), so a decoding mistake in the core shows up as a mismatch.

mod common;

use std::fmt::Write as _;
use std::process::Command;

use terse_rv::cpu::{Bus, Code, Cpu, FaultKind, RAM_BASE, ROM_BASE, Stop};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
    /// An interesting 32-bit value.
    fn value(&mut self) -> u32 {
        const SPECIAL: [u32; 10] = [0, 1, 2, 0xFFFF_FFFF, 0x8000_0000, 0x7FFF_FFFF, 0xFFFF_FFFE, 0x8000_0001, 31, 32];
        match self.below(4) {
            0 => SPECIAL[self.below(10) as usize],
            1 => self.next() & 0xFF,
            _ => self.next(),
        }
    }
    /// A signed immediate of `bits` bits.
    fn simm(&mut self, bits: u32) -> i32 {
        let v = self.next() & ((1 << bits) - 1);
        ((v << (32 - bits)) as i32) >> (32 - bits)
    }
}

const RAM_LEN: usize = 0x10000;
const ROM_LEN: usize = 0x2000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    x: [u32; 32],
    pc: u32,
    ram: Vec<u8>,
}

impl State {
    fn set(&mut self, rd: u32, v: u32) {
        if rd != 0 {
            self.x[rd as usize] = v;
        }
    }
    fn r(&self, r: u32) -> u32 {
        self.x[r as usize]
    }
    fn load(&self, a: u32, n: usize) -> u32 {
        let o = (a - RAM_BASE) as usize;
        let mut b = [0u8; 4];
        b[..n].copy_from_slice(&self.ram[o..o + n]);
        u32::from_le_bytes(b)
    }
    fn store(&mut self, a: u32, n: usize, v: u32) {
        let o = (a - RAM_BASE) as usize;
        self.ram[o..o + n].copy_from_slice(&v.to_le_bytes()[..n]);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Step,
    Ecall,
    Fault(FaultKind),
}

struct Case {
    asm: String,
    compressed: bool,
    init: State,
    want: State,
    outcome: Outcome,
}

/// x8-x15, the registers of the compressed 3-bit fields.
fn creg(rng: &mut Rng) -> u32 {
    8 + rng.below(8)
}

fn reg(rng: &mut Rng) -> u32 {
    rng.below(32)
}

fn nonzero_reg(rng: &mut Rng) -> u32 {
    1 + rng.below(31)
}

/// A RAM address aligned to `n` where an access of `n` bytes fits.
fn ram_addr(rng: &mut Rng, n: u32) -> u32 {
    RAM_BASE + (rng.below(RAM_LEN as u32 - 4) & !(n - 1))
}

fn sext(v: u32, bits: u32) -> u32 {
    (((v << (32 - bits)) as i32) >> (32 - bits)) as u32
}

fn gen_case(rng: &mut Rng, kind: usize) -> Case {
    let mut init = State { x: [0; 32], pc: ROM_BASE + 0x800 + 2 * rng.below(0x400), ram: Vec::new() };
    for i in 1..32 {
        init.x[i] = rng.value();
    }
    init.ram = (0..RAM_LEN).map(|_| rng.next() as u8).collect();
    let mut s = init.clone();
    let mut outcome = Outcome::Step;
    let mut compressed = false;
    let pc = s.pc;
    let asm: String;

    // 32-bit forms first, then compressed ones.
    const ALU: [&str; 10] = ["add", "sub", "sll", "slt", "sltu", "xor", "srl", "sra", "or", "and"];
    const MUL: [&str; 8] = ["mul", "mulh", "mulhsu", "mulhu", "div", "divu", "rem", "remu"];
    const IMM: [&str; 6] = ["addi", "slti", "sltiu", "xori", "ori", "andi"];
    const SHI: [&str; 3] = ["slli", "srli", "srai"];
    const BR: [&str; 6] = ["beq", "bne", "blt", "bge", "bltu", "bgeu"];
    const LD: [(&str, u32); 5] = [("lb", 1), ("lh", 2), ("lw", 4), ("lbu", 1), ("lhu", 2)];
    const ST: [(&str, u32); 3] = [("sb", 1), ("sh", 2), ("sw", 4)];

    let alu = |op: &str, a: u32, b: u32| -> u32 {
        match op {
            "add" | "addi" => a.wrapping_add(b),
            "sub" => a.wrapping_sub(b),
            "sll" | "slli" => a << (b & 31),
            "slt" | "slti" => ((a as i32) < (b as i32)) as u32,
            "sltu" | "sltiu" => (a < b) as u32,
            "xor" | "xori" => a ^ b,
            "srl" | "srli" => a >> (b & 31),
            "sra" | "srai" => ((a as i32) >> (b & 31)) as u32,
            "or" | "ori" => a | b,
            "and" | "andi" => a & b,
            "mul" => a.wrapping_mul(b),
            "mulh" => ((a as i32 as i128 * b as i32 as i128) >> 32) as u32,
            "mulhsu" => ((a as i32 as i128 * b as i128) >> 32) as u32,
            "mulhu" => ((a as u128 * b as u128) >> 32) as u32,
            "div" => match (a as i32, b as i32) {
                (_, 0) => u32::MAX,
                (i32::MIN, -1) => a,
                (x, y) => (x / y) as u32,
            },
            "divu" => if b == 0 { u32::MAX } else { a / b },
            "rem" => match (a as i32, b as i32) {
                (_, 0) => a,
                (i32::MIN, -1) => 0,
                (x, y) => (x % y) as u32,
            },
            "remu" => if b == 0 { a } else { a % b },
            _ => unreachable!("{op}"),
        }
    };

    match kind {
        0 => {
            let op = ALU[rng.below(10) as usize];
            let (rd, rs1, rs2) = (reg(rng), reg(rng), reg(rng));
            asm = format!("{op} x{rd}, x{rs1}, x{rs2}");
            s.set(rd, alu(op, s.r(rs1), s.r(rs2)));
        }
        1 => {
            let op = MUL[rng.below(8) as usize];
            let (rd, rs1, rs2) = (reg(rng), reg(rng), reg(rng));
            // Make the division special cases common.
            match rng.below(4) {
                0 => s.x[rs2 as usize] = 0,
                1 => {
                    s.x[rs1 as usize] = 0x8000_0000;
                    s.x[rs2 as usize] = u32::MAX;
                }
                _ => {}
            }
            s.x[0] = 0;
            init.x = s.x;
            asm = format!("{op} x{rd}, x{rs1}, x{rs2}");
            s.set(rd, alu(op, s.r(rs1), s.r(rs2)));
        }
        2 => {
            let op = IMM[rng.below(6) as usize];
            let (rd, rs1, imm) = (reg(rng), reg(rng), rng.simm(12));
            asm = format!("{op} x{rd}, x{rs1}, {imm}");
            s.set(rd, alu(op, s.r(rs1), imm as u32));
        }
        3 => {
            let op = SHI[rng.below(3) as usize];
            let (rd, rs1, sh) = (reg(rng), reg(rng), rng.below(32));
            asm = format!("{op} x{rd}, x{rs1}, {sh}");
            s.set(rd, alu(op, s.r(rs1), sh));
        }
        4 => {
            let (rd, imm) = (reg(rng), rng.below(1 << 20));
            if rng.below(2) == 0 {
                asm = format!("lui x{rd}, {imm}");
                s.set(rd, imm << 12);
            } else {
                asm = format!("auipc x{rd}, {imm}");
                s.set(rd, pc.wrapping_add(imm << 12));
            }
        }
        5 => {
            let (rd, off) = (reg(rng), rng.simm(21) & !1);
            asm = format!("jal x{rd}, {off}");
            s.set(rd, pc + 4);
            s.pc = pc.wrapping_add(off as u32);
            return Case { asm, compressed, init, want: s, outcome };
        }
        6 => {
            let (rd, rs1, imm) = (reg(rng), reg(rng), rng.simm(12));
            asm = format!("jalr x{rd}, {imm}(x{rs1})");
            let t = s.r(rs1).wrapping_add(imm as u32) & !1;
            s.set(rd, pc + 4);
            s.pc = t;
            return Case { asm, compressed, init, want: s, outcome };
        }
        7 => {
            let op = BR[rng.below(6) as usize];
            let (rs1, rs2, off) = (reg(rng), reg(rng), rng.simm(13) & !1);
            if rng.below(3) == 0 {
                s.x[rs2 as usize] = s.x[rs1 as usize];
                s.x[0] = 0;
                init.x = s.x;
            }
            asm = format!("{op} x{rs1}, x{rs2}, {off}");
            let (a, b) = (s.r(rs1), s.r(rs2));
            let taken = match op {
                "beq" => a == b,
                "bne" => a != b,
                "blt" => (a as i32) < (b as i32),
                "bge" => (a as i32) >= (b as i32),
                "bltu" => a < b,
                _ => a >= b,
            };
            s.pc = if taken { pc.wrapping_add(off as u32) } else { pc + 4 };
            return Case { asm, compressed, init, want: s, outcome };
        }
        8 => {
            let (op, n) = LD[rng.below(5) as usize];
            let (rd, rs1, imm) = (reg(rng), nonzero_reg(rng), rng.simm(12));
            let a = ram_addr(rng, n);
            s.x[rs1 as usize] = a.wrapping_sub(imm as u32);
            init.x = s.x;
            asm = format!("{op} x{rd}, {imm}(x{rs1})");
            let v = s.load(a, n as usize);
            s.set(rd, if op.ends_with('u') || n == 4 { v } else { sext(v, 8 * n) });
        }
        9 => {
            let (op, n) = ST[rng.below(3) as usize];
            let (rs2, rs1, imm) = (reg(rng), nonzero_reg(rng), rng.simm(12));
            let a = ram_addr(rng, n);
            s.x[rs1 as usize] = a.wrapping_sub(imm as u32);
            init.x = s.x;
            asm = format!("{op} x{rs2}, {imm}(x{rs1})");
            let v = s.r(rs2);
            s.store(a, n as usize, v);
        }
        10 => {
            if rng.below(2) == 0 {
                asm = "fence".into();
            } else {
                asm = "ecall".into();
                outcome = Outcome::Ecall;
            }
        }
        // ---- Compressed ----
        11 => {
            compressed = true;
            let (rd, imm) = (creg(rng), 4 * (1 + rng.below(255)));
            asm = format!("c.addi4spn x{rd}, x2, {imm}");
            s.set(rd, s.r(2).wrapping_add(imm));
        }
        12 => {
            compressed = true;
            let (r, base, imm) = (creg(rng), creg(rng), 4 * rng.below(32));
            let a = ram_addr(rng, 4);
            s.x[base as usize] = a.wrapping_sub(imm);
            init.x = s.x;
            if rng.below(2) == 0 {
                asm = format!("c.lw x{r}, {imm}(x{base})");
                let v = s.load(a, 4);
                s.set(r, v);
            } else {
                asm = format!("c.sw x{r}, {imm}(x{base})");
                let v = s.r(r);
                s.store(a, 4, v);
            }
        }
        13 => {
            compressed = true;
            let rd = nonzero_reg(rng);
            let imm = loop {
                let i = rng.simm(6);
                if i != 0 {
                    break i;
                }
            };
            match rng.below(3) {
                0 => {
                    asm = format!("c.addi x{rd}, {imm}");
                    s.set(rd, s.r(rd).wrapping_add(imm as u32));
                }
                1 => {
                    asm = format!("c.li x{rd}, {imm}");
                    s.set(rd, imm as u32);
                }
                _ => {
                    asm = "c.nop".into();
                }
            }
        }
        14 => {
            compressed = true;
            let off = rng.simm(12) & !1;
            if rng.below(2) == 0 {
                asm = format!("c.jal {off}");
                s.set(1, pc + 2);
            } else {
                asm = format!("c.j {off}");
            }
            s.pc = pc.wrapping_add(off as u32);
            return Case { asm, compressed, init, want: s, outcome };
        }
        15 => {
            compressed = true;
            if rng.below(2) == 0 {
                let imm = loop {
                    let i = rng.simm(6) * 16;
                    if i != 0 {
                        break i;
                    }
                };
                asm = format!("c.addi16sp x2, {imm}");
                s.set(2, s.r(2).wrapping_add(imm as u32));
            } else {
                let rd = loop {
                    let r = nonzero_reg(rng);
                    if r != 2 {
                        break r;
                    }
                };
                let imm = loop {
                    let i = rng.simm(6);
                    if i != 0 {
                        break i;
                    }
                };
                // llvm-mc wants the 20-bit field: 1-31 or FFFE0h-FFFFFh.
                let field = imm as u32 & 0xF_FFFF;
                asm = format!("c.lui x{rd}, {field}");
                s.set(rd, (imm as u32) << 12);
            }
        }
        16 => {
            compressed = true;
            let rd = creg(rng);
            match rng.below(3) {
                0 => {
                    let sh = 1 + rng.below(31);
                    asm = format!("c.srli x{rd}, {sh}");
                    s.set(rd, s.r(rd) >> sh);
                }
                1 => {
                    let sh = 1 + rng.below(31);
                    asm = format!("c.srai x{rd}, {sh}");
                    s.set(rd, ((s.r(rd) as i32) >> sh) as u32);
                }
                _ => {
                    let imm = rng.simm(6);
                    asm = format!("c.andi x{rd}, {imm}");
                    s.set(rd, s.r(rd) & imm as u32);
                }
            }
        }
        17 => {
            compressed = true;
            let op = ["c.sub", "c.xor", "c.or", "c.and"][rng.below(4) as usize];
            let (rd, rs2) = (creg(rng), creg(rng));
            asm = format!("{op} x{rd}, x{rs2}");
            let (a, b) = (s.r(rd), s.r(rs2));
            s.set(rd, alu(&op[2..], a, b));
        }
        18 => {
            compressed = true;
            let (rs1, off) = (creg(rng), rng.simm(9) & !1);
            if rng.below(2) == 0 {
                s.x[rs1 as usize] = 0;
                init.x = s.x;
            }
            let zero = s.r(rs1) == 0;
            let beqz = rng.below(2) == 0;
            asm = format!("{} x{rs1}, {off}", if beqz { "c.beqz" } else { "c.bnez" });
            s.pc = if zero == beqz { pc.wrapping_add(off as u32) } else { pc + 2 };
            return Case { asm, compressed, init, want: s, outcome };
        }
        19 => {
            compressed = true;
            let (rd, sh) = (nonzero_reg(rng), 1 + rng.below(31));
            asm = format!("c.slli x{rd}, {sh}");
            s.set(rd, s.r(rd) << sh);
        }
        20 => {
            compressed = true;
            let (r, imm) = (reg(rng), 4 * rng.below(64));
            let a = ram_addr(rng, 4);
            s.x[2] = a.wrapping_sub(imm);
            init.x = s.x;
            if r != 0 && rng.below(2) == 0 {
                asm = format!("c.lwsp x{r}, {imm}(x2)");
                let v = s.load(a, 4);
                s.set(r, v);
            } else {
                asm = format!("c.swsp x{r}, {imm}(x2)");
                let v = s.r(r);
                s.store(a, 4, v);
            }
        }
        21 => {
            compressed = true;
            let rs1 = nonzero_reg(rng);
            let t = s.r(rs1) & !1;
            if rng.below(2) == 0 {
                asm = format!("c.jr x{rs1}");
            } else {
                asm = format!("c.jalr x{rs1}");
                s.set(1, pc + 2);
            }
            s.pc = t;
            return Case { asm, compressed, init, want: s, outcome };
        }
        22 => {
            compressed = true;
            let (rd, rs2) = (nonzero_reg(rng), nonzero_reg(rng));
            if rng.below(2) == 0 {
                asm = format!("c.mv x{rd}, x{rs2}");
                s.set(rd, s.r(rs2));
            } else {
                asm = format!("c.add x{rd}, x{rs2}");
                s.set(rd, s.r(rd).wrapping_add(s.r(rs2)));
            }
        }
        23 => {
            compressed = true;
            asm = "c.ebreak".into();
            outcome = Outcome::Fault(FaultKind::Ebreak);
            return Case { asm, compressed, init: init.clone(), want: init, outcome };
        }
        _ => unreachable!(),
    }
    s.pc = pc + if compressed { 2 } else { 4 };
    if outcome == Outcome::Ecall {
        s.pc = pc + 4;
    }
    Case { asm, compressed, init, want: s, outcome }
}

const KINDS: usize = 24;

/// Encodes each line with llvm-mc; returns the bytes of each.
fn encode(lines: &[String], compressed: bool) -> Vec<Vec<u8>> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cases.s");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let attrs = if compressed { "+m,+c" } else { "+m" };
    let out = Command::new("llvm-mc")
        .args(["-triple=riscv32", &format!("-mattr={attrs}"), "--show-encoding"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    let enc: Vec<Vec<u8>> = text
        .lines()
        .filter_map(|l| l.split("encoding: [").nth(1))
        .map(|e| {
            e.trim_end_matches(']').split(',').map(|b| u8::from_str_radix(b.trim_start_matches("0x"), 16).unwrap()).collect()
        })
        .collect();
    assert_eq!(enc.len(), lines.len(), "llvm-mc output:\n{text}");
    enc
}

#[test]
fn every_form_against_llvm_mc() {
    if !common::have_tools() {
        return;
    }
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let per_kind = 400;
    let cases: Vec<Case> = (0..KINDS * per_kind).map(|i| gen_case(&mut rng, i % KINDS)).collect();
    let (wide, narrow): (Vec<usize>, Vec<usize>) = (0..cases.len()).partition(|&i| !cases[i].compressed);
    let mut encodings = vec![Vec::new(); cases.len()];
    for (set, c) in [(&wide, false), (&narrow, true)] {
        let lines: Vec<String> = set.iter().map(|&i| cases[i].asm.clone()).collect();
        for (&i, e) in set.iter().zip(encode(&lines, c)) {
            encodings[i] = e;
        }
    }
    let mut failures = String::new();
    let mut count = 0;
    for (case, enc) in cases.iter().zip(&encodings) {
        assert_eq!(enc.len(), if case.compressed { 2 } else { 4 }, "{}: {enc:x?}", case.asm);
        let mut rom = vec![0u8; ROM_LEN];
        let at = (case.init.pc - ROM_BASE) as usize;
        rom[at..at + enc.len()].copy_from_slice(enc);
        let mut ram = case.init.ram.clone();
        let mut cpu = Cpu::new(case.init.pc, 0);
        cpu.x = case.init.x;
        let mut budget = 1;
        let stop = cpu.run(&mut Bus { rom: &rom, ram: &mut ram }, &mut budget);
        let outcome = match stop {
            Stop::Budget => Outcome::Step,
            Stop::Ecall => Outcome::Ecall,
            Stop::Fault(f) => Outcome::Fault(f.kind),
        };
        let got = State { x: cpu.x, pc: cpu.pc, ram };
        // The same from decoded ROM.
        let mut ram2 = case.init.ram.clone();
        let mut cached = Cpu::new(case.init.pc, 0);
        cached.x = case.init.x;
        let mut budget2 = 1;
        let stop2 = cached.run_cached(&Code::new(&rom), &mut Bus { rom: &rom, ram: &mut ram2 }, &mut budget2);
        assert_eq!((stop2, &cached, &ram2, budget2), (stop, &cpu, &got.ram, budget), "{}: cached", case.asm);
        if outcome != case.outcome || got != case.want {
            let regs: Vec<String> = (0..32)
                .filter(|&r| got.x[r] != case.want.x[r])
                .map(|r| format!("x{r}: got {:08x} want {:08x}", got.x[r], case.want.x[r]))
                .collect();
            let _ = writeln!(
                failures,
                "{} [{enc:02x?}]: outcome {outcome:?} want {:?}; pc {:08x} want {:08x}; {}; ram {}",
                case.asm,
                case.outcome,
                got.pc,
                case.want.pc,
                regs.join(", "),
                if got.ram == case.want.ram { "ok" } else { "differs" }
            );
        }
        count += 1;
    }
    assert!(failures.is_empty(), "{failures}");
    eprintln!("{count} instructions checked");
}
