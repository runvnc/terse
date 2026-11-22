//! Edge cases of the core and frame loop, by hand: x0, jump
//! targets, the end of ROM, faults, the budget, and execution from RAM.

mod common;

use common::{assemble, frame, machine};
use terse_core::cpu::{Bus, Cpu, Fault, FaultKind, RAM_BASE, RAM_INSTRUCTION_COST, ROM_BASE, Stop};
use terse_core::{BUDGET_PER_FRAME, FrameInput, Machine, Stopped, charge};

/// Ends a snippet: exit(a0).
const EXIT: &str = "    li a7, 0x106\n    ecall\n";

fn run(src: &str) -> Machine {
    let mut m = machine(&assemble(src), &[]);
    for _ in 0..10 {
        if frame(&mut m).stopped.is_some() {
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

/// SPEC §stopping: alignment is checked first; SPEC §cpu: a 48-bit or longer
/// encoding is fetched as 32 bits and is illegal with those bits, or a fetch
/// fault at its second parcel when that is outside ROM and RAM.
#[test]
fn misaligned_before_outside_and_long_encodings() {
    let lw = [0x03, 0xA5, 0x05, 0x00];
    assert_eq!(single_fault(&lw, 64, ROM_BASE, &[(11, 2)]), Fault { kind: FaultKind::Misaligned, pc: ROM_BASE, addr: 2 });
    let sw = [0x23, 0xA0, 0xA5, 0x00];
    assert_eq!(single_fault(&sw, 64, ROM_BASE, &[(11, 0x2000_0001)]).kind, FaultKind::Misaligned);
    assert_eq!(single_fault(&sw, 64, ROM_BASE, &[(11, ROM_BASE + 2)]).kind, FaultKind::Misaligned);
    // 48-bit (low bits 011111) and 64-bit (0111111) encodings.
    for (lo, enc) in [(0x1Fu8, 0x0000_001Fu32), (0x3F, 0x0000_003F), (0x7F, 0xFFFF_007F)] {
        let rom = [lo, 0x00, enc.to_le_bytes()[2], enc.to_le_bytes()[3], 0x00, 0x00];
        assert_eq!(single_fault(&rom, 64, ROM_BASE, &[]), Fault { kind: FaultKind::Illegal(enc), pc: ROM_BASE, addr: ROM_BASE });
        assert_eq!(single_fault(&rom[..2], 64, ROM_BASE, &[]), Fault { kind: FaultKind::Fetch, pc: ROM_BASE, addr: ROM_BASE + 2 });
    }
}

#[test]
fn a_fault_stops_the_machine() {
    if !common::have_tools() {
        return;
    }
    let mut m = machine(&assemble("    li a0, 3\n    li a1, 0\n    sw a0, 0(a1)\n"), &[]);
    let r = frame(&mut m);
    let fault = Fault { kind: FaultKind::Store, pc: ROM_BASE + 4, addr: 0 };
    assert_eq!(r.stopped, Some(Stopped::Fault(fault)));
    assert_eq!((m.fault(), m.exit_code()), (Some(fault), None), "a fault is not an exit");
    assert_eq!(m.fault().unwrap().to_string(), "store outside RAM at pc 00010004, address 00000000");
    assert_eq!(m.cpu().pc, ROM_BASE + 4, "pc stays at the faulting instruction");
    let r = frame(&mut m);
    assert_eq!((r.used, m.frame()), (0, 1), "no frame runs after a fault");
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

/// Copies `code_a` (a0 += 1; return) to RAM and calls it, then rewrites its
/// first word with `code_b`'s (a0 += 5) and calls it again; exits with a0.
/// `target` is the register the calls go through: t1 (RAM) or t0 (the
/// same code in ROM).
fn rewrite_program(target: &str) -> String {
    format!(
        "    li a0, 0\n    la t0, code_a\n    li t1, 0x100000\n    lw t2, 0(t0)\n    sw t2, 0(t1)\n    lw t2, 4(t0)\n\
         sw t2, 4(t1)\n    jalr ra, 0({target})\n    la t0, code_b\n    lw t2, 0(t0)\n    sw t2, 0(t1)\n\
         jalr ra, 0({target})\n    li a7, 0x106\n    ecall\n    .balign 4\n    .option push\n    .option norvc\n\
         code_a:\n    addi a0, a0, 1\n    jalr x0, 0(ra)\ncode_b:\n    addi a0, a0, 5\n    jalr x0, 0(ra)\n    .option pop\n"
    )
}

#[test]
fn code_in_ram_is_decoded_afresh_and_costs_more() {
    if !common::have_tools() {
        return;
    }
    let mut ram = machine(&assemble(&rewrite_program("t1")), &[]);
    let r_ram = frame(&mut ram);
    assert_eq!(ram.stopped(), Some(Stopped::Exit(6)), "the rewritten instruction ran");
    let mut rom = machine(&assemble(&rewrite_program("t0")), &[]);
    let r_rom = frame(&mut rom);
    assert_eq!(rom.exit_code(), Some(6));
    // Four instructions ran from RAM (two per call), each costing 4 not 1.
    assert_eq!(r_ram.used - r_rom.used, 4 * (RAM_INSTRUCTION_COST as u64 - 1));
    assert_eq!(ram.instructions() - rom.instructions(), 4 * (RAM_INSTRUCTION_COST as u64 - 1));
}

#[test]
fn ram_loop_runs_a_quarter_as_many_instructions() {
    if !common::have_tools() {
        return;
    }
    // A two-instruction loop forever, in ROM and copied to RAM; count its
    // iterations in frame 1 through t3, read after the frame.
    let src = |in_ram: bool| {
        format!(
            "    la t0, body\n    li t1, 0x100000\n    lw t2, 0(t0)\n    sw t2, 0(t1)\n    lw t2, 4(t0)\n    sw t2, 4(t1)\n\
             li a7, 0x100\n    ecall\n    jr {}\n    .balign 4\n    .option push\n    .option norvc\n\
             body:\n    addi t3, t3, 1\n    jal x0, body\n    .option pop\n",
            if in_ram { "t1" } else { "t0" }
        )
    };
    for (in_ram, per_iteration) in [(false, 2), (true, 2 * RAM_INSTRUCTION_COST)] {
        let mut m = machine(&assemble(&src(in_ram)), &[]);
        frame(&mut m);
        let before = m.cpu().x[28];
        let r = frame(&mut m);
        let iterations = (m.cpu().x[28] - before) as i64;
        assert_eq!(r.used as i64, BUDGET_PER_FRAME + (r.used as i64 - BUDGET_PER_FRAME).max(0));
        assert!((iterations - (BUDGET_PER_FRAME - 1) / per_iteration).abs() <= 1, "{in_ram}: {iterations}");
    }
}

/// A loop that runs over several frames' budgets.
const LOOP: &str = "    li t0, 0\n    li t1, 150000\n1:  addi t0, t0, 1\n    bne t0, t1, 1b\n    mv a0, x0\n    li a7, 0x106\n    ecall\n";

#[test]
fn budget_runs_out_mid_loop_and_resumes_exactly() {
    if !common::have_tools() {
        return;
    }
    let rom = assemble(LOOP);
    let mut m = machine(&rom, &[]);
    let r0 = frame(&mut m);
    assert_eq!((r0.used, r0.waited, r0.stopped), (BUDGET_PER_FRAME as u64, false, None));
    assert_eq!(m.instructions(), 125_000);
    let r1 = frame(&mut m);
    assert_eq!(r1.used, 125_000);
    let r2 = frame(&mut m);
    assert_eq!(r2.stopped, Some(Stopped::Exit(0)));
    // li, li (lui + addi), then 2 per iteration, then mv, li, ecall.
    let total = 3 + 2 * 150_000 + 3;
    assert_eq!(m.instructions(), total);
    assert_eq!(r2.used, total - 250_000 + charge::CALL as u64);

    // The core gives the same state however the budget is cut up.
    let mut ram = vec![0u8; 64];
    let mut whole = Cpu::new(ROM_BASE, RAM_BASE + 64);
    let mut b = 250_000;
    whole.run(&mut Bus { rom: &rom, ram: &mut ram }, &mut b);
    let mut pieces = Cpu::new(ROM_BASE, RAM_BASE + 64);
    let mut used = 0;
    for n in (1..).map(|i| i % 13 + 1) {
        let mut b = n.min(250_000 - used);
        used += b;
        assert_eq!(pieces.run(&mut Bus { rom: &rom, ram: &mut ram }, &mut b), Stop::Budget);
        if used == 250_000 {
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
    // Frame 0: wait. Frame 1: a terminal write of 4,000 bytes (8,000 units
    // of charge), then wait. Then exit with the frame number.
    let src = "    li a7, 0x100\n    ecall\n    mv s0, a0\n    li a0, 0x100000\n    li a1, 4000\n    li a7, 0x200\n    ecall\n\
               li a7, 0x100\n    ecall\n    add a0, a0, s0\n    li a7, 0x106\n    ecall\n";
    let mut m = machine(&assemble(src), &[common::TERMINAL]);
    let call = charge::CALL as u64;
    let r = frame(&mut m);
    assert!(r.waited);
    assert_eq!(r.used, 2 + call, "li + ecall + the call");
    let r = frame(&mut m);
    assert!(r.waited);
    // mv, li (lui), li (lui+addi), li, ecall + call + 4,000, li, ecall +
    // call; 4,000 zero bytes print nothing but are still charged.
    assert_eq!(r.used, 1 + 1 + 2 + 1 + 1 + call + 2 * 4000 + 1 + 1 + call);
    let r = frame(&mut m);
    // wait_frame returned 1, then 2.
    assert_eq!(r.stopped, Some(Stopped::Exit(3)));
}

#[test]
fn a_charge_beyond_the_budget_carries_into_the_next_frame() {
    if !common::have_tools() {
        return;
    }
    // Frame 1: 123,800 units of loop, then a 2,400-cell write (2,400 units
    // of copy): over budget. Frame 2: a counting loop gets what is left.
    let src = "    li a7, 0x100\n    ecall\n    lui t0, 15\n    addi t0, t0, 460\n1:  addi t0, t0, -1\n    bnez t0, 1b\n\
               li a0, 0\n    li a1, 0x100000\n    li a2, 2400\n    li a7, 0x207\n    ecall\n\
               2:  addi t3, t3, 1\n    j 2b\n";
    let mut m = machine(&assemble(src), &[common::TERMINAL]);
    frame(&mut m);
    let r = frame(&mut m);
    let carry = m.budget();
    assert!(carry < -1000 && carry > -1300, "{carry} {r:?}");
    assert_eq!(r.used as i64, BUDGET_PER_FRAME - carry);
    let before = m.cpu().x[28];
    let r = frame(&mut m);
    assert_eq!(r.used as i64, BUDGET_PER_FRAME + carry);
    let iterations = (m.cpu().x[28] - before) as i64;
    assert!((iterations * 2 - (BUDGET_PER_FRAME + carry)).abs() <= 2, "{iterations}");
}

#[test]
fn write_takes_the_whole_buffer_and_charges_carry() {
    if !common::have_tools() {
        return;
    }
    // Writes of 4,096 bytes of RAM (zeros) again and again: each takes the
    // whole buffer (SPEC §terminal.calls) and costs 4 + 2 per byte, so a frame ends
    // with the budget at most one write below zero, which carries.
    let src = "    li s0, 0x100000\n    lui s1, 1\n    li s2, 0\n\
               1:  mv a0, s0\n    mv a1, s1\n    li a7, 0x200\n    ecall\n    add s2, s2, a0\n    j 1b\n";
    let mut m = Machine::new(&common::program(&assemble(src), 32, &[common::TERMINAL]));
    let per = 5 + charge::CALL as i64 + 2 * 4096;
    let mut start = BUDGET_PER_FRAME;
    for _ in 0..5 {
        let r = frame(&mut m);
        assert_eq!(m.cpu().x[10], 4096, "the length back");
        assert!(m.budget() <= 0 && m.budget() > -per, "carry {}", m.budget());
        assert_eq!(r.used as i64, start - m.budget());
        start = BUDGET_PER_FRAME + m.budget();
    }
    assert_eq!(m.cpu().x[18] % 4096, 0);
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

/// SPEC §calls.convention: a buffer of length 0 is valid at any address, for every call
/// (address 0, all ones, just past the end of ROM, far past the end of
/// RAM); a non-empty one is checked as before, and so are a call's other
/// arguments. Vector and page submits reject a length of 0 with 1 by their
/// own rule, wherever it points.
#[test]
fn empty_buffers_are_valid_anywhere() {
    if !common::have_tools() {
        return;
    }
    let past_ram = RAM_BASE + 4 * 8192 + 1000;
    let mut src = String::from("    li s0, 0x100000\n");
    let calls: [(u32, [u32; 3]); 14] = [
        (0x200, [0, 0, 0]),                  // write
        (0x200, [u32::MAX, 0, 0]),
        (0x200, [past_ram, 0, 0]),
        (0x104, [0, 0, 0]),                  // random
        (0x206, [0, 0, 0]),                  // cells_read
        (0x207, [5, u32::MAX, 0]),           // cells_write
        (0x208, [3, past_ram, 0]),           // palette_set
        (0x206, [2400, 0, 0]),               // first cell out of range: still bad
        (0x104, [0, 257, 0]),                // longer than 256: still bad
        (0x200, [0, 1, 0]),                  // one byte at 0: bad
        (0x300, [0, 0, 0]),                  // vector submit, length 0: rejected
        (0x300, [RAM_BASE, 0, 0]),
        (0x700, [u32::MAX, 0, 0]),           // page submit, length 0: rejected
        (0x200, [0x7FFF_0000, 0, 0]),
    ];
    for (i, (n, a)) in calls.iter().enumerate() {
        src += &format!(
            "    li a0, {}\n    li a1, {}\n    li a2, {}\n    li a7, {n}\n    ecall\n    sw a0, {}(s0)\n",
            a[0] as i32,
            a[1] as i32,
            a[2] as i32,
            4 * i
        );
    }
    // write(end of ROM, 0): the label is the first address past the ROM.
    src += "    la a0, rom_end\n    li a1, 0\n    li a7, 0x200\n    ecall\n    sw a0, 56(s0)\n";
    src += &format!("    li a0, 0\n{EXIT}rom_end:\n");
    let mut m = machine(&assemble(&src), &[common::TERMINAL, common::VECTOR, common::PAGE]);
    for _ in 0..3 {
        frame(&mut m);
    }
    assert_eq!(m.exit_code(), Some(0), "{:?}", m.fault());
    let got: Vec<u32> = m.ram()[..60].chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let bad = terse_core::ERR_BUFFER;
    assert_eq!(got, [0, 0, 0, 0, 0, 0, 0, bad, bad, bad, 1, 1, 1, 0, 0]);
}

/// SPEC §frames step 1, 4 `wait_frame`: the frame's input events are queued
/// before the content-done events of its completions; and `wait_frame`
/// always returns in the next frame with that frame's counter, since the
/// most content one frame can deliver (8 × 16,384 bytes, 32,768 units)
/// leaves budget to run.
#[test]
fn input_events_before_completions_and_wait_frame_returns_next_frame() {
    if !common::have_tools() {
        return;
    }
    let src = "    li s0, 0x100000\n    li s2, 0x101000\n    li s3, 8\n    li t2, 16384\n\
               1:  li t1, 1\n    sb t1, 0(s0)\n    sw t2, 40(s0)\n    sw s2, 44(s0)\n    mv a0, s0\n    li a7, 0x400\n    ecall\n\
               addi s0, s0, 64\n    add s2, s2, t2\n    addi s3, s3, -1\n    bnez s3, 1b\n\
               li a7, 0x100\n    ecall\n    mv s1, a0\n    li a7, 0x102\n    ecall\n    sub s1, a0, s1\n\
               li a0, 0x100800\n    li a7, 0x101\n    ecall\n    li a0, 0x100804\n    li a7, 0x101\n    ecall\n\
               mv a0, s1\n    li a7, 0x106\n    ecall\n";
    let mut m = Machine::new(&common::program(&assemble(src), 32, &[common::CONTENT]));
    let r = frame(&mut m);
    assert!(r.waited);
    assert_eq!(m.content().unwrap().pending_handles(), [1, 2, 3, 4, 5, 6, 7, 8]);
    let completions: Vec<terse_core::Completion> = (1..=8)
        .map(|handle| terse_core::Completion { handle, outcome: terse_core::Outcome::Got { size: 16384, data: vec![handle; 16384] } })
        .collect();
    let events = [terse_core::Event::key_down(0x04, 0)];
    let r = m.run_frame(&FrameInput { events: &events, completions: &completions, ..Default::default() });
    assert_eq!(r.completions_ignored, 0);
    assert_eq!(m.exit_code(), Some(0), "wait_frame returned the counter of the frame it returned in: {:?}", m.fault());
    assert_eq!(m.ram()[0x800..0x808], [0x01, 0x04, 0, 0, 0x05, 1, 0x01, 0]);
    assert!(r.used > 8 * 4096);
}

/// SPEC §terminal.calls: a `write` longer than 4,096 bytes returns FFFF_FFFEh and
/// writes nothing (one of 4,096 is taken whole). `palette_set` with `first` above 255 or `count` above 256 returns
/// FFFF_FFFEh (SPEC §terminal.calls).
#[test]
fn write_budget_and_palette_set_arguments() {
    if !common::have_tools() {
        return;
    }
    let src = "    lui a0, 0x100\n    lui a1, 1\n    addi a1, a1, 1\n    li a7, 0x200\n    ecall\n    mv s1, a0\n\
               li a7, 0x100\n    ecall\n    lui s0, 0x100\n    sw s1, 0(s0)\n\
               li a0, 256\n    mv a1, s0\n    li a2, 1\n    li a7, 0x208\n    ecall\n    sw a0, 4(s0)\n\
               li a0, 0\n    mv a1, s0\n    li a2, 257\n    ecall\n    sw a0, 8(s0)\n\
               li a0, 255\n    mv a1, s0\n    li a2, 256\n    ecall\n    sw a0, 12(s0)\n\
               li a0, 0\n    li a7, 0x106\n    ecall\n";
    let mut m = Machine::new(&common::program(&assemble(src), 32, &[common::TERMINAL]));
    for _ in 0..4 {
        frame(&mut m);
    }
    assert_eq!(m.exit_code(), Some(0), "{:?}", m.fault());
    let w: Vec<u32> = m.ram()[..16].chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    assert_eq!(w, [terse_core::ERR_BUFFER, terse_core::ERR_BUFFER, terse_core::ERR_BUFFER, 256]);
    assert_eq!(m.terminal().unwrap().cursor(), (0, 0), "nothing written");
}

#[test]
fn reset_state() {
    let m = machine(&[0x01, 0x00], &[]);
    let x = &m.cpu().x;
    assert_eq!(m.cpu().pc, ROM_BASE);
    assert_eq!(x[2], RAM_BASE + 4 * 8192, "sp at the end of RAM");
    assert_eq!((m.budget(), m.stopped(), m.frame()), (0, None, 0));
    assert!(x.iter().enumerate().all(|(i, &v)| i == 2 || v == 0));
    assert!(m.ram().iter().all(|&b| b == 0));
    let _ = FrameInput::default();
}


/// SPEC §memory and §reset at the largest image: 262,144 bytes of ROM
/// (its last byte at 0004_FFFFh) and 96 pages of RAM (its last byte at
/// 001B_FFFFh). `sp` starts at 001C_0000h; the last byte of each region
/// can be read (and RAM's written); the byte after either faults.
#[test]
fn the_largest_rom_and_ram() {
    let words: [u32; 6] = [
        0x05a0_0513, // li a0, 0x5a
        0xfea1_0fa3, // sb a0, -1(sp): RAM's last byte
        0xfff1_4583, // lbu a1, -1(sp)
        0x0005_0637, // lui a2, 0x50: the end of the largest ROM
        0xfff6_4683, // lbu a3, -1(a2): ROM's last byte
        0, // replaced below by the access past the end
    ];
    let run = |last: u32| {
        let mut rom = vec![0u8; terse_core::image::ROM_MAX];
        for (i, w) in words.iter().enumerate() {
            rom[i * 4..i * 4 + 4].copy_from_slice(&if i == 5 { last } else { *w }.to_le_bytes());
        }
        *rom.last_mut().unwrap() = 0x77;
        let image = common::program(&rom, terse_core::image::RAM_PAGES_MAX, &[]);
        let mut m = Machine::new(&image);
        assert_eq!(m.cpu().x[2], 0x001C_0000, "sp at the end of 96 pages of RAM");
        frame(&mut m);
        assert_eq!((m.cpu().x[11], m.cpu().x[13]), (0x5a, 0x77), "the last bytes of RAM and ROM");
        m.fault().expect("the access past the end faults")
    };
    let at = ROM_BASE + 20;
    // sb a0, 0(sp); lbu a4, 0(sp); lbu a4, 0(a2).
    assert_eq!(run(0x00a1_0023), Fault { kind: FaultKind::Store, pc: at, addr: 0x001C_0000 });
    assert_eq!(run(0x0001_4703), Fault { kind: FaultKind::Load, pc: at, addr: 0x001C_0000 });
    assert_eq!(run(0x0006_4703), Fault { kind: FaultKind::Load, pc: at, addr: 0x0005_0000 });
}
