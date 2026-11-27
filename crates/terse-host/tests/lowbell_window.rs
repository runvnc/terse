//! programs/lowbell in the window, driven as a player drives it
//! (`--drive`, in a headless compositor the host starts; skipped without
//! weston): typed commands answered, the beam on the hidden chalk with the
//! picture drawn as listed, the picture still where nothing is animated
//! (the dark clock room) and only the text changing while the player types,
//! each machine frame within half its budget; with both renderers, at
//! 640 x 480 and 1900 x 1000. Late frames are checked by hand (--drive).

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output};

static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn have_weston() -> bool {
    let ok = Command::new("weston").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok {
        terse_testkit::skip!("weston", "no weston: --drive needs a headless compositor");
    }
    ok
}

fn image() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/lowbell/lowbell.terse")
}

fn drive(dir: &Path, script: &str, renderer: &str, size: &str) -> Output {
    let s = dir.join(format!("script-{renderer}-{size}.txt"));
    std::fs::write(&s, script).unwrap();
    Command::new(env!("CARGO_BIN_EXE_terse-host"))
        .arg(image())
        .arg("--drive")
        .arg(&s)
        .args(["--renderer", renderer, "--size", size, "--consent", "deny"])
        // The host sheds its phosphor look when drawing runs long
        // (PERFORMANCE.md R15), which changes the whole picture whatever the
        // program draws: the look is kept whole, so that `expect idle`
        // sees only the program's picture.
        .arg("--no-shed")
        // gpu runs on llvmpipe here: the host's slow-frame rule is about the host, the program's cost is still checked by expect budget.
        .args(if renderer == "gpu" { &["--perf-rule", "off"][..] } else { &[] })
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env_remove("TERSE_DRIVE_DISPLAY")
        .env_remove("TERSE_DRIVE_NESTED")
        .current_dir(dir)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!("status {:?}\n--- stdout\n{}--- stderr\n{}", o.status, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn no_gpu(renderer: &str, o: &Output) -> bool {
    let none = renderer == "gpu" && String::from_utf8_lossy(&o.stderr).contains("no graphics adapter");
    if none {
        terse_testkit::skip!("gpu", "no graphics adapter in the compositor");
    }
    none
}

/// Commands typed one character a frame, as a keyboard types them.
fn typed(cmds: &[&str]) -> String {
    cmds.iter().map(|c| format!("type {c}\npress enter\nwait 6\n")).collect()
}

#[test]
fn played_in_the_window() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for (renderer, size) in [("software", "640x480"), ("software", "1900x1000"), ("gpu", "1900x1000")] {
        let mut script = String::from("settle max 300\nexpect vectors contains c aa8250ff\nshot start.png\npress enter\nwait 4\n");
        script += &typed(&["fill lantern", "light lantern", "down"]);
        // The dark clock room: nothing animated.
        script += "settle\nexpect terminal contains The Clock Room\n";
        script += "expect idle 60\n";
        // Typing: the scene's list and every row but the input line stay as
        // they were, while the letters show and go.
        script += "dump vectors > v0.txt\ndump terminal > t0.txt\ntype hint\nwait 4\nexpect terminal contains > hint\n";
        script += "dump vectors > v1.txt\ndump terminal > t1.txt\n";
        script += "press backspace\npress backspace\npress backspace\npress backspace\nwait 4\n";
        script += &typed(&["up", "shine lantern at frame"]);
        script += "settle\nexpect terminal contains Someone scratched a drawing\nexpect vectors contains c ebebdcff\nshot chalk.png\n";
        script += "expect idle 60\n";
        script += "hold left 30\nsettle\nexpect vectors contains c 4e4e49ff\nshot faint.png\nexpect budget 50\n";
        // Late frames (expect no-slow) depend on the computer's load and the
        // host's compose time, not on the program: they are checked by hand
        // with `terse-host --drive`, not here. The program's own cost is
        // checked above (expect budget).
        script += "quit\n";
        let o = drive(dir.path(), &script, renderer, size);
        if no_gpu(renderer, &o) {
            continue;
        }
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        assert!(o.status.success() && out.contains("drive: done, no expectation failed"), "{renderer} {size}: {}", text(&o));
        let read = |f: &str| std::fs::read_to_string(dir.path().join(f)).unwrap();
        assert_eq!(read("v0.txt"), read("v1.txt"), "{renderer} {size}: typing changed the picture");
        let (t0, t1) = (read("t0.txt"), read("t1.txt"));
        let rows = |t: &str| t.lines().take(29).map(str::to_string).collect::<Vec<_>>();
        assert_eq!(rows(&t0), rows(&t1), "{renderer} {size}: typing changed more than the input line");
        assert_ne!(t0, t1, "{renderer} {size}: the letters typed did not show");
        for shot in ["start.png", "chalk.png", "faint.png"] {
            let p = dir.path().join(shot);
            assert!(p.exists(), "{shot}");
            std::fs::rename(&p, dir.path().join(format!("{renderer}-{size}-{shot}"))).unwrap();
        }
    }
}
