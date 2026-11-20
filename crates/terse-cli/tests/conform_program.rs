//! programs/conform-program: the conformance image of the machine's program
//! profile (SPEC 2.1, 15), programs/conform's sources built with
//! `-DPROGRAM`: devices 2 and 3 only, and a section for the `device` call.
//! Its state hash every 10 frames of the scripted run must match
//! `expected-hashes.txt`, the final screen and display list
//! `expected-screen.txt`, and every section must pass, on a full host and
//! on the program profile (`terse run --profile program`) alike, with the
//! devices compiled in and on the device bus. `UPDATE_GOLDEN=1` rewrites
//! the expected files.

mod common;

use std::path::PathBuf;
use std::process::Command;

const FRAMES: &str = "200";
/// PASS lines on the results screen: programs/conform's sections less the
/// 3D display, content and the page, plus the `device` section.
const SECTIONS: usize = 22;

fn dir() -> PathBuf {
    common::programs().join("conform-program")
}

fn path(name: &str) -> String {
    dir().join(name).to_str().unwrap().to_string()
}

/// `terse run conform-program.terse --events conform-program.events`: no
/// store, config or data directory is touched (the image has no content
/// device).
fn run(extra: &[&str]) -> String {
    let out = common::terse_command()
        .args(["run", &path("conform-program.terse"), "--events", &path("conform-program.events"), "--frames", FRAMES])
        .args(extra)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("conform-program/{name}"), got);
}

#[test]
fn conform_program_image_is_up_to_date() {
    let image = std::fs::read(dir().join("conform-program.terse")).unwrap();
    let parsed = terse_core::Image::parse(&image).unwrap();
    assert_eq!(parsed.title, b"conform-program");
    let devices: Vec<(u8, u8, bool)> = parsed.devices.iter().map(|d| (d.slot, d.version, d.optional)).collect();
    assert_eq!(devices, [(2, 0, false), (3, 0, false)]);
    if !common::have_tools() {
        return;
    }
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let out = Command::new("sh").arg(path("build.sh")).arg(tmp.path()).env("TERSE", env!("CARGO_BIN_EXE_terse")).output().unwrap();
    assert!(out.status.success(), "build.sh: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let built = std::fs::read(tmp.path().join("conform-program.terse")).unwrap();
    assert!(built == image, "programs/conform-program/conform-program.terse is stale; run programs/conform-program/build.sh");
}

/// The events are programs/conform's: conform.c checks them.
#[test]
fn events_are_conforms() {
    let body = |p: PathBuf| std::fs::read_to_string(p).unwrap().lines().skip_while(|l| l.starts_with('#')).collect::<Vec<_>>().join("\n");
    assert_eq!(body(dir().join("conform-program.events")), body(common::programs().join("conform/conform.events")));
}

/// The authoritative check: state hashes every 10 frames.
#[test]
fn state_hashes_match() {
    let got = run(&["--hash-every", "10"]);
    assert_eq!(got.lines().count(), 20);
    golden("expected-hashes.txt", &got);
}

/// The sections the program checks itself all pass, and the final screen
/// and display list are as expected.
#[test]
fn every_section_passes() {
    let screen = run(&["--dump", "--dump-vectors"]);
    assert!(screen.contains("All sections pass."), "{screen}");
    assert!(!screen.contains("FAIL"), "{screen}");
    assert_eq!(screen.matches("PASS ").count(), SECTIONS, "{screen}");
    assert!(screen.contains("PASS device: the devices it has"), "{screen}");
    golden("expected-screen.txt", &screen);
}

/// On the program profile: the same hashes and the same screen (the image
/// asks for nothing outside it).
#[test]
fn the_same_on_the_program_profile() {
    let hashes = run(&["--hash-every", "10", "--profile", "program"]);
    assert_eq!(hashes, std::fs::read_to_string(dir().join("expected-hashes.txt")).unwrap());
    let screen = run(&["--dump", "--dump-vectors", "--profile", "program"]);
    assert_eq!(screen, std::fs::read_to_string(dir().join("expected-screen.txt")).unwrap());
}

/// The program profile refuses what is outside it: an image that requires
/// content, the page or the 3D display, and a system.
#[test]
fn the_program_profile_refuses_other_devices() {
    for (program, device) in [("reader", "4"), ("holo", "9"), ("fetch", "4"), ("tune", "6")] {
        let image = common::programs().join(program).join(format!("{program}.terse"));
        let out = common::terse_command().args(["run", image.to_str().unwrap(), "--profile", "program", "--frames", "1"]).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{program}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(&format!("requires device {device}, which the program profile does not provide (it has devices 1, 2, 3)")), "{program}: {err}");
    }
    let out = common::terse_command().args(["run", "--shell", "--profile", "program", "--frames", "1"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("the program profile has no device 8"));
}
