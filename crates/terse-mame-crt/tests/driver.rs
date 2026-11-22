//! terse-dev-mame on the bus: its handshake for each group, that the look
//! never changes a machine (the same state hashes, frame by frame, as with
//! the reference drivers, clean or phosphor, drawn or not), and its
//! pictures (skipped, with a line, without a graphics adapter).
//!
//! The runs use terse-bus and the reference drivers from the same build
//! (`target/<profile>/`); a test that needs one that is not built says so
//! and returns.

use std::path::{Path, PathBuf};
use std::process::Command;

fn mame() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_terse-dev-mame"))
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn image(rel: &str) -> String {
    root().join(rel).display().to_string()
}

/// terse-bus and the reference drivers (other packages' programs), built
/// from the current sources; terse-bus starts the drivers beside it.
fn built(names: &[&str]) {
    terse_testkit::programs(names);
}

/// Scratch store, configuration and data: never the user's.
struct Dirs(tempfile::TempDir);

impl Dirs {
    fn new() -> Self {
        let d = tempfile::tempdir().unwrap();
        for s in ["store", "config", "data"] {
            std::fs::create_dir_all(d.path().join(s)).unwrap();
        }
        Dirs(d)
    }

    fn args(&self) -> Vec<String> {
        let p = |s: &str| self.0.path().join(s).display().to_string();
        vec!["--store".into(), p("store"), "--config".into(), p("config"), "--data".into(), p("data"), "--relay".into(), "none".into()]
    }
}

fn bus(args: &[String], mame_drivers: bool) -> std::process::Output {
    let dirs = Dirs::new();
    let mut c = Command::new(terse_testkit::program("terse-bus"));
    c.args(args).args(dirs.args());
    if mame_drivers {
        let m = mame().display().to_string();
        c.args(["--driver".to_string(), format!("3,9={m}"), "--driver".into(), format!("2={m}")]);
    }
    c.env_remove("TERSE_MAME_TIMES");
    c.output().unwrap()
}

fn ok(o: &std::process::Output, what: &str) -> String {
    assert!(o.status.success(), "{what}: {}\n{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout));
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn handshake_answers_each_group() {
    built(&["terse-bus"]);
    let m = mame().display().to_string();
    for (slot, want) in [("3", ["slot 3 0", "slot 9 0"].as_slice()), ("2", ["slot 2 0"].as_slice())] {
        let o = Command::new(terse_testkit::program("terse-bus")).args(["--probe-driver", &m, "--slot", slot]).output().unwrap();
        let out = ok(&o, "probe");
        for w in want {
            assert!(out.lines().any(|l| l == *w), "--slot {slot}: {out}");
        }
        assert!(out.contains("terse-dev-mame"), "{out}");
        // The stroke terminal does not give full text (slot 10).
        assert!(!out.lines().any(|l| l.starts_with("slot 10")), "{out}");
    }
}

/// The same machine, the same inputs: the same state after every tenth
/// frame whichever drivers draw it, in the clean style and the phosphor
/// style (the program's own or forced), with a picture drawn every frame
/// or none. Full text (slot 10) is withheld from both runs: the stroke
/// terminal does not give it, and a program may write other text when it
/// has it (sparks' hearts).
#[test]
fn the_look_never_changes_the_machine() {
    built(&["terse-bus", "terse-dev-terminal", "terse-dev-vector"]);
    let tmp = tempfile::tempdir().unwrap();
    let shot = tmp.path().join("shot.png").display().to_string();
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
    let runs: Vec<Vec<String>> = vec![
        s(&[&image("programs/sparks/sparks.terse"), "--events", &image("programs/sparks/sessions/phosphor.txt"), "--frames", "400"]),
        s(&[&image("programs/hello/hello.terse"), "--events", &image("programs/hello/hello.events"), "--frames", "40"]),
        s(&[&image("programs/hello/hello.terse"), "--events", &image("programs/hello/hello.events"), "--frames", "40", "--force-phosphor"]),
        s(&[&image("programs/holo/holo.terse"), "--frames", "60", "--force-phosphor"]),
        s(&[&image("crates/terse-mame-crt/pattern/pattern.terse"), "--frames", "60"]),
        s(&[&image("crates/terse-mame-crt/pattern/text.terse"), "--frames", "20"]),
    ];
    for run in runs {
        for picture in [false, true] {
            let mut args = run.clone();
            args.extend(s(&["--hash-every", "10", "--without", "10"]));
            if picture {
                args.extend(s(&["--screenshot", &shot, "--size", "640x480"]));
            }
            let want = ok(&bus(&args, false), "reference drivers");
            let got = ok(&bus(&args, true), "terse-dev-mame");
            assert!(want.lines().count() >= 2, "{want}");
            assert_eq!(got, want, "{run:?} (picture: {picture})");
        }
    }
}

/// The phosphor style's picture with the MAME pipeline: light where the
/// pattern's lines are, black between, text drawn. Skipped without a
/// graphics adapter (the driver then says so and the core draws its beam).
#[test]
fn pattern_picture() {
    built(&["terse-bus"]);
    let tmp = tempfile::tempdir().unwrap();
    let shot = tmp.path().join("pattern.png");
    let args: Vec<String> =
        [image("crates/terse-mame-crt/pattern/pattern.terse"), "--frames".into(), "30".into(), "--screenshot".into(), shot.display().to_string(), "--size".into(), "640x480".into()]
            .to_vec();
    let o = bus(&args, true);
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    if !err.contains("MAME vector CRT pipeline on") {
        terse_testkit::skip!("gpu", "no graphics adapter ({})", err.lines().find(|l| l.contains("terse-dev-mame")).unwrap_or("").trim());
        return;
    }
    ok(&o, "pattern");
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&shot).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (640, 480));
    let ch = info.color_type.samples();
    let px = |x: usize, y: usize| {
        let p = &buf[(y * 640 + x) * ch..][..3];
        p[0] as u32 + p[1] as u32 + p[2] as u32
    };
    // The width-16 white line at y = 52 units, x from 40 to 260: lit along
    // its length; black well away from anything.
    for x in (60..240).step_by(20) {
        assert!((50..=54).map(|y| px(x, y)).max().unwrap() > 300, "line at x {x}");
    }
    assert_eq!(px(150, 112), 0, "black between the lines");
    // The star's centre (440, 110), where eight lines cross, is the
    // brightest: the light adds.
    let centre = (108..=112).flat_map(|y| (438..=442).map(move |x| (x, y))).map(|(x, y)| px(x, y)).max().unwrap();
    let arm = (108..=112).map(|y| px(500, y)).max().unwrap();
    assert!(centre > arm, "crossing {centre} vs one line {arm}");
    // Text: the label's row (row 1: y 16 to 32) has light.
    assert!((16..32).flat_map(|y| (16..200).map(move |x| (x, y))).filter(|&(x, y)| px(x, y) > 100).count() > 50);
}
