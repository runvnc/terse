//! programs/conform-reading: the conformance image of the machine's reading
//! profile (SPEC §conformance), programs/conform's sources built with `-DREADING`:
//! devices 2, 4 and 7 only, no optional content operation. Its state hash
//! every 10 frames of the scripted run must match `expected-hashes.txt`,
//! the final screen and page `expected-screen.txt`, and every section must
//! pass, on a full host and on the reading profile (`terse run --profile
//! reading`) alike. The host holds the two blobs of `blobs/` (the bytes
//! programs/conform puts), as README "Conformance image" says.
//! `UPDATE_GOLDEN=1` rewrites the expected files.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

const FRAMES: &str = "200";
/// PASS lines on the results screen: programs/conform's sections less the
/// two of the vector displays.
const SECTIONS: usize = 27;

fn dir() -> PathBuf {
    common::programs().join("conform-reading")
}

fn path(name: &str) -> String {
    dir().join(name).to_str().unwrap().to_string()
}

/// `terse run conform-reading.terse --events conform-reading.events` with a
/// store holding the blobs of `blobs/` and no subscriptions.
fn run(extra: &[&str]) -> String {
    let store = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = store.path().to_str().unwrap();
    for blob in std::fs::read_dir(dir().join("blobs")).unwrap() {
        let blob = blob.unwrap().path();
        let out = common::terse_command().args(["put", blob.to_str().unwrap(), "--store", store, "--relay", "none"]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    let out = common::terse_command()
        .args(["run", &path("conform-reading.terse"), "--events", &path("conform-reading.events"), "--frames", FRAMES])
        .args(["--store", store, "--config", config.path().to_str().unwrap(), "--data", data.path().to_str().unwrap()])
        .args(extra)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("conform-reading/{name}"), got);
}

#[test]
fn conform_reading_image_is_up_to_date() {
    let image = std::fs::read(dir().join("conform-reading.terse")).unwrap();
    let parsed = terse_core::Image::parse(&image).unwrap();
    assert_eq!(parsed.title, b"conform-reading");
    let devices: Vec<(u8, u8)> = parsed.devices.iter().map(|d| (d.slot, d.version)).collect();
    assert_eq!(devices, [(2, 0), (4, 0), (7, 0)]);
    if !common::have_tools() {
        return;
    }
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let out = Command::new("sh")
        .arg(path("build.sh"))
        .arg(tmp.path())
        .env("TERSE", env!("CARGO_BIN_EXE_terse"))
        .output()
        .unwrap();
    assert!(out.status.success(), "build.sh: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let built = std::fs::read(tmp.path().join("conform-reading.terse")).unwrap();
    assert!(built == image, "programs/conform-reading/conform-reading.terse is stale; run programs/conform-reading/build.sh");
}

/// The blobs are the bytes programs/conform puts (conform.c put_id and
/// rom_put_id).
#[test]
fn blobs_are_what_conform_puts() {
    let put: Vec<u8> = (0..100u32).map(|i| (i * 7 + 3) as u8).collect();
    assert_eq!(std::fs::read(dir().join("blobs/put-100.bin")).unwrap(), put);
    assert_eq!(std::fs::read(dir().join("blobs/put-rom.txt")).unwrap(), b"Terse conformance image: a blob put from ROM.\n");
    assert_eq!(std::fs::read_dir(dir().join("blobs")).unwrap().count(), 2);
    let ids = |p: &Path| terse_net::to_hex(&terse_net::blob_id(&std::fs::read(p).unwrap()));
    assert_eq!(ids(&dir().join("blobs/put-100.bin")), "5203b152256ce5a2aea12f124e3bd9def9ebe212b49c2751237739f630ea512a");
}

/// The authoritative check: state hashes every 10 frames.
#[test]
fn state_hashes_match() {
    let got = run(&["--hash-every", "10"]);
    assert_eq!(got.lines().count(), 20);
    golden("expected-hashes.txt", &got);
}

/// The sections the program checks itself all pass, and the final screen
/// and page are as expected.
#[test]
fn every_section_passes() {
    let screen = run(&["--dump", "--dump-page"]);
    assert!(screen.contains("All sections pass."), "{screen}");
    assert!(!screen.contains("FAIL"), "{screen}");
    assert_eq!(screen.matches("PASS ").count(), SECTIONS, "{screen}");
    golden("expected-screen.txt", &screen);
}

/// On the reading profile: the same hashes and the same screen (the image
/// asks for nothing outside it).
#[test]
fn the_same_on_the_reading_profile() {
    let hashes = run(&["--hash-every", "10", "--profile", "reading"]);
    let want = std::fs::read_to_string(dir().join("expected-hashes.txt")).unwrap();
    assert_eq!(hashes, want);
    let screen = run(&["--dump", "--dump-page", "--profile", "reading"]);
    assert_eq!(screen, std::fs::read_to_string(dir().join("expected-screen.txt")).unwrap());
}
