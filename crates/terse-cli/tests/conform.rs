//! programs/conform: the conformance image. The state hash (STATE.md) every
//! 10 frames of the scripted run must match `expected-hashes.txt`, the final
//! screen (terminal and decoded display list) `expected-screen.txt`, and every
//! section the program checks itself must pass. `UPDATE_GOLDEN=1` rewrites
//! the expected files.

use std::path::{Path, PathBuf};
use std::process::Command;

const TERSE: &str = env!("CARGO_BIN_EXE_terse");
/// The scripted run: every phase, the stress phase included, ends by frame 150.
const FRAMES: &str = "200";

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/conform")
}

fn path(name: &str) -> String {
    dir().join(name).to_str().unwrap().to_string()
}

/// `terse run conform.terse --events conform.events` with an empty blob store
/// and no subscriptions, as README "Conformance image" describes.
fn run(extra: &[&str]) -> String {
    let store = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let out = Command::new(TERSE)
        .args(["run", &path("conform.terse"), "--events", &path("conform.events"), "--frames", FRAMES])
        .args(["--store", store.path().to_str().unwrap(), "--config", config.path().to_str().unwrap()])
        .args(extra)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn golden(name: &str, got: &str) {
    let file = dir().join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&file, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    if want != got {
        let first = want.lines().zip(got.lines()).position(|(a, b)| a != b);
        panic!("{name} differs (first differing line {first:?}); got:\n{got}");
    }
}

#[test]
fn conform_image_is_up_to_date() {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let (bin, image) = (tmp.join("conform.bin"), tmp.join("conform.terse"));
    let ok = |args: &[&str]| {
        let out = Command::new(TERSE).args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    ok(&["asm", &path("conform.asm"), "-o", bin.to_str().unwrap()]);
    ok(&[
        "pack", "--rom", bin.to_str().unwrap(), "--title", "conform", "--ram-pages", "16", "--device", "2:0",
        "--device", "3:0", "--device", "4:0", "-o", image.to_str().unwrap(),
    ]);
    let built = std::fs::read(&image).unwrap();
    let checked_in = std::fs::read(dir().join("conform.terse")).unwrap();
    assert!(built == checked_in, "programs/conform/conform.terse is stale; run programs/conform/build.sh");
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
    assert_eq!(screen.matches("PASS ").count(), 25, "{screen}");
    golden("expected-screen.txt", &screen);
}
