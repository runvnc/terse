//! programs/conform: the conformance image (SPEC 13). The state hash
//! (STATE.md) every 10 frames of the scripted run must match
//! `expected-hashes.txt`, the final screen (terminal and decoded display list)
//! `expected-screen.txt`, and every section the program checks itself must
//! pass. `UPDATE_GOLDEN=1` rewrites the expected files.
//!
//! The image is built from C and assembly (`programs/conform/build.sh`);
//! `conform_image_is_up_to_date` rebuilds it and compares, and is skipped
//! when clang cannot target riscv32 or ld.lld or llvm-objcopy is missing.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const TERSE: &str = env!("CARGO_BIN_EXE_terse");
/// The scripted run: every phase, the stress phase included, ends by frame 110.
const FRAMES: &str = "200";
/// PASS lines on the results screen: one per section.
const SECTIONS: usize = 26;

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

/// Whether the RISC-V tools build.sh uses are here.
fn have_tools(tmp: &Path) -> bool {
    let src = tmp.join("probe.c");
    std::fs::write(&src, "int x;\n").unwrap();
    let clang = std::env::var("CLANG").unwrap_or_else(|_| "clang".into());
    let ok = |cmd: &mut Command| cmd.stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    ok(Command::new(&clang)
        .args(["--target=riscv32-unknown-elf", "-march=rv32imc", "-mabi=ilp32", "-c"])
        .arg(&src)
        .arg("-o")
        .arg(tmp.join("probe.o")))
        && ok(Command::new(std::env::var("LLD").unwrap_or_else(|_| "ld.lld".into())).arg("--version"))
        && ok(Command::new(std::env::var("OBJCOPY").unwrap_or_else(|_| "llvm-objcopy".into())).arg("--version"))
}

#[test]
fn conform_image_is_up_to_date() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    if !have_tools(tmp.path()) {
        eprintln!("skipped: clang (riscv32), ld.lld or llvm-objcopy not found");
        return;
    }
    let out = Command::new("sh")
        .arg(path("build.sh"))
        .arg(tmp.path())
        .env("TERSE", TERSE)
        .output()
        .unwrap();
    assert!(out.status.success(), "build.sh: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let built = std::fs::read(tmp.path().join("conform.terse")).unwrap();
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
    assert_eq!(screen.matches("PASS ").count(), SECTIONS, "{screen}");
    golden("expected-screen.txt", &screen);
}
