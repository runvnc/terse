//! Device 9, the 3D vector display (SPEC §vector3d): its calls' results and
//! charges, the style and persistence it shares with device 3, its state
//! (STATE.md 1.11) and a snapshot's round trip.

mod common;

use terse_core::{DeviceReq, Machine, Snapshot};

const V3: DeviceReq = DeviceReq::new(9, 0);

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
/// the call only (SPEC §charges).
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

/// Builds a display list (SPEC §vector3d.list).
#[derive(Default)]
struct List(Vec<u8>);

impl List {
    fn xyz(&mut self, op: u8, p: [i32; 3]) -> &mut Self {
        self.0.push(op);
        for v in p {
            self.0.extend_from_slice(&(v as i16).to_le_bytes());
        }
        self
    }
    fn mv(&mut self, p: [i32; 3]) -> &mut Self {
        self.xyz(0x03, p)
    }
    fn line(&mut self, p: [i32; 3]) -> &mut Self {
        self.xyz(0x04, p)
    }
    fn circle(&mut self, r: u16) -> &mut Self {
        self.0.push(0x06);
        self.0.extend_from_slice(&r.to_le_bytes());
        self
    }
    fn face(&mut self, corners: &[[i32; 3]]) -> &mut Self {
        self.0.extend([0x0C, corners.len() as u8]);
        for c in corners {
            for v in c {
                self.0.extend_from_slice(&(*v as i16).to_le_bytes());
            }
        }
        self
    }
}

