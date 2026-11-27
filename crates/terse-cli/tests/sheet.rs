//! programs/sheet/sheet.terse: the example (an estimate for a service),
//! numbers and formulas typed in scripted sessions and read back from the
//! page as the reference layout shows it, errors and cycles, editing,
//! selection with keys and the pointer (and the selection box on the
//! vector display), copy, paste and fill, charts, saving and reopening,
//! publishing a document and opening it, a host without the optional
//! devices, the frame budget (a whole sheet of formulas recalculated
//! across frames), and the image is up to date. Set UPDATE_GOLDEN=1 to
//! rewrite programs/sheet/expected/.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use terse_core::page::{Layout, reference_layout};
use terse_core::vector::{Primitive, decode};
use terse_core::{Grants, Image, Machine, Script};
use terse_net::service::Headless;
use terse_net::{BlobStore, Consent, ContentService, FeedStore, Identity, Node, Owner, Records};

const IMAGE: &[u8] = include_bytes!("../../../programs/sheet/sheet.terse");
const ESTIMATE: &str = include_str!("../../../programs/sheet/estimate.tsv");
const RAM_PAGES: u8 = 42;
/// The width of every column on the page, in screen units (sheet.c COLW).
const COLW: u32 = 80;

fn owner() -> Owner {
    Owner::Image(Image::parse(IMAGE).unwrap().id)
}

fn identity() -> Identity {
    Identity::from_seed([42; 32])
}

/// The document header of the sheet format (SPEC §content.documents).
fn header() -> Vec<u8> {
    let mut h = b"TDOC\0".to_vec();
    h.extend([0; 32]);
    h.push(5);
    h.extend(b"sheet");
    h
}

/// A scripted session: each step one frame after the last.
#[derive(Default)]
struct Keys {
    lines: String,
    f: u32,
}

impl Keys {
    fn new() -> Self {
        Keys { lines: String::new(), f: 3 }
    }
    fn at(mut self, what: &str) -> Self {
        self.lines += &format!("{} {what}\n", self.f);
        self.f += 1;
        self
    }
    fn press(self, key: &str) -> Self {
        self.at(&format!("press {key}"))
    }
    fn text(self, t: &str) -> Self {
        self.at(&format!("text {t}"))
    }
    /// Types an entry and Enter.
    fn enter(self, t: &str) -> Self {
        self.text(t).press("enter")
    }
    fn wait(mut self, n: u32) -> Self {
        self.f += n;
        self
    }
    /// Ctrl+N twice: a new, empty sheet.
    fn new_sheet(self) -> Self {
        self.press("n ctrl").press("n ctrl")
    }
    fn goto(self, cell: &str) -> Self {
        // Ctrl+Home, then right and down.
        let (c, r) = parse_name(cell);
        let mut k = self.press("home ctrl");
        for _ in 0..c {
            k = k.press("right");
        }
        for _ in 0..r {
            k = k.press("down");
        }
        k
    }
}

fn parse_name(cell: &str) -> (u32, u32) {
    let col = cell.as_bytes()[0] - b'A';
    let row: u32 = cell[1..].parse().unwrap();
    (col as u32, row - 1)
}

struct Run {
    m: Machine,
    h: Headless,
    dir: tempfile::TempDir,
    used: Vec<u64>,
    frames_run: u32,
}

impl Run {
    fn new(keys: &Keys) -> Self {
        Run::with(&keys.lines, Consent::Deny, None, None, |_| {})
    }

    /// A run on a new store and data directory (or `data`), with `setup`
    /// called on the store before the content service starts and `grants`
    /// the optional devices (all by default).
    fn with(script: &str, consent: Consent, data: Option<&Path>, grants: Option<Grants>, setup: impl FnOnce(&Path)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        setup(&store);
        let mut node = Node::new(BlobStore::open(&store, None).unwrap(), None);
        node.set_identity(identity());
        node.set_records(Records::in_data_dir(data.unwrap_or(&dir.path().join("data"))));
        let image = Image::parse(IMAGE).unwrap();
        let (m, content) = match grants {
            Some(g) => {
                let has_content = g.0.get(2).copied().unwrap_or(false);
                let content = has_content.then(|| ContentService::start_with_consent(node, consent));
                (Machine::with_grants(&image, g), content)
            }
            None => (Machine::new(&image), Some(ContentService::start_with_consent(node, consent))),
        };
        let h = Headless::new(content, Script::parse(script).unwrap());
        Run { m, h, dir, used: Vec::new(), frames_run: 0 }
    }

    fn frames(&mut self, n: u32) -> &mut Self {
        for _ in 0..n {
            let r = self.h.frame(&mut self.m);
            assert_eq!(r.stopped, None, "stopped at frame {}", self.m.frame());
            assert!(r.waited, "frame {} did not end in wait_frame ({} units)", self.m.frame(), r.used);
            self.used.push(r.used);
            self.frames_run += 1;
        }
        self
    }

