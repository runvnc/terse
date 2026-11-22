//! The implementation guide's programs, `examples/program/{c,rust}`: each
//! is built with `terse build` as its page says and run headless to the
//! screen, display list or end the page shows (the invented device's
//! program also on `terse-bus` with `native/driver-experimental/dice.c`);
//! and every listing in the manual is the file it names, as
//! `scripts/manual-listings.py` copies it, within the 64 columns a
//! preformatted line may have (docs/manual/README). Builds that need clang
//! or Rust's RISC-V target, and the driver run that needs a C compiler,
//! are skipped with a line saying so.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

const TARGET: &str = "riscv32imc-unknown-none-elf";

fn have_rust() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    let ok = *OK.get_or_init(|| {
        let dir = Command::new("rustc")
            .args(["--print", "target-libdir", "--target", TARGET])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        dir.is_some_and(|d| {
            std::fs::read_dir(d).is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("libcore-"))))
        })
    });
    if !ok {
        terse_testkit::skip!("rust-target", "needs rustc with the {TARGET} target (rustup target add {TARGET})");
    }
    ok
}

/// The languages whose tools are here: "c" (clang, ld.lld) and "rust".
fn languages() -> Vec<&'static str> {
    let mut langs = Vec::new();
    if common::have_tools() {
        langs.push("c");
    }
    if have_rust() {
        langs.push("rust");
    }
    langs
}

fn example(lang: &str, stem: &str) -> PathBuf {
    let ext = if lang == "c" { "c" } else { "rs" };
    common::workspace().join(format!("examples/program/{lang}/{stem}.{ext}"))
}

fn terse(dir: &Path, args: &[&str]) -> Output {
    common::terse_command().current_dir(dir).args(args).output().unwrap()
}

