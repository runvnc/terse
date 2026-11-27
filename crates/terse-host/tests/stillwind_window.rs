//! programs/stillwind in the window, driven as a player drives it
//! (`--drive`, in a headless compositor the host starts; skipped without
//! weston): the painted meadow still while nothing moves, Ysolde walked
//! behind the boulder with the arrows (the boulder's copy hides her), a
//! typed command walking her up the path into the next scene, whose blob
//! was fetched while she was in the meadow, so that the scene change
//! makes no frame late; the phosphor look; every machine frame within half
//! its budget. With the software renderer, and with the GPU one (llvmpipe
//! in the headless compositor, whose timing is not the program's: there
//! without `expect no-slow`, with `--perf-rule off`). A second run without
//! the Spire's foot in the store: the scene's notice says so.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn have_weston() -> bool {
    let ok = Command::new("weston").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok {
        terse_testkit::skip!("weston", "no weston: --drive needs a headless compositor");
    }
    ok
}

fn program() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/stillwind")
}

/// A store holding the scenes' blobs (all but `leave_out`'s).
fn store(dir: &Path, leave_out: Option<&str>) -> PathBuf {
    let path = dir.join("store");
    let store = terse_net::BlobStore::open(&path, None).unwrap();
    for e in std::fs::read_dir(program().join("blobs")).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if name.ends_with(".scene") && !leave_out.is_some_and(|l| name.starts_with(&format!("{l}-"))) {
            store.put(&std::fs::read(&p).unwrap()).unwrap();
        }
    }
    path
}

fn drive(dir: &Path, script: &str, renderer: &str, store: &Path) -> Output {
    let s = dir.join(format!("script-{renderer}.txt"));
    std::fs::write(&s, script).unwrap();
    Command::new(env!("CARGO_BIN_EXE_terse-host"))
        .arg(program().join("stillwind.terse"))
        .arg("--drive")
        .arg(&s)
        .args(["--renderer", renderer, "--size", "1280x960", "--consent", "deny", "--no-shed"])
        .arg("--store")
        .arg(store)
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

const SCRIPT: &str = "\
settle max 300
settle
expect idle 60
shot meadow.png
hold up 50
hold left 110
settle
expect idle 30
shot rock.png
expect no-slow
type north
press enter
wait 500
settle
expect terminal contains The Spire's Foot
expect no-slow
shot foot.png
type phosphor
press enter
wait 10
settle
shot phosphor.png
type clean
press enter
wait 10
settle
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
    let store = store(dir.path(), None);
    for renderer in ["software", "gpu"] {
        let script = if renderer == "gpu" { SCRIPT.replace("expect no-slow\n", "") } else { SCRIPT.to_string() };
        let o = drive(dir.path(), &script, renderer, &store);
        if no_gpu(renderer, &o) {
            continue;
        }
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        assert!(o.status.success() && out.contains("drive: done, no expectation failed"), "{renderer}: {}", text(&o));
        for shot in ["meadow.png", "rock.png", "foot.png", "phosphor.png"] {
            let p = dir.path().join(shot);
            assert!(p.exists(), "{renderer}: no {shot}");
            std::fs::rename(&p, dir.path().join(format!("{renderer}-{shot}"))).unwrap();
        }
    }
}

#[test]
fn a_missing_scene_says_so_in_the_window() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path(), Some("foot"));
    let script = "settle max 300\nsettle\ntype north\npress enter\nwait 500\nsettle\nexpect terminal contains The Spire's Foot\n\
                  expect page contains could not be fetched\nshot missing.png\nexpect budget 50\nquit\n";
    let o = drive(dir.path(), script, "software", &store);
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(o.status.success() && out.contains("drive: done, no expectation failed"), "{}", text(&o));
}