    /// Runs the session to its end and a few frames more.
    fn finish(&mut self, keys: &Keys) -> &mut Self {
        let n = (keys.f + 4).saturating_sub(self.frames_run);
        self.frames(n)
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn layout(&self) -> Layout {
        let page = self.m.page().unwrap();
        page.document().map(|d| reference_layout(d, page.region().w)).unwrap_or_default()
    }

    fn view(&self) -> String {
        self.layout().dump(self.m.page().unwrap())
    }

    fn line_text(&self, i: usize) -> String {
        self.layout().lines[i].spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// The entry line: the cell's name and content.
    fn entry(&self) -> String {
        self.line_text(0)
    }

    /// The status line (the last line of the page).
    fn status(&self) -> String {
        let l = self.layout();
        l.lines.last().unwrap().spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// The grid as shown: (row number, column letter) to the text of the
    /// cell, from the table between the header and the hidden last row.
    fn grid(&self) -> BTreeMap<String, String> {
        let l = self.layout();
        let mut out = BTreeMap::new();
        let Some(head) = l.lines.iter().position(|l| l.spans.iter().any(|s| s.style.anchor == 2000)) else { return out };
        // The header's cells are lines at the same height.
        let y = l.lines[head].y;
        let letters: Vec<(u32, String)> =
            l.lines.iter().filter(|l| l.y == y).flat_map(|l| &l.spans).map(|s| (s.x0 / 2 / COLW, s.text.clone())).collect();
        let col = |x0: u32| letters.iter().find(|(k, _)| *k == x0 / 2 / COLW).map(|(_, l)| l.clone());
        let mut row = String::new();
        let first = l.lines.iter().position(|l| l.y > y).unwrap();
        for line in &l.lines[first..] {
            if line.spans.iter().any(|s| s.style.anchor == 999) {
                break;
            }
            if let Some(n) = line.spans.iter().find(|s| (1000..1100).contains(&s.style.anchor)) {
                row = n.text.clone();
            }
            for s in &line.spans {
                if s.style.anchor != 0 {
                    continue;
                }
                let name = format!("{}{row}", col(s.x0).unwrap_or_else(|| "?".into()));
                out.entry(name).or_insert_with(String::new).push_str(&s.text);
            }
        }
        out
    }

    fn cell(&self, name: &str) -> String {
        self.grid().get(name).cloned().unwrap_or_default()
    }

    fn shapes(&self) -> Vec<Primitive> {
        decode(self.m.vector_list()).collect()
    }

    /// The selected cell's box on the vector display: its corners in
    /// screen units.
    fn sel_box(&self) -> Option<(i16, i16, i16, i16)> {
        let lines: Vec<_> = self
            .shapes()
            .into_iter()
            .filter_map(|p| match p {
                Primitive::Line { x0, y0, x1, y1, width: 32, .. } => Some((x0 / 16, y0 / 16, x1 / 16, y1 / 16)),
                _ => None,
            })
            .collect();
        (lines.len() == 4).then(|| (lines[0].0, lines[0].1, lines[1].2, lines[1].3))
    }

    /// The figures of the page: (width, height).
    fn figures(&self) -> Vec<(u16, u16)> {
        self.layout().lines.iter().filter_map(|l| l.figure.map(|f| (f.width, f.height))).collect()
    }
}

fn record(data: &Path, name: &str) -> Option<Vec<u8>> {
    Records::in_data_dir(data).load(&owner(), name.as_bytes()).unwrap()
}

fn report(what: &str, used: &[u64]) -> u64 {
    let stats = common::FrameStats::new(used);
    eprintln!("sheet {what}: {}", stats.describe());
    assert!(common::pct(stats.busiest) < 80.0, "{what}: busiest frame {} units", stats.busiest);
    stats.busiest
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("sheet/expected/{name}.txt"), got);
}

#[test]
fn the_example_is_an_estimate_with_a_chart() {
    let mut run = Run::new(&Keys::new());
    run.frames(8);
    let g = run.grid();
    for (cell, want) in [
        ("A1", "Estimate"),
        ("A2", "Item"),
        ("D2", "Amount"),
        ("A3", "Panels"),
        ("D3", "546"),
        ("D4", "243.75"),
        ("D5", "80.60"),
        ("D6", "608"),
        ("D7", "40"),
        ("C8", "Subtotal"),
        ("D8", "1,518.35"),
        ("B9", "8.25%"),
        ("D9", "125.26"),
        ("D10", "1,643.61"),
    ] {
        assert_eq!(g.get(cell).map(String::as_str), Some(want), "{cell}: {g:?}");
    }
    assert_eq!(run.entry(), "A1   Estimate");
    assert!(run.status().starts_with("An example estimate: column D is formulas"), "{}", run.status());
    assert!(run.status().contains("= to start a formula"), "{}", run.status());
    // The chart: a figure across the page, then the labels and values.
    assert_eq!(run.figures(), vec![(640, 72)]);
    let text: String = run.layout().lines.iter().map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>() + "\n").collect();
    assert!(text.contains("Posts\nConcrete\nLabour (h)\nDelivery\n"), "{text}");
    // The example in the program is programs/sheet/estimate.tsv.
    let rom = &Image::parse(IMAGE).unwrap().rom;
    assert!(rom.windows(ESTIMATE.len()).any(|w| w == ESTIMATE.as_bytes()), "estimate.tsv is the example in sheet.c");
    golden("estimate", &run.view());
    report("the example", &run.used);
}

/// Numbers, text and formulas typed into a new sheet, read back as shown.
#[test]
fn formulas() {
    let entries = [
        ("2", "2"),
        ("3.5", "3.50"),
        ("=A1+A2*2", "9"),
        ("=(A1+A2)*2", "11"),
        ("=SUM(A1:A3)", "14.50"),
        ("=AVERAGE(A1:A2)", "2.75"),
        ("=MIN(A1:A3)", "2"),
        ("=MAX(A1,A2,A3)", "9"),
        ("=COUNT(A1:A8)", "8"),
        ("=ROUND(10/3,2)", "3.33"),
        ("=10/3", "3.3333"),
        ("=2^10-ABS(-5)", "1,019"),
        ("=2^-2", "0.25"),
        ("=IF(A1>1,\"big\",\"small\")", "big"),
        ("=IF(A1<>2,1)", "FALSE"),
        ("=A1=2", "TRUE"),
        ("=ROUND(1234.5,-2)", "1,200"),
        ("1,250.50", "1,250.50"),
        ("-7", "-7"),
        ("15%", "15%"),
        ("=A20*200", "30"),
        ("Hello", "Hello"),
        ("'42", "42"),
        ("=-(A1+1)%", "-0.03"),
    ];
    let mut k = Keys::new().new_sheet();
    for (t, _) in &entries {
        k = k.enter(t);
    }
    // Rows 1-16 are shown from A1.
    let k1 = Keys { lines: k.lines.clone(), f: k.f }.goto("A1");
    let mut run = Run::new(&k1);
    run.finish(&k1);
    let g = run.grid();
    for (i, (t, want)) in entries.iter().enumerate().take(16) {
        let name = format!("A{}", i + 1);
        assert_eq!(g.get(&name).map(String::as_str).unwrap_or(""), *want, "{name} = {t}: {g:?}");
    }
    // The rest where the typing left the view (rows 10-25).
    let k2 = Keys { lines: k.lines.clone(), f: k.f };
    let mut run = Run::new(&k2);
    run.finish(&k2);
    let g = run.grid();
    for (i, (t, want)) in entries.iter().enumerate().skip(16) {
        let name = format!("A{}", i + 1);
        assert_eq!(g.get(&name).map(String::as_str).unwrap_or(""), *want, "{name} = {t}: {g:?}");
    }
    // The entry line shows the formula and its value.
    let k3 = k.goto("A3");
    let mut run = Run::new(&k3);
    run.finish(&k3);
    assert_eq!(run.entry(), "A3   =A1+A2*2   = 9");
    report("typing formulas", &run.used);
}

#[test]
fn errors() {
    let entries = [
        ("=1/0", "#DIV/0!"),
        ("=A1+1", "#DIV/0!"),
        ("=1+", "#ERROR!"),
        ("=FOO(1)", "#NAME?"),
        ("=1*/2", "#ERROR!"),
        ("=A100", "#REF!"),
        ("=AA1", "#REF!"),
        ("=\"a\"+1", "#VALUE!"),
        ("=A1:A2", "#VALUE!"),
        ("=900000000000000*10", "#NUM!"),
        ("=2^0.5", "#NUM!"),
        ("=COUNT(A1:A11)", "0"),
        ("=SUM(A13:A13)+1", "#CYCLE!"),
        ("=total", "#NAME?"),
    ];
    let mut k = Keys::new().new_sheet();
    for (t, _) in &entries {
        k = k.enter(t);
    }
    let mut run = Run::new(&k);
    run.finish(&k);
    let g = run.grid();
    for (i, (t, want)) in entries.iter().enumerate() {
        let name = format!("A{}", i + 1);
        assert_eq!(g.get(&name).map(String::as_str).unwrap_or(""), *want, "{name} = {t}: {g:?}");
    }
}

/// A cycle makes every cell of it, and every cell that uses one, #CYCLE!;
/// breaking it brings the values back.
#[test]
fn cycles() {
    let k = Keys::new().new_sheet().enter("=B1").goto("B1").enter("=C1+1").goto("C1").enter("=A1").goto("D1").enter("=B1*2");
    let mut run = Run::new(&k);
    run.finish(&k);
    let g = run.grid();
    for c in ["A1", "B1", "C1", "D1"] {
        assert_eq!(g[c], "#CYCLE!", "{c}: {g:?}");
    }
    let k = k.goto("C1").enter("5");
    let mut run = Run::new(&k);
    run.finish(&k);
    let g = run.grid();
    assert_eq!((g["A1"].as_str(), g["B1"].as_str(), g["C1"].as_str(), g["D1"].as_str()), ("6", "6", "5", "12"));
}

#[test]
fn editing_an_entry() {
    // F2 edits the content; the caret moves; Backspace and typing change it.
    let k = Keys::new().new_sheet().enter("=1+2").press("up").press("f2").press("left").press("backspace").text("5");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "A1   =15|2");
    // Esc cancels.
    let k2 = Keys { lines: k.lines.clone(), f: k.f }.press("esc");
    let mut run = Run::new(&k2);
    run.finish(&k2);
    assert_eq!(run.cell("A1"), "3");
    assert_eq!(run.entry(), "A1   =1+2   = 3");
    // Enter keeps it and moves down; Tab moves right, and Enter after it
    // goes to the next row where the Tabs began.
    let k3 = Keys { lines: k.lines.clone(), f: k.f }.press("enter").text("x").press("tab").text("y").press("enter");
    let mut run = Run::new(&k3);
    run.finish(&k3);
    let g = run.grid();
    assert_eq!((g["A1"].as_str(), g["A2"].as_str(), g["B2"].as_str()), ("152", "x", "y"));
    assert_eq!(run.entry(), "A3   ");
    // Delete clears the selection; a new entry replaces the old.
    let k4 = Keys { lines: k3.lines.clone(), f: k3.f }.goto("A2").press("right shift").press("delete").goto("A1").enter("7");
    let mut run = Run::new(&k4);
    run.finish(&k4);
    let g = run.grid();
    assert_eq!(g.get("A2"), None);
    assert_eq!(g.get("B2"), None);
    assert_eq!(g["A1"], "7");
}

