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
//!
//! Then editing as in other spreadsheets, one script per group of
//! behaviours (the owner could not find how to edit or enter a formula, and
//! a click on another cell during an edit did not do what he expected):
//! typing and the keys that keep an entry; editing in place (F2,
//! double-click, a click on a character of the content line or the cell);
//! pointing at cells and ranges while a formula is typed (click, drag,
//! arrows); selecting, copying, cutting, clearing, undoing; long formulas
//! and errors. Each ends still (`expect idle`), with both renderers.

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
        let (band0, band1) = (y0 + top * s, y0 + (top + 72.0) * s);
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
        // (The page's first line names the document generation and the one
        // shown: if this fails, it tells whether the host showed an older
        // layout than the one the sheet's locates were answered from.)
        assert_eq!(
            box_y(&vectors),
            Some(top - 3),
            "picture {i}: the page shows A{row} (its row at y {top}), the box is elsewhere:\n{}\n{vectors}\n{page}",
            page.lines().next().unwrap_or("")
        );
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
    let script = "settle max 300\nexpect page contains -- figure 640x72\npress g ctrl\nwait 20\nexpect page contains Chart removed\nexpect page lacks -- figure\nexpect page lacks Select the values first\npress n ctrl\nwait 5\npress n ctrl\nwait 30\nexpect page contains A new sheet\nexpect page lacks Estimate\nquit\n";
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
    // Screen units: column C is x 240..320; row 5's number is at y 154 in this host's layout (below the menu bar).
    let script = "settle max 300\nclick 280 159\nwait 15\nexpect page contains C5\ndrag 200 114 360 164\nwait 20\nexpect page contains B3:D5\nquit\n";
    for size in ["1900x1000", "640x480"] {
        let o = drive(dir.path(), script, "software", size);
        assert!(o.status.success(), "{size}: {}", text(&o));
    }
}

/// Runs an editing script (its lines joined) with both renderers at the
/// owner's window size: every expectation holds and it ends still.
/// Screen units used: columns B to F are x 160..560 in steps of 80; row r's
/// middle is y 74 + 22 (r − 1) in this host's layout (below the menu bar,
/// 16 units); the content line is y 18..37, its text from x 41.
fn editing_script(name: &str, lines: &[&str]) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = lines.join("\n") + &format!("\nshot {name}.png\nexpect idle 60\nquit\n");
    for renderer in ["software", "gpu"] {
        let o = drive(dir.path(), &script, renderer, "1900x1000");
        if no_gpu(renderer, &o) {
            continue;
        }
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(o.status.success(), "{name}, {renderer}: {}", text(&o));
        assert!(out.contains("ok: expect idle 60"), "{name}, {renderer}: {}", text(&o));
    }
}

/// Typing replaces the cell (its entry shown in the cell and on the
/// content line, with the cursor); = starts a formula; Enter keeps and
/// moves down, Tab right and Enter after Tabs back to the first column;
/// Esc puts the old content back; an arrow keeps the entry and moves;
/// Shift+Enter moves up; dependents are recalculated.
#[test]
fn typing_entries_and_formulas() {
    editing_script(
        "typing",
        &[
            "settle max 300",
            "expect page contains An example estimate: column D is formulas",
            "click 360 228",
            "wait 8",
            "expect page contains D8   =SUM(D3:D7)   = 1,518.35",
            "click 440 118",
            "wait 5",
            "type 42",
            "wait 5",
            "expect page contains E3   42|",
            "expect page contains Enter  Enter keeps",
            "press enter",
            "wait 5",
            "type =e3*2",
            "press tab",
            "wait 5",
            "type x",
            "press enter",
            "wait 10",
            "expect page contains E5",
            "press up",
            "wait 8",
            "expect page contains E4   =E3*2   = 84",
            "type 7",
            "wait 3",
            "press esc",
            "wait 8",
            "expect page contains E4   =E3*2   = 84",
            "press right",
            "wait 3",
            "expect page contains F4   x",
            "press left",
            "press up",
            "type 50",
            "press down",
            "wait 10",
            "expect page contains E4   =E3*2   = 100",
            "press enter shift",
            "wait 5",
            "expect page contains E3   50",
        ],
    );
}

/// A double-click and F2 edit the formula in place, with the cursor at its
/// end; the arrows move it; a click on a character of the content line, or
/// of the cell being edited, puts the cursor before it (each character is
/// an anchor of the page); Backspace deletes; Esc cancels.
#[test]
fn editing_in_place() {
    editing_script(
        "edit",
        &[
            "settle max 300",
            "click 360 118",
            "wait 5",
            "click 360 118",
            "wait 8",
            "expect page contains D3   =B3*C3|",
            "expect page contains Edit",
            "press left",
            "press left",
            "type 1+",
            "wait 5",
            "expect page contains D3   =B3*1+|C3",
            "press esc",
            "wait 8",
            "expect page contains D3   =B3*C3   = 546",
            "click 74 26",
            "wait 8",
            "expect page contains D3   =B3|*C3",
            "type +1",
            "press enter",
            "wait 3",
            "press up",
            "wait 10",
            "expect page contains D3   =B3+1*C3   = 57.50",
            "press f2",
            "wait 5",
            "press end",
            "wait 5",
            "click 385 118",
            "wait 8",
            "expect page contains D3   =B3+1*C|3",
            "press backspace",
            "press end",
            "press backspace",
            "wait 3",
            "expect page contains D3   =B3+1*|",
            "press esc",
            "wait 8",
        ],
    );
}

