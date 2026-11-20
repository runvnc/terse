//! Optional devices (SPEC 2): the grants are fixed at reset, the `device`
//! call (SPEC 4) tells the program, and the grants are part of the state
//! (STATE.md 1.4), so a snapshot restores only where its devices can be
//! granted and a device it says was absent stays absent.

use terse_core::{DeviceReq, FrameInput, Grants, Image, Machine, RestoreError, Snapshot};

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

/// Asks `device` of each slot in `slots` and stores the answers at the
/// start of RAM, then submits a content request (whose answer goes after
/// them) and waits for frames for ever.
fn prober(slots: &[i32]) -> Vec<u8> {
    let mut w = vec![0x0010_02B7]; // lui t0, 0x100: t0 = RAM
    for (i, &s) in slots.iter().enumerate() {
        w.extend([li(10, s), li(17, 0x109), ECALL, sw(10, 4 * i as i32, 5)]);
    }
    // Content submit with a block at RAM + 256: FFFF_FFFFh without device 4.
    w.extend([i_type(256, 5, 0, 10, 0x13), li(17, 0x400), ECALL, sw(10, 4 * slots.len() as i32, 5)]);
    let wait = w.len();
    w.extend([li(17, 0x100), ECALL]);
    // jal x0 back to the wait.
    let back = -(((w.len() - wait) * 4) as i32);
    let imm = back as u32;
    w.push((imm >> 20 & 1) << 31 | (imm >> 1 & 0x3FF) << 21 | (imm >> 11 & 1) << 20 | (imm >> 12 & 0xFF) << 12 | 0x6F);
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

const SLOTS: [i32; 9] = [0, 1, 2, 3, 4, 6, 7, 8, 9];

fn image(devices: &[DeviceReq]) -> Image {
    Image::parse(&Image::build("optional", 1, devices, &prober(&SLOTS))).unwrap()
}

fn answers(m: &Machine) -> Vec<u32> {
    m.ram()[..4 * (SLOTS.len() + 1)].chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

const ABSENT: u32 = 0xFFFF_FFFF;

fn run(m: &mut Machine, frames: u32) -> Vec<[u8; 32]> {
    (0..frames).map(|_| {
        m.run_frame(&FrameInput::default());
        m.state_hash()
    }).collect()
}

/// Terminal required; content and sound optional.
fn devices() -> [DeviceReq; 3] {
    [DeviceReq::new(2, 0), DeviceReq::optional(4, 0), DeviceReq::optional(6, 0)]
}

#[test]
fn the_device_call_tells_what_was_granted() {
    let img = image(&devices());
    let mut all = Machine::new(&img);
    assert_eq!(all.grants(), &Grants(vec![true, true]));
    run(&mut all, 1);
    let a = answers(&all);
    // Slots 0 1 2 3 4 6 7 8 9, then content submit.
    assert_eq!(&a[..9], &[ABSENT, 0, 0, ABSENT, 0, 0, ABSENT, ABSENT, ABSENT]);
    assert_ne!(a[9], ABSENT, "content is there");

    let mut without = Machine::with_grants(&img, Grants(vec![false, true]));
    run(&mut without, 1);
    let a = answers(&without);
    assert_eq!(&a[..9], &[ABSENT, 0, 0, ABSENT, ABSENT, 0, ABSENT, ABSENT, ABSENT]);
    assert_eq!(a[9], ABSENT, "a call to a device not granted is a call to an absent device");
    assert!(without.content().is_none() && without.sound().is_some());
}

/// A slot above 255 is no device; device 8 only for a shell.
#[test]
fn device_eight_and_large_slots() {
    let img = Image::parse(&Image::build("p", 1, &[DeviceReq::new(8, 0)], &prober(&[8, 256, -1]))).unwrap();
    let mut alone = Machine::new(&img);
    run(&mut alone, 1);
    assert_eq!(&answers_n(&alone, 3), &[ABSENT, ABSENT, ABSENT]);
    let mut shell = Machine::new_shell(&img, Vec::new());
    run(&mut shell, 1);
    assert_eq!(&answers_n(&shell, 3), &[0, ABSENT, ABSENT]);
}

fn answers_n(m: &Machine, n: usize) -> Vec<u32> {
    m.ram()[..4 * n].chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

/// Both ways the run is deterministic; the serializations differ only in
/// the grant bytes after RAM and in the sections of the devices withheld.
#[test]
fn state_with_and_without() {
    let img = image(&devices());
    let with = || Machine::new(&img);
    let without = || Machine::with_grants(&img, Grants(vec![false, true]));
    assert_eq!(run(&mut with(), 5), run(&mut with(), 5));
    assert_eq!(run(&mut without(), 5), run(&mut without(), 5));
    let (a, b) = (with().state(), without().state());
    let ram_end = 200 + 8192;
    assert_eq!(a[..ram_end], b[..ram_end]);
    assert_eq!(&a[ram_end..ram_end + 2], &[1, 1]);
    assert_eq!(&b[ram_end..ram_end + 2], &[0, 1]);
    // Content's section: 425 bytes with the device, its flag without.
    assert_eq!(a.len() - b.len(), 424);
}

/// An image with no optional devices has no grant bytes: its state is laid
/// out as before optional devices existed.
#[test]
fn no_grant_bytes_without_optional_devices() {
    let img = image(&[DeviceReq::new(2, 0), DeviceReq::new(4, 0)]);
    let m = Machine::new(&img);
    assert!(m.grants().0.is_empty());
    let s = m.state();
    // System section right after RAM: time latch 0, an empty queue.
    assert_eq!(&s[200 + 8192..200 + 8192 + 5], &[0, 0, 0, 0, 0]);
    assert_eq!(s.len(), 200 + 8192 + 37 + 10_379 + 1 + 425 + 1);
}

#[test]
fn snapshots_carry_the_grants() {
    let img = image(&devices());
    let mut with = Machine::new(&img);
    let mut without = Machine::with_grants(&img, Grants(vec![false, true]));
    run(&mut with, 3);
    run(&mut without, 3);
    let (sw, swo) = (Snapshot::of(&with), Snapshot::of(&without));
    assert_eq!(sw.grants(&img), Ok(Grants(vec![true, true])));
    assert_eq!(swo.grants(&img), Ok(Grants(vec![false, true])));

    // Taken with content: restores where content can be granted, not where not.
    let back = Snapshot::decode(&sw.encode()).unwrap().restore(&img).unwrap();
    assert_eq!(back.state_hash(), with.state_hash());
    let refused = sw.restore_where(&img, |d| d.slot != 4).err();
    assert!(matches!(refused, Some(RestoreError::NeedsDevice(_))), "{refused:?}");
    // Taken without content: content stays absent on a host that has it.
    let mut back = swo.restore(&img).unwrap();
    assert_eq!(back.grants(), &Grants(vec![false, true]));
    assert!(back.content().is_none());
    assert_eq!(back.state_hash(), without.state_hash());
    assert_eq!(run(&mut back, 3), run(&mut without, 3));
    // Without sound too: withheld sound is no obstacle.
    assert!(swo.restore_where(&img, |d| d.slot == 6).is_ok());
    assert!(matches!(swo.restore_where(&img, |d| d.slot != 6), Err(RestoreError::NeedsDevice(_))));

    // A grant byte changed is refused: the sections do not match it, or
    // it is not a flag.
    let at = 200 + 8192;
    for (v, what) in [(1u8, "content granted"), (2, "not a flag")] {
        let mut s = swo.state.clone();
        s[at] = v;
        assert!(matches!(Machine::restore(&img, &s), Err(RestoreError::Malformed(_))), "{what}");
    }
}

/// An optional device this implementation does not have (a reserved slot)
/// is never granted; a state that says it was cannot be restored.
#[test]
fn unknown_optional_device() {
    let img = image(&[DeviceReq::new(2, 0), DeviceReq::optional(5, 0)]);
    let m = Machine::new(&img);
    assert_eq!(m.grants(), &Grants(vec![false]));
    let mut s = m.state();
    s[200 + 8192] = 1;
    assert!(matches!(Machine::restore(&img, &s), Err(RestoreError::NeedsDevice(_))));
}
