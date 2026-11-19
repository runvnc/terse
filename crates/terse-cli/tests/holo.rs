//! programs/holo/holo.terse: golden screens of each scene (the terminal
//! text and the 3D display list with what the reference eye sees), the
//! keys, the display style it sets through device 9, the solids (faces),
//! the frame budget, and the image is up to date. Set UPDATE_GOLDEN=1 to rewrite
//! programs/holo/expected/.
//!
//! `budget` prints the busiest and typical frame (run with `--nocapture`).

mod common;

use std::path::Path;

use terse_core::{FrameResult, Image, Machine, Script};
use terse_net::service::Headless;

const IMAGE: &[u8] = include_bytes!("../../../programs/holo/holo.terse");
const RAM_PAGES: u8 = 2;

/// What `terse run --dump --dump-vectors` prints.
fn screen(m: &Machine) -> String {
    let v = m.vector().unwrap();
    let style = v.style_line().map_or(String::new(), |l| l + "\n");
    format!("{}{style}{}", m.terminal().unwrap().text(), m.vector3().unwrap().dump())
}

/// Runs `frames` frames as `terse run` does (holo has no content device).
fn run(script: &str, frames: u32) -> (Machine, Vec<FrameResult>) {
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let mut h = Headless::new(None, Script::parse(script).unwrap());
    let results = (0..frames).map(|_| h.frame(&mut m)).collect();
    (m, results)
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("holo/expected/{name}"), got);
}

/// Each scene after a second and a half of turning.
#[test]
fn scenes() {
    for (key, name) in [("1", "wireframes"), ("2", "rings"), ("3", "starfield"), ("4", "orrery"), ("5", "city"), ("1\n3 press h", "wireframes-solid")] {
        let (m, results) = run(&format!("2 press {key}\n"), 90);
        assert!(results.iter().all(|r| r.stopped.is_none() && r.waited));
        let got = screen(&m);
        assert!(got.contains(&name.split('-').next().unwrap().to_uppercase()), "{got}");
        assert!(!got.contains(", capped"), "{name}: within the caps");
        golden(&format!("{name}.txt"), &got);
    }
}

/// Space and Tab go round the scenes; the arrows turn the model; A stops
/// the turning and R resets it; P switches the style, which device 9 sets
/// for the whole screen (holo has no device 3).
#[test]
fn keys() {
    let list = |script: &str, frames: u32| run(script, frames).0.vector3().unwrap().list().to_vec();
    // Space six times from the first: round the five scenes to the
    // second; Tab once more.
    let spaces = "2 press space\n3 press space\n4 press space\n5 press space\n6 press space\n7 press space\n";
    assert_eq!(list(spaces, 20), list("2 press 2\n", 20));
    assert_eq!(list(&format!("{spaces}8 press tab\n"), 20), list("2 press 3\n", 20));
    // A: the models stand still from frame 10 on.
    let (a, b) = (list("10 press a\n", 20), list("10 press a\n", 40));
    assert_eq!(a, b);
    assert_ne!(list("", 20), list("", 40));
    // Holding Right turns the model; R undoes the turn and the time.
    let turned = list("2 press a\n3 down right\n13 up right\n", 20);
    assert_ne!(turned, list("2 press a\n", 20));
    assert_eq!(list("3 down right\n13 up right\n30 press r\n32 press a\n", 40), list("2 press a\n", 20));
    // H: the wireframes solid (faces first, so the edges' commands are as
    // before); again, wire.
    let wire = list("2 press 1\n", 20);
    let solid = list("2 press 1\n3 press h\n", 20);
    assert!(!terse_core::vector3::opcodes(&wire).contains(&0x0C));
    assert_eq!(terse_core::vector3::opcodes(&solid).iter().filter(|&&o| o == 0x0C).count(), 6 + 20);
    assert_eq!(list("2 press 1\n3 press h\n4 press h\n", 20), wire);
    // P: phosphor, set through device 9.
    let (m, _) = run("5 press p\n", 10);
    assert!(m.vector().unwrap().phosphor());
    assert_eq!(m.vector().unwrap().persistence(), 200);
    assert!(m.vector_list().is_empty() && !m.lists_vector());
    golden("phosphor.txt", &screen(&m));
    let (m, _) = run("5 press p\n6 press p\n", 10);
    assert!(!m.vector().unwrap().phosphor());
}

