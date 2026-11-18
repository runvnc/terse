//! Machine-level tests: reset state, frame loop (SPEC 1.3), interrupts, ports.

mod common;

use common::*;
use terse_core::{Event, FrameInput, Image, Machine, TSTATES_PER_FRAME};

#[test]
fn reset_state() {
    let m = machine("halt\n");
    let r = m.registers();
    assert_eq!((r.pc, r.sp, r.af, r.bc, r.de, r.hl, r.ix, r.iy), (0, 0, 0, 0, 0, 0, 0, 0));
    assert_eq!((r.af_alt, r.bc_alt, r.de_alt, r.hl_alt, r.i, r.r), (0, 0, 0, 0, 0, 0));
    assert_eq!((r.iff1, r.iff2, r.im, r.halted), (false, false, 0, false));
    assert_eq!(m.slots(), &[0, 1, 2, 3, 0x80, 0x81, 0x82, 0x83]);
    assert!(m.ram().iter().all(|&b| b == 0));
    assert_eq!(m.frame(), 0);
}

#[test]
fn negative_remainder_carries() {
    // RST 38h at 38h calls itself forever: 11 T-states per step.
    let mut m = machine("rst 38h\n org 38h\n rst 38h\n");
    let r = frame(&mut m);
    // ceil(300000 / 11) = 27273 steps = 300003 T-states.
    assert_eq!(r.tstates, 300_003);
    assert_eq!(m.budget(), -3);
    assert!(!r.halted_early);
    let r = frame(&mut m);
    // Budget 299997: 27273 steps again.
    assert_eq!(r.tstates, 300_003);
    assert_eq!(m.budget(), -6);
}

#[test]
fn halt_discards_rest_of_frame() {
    // IFF1 is 0 at reset, so nothing can wake the CPU.
    let mut m = machine("nop\n halt\n");
    let r = frame(&mut m);
    assert!(r.halted_early);
    assert_eq!(r.tstates, 8);
    assert_eq!(m.budget(), 0, "unused budget does not carry");
    assert!(m.registers().halted);
    let r = frame(&mut m);
    assert!(r.halted_early);
    assert_eq!(r.tstates, 0);
    assert_eq!(m.frame(), 2);
}

const COUNTER_IM1: &str = "
        ld sp,0
        im 1
        ei
loop    halt
        jr loop
        org 38h
        push hl
        ld hl,8000h
        inc (hl)
        pop hl
        ei
        reti
";

#[test]
fn im1_interrupt_once_per_frame() {
    let mut m = machine(COUNTER_IM1);
    for n in 1..=5u8 {
        let r = frame(&mut m);
        assert!(r.halted_early);
        assert_eq!(m.peek(0x8000), n);
        assert!(!m.interrupt_requested());
    }
    // ld sp(10) im1(8) ei(4) halt(4) ack(13) push(11) ld(10) inc(11) pop(10) ei(4) reti(14) jr(12) halt(4)
    let mut m = machine(COUNTER_IM1);
    assert_eq!(frame(&mut m).tstates, 10 + 8 + 4 + 4 + 13 + 11 + 10 + 11 + 10 + 4 + 14 + 12 + 4);
}

#[test]
fn interrupt_request_is_a_latch() {
    // Interrupts stay disabled until frame 3; the request from frames 0-3 is
    // accepted once.
    let mut m = machine(
        "
        ld sp,0
        im 1
wait    in a,(14h)
        cp 3
        jr nz,wait
        ei
loop    halt
        jr loop
        org 38h
        ld hl,8000h
        inc (hl)
        ei
        reti
",
    );
    for _ in 0..3 {
        let r = frame(&mut m);
        assert!(!r.halted_early);
        assert!(m.interrupt_requested());
        assert_eq!(m.peek(0x8000), 0);
    }
    frame(&mut m);
    assert_eq!(m.peek(0x8000), 1);
    frame(&mut m);
    assert_eq!(m.peek(0x8000), 2);
}

#[test]
fn im2_vector_read_at_i_ff() {
    let mut m = machine(
        "
        ld sp,0
        ld a,1
        ld i,a
        im 2
        ei
loop    halt
        jr loop
        org 1FFh
        dw handler
handler ld hl,8000h
        inc (hl)
        ei
        reti
",
    );
    frame(&mut m);
    frame(&mut m);
    assert_eq!(m.peek(0x8000), 2);
}

#[test]
fn interrupt_acknowledge_timings() {
    // Each handler ends in DI; HALT, so the frame stops right after it.
    let im0 = machine("ei\n halt\n org 38h\n di\n halt\n");
    let im1 = machine("im 1\n ei\n halt\n org 38h\n di\n halt\n");
    let im2 = machine("ld a,1\n ld i,a\n im 2\n ei\n halt\n org 1FFh\n dw 300h\n org 300h\n di\n halt\n");
    let cases = [(im0, 4 + 4 + 13 + 8), (im1, 8 + 4 + 4 + 13 + 8), (im2, 7 + 9 + 8 + 4 + 4 + 19 + 8)];
    for (mut m, want) in cases {
        let r = frame(&mut m);
        assert!(r.halted_early);
        assert_eq!(r.tstates, want);
    }
}

