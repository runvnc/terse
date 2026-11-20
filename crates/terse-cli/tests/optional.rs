//! Optional devices from the command line (SPEC §image, §profiles): sparks lists
//! content (device 4) as optional, so it runs on the program profile
//! (`terse run --profile program`, devices 1, 2 and 3) and with `--without
//! 4`, with the same game; the grants are in its state, so snapshots move
//! between hosts only where the devices can be granted; `pack --optional`
//! and `info`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use terse_core::{Grants, Image, Machine, Script};
use terse_net::service::Headless;

fn sparks() -> String {
    common::programs().join("sparks/sparks.terse").to_str().unwrap().to_string()
}

fn keys() -> String {
    common::programs().join("sparks/sessions/keys.txt").to_str().unwrap().to_string()
}

fn expected(name: &str) -> PathBuf {
    common::programs().join("sparks/expected").join(name)
}

fn terse(args: &[&str]) -> Output {
    common::terse_command().args(args).output().unwrap()
}

fn terse_ok(args: &[&str]) -> String {
    let out = terse(args);
    assert!(out.status.success(), "terse {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// A store, config and data directory for runs that have content.
struct Dirs(tempfile::TempDir);

impl Dirs {
    fn new() -> Self {
        Dirs(tempfile::tempdir().unwrap())
    }

    fn p(&self, n: &str) -> String {
        self.0.path().join(n).to_str().unwrap().to_string()
    }

    fn args(&self) -> Vec<String> {
        ["--store", "--config", "--data"].iter().zip(["store", "config", "data"]).flat_map(|(o, d)| [o.to_string(), self.p(d)]).collect()
    }
}

/// `terse run sparks.terse --events keys.txt <extra>` with `dirs`.
fn run(extra: &[&str], dirs: &Dirs) -> String {
    let (image, events, d) = (sparks(), keys(), dirs.args());
    let mut args = vec!["run", &image, "--events", &events];
    args.extend_from_slice(extra);
    args.extend(d.iter().map(String::as_str));
    terse_ok(&args)
}

/// On the program profile sparks plays exactly as with content: the same
/// screen after 900 frames of the recorded session as the golden one made
/// with content (a new data directory: no saved high score either way).
/// Its hashes, golden in `program-hashes.txt`, are those of the library's
/// machine with content not granted.
#[test]
fn sparks_on_the_program_profile() {
    let dirs = Dirs::new();
    let screen = run(&["--profile", "program", "--frames", "900", "--dump", "--dump-vectors"], &dirs);
    assert_eq!(screen, std::fs::read_to_string(expected("keys-900.txt")).unwrap());
    // No content: the run touched no store, config or data directory.
    assert!(!Path::new(&dirs.p("store")).exists() && !Path::new(&dirs.p("data")).exists());

    let image = Image::parse(&std::fs::read(sparks()).unwrap()).unwrap();
    let mut m = Machine::with_grants(&image, Grants(vec![false]));
    let mut h = Headless::new(None, Script::parse(&std::fs::read_to_string(keys()).unwrap()).unwrap());
    let mut got = String::new();
    for f in 1..=3000u32 {
        h.frame(&mut m);
        if [30, 900, 1800, 3000].contains(&f) {
            got += &format!("{f} {}\n", terse_net::to_hex(&m.state_hash()));
        }
    }
    common::golden("sparks/expected/program-hashes.txt", &got);
    let cli = run(&["--profile", "program", "--frames", "1800", "--hash-every", "900"], &dirs);
    assert_eq!(cli, got.lines().skip(1).take(2).map(|l| format!("{l}\n")).collect::<String>());
    // --without 4 on a full host is the same machine.
    assert_eq!(run(&["--without", "4", "--frames", "1800", "--hash-every", "900"], &dirs), cli);
}

/// With and without content, the game is the same frame for frame: the
/// terminal and the display list match all through the recorded session.
/// (RAM is not: the record requests write their blocks, and the handle of
/// the load differs.)
#[test]
fn the_same_game_without_content() {
    let image = Image::parse(&std::fs::read(sparks()).unwrap()).unwrap();
    let script = Script::parse(&std::fs::read_to_string(keys()).unwrap()).unwrap();
    let mut with = Machine::new(&image);
    let mut without = Machine::with_grants(&image, Grants(vec![false]));
    let store = tempfile::tempdir().unwrap();
    let mut node = terse_net::Node::new(terse_net::BlobStore::open(store.path().join("store"), None).unwrap(), None);
    node.set_records(terse_net::Records::in_data_dir(&store.path().join("data")));
    let mut hw = Headless::new(Some(terse_net::ContentService::start(node)), script.clone());
    let mut hwo = Headless::new(None, script);
    assert!(with.content().is_some() && without.content().is_none());
    for f in 1..=3000 {
        hw.frame(&mut with);
        hwo.frame(&mut without);
        assert_eq!(with.vector_list(), without.vector_list(), "frame {f}");
        if f % 50 == 0 {
            assert_eq!(with.terminal().unwrap().text(), without.terminal().unwrap().text(), "frame {f}");
        }
    }
}

/// STATE.md 4: a snapshot taken without content restores on a full host
/// and goes on without it, as the uninterrupted run did; one taken with
/// content does not restore where content cannot be granted.
#[test]
fn snapshots_and_grants() {
    let dirs = Dirs::new();
    let whole = run(&["--profile", "program", "--frames", "600", "--hash-every", "300"], &dirs);
    let snap = dirs.p("program.snap");
    run(&["--profile", "program", "--frames", "300", "--save-state", &snap], &dirs);
    let later = run(&["--load-state", &snap, "--frames", "300", "--hash-every", "300"], &dirs);
    assert_eq!(later, whole.lines().nth(1).unwrap().to_string() + "\n", "content stays absent on a full host");

    let full = dirs.p("full.snap");
    run(&["--frames", "300", "--save-state", &full], &dirs);
    for refusing in [&["--profile", "program"][..], &["--without", "4"]] {
        let image = sparks();
        let mut args = vec!["run", &image, "--load-state", &full, "--frames", "1"];
        args.extend_from_slice(refusing);
        let d = dirs.args();
        args.extend(d.iter().map(String::as_str));
        let out = terse(&args);
        assert_eq!(out.status.code(), Some(2), "{refusing:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("the machine had device 4 version 0, which this host does not provide"), "{err}");
    }
}

/// `pack --optional` sets the top bit of the version byte; `info` shows the
/// required and the optional devices; the version is 0 to 127.
#[test]
fn pack_and_info() {
    let dir = tempfile::tempdir().unwrap();
    let p = |n: &str| dir.path().join(n).to_str().unwrap().to_string();
    std::fs::write(p("rom"), [0x6F, 0, 0, 0]).unwrap();
    let base = ["pack", "--rom", &p("rom"), "--title", "opt", "--ram-pages", "1", "-o"];
    terse_ok(&[&base[..], &[&p("a.terse"), "--optional", "6:0", "--device", "2:0", "--optional", "4:0"]].concat());
    let bytes = std::fs::read(p("a.terse")).unwrap();
    assert_eq!(bytes[10], 3);
    assert_eq!(&bytes[32..38], &[2, 0x00, 6, 0x80, 4, 0x80], "required first, then the optional ones in order");
    let info = terse_ok(&["info", &p("a.terse")]);
    assert!(info.contains("devices:   2:0\n") && info.contains("optional:  6:0 4:0\n"), "{info}");
    let hello = terse_ok(&["info", common::programs().join("hello/hello.terse").to_str().unwrap()]);
    assert!(hello.contains("optional:  -\n"), "{hello}");
    for bad in [&["--optional", "4:128"][..], &["--device", "2:200"], &["--optional", "8:0"], &["--optional", "1:0"]] {
        let out = terse(&[&base[..], &[&p("b.terse")], bad].concat());
        assert!(!out.status.success(), "{bad:?}");
    }
    // A reserved slot may be optional: never granted, the image runs.
    terse_ok(&[&base[..], &[&p("c.terse"), "--optional", "5:0"]].concat());
    terse_ok(&["run", &p("c.terse"), "--frames", "1"]);
    // `--without` a device the image requires: refused, as a host without it.
    let out = terse(&["run", common::programs().join("hello/hello.terse").to_str().unwrap(), "--without", "2", "--frames", "1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("requires device 2, which this run does not provide (--without 2)"));
    assert!(!terse(&["run", &p("c.terse"), "--without", "1", "--frames", "1"]).status.success());
}

/// sketch lists content as optional too: without it, it says so, draws as
/// ever, and S and P only say that they cannot save or publish.
#[test]
fn sketch_without_content() {
    let dir = tempfile::tempdir().unwrap();
    let events = dir.path().join("events");
    std::fs::write(&events, "3 pointer 100 100 1\n4 pointer 110 100 1\n6 pointer 120 105 0\n8 press s\n10 press p\n").unwrap();
    let image = common::programs().join("sketch/sketch.terse");
    let run = |frames: &str| {
        terse_ok(&["run", image.to_str().unwrap(), "--profile", "program", "--events", events.to_str().unwrap(), "--frames", frames, "--dump", "--dump-vectors"])
    };
    assert!(run("2").contains("Drawings are not kept"));
    let drawn = run("8");
    assert!(drawn.contains("\nline "), "the stroke is drawn: {drawn}");
    assert!(run("10").contains("No saving on this host"));
    assert!(run("12").contains("No publishing on this host"));
}