/// `terse run --dump --dump-vectors` prints the same screen.
#[test]
fn cli_dump_matches() {
    let dir = tempfile::tempdir().unwrap();
    let events = dir.path().join("events.txt");
    std::fs::write(&events, "2 press 4\n").unwrap();
    let image = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/holo/holo.terse");
    let out = common::terse_command()
        .args(["run", image.to_str().unwrap(), "--events", events.to_str().unwrap(), "--frames", "90"])
        .args(["--dump", "--dump-vectors"])
        .arg("--store")
        .arg(dir.path().join("store"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let want = std::fs::read_to_string(common::programs().join("holo/expected/orrery.txt")).unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), want);
}

/// Every scene for 600 frames: each frame ends in wait_frame within its budget.
#[test]
fn budget() {
    let script = "1 press 1\n600 press 2\n1200 press 3\n1800 press 4\n2400 press 5\n3000 press 1\n3001 press h\n3600 press 5\n3601 press p\n";
    let (m, results) = run(script, 4200);
    assert!(results.iter().all(|r| r.stopped.is_none() && r.waited), "every frame ends in wait_frame");
    let used: Vec<u64> = results.iter().map(|r| r.used).collect();
    let stats = common::FrameStats::new(&used);
    const SCENES: [&str; 7] = ["wireframes", "rings", "starfield", "orrery", "city", "wireframes, solid", "city, phosphor"];
    let busiest_scene = SCENES[stats.busiest_frame / 600];
    eprintln!("holo: {} ({busiest_scene}); {} ROM bytes", stats.describe(), Image::parse(IMAGE).unwrap().rom.len());
    for (i, scene) in SCENES.iter().enumerate() {
        let s = common::FrameStats::new(&used[(600 * i).max(1)..600 * (i + 1)]);
        eprintln!("  {scene}: busiest {} units ({:.2}%), typical {} ({:.2}%)", s.busiest, common::pct(s.busiest), s.median, common::pct(s.median));
    }
    assert!(stats.busiest < 125_000 / 4, "{}", stats.describe());
    // The city's list (the longest) fits holo's 8,192-byte buffer.
    let city = m.vector3().unwrap().list().len();
    eprintln!("  city list: {city} bytes");
    assert!(city < 8192);
}

/// Without faces, the projection is what it was before faces existed: the
/// segments of the first four scenes from four eyes, hashed, as the
/// implementation before faces gave them.
#[test]
fn no_faces_same_segments() {
    let want: [(&str, (f32, f32), usize, u64); 16] = [
        ("1", (0.0, 0.0), 50, 0xad31110ae70592ef),
        ("1", (-160.0, 0.0), 50, 0x233cf4d61fa9d3db),
        ("1", (160.0, 90.0), 50, 0xab5a6cbfe5834842),
        ("1", (37.5, -120.25), 50, 0x88fce3e7c4d310f2),
        ("2", (0.0, 0.0), 389, 0xaeea617f6ea93481),
        ("2", (-160.0, 0.0), 389, 0x1157c54edcd639af),
        ("2", (160.0, 90.0), 389, 0x07eebf6eb3ea964b),
        ("2", (37.5, -120.25), 389, 0x62434411d629ff61),
        ("3", (0.0, 0.0), 218, 0x1bd850b558ac6db8),
        ("3", (-160.0, 0.0), 207, 0xd479a85291d07303),
        ("3", (160.0, 90.0), 201, 0xc6a0289d9385af17),
        ("3", (37.5, -120.25), 200, 0x4893635f3ee9d48a),
        ("4", (0.0, 0.0), 838, 0x96d610f9e155d221),
        ("4", (-160.0, 0.0), 838, 0x09ba55523892a517),
        ("4", (160.0, 90.0), 838, 0x89bb9f19248a7b45),
        ("4", (37.5, -120.25), 838, 0xc4207a4c7e77ffc7),
    ];
    for (key, eye, n, hash) in want {
        let (m, _) = run(&format!("2 press {key}\n"), 90);
        let mut out = Vec::new();
        terse_core::vector3::project(m.vector3().unwrap().list(), eye, &mut out);
        let mut h: u64 = 0xcbf29ce484222325;
        for b in format!("{out:?}").bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        assert_eq!((out.len(), h), (n, hash), "scene {key}, eye {eye:?}");
    }
}