/// Modes of an entry, as in other spreadsheets: typing replaces the cell
/// and an arrow keeps the entry and moves; while a formula is typed, an
/// arrow after an operator points at a cell (Shift grows a range) and puts
/// its reference in; F2 switches to moving the text cursor; Esc restores
/// the old content; the entry shows in its cell with the cursor; formulas
/// are kept in capitals with their brackets closed.
#[test]
fn entering_and_pointing_with_keys() {
    let base = Keys::new().new_sheet().enter("5").enter("7").enter("11").goto("A1");
    // Typing replaces; Right keeps it and moves right.
    let k = Keys { lines: base.lines.clone(), f: base.f }.text("8").press("right");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("A1").as_str(), run.entry().as_str()), ("8", "B1   "));
    // Pointing: = then Down from B1... (A4: =, Up points at A3, Shift+Up
    // grows it to A2:A3; + then Up points at A3 again).
    let k = Keys { lines: base.lines.clone(), f: base.f }.goto("A4").text("=sum(").press("up").press("up shift").wait(1);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "A4   =sum(A2:A3|");
    assert!(run.status().starts_with("Point"), "{}", run.status());
    let k = Keys { lines: k.lines.clone(), f: k.f }.text(")+").press("up").press("enter");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.cell("A4"), "29");
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("up");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "A4   =SUM(A2:A3)+A3   = 29");
    // An open bracket is closed; a lowercase reference is read.
    let k = Keys { lines: base.lines.clone(), f: base.f }.goto("B1").enter("=max(a1:a3").press("up");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "B1   =MAX(A1:A3)   = 11");
    // F2: the arrows move the cursor; the entry shows in its cell too; Esc
    // puts the old content back.
    let k = Keys { lines: base.lines.clone(), f: base.f }.press("f2").press("left").text("0");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "A1   0|5");
    assert!(run.status().starts_with("Edit"), "{}", run.status());
    let shown: String = run.layout().lines.iter().flat_map(|l| &l.spans).filter(|s| (3200..3400).contains(&s.style.anchor)).map(|s| s.text.clone()).collect();
    assert_eq!(shown, "05", "the cell shows the entry, each character an anchor");
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("esc");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("A1").as_str(), run.entry().as_str()), ("5", "A1   5"));
    // F2 while typing a formula switches to the cursor; again, to pointing.
    let k = Keys { lines: base.lines.clone(), f: base.f }.goto("C2").text("=1+").press("f2").press("left").text("0").press("right").press("f2").press("up").wait(1);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "C2   =10+C1|");
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("enter");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.cell("C2"), "10");
    // F4 cycles $ on the reference at the cursor.
    let k = Keys { lines: base.lines.clone(), f: base.f }.goto("C1").text("=A1").press("f4").wait(1);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "C1   =$A$1|");
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("f4").press("f4").press("f4").press("f4");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "C1   =$A$1|");
}

