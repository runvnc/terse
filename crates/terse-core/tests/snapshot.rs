//! Restoring a machine from its state serialization and snapshot files
//! (STATE.md 4, SPEC §machines.snapshots): round trips, what a restore refuses, and the
//! pending content requests that fail in the first frame after it.

mod common;

use terse_core::content::status;
use terse_core::{
    Completion, DeviceReq, Event, FrameInput, Image, Machine, Outcome, RestoreError, Snapshot, Stopped,
};

const ALL: [DeviceReq; 4] = [common::TERMINAL, common::VECTOR, common::CONTENT, common::PAGE];

fn i_type(op: u32, rd: u32, f3: u32, rs1: u32, imm: i32) -> u32 {
    ((imm as u32 & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}
fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    i_type(0x13, rd, 0, rs1, imm)
}
fn store(f3: u32, rs2: u32, rs1: u32, imm: u32) -> u32 {
    ((imm >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | ((imm & 31) << 7) | 0x23
}
const ECALL: u32 = 0x0000_0073;
const LOOP: u32 = 0x0000_006F; // j .

fn rom(words: &[u32], tail: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    out.extend_from_slice(tail);
    out
}

/// Restores `m`'s state for `image` and checks the round trip: the same
/// serialization, and the snapshot file decodes to the same snapshot.
fn round_trip(m: &Machine, image: &Image) -> Machine {
    let snapshot = Snapshot::of(m);
    let file = snapshot.encode();
    assert_eq!(Snapshot::decode(&file).unwrap(), snapshot);
    let r = snapshot.restore(image).unwrap();
    assert_eq!(r.state(), m.state(), "serialize, restore, serialize");
    assert_eq!(r.state_hash(), m.state_hash());
    r
}

#[test]
fn at_reset_with_every_device() {
    let image = common::program(&[0x01, 0x00], 2, &ALL);
    let m = Machine::new(&image);
    round_trip(&m, &image);
    let bare = common::program(&[0x01, 0x00], 1, &[]);
    round_trip(&Machine::new(&bare), &bare);
}

#[test]
fn stopped_machines() {
    // li a0, 5; li a7, 0x106 (exit); ecall
    let image = common::program(&rom(&[addi(10, 0, 5), addi(17, 0, 0x106), ECALL], &[]), 1, &[common::TERMINAL]);
    let mut m = Machine::new(&image);
    m.run_frame(&FrameInput::default());
    assert_eq!(m.stopped(), Some(Stopped::Exit(5)));
    let r = round_trip(&m, &image);
    assert_eq!(r.stopped(), Some(Stopped::Exit(5)));
    // c.unimp: an illegal instruction.
    let image = common::program(&[0x00, 0x00], 1, &[]);
    let mut m = Machine::new(&image);
    m.run_frame(&FrameInput::default());
    let r = round_trip(&m, &image);
    assert_eq!(r.fault(), m.fault());
    assert!(r.fault().is_some());
}

#[test]
fn an_incomplete_utf8_sequence_carries_on() {
    // write(the two bytes after the code, 2), then loop: the first two
    // bytes of U+65E5 (E6 97 A5).
    let words = [0x0000_0517, addi(10, 10, 24), addi(11, 0, 2), addi(17, 0, 0x200), ECALL, LOOP];
    let image = common::program(&rom(&words, &[0xE6, 0x97]), 1, &[common::TERMINAL]);
    let mut m = Machine::new(&image);
    m.run_frame(&FrameInput::default());
    let mut r = round_trip(&m, &image);
    for _ in 0..3 {
        m.run_frame(&FrameInput::default());
        r.run_frame(&FrameInput::default());
        assert_eq!(r.state_hash(), m.state_hash());
    }
}

/// A program that submits a get for 16 bytes of blob 0 (block at the start
/// of RAM, data at RAM + 256), then loops.
fn get_image() -> Image {
    let words = [
        0x0010_02B7,           // lui t0, 0x100: RAM
        addi(6, 0, 1),         // t1 = get
        store(0, 6, 5, 0),     // sb t1, 0(t0)
        addi(6, 0, 16),        // length
        store(2, 6, 5, 40),    // sw t1, 40(t0)
        addi(6, 5, 256),       // addr
        store(2, 6, 5, 44),    // sw t1, 44(t0)
        addi(10, 5, 0),        // a0 = the block
        addi(17, 0, 0x400),    // submit
        ECALL,
        LOOP,
    ];
    common::program(&rom(&words, &[]), 1, &[common::CONTENT])
}

#[test]
fn pending_requests_fail_in_the_first_frame() {
    let image = get_image();
    let mut m = Machine::new(&image);
    m.run_frame(&FrameInput::default());
    assert_eq!(m.take_content_requests().len(), 1);
    assert_eq!(m.content().unwrap().pending_handles(), [1]);
    let mut r = round_trip(&m, &image);
    assert!(r.take_content_requests().is_empty(), "the host never had it");
    // The host's answer for the original, which the restored machine
    // ignores: its request has failed by then.
    let done = [Completion { handle: 1, outcome: Outcome::Got { size: 20, data: vec![9; 16] } }];
    let input = FrameInput { completions: &done, ..Default::default() };
    m.run_frame(&input);
    let result = r.run_frame(&input);
    assert_eq!(result.completions_ignored, 1);
    assert_eq!(m.ram()[1], status::DONE);
    assert_eq!(r.ram()[1], status::FAILED);
    let size = |m: &Machine| u32::from_le_bytes(m.ram()[48..52].try_into().unwrap());
    assert_eq!((size(&m), size(&r)), (20, 0), "nothing else of the block is written");
    let queue: Vec<Event> = r.system().queue().copied().collect();
    assert_eq!(queue, [Event::content_done(1, status::FAILED)]);
    assert!(r.content().unwrap().pending_handles().is_empty());
    // Only once.
    r.run_frame(&FrameInput::default());
    assert_eq!(r.system().queue().count(), 1);
}

#[test]
fn another_image_is_refused() {
    let a = common::program(&[0x01, 0x00], 1, &[common::TERMINAL]);
    let b = common::program(&[0x01, 0x00], 2, &[common::TERMINAL]);
    let s = Snapshot::of(&Machine::new(&a));
    assert_eq!(s.restore(&b).err(), Some(RestoreError::WrongImage));
    assert_eq!(Machine::restore(&b, &s.state).err(), Some(RestoreError::WrongImage));
}

#[test]
fn a_shell_is_not_restored() {
    let image = common::program(&[0x01, 0x00], 1, &[DeviceReq::new(8, 0)]);
    let shell = Machine::new_shell(&image, Vec::new());
    assert!(matches!(Machine::restore(&image, &shell.state()), Err(RestoreError::Malformed(_))));
    // The same image not run as a shell has the section as 00h.
    round_trip(&Machine::new(&image), &image);
}

#[test]
fn malformed_states_are_refused() {
    let image = common::program(&[0x01, 0x00], 2, &ALL);
    let mut m = Machine::new(&image);
    m.run_frame(&FrameInput::default());
    let good = m.state();
    // Cut anywhere, or one byte too long.
    for n in (0..good.len()).step_by(97).chain([good.len() - 1]) {
        assert!(Machine::restore(&image, &good[..n]).is_err(), "cut at {n}");
    }
    let mut long = good.clone();
    long.push(0);
    assert!(Machine::restore(&image, &long).is_err());
    let l = 2 * 8192;
    let (s, t) = (200 + l, 200 + l + 37);
    let v = t + 10_379;
    let p = v + 8 + 425;
    let bad = |at: usize, value: &[u8], what: &str| {
        let mut b = good.clone();
        b[at..at + value.len()].copy_from_slice(value);
        assert!(matches!(Machine::restore(&image, &b), Err(RestoreError::Malformed(_))), "{what}");
    };
    bad(0, b"terse-state-2", "domain");
    bad(177, &[3], "stopped");
    bad(177, &[1, 0, 2], "an exit with a fault kind");
    bad(192, &1i32.to_le_bytes(), "a positive budget");
    bad(196, &(l as u32 + 8192).to_le_bytes(), "RAM size");
    bad(s + 4, &[65], "65 events");
    bad(t, &[2], "terminal flag");
    bad(t + 9601, &[80], "cursor column");
    bad(t + 10_375, &[1, 0x41], "an incomplete sequence that is not one");
    bad(t + 10_375, &[1, 0xE6, 1], "padding after the sequence");
    bad(v + 1, &[2], "style");
    bad(v + 4, &16_385u32.to_le_bytes(), "list length");
    bad(v + 8 + 1, &[0x12], "request op");
    bad(v + 8 + 1, &[0x01, 0, 0, 0, 0], "a request block outside RAM");
    bad(p + 1, &700u16.to_le_bytes(), "a rectangle not clamped");
    bad(p + 17, &65_537u32.to_le_bytes(), "document length");
    // Every single-byte change either restores to exactly that state or
    // is refused; nothing panics.
    for at in (0..good.len()).step_by(13) {
        let mut b = good.clone();
        b[at] ^= 0x5A;
        if let Ok(r) = Machine::restore(&image, &b) {
            assert_eq!(r.state(), b, "byte {at}");
        }
    }
}

#[test]
fn snapshot_files() {
    let image = common::program(&[0x01, 0x00], 128, &ALL);
    let m = Machine::new(&image);
    let s = Snapshot::of(&m);
    let file = s.encode();
    // Nothing has run: RAM is all zero and no page is stored.
    assert!(file.len() < 32 * 1024, "zero pages are not stored: {} bytes", file.len());
    assert_eq!(s.state.len(), 200 + 128 * 8192 + 37 + 10_379 + 8 + 425 + 21);
    assert_eq!(Snapshot::decode(&file).unwrap(), s);
    assert_eq!(&file[..4], b"TSNP");
    assert_eq!(&file[file.len() - 32..], &m.state_hash());
    for n in [0, 3, 50, 293, 300, file.len() - 1] {
        assert!(Snapshot::decode(&file[..n]).is_err(), "cut at {n}");
    }
    let mut longer = file.clone();
    longer.push(0);
    assert!(Snapshot::decode(&longer).is_err(), "bytes after the hash");
    let mut flipped = file.clone();
    *flipped.last_mut().unwrap() ^= 1;
    assert_eq!(Snapshot::decode(&flipped).err(), Some(RestoreError::Malformed("the state hash does not match")));
    let mut flipped = file.clone();
    flipped[400] ^= 1;
    assert!(Snapshot::decode(&flipped).is_err(), "a changed byte of the state");
    let mut map = file.clone();
    map[294 + 15] = 0x80;
    assert!(Snapshot::decode(&map).is_err(), "a page past the end");
    // A page that is stored although zero reads the same.
    let mut stored = file[..310].to_vec();
    stored[294] = 1;
    stored.extend_from_slice(&[0; 8192]);
    stored.extend_from_slice(&file[310..]);
    assert_eq!(Snapshot::decode(&stored).unwrap(), s);

    // An identity and a time.
    let mut m = Machine::new(&image);
    m.set_identity(Some(terse_core::ProgramId { publisher: [3; 32], name: b"tool".to_vec() }));
    let t = FrameInput { unix_time: 1_800_000_000, ..Default::default() };
    m.run_frame(&t);
    let s = Snapshot::of(&m);
    let info = s.info();
    assert_eq!((info.frames, info.time, info.image), (1, 1_800_000_000, image.id));
    let back = Snapshot::decode(&s.encode()).unwrap();
    assert_eq!(back.identity, m.identity().cloned());
    assert_eq!(terse_core::SnapshotInfo::from_file_head(&s.encode()), Some(info.clone()));
    assert_eq!(info.record()[0], 1);
    assert_eq!(&info.record()[33..65], &[3; 32]);
    assert_eq!(back.restore(&image).unwrap().identity(), m.identity());
}

/// A page holding a document with a table survives the round trip, and
/// the restored machine's document lays out as the original's.
#[test]
fn a_table_document_round_trips() {
    use terse_core::page::{op, reference_layout};
    let mut doc = vec![op::TABLE, 2, 1, 0, 2, op::CELL, op::TEXT, 3, 0, b'K', b'e', b'y', op::CELL, op::TEXT, 1, 0, b'v'];
    doc.extend_from_slice(&[op::CELL, op::TEXT, 1, 0, b'a', op::CELL, op::TEXT, 1, 0, b'1', op::END_TABLE]);
    // submit(the bytes after the code, their length), then loop.
    let words = [0x0000_0517, addi(10, 10, 24), addi(11, 0, doc.len() as i32), addi(17, 0, 0x700), ECALL, LOOP];
    let image = common::program(&rom(&words, &doc), 1, &[common::PAGE]);
    let mut m = Machine::new(&image);
    m.run_frame(&FrameInput::default());
    assert_eq!(m.page().unwrap().document(), Some(&doc[..]));
    let r = round_trip(&m, &image);
    let (a, b) = (reference_layout(m.page().unwrap().document().unwrap(), 200), reference_layout(r.page().unwrap().document().unwrap(), 200));
    assert_eq!(a, b);
    assert!(a.lines.iter().any(|l| l.rule_span.is_some()), "a table, with its header rule");
}

/// STATE.md 4: what a reader of a snapshot file refuses besides a
/// malformed S: N below 200 or above the longest serialization
/// (1,423,091 bytes), L that is 0, not a multiple of 8,192, above
/// 1,048,576 or past N, and a page-map bit past the last page. And SPEC
/// §machines.calls `snapshot_info`: the time is the one latched for the machine's
/// last frame as its state holds it, so a restored machine snapshotted
/// before it runs again keeps the time of the first snapshot.
#[test]
fn snapshot_file_fields_and_time() {
    let image = common::program(&[0x01, 0x00], 2, &[]);
    let mut m = Machine::new(&image);
    assert_eq!(Snapshot::of(&m).time, 0, "no frame run: the time latch is 0");
    m.run_frame(&FrameInput { unix_time: 1_700_000_000, ..Default::default() });
    let s = Snapshot::of(&m);
    let file = s.encode();
    let with = |at: usize, v: u32| {
        let mut f = file.clone();
        f[at..at + 4].copy_from_slice(&v.to_le_bytes());
        Snapshot::decode(&f).err()
    };
    let malformed = |why| Some(RestoreError::Malformed(why));
    assert_eq!(with(90, 199), malformed("state length"));
    assert_eq!(with(90, 1_423_092), malformed("state length"));
    assert_eq!(with(94 + 196, 0), malformed("RAM size"));
    assert_eq!(with(94 + 196, 8193), malformed("RAM size"));
    assert_eq!(with(94 + 196, 129 * 8192), malformed("RAM size"));
    assert_eq!(with(94 + 196, 64 * 8192), malformed("RAM size"), "past N");
    let mut map = file.clone();
    map[294] |= 1 << 2; // page 2 of a 2-page RAM
    assert_eq!(Snapshot::decode(&map).err(), malformed("page map"));
    // Restored, paused (no frame run), snapshotted again: the same time.
    let back = Snapshot::decode(&file).unwrap().restore(&image).unwrap();
    assert_eq!(Snapshot::of(&back).time, 1_700_000_000);
    assert_eq!(Snapshot::of(&back).info(), s.info());
    // STATE.md 1.13: the argument is there exactly when 32 bytes remain
    // after section 1.12; any other remainder is refused.
    let state = m.state();
    for extra in [1, 31, 33, 64] {
        let mut longer = state.clone();
        longer.extend(std::iter::repeat_n(7, extra));
        assert!(Machine::restore(&image, &longer).is_err(), "{extra} bytes more");
    }
    let mut with_argument = state.clone();
    with_argument.extend([7; 32]);
    assert_eq!(Machine::restore(&image, &with_argument).unwrap().argument(), Some(&[7; 32]));
}