#[test]
fn halted_cpu_is_woken_and_resumes_after_halt() {
    let mut m = machine(COUNTER_IM1);
    frame(&mut m);
    let r = m.registers();
    assert!(r.halted);
    // rustzx-z80 keeps PC on the HALT opcode while halted.
    assert_eq!(m.peek(r.pc), 0x76);
}

#[test]
fn terminal_charges_count_against_budget() {
    let mut m = machine("ld a,1\n out (25h),a\n out (25h),a\n halt\n");
    // ld(7) + 2 * (out(11) + clear(600)) + halt(4)
    assert_eq!(frame(&mut m).tstates, 7 + 2 * (11 + 600) + 4);
}

#[test]
fn charges_carry_into_next_frame() {
    // Spin until the last instruction of the frame is a 611 T-state clear.
    let mut m = machine("ld a,1\nloop out (25h),a\n jr loop\n");
    let r = frame(&mut m);
    // ld (7), then out (11 + 600) and jr (12) alternately while the budget is positive.
    let steps = std::iter::once(7).chain([611, 12].into_iter().cycle());
    let mut budget = TSTATES_PER_FRAME;
    for t in steps {
        if budget <= 0 {
            break;
        }
        budget -= t;
    }
    assert_eq!(r.tstates as i64, TSTATES_PER_FRAME - budget);
    assert_eq!(m.budget(), budget);
    assert!(budget < 0);
}

#[test]
fn exit_port_stops_the_machine() {
    let mut m = machine("ld a,42\n out (1Fh),a\n jr $\n");
    let r = frame(&mut m);
    assert_eq!(r.exit, Some(42));
    assert_eq!(r.tstates, 7 + 11);
    let pc = m.registers().pc;
    let r = frame(&mut m);
    assert_eq!((r.exit, r.tstates), (Some(42), 0));
    assert_eq!(m.registers().pc, pc);
}

#[test]
fn absent_terminal_reads_ff() {
    let mut m = machine_with(&asm("in a,(2Bh)\n ld (8000h),a\n ld a,1\n out (25h),a\n halt\n"), &[]);
    assert!(m.terminal().is_none());
    let r = frame(&mut m);
    assert_eq!(m.peek(0x8000), 0xFF);
    assert_eq!(r.tstates, 11 + 13 + 7 + 11 + 4, "no charge from an absent device");
    // With the terminal present the same read gives 80 columns.
    let mut m = machine("in a,(2Bh)\n ld (8000h),a\n halt\n");
    frame(&mut m);
    assert_eq!(m.peek(0x8000), 80);
}

#[test]
fn reserved_and_free_ports_read_ff() {
    let mut m = machine(
        "
        ld hl,8000h
        in a,(08h)
        ld (hl),a
        inc hl
        in a,(1Dh)
        ld (hl),a
        inc hl
        in a,(30h)
        ld (hl),a
        inc hl
        in a,(0F0h)
        ld (hl),a
        halt
",
    );
    frame(&mut m);
    assert_eq!([m.peek(0x8000), m.peek(0x8001), m.peek(0x8002), m.peek(0x8003)], [0xFF; 4]);
}

#[test]
fn upper_port_byte_is_ignored() {
    let mut m = machine("ld bc,0AB2Bh\n in a,(c)\n ld (8000h),a\n halt\n");
    frame(&mut m);
    assert_eq!(m.peek(0x8000), 80);
}

#[test]
fn paging_through_ports() {
    let mut m = machine(
        "
        ld a,83h        ; RAM page 3 into slot 1
        out (01h),a
        ld a,99
        ld (2000h),a
        in a,(01h)
        ld (8000h),a
        ld a,10h        ; unmapped
        out (01h),a
        ld a,(2000h)
        ld (8001h),a
        halt
",
    );
    frame(&mut m);
    assert_eq!(m.ram()[3 * 0x2000], 99);
    assert_eq!(m.peek(0xE000), 99, "slot 7 shows the same page");
    assert_eq!(m.peek(0x8000), 0x83);
    assert_eq!(m.peek(0x8001), 0xFF);
    assert_eq!(m.slots()[1], 0x10);
}