/// Undo and redo cover entries, pastes, fills, clears and moves; a new
/// change after an undo drops what was undone.
#[test]
fn undo_and_redo() {
    let base = Keys::new().new_sheet().enter("1").enter("2").enter("=A1+A2");
    let k = Keys { lines: base.lines.clone(), f: base.f }.press("z ctrl").wait(3);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.cell("A3"), "");
    assert_eq!(run.entry(), "A3   ", "the selection goes to the change undone");
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("z ctrl").press("z ctrl").wait(3).press("y ctrl").wait(3);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("A1").as_str(), run.cell("A2").as_str(), run.cell("A3").as_str()), ("1", "", ""));
    // Ctrl+Shift+Z redoes too; then nothing is left to redo.
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("z ctrl shift").wait(3).press("y ctrl").wait(3).press("y ctrl").wait(3);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.cell("A3"), "3");
    assert!(run.status().starts_with("Nothing to redo"), "{}", run.status());
    // A paste, a fill and a clear, undone one by one.
    let k = Keys { lines: base.lines.clone(), f: base.f }
        .goto("A1")
        .press("down shift")
        .press("down shift")
        .press("c ctrl")
        .goto("B1")
        .press("v ctrl")
        .wait(3)
        .goto("B1")
        .press("right shift")
        .press("down shift")
        .press("down shift")
        .press("r ctrl")
        .wait(3)
        .press("delete")
        .wait(3);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("B3").as_str(), run.cell("C3").as_str()), ("", ""));
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("z ctrl").wait(3);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("B3").as_str(), run.cell("C3").as_str()), ("3", "3"));
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("z ctrl").wait(3);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("B3").as_str(), run.cell("C3").as_str()), ("3", ""));
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("z ctrl").wait(3);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("B1").as_str(), run.cell("B3").as_str()), ("", ""));
    // A new entry after an undo: nothing to redo.
    let k = Keys { lines: k.lines.clone(), f: k.f }.goto("D1").enter("9").press("y ctrl").wait(3);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert!(run.status().starts_with("Nothing to redo"), "{}", run.status());
    assert_eq!(run.cell("B1"), "");
}

/// Copying keeps $ references and moves the others; a cut and paste
/// moves the cells (their formulas unchanged); one cell pastes into a
/// whole selected range.
#[test]
fn absolute_references_cut_and_paste_into_a_range() {
    let k = Keys::new()
        .new_sheet()
        .enter("2")
        .enter("3")
        .goto("B1")
        .enter("=A1*$A$1")
        .goto("B1")
        .press("c ctrl")
        .goto("C2")
        .press("v ctrl")
        .wait(3);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "C2   =B2*$A$1   = 0");
    let k = Keys { lines: k.lines.clone(), f: k.f }.goto("B2").press("v ctrl").wait(3).goto("B2");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "B2   =A2*$A$1   = 6");
    // A$1 and $A1.
    let k = Keys::new().new_sheet().enter("=A$1+$A1").press("up").press("c ctrl").goto("C3").press("v ctrl").wait(3).goto("C3");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "C3   =C$1+$A3   = 0");
    // Cut and paste: moved, the source emptied, the formula as it was.
    let k = Keys::new().new_sheet().enter("4").enter("=A1*2").press("up").press("x ctrl").goto("C5").press("v ctrl").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("A2").as_str(), run.cell("C5").as_str()), ("", "8"));
    assert_eq!(run.entry(), "C5   =A1*2   = 8");
    // One cell into a range.
    let k = Keys::new().new_sheet().enter("7").press("up").press("c ctrl").goto("B1").press("down shift").press("down shift").press("v ctrl").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("B1").as_str(), run.cell("B2").as_str(), run.cell("B3").as_str()), ("7", "7", "7"));
}

/// The functions and operators people expect, booleans, & and typed TRUE.
#[test]
fn more_functions() {
    let entries = [
        ("4", "4"),
        ("x", "x"),
        ("TRUE", "TRUE"),
        ("=COUNTA(A1:A3)", "3"),
        ("=COUNT(A1:A3)", "1"),
        ("=AND(A1>3,A3)", "TRUE"),
        ("=OR(A1>5,FALSE)", "FALSE"),
        ("=NOT(A1=4)", "FALSE"),
        ("=INT(-2.5)", "-3"),
        ("=INT(7.9)", "7"),
        ("=\"n=\"&A1&A2", "n=4x"),
        ("=A1/8&\"!\"", "0.5!"),
        ("=IF(A3,\"yes\",\"no\")", "yes"),
        ("=A3+1", "2"),
        ("=SUM(A1:A3)", "4"),
        ("=12.5%*8", "1"),
    ];
    let mut k = Keys::new().new_sheet();
    for (t, _) in &entries {
        k = k.enter(t);
    }
    let k = k.goto("A1");
    let mut run = Run::new(&k);
    run.finish(&k);
    let g = run.grid();
    for (i, (t, want)) in entries.iter().enumerate() {
        let name = format!("A{}", i + 1);
        assert_eq!(g.get(&name).map(String::as_str).unwrap_or(""), *want, "{name} = {t}: {g:?}");
    }
}