/// The cost of cutting segments against faces (run with `--release
/// --ignored --nocapture`): the worst lists the caps allow and a typical
/// scene, from an eye that moves every time.
#[test]
#[ignore]
fn face_cost() {
    use terse_core::vector3::project;
    let time = |name: &str, list: &[u8]| {
        let mut out = Vec::new();
        let p = project(list, (0.0, 0.0), &mut out);
        let n = 200;
        let start = std::time::Instant::now();
        for i in 0..n {
            out.clear();
            let x = -160.0 + 320.0 * i as f32 / n as f32;
            std::hint::black_box(project(list, (x, x / 2.0), &mut out));
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / n as f64;
        eprintln!("{name}: {} bytes, {} faces, {} segments drawn (reference eye){}: {ms:.3} ms", list.len(), p.faces, p.segments, if p.capped { ", capped" } else { "" });
    };
    let u = 16;
    // 256 circles of radius 39 (16,384 chords) behind 256 faces whose boxes
    // cover the screen but which are thin slivers: nothing is hidden, so
    // every chord is tested against every triangle.
    let circles = |l: &mut List, z: i32| {
        for i in 0..256 {
            l.mv([(40 + (i % 16) * 37) * u, (40 + (i / 16) * 26) * u, z * u]).circle(39 * u as u16);
        }
    };
    let mut l = List::default();
    circles(&mut l, 200);
    time("16,384 chords, no faces", &l.0);
    for faces in [256, 128, 64, 16, 1] {
        let mut l = List::default();
        for i in 0..faces {
            let d = i - 128;
            l.face(&[[0, 0, 0], [640 * u, 480 * u, 0], [640 * u + d, 480 * u - d, 0], [d, -d, 0]]);
        }
        circles(&mut l, 200);
        time(&format!("{faces} slivers over chords"), &l.0);
        // Quads across the near plane: 4 triangles each after the cut.
        let mut l = List::default();
        for i in 0..faces {
            let d = i - 128;
            l.face(&[[0, 0, -900 * u], [640 * u, 480 * u, 500 * u], [640 * u + d, 480 * u - d, -900 * u], [d, -d, 500 * u]]);
        }
        circles(&mut l, 600);
        time(&format!("{faces} slivers across the near plane over chords"), &l.0);
        // Large quads leaning back: nearer than the chords off the screen's
        // left, behind them on it, so every triangle is tested in full and
        // hides nothing.
        let mut l = List::default();
        for i in 0..faces {
            let z = 300 + i;
            l.face(&[[-1900 * u, -100 * u, -700 * u], [700 * u, -100 * u, z * u], [700 * u, 600 * u, z * u], [-1900 * u, 600 * u, -700 * u]]);
        }
        circles(&mut l, 0);
        time(&format!("{faces} large leaning quads behind chords"), &l.0);
    }
    // The same behind 256 large overlapping quads in staggered depths, each
    // with a hole-free cover of part of the screen: much is hidden.
    let mut l = List::default();
    for i in 0..256 {
        let (x, y) = ((i % 16) * 30 * u, (i / 16) * 22 * u);
        l.face(&[[x - 200 * u, y - 150 * u, i * u], [x + 300 * u, y - 150 * u, i * u], [x + 300 * u, y + 200 * u, i * u], [x - 200 * u, y + 200 * u, i * u]]);
    }
    circles(&mut l, 400);
    time("large quads over 16,384 chords", &l.0);
    // Strips: 256 thin horizontal quads in front of vertical lines, each
    // hiding a short part of every line (many cuts).
    let mut l = List::default();
    for i in 0..256 {
        let y = (2 + i * 2) * u * 15 / 16;
        l.face(&[[-100 * u, y, 0], [740 * u, y, 0], [740 * u, y + u, 0], [-100 * u, y + u, 0]]);
    }
    let mut x = 0;
    while l.0.len() + 14 <= 16_384 {
        l.mv([x * u / 4, 0, 100 * u]).line([x * u / 4, 480 * u, 100 * u]);
        x += 1;
    }
    time("strips over vertical lines", &l.0);
    // A typical scene: 20 boxes (5 faces, 12 edges each) on a grid of 42 lines.
    let mut l = List::default();
    for i in 0..21 {
        l.mv([(120 + i * 20) * u, 300 * u, 0]).line([(120 + i * 20) * u, 300 * u, 400 * u]);
        l.mv([120 * u, 300 * u, i * 20 * u]).line([520 * u, 300 * u, i * 20 * u]);
    }
    for i in 0..20 {
        let (x0, z0, h) = (140 + (i % 5) * 80, 20 + (i / 5) * 90, 40 + (i * 37) % 100);
        let (x1, z1, y0, y1) = (x0 + 40, z0 + 40, 300 - h, 300);
        let c = |x: i32, y: i32, z: i32| [x * u, y * u, z * u];
        l.face(&[c(x0, y0, z0), c(x1, y0, z0), c(x1, y1, z0), c(x0, y1, z0)]);
        l.face(&[c(x0, y0, z1), c(x1, y0, z1), c(x1, y1, z1), c(x0, y1, z1)]);
        l.face(&[c(x0, y0, z0), c(x0, y0, z1), c(x0, y1, z1), c(x0, y1, z0)]);
        l.face(&[c(x1, y0, z0), c(x1, y0, z1), c(x1, y1, z1), c(x1, y1, z0)]);
        l.face(&[c(x0, y0, z0), c(x1, y0, z0), c(x1, y0, z1), c(x0, y0, z1)]);
        for y in [y0, y1] {
            l.mv(c(x0, y, z0)).line(c(x1, y, z0)).line(c(x1, y, z1)).line(c(x0, y, z1)).line(c(x0, y, z0));
        }
        for (x, z) in [(x0, z0), (x1, z0), (x1, z1), (x0, z1)] {
            l.mv(c(x, y0, z)).line(c(x, y1, z));
        }
    }
    time("typical: 20 boxes on a grid", &l.0);
}

/// SPEC §vector3d.list, §vector3d.caps: a circle of radius 0 is 64 chords of length 0 (64
/// segments, no length); Scale 0 puts every later point at the
/// transform's origin; a line wholly beyond the screen grown by 32 units
/// is counted by the caps (one segment) but gives no piece, one within
/// the margin gives one.
#[test]
fn radius_0_scale_0_and_the_margin() {
    use terse_core::vector3::project;
    let count = |list: &List| {
        let mut out = Vec::new();
        let p = project(&list.0, (0.0, 0.0), &mut out);
        (out.len(), p.length)
    };
    let mut l = List::default();
    l.mv([320 * 16, 240 * 16, 0]).circle(0);
    assert_eq!(count(&l), (64, 0.0));
    let mut l = List::default();
    l.xyz(0x09, [100 * 16, 200 * 16, 0]);
    l.0.extend([0x0B, 0, 0]); // Scale 0
    l.mv([3000, -3000, 100]).line([-2000, 5000, 900]);
    let mut out = Vec::new();
    let p = project(&l.0, (0.0, 0.0), &mut out);
    assert_eq!((out.len(), p.length), (1, 0.0));
    let terse_core::Primitive::Beam { x0, y0, x1, y1, .. } = out[0] else { panic!() };
    assert_eq!((x0, y0, x1, y1), (1600, 3200, 1600, 3200), "a dot at the origin of the transform");
    let mut l = List::default();
    l.mv([-100 * 16, 240 * 16, 0]).line([-40 * 16, 240 * 16, 0]);
    assert_eq!(count(&l), (0, 0.0), "beyond the margin");
    let mut l = List::default();
    l.mv([-30 * 16, 240 * 16, 0]).line([-10 * 16, 240 * 16, 0]);
    assert_eq!(count(&l), (1, 0.0), "within the margin, counted length 0");
}

/// SPEC §vector3d.list: a piece with a cut end shorter than 1/2 unit is not drawn
/// unless the whole line is that short, the whole line being its part in
/// front of the near plane, on the glass, before it is clipped to the
/// screen: here a line 1,000 units long, of which 0.4 units on the screen
/// are left visible by a face.
#[test]
fn short_piece_measured_against_the_whole_line() {
    use terse_core::vector3::project;
    let mut l = List::default();
    l.mv([-16000, 3840, 0]).line([6, 3840, 0]);
    let mut out = Vec::new();
    project(&l.0, (0.0, 0.0), &mut out);
    assert_eq!(out.len(), 1, "without the face");
    l.face(&[[-3200, 0, -16], [5, 0, -16], [5, 7680, -16], [-3200, 7680, -16]]);
    out.clear();
    project(&l.0, (0.0, 0.0), &mut out);
    assert_eq!(out.len(), 0, "the piece left is 0.4 units, the line 1,000");
}
