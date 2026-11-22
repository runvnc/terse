//! programs/sheet in the window, driven as a user drives it (`--drive`,
//! in a headless compositor the host starts; skipped without weston). Each
//! test is a defect the headless tests did not see:
//!
//! - the chart flashed whenever the page was laid out again (the host drew
//!   a page figure blank until it was rasterised again for the new layout);
//! - the selection box was drawn for the new selection over the old page
//!   for the frames the page took to lay out (the overlay ran ahead);
//! - Ctrl+N twice never cleared the sheet from a keyboard: the Control key's
//!   own key event disarmed it;
//! - Ctrl+G or Ctrl+L with one cell selected said "Select the values first"
//!   instead of removing the chart;
//! - and the window's picture is still when nobody types, at the owner's
//!   window size and at 640 × 480, with both renderers.

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

fn sheet() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/sheet/sheet.terse")
}

/// The sheet driven by `script` in a window of `size`, scratch directories.
fn drive(dir: &Path, script: &str, renderer: &str, size: &str) -> Output {
    let s = dir.join(format!("script-{renderer}-{size}.txt"));
    std::fs::write(&s, script).unwrap();
    Command::new(env!("CARGO_BIN_EXE_terse-host"))
        .arg(sheet())
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

/// Skips the GPU part where the compositor gives no adapter.
fn no_gpu(renderer: &str, o: &Output) -> bool {
    let none = renderer == "gpu" && String::from_utf8_lossy(&o.stderr).contains("no graphics adapter");
    if none {
        terse_testkit::skip!("gpu", "no graphics adapter in the compositor");
    }
    none
}

/// The example with its chart is still when nobody types, and typing
/// into it, moving and holding an arrow key change the cells, the chart's
/// bars and values, but the chart never blinks (it vanished for a frame or
/// more at every key). (Parts of the table may change back within 2
/// pictures, legitimately: keys repeat every 2 frames.)
#[test]
fn the_example_is_still_and_its_chart_never_blinks_while_typing() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = "settle max 300\nexpect idle 120\ndump page > page.txt\ndump state > state.txt\nwatch 240\npress down\nwait 6\npress down\nwait 6\npress right\nwait 6\ntype 77\npress enter\nwait 20\nhold down 50\nwait 20\ntype =B3*2\npress enter\nwait 150\nquit\n";
    for (renderer, size) in [("software", "1900x1000"), ("software", "640x480"), ("gpu", "1900x1000")] {
        let o = drive(dir.path(), script, renderer, size);
        if no_gpu(renderer, &o) {
            continue;
        }
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        assert!(o.status.success(), "{renderer} {size}: {}", text(&o));
        assert!(out.contains("ok: expect idle 120"), "{renderer} {size}: {}", text(&o));
        let watch = out.lines().find(|l| l.contains("watch 240:")).unwrap_or_else(|| panic!("{renderer} {size}: {}", text(&o)));
        assert!(!watch.contains(" 0 changed"), "the typing showed: {watch}");
        // The chart's band in window pixels: its top on the page (screen
        // units), through the screen's rectangle.
        let page = std::fs::read_to_string(dir.path().join("page.txt")).unwrap();
        let state = std::fs::read_to_string(dir.path().join("state.txt")).unwrap();
        let top: f32 = page.lines().find(|l| l.contains("-- figure")).and_then(|l| l.split_whitespace().next()?.parse().ok()).expect("the chart on the page");
        let at = state.split("screen at ").nth(1).unwrap();
        let (xy, rest) = at.split_once(" size ").unwrap();
        let y0: f32 = xy.split(',').nth(1).unwrap().parse().unwrap();
        let h: f32 = rest.split_whitespace().next().unwrap().split(',').next().unwrap().split('x').nth(1).unwrap().parse().unwrap();
        let s = h / 480.0;
        let (band0, band1) = (y0 + top * s, y0 + (top + 90.0) * s);
        for l in out.lines().filter(|l| l.contains(": blink in ")) {
            let y = l.split("y ").nth(1).unwrap();
            let (a, b) = y.split_once("..").unwrap();
            let (a, b): (f32, f32) = (a.parse().unwrap(), b.trim().parse().unwrap());
            assert!(b <= band0 || a >= band1, "{renderer} {size}: the chart (y {band0}..{band1}) blinked: {l}\n{}", text(&o));
        }
    }
}