/// A paste as a window delivers it (SPEC §events: the paste key down, the
/// text, the key up): with cells copied in the sheet, Ctrl+V pastes them
/// (the text is not typed); after Esc it pastes the text, a tab starting
/// the next cell; while an entry is typed, the text goes into it.
#[test]
fn pasting_cells_and_text() {
    let base = Keys::new().new_sheet().enter("5").enter("=A1*2").goto("A1").press("down shift").press("c ctrl");
    // Ctrl+C also offers the clipboard the cells as text (SPEC §system
    // `copy`): values as shown, tabs between cells, a line per row.
    let mut run = Run::new(&base);
    run.finish(&base);
    assert_eq!(run.m.take_copied().as_deref(), Some(&b"5\n10\n"[..]));
    let k = Keys::new().new_sheet().enter("x").enter("1.5").goto("A1").press("down shift").press("right shift").press("1 ctrl shift").wait(3).press("c ctrl");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.m.take_copied().as_deref(), Some(&b"x\t\n1.50\t\n"[..]));
    // The sheet's own cells: references adjusted; the clipboard's text ignored.
    let k = Keys { lines: base.lines.clone(), f: base.f }.goto("C3").at("down v ctrl").text("stale\ttext").at("up v ctrl").wait(4).goto("C4");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "C4   =C3*2   = 10");
    assert_eq!(run.cell("D3"), "");
    // Esc forgets the copy: the text from another program goes into cells.
    let k = Keys { lines: base.lines.clone(), f: base.f }.press("esc").goto("C3").at("down v ctrl").text("7\t8\tPears").at("up v ctrl").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("C3").as_str(), run.cell("D3").as_str(), run.cell("E3").as_str()), ("7", "8", "Pears"));
    assert!(run.status().starts_with("Pasted"), "{}", run.status());
    assert!(run.entry().starts_with("C3:E3 "), "{}", run.entry());
    // Undone as one change.
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("z ctrl").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("C3").as_str(), run.cell("E3").as_str()), ("", ""));
    // Into an entry being typed: the text joins it, a tab as a space.
    let k = Keys::new().new_sheet().text("=1+").at("down v ctrl").text("2\t").at("up v ctrl").press("enter").press("up");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "A1   =1+2    = 3");
    // Text typed while Control is held (a window's text for Ctrl+Shift+1)
    // is not an entry.
    let k = Keys::new().new_sheet().at("down lctrl").text("!").at("up lctrl").wait(2);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "A1   ");
}

/// Ctrl+Plus and Ctrl+Minus insert and delete the selected rows (or the
/// columns, when whole columns are selected): every formula's references
/// move as in other spreadsheets, a reference to a deleted cell becomes
/// #REF!, ranges grow and shrink, the chart follows; undone at once.
#[test]
fn inserting_and_deleting_rows_and_columns() {
    // The example: insert a row above row 5 (select row 5 by its number).
    let k = Keys::new().wait(4).goto("A5").press("0x2e ctrl shift").wait(40).goto("D9");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "D9   =SUM(D3:D8)   = 1,518.35", "the subtotal's range grew");
    assert_eq!(run.cell("A6"), "Concrete");
    assert_eq!(run.cell("A5"), "");
    assert!(run.status().contains("Bar chart") || run.figures() == vec![(640, 72)]);
    let k = Keys { lines: k.lines.clone(), f: k.f }.goto("D6");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.entry(), "D6   =B6*C6   = 80.60", "moved rows keep their own references");
    // Delete rows 3 and 4: the sum shrinks; a formula on a deleted row is #REF!.
    let k = Keys::new().wait(4).goto("A11").enter("=D3+1").goto("A3").press("down shift").press("0x2d ctrl").wait(40);
    let mut run = Run::new(&k);
    run.finish(&k);
    let k2 = Keys { lines: k.lines.clone(), f: k.f }.goto("D6");
    let mut run = Run::new(&k2);
    run.finish(&k2);
    assert_eq!(run.entry(), "D6   =SUM(D3:D5)   = 728.60");
    let k2 = Keys { lines: k.lines.clone(), f: k.f }.goto("A9");
    let mut run = Run::new(&k2);
    run.finish(&k2);
    assert!(run.entry().starts_with("A9   =#REF!+1"), "{}", run.entry());
    // Undo puts the rows back.
    let k2 = Keys { lines: k.lines.clone(), f: k.f }.press("z ctrl").wait(40).goto("D8");
    let mut run = Run::new(&k2);
    run.finish(&k2);
    assert_eq!(run.entry(), "D8   =SUM(D3:D7)   = 1,518.35");
    // Columns: select column B by its letter (a click), insert one.
    let mut first = Run::new(&Keys::new());
    first.frames(8);
    let l = first.layout();
    let head = l.lines.iter().find(|l| l.spans.iter().any(|s| s.style.anchor == 2000)).unwrap().y;
    let k = Keys::new()
        .wait(4)
        .at(&format!("pointer {} {} 1", 2 * COLW + 10, head + 2))
        .at(&format!("pointer {} {} 0", 2 * COLW + 10, head + 2))
        .press("0x2e ctrl shift")
        .wait(40);
    let mut run = Run::new(&k);
    run.finish(&k);
    let k2 = Keys { lines: k.lines.clone(), f: k.f }.goto("E3");
    let mut run = Run::new(&k2);
    run.finish(&k2);
    assert_eq!(run.entry(), "E3   =C3*D3   = 546", "B and C moved right");
    assert_eq!(run.cell("B3"), "");
    // Rows pushed off the sheet must be empty.
    let k = Keys::new().new_sheet().goto("A1").enter("x").goto("A99").enter("y").goto("A1").press("0x2e ctrl shift").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert!(run.status().starts_with("No room"), "{}", run.status());
}

