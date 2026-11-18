//! Edge cases of the profile-2 core and frame loop, by hand: x0, jump
//! targets, the end of ROM, faults, the budget, and execution from RAM.

mod common;

use common::{assemble, frame, machine};
use terse_core::FrameInput;
use terse_rv::cpu::{Bus, Cpu, Fault, FaultKind, RAM_BASE, ROM_BASE, Stop};
use terse_rv::{INSTRUCTIONS_PER_FRAME, RvMachine};

/// Ends a snippet: exit(a0).
const EXIT: &str = "    li a7, 0x106\n    ecall\n";

fn run(src: &str) -> RvMachine {
    let mut m = machine(&assemble(src), &[]);
    for _ in 0..10 {
        if frame(&mut m).exit.is_some() {
            break;
        }
    }
    m
}

#[test]
fn writes_to_x0_are_discarded() {
    if !common::have_tools() {
        return;
    }
    let m = run(&format!(
        "    li x0, 5\n    lui x0, 0x12345\n    addi x0, x0, 1\n    c.addi x0, 7\n    c.mv x0, sp\n    c.li x0, 3\n\
         lw x0, -4(sp)\n    jal x0, 1f\n1:  auipc x0, 1\n    mul x0, sp, sp\n    mv a0, x0\n{EXIT}"
    ));
    assert_eq!(m.cpu().x[0], 0);
    assert_eq!(m.exit_code(), Some(0));
    assert!(m.fault().is_none());
}

#[test]
fn jalr_clears_the_low_bit_and_c_allows_two_byte_targets() {
    if !common::have_tools() {
        return;
    }
    // `target` is 2 mod 4 (after a 2-byte c.nop), and the register holds
    // target + 1: the low bit is dropped and the 2-byte alignment is legal.
    let m = run(&format!(
        "    la t0, target\n    addi t0, t0, 1\n    li a0, 9\n    jalr ra, 0(t0)\n    li a0, 1\n{EXIT}\
         .balign 4\n    c.nop\ntarget:\n    andi t1, t0, 3\n    addi a0, t1, 40\n{EXIT}"
    ));
    assert_eq!(m.exit_code(), Some(43), "{:?}", m.fault());
    // The same through c.jr and c.jalr.
    let m = run(&format!(
        "    la t0, target\n    addi t0, t0, 1\n    c.jalr t0\n    li a0, 1\n{EXIT}\
         .balign 4\n    c.nop\ntarget:\n    li a0, 44\n{EXIT}"
    ));
    assert_eq!(m.exit_code(), Some(44));
}

fn single_fault(rom: &[u8], ram_len: usize, pc: u32, set: &[(usize, u32)]) -> Fault {
    let mut ram = vec![0u8; ram_len];
    let mut cpu = Cpu::new(pc, RAM_BASE + ram_len as u32);
    for &(r, v) in set {
        cpu.x[r] = v;
    }
    let mut budget = 10;
    match cpu.run(&mut Bus { rom, ram: &mut ram }, &mut budget) {
        Stop::Fault(f) => {
            assert_eq!(cpu.pc, f.pc, "pc stays at the faulting instruction");
            if f.pc == pc {
                assert_eq!(budget, 10, "the faulting instruction is not counted");
            }
            f
        }
        s => panic!("{s:?}"),
    }
}

#[test]
fn instruction_straddling_the_end_of_rom() {
    // c.nop, then the first half of `addi x0, x0, 0` as the last two bytes.
    let rom = [0x01, 0x00, 0x13, 0x00];
    let f = single_fault(&rom, 64, ROM_BASE, &[]);
    assert_eq!(f, Fault { kind: FaultKind::Fetch, pc: ROM_BASE + 2, addr: ROM_BASE + 4 });
    // With the whole instruction it runs, then the fetch after ROM faults.
    let rom = [0x01, 0x00, 0x13, 0x00, 0x00, 0x00];
    let f = single_fault(&rom, 64, ROM_BASE, &[]);
    assert_eq!(f, Fault { kind: FaultKind::Fetch, pc: ROM_BASE + 6, addr: ROM_BASE + 6 });
    // An odd-length ROM: the last byte alone is not an instruction.
    let rom = [0x01, 0x00, 0x01];
    assert_eq!(single_fault(&rom, 64, ROM_BASE, &[]).kind, FaultKind::Fetch);
}

