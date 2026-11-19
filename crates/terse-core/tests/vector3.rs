//! Device 9, the 3D vector display (SPEC 13): its calls' results and
//! charges, the style and persistence it shares with device 3, its state
//! (STATE.md 1.11) and a snapshot's round trip.

mod common;

use terse_core::{DeviceReq, Machine, Snapshot};

const V3: DeviceReq = DeviceReq { slot: 9, version: 0 };

/// Stores a0 at RAM offset `at` (from s0 = the start of RAM).
fn keep(at: u32) -> String {
    format!("    sw a0, {at}(s0)\n")
}

/// The calls of device 9 and their results, kept from offset 256 of RAM.
fn calls_program() -> String {
    let mut s = String::from(
        "    lui s0, 0x100\n\
         \x20   li t0, 5\n\
         \x20   sb t0, 0(s0)\n",
    );
    let call = |s: &mut String, a0: &str, a1: &str, a7: u32, at: u32| {
        s.push_str(&format!("    {a0}\n    {a1}\n    li a7, {a7}\n    ecall\n"));
        s.push_str(&keep(at));
    };
    call(&mut s, "mv a0, s0", "li a1, 1", 0x900, 256); // a point: done
    call(&mut s, "li a0, 0x10000", "li a1, 1", 0x900, 260); // from ROM: rejected
    call(&mut s, "mv a0, s0", "li a1, 0", 0x900, 264); // empty: rejected
    call(&mut s, "mv a0, s0", "li a1, 16385", 0x900, 268); // too long: rejected
    call(&mut s, "li a0, 1", "nop", 0x902, 272); // phosphor
    call(&mut s, "li a0, 2", "nop", 0x902, 276); // rejected
    call(&mut s, "li a0, 7", "nop", 0x903, 280);
    call(&mut s, "li a0, 256", "nop", 0x903, 284); // rejected
    call(&mut s, "nop", "nop", 0x904, 288); // unknown
    call(&mut s, "li a0, 0", "nop", 0x304, 292); // device 3's fill
    s.push_str("    li a7, 0x100\n    ecall\n");
    call(&mut s, "nop", "nop", 0x901, 296); // clear
    s.push_str("    li a7, 0x100\n    ecall\n1:  j 1b\n");
    s
}

fn word(m: &Machine, at: usize) -> u32 {
    u32::from_le_bytes(m.ram()[at..at + 4].try_into().unwrap())
}

#[test]
fn calls_and_shared_style() {
    if !common::have_tools() {
        return;
    }
    let rom = common::assemble(&calls_program());
    for devices in [&[V3][..], &[common::VECTOR, V3]] {
        let both = devices.len() == 2;
        let mut m = Machine::new(&common::program(&rom, 1, devices));
        common::frame(&mut m);
        let got: Vec<u32> = (0..10).map(|i| word(&m, 256 + 4 * i)).collect();
        let fill = if both { 0 } else { u32::MAX };
        assert_eq!(got, [0, 1, 1, 1, 0, 1, 0, 1, u32::MAX, fill], "devices {devices:?}");
        assert_eq!(m.vector3().unwrap().list(), [5]);
        // One setting for both displays.
        let v = m.vector().unwrap();
        assert_eq!((v.phosphor(), v.persistence()), (true, 7));
        common::frame(&mut m);
        assert_eq!(word(&m, 296), 0);
        assert!(m.vector3().unwrap().list().is_empty());
    }
    // Without device 9 its calls are unknown.
    let mut m = Machine::new(&common::program(&rom, 1, &[common::VECTOR]));
    common::frame(&mut m);
    assert_eq!(word(&m, 256), u32::MAX);
    assert!(m.vector3().is_none());
}

/// A submit costs the call and a unit per 4 bytes copied, a rejected one
/// the call only (SPEC 14).
#[test]
fn charges() {
    if !common::have_tools() {
        return;
    }
    let used = |a0: &str| {
        let src = format!("    lui s0, 0x100\n    {a0}\n    li a1, 100\n    li a7, 0x900\n    ecall\n    li a7, 0x100\n    ecall\n1:  j 1b\n");
        let mut m = Machine::new(&common::program(&common::assemble(&src), 1, &[V3]));
        common::frame(&mut m).used
    };
    let (done, rejected) = (used("mv a0, s0"), used("li a0, 0x10000"));
    assert_eq!(done - rejected, 25);
}

/// STATE.md 1.7 and 1.11: an image with device 9 has the vector section
/// (the shared style) and, after the page and machines, the 3D list.
#[test]
fn state_and_snapshots() {
    let lone = common::program(&[0x01, 0x00], 1, &[V3]);
    let m = Machine::new(&lone);
    let s = m.state();
    let l = 8192;
    let v = 200 + l + 37 + 1; // after the system section and the terminal flag
    assert_eq!(&s[v..v + 8], &[1, 0, 160, 0, 0, 0, 0, 0], "vector: present, clean, 160, fill, empty");
    assert_eq!(&s[s.len() - 4..], &[0, 0, 0, 0], "3D list: empty");
    assert_eq!(s.len(), 45 + 128 + 23 + 4 + l + 37 + 1 + 8 + 1 + 1 + 4);
    // With a list, the hash changes, and a snapshot restores it.
    if common::have_tools() {
        let src = "    lui s0, 0x100\n    li t0, 5\n    sb t0, 0(s0)\n    sb t0, 1(s0)\n    mv a0, s0\n    li a1, 2\n    li a7, 0x900\n    ecall\n\
                   \x20   li a0, 1\n    li a7, 0x902\n    ecall\n    li a7, 0x100\n    ecall\n1:  j 1b\n";
        for devices in [&[V3][..], &[common::VECTOR, common::PAGE, V3]] {
            let image = common::program(&common::assemble(src), 1, devices);
            let mut m = Machine::new(&image);
            let before = m.state_hash();
            common::frame(&mut m);
            assert_ne!(m.state_hash(), before);
            assert_eq!(&m.state()[m.state().len() - 6..], &[2, 0, 0, 0, 5, 5]);
            let snapshot = Snapshot::of(&m);
            assert_eq!(Snapshot::decode(&snapshot.encode()).unwrap(), snapshot);
            let r = snapshot.restore(&image).unwrap();
            assert_eq!((r.state(), r.state_hash()), (m.state(), m.state_hash()));
            assert_eq!(r.vector3().unwrap().list(), [5, 5]);
            assert!(r.vector().unwrap().phosphor());
            // Without device 3 the fill mode cannot have been set.
            if devices.len() == 1 {
                let mut bad = m.state();
                bad[v + 3] = 1;
                assert!(Machine::restore(&image, &bad).is_err());
            }
            // A list longer than a submit can give is refused.
            let mut bad = m.state();
            let n = bad.len();
            bad[n - 6..n - 2].copy_from_slice(&16_385u32.to_le_bytes());
            assert!(Machine::restore(&image, &bad).is_err());
        }
    }
    // An image without device 9 has no such section: a machine with the
    // other devices serializes as before.
    let without = Machine::new(&common::program(&[0x01, 0x00], 1, &[]));
    assert_eq!(without.state().len(), 45 + 128 + 23 + 4 + l + 37 + 4);
}