/// Ctrl+Shift+A and Ctrl+Shift+D sort by the active cell's column: numbers
/// before text, empty cells last, stable; a header row of text over numbers
/// stays; rows move with their formulas; undone at once.
#[test]
fn sorting() {
    // The example's items with their header (A2:D7, the active cell in
    // column D): by amount, the header row kept.
    let select = |k: Keys| k.goto("D2").press("left shift").press("left shift").press("left shift").press("down shift").press("down shift").press("down shift").press("down shift").press("down shift");
    let k = select(Keys::new().wait(4)).press("a ctrl shift").wait(30);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert!(run.status().contains("kept as the header"), "{}", run.status());
    let g = run.grid();
    let items: Vec<&str> = (3..=7).map(|r| g.get(&format!("A{r}")).map(String::as_str).unwrap_or("")).collect();
    assert_eq!(items, ["Delivery", "Concrete", "Posts", "Panels", "Labour (h)"]);
    assert_eq!(g["A2"], "Item");
    let k2 = Keys { lines: k.lines.clone(), f: k.f }.goto("D3");
    let mut run = Run::new(&k2);
    run.finish(&k2);
    assert_eq!(run.entry(), "D3   =B3*C3   = 40", "the formula moved with its row");
    // Descending; then undo.
    let k = select(Keys::new().wait(4)).press("d ctrl shift").wait(30);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.cell("A3"), "Labour (h)");
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("z ctrl").wait(30);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.cell("A3"), "Panels");
    // Mixed values in a selected range: numbers, then text, then empty;
    // equal keys keep their order.
    let mut k = Keys::new().new_sheet();
    for (a, b) in [("10", "1"), ("pear", "2"), ("", "3"), ("Apple", "4"), ("2", "5"), ("apple", "6")] {
        k = k.text(a).press("tab").enter(b).press("left");
    }
    let k = k.goto("A1").press("down shift").press("down shift").press("down shift").press("down shift").press("down shift").press("right shift");
    let k = k.press("a ctrl shift").wait(30);
    let mut run = Run::new(&k);
    run.finish(&k);
    let g = run.grid();
    let col: Vec<String> = (1..=6).map(|r| format!("{}{}", g.get(&format!("A{r}")).cloned().unwrap_or_default(), g.get(&format!("B{r}")).cloned().unwrap_or_default())).collect();
    assert_eq!(col, ["25", "101", "Apple4", "apple6", "pear2", "3"]);
}

/// Number formats: Ctrl+Shift+1 two decimals with separators, Ctrl+period
/// and Ctrl+comma one decimal more or fewer, Ctrl+Shift+5 percent,
/// Ctrl+Shift+~ general; formats copy with cells, survive undo and sort,
/// and are kept in the saved sheet (which a sheet without formats still
/// loads).
#[test]
fn number_formats() {
    let base = Keys::new().new_sheet().enter("1234.5").enter("0.125").enter("=A1*2").goto("A1").press("down shift").press("down shift");
    let k = Keys { lines: base.lines.clone(), f: base.f }.press("1 ctrl shift").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("A1").as_str(), run.cell("A2").as_str(), run.cell("A3").as_str()), ("1,234.50", "0.13", "2,469.00"));
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("0x36 ctrl").press("0x36 ctrl").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!((run.cell("A1").as_str(), run.cell("A2").as_str()), ("1,235", "0"));
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("0x37 ctrl").press("0x37 ctrl").press("0x37 ctrl").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.cell("A2"), "0.125");
    let k = Keys { lines: base.lines.clone(), f: base.f }.press("5 ctrl shift").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.cell("A2"), "13%");
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("z ctrl").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.cell("A2"), "0.125", "undone: as typed");
    // Copied with the cell; a clear keeps it; saved and loaded.
    let k = Keys { lines: base.lines.clone(), f: base.f }
        .press("1 ctrl shift")
        .wait(4)
        .goto("A1")
        .press("c ctrl")
        .goto("C1")
        .press("v ctrl")
        .wait(3)
        .press("esc")
        .goto("A2")
        .press("delete")
        .wait(3)
        .enter("7")
        .press("s ctrl")
        .wait(6);
    let mut run = Run::with(&k.lines, Consent::Deny, None, None, |_| {});
    run.finish(&k);
    assert_eq!((run.cell("C1").as_str(), run.cell("A2").as_str()), ("1,234.50", "7.00"));
    let saved = String::from_utf8(record(&run.data(), "sheet").unwrap()).unwrap();
    assert!(saved.ends_with("\x0Cformat A1 2s\n\x0Cformat C1 2s\n\x0Cformat A2 2s\n\x0Cformat A3 2s\n"), "{saved:?}");
    let data = run.data();
    let mut run2 = Run::with("", Consent::Deny, Some(&data), None, |_| {});
    run2.frames(8);
    assert_eq!((run2.cell("A1").as_str(), run2.cell("C1").as_str(), run2.cell("A3").as_str()), ("1,234.50", "1,234.50", "2,469.00"));
    // Ctrl+Shift+~ back to general.
    let k = Keys { lines: base.lines.clone(), f: base.f }.press("1 ctrl shift").wait(4).press("0x35 ctrl shift").wait(4);
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.cell("A1"), "1,234.50");
    assert_eq!(run.cell("A2"), "0.125");
}

/// Ctrl+arrows go to the edges of the filled cells, Ctrl+A selects the
/// sheet, Ctrl+End the last cell used.
#[test]
fn moving_by_blocks() {
    let k = Keys::new().new_sheet().enter("1").enter("2").enter("3").goto("A1").press("down ctrl");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert!(run.entry().starts_with("A3 "), "{}", run.entry());
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("down ctrl");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert!(run.entry().starts_with("A99 "), "{}", run.entry());
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("up ctrl").press("up ctrl").press("down ctrl shift");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert!(run.entry().starts_with("A1:A3 "), "{}", run.entry());
    let k = Keys { lines: k.lines.clone(), f: k.f }.press("a ctrl");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert!(run.entry().starts_with("A1:Z99 "), "{}", run.entry());
    let k = Keys { lines: k.lines.clone(), f: k.f }.goto("C7").enter("x").press("home ctrl").press("end ctrl");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert!(run.entry().starts_with("C7 "), "{}", run.entry());
}