#[test]
fn faults() {
    // lw a0, 0(a1) with a1 = 0: a null dereference.
    let lw = [0x03, 0xA5, 0x05, 0x00];
    let f = single_fault(&lw, 64, ROM_BASE, &[(11, 0)]);
    assert_eq!(f, Fault { kind: FaultKind::Load, pc: ROM_BASE, addr: 0 });
    // Misaligned, in RAM.
    let f = single_fault(&lw, 64, ROM_BASE, &[(11, RAM_BASE + 2)]);
    assert_eq!(f.kind, FaultKind::Misaligned);
    // A load past the end of RAM, and one that runs over it.
    assert_eq!(single_fault(&lw, 64, ROM_BASE, &[(11, RAM_BASE + 64)]).kind, FaultKind::Load);
    // Loads from ROM are fine: lw a0, 0(a1) reads its own encoding, then
    // the fetch past the end faults.
    let f = single_fault(&lw, 64, ROM_BASE, &[(11, ROM_BASE)]);
    assert_eq!(f.kind, FaultKind::Fetch);
    // sw a0, 0(a1): to ROM, to nowhere, misaligned.
    let sw = [0x23, 0xA0, 0xA5, 0x00];
    assert_eq!(single_fault(&sw, 64, ROM_BASE, &[(11, ROM_BASE)]), Fault { kind: FaultKind::Store, pc: ROM_BASE, addr: ROM_BASE });
    assert_eq!(single_fault(&sw, 64, ROM_BASE, &[(11, 0x2000_0000)]).kind, FaultKind::Store);
    assert_eq!(single_fault(&sw, 64, ROM_BASE, &[(11, RAM_BASE + 1)]).kind, FaultKind::Misaligned);
    // sh a0, 1(a1) at an odd address; lh likewise.
    assert_eq!(single_fault(&[0xA3, 0x90, 0xA5, 0x00], 64, ROM_BASE, &[(11, RAM_BASE)]).kind, FaultKind::Misaligned);
    assert_eq!(single_fault(&[0x03, 0x95, 0x15, 0x00], 64, ROM_BASE, &[(11, RAM_BASE)]).kind, FaultKind::Misaligned);
    // ebreak, c.ebreak, c.unimp (0000h), a CSR read (rdcycle).
    assert_eq!(single_fault(&[0x73, 0x00, 0x10, 0x00], 64, ROM_BASE, &[]).kind, FaultKind::Ebreak);
    assert_eq!(single_fault(&[0x02, 0x90], 64, ROM_BASE, &[]).kind, FaultKind::Ebreak);
    assert_eq!(single_fault(&[0x00, 0x00], 64, ROM_BASE, &[]).kind, FaultKind::Illegal(0));
    assert_eq!(single_fault(&[0x73, 0x25, 0x00, 0xC0], 64, ROM_BASE, &[]).kind, FaultKind::Illegal(0xC000_2573));
    // Jumps to nowhere fault at the fetch: jalr x0, 0(a1).
    let jr = [0x67, 0x80, 0x05, 0x00];
    assert_eq!(single_fault(&jr, 64, ROM_BASE, &[(11, 0)]), Fault { kind: FaultKind::Fetch, pc: 0, addr: 0 });
    assert_eq!(single_fault(&jr, 64, ROM_BASE, &[(11, ROM_BASE + 4)]).kind, FaultKind::Fetch);
    assert_eq!(single_fault(&jr, 64, ROM_BASE, &[(11, RAM_BASE + 64)]).kind, FaultKind::Fetch);
}

#[test]
fn a_fault_stops_the_machine() {
    if !common::have_tools() {
        return;
    }
    let mut m = machine(&assemble("    li a0, 3\n    li a1, 0\n    sw a0, 0(a1)\n"), &[]);
    let r = frame(&mut m);
    assert_eq!(r.exit, Some(terse_rv::machine::FAULT_EXIT));
    assert_eq!(m.fault(), Some(Fault { kind: FaultKind::Store, pc: ROM_BASE + 4, addr: 0 }));
    assert_eq!(m.fault().unwrap().to_string(), "store outside RAM at pc 00010004, address 00000000");
    let r = frame(&mut m);
    assert_eq!((r.tstates, m.frame()), (0, 1), "no frame runs after a fault");
}

#[test]
fn code_runs_from_ram() {
    if !common::have_tools() {
        return;
    }
    // Copy `li a0, 77; li a7, 0x106; ecall` (as words) into RAM and jump there.
    let m = run(
        "    la t0, code\n    li t1, 0x100000\n    lw t2, 0(t0)\n    sw t2, 0(t1)\n    lw t2, 4(t0)\n    sw t2, 4(t1)\n\
         lw t2, 8(t0)\n    sw t2, 8(t1)\n    jr t1\n    .balign 4\ncode:\n    .word 0x04d00513\n    .word 0x10600893\n\
         .word 0x00000073\n",
    );
    assert_eq!(m.exit_code(), Some(77), "{:?}", m.fault());
}

/// A loop that runs over several frames' budgets.
const LOOP: &str = "    li t0, 0\n    li t1, 600000\n1:  addi t0, t0, 1\n    bne t0, t1, 1b\n    mv a0, x0\n    li a7, 0x106\n    ecall\n";