/// While a formula is typed and the cursor follows =, an operator or a
/// bracket, a click on another cell puts its reference in (a second click
/// replaces it), a drag a range, the arrows move it (Shift grows it);
/// elsewhere a click on another cell keeps the entry and selects that cell.
#[test]
fn pointing_at_cells_in_a_formula() {
    editing_script(
        "point",
        &[
            "settle max 300",
            "click 440 118",
            "wait 5",
            "type =",
            "click 200 118",
            "wait 8",
            "expect page contains E3   =B3|",
            "expect page contains Point",
            "click 280 118",
            "wait 8",
            "expect page contains E3   =C3|",
            "type *",
            "click 200 118",
            "wait 8",
            "press enter",
            "wait 10",
            "press up",
            "wait 8",
            "expect page contains E3   =C3*B3   = 546",
            "click 440 140",
            "wait 5",
            "type =sum(",
            "pointer 360 118 1",
            "wait 3",
            "pointer 360 164 1",
            "wait 3",
            "pointer 360 206 1",
            "wait 8",
            "expect page contains E4   =sum(D3:D7|",
            "pointer 360 206 0",
            "wait 3",
            "press up",
            "wait 3",
            "expect page contains E4   =sum(D6|",
            "press up shift",
            "wait 8",
            "expect page contains E4   =sum(D5:D6|",
            "press down",
            "wait 3",
            "expect page contains E4   =sum(D6|",
            "type )",
            "press enter",
            "wait 10",
            "click 440 140",
            "wait 8",
            "expect page contains E4   =SUM(D6)   = 608",
            "type abc",
            "click 120 250",
            "wait 10",
            "expect page contains A9",
            "expect page lacks Point",
            "click 440 140",
            "wait 8",
            "expect page contains E4   abc",
        ],
    );
}

/// Shift+click extends the selection; Ctrl+C and Ctrl+V paste the copied
/// cells (the window delivers the paste key, SPEC §events); a click on a column letter
/// or a row number selects it; Delete clears; Ctrl+Z and Ctrl+Y undo and
/// redo; Ctrl+X and a paste over a dragged range move the cells.
#[test]
fn selecting_copying_and_undoing() {
    editing_script(
        "select",
        &[
            "settle max 300",
            "click 200 118",
            "down lshift",
            "click 280 184",
            "up lshift",
            "wait 8",
            "expect page contains B3:C6",
            "press c ctrl",
            "wait 3",
            "expect page contains Copied",
            "click 520 250",
            "wait 3",
            "press v ctrl",
            "wait 15",
            "expect page contains Pasted",
            "expect page contains F9:G12",
            "click 520 250",
            "wait 5",
            "expect page contains F9   12",
            "click 360 52",
            "wait 8",
            "expect page contains D1:D99",
            "click 40 206",
            "wait 8",
            "expect page contains A7:Z7",
            "press delete",
            "wait 15",
            "expect page lacks Delivery",
            "press z ctrl",
            "wait 15",
            "expect page contains Delivery",
            "expect page contains Undone",
            "press z ctrl",
            "wait 15",
            "expect page lacks F9   12",
            "press y ctrl",
            "wait 15",
            "expect page contains Redone",
            "press x ctrl",
            "drag 200 118 280 162",
            "wait 8",
            "expect page contains B3:C5",
            "press v ctrl",
            "wait 20",
            "expect page contains Moved",
            "expect page contains 45.50",
        ],
    );
}

/// A formula longer than the content line shows the part around the
/// cursor; errors have their conventional names, and the content line
/// says what they mean; a circular reference is reported, not a hang.
#[test]
fn long_formulas_and_errors() {
    let long = "=D3+D4+D5+D6+D7".repeat(7).replace("+D7=", "+D7+") + "-1";
    let typed = format!("type {long}");
    editing_script(
        "errors",
        &[
            "settle max 300",
            "click 440 118",
            &typed,
            "wait 5",
            "expect page contains D6+D7-1|",
            "press home",
            "wait 5",
            "expect page contains E3   |=D3+D4",
            "press enter",
            "wait 10",
            "click 440 118",
            "wait 5",
            "expect page contains 10,627.45",
            "click 440 162",
            "type =1/0",
            "press enter",
            "type =foo(1)",
            "press enter",
            "wait 10",
            "click 440 162",
            "wait 5",
            "expect page contains #DIV/0! division by zero",
            "click 440 184",
            "wait 5",
            "expect page contains #NAME? no function of that name",
            "type =E7+1",
            "press enter",
            "type =E6",
            "press enter",
            "wait 10",
            "click 440 206",
            "wait 5",
            "expect page contains #CYCLE! circular reference",
        ],
    );
}

