//! PERFORMANCE.md R20 in terse-host's window: the host times the responses
//! it gives a program from this computer alone and flags, as its own fault,
//! one past its bound: a content request answered from its store over 100
//! ms, a page document shown whole over 200 ms after its submit. Each test
//! drives programs/stillwind (`--drive`, in a headless compositor the host
//! starts; skipped without weston), whose scenes are blobs it gets from
//! the store and page documents with layered figures, with the record in
//! a scratch directory. `TERSE_HOST_SLOW=store:<ms>` slows every request
//! the host answers itself, `figures:<ms>` every figure's rasterising; the
//! normal run is not flagged.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use terse_perflog::record::{self, Line};

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

/// A store holding the scenes' blobs.
fn store(dir: &Path) -> PathBuf {
    let path = dir.join("store");
    let store = terse_net::BlobStore::open(&path, None).unwrap();
    for e in std::fs::read_dir(program().join("blobs")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "scene") {
            store.put(&std::fs::read(&p).unwrap()).unwrap();
        }
    }
    path
}

/// Stillwind from its start to the Spire's foot: the first scene fetched
/// from the store and shown, then the next, submitted from RAM.
const SCRIPT: &str = "\
settle max 300
settle
type north
press enter
wait 400
settle
expect terminal contains The Spire's Foot
expect no-slow
quit
";

fn drive(dir: &Path, slow: Option<&str>) -> (Output, Vec<Line>) {
    let s = dir.join("script.txt");
    std::fs::write(&s, SCRIPT).unwrap();
    let perf = dir.join("perf");
    std::fs::create_dir_all(&perf).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_terse-host"));
    c.arg(program().join("stillwind.terse"))
        .arg("--drive")
        .arg(&s)
        .args(["--renderer", "software", "--size", "1280x960", "--consent", "deny", "--no-shed", "--perf-log"])
        .arg(&perf)
        .arg("--store")
        .arg(store(dir))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env("TERSE_AUDIO", "off")
        .env_remove("TERSE_DRIVE_DISPLAY")
        .env_remove("TERSE_DRIVE_NESTED")
        .env_remove("TERSE_HOST_SLOW")
        .current_dir(dir);
    if let Some(v) = slow {
        c.env("TERSE_HOST_SLOW", v);
    }
    let out = c.output().unwrap();
    let mut lines = Vec::new();
    for e in std::fs::read_dir(&perf).unwrap().flatten() {
        if e.file_name().to_string_lossy().ends_with(".log") {
            lines.extend(record::parse(&std::fs::read_to_string(e.path()).unwrap()));
        }
    }
    (out, lines)
}

fn text(o: &Output) -> String {
    format!("status {:?}\n--- stdout\n{}--- stderr\n{}", o.status, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn slow_lines<'a>(lines: &'a [Line], what: &str) -> Vec<(u64, f64)> {
    lines.iter().filter_map(|l| l.as_slow()).filter(|s| s.2 == what).map(|s| (s.0, s.1)).collect()
}

/// The normal run: every scene comes from the store at once and is shown
/// within its bound; nothing is flagged, `expect no-slow` passes.
#[test]
fn the_normal_path_is_not_flagged() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (o, lines) = drive(dir.path(), None);
    assert!(o.status.success() && String::from_utf8_lossy(&o.stdout).contains("drive: done, no expectation failed"), "{}", text(&o));
    assert!(lines.iter().all(|l| l.as_slow().is_none()), "{lines:?}\n{}", text(&o));
}

/// A store that takes 300 ms to answer: the scenes' gets are flagged as
/// the host's (`slow ... store`), said on standard error, and `expect
/// no-slow` fails; the program is not blamed (no late frame of its own).
#[test]
fn a_slow_store_is_flagged() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (o, lines) = drive(dir.path(), Some("store:300"));
    let err = String::from_utf8_lossy(&o.stderr);
    let store = slow_lines(&lines, "store");
    assert!(!store.is_empty() && store.iter().all(|s| s.1 >= 300.0), "{lines:?}\n{}", text(&o));
    assert!(err.contains("terse-host: slow: a content request answered from this computer's own store took"), "{}", text(&o));
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stdout).contains("host slow"), "expect no-slow fails: {}", text(&o));
    assert!(!lines.iter().any(|l| matches!(l, Line::Late { verdict: terse_perflog::Verdict::Blamed, image, .. } if image != "-" && !image.is_empty())), "{lines:?}");
    let summary = terse_perflog::summary::summarise(&dir.path().join("perf")).unwrap();
    assert!(summary.contains("slow responses of the host (R20): a content request answered from the host's own store"), "{summary}");
}

/// Figures that take 150 ms each to rasterise: the scene's page is shown
/// whole long after its submit, flagged as the host's (`slow ... page`).
#[test]
fn slow_figures_are_flagged() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (o, lines) = drive(dir.path(), Some("figures:150"));
    let page = slow_lines(&lines, "page");
    assert!(!page.is_empty() && page.iter().all(|s| s.1 > 200.0), "{lines:?}\n{}", text(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("terse-host: slow: a page took"), "{}", text(&o));
    assert!(!o.status.success(), "expect no-slow fails: {}", text(&o));
    assert!(slow_lines(&lines, "store").is_empty(), "{lines:?}");
}
