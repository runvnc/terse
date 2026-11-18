//! Device 3 through the CPU and the frame loop (SPEC 8.2, 8.4).

mod common;

use common::*;
use terse_core::vector::{Primitive, decode};
use terse_core::{DeviceReq, Machine};

const VECTOR: DeviceReq = DeviceReq { slot: 3, version: 0 };

/// A list at CPU 8000h (linear 0) of `len` bytes, submitted; port 35h is
/// read into A and stored at 9000h, the T-states of the submitting OUT are
/// measured against a NOP-padded twin by the caller.
fn program(list: &[u8], len: u16, command: u8) -> String {
    let bytes: Vec<String> = list.iter().map(|b| b.to_string()).collect();
    format!(
        "
        ld hl,list
        ld de,8000h
        ld bc,{n}
        ldir
        xor a
        out (30h),a
        out (31h),a
        out (32h),a
        ld a,{lo}
        out (33h),a
        ld a,{hi}
        out (34h),a
        ld a,{command}
        out (35h),a
        in a,(35h)
        ld (9000h),a
        in a,(30h)
        ld (9001h),a
        halt
list:   db {bytes}
",
        n = list.len().max(1),
        lo = len & 0xFF,
        hi = len >> 8,
        bytes = if bytes.is_empty() { "0".into() } else { bytes.join(",") },
    )
}

const LIST: [u8; 10] = [0x04, 0x10, 0, 0x20, 0, 0x05, 0x30, 0, 0x40, 0];

fn run(src: &str, devices: &[DeviceReq]) -> (Machine, u64) {
    let mut m = machine_with(&asm(src), devices);
    let t = frame(&mut m).tstates;
    (m, t)
}

#[test]
fn submit_through_the_cpu_and_charge() {
    let (m, t) = run(&program(&LIST, 10, 1), &[TERMINAL, VECTOR]);
    assert_eq!(m.peek(0x9000), 0x00, "done");
    assert_eq!(m.peek(0x9001), 0xFF, "port 30h is write-only");
    assert_eq!(m.vector_list(), LIST);
    assert_eq!(
        decode(m.vector_list()).collect::<Vec<_>>(),
        [Primitive::Line { x0: 16, y0: 32, x1: 48, y1: 64, width: 16, rgba: [255; 4] }]
    );
    // The same program with a rejected length differs only by the charge.
    let (m2, t2) = run(&program(&LIST, 0, 1), &[TERMINAL, VECTOR]);
    assert_eq!(m2.peek(0x9000), 0x01, "rejected");
    assert!(m2.vector_list().is_empty());
    assert_eq!(t - t2, 10, "1 T-state per byte copied");
    assert_ne!(m.vector_generation(), m2.vector_generation());
}

#[test]
fn absent_device_reads_ffh_and_draws_nothing() {
    let (m, _) = run(&program(&LIST, 10, 1), &[TERMINAL]);
    assert!(m.vector().is_none());
    assert_eq!(m.peek(0x9000), 0xFF);
    assert!(m.vector_list().is_empty());
    assert_eq!(m.vector_generation(), 0);
}

#[test]
fn rejected_range_past_ram() {
    // 4 RAM pages: linear 8000h is past the end.
    let src = "
        xor a
        out (30h),a
        ld a,80h
        out (31h),a
        xor a
        out (32h),a
        ld a,1
        out (33h),a
        xor a
        out (34h),a
        ld a,1
        out (35h),a
        in a,(35h)
        ld (9000h),a
        halt
";
    let (m, _) = run(src, &[VECTOR]);
    assert_eq!(m.peek(0x9000), 0x01);
}

#[test]
fn list_persists_across_frames() {
    // The CPU halts with interrupts disabled after frame 0's submit.
    let mut m = machine_with(&asm(&program(&LIST, 10, 1)), &[VECTOR]);
    for _ in 0..3 {
        frame(&mut m);
        assert_eq!(m.vector_list(), LIST);
    }
}

#[test]
fn clear_through_the_cpu() {
    let (m, _) = run(&program(&LIST, 10, 2), &[VECTOR]);
    assert_eq!(m.peek(0x9000), 0x00);
    assert!(m.vector_list().is_empty());
}
