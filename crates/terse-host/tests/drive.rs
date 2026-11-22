//! `terse-host --drive` (README "Driving the window", STATE.md 6.5): the
//! window driven by a script and watched, in a headless compositor the
//! host starts for itself (weston; skipped where there is none). Keys
//! typed as a keyboard types them change hello's screen; a page that
//! changes while nobody types is reported, and its expectation fails; a
//! page re-submitted every frame unchanged, figure and all, is still (it
//! made its figure blink before the host kept figures across layouts).

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output};

use terse_core::image::DeviceReq;
use terse_core::worst::{Builder, Call};

/// The tests run one at a time: each runs a window in real time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn have_weston() -> bool {
    let ok = Command::new("weston").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok {
        terse_testkit::skip!("weston", "no weston: --drive needs a headless compositor");
    }
    ok
}

/// `terse-host <image> --drive <script>` with scratch directories, no
/// relay, the renderer given.
fn drive(dir: &Path, image: &Path, script: &str, renderer: &str) -> Output {
    let s = dir.join(format!("script-{renderer}.txt"));
    std::fs::write(&s, script).unwrap();
    Command::new(env!("CARGO_BIN_EXE_terse-host"))
        .arg(image)
        .arg("--drive")
        .arg(&s)
        .args(["--renderer", renderer, "--consent", "deny"])
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

/// A page document: `words` in light grey, then a blue 200 × 100 figure.
fn doc(words: &str) -> Vec<u8> {
    let mut d = vec![0x04, 230, 230, 230, 0x01];
    d.extend((words.len() as u16).to_le_bytes());
    d.extend(words.as_bytes());
    let mut p = vec![0x01, 66, 120, 210, 255];
    for (op, x, y) in [(0x03u8, 0i16, 0i16), (0x04, 3200, 0), (0x04, 3200, 1600), (0x04, 0, 1600)] {
        p.push(op);
        p.extend(x.to_le_bytes());
        p.extend(y.to_le_bytes());
    }
    p.push(0x07);
    d.push(0x08);
    d.extend(200u16.to_le_bytes());
    d.extend(100u16.to_le_bytes());
    d.push(0);
    d.extend((p.len() as u16).to_le_bytes());
    d.extend(p);
    d.push(0x00);
    d
}

/// An image that sets the page's rectangle and submits documents: the
/// cycle's frames each submit the document given (or nothing).
fn page_image(dir: &Path, name: &str, cycle: &[Option<&str>]) -> std::path::PathBuf {
    let mut b = Builder::new();
    let docs: Vec<Option<(u32, u32)>> = cycle.iter().map(|w| w.map(|w| doc(w)).map(|d| (b.blob(&d), d.len() as u32))).collect();
    for d in docs {
        let mut calls = vec![Call::new(0x702, [0, 0, 640, 480])];
        if let Some((at, n)) = d {
            calls.push(Call::new(0x700, [at, n, 0, 0]));
        }
        b.frame(calls);
    }
    let image = b.build(name, &[DeviceReq::new(2, 0), DeviceReq::new(7, 0)]);
    let path = dir.join(format!("{name}.terse"));
    std::fs::write(&path, image).unwrap();
    path
}

/// Typed keys reach hello as a keyboard gives them (key, text, key up,
/// with Shift for capitals), and its screen shows them.
#[test]
fn keys_typed_change_the_screen() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let hello = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/hello/hello.terse");
    let o = drive(dir.path(), &hello, "wait 10\ntype Hi there\nwait 5\nexpect terminal contains Hi there\nshot hi.png\nquit\n", "software");
    assert!(o.status.success(), "{}", text(&o));
    assert!(String::from_utf8_lossy(&o.stdout).contains("ok: terminal contains \"Hi there\""), "{}", text(&o));
    assert!(dir.path().join("hi.png").exists(), "{}", text(&o));
}

/// A page that changes every 3 frames with nobody typing fails `expect
/// idle`; one re-submitted every frame unchanged (its figure too) passes,
/// with both renderers.
#[test]
fn a_changing_picture_is_caught_and_a_still_one_is_not() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let flicker = page_image(dir.path(), "flicker", &[Some("one"), None, None, Some("two"), None, None]);
    let still = page_image(dir.path(), "still", &[Some("the same words")]);
    let script = "settle max 120\nexpect idle 60\nquit\n";
    for renderer in ["software", "gpu"] {
        let o = drive(dir.path(), &flicker, script, renderer);
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        if renderer == "gpu" && String::from_utf8_lossy(&o.stderr).contains("no graphics adapter") {
            terse_testkit::skip!("gpu", "no graphics adapter in the compositor");
            continue;
        }
        assert_eq!(o.status.code(), Some(1), "{renderer}: {}", text(&o));
        assert!(out.contains("the picture changed while idle"), "{renderer}: {}", text(&o));
        let o = drive(dir.path(), &still, script, renderer);
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        assert!(o.status.success(), "{renderer}: {}", text(&o));
        assert!(out.contains("ok: expect idle 60") && !out.contains("blinks"), "{renderer}: {}", text(&o));
    }
}

/// A document whose words change every 10 frames under the same figure
/// (as a spreadsheet's do under its chart): the words change, the figure
/// never blinks (SPEC §page.layout: a host keeps a figure shown across
/// layouts; it used to vanish for the frames each new layout's figure took
/// to rasterise).
#[test]
fn a_figure_does_not_blink_when_the_words_around_it_change() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut cycle = vec![Some("one")];
    cycle.extend([None; 9]);
    cycle.push(Some("two"));
    cycle.extend([None; 9]);
    let image = page_image(dir.path(), "words", &cycle);
    for renderer in ["software", "gpu"] {
        let o = drive(dir.path(), &image, "wait 30\nwatch 150\nwait 160\nquit\n", renderer);
        if renderer == "gpu" && String::from_utf8_lossy(&o.stderr).contains("no graphics adapter") {
            terse_testkit::skip!("gpu", "no graphics adapter in the compositor");
            continue;
        }
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        assert!(o.status.success(), "{renderer}: {}", text(&o));
        assert!(out.contains("watch 150: 150 pictures") && out.contains(" changed"), "{renderer}: {}", text(&o));
        assert!(!out.contains("blinks"), "{renderer}: the figure blinked: {}", text(&o));
    }
}
