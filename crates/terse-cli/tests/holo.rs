//! programs/holo/holo.terse: golden screens of each scene (the terminal
//! text and the 3D display list with what the reference eye sees), the
//! keys, the display style it sets through device 9, the frame budget, and
//! the image is up to date. Set UPDATE_GOLDEN=1 to rewrite
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
    for (key, name) in [("1", "wireframes"), ("2", "rings"), ("3", "starfield"), ("4", "orrery")] {
        let (m, results) = run(&format!("2 press {key}\n"), 90);
        assert!(results.iter().all(|r| r.stopped.is_none() && r.waited));
        let got = screen(&m);
        assert!(got.contains(&name.to_uppercase()), "{got}");
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
    // Space five times from the first: the second scene; Tab once more.
    let spaces = "2 press space\n3 press space\n4 press space\n5 press space\n6 press space\n";
    assert_eq!(list(spaces, 20), list("2 press 2\n", 20));
    assert_eq!(list(&format!("{spaces}7 press tab\n"), 20), list("2 press 3\n", 20));
    // A: the models stand still from frame 10 on.
    let (a, b) = (list("10 press a\n", 20), list("10 press a\n", 40));
    assert_eq!(a, b);
    assert_ne!(list("", 20), list("", 40));
    // Holding Right turns the model; R undoes the turn and the time.
    let turned = list("2 press a\n3 down right\n13 up right\n", 20);
    assert_ne!(turned, list("2 press a\n", 20));
    assert_eq!(list("3 down right\n13 up right\n30 press r\n32 press a\n", 40), list("2 press a\n", 20));
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
    let script = "1 press 1\n600 press 2\n1200 press 3\n1800 press 4\n2400 press p\n";
    let (m, results) = run(script, 3000);
    assert!(results.iter().all(|r| r.stopped.is_none() && r.waited), "every frame ends in wait_frame");
    let used: Vec<u64> = results.iter().map(|r| r.used).collect();
    let stats = common::FrameStats::new(&used);
    let busiest_scene = ["wireframes", "rings", "starfield", "orrery", "orrery, phosphor"][stats.busiest_frame / 600];
    eprintln!("holo: {} ({busiest_scene}); {} ROM bytes", stats.describe(), Image::parse(IMAGE).unwrap().rom.len());
    for (i, scene) in ["wireframes", "rings", "starfield", "orrery", "orrery, phosphor"].iter().enumerate() {
        let s = common::FrameStats::new(&used[(600 * i).max(1)..600 * (i + 1)]);
        eprintln!("  {scene}: busiest {} units ({:.2}%), typical {} ({:.2}%)", s.busiest, common::pct(s.busiest), s.median, common::pct(s.median));
    }
    assert!(stats.busiest < 125_000 / 4, "{}", stats.describe());
    assert!(m.vector3().unwrap().list().len() < 4096);
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(IMAGE, &["holo/holo.c"], true, "holo", RAM_PAGES, &[(2, 0), (9, 0)]);
}