/// The selection: keys and the pointer, the range, the box on the vector
/// display at the column and row edges the page laid out.
#[test]
fn selecting_with_keys_and_the_pointer() {
    let k = Keys::new().goto("B3").press("down shift").press("right shift");
    let mut run = Run::new(&k);
    run.finish(&k);
    // The active cell stays where the range began, as in other
    // spreadsheets: typing goes there.
    assert_eq!(run.entry(), "B3:C4   12");
    // The box is around B3, between the column lines at 2 × 80 and 3 × 80
    // and on the row's edges from locate.
    let l = run.layout();
    let row3 = l.lines.iter().find(|l| l.spans.iter().any(|s| s.style.anchor == 1002)).unwrap();
    let row4 = l.lines.iter().find(|l| l.spans.iter().any(|s| s.style.anchor == 1003)).unwrap();
    let (x0, y0, x1, y1) = run.sel_box().expect("a selection box");
    assert_eq!((x0, x1), (2 * COLW as i16, 3 * COLW as i16), "B is the 3rd column of the page");
    assert_eq!((y0 as u32, y1 as u32), (row3.y - 3, row4.y - 3));
    // A click on D6 selects it; a drag to E7 selects D6:E7.
    let row6 = l.lines.iter().find(|l| l.spans.iter().any(|s| s.style.anchor == 1005)).unwrap().y;
    let row7 = l.lines.iter().find(|l| l.spans.iter().any(|s| s.style.anchor == 1006)).unwrap().y;
    let k2 = Keys { lines: k.lines.clone(), f: k.f }
        .at(&format!("pointer {} {} 1", 4 * COLW + 10, row6 + 2))
        .at(&format!("pointer {} {} 1", 5 * COLW + 10, row7 + 2))
        .at(&format!("pointer {} {} 0", 5 * COLW + 10, row7 + 2));
    let mut run = Run::new(&k2);
    run.finish(&k2);
    // (the active cell D6, where the drag began, holds a formula)
    assert_eq!(run.entry(), "D6:E7   =B6*C6   = 608");
    // The selection's row numbers and column letters are in its colour.
    let l = run.layout();
    let accent = [26, 100, 210];
    let coloured: Vec<String> =
        l.lines.iter().flat_map(|l| &l.spans).filter(|s| s.style.rgb == accent && s.style.size == 12).map(|s| s.text.clone()).collect();
    assert_eq!(coloured, vec!["D", "E", "6", "7"]);
}

#[test]
fn copy_paste_and_fill() {
    let k = Keys::new()
        .new_sheet()
        .enter("1")
        .enter("2")
        .enter("3")
        .goto("B1")
        .enter("=A1*10")
        .goto("B1")
        .press("down shift")
        .press("down shift")
        .press("d ctrl")
        .goto("B3")
        .press("c ctrl")
        .goto("D5")
        .press("v ctrl");
    let mut run = Run::new(&k);
    run.finish(&k);
    let g = run.grid();
    assert_eq!((g["B1"].as_str(), g["B2"].as_str(), g["B3"].as_str()), ("10", "20", "30"));
    assert_eq!(run.entry(), "D5   =C5*10   = 0");
    // Fill right, and a copy that moves a reference off the sheet.
    let k2 = Keys { lines: k.lines.clone(), f: k.f }
        .goto("A1")
        .press("right shift")
        .press("right shift")
        .press("r ctrl")
        .goto("B2")
        .press("c ctrl")
        .goto("A9")
        .press("v ctrl");
    let mut run = Run::new(&k2);
    run.finish(&k2);
    let g = run.grid();
    assert_eq!((g["B1"].as_str(), g["C1"].as_str()), ("1", "1"));
    assert_eq!(g["A9"], "#REF!");
    assert_eq!(run.entry(), "A9   =#REF!*10   #REF! a reference outside the sheet (A1 to Z99)");
}

#[test]
fn charts() {
    // The example's bar chart; Ctrl+G on its range removes it; Ctrl+L
    // shows a line chart of another range.
    let k = Keys::new().goto("D3").press("down shift").press("down shift").press("down shift").press("down shift").press("g ctrl");
    let mut run = Run::new(&k);
    run.finish(&k);
    assert_eq!(run.figures(), vec![]);
    assert!(run.status().starts_with("Chart removed"), "{}", run.status());
    let k2 = Keys { lines: k.lines.clone(), f: k.f }.goto("B3").press("down shift").press("down shift").press("l ctrl");
    let mut run = Run::new(&k2);
    run.finish(&k2);
    assert_eq!(run.figures(), vec![(640, 72)]);
    assert!(run.status().contains("Line chart of B3:B5"), "{}", run.status());
    // A single cell is not a range.
    let k3 = Keys::new().new_sheet().press("g ctrl");
    let mut run = Run::new(&k3);
    run.finish(&k3);
    assert!(run.status().starts_with("Select the values first"), "{}", run.status());
    // Charts of more than 8 values have no label table.
    let mut k4 = Keys::new().new_sheet();
    for i in 0..12 {
        k4 = k4.enter(&format!("{}", i * 3));
    }
    let k4 = k4.goto("A1");
    let mut k4 = k4;
    for _ in 0..11 {
        k4 = k4.press("down shift");
    }
    let k4 = k4.press("g ctrl");
    let mut run = Run::new(&k4);
    run.finish(&k4);
    assert_eq!(run.figures(), vec![(640, 72)]);
    assert!(run.status().contains("(labels for at most 8)"), "{}", run.status());
}

#[test]
fn saves_and_reopens() {
    let k = Keys::new()
        .new_sheet()
        .enter("Rent")
        .enter("Food")
        .goto("B1")
        .enter("1200")
        .enter("350.5")
        .enter("=SUM(B1:B2)")
        .goto("B1")
        .press("down shift")
        .press("g ctrl")
        .press("s ctrl")
        .wait(4);
    let mut run = Run::with(&k.lines, Consent::Deny, None, None, |_| {});
    run.finish(&k);
    assert!(run.status().starts_with("Saved"), "{}", run.status());
    let want = "Rent\t1200\nFood\t350.5\n\t=SUM(B1:B2)\n\x0Cchart bar B1:B2\n";
    assert_eq!(String::from_utf8(record(&run.data(), "sheet").unwrap()).unwrap(), want);
    // Started again with the same records: the sheet is back.
    let data = run.data();
    let mut run2 = Run::with("", Consent::Deny, Some(&data), None, |_| {});
    run2.frames(8);
    let g = run2.grid();
    assert_eq!((g["A1"].as_str(), g["B2"].as_str(), g["B3"].as_str()), ("Rent", "350.50", "1,550.50"));
    assert_eq!(run2.figures(), vec![(640, 72)]);
}