#[test]
fn system_ports_from_the_cpu() {
    let mut m = machine(
        "
        ld hl,8000h
        in a,(10h)
        ld (hl),a
        inc hl
        in a,(11h)
        ld (hl),a
        inc hl
        out (10h),a     ; remove head
        in a,(10h)
        ld (hl),a
        inc hl
        in a,(18h)
        ld (hl),a
        inc hl
        in a,(1Bh)
        ld (hl),a
        inc hl
        in a,(1Ch)
        ld (hl),a
        inc hl
        in a,(1Ch)
        ld (hl),a
        halt
",
    );
    let events = [Event::key_down(0x29, 0), Event::text('x')];
    let r = m.run_frame(&FrameInput { events: &events, unix_time: 0x6000_0001, random: &[7, 9, 11], completions: &[] });
    assert_eq!(r.random_used, 2);
    let got: Vec<u8> = (0..7).map(|i| m.peek(0x8000 + i)).collect();
    assert_eq!(got, [1, 0x29, 3, 0x01, 0x60, 7, 9]);
}

#[test]
fn event_overflow_is_reported() {
    let mut m = machine("di\n halt\n");
    let events = vec![Event::key_down(4, 0); 70];
    let r = m.run_frame(&FrameInput { events: &events, ..Default::default() });
    assert_eq!(r.events_dropped, 6);
    assert_eq!(m.system().queue().len(), 64);
}

/// Standard Z80 T-states for a selection of instructions (Zilog Z80 CPU
/// User Manual). Each program is followed by HALT (4 T-states); interrupts
/// are disabled at reset, so the frame ends there.
#[test]
fn instruction_timings() {
    let cases: &[(&str, u64)] = &[
        ("nop", 4),
        ("ld a,1", 7),
        ("ld hl,8000h", 10),
        ("ld (hl),1", 10),
        ("inc (hl)", 11),
        ("inc bc", 6),
        ("ld sp,hl", 6),
        ("ex de,hl", 4),
        ("exx", 4),
        ("push bc", 11),
        ("pop bc", 10),
        ("ex (sp),hl", 19),
        ("call next\nnext", 17),
        ("jp next\nnext", 10),
        ("jr nz,next\nnext", 12),
        ("jr z,next\nnext", 7),
        ("djnz next\nnext", 13),
        ("ret z", 5),
        ("in a,(0)", 11),
        ("out (0FFh),a", 11),
        ("in a,(c)", 12),
        ("ld bc,00FFh\n out (c),a", 10 + 12),
        ("ld bc,00FFh\n ini", 10 + 16),
        ("ld bc,00FFh\n outi", 10 + 16),
        ("ld bc,1\n ldir", 10 + 16),
        ("ld bc,2\n ldir", 10 + 21 + 16),
        ("ld bc,1\n cpir", 10 + 16),
        ("add ix,bc", 15),
        ("ld (ix+0),1", 19),
        ("ld a,(iy+5)", 19),
        ("bit 0,(ix+0)", 20),
        ("set 0,(ix+0)", 23),
        ("rlc b", 8),
        ("bit 0,(hl)", 12),
        ("rlc (hl)", 15),
        ("neg", 8),
        ("ld a,i", 9),
        ("im 1", 8),
        ("ld (8000h),hl", 16),
        ("ld (8000h),bc", 20),
        ("ld hl,(8000h)", 16),
        ("rrd", 18),
        ("adc hl,de", 15),
        ("ld a,ixh", 8),
        ("daa", 4),
    ];
    for &(src, want) in cases {
        let mut m = machine(&format!(" {src}\n halt\n"));
        let r = frame(&mut m);
        assert!(r.halted_early, "{src}");
        assert_eq!(r.tstates, want + 4, "{src}");
    }
}

#[test]
fn determinism() {
    let src = "
        ld sp,0
        ld hl,0A000h
        ld (9000h),hl
        im 1
        ei
loop    halt
        jr loop
        org 38h
        in a,(1Ch)
        ld hl,(9000h)
        ld (hl),a
        inc hl
        ld (9000h),hl
        in a,(10h)
        out (10h),a
        out (20h),a
        ei
        reti
";
    let run = || {
        let mut m = machine(src);
        for f in 0..20u8 {
            let events = [Event::text(char::from(b'a' + f)), Event::key_down(f, 1)];
            let random = [f.wrapping_mul(37), 1, 2];
            m.run_frame(&FrameInput { events: &events, unix_time: 1_700_000_000 + f as u32, random: &random, completions: &[] });
        }
        m
    };
    let (a, b) = (run(), run());
    assert_eq!(a.registers(), b.registers());
    assert_eq!(a.ram(), b.ram());
    assert_eq!(a.slots(), b.slots());
    assert_eq!(a.terminal(), b.terminal());
    assert_eq!(a.budget(), b.budget());
    assert_eq!(a.system().queue().collect::<Vec<_>>(), b.system().queue().collect::<Vec<_>>());
    assert!(a.ram().iter().any(|&x| x != 0));
}

#[test]
fn image_parse_feeds_machine() {
    let file = Image::build("t", 4, &[], &[0x76]);
    let img = Image::parse(&file).unwrap();
    let mut m = Machine::new(&img);
    assert!(frame(&mut m).halted_early);
    assert_eq!(m.image_id(), &img.id);
}
