//! Which encodings are instructions, against llvm-mc's disassembler
//! (`-mattr=+m,+c`, so without F, D, A): every one of the 49,152 16-bit
//! encodings, and a large sample of 32-bit words biased towards the major
//! opcodes. The core must fault with `Illegal` exactly where LLVM finds no
//! instruction, apart from the differences listed (and explained) below.

mod common;

use std::collections::BTreeMap;
use std::process::Command;

use terse_core::cpu::{Bus, Cpu, FaultKind, RAM_BASE, ROM_BASE, Stop};

/// Disassembles each encoding; returns LLVM's text, or None if invalid.
fn llvm(encodings: &[Vec<u8>], attrs: &str) -> Vec<Option<String>> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("enc.txt");
    let text: String = encodings
        .iter()
        .map(|e| e.iter().map(|b| format!("0x{b:02x}")).collect::<Vec<_>>().join(" ") + "\n")
        .collect();
    std::fs::write(&path, text).unwrap();
    let out = Command::new("llvm-mc")
        .args(["--disassemble", "-triple=riscv32", &format!("-mattr={attrs}"), "--show-encoding"])
        .arg(&path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    // "enc.txt:LINE:COL: warning: invalid instruction encoding"
    let mut invalid = std::collections::HashSet::new();
    for l in stderr.lines().filter(|l| l.contains("invalid instruction encoding")) {
        let line: usize = l.split(':').nth(1).unwrap().parse().unwrap();
        invalid.insert(line - 1);
    }
    let stdout = String::from_utf8(out.stdout).unwrap();
    let mut valid = stdout.lines().filter(|l| l.contains("encoding:"));
    (0..encodings.len())
        .map(|i| {
            if invalid.contains(&i) {
                None
            } else {
                let l = valid.next().expect("one line per valid encoding");
                Some(l.split('#').next().unwrap().split_whitespace().collect::<Vec<_>>().join(" "))
            }
        })
        .collect()
}

/// Whether the core decodes `enc` as an instruction (anything but an
/// Illegal fault when executed once).
fn decodes(enc: &[u8]) -> bool {
    let mut rom = vec![0u8; 64];
    rom[..enc.len()].copy_from_slice(enc);
    let mut ram = vec![0u8; 0x1000];
    let mut cpu = Cpu::new(ROM_BASE, RAM_BASE + 0x800);
    for r in 1..32 {
        cpu.x[r] = RAM_BASE + 0x800;
    }
    let mut budget = 1;
    !matches!(cpu.run(&mut Bus { rom: &rom, ram: &mut ram }, &mut budget), Stop::Fault(f) if matches!(f.kind, FaultKind::Illegal(_)))
}

#[test]
fn all_compressed_encodings() {
    if !common::have_tools() {
        return;
    }
    let encs: Vec<Vec<u8>> = (0..=0xFFFFu32).filter(|i| i & 3 != 3).map(|i| (i as u16).to_le_bytes().to_vec()).collect();
    let theirs = llvm(&encs, "+m,+c");
    // Differences by LLVM mnemonic, with counts.
    let mut diffs: BTreeMap<String, usize> = BTreeMap::new();
    for (e, t) in encs.iter().zip(&theirs) {
        let ours = decodes(e);
        if ours != t.is_some() {
            let what = match t {
                Some(text) => format!("llvm decodes, core faults: {}", text.split(' ').next().unwrap()),
                None => format!("core decodes, llvm does not: {:04x}", u16::from_le_bytes([e[0], e[1]])),
            };
            *diffs.entry(what).or_default() += 1;
        }
    }
    // c.unimp (0000h) is LLVM's `unimp`, which the spec defines to be an
    // illegal instruction; it must fault.
    let allowed: BTreeMap<String, usize> = [("llvm decodes, core faults: unimp".to_string(), 1)].into();
    assert_eq!(diffs, allowed, "differences from llvm-mc");
}

#[test]
fn sampled_32_bit_encodings() {
    if !common::have_tools() {
        return;
    }
    let mut s: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as u32
    };
    // Every major opcode (bits 6-2) equally often, the other bits random;
    // and funct7 often one of the defined values.
    let mut encs = Vec::new();
    for n in 0..200_000u32 {
        let mut w = next() & !0x7F | (n % 32) << 2 | 3;
        if (n % 32) & 7 == 7 {
            // Bits 4-2 = 111: the prefixes of 48-bit and longer encodings,
            // which llvm-mc reads across lines. None is an instruction here.
            assert!(!decodes(&w.to_le_bytes()), "{w:08x}");
            continue;
        }
        match next() % 4 {
            0 => w &= 0x01FF_FFFF,               // funct7 0
            1 => w = w & 0x01FF_FFFF | 1 << 25,   // funct7 1 (M)
            2 => w = w & 0x01FF_FFFF | 0x20 << 25, // funct7 0100000
            _ => {}
        }
        encs.push(w.to_le_bytes().to_vec());
    }
    // A few exact system instructions.
    for w in [0x0000_0073u32, 0x0010_0073, 0x1050_0073, 0x3020_0073, 0xC000_1073, 0x0000_100F, 0x0FF0_000F, 0x8330_000F] {
        encs.push(w.to_le_bytes().to_vec());
    }
    let theirs = llvm(&encs, "+m");
    let mut diffs: BTreeMap<String, usize> = BTreeMap::new();
    for (e, t) in encs.iter().zip(&theirs) {
        let ours = decodes(e);
        if ours != t.is_some() {
            let w = u32::from_le_bytes(e[..4].try_into().unwrap());
            let what = match t {
                Some(text) => text.split(' ').next().unwrap().to_string(),
                // FENCE with an empty predecessor or successor set, or with
                // rd/rs1/fm bits LLVM does not accept: the spec makes these
                // hints (rd and rs1 "ignored"), so they run as no-ops.
                None if w & 0x707F == 0x000F => "fence (hint forms)".to_string(),
                None => format!("core decodes, llvm does not: {w:08x}"),
            };
            *diffs.entry(what).or_default() += 1;
        }
    }
    // Profile 2 has no CSRs, no privileged instructions and no FENCE.I: LLVM
    // decodes those (Zicsr, Zifencei and the privileged spec are on by
    // default), the core faults. Nothing else may differ.
    let unsupported = |m: &str| {
        m == "fence (hint forms)"
            || m.starts_with("csr")
            || m.starts_with("rd")
            || matches!(m, "unimp" | "fence.i" | "wfi" | "mret" | "sret" | "dret" | "sfence.vma" | "hfence.vvma" | "hfence.gvma")
            || m.starts_with("frcsr")
            || m.starts_with("fscsr")
            || m.starts_with("frrm")
            || m.starts_with("fsrm")
            || m.starts_with("frflags")
            || m.starts_with("fsflags")
            || m.starts_with("hlv")
            || m.starts_with("hsv")
            || m.starts_with("sinval")
            || m.starts_with("sfence")
            || m.starts_with("hinval")
    };
    let unexpected: BTreeMap<&String, &usize> = diffs.iter().filter(|(m, _)| !unsupported(m)).collect();
    assert!(unexpected.is_empty(), "differences from llvm-mc: {unexpected:?} (all: {diffs:?})");
    eprintln!("system/CSR encodings LLVM decodes and the core rejects: {diffs:?}");
}