#[test]
fn budget_runs_out_mid_loop_and_resumes_exactly() {
    if !common::have_tools() {
        return;
    }
    let rom = assemble(LOOP);
    let mut m = machine(&rom, &[]);
    let r0 = frame(&mut m);
    assert_eq!((r0.tstates, r0.halted_early, r0.exit), (INSTRUCTIONS_PER_FRAME as u64, false, None));
    assert_eq!(m.instructions(), 500_000);
    let r1 = frame(&mut m);
    assert_eq!(r1.tstates, 500_000);
    let r2 = frame(&mut m);
    assert_eq!(r2.exit, Some(0));
    // li, li (lui + addi), then 2 per iteration, then mv, li, ecall.
    let total = 3 + 2 * 600_000 + 3;
    assert_eq!(m.instructions(), total);
    assert_eq!(r2.tstates, total - 1_000_000);

    // The core gives the same state however the budget is cut up.
    let mut ram = vec![0u8; 64];
    let mut whole = Cpu::new(ROM_BASE, RAM_BASE + 64);
    let mut b = 1_000_000;
    whole.run(&mut Bus { rom: &rom, ram: &mut ram }, &mut b);
    let mut pieces = Cpu::new(ROM_BASE, RAM_BASE + 64);
    let mut used = 0;
    for n in (1..).map(|i| i % 13 + 1) {
        let mut b = n.min(1_000_000 - used);
        used += b;
        assert_eq!(pieces.run(&mut Bus { rom: &rom, ram: &mut ram }, &mut b), Stop::Budget);
        if used == 1_000_000 {
            break;
        }
    }
    assert_eq!(pieces, whole);
}

#[test]
fn wait_frame_discards_the_rest_and_device_charges_carry() {
    if !common::have_tools() {
        return;
    }
    // Frame 0: wait. Frame 1: a terminal write of 4,000 bytes (1,000
    // instructions of charge), then wait. Then exit with the frame number.
    let src = "    li a7, 0x100\n    ecall\n    mv s0, a0\n    li a0, 0x100000\n    li a1, 4000\n    li a7, 0x200\n    ecall\n\
               li a7, 0x100\n    ecall\n    add a0, a0, s0\n    li a7, 0x106\n    ecall\n";
    let mut m = machine(&assemble(src), &[common::TERMINAL]);
    let r = frame(&mut m);
    assert!(r.halted_early);
    assert_eq!(r.tstates, 2, "li + ecall");
    let r = frame(&mut m);
    assert!(r.halted_early);
    // mv, li (lui+addi), li, li, ecall + 1,000 charge, li, ecall; 4,000
    // zero bytes print nothing but are still charged.
    // (li a0, 0x100000 is one lui; li a1, 4000 is lui + addi.)
    assert_eq!(r.tstates, 1 + 1 + 2 + 1 + 1 + 1000 + 1 + 1);
    let r = frame(&mut m);
    // wait_frame returned 1, then 2.
    assert_eq!(r.exit, Some(3));
}

#[test]
fn calls_clobber_only_a0_and_a1() {
    if !common::have_tools() {
        return;
    }
    // Fill every register, make an unknown call and a two-result call, sum
    // the registers that must be unchanged.
    let mut src = String::new();
    for r in 3..32 {
        if r != 10 && r != 11 && r != 17 {
            src += &format!("    li x{r}, {}\n", r * 1000);
        }
    }
    src += "    li a7, 0x999\n    ecall\n    mv s0, a0\n    li a7, 0x203\n    ecall\n    li a0, 0\n";
    for r in 3..32 {
        if ![8, 10, 11, 17].contains(&r) {
            src += &format!("    li t6, {}\n    bne x{r}, t6, bad\n", r * 1000);
        }
    }
    src += "    li t6, -1\n    bne s0, t6, bad\n    li a0, 1\nbad:\n    li a7, 0x106\n    ecall\n";
    // t6 is x31: compare it last, by value.
    let src = src.replace("    li t6, 31000\n    bne x31, t6, bad\n", "");
    let m = run(&src);
    assert_eq!(m.exit_code(), Some(1), "{:?}", m.fault());
}

#[test]
fn reset_state() {
    let m = machine(&[0x01, 0x00], &[]);
    let x = &m.cpu().x;
    assert_eq!(m.cpu().pc, ROM_BASE);
    assert_eq!(x[2], RAM_BASE + 4 * 8192, "sp at the end of RAM");
    assert!(x.iter().enumerate().all(|(i, &v)| i == 2 || v == 0));
    assert!(m.ram().iter().all(|&b| b == 0));
    let _ = FrameInput::default();
}
