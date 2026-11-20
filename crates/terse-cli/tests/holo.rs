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
/// On the device bus too (common/twin.rs), the same hash every frame.
fn run(script: &str, frames: u32) -> (Machine, Vec<FrameResult>) {
    let image = Image::parse(IMAGE).unwrap();
    let mut m = Machine::new(&image);
    let mut h = Headless::new(None, Script::parse(script).unwrap());
    let mut twin = common::twin::Twin::start(twin_setup(&image, script));
    let results = (0..frames)
        .map(|_| {
            let r = h.frame(&mut m);
            if let Some(t) = &mut twin {
                t.follow_machine(&m);
            }
            r
        })
        .collect();
    (m, results)
}

/// holo as a single machine with no content device, for the bus twin.
fn twin_setup<'a>(image: &'a Image, script: &'a str) -> common::twin::Setup<'a> {
    common::twin::Setup {
        image,
        shell: None,
        identity: None,
        store: None,
        data: None,
        subscriptions: Vec::new(),
        seed: None,
        consent: terse_net::Consent::Deny,
        relays: Vec::new(),
        script,
        content: false,
        announced: false,
    }
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("holo/expected/{name}"), got);
}

/// Each scene after a second and a half of turning.
#[test]
fn scenes() {
    for (key, name) in [("1", "wireframes"), ("2", "rings"), ("3", "starfield"), ("4", "orrery"), ("5", "city"), ("6", "cockpit"), ("1\n3 press h", "wireframes-solid")] {
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
    // Space seven times from the first: round the six scenes to the
    // second; Tab twice more (the orrery: what the starfield shows depends on
    // how long the stars moved, and the cockpit moves only some).
    let spaces = "2 press space\n3 press space\n4 press space\n5 press space\n6 press space\n7 press space\n8 press space\n";
    assert_eq!(list(spaces, 20), list("2 press 2\n", 20));
    assert_eq!(list(&format!("{spaces}9 press tab\n10 press tab\n"), 20), list("2 press 4\n", 20));
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
    assert!(m.vector_list().is_empty() && !m.has_vector());
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
    let script = "1 press 1\n600 press 2\n1200 press 3\n1800 press 4\n2400 press 5\n3000 press 1\n3001 press h\n3600 press 5\n3601 press p\n4200 press p\n4201 press 6\n4800 press p\n";
    let (m, results) = run(script, 5400);
    assert!(results.iter().all(|r| r.stopped.is_none() && r.waited), "every frame ends in wait_frame");
    let used: Vec<u64> = results.iter().map(|r| r.used).collect();
    let stats = common::FrameStats::new(&used);
    const SCENES: [&str; 9] =
        ["wireframes", "rings", "starfield", "orrery", "city", "wireframes, solid", "city, phosphor", "cockpit", "cockpit, phosphor"];
    let busiest_scene = SCENES[stats.busiest_frame / 600];
    eprintln!("holo: {} ({busiest_scene}); {} ROM bytes", stats.describe(), Image::parse(IMAGE).unwrap().rom.len());
    for (i, scene) in SCENES.iter().enumerate() {
        let s = common::FrameStats::new(&used[(600 * i).max(1)..600 * (i + 1)]);
        eprintln!("  {scene}: busiest {} units ({:.2}%), typical {} ({:.2}%)", s.busiest, common::pct(s.busiest), s.median, common::pct(s.median));
    }
    assert!(stats.busiest < 125_000 / 4, "{}", stats.describe());
    // The city's list (the longest) and the cockpit's fit holo's 8,192-byte
    // buffer.
    let cockpit = m.vector3().unwrap().list().len();
    let city = run("2 press 5\n", 90).0.vector3().unwrap().list().len();
    eprintln!("  city list: {city} bytes, cockpit list: {cockpit} bytes");
    assert!(city < 8192 && cockpit < 8192);
}

/// The cockpit: three consoles, each a Surface showing a region of the
/// terminal (the instruments, the log, the list), in effect and within the
/// caps; the regions are what the reference eye sees on the consoles, and
/// the status line stays on the glass.
#[test]
fn cockpit_surfaces() {
    let (m, _) = run("2 press 6\n", 90);
    let list = m.vector3().unwrap().list();
    let (panels, p) = terse_core::vector3::surfaces(list, (0.0, 0.0));
    assert_eq!(p.surface_rects(), [[8, 64, 224, 128], [240, 64, 224, 128], [472, 64, 160, 128]]);
    assert!(!p.capped && panels.len() == 3 && panels.iter().all(|p| p.front));
    // The consoles lie below the window, on the lower half of the screen,
    // tilted: their top edges further away than their bottom ones.
    for q in &panels {
        assert!(q.corners.iter().all(|c| c[1] > 240.0), "{q:?}");
        assert!(q.corners[0][2] > q.corners[3][2] && q.corners[1][2] > q.corners[2][2], "{q:?}");
    }
    let text = m.terminal().unwrap().text();
    let row = |r: usize| text.lines().nth(r).unwrap().to_string();
    assert!(row(1).contains("COCKPIT  bank +000  pitch +000  target GAMMA"), "{text}");
    assert!(row(5).contains("HEADING") && row(5).contains("ALPHA"), "{text}");
    // The readouts change every frame.
    let (later, _) = run("2 press 6\n", 91);
    assert_ne!(later.terminal().unwrap().text(), text);
}

/// A click on a row of the list on the tilted right-hand console reaches
/// holo as a pointer inside the list's region of the terminal (SPEC 13.3
/// Surface), from any eye: the row is selected. The press is found by the
/// core's own mapping, then checked by running holo as a single machine
/// in a system, which routes it.
#[test]
fn cockpit_click_through_a_console() {
    let image = Image::parse(IMAGE).unwrap();
    for eye in [(0.0f32, 0.0f32), (-160.0, 0.0), (160.0, 80.0)] {
        let (m, _) = run("2 press 6\n", 59);
        let list = m.vector3().unwrap().list().to_vec();
        // A glass position over DELTA (the list's row 4, screen y 128-143,
        // x 472-631) on its console.
        let over = |x: u16, y: u16| {
            let (sx, sy) = terse_core::vector3::surface_pointer(&list, eye, x, y);
            (sx, sy) != (x, y) && (480..600).contains(&sx) && (130..142).contains(&sy)
        };
        let (x, y) = (240..480u16).flat_map(|y| (320..640u16).map(move |x| (x, y))).find(|&(x, y)| over(x, y)).expect("DELTA on its console");
        assert!(y > 300, "on the console, below the window: ({x}, {y})");
        let script = format!("2 press 6\n60 pointer {x} {y} 1\n61 pointer {x} {y} 0\n");
        let mut system = terse_core::System::new(&image, false, Vec::new());
        system.set_eye(eye);
        let mut h = terse_net::HeadlessSystem::new(None, Script::parse(&script).unwrap());
        let mut twin = common::twin::Twin::start(twin_setup(&image, &script));
        if let Some(t) = &mut twin {
            t.set_eye(eye);
        }
        for _ in 0..90 {
            h.frame(&mut system);
            if let Some(t) = &mut twin {
                t.follow(&system);
            }
        }
        let got = screen(system.shell());
        assert!(got.contains(">DELTA") && got.contains("target DELTA"), "eye {eye:?}, click at ({x}, {y}):\n{got}");
        if eye == (0.0, 0.0) {
            golden("cockpit-clicked.txt", &format!("-- clicked at ({x}, {y}) --\n{got}"));
        }
        // Over no Surface (the window): delivered as it is, not on the list.
        let unmapped = "2 press 6\n60 pointer 320 200 1\n61 pointer 320 200 0\n";
        let mut system = terse_core::System::new(&image, false, Vec::new());
        system.set_eye(eye);
        let mut h = terse_net::HeadlessSystem::new(None, Script::parse(unmapped).unwrap());
        for _ in 0..90 {
            h.frame(&mut system);
        }
        assert!(screen(system.shell()).contains(">GAMMA"), "eye {eye:?}");
    }
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
        Command::Rotate(axis, a) | Command::Screen(axis, a) => {
            v.push(*axis);
            v.extend(a.to_le_bytes());
        }
        Command::Face(n, corners) => {
            v.push(*n);
            corners[..*n as usize].iter().for_each(|p| v.extend(xyz(p)));
        }
        Command::Surface(r, w) => {
            r.iter().for_each(|x| v.extend(x.to_le_bytes()));
            v.extend(w.to_le_bytes());
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
