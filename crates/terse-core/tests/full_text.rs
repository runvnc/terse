//! Device 10, full text (SPEC §terminal.text): a capability with no calls
//! and no state. Listed optional it is granted or not, the `device` call
//! says which, the grant is in the state like any optional device's, and
//! it tells the terminal how to be drawn, never what it holds.

use terse_core::text::FULL_TEXT;
use terse_core::{DeviceReq, FrameInput, Grants, Image, Machine};

fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, op: u32) -> u32 {
    ((imm as u32 & 0xFFF) << 20) | rs1 << 15 | funct3 << 12 | rd << 7 | op
}

fn li(rd: u32, imm: i32) -> u32 {
    i_type(imm, 0, 0, rd, 0x13)
}

fn sw(rs2: u32, off: i32, rs1: u32) -> u32 {
    let imm = off as u32 & 0xFFF;
    (imm >> 5) << 25 | rs2 << 20 | rs1 << 15 | 2 << 12 | (imm & 0x1F) << 7 | 0x23
}

const ECALL: u32 = 0x73;
const ABSENT: u32 = 0xFFFF_FFFF;

/// Stores `device(10)` and the result of call A00h at RAM 0 and 4, writes
/// "é你─" (UTF-8, from RAM 16) to the terminal, then exits.
fn rom() -> Vec<u8> {
    let text = "é你─".as_bytes();
    let mut w = vec![0x0010_02B7]; // lui t0, 0x100: t0 = RAM
    w.extend([li(10, FULL_TEXT as i32), li(17, 0x109), ECALL, sw(10, 0, 5)]);
    w.extend([li(10, 0), li(17, 0xA00), ECALL, sw(10, 4, 5)]);
    for (i, &b) in text.iter().enumerate() {
        // li t1, b; sb t1, 16+i(t0)
        let imm = 16 + i as u32;
        w.extend([li(6, b as i32), (imm >> 5) << 25 | 6 << 20 | 5 << 15 | (imm & 0x1F) << 7 | 0x23]);
    }
    w.extend([i_type(16, 5, 0, 10, 0x13), li(11, text.len() as i32), li(17, 0x200), ECALL]);
    w.extend([li(10, 0), li(17, 0x106), ECALL]);
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn image(full_text: DeviceReq) -> Image {
    Image::parse(&Image::build("text", 1, &[DeviceReq::new(2, 0), full_text], &rom())).unwrap()
}

fn word(m: &Machine, at: usize) -> u32 {
    u32::from_le_bytes(m.ram()[at..at + 4].try_into().unwrap())
}

#[test]
fn optional_full_text_is_granted_or_not() {
    let img = image(DeviceReq::optional(FULL_TEXT, 0));
    let mut with = Machine::new(&img);
    let mut without = Machine::with_grants(&img, Grants(vec![false]));
    assert_eq!(with.grants(), &Grants(vec![true]));
    for m in [&mut with, &mut without] {
        m.run_frame(&FrameInput::default());
        assert!(m.stopped().is_some(), "it exits");
    }
    assert_eq!((word(&with, 0), word(&without, 0)), (0, ABSENT), "device(10)");
    assert_eq!((word(&with, 4), word(&without, 4)), (ABSENT, ABSENT), "slot 10 has no calls");
    let (a, b) = (with.devices().terminal().unwrap(), without.devices().terminal().unwrap());
    assert!(a.full_text() && !b.full_text());
    // The same cells and cursor: é, then 你 and its continuation, then ─.
    assert_eq!(a, b, "the terminal's state does not depend on full text");
    assert_eq!([a.cell(0, 0).ch, a.cell(1, 0).ch, a.cell(2, 0).ch, a.cell(3, 0).ch], [0xE9, 0x4F60, 0xFFFF, 0x2500]);
    assert_eq!(a.cursor(), (4, 0));
    // The states differ only in the answer the program stored (4 bytes of
    // RAM) and the grant byte (STATE.md 1.4).
    let (sa, sb) = (with.state(), without.state());
    assert_eq!(sa.len(), sb.len());
    assert_eq!(sa.iter().zip(&sb).filter(|(x, y)| x != y).count(), 5);
}

#[test]
fn required_full_text_and_none() {
    let mut m = Machine::new(&image(DeviceReq::new(FULL_TEXT, 0)));
    m.run_frame(&FrameInput::default());
    assert_eq!(word(&m, 0), 0);
    assert!(m.devices().terminal().unwrap().full_text());
    // Not listed: absent, and the terminal is drawn with basic text.
    let img = Image::parse(&Image::build("text", 1, &[DeviceReq::new(2, 0)], &rom())).unwrap();
    let mut m = Machine::new(&img);
    m.run_frame(&FrameInput::default());
    assert_eq!(word(&m, 0), ABSENT);
    assert!(!m.devices().terminal().unwrap().full_text());
}
