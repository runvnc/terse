//! The `terse` command line: info, run (dump, exit code, fault, scripted
//! events), pack, and the blob store without a relay.

mod common;

use std::path::{Path, PathBuf};


fn hello_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/hello")
}

fn terse(args: &[&str]) -> std::process::Output {
    common::terse_command().args(args).output().unwrap()
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

/// A fault (SPEC §stopping) is printed and ends `terse run` by SIGTERM, which no
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
        (&["--ram-pages", "1", "--device", "11:0"][..], "device 11 version 0"),
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

/// `--hash-every N` prints `<counter> <hash>` after every frame that leaves
/// the frame counter at a multiple of N: from reset, after N, 2N, ...
/// frames; from a snapshot, by the counter, not by frames run.
#[test]
fn hash_every_counts_by_the_frame_counter() {
    let dir = tempfile::tempdir().unwrap();
    let snap = dir.path().join("s").to_str().unwrap().to_string();
    let image = hello_dir().join("hello.terse");
    let image = image.to_str().unwrap();
    let out = String::from_utf8(terse_ok(&["run", image, "--frames", "7", "--hash-every", "3", "--save-state", &snap]).stdout).unwrap();
    let counters: Vec<&str> = out.lines().map(|l| l.split(' ').next().unwrap()).collect();
    assert_eq!(counters, ["3", "6"]);
    // The 7th frame left the counter at 7: two more frames reach 9.
    let out = String::from_utf8(terse_ok(&["run", image, "--load-state", &snap, "--frames", "4", "--hash-every", "3"]).stdout).unwrap();
    let counters: Vec<&str> = out.lines().map(|l| l.split(' ').next().unwrap()).collect();
    assert_eq!(counters, ["9"]);
    // The hash after 30 frames (STATE.md 3).
    let out = String::from_utf8(terse_ok(&["run", image, "--frames", "30", "--hash-every", "30"]).stdout).unwrap();
    assert_eq!(out, "30 cb3d8c6473a9b608c48e77db48c7f19724fbe2c354094c837f8b79fb8b923c9d\n");
}

/// `--hash-all`, after the last frame only: the shell's hash, then
/// `<handle> <hash>` for each machine the shell has, `-` while it has no
/// machine (its image is fetched between frames: a machine started in
/// frame 2 is loaded at the start of frame 3).
#[test]
fn hash_all_lists_the_shells_machines() {
    let dir = tempfile::tempdir().unwrap();
    let p = |n: &str| dir.path().join(n).to_str().unwrap().to_string();
    std::fs::write(p("ev"), "2 press enter\n").unwrap();
    let run = |frames: &str| {
        let out = terse_ok(&[
            "run", "--shell", "--frames", frames, "--hash-all", "--hash-every", "1", "--events", &p("ev"), "--store", &p("store"),
            "--config", &p("config"), "--data", &p("data"),
        ]);
        String::from_utf8(out.stdout).unwrap()
    };
    let lines = |s: &str| s.lines().map(str::to_string).collect::<Vec<_>>();
    let two = lines(&run("2"));
    assert_eq!(two.len(), 3, "two frames' hashes, then the shell's: {two:?}");
    let three = lines(&run("3"));
    assert_eq!((three.len(), three[4].as_str()), (5, "1 -"), "{three:?}");
    let four = lines(&run("4"));
    assert_eq!(four.len(), 6);
    assert!(four[5].starts_with("1 ") && four[5].len() == 2 + 64, "{four:?}");
    // The shell's hash is the last frame's.
    assert_eq!(four[4], four[3].split(' ').nth(1).unwrap());
}

/// `--save-state`, `--load-state` and `--record` are for a single machine:
/// with `--shell` they are refused before anything runs or is written.
#[test]
fn state_files_are_not_for_a_shell() {
    let dir = tempfile::tempdir().unwrap();
    let p = |n: &str| dir.path().join(n).to_str().unwrap().to_string();
    for opt in ["--save-state", "--load-state", "--record"] {
        let out = terse(&["run", "--shell", "--frames", "1", opt, &p("f"), "--store", &p("store"), "--data", &p("data")]);
        assert_eq!(out.status.code(), Some(2), "{opt}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("for a single machine, not --shell"), "{opt}");
        assert!(!Path::new(&p("f")).exists() && !Path::new(&p("store")).exists(), "{opt}: nothing written");
    }
}

/// An image title is UTF-8 (SPEC §image): `terse pack` refuses one that is not.
#[test]
fn pack_refuses_a_title_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("x.bin");
    std::fs::write(&bin, [0x01, 0x00]).unwrap();
    let out_path = dir.path().join("x.terse");
    let out = common::terse_command()
        .args(["pack", "--rom", bin.to_str().unwrap(), "--ram-pages", "1", "-o", out_path.to_str().unwrap(), "--title"])
        .arg(std::ffi::OsStr::from_bytes(b"caf\xE9"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--title caf\u{FFFD}: not valid UTF-8"), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!out_path.exists());
    // The same title in UTF-8 is packed.
    terse_ok(&["pack", "--rom", bin.to_str().unwrap(), "--ram-pages", "1", "-o", out_path.to_str().unwrap(), "--title", "café"]);
    assert!(out_path.exists());
}
