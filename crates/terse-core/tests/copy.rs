//! The system's `copy` (SPEC §system): text a program offers the user's
//! clipboard reaches the host only in a frame in which the program read the
//! key-down of a copy key (C, X or Insert with Control, without Alt or
//! Meta); the call returns 0 either way and is charged for its bytes, and a
//! buffer out of range or longer than 65,536 bytes is a bad buffer.

use terse_core::{Event, FrameInput, Image, Machine, modifiers};

const ECALL: u32 = 0x73;
const TEXT: &[u8] = b"a\tb\n";

fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, op: u32) -> u32 {
    ((imm as u32 & 0xFFF) << 20) | rs1 << 15 | funct3 << 12 | rd << 7 | op
}

fn li(rd: u32, imm: i32) -> u32 {
    i_type(imm, 0, 0, rd, 0x13)
}

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    i_type(imm, rs1, 0, rd, 0x13)
}

fn lui(rd: u32, imm20: u32) -> u32 {
    imm20 << 12 | rd << 7 | 0x37
}

fn sw(rs2: u32, off: i32, rs1: u32) -> u32 {
    let imm = off as u32 & 0xFFF;
    (imm >> 5) << 25 | rs2 << 20 | rs1 << 15 | 2 << 12 | (imm & 0x1F) << 7 | 0x23
}

/// A branch from instruction `at` to instruction `to` if rs1 != rs2.
fn bne(rs1: u32, rs2: u32, at: usize, to: usize) -> u32 {
    let imm = ((to as i32 - at as i32) * 4) as u32;
    (imm >> 12 & 1) << 31 | (imm >> 5 & 0x3F) << 25 | rs2 << 20 | rs1 << 15 | 1 << 12 | (imm >> 1 & 0xF) << 8 | (imm >> 11 & 1) << 7 | 0x63
}

fn jal_back(at: usize, to: usize) -> u32 {
    let imm = ((to as i32 - at as i32) * 4) as u32;
    (imm >> 20 & 1) << 31 | (imm >> 1 & 0x3FF) << 21 | (imm >> 11 & 1) << 20 | (imm >> 12 & 0xFF) << 12 | 0x6F
}

/// Every frame: reads every event, then copies TEXT (from ROM) and stores
/// the result at RAM + 16, then tries a copy of 65,537 bytes and stores its
/// result at RAM + 20.
fn program() -> Vec<u8> {
    let mut w = vec![lui(5, 0x100)]; // t0 = RAM
    let top = w.len();
    w.extend([addi(10, 5, 0), li(17, 0x101), ECALL]);
    let at = w.len();
    w.push(bne(10, 0, at, top));
    // a0 = the text in ROM (filled in below), a1 = its length.
    let text_at = w.len();
    w.extend([lui(10, 0x10), 0, li(11, TEXT.len() as i32), li(17, 0x10A), ECALL, sw(10, 16, 5)]);
    // 65,537 bytes: a1 = 0x10001.
    w.extend([lui(10, 0x10), lui(11, 0x10), addi(11, 11, 1), li(17, 0x10A), ECALL, sw(10, 20, 5)]);
    w.extend([li(17, 0x100), ECALL]);
    let at = w.len();
    w.push(jal_back(at, top));
    let text_off = (w.len() * 4) as i32;
    w[text_at + 1] = addi(10, 10, text_off);
    let mut rom: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
    rom.extend(TEXT);
    rom
}

fn machine() -> Machine {
    Machine::new(&Image::parse(&Image::build("copy", 1, &[], &program())).unwrap())
}

fn frame(m: &mut Machine, events: &[Event]) -> Option<Vec<u8>> {
    let r = m.run_frame(&FrameInput { events, ..Default::default() });
    assert!(r.waited, "the frame ended in wait_frame");
    let results: Vec<u32> = m.ram()[16..24].chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    assert_eq!(results, [0, 0xFFFF_FFFE], "copy returns 0; a text over 65,536 bytes is a bad buffer");
    m.take_copied()
}

#[test]
fn only_the_users_copy_key_reaches_the_clipboard() {
    use modifiers::{ALT, CONTROL, META, SHIFT};
    let mut m = machine();
    assert_eq!(frame(&mut m, &[]), None, "no key: dropped");
    for (key, mods) in [(0x06, CONTROL), (0x1B, CONTROL), (0x49, CONTROL), (0x06, CONTROL | SHIFT)] {
        assert_eq!(frame(&mut m, &[Event::key_down(key, mods)]).as_deref(), Some(TEXT), "key {key:#x} mods {mods}");
        assert_eq!(frame(&mut m, &[]), None, "only in the frame of the key");
    }
    for (key, mods) in [(0x06, 0), (0x06, CONTROL | ALT), (0x06, CONTROL | META), (0x19, CONTROL)] {
        assert_eq!(frame(&mut m, &[Event::key_down(key, mods)]), None, "key {key:#x} mods {mods}");
    }
    assert_eq!(frame(&mut m, &[Event::key_up(0x06, CONTROL), Event::text('c')]), None, "a key-up or text is no copy key");
}

/// What reaches the clipboard is not machine state: two machines that read
/// the same events have the same state whether or not their text was taken.
#[test]
fn the_copy_leaves_no_state() {
    let (mut a, mut b) = (machine(), machine());
    let ctrl_c = [Event::key_down(0x06, modifiers::CONTROL)];
    a.run_frame(&FrameInput { events: &ctrl_c, ..Default::default() });
    b.run_frame(&FrameInput { events: &ctrl_c, ..Default::default() });
    assert!(a.take_copied().is_some());
    assert_eq!(a.state_hash(), b.state_hash());
}
