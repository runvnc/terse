//! The stroke fonts on the bus (`TERSE_MAME_FONT`): whichever font draws
//! the text, the machine runs the same (the same state hashes as with the
//! reference drivers); the fonts' pictures differ; a name that is not a
//! font is said and the arcade font used. Pictures are skipped, with a
//! line, without a graphics adapter.

use std::path::{Path, PathBuf};
use std::process::Command;

const FONTS: [&str; 3] = ["arcade", "hershey", "drafting"];

fn mame() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_terse-dev-mame"))
}

fn image(rel: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel).display().to_string()
}

/// terse-bus and the reference drivers (other packages' programs), built
/// from the current sources; terse-bus starts the drivers beside it.
fn built(names: &[&str]) {
    terse_testkit::programs(names);
}

/// terse-bus with scratch store, configuration and data (never the
/// user's), no relay, and terse-dev-mame for both groups in `font` (or the
/// reference drivers for None).
fn bus(args: &[&str], font: Option<&str>) -> std::process::Output {
    let d = tempfile::tempdir().unwrap();
    let p = |s: &str| {
        let p = d.path().join(s);
        std::fs::create_dir_all(&p).unwrap();
        p.display().to_string()
    };
    let mut c = Command::new(terse_testkit::program("terse-bus"));
    c.args(args).args(["--store", &p("store"), "--config", &p("config"), "--data", &p("data"), "--relay", "none"]);
    c.env_remove("TERSE_MAME_TIMES").env_remove("TERSE_MAME_FONT");
    if let Some(f) = font {
        let m = mame().display().to_string();
        c.args(["--driver", &format!("3,9={m}"), "--driver", &format!("2={m}")]).env("TERSE_MAME_FONT", f);
    }
    c.output().unwrap()
}

fn ok(o: &std::process::Output, what: &str) -> String {
    assert!(o.status.success(), "{what}: {}\n{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout));
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn every_font_leaves_the_machine_alone() {
    built(&["terse-bus", "terse-dev-terminal", "terse-dev-vector"]);
    let tmp = tempfile::tempdir().unwrap();
    let shot = tmp.path().join("shot.png").display().to_string();
    let text = image("crates/terse-mame-crt/pattern/text.terse");
    let hello = image("programs/hello/hello.terse");
    let events = image("programs/hello/hello.events");
    let runs: [Vec<&str>; 2] = [
        vec![&text, "--frames", "20"],
        vec![&hello, "--events", &events, "--frames", "40", "--force-phosphor"],
    ];
    for run in &runs {
        let mut args = run.clone();
        args.extend(["--hash-every", "10", "--without", "10", "--screenshot", &shot, "--size", "640x480"]);
        let want = ok(&bus(&args, None), "reference drivers");
        assert!(want.lines().count() >= 2, "{want}");
        for f in FONTS {
            assert_eq!(ok(&bus(&args, Some(f)), f), want, "{f}: {run:?}");
        }
    }
}

fn pixels(path: &Path) -> Vec<u8> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut buf).unwrap();
    buf
}

#[test]
fn the_fonts_draw_differently() {
    built(&["terse-bus"]);
    let tmp = tempfile::tempdir().unwrap();
    let text = image("crates/terse-mame-crt/pattern/text.terse");
    let mut shots = Vec::new();
    for f in FONTS.iter().chain(&["courier"]) {
        let shot = tmp.path().join(format!("{f}.png"));
        let o = bus(&[&text, "--frames", "10", "--screenshot", &shot.display().to_string(), "--size", "640x480"], Some(f));
        let err = String::from_utf8_lossy(&o.stderr).into_owned();
        ok(&o, f);
        if *f == "courier" {
            assert!(err.contains("TERSE_MAME_FONT"), "{err}");
        }
        if !err.contains("MAME vector CRT pipeline on") {
            terse_testkit::skip!("gpu", "no graphics adapter");
            return;
        }
        shots.push(pixels(&shot));
    }
    assert_ne!(shots[0], shots[1], "arcade and hershey");
    assert_ne!(shots[0], shots[2], "arcade and drafting");
    assert_ne!(shots[1], shots[2], "hershey and drafting");
    // An unknown name: the arcade font.
    assert_eq!(shots[3], shots[0], "courier is arcade");
}