/// Control chords type nothing (the owner saw Ctrl+C put a "c" in the
/// cell: a window reports the chord's character too, and the host now
/// keeps it from the program, SPEC §events); Ctrl+V pastes the cells copied
/// in the sheet; after Esc it pastes the text of a paste, a tab starting
/// the next cell (the drive's `down v ctrl`, `text`, `up v ctrl` are a
/// paste as the window delivers it: the key, the text, then the key's
/// release, held back until the text is in).
#[test]
fn chords_and_pasting() {
    editing_script(
        "paste",
        &[
            "settle max 300",
            "press c ctrl",
            "wait 5",
            "expect page contains Copied",
            "expect page lacks A1   c|",
            "click 520 118",
            "press v ctrl",
            "wait 10",
            "expect page contains F3   Estimate",
            "press esc",
            "wait 3",
            "expect page contains Copy forgotten",
            "click 520 140",
            "down v ctrl",
            "text 7\t8",
            "up v ctrl",
            "wait 12",
            "expect page contains F4:G4   7",
            "click 600 140",
            "wait 5",
            "expect page contains G4   8",
            "press 1 ctrl shift",
            "wait 8",
            "expect page contains 8.00",
            "expect page lacks G4   !",
        ],
    );
}

/// Ctrl+Plus inserts a row above the selection, the subtotal's range
/// grows; Ctrl+Minus deletes it again; a click on a column letter and
/// Ctrl+Plus inserts a column, undone with Ctrl+Z.
#[test]
fn rows_and_columns() {
    editing_script(
        "rows",
        &[
            "settle max 300",
            "click 120 162",
            "press 0x2e ctrl shift",
            "wait 40",
            "click 360 250",
            "wait 5",
            "expect page contains D9   =SUM(D3:D8)   = 1,518.35",
            "click 120 162",
            "press 0x2d ctrl",
            "wait 40",
            "click 360 228",
            "wait 5",
            "expect page contains D8   =SUM(D3:D7)   = 1,518.35",
            "click 200 52",
            "press 0x2e ctrl shift",
            "wait 40",
            "expect page contains Inserted 1 column",
            "press z ctrl",
            "wait 40",
        ],
    );
}

/// Ctrl+Shift+A sorts the items by amount with the header kept;
/// Ctrl+Shift+5 shows a percentage; each undone with Ctrl+Z. (Apart from
/// the rows and columns so that each script ends before a slow-frame
/// notice of a loaded machine, shown from its start, times out.)
#[test]
fn sorting_and_formats() {
    editing_script(
        "sort",
        &[
            "settle max 300",
            "drag 360 96 120 206",
            "wait 5",
            "press a ctrl shift",
            "wait 40",
            "expect page contains kept as the header",
            "click 120 118",
            "wait 5",
            "expect page contains A3   Delivery",
            "press z ctrl",
            "wait 40",
            "click 280 206",
            "press 5 ctrl shift",
            "wait 8",
            "expect page contains 4000%",
            "press z ctrl",
            "wait 10",
        ],
    );
}

/// The menu bar: a click on File opens it (still afterwards), Esc closes
/// it; F10 and the arrows reach Format, Enter applies an item; File, Save
/// as asks for a name on the content line and keeps the sheet under it;
/// File, Open lists it, and a click on it opens it.
#[test]
fn menus_save_as_and_open() {
    editing_script(
        "menus",
        &[
            "settle max 300",
            "expect terminal contains File",
            "click 20 8",
            "wait 5",
            "expect terminal contains Save as",
            "expect idle 30",
            "press esc",
            "wait 5",
            "expect terminal lacks Save as",
            "expect idle 30",
            "click 280 206",
            "press f10",
            "press right",
            "press right",
            "press right",
            "wait 3",
            "expect terminal contains Percent",
            "press down",
            "press down",
            "press enter",
            "wait 8",
            "expect page contains 4000%",
            "press f10",
            "press down",
            "press down",
            "press down",
            "press enter",
            "wait 5",
            "expect page contains Save as",
            "type Budget",
            "press enter",
            "wait 20",
            "expect page contains Saved",
            "expect terminal contains Budget",
            "press n ctrl",
            "press n ctrl",
            "wait 20",
            "expect page lacks Estimate",
            "press o ctrl",
            "wait 10",
            "expect terminal contains Budget",
            "click 30 24",
            "wait 30",
            "expect page contains Opened Budget",
            "expect page contains Estimate",
        ],
    );
}