fn ok(out: Output, what: &str) -> String {
    assert!(out.status.success(), "{what}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Builds the example `stem` in `lang` into `dir/<stem>.terse` with `extra`.
fn build(dir: &Path, lang: &str, stem: &str, extra: &[&str]) -> String {
    let source = example(lang, stem);
    let out = format!("{stem}.terse");
    let mut args = vec!["build", source.to_str().unwrap(), "-o", &out];
    args.extend(extra);
    ok(terse(dir, &args), &format!("terse build {stem} ({lang})"));
    out
}

/// The non-blank lines of a dump.
fn shown(text: &str) -> Vec<&str> {
    text.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect()
}

#[test]
fn hello_says_hello_and_stops_at_esc() {
    for lang in languages() {
        let dir = tempfile::tempdir().unwrap();
        let image = build(dir.path(), lang, "hello", &[]);
        let screen = ok(terse(dir.path(), &["run", &image, "--frames", "60", "--dump"]), "hello");
        assert_eq!(shown(&screen), [" Hello from hello!", "Press Esc to exit."], "{lang}");
        std::fs::write(dir.path().join("esc.txt"), "5 press esc\n").unwrap();
        let frames = ok(terse(dir.path(), &["run", &image, "--frames", "60", "--events", "esc.txt", "--hash-every", "1"]), "hello, Esc");
        assert_eq!(frames.lines().last().and_then(|l| l.split(' ').next()), Some("6"), "{lang}: stops after frame 6");
    }
}

#[test]
fn line_swings_left() {
    for lang in languages() {
        let dir = tempfile::tempdir().unwrap();
        let image = build(dir.path(), lang, "line", &[]);
        std::fs::write(dir.path().join("keys.txt"), "5 press left\n6 press left\n").unwrap();
        let out = ok(terse(dir.path(), &["run", &image, "--frames", "10", "--events", "keys.txt", "--dump", "--dump-vectors"]), "line");
        assert_eq!(shown(&out), ["-- vectors: 11 list bytes --", "line 5120 6400 4800 1280 w 16 c ffffffff"], "{lang}");
    }
}

#[test]
fn chase_scores_one() {
    for lang in languages() {
        let dir = tempfile::tempdir().unwrap();
        let image = build(dir.path(), lang, "chase", &[]);
        std::fs::write(dir.path().join("chase.txt"), "1 press left\n81 press up\n").unwrap();
        let out = ok(terse(dir.path(), &["run", &image, "--frames", "150", "--events", "chase.txt", "--dump", "--dump-vectors"]), "chase");
        assert_eq!(
            shown(&out),
            ["Score 1", "-- vectors: 17 list bytes --", "disc 2560 1632 r 64 c ffffffff", "circle 640 640 r 160 w 16 c ffffffff"],
            "{lang}"
        );
        // Esc ends the game.
        std::fs::write(dir.path().join("esc.txt"), "5 press esc\n").unwrap();
        let frames = ok(terse(dir.path(), &["run", &image, "--frames", "60", "--events", "esc.txt", "--hash-every", "1"]), "chase, Esc");
        assert!(frames.lines().count() < 60, "{lang}: Esc did not end chase");
    }
}

#[test]
fn myshell_shows_hello() {
    for lang in languages() {
        let dir = tempfile::tempdir().unwrap();
        let image = build(dir.path(), lang, "myshell", &["--device", "8:0"]);
        let out = ok(terse(dir.path(), &["run", "--shell", &image, "--frames", "30", "--dump"]), "myshell");
        let lines = shown(&out);
        assert_eq!(lines[..2], ["-- machine 1 running 0 0 640 480 hello --", " Tersenet · hello"], "{lang}: {out}");
    }
}

#[test]
fn roll_without_its_device() {
    for lang in languages() {
        let dir = tempfile::tempdir().unwrap();
        let image = build(dir.path(), lang, "roll", &["--optional", "240:0=example.dice"]);
        let out = ok(terse(dir.path(), &["run", &image, "--frames", "30", "--dump"]), "roll");
        assert_eq!(shown(&out), ["No dice here: run me with example.dice."], "{lang}");
        let info = ok(terse(dir.path(), &["info", &image]), "terse info");
        assert!(info.contains("experimental: 240=example.dice (not covered by conformance)"), "{info}");
    }
}

/// roll on terse-bus with the example driver: the rolls the page shows.
#[cfg(unix)]
#[test]
fn roll_with_the_dice_driver() {
    let langs = languages();
    if langs.is_empty() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let dice = dir.path().join("dice");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let built = Command::new(cc)
        .args(["-std=c11", "-O2", "-o"])
        .arg(&dice)
        .arg(common::workspace().join("native/driver-experimental/dice.c"))
        .status()
        .is_ok_and(|s| s.success());
    if !built {
        terse_testkit::skip!("cc", "no C compiler for native/driver-experimental/dice.c");
        return;
    }
    let bus = terse_testkit::programs(&["terse-bus", "terse-dev-terminal", "terse-dev-vector"]).remove(0);
    for lang in langs {
        let image = build(dir.path(), lang, "roll", &["--optional", "240:0=example.dice"]);
        let driver = format!("example.dice={}", dice.display());
        let home = |d: &str| dir.path().join(format!("{lang}-{d}")).display().to_string();
        let out = Command::new(&bus)
            .current_dir(dir.path())
            .args([&image, "--driver", &driver, "--frames", "100", "--dump", "--relay", "none"])
            .args(["--store", &home("store"), "--config", &home("config"), "--data", &home("data")])
            .output()
            .unwrap();
        let out = ok(out, "terse-bus roll");
        assert_eq!(shown(&out), ["You rolled 1.", "You rolled 2.", "You rolled 5."], "{lang}");
    }
}

/// The listings of the manual, as scripts/manual-listings.py writes them:
/// a fence opened by three backquotes, a word and a path (with a `/`)
/// holds that file, or, with two more words between slashes, the part of
/// it between the lines holding them; blank lines at either end are left
/// out. Every line of a listing fits in 64 columns.
#[test]
fn the_manual_quotes_the_files() {
    let root = common::workspace();
    let mut listings = 0;
    for entry in std::fs::read_dir(root.join("docs/manual")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "txt") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.split('\n').collect();
        let mut i = 0;
        while i < lines.len() {
            let Some(info) = lines[i].strip_prefix("```") else {
                i += 1;
                continue;
            };
            let close = (i + 1..lines.len()).find(|&j| lines[j].trim_end() == "```").unwrap_or(lines.len());
            let words: Vec<&str> = info.split_whitespace().collect();
            if words.len() >= 2 && words[1].contains('/') {
                let body = &lines[i + 1..close];
                let want = excerpt(&root, &words[1..]);
                let at = format!("{}:{}", path.display(), i + 1);
                assert!(body == want.iter().map(String::as_str).collect::<Vec<_>>(), "{at}: the listing is not {} (run scripts/manual-listings.py)", words[1]);
                for l in body {
                    assert!(l.chars().count() <= 64, "{at}: a line over 64 columns: {l}");
                }
                listings += 1;
            }
            i = close + 1;
        }
    }
    assert!(listings >= 11, "{listings} listings");
}

/// `words`: the path, then optionally /from/ and /to/ (possibly with blanks inside).
fn excerpt(root: &Path, words: &[&str]) -> Vec<String> {
    let text = std::fs::read_to_string(root.join(words[0])).unwrap();
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let rest = words[1..].join(" ");
    if !rest.is_empty() {
        let marks: Vec<&str> = rest.split('/').map(str::trim).filter(|s| !s.is_empty()).collect();
        let (from, to) = (marks[0], marks[1]);
        let first = lines.iter().position(|l| l.contains(from)).unwrap();
        let last = first + 1 + lines[first + 1..].iter().position(|l| l.contains(to)).unwrap();
        lines = lines[first + 1..last].to_vec();
    }
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines
}
