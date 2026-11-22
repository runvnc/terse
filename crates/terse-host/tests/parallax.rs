//! The eye and the pointer (spec/vector3d.md §vector3d.space): in a window
//! the eye follows the pointer, except over a machine that lists the 3D
//! display, where it rests, so that the pointer is over what the program
//! drew there; a pointer-driven program is hit where it drew its target.
//! Driven in the real window (`--drive`, in its own headless compositor;
//! skipped where there is none), parallax on as by default.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output};

/// The tests run one at a time: each runs a window in real time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn have_weston() -> bool {
    let ok = Command::new("weston").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok {
        terse_testkit::skip!("weston", "no weston: --drive needs a headless compositor");
    }
    ok
}

fn program(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../programs/{name}/{name}.terse"))
}

/// `terse-host <image> --drive <script>` with scratch directories, no relay.
fn drive(dir: &Path, image: &Path, script: &str, renderer: &str) -> Output {
    let s = dir.join(format!("script-{renderer}.txt"));
    std::fs::write(&s, script).unwrap();
    Command::new(env!("CARGO_BIN_EXE_terse-host"))
        .arg(image)
        .arg("--drive")
        .arg(&s)
        // The rule's notice over the bottom rows would make two pictures
        // differ on a loaded machine: these tests are about the eye.
        .args(["--renderer", renderer, "--consent", "deny", "--mute", "--perf-rule", "off"])
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

/// holo lists the 3D display: with its models still (A), the picture with
/// the pointer at the centre and with it at a corner is the same (the eye
/// rests), by both renderers.
#[test]
fn the_eye_rests_over_a_3d_program() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for renderer in ["software", "gpu"] {
        let (a, b) = (dir.path().join(format!("centre-{renderer}.png")), dir.path().join(format!("corner-{renderer}.png")));
        let script = format!(
            "settle max 300\npress a\npointer 320 240\nwait 60\nshot {}\npointer 630 470\nwait 60\nshot {}\nquit\n",
            a.display(),
            b.display()
        );
        let o = drive(dir.path(), &program("holo"), &script, renderer);
        assert!(o.status.success(), "{renderer}: {}", text(&o));
        let (a, b) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        assert!(a == b, "{renderer}: the picture moved with the pointer");
    }
}

/// The sheet (pointer-driven; it does not list the 3D display): a click on
/// a cell, where it is drawn, selects that cell.
#[test]
fn a_click_hits_the_cell_drawn_under_the_pointer() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // D10 (the total) is drawn at about (360, 257) in screen units.
    let script = "settle max 300\npointer 600 60\nwait 30\nclick 360 257\nsettle max 300\nexpect page contains D10\nquit\n";
    for renderer in ["software", "gpu"] {
        let o = drive(dir.path(), &program("sheet"), script, renderer);
        assert!(o.status.success(), "{renderer}: {}", text(&o));
    }
}
