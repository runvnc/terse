//! programs/lowbell in the window, driven as a player drives it
//! (`--drive`, in a headless compositor the host starts; skipped without
//! weston): typed commands answered, the beam on the hidden chalk with the
//! picture drawn as listed, the picture still where nothing is animated
//! (the dark boathouse) and only the text changing while the player types,
//! no late frames; with both renderers, at 640 x 480 and 1900 x 1000.

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
    // (renderer, size, strict): strict runs also expect the picture still
    // where nothing moves, typing to change only the text, and no late
    // frame. At 1900 x 1000 the host's phosphor look is shed and restored
    // whenever drawing it runs long (PERFORMANCE.md R15), which changes the
    // whole picture: there only what is drawn is checked.
    for (renderer, size, strict) in [("software", "640x480", true), ("software", "1900x1000", false), ("gpu", "1900x1000", false)] {
        let still = |s: &str| if strict { s.to_string() } else { String::new() };
        let mut script = String::from("settle max 300\nexpect vectors contains c aa8250ff\nshot start.png\npress enter\nwait 4\n");
        script += &typed(&["light lantern", "open box", "take key", "unlock door", "open door", "east"]);
        // The dark boathouse: nothing animated.
        script += "settle\nexpect terminal contains Inside the Boathouse\n";
        script += &still("expect idle 60\nwatch 30\ntype hint\nwait 10\npress backspace\npress backspace\npress backspace\npress backspace\nwait 10\n");
        script += &typed(&["shine lantern at wall"]);
        script += "settle\nexpect terminal contains Chalk marks\nexpect vectors contains c ebebdcff\nshot chalk.png\n";
        script += &still("expect idle 60\n");
        script += "hold left 30\nsettle\nexpect vectors contains c 4e4e49ff\nshot faint.png\nexpect budget 50\n";
        script += &still("expect no-slow\n");
        script += "quit\n";
        let o = drive(dir.path(), &script, renderer, size);
        if no_gpu(renderer, &o) {
            continue;
        }
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        assert!(o.status.success() && out.contains("drive: done, no expectation failed"), "{renderer} {size}: {}", text(&o));
        if strict {
            // While "hint" was typed and rubbed out, only the input line
            // changed, never the picture (the top 256 of 480 units).
            let watch = out.lines().find(|l| l.contains("watch 30:")).unwrap_or_else(|| panic!("{}", text(&o)));
            let h: f32 = size.split('x').nth(1).unwrap().parse().unwrap();
            if let Some(y) = watch.split(" y ").nth(1) {
                let top: f32 = y.split("..").next().unwrap().trim().parse().unwrap();
                assert!(top > h * 0.9, "{renderer} {size}: typing changed more than the input line: {watch}");
            }
        }
        for shot in ["start.png", "chalk.png", "faint.png"] {
            let p = dir.path().join(shot);
            assert!(p.exists(), "{shot}");
            std::fs::rename(&p, dir.path().join(format!("{renderer}-{size}-{shot}"))).unwrap();
        }
    }
}
