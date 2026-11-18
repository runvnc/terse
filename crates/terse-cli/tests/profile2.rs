//! `terse pack --profile 2`, `info` and `run` with profile-2 images
//! (RV-PROFILE.md): the header picks the machine, and a fault is reported.

use std::path::Path;
use std::process::Command;

const TERSE: &str = env!("CARGO_BIN_EXE_terse");

fn workspace() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn terse(args: &[&str]) -> std::process::Output {
    Command::new(TERSE).args(args).output().unwrap()
}

#[test]
fn info_and_run_a_profile_2_image() {
    let hello = workspace().join("programs/rv/hello/hello.terse");
    let out = terse(&["info", hello.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("profile:   2\n"));
    let out = terse(&["run", hello.to_str().unwrap(), "--frames", "30", "--dump"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let screen = String::from_utf8(out.stdout).unwrap();
    assert!(screen.starts_with(" Terse · hello\n"), "{screen}");
    assert!(screen.contains("frame 0000001D"));
}

#[test]
fn pack_profile_2_and_report_a_fault() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("rom.bin");
    let image = dir.path().join("t.terse");
    // c.nop, then c.unimp (0000h): an illegal instruction.
    std::fs::write(&rom, [0x01, 0x00, 0x00, 0x00]).unwrap();
    let (rom, image) = (rom.to_str().unwrap(), image.to_str().unwrap());
    let out = terse(&["pack", "--profile", "2", "--rom", rom, "--title", "t", "--ram-pages", "4", "-o", image]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = terse(&["info", image]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("profile:   2\n"));
    let out = terse(&["run", image, "--frames", "5"]);
    assert_eq!(out.status.code(), Some(255));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("fault: illegal instruction 0000 at pc 00010002, address 00010002"), "{err}");
    let out = terse(&["pack", "--profile", "3", "--rom", rom, "--title", "t", "--ram-pages", "4", "-o", image]);
    assert!(!out.status.success());
    // Without --profile the image is profile 1 (and the same bytes are a Z80 program).
    let out = terse(&["pack", "--rom", rom, "--title", "t", "--ram-pages", "4", "-o", image]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&terse(&["info", image]).stdout).contains("profile:   1\n"));
}
