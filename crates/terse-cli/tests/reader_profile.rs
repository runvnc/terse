//! A smoke test of the reader on the machine's reading profile (`terse run
//! --profile reading`: devices 1, 2, 4 and 7; content requests outside the
//! profile complete with status 05h): the scripts of the reader's golden
//! tests (tests/reader.rs) start it and drive it without a fault, and those
//! that use only the profile's operations show exactly what they show on a
//! full host.

mod common;

use std::path::Path;
use std::process::Output;

fn reader() -> String {
    common::programs().join("reader/reader.terse").to_str().unwrap().to_string()
}

fn terse_ok(args: &[&str]) -> String {
    let out = common::terse_command().args(args).output().unwrap();
    assert!(out.status.success(), "terse {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &to.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

/// A store with three articles of a feed the host follows, and a host with
/// its own identity.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str| dir.path().join(n).to_str().unwrap().to_string();
        let key = terse_ok(&["key", "new", "--config", &p("author")]).trim().to_string();
        for i in 1..=3 {
            let file = p(&format!("a{i}.txt"));
            let link = "0".repeat(64);
            std::fs::write(&file, format!("Article {i}\n\nBody {i}, with a [link](terse:{link}).\n")).unwrap();
            terse_ok(&["feed", "publish", &file, "--title", &format!("Article {i}"), "--store", &p("store"), "--config", &p("author"), "--relay", "none"]);
        }
        terse_ok(&["key", "new", "--config", &p("host")]);
        terse_ok(&["feed", "subscribe", &key, "--store", &p("store"), "--config", &p("host"), "--relay", "none"]);
        Fixture { dir }
    }

    /// `terse run reader.terse --consent allow` with `script` for 90
    /// frames, on copies of the store and config, `--profile reading` if
    /// `reading`.
    fn run(&self, script: &str, reading: bool) -> Output {
        let run = tempfile::tempdir().unwrap();
        let p = |n: &str| run.path().join(n).to_str().unwrap().to_string();
        copy(&self.dir.path().join("store"), run.path().join("store").as_path());
        copy(&self.dir.path().join("host"), run.path().join("host").as_path());
        std::fs::write(p("events"), script).unwrap();
        let mut cmd = common::terse_command();
        cmd.args(["run", &reader(), "--store", &p("store"), "--config", &p("host"), "--data", &p("data"), "--consent", "allow"])
            .args(["--events", &p("events"), "--frames", "90", "--dump", "--dump-page"]);
        if reading {
            cmd.args(["--profile", "reading"]);
        }
        cmd.output().unwrap()
    }
}

/// The scripts of tests/reader.rs, and whether they use only the reading
/// profile's operations (get, subscriptions, timeline, names, identity,
/// subscribe, unsubscribe).
const SCRIPTS: [(&str, bool); 21] = [
    ("", true),
    ("12 press enter\n", true),
    ("12 press enter\n30 press end\n", true),
    ("12 press end\n14 press enter\n", true),
    ("12 press end\n13 press up\n14 press enter\n", true),
    ("12 press down\n14 press enter\n20 press end\n", true),
    ("12 press down\n13 press down\n", true),
    ("12 press enter\n30 press tab\n32 press enter\n", true),
    ("12 press m\n", true),
    ("10 text ?\n", true),
    ("10 press f1\n", true),
    ("10 press f\n", true),
    ("10 press f\n12 press s\n", true),
    ("10 press f\n14 press u\n", true),
    ("10 press f\n12 press down\n14 press u\n", true),
    ("10 press t\n", false),
    ("10 text /\n12 text article\n14 press enter\n", false),
    ("10 press f\n12 text /\n", false),
    ("10 press f\n14 press p\n16 text Carol\n18 press enter\n", false),
    ("10 press f\n14 press n\n16 text Al\n18 press enter\n", false),
    ("10 press f\n12 press down\n14 press n\n", false),
];

#[test]
fn reader_runs_on_the_reading_profile() {
    let f = Fixture::new();
    for (script, reading_only) in SCRIPTS {
        let out = f.run(script, true);
        assert!(out.status.success(), "{script:?}: {:?} {}", out.status, String::from_utf8_lossy(&out.stderr));
        assert!(!String::from_utf8_lossy(&out.stderr).contains("fault"), "{script:?}");
        let screen = String::from_utf8(out.stdout).unwrap();
        assert!(screen.lines().take(30).any(|l| !l.trim().is_empty()), "{script:?}: a blank screen");
        if reading_only {
            let full = f.run(script, false);
            assert_eq!(screen, String::from_utf8(full.stdout).unwrap(), "{script:?}: the same as on a full host");
        }
    }
}

/// The reading profile refuses images that list a device it does not
/// provide (SPEC 2), and a system (it has no device 8).
#[test]
fn the_reading_profile_refuses_other_devices() {
    let dir = tempfile::tempdir().unwrap();
    let p = |n: &str| dir.path().join(n).to_str().unwrap().to_string();
    for (program, device) in [("sparks", "3"), ("holo", "9"), ("conform", "3")] {
        let image = common::programs().join(program).join(format!("{program}.terse"));
        let out = common::terse_command()
            .args(["run", image.to_str().unwrap(), "--profile", "reading", "--frames", "1", "--store", &p("s"), "--data", &p("d")])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{program}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(&format!("requires device {device}, which the reading profile does not provide")), "{program}: {err}");
    }
    let out = common::terse_command().args(["run", "--shell", "--profile", "reading", "--frames", "1"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = common::terse_command().args(["run", &reader(), "--profile", "core", "--frames", "1"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown profile core"));
    // hello (device 2 only) and fetch (2 and 4) run.
    for program in ["hello", "fetch"] {
        let image = common::programs().join(program).join(format!("{program}.terse"));
        terse_ok(&["run", image.to_str().unwrap(), "--profile", "reading", "--frames", "5", "--store", &p("s"), "--data", &p("d"), "--relay", "none"]);
    }
}

/// `terse run --profile reading`: the search of the articles on this host
/// uses only the profile's operations (timeline and get), so it finds them,
/// while the relays' sections say the search was refused.
#[test]
fn search_of_this_hosts_articles_on_the_reading_profile() {
    let f = Fixture::new();
    let out = f.run("10 text /\n12 text BODY 2\n14 press enter\n", true);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let screen = String::from_utf8(out.stdout).unwrap();
    assert!(screen.contains("sans16b |In your articles\n"), "{screen}");
    assert!(screen.contains("sans12 |1 found in 3 articles\n"), "{screen}");
    assert!(screen.contains("[Body 2]{2}[, with a [link](terse:"), "the excerpt, the match in it: {screen}");
    assert!(screen.contains("sans12 |the relays refused the search\n"), "{screen}");
}
