//! programs/painted-test in the window, driven as a player drives it
//! (`--drive`, in a headless compositor the host starts; skipped without
//! weston): the painted scene still while nothing moves, the traveller
//! walked about with the arrows (behind the boulder and up to the door),
//! the door opened by a typed command, the phosphor style chosen, the
//! program within half its budget, and no late frame up to the switch of
//! style (with the software renderer; the GPU in the headless compositor
//! is llvmpipe, whose timing is not the program's; the switch itself may
//! make a frame or two late while the host sets the phosphor up, which is
//! the host's, CONCEPT.md); with both renderers.

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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/painted-test/painted-test.terse")
}

fn drive(dir: &Path, script: &str, renderer: &str, extra: &[&str]) -> Output {
    let s = dir.join(format!("script-{renderer}.txt"));
    std::fs::write(&s, script).unwrap();
    Command::new(env!("CARGO_BIN_EXE_terse-host"))
        .arg(image())
        .arg("--drive")
        .arg(&s)
        .args(["--renderer", renderer, "--size", "1280x960", "--consent", "deny"])
        .args(extra)
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

const SCRIPT: &str = "\
settle max 300
expect idle 60
hold up 8
hold left 160
settle
expect idle 30
shot rock.png
hold left 300
hold down 100
hold right 313
hold up 200
type open door
press enter
wait 6
settle
expect terminal contains The door swings inward
expect budget 50
expect no-slow
type phosphor
press enter
wait 6
settle
shot phosphor.png
hold left 30
expect budget 50
quit
";

#[test]
fn walked_in_the_window() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for renderer in ["software", "gpu"] {
        // In the headless compositor the GPU is a software one (llvmpipe):
        // its frames are the emulator's, so there the walk is checked
        // without the timing (as mame_look.rs does).
        let o = if renderer == "gpu" {
            drive(dir.path(), &SCRIPT.replace("expect no-slow\n", ""), renderer, &["--perf-rule", "off"])
        } else {
            drive(dir.path(), SCRIPT, renderer, &[])
        };
        if renderer == "gpu" && String::from_utf8_lossy(&o.stderr).contains("no graphics adapter") {
            terse_testkit::skip!("gpu", "no graphics adapter in the compositor");
            continue;
        }
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        assert!(o.status.success() && out.contains("drive: done, no expectation failed"), "{renderer}: {}", text(&o));
        for shot in ["rock.png", "phosphor.png"] {
            assert!(dir.path().join(shot).exists(), "{renderer}: no {shot}");
        }
    }
}