/// The city: its faces hide parts of the ground and of other blocks, and
/// what is hidden depends on the eye.
#[test]
fn city_hides() {
    let (m, _) = run("2 press 5\n", 90);
    let list = m.vector3().unwrap().list();
    let p = terse_core::vector3::project(list, (0.0, 0.0), &mut Vec::new());
    assert_eq!(p.faces, 55);
    // Without its faces it would draw every edge whole.
    let edges: Vec<u8> = terse_core::vector3::decode(list)
        .filter(|c| c.opcode() != 0x0C)
        .flat_map(|c| encode(&c))
        .collect();
    let whole = terse_core::vector3::project(&edges, (0.0, 0.0), &mut Vec::new());
    assert_eq!(whole.length, p.length);
    assert!(p.segments < whole.segments, "{} of {}", p.segments, whole.segments);
    let (mut a, mut b) = (Vec::new(), Vec::new());
    terse_core::vector3::project(list, (-160.0, 0.0), &mut a);
    terse_core::vector3::project(list, (160.0, 0.0), &mut b);
    let cut = |v: &[terse_core::Primitive]| v.iter().filter(|p| matches!(p, terse_core::Primitive::Beam { butt, .. } if *butt != 0)).count();
    assert!(cut(&a) > 0 && cut(&b) > 0);
    assert_ne!(a, b);
}

/// A command's bytes (SPEC 13.3).
fn encode(c: &terse_core::vector3::Command) -> Vec<u8> {
    use terse_core::vector3::Command;
    let xyz = |p: &[i16; 3]| p.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
    let mut v = vec![c.opcode()];
    match c {
        Command::Colour(rgba) => v.extend(rgba),
        Command::Width(w) | Command::Circle(w) | Command::Scale(w) => v.extend(w.to_le_bytes()),
        Command::Move(p) | Command::Line(p) | Command::Translate(p) => v.extend(xyz(p)),
        Command::Rotate(axis, a) => {
            v.push(*axis);
            v.extend(a.to_le_bytes());
        }
        Command::Face(n, corners) => {
            v.push(*n);
            corners[..*n as usize].iter().for_each(|p| v.extend(xyz(p)));
        }
        Command::Point | Command::Push | Command::Pop => {}
    }
    v
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(IMAGE, &["holo/holo.c"], true, "holo", RAM_PAGES, &[(2, 0), (9, 0)]);
}

/// How long the city's projection takes (run with `--release --ignored
/// --nocapture`): its list at frame 90 from an eye that moves every time.
#[test]
#[ignore]
fn city_projection_time() {
    for (key, name) in [("5", "city"), ("1\n3 press h", "wireframes, solid"), ("2", "rings")] {
        let (m, _) = run(&format!("2 press {key}\n"), 90);
        let list = m.vector3().unwrap().list();
        let n = 2000;
        let mut out = Vec::new();
        let start = std::time::Instant::now();
        for i in 0..n {
            out.clear();
            let a = i as f32 / 120.0 * std::f32::consts::TAU;
            std::hint::black_box(terse_core::vector3::project(list, (160.0 * a.cos(), 80.0 * a.sin()), &mut out));
        }
        eprintln!("{name}: {} segments drawn, {:.1} µs a projection", out.len(), start.elapsed().as_secs_f64() * 1e6 / n as f64);
    }
}
