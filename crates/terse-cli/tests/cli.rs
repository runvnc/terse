//! The `terse` command line: info, run (dump, exit code, fault, scripted
//! events), pack, and the blob store without a relay.

use std::path::{Path, PathBuf};
use std::process::Command;

const TERSE: &str = env!("CARGO_BIN_EXE_terse");

fn hello_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/hello")
}

fn terse(args: &[&str]) -> std::process::Output {
    Command::new(TERSE).args(args).output().unwrap()
}

/// Runs `terse` and requires success.
fn terse_ok(args: &[&str]) -> std::process::Output {
    let out = terse(args);
    assert!(out.status.success(), "terse {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    out
}

#[test]
fn put_get_and_run_by_id_without_a_relay() {
    let store = tempfile::tempdir().unwrap();
    let store = store.path().to_str().unwrap();
    let image = hello_dir().join("hello.terse");
    let out = terse_ok(&["put", image.to_str().unwrap(), "--store", store]);
    let id = String::from_utf8(out.stdout).unwrap().trim().to_string();
    let out = terse_ok(&["info", &id, "--store", store]);
    assert!(String::from_utf8(out.stdout).unwrap().contains(&format!("id:        {id}")));
    let out = terse(&["run", &id, "--store", store, "--frames", "3", "--dump"]);
    assert!(String::from_utf8(out.stdout).unwrap().contains("héllo, wörld — 你好"));
    let out = terse_ok(&["get", &id, "--store", store]);
    assert!(out.stdout == std::fs::read(&image).unwrap());

    let missing = "00".repeat(32);
    let out = terse(&["get", &missing, "--store", store]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found"));
    let out = terse(&["put", "/dev/null", "--store", store]);
    assert!(!out.status.success());
}

#[test]
fn info_and_run() {
    let image = hello_dir().join("hello.terse");
    let out = terse_ok(&["info", image.to_str().unwrap()]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("title:     hello"), "{text}");
    assert!(text.contains("devices:   2:0"), "{text}");
    assert!(text.contains("profile:   1\n"), "{text}");
    assert!(text.contains("ram_pages: 1 (8 KB)"), "{text}");

    let out = terse(&["run", image.to_str().unwrap(), "--frames", "3", "--dump"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 30);
    assert!(text.contains("héllo, wörld — 你好"));
    assert!(text.contains("frame 00000002"));
}

/// Packs a raw ROM into an image in `dir`; returns its path.
fn pack(dir: &Path, name: &str, rom: &[u8]) -> PathBuf {
    let bin = dir.join(format!("{name}.bin"));
    let image = dir.join(format!("{name}.terse"));
    std::fs::write(&bin, rom).unwrap();
    terse_ok(&["pack", "--rom", bin.to_str().unwrap(), "--title", name, "--ram-pages", "1", "-o", image.to_str().unwrap()]);
    image
}

#[test]
fn run_exit_code_is_the_machines() {
    let dir = tempfile::tempdir().unwrap();
    // c.li a0, 7; li a7, 106h (exit); ecall; c.j .
    let rom = [0x1D, 0x45, 0x93, 0x08, 0x60, 0x10, 0x73, 0x00, 0x00, 0x00, 0x01, 0xA0];
    let image = pack(dir.path(), "exit7", &rom);
    let out = terse(&["run", image.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(7));
}

/// A fault (SPEC 1.5) is printed and ends `terse run` by SIGTERM, which no
/// exit code can be mistaken for.
#[test]
fn a_fault_ends_run_by_sigterm() {
    use std::os::unix::process::ExitStatusExt;
    let dir = tempfile::tempdir().unwrap();
    // c.nop, then c.unimp (0000h): an illegal instruction.
    let image = pack(dir.path(), "fault", &[0x01, 0x00, 0x00, 0x00]);
    let out = terse(&["run", image.to_str().unwrap(), "--frames", "5"]);
    assert_eq!(out.status.code(), None);
    assert_eq!(out.status.signal(), Some(15), "{:?}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("terse: fault: illegal instruction 0000 at pc 00010002"), "{err}");
}

#[test]
fn pack_refuses_bad_headers() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("x.bin");
    std::fs::write(&bin, [0x01, 0x00]).unwrap();
    let out_path = dir.path().join("x.terse");
    let (bin, out_path) = (bin.to_str().unwrap(), out_path.to_str().unwrap());
    let base = ["pack", "--rom", bin, "--title", "x", "-o", out_path];
    for (extra, why) in [
        (&["--ram-pages", "0"][..], "RAM pages 0"),
        (&["--ram-pages", "129"][..], "RAM pages 129"),
        (&["--ram-pages", "1", "--device", "9:0"][..], "device 9 version 0"),
    ] {
        let out = terse(&[&base[..], extra].concat());
        assert!(!out.status.success(), "{extra:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains(why), "{extra:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let out = terse(&["pack", "--rom", bin, "--title", "a title over sixteen bytes", "--ram-pages", "1", "-o", out_path]);
    assert!(!out.status.success());
}

/// `--events`: scripted input for `terse run`.
#[test]
fn run_with_scripted_events() {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let events = tmp.join("hello-events.txt");
    std::fs::write(&events, "# type, then leave\n5 text Hi 你好\n6 press enter\n8 down esc\n").unwrap();
    let image = hello_dir().join("hello.terse");
    let out = terse(&["run", image.to_str().unwrap(), "--events", events.to_str().unwrap(), "--frames", "30", "--dump"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Hi 你好"), "{text}");
    assert!(text.contains("frame 00000008"), "exited at frame 8: {text}");
    std::fs::write(&events, "5 jump\n").unwrap();
    let out = terse(&["run", image.to_str().unwrap(), "--events", events.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("line 1: unknown command"));
}