#[test]
fn publishes_and_opens_a_document() {
    let k = Keys::new().wait(4).press("p ctrl").wait(6);
    let mut run = Run::with(&k.lines, Consent::Allow, None, None, |_| {});
    run.finish(&k);
    assert!(run.status().starts_with("Published as \"Estimate\""), "{}", run.status());
    let store = run.dir.path().join("store");
    let entries = FeedStore::beside_blobs(&store).entries(&identity().feed_key()).unwrap();
    assert_eq!(entries.len(), 1);
    let mut want = header();
    want.extend(ESTIMATE.as_bytes());
    let e = &entries[0].entry;
    assert_eq!((e.kind, e.title.as_str()), (4, "Estimate"));
    let blob = BlobStore::open(&store, None).unwrap().read(&e.blob).unwrap().unwrap();
    assert_eq!(String::from_utf8_lossy(&blob), String::from_utf8_lossy(&want));

    // Refused: nothing is published.
    let mut run = Run::with(&k.lines, Consent::Deny, None, None, |_| {});
    run.finish(&k);
    assert!(run.status().starts_with("Not published"), "{}", run.status());

    // Opened as the argument: someone else's sheet, shown and editable.
    let mut doc = header();
    doc.extend(b"Total\t=2*21\n");
    let mut id = [0; 32];
    let mut run = Run::with("", Consent::Deny, None, None, |store| {
        id = BlobStore::open(store, None).unwrap().put(&doc).unwrap();
    });
    run.m.set_argument(Some(id));
    run.frames(8);
    assert_eq!(run.cell("B1"), "42", "{}", run.status());
    assert!(run.status().starts_with("A published sheet"), "{}", run.status());

    // Not a sheet document: said, and the saved sheet (here: the example).
    let mut run = Run::with("", Consent::Deny, None, None, |store| {
        id = BlobStore::open(store, None).unwrap().put(b"TDOC\0not a sheet").unwrap();
    });
    run.m.set_argument(Some(id));
    run.frames(8);
    assert!(run.status().starts_with("Not a sheet document"), "{}", run.status());
}

/// Without the 2D display the selected cell is shown in its colour (an
/// empty one by a dot); without content nothing is kept.
#[test]
fn without_the_optional_devices() {
    let k = Keys::new().goto("E2").press("s ctrl");
    let mut run = Run::with(&k.lines, Consent::Deny, None, Some(Grants(vec![true, false, false])), |_| {});
    run.finish(&k);
    assert!(run.status().starts_with("No saving on this host"), "{}", run.status());
    assert_eq!(run.cell("E2"), "·");
    assert!(run.m.vector().is_none());
    let k = Keys::new().goto("D3");
    let mut run = Run::with(&k.lines, Consent::Deny, None, Some(Grants(vec![true, false, true])), |_| {});
    run.finish(&k);
    let l = run.layout();
    let sel = l.lines.iter().flat_map(|l| &l.spans).find(|s| s.text == "546").unwrap();
    assert!(sel.style.bold && sel.style.rgb == [26, 100, 210], "{sel:?}");
}

/// The saved form as typed: a whole sheet of formulas, each one the next
/// cell plus one, so the chain runs through all 2,574 cells; recalculated
/// over many frames, none of them long.
#[test]
fn a_full_sheet_is_recalculated_across_frames() {
    let mut tsv = String::new();
    for r in 0..99u32 {
        let cells: Vec<String> = (0..26u32)
            .map(|c| {
                let i = r * 26 + c;
                if i == 99 * 26 - 1 {
                    "1".to_string()
                } else {
                    let n = i + 1;
                    format!("={}{}+1", (b'A' + (n % 26) as u8) as char, n / 26 + 1)
                }
            })
            .collect();
        tsv += &cells.join("\t");
        tsv.push('\n');
    }
    let data = tempfile::tempdir().unwrap();
    Records::in_data_dir(data.path()).save(&owner(), b"sheet", tsv.as_bytes()).unwrap();
    let mut run = Run::with("", Consent::Deny, Some(data.path()), None, |_| {});
    let mut frames = 0;
    while frames < 600 {
        run.frames(1);
        frames += 1;
        if run.cell("A1") == "2,574" {
            break;
        }
    }
    assert_eq!(run.cell("A1"), "2,574", "after {frames} frames");
    assert_eq!(run.cell("B1"), "2,573");
    eprintln!("sheet: a chain of 2,574 formulas loaded and recalculated in {frames} frames");
    report("a full sheet", &run.used);

    // A sheet of sums over ranges: every cell of rows 2-99 sums the 26
    // cells of the row above.
    let mut tsv = (1..=26).map(|i| i.to_string()).collect::<Vec<_>>().join("\t") + "\n";
    for r in 1..99 {
        tsv += &vec![format!("=SUM(A{r}:Z{r})"); 26].join("\t");
        tsv.push('\n');
    }
    let data = tempfile::tempdir().unwrap();
    Records::in_data_dir(data.path()).save(&owner(), b"sheet", tsv.as_bytes()).unwrap();
    let mut run = Run::with("", Consent::Deny, Some(data.path()), None, |_| {});
    let mut frames = 0;
    while frames < 900 && !(run.cell("A3") == "9,126" && !run.status().contains("Calculating")) {
        run.frames(1);
        frames += 1;
    }
    assert_eq!(run.cell("A2"), "351");
    assert_eq!(run.cell("A3"), "9,126");
    eprintln!("sheet: 2,548 sums of 26 cells loaded and recalculated in {frames} frames");
    report("a sheet of sums", &run.used);
}

/// A long session of typing, moving, filling and charting: no frame
/// comes near the budget.
#[test]
fn a_long_session_stays_within_the_budget() {
    let mut k = Keys::new().new_sheet();
    for r in 0..40 {
        k = k.enter(&format!("Item {r}")).press("up").press("right").enter(&format!("{}", r * 7 + 3)).press("up").press("right");
        k = k.enter(&format!("=B{}*1.0825", r + 1)).press("left").press("left");
    }
    k = k.goto("C1");
    for _ in 0..39 {
        k = k.press("down shift");
    }
    k = k.press("g ctrl").press("d ctrl").goto("D1").enter("=SUM(C1:C40)").press("pagedown").press("pagedown").press("s ctrl").wait(6);
    let mut run = Run::new(&k);
    run.finish(&k);
    let busiest = report("a long session", &run.used);
    assert!(busiest < 100_000, "{busiest}");
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(IMAGE, &["sheet/sheet.c"], true, "sheet", RAM_PAGES, &[(7, 0), (2, 0x80), (3, 0x80), (4, 0x80)]);
}