/// The y of the selection box (screen units) in a `dump vectors`, and the
/// top of row `row`'s number in a `dump page` (this host's layout).
fn box_y(vectors: &str) -> Option<u32> {
    let l = vectors.lines().find(|l| l.starts_with("line ") && l.ends_with("c 1a64d2ff"))?;
    l.split_whitespace().nth(2)?.parse::<u32>().ok().map(|y| y / 16)
}

fn row_top(page: &str, row: u32) -> Option<u32> {
    page.lines().find_map(|l| {
        let (pos, t) = l.split_once('|')?;
        let w: Vec<&str> = pos.split_whitespace().collect();
        (w.len() == 3 && w[2] == "6.0" && t.trim() == row.to_string()).then(|| w[0].parse::<f32>().ok().map(|y| y as u32)).flatten()
    })
}

/// In every picture after a key moves the selection, the box is drawn
/// where the page shows the selected row (its number highlighted, its
/// name on the entry line): the overlay waits for the page's layout.
#[test]
fn the_selection_box_is_where_the_page_shows_the_selection() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut script = String::from("settle max 300\npress down\n");
    for i in 0..4 {
        script += &format!("dump page > p{i}.txt\ndump vectors > v{i}.txt\nwait 1\n");
    }
    script += "press down\n";
    for i in 4..8 {
        script += &format!("dump page > p{i}.txt\ndump vectors > v{i}.txt\nwait 1\n");
    }
    script += "quit\n";
    let o = drive(dir.path(), &script, "software", "1900x1000");
    assert!(o.status.success(), "{}", text(&o));
    for i in 0..8 {
        let page = std::fs::read_to_string(dir.path().join(format!("p{i}.txt"))).unwrap();
        let vectors = std::fs::read_to_string(dir.path().join(format!("v{i}.txt"))).unwrap();
        // The entry line: "A<row>".
        let entry = page.lines().find(|l| l.contains("|A")).unwrap_or("");
        let row: u32 = entry.split('|').nth(1).unwrap_or("").split_whitespace().next().unwrap_or("A0")[1..].parse().unwrap_or(0);
        let top = row_top(&page, row).unwrap_or_else(|| panic!("picture {i}: row {row} not on the page:\n{page}"));
        assert_eq!(box_y(&vectors), Some(top - 3), "picture {i}: the page shows A{row} (its row at y {top}), the box is elsewhere:\n{vectors}");
    }
}

/// Ctrl+N twice and Ctrl+G with one cell, pressed as a keyboard presses
/// them (Control down, the key, its release, Control up).
#[test]
fn control_keys_from_a_keyboard_work() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = "settle max 300\nexpect page contains Bar chart of D3:D7\npress g ctrl\nwait 20\nexpect page lacks Bar chart of\nexpect page lacks Select the values first\npress n ctrl\nwait 5\npress n ctrl\nwait 30\nexpect page contains A new sheet\nexpect page lacks Estimate\nquit\n";
    let o = drive(dir.path(), script, "software", "880x660");
    assert!(o.status.success(), "{}", text(&o));
}

/// A click and a drag in a window larger than the picture (letterboxed)
/// and in one smaller than it select the cells under the pointer.
#[test]
fn the_pointer_selects_the_cell_under_it_at_any_window_size() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Screen units: column C is x 240..320; row 5's number is at y 140 in this host's layout.
    let script = "settle max 300\nclick 280 145\nwait 15\nexpect page contains C5\ndrag 200 100 360 150\nwait 20\nexpect page contains B3:D5\nquit\n";
    for size in ["1900x1000", "640x480"] {
        let o = drive(dir.path(), script, "software", size);
        assert!(o.status.success(), "{size}: {}", text(&o));
    }
}
