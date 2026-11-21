//! programs/sketch/sketch.terse: drawing with the pointer (strokes in the
//! display list and in the record "drawing"), undo and clear, the display
//! style, saving and loading, the limits, publishing a document, showing a
//! document given as the argument, the frame budget, and the image is up to
//! date. Set UPDATE_GOLDEN=1 to rewrite programs/sketch/expected/.

mod common;

use std::path::{Path, PathBuf};

use terse_core::vector::{Primitive, decode};
use terse_core::{Image, Machine, Script};
use terse_net::service::Headless;
use terse_net::{BlobStore, Consent, ContentService, FeedStore, Identity, Node, Owner, Records};

const IMAGE: &[u8] = include_bytes!("../../../programs/sketch/sketch.terse");
const RAM_PAGES: u8 = 4;

/// The document header of a sketch (SPEC §content.documents): "TDOC", version 0, a zero
/// owner key (a built-in program's format), the name "sketch".
fn header() -> Vec<u8> {
    let mut h = b"TDOC\0".to_vec();
    h.extend([0; 32]);
    h.push(6);
    h.extend(b"sketch");
    h
}

/// A stroke in the drawing data format of sketch.c.
fn stroke(colour: u8, points: &[(u16, u16)]) -> Vec<u8> {
    let mut v = vec![colour];
    v.extend((points.len() as u16).to_le_bytes());
    for &(x, y) in points {
        v.extend(x.to_le_bytes());
        v.extend(y.to_le_bytes());
    }
    v
}

fn owner() -> Owner {
    Owner::Image(Image::parse(IMAGE).unwrap().id)
}

struct Run {
    m: Machine,
    h: Headless,
    dir: tempfile::TempDir,
    used: Vec<u64>,
}

impl Run {
    fn new(script: &str) -> Self {
        Run::with(script, Consent::Deny, None, |_| {})
    }

    /// A run on a new store and data directory (or `data`), with `setup`
    /// called on the store before the content service starts.
    fn with(script: &str, consent: Consent, data: Option<&Path>, setup: impl FnOnce(&Path)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        setup(&store);
        let mut node = Node::new(BlobStore::open(&store, None).unwrap(), None);
        node.set_identity(identity());
        node.set_records(Records::in_data_dir(data.unwrap_or(&dir.path().join("data"))));
        let content = ContentService::start_with_consent(node, consent);
        let h = Headless::new(Some(content), Script::parse(script).unwrap());
        Run { m: Machine::new(&Image::parse(IMAGE).unwrap()), h, dir, used: Vec::new() }
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            let r = self.h.frame(&mut self.m);
            assert_eq!(r.stopped, None, "stopped at frame {}", self.m.frame());
            assert!(r.waited, "frame {} did not end in wait_frame ({} units)", self.m.frame(), r.used);
            self.used.push(r.used);
        }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn status(&self) -> String {
        let text = self.m.terminal().unwrap().text();
        text.lines().nth(29).unwrap_or("").get(..28).unwrap_or("").trim().to_string()
    }

    fn shapes(&self) -> Vec<Primitive> {
        decode(self.m.vector_list()).collect()
    }

    /// The strokes' lines (without the colour disc and its ring).
    fn lines(&self) -> Vec<Primitive> {
        let s = self.shapes();
        assert!(matches!(s[s.len() - 2], Primitive::Disc { .. }) && matches!(s[s.len() - 1], Primitive::Circle { .. }));
        s[..s.len() - 2].to_vec()
    }

    fn screen(&self) -> String {
        let v = self.m.vector().unwrap();
        let style = v.style_line().map_or(String::new(), |l| l + "\n");
        format!("{}{style}-- vectors: {} list bytes --\n{}", self.m.terminal().unwrap().text(), v.list().len(), v.dump())
    }
}

fn identity() -> Identity {
    Identity::from_seed([42; 32])
}

fn record(data: &Path, name: &str) -> Option<Vec<u8>> {
    Records::in_data_dir(data).load(&owner(), name.as_bytes()).unwrap()
}

fn line(x0: i16, y0: i16, x1: i16, y1: i16, rgba: [u8; 4]) -> Primitive {
    Primitive::Line { x0, y0, x1, y1, width: 32, rgba }
}

const WHITE: [u8; 4] = [240, 240, 240, 255];
const RED: [u8; 4] = [255, 70, 60, 255];

/// Two strokes: a white one of three points (one point of the drag is too
/// close and skipped; the release adds the last), a red dot; then S.
const TWO_STROKES: &str = "\
3 pointer 100 100 1
4 pointer 110 100 1
5 pointer 111 100 1
6 pointer 120 105 0
8 press 2
9 pointer 200 200 1
10 pointer 200 200 0
12 press s
";

fn two_strokes_data() -> Vec<u8> {
    let mut d = stroke(0, &[(100, 100), (110, 100), (120, 105)]);
    d.extend(stroke(1, &[(200, 200)]));
    d
}

#[test]
fn draws_and_saves() {
    let mut run = Run::new(TWO_STROKES);
    run.frames(2);
    assert_eq!(run.lines(), [], "an empty drawing");
    assert_eq!(run.status(), "");
    run.frames(14);
    assert_eq!(
        run.lines(),
        [
            line(1600, 1600, 1600, 1600, WHITE),
            line(1600, 1600, 1760, 1600, WHITE),
            line(1760, 1600, 1920, 1680, WHITE),
            line(3200, 3200, 3200, 3200, RED),
        ]
    );
    // The colour disc shows red.
    assert!(matches!(run.shapes()[4], Primitive::Disc { rgba: RED, radius: 80, .. }));
    assert_eq!(run.status(), "Saved");
    assert_eq!(record(&run.data(), "drawing").unwrap(), two_strokes_data());
    common::golden("sketch/expected/two-strokes.txt", &run.screen());

    // A new run with the same records shows it.
    let mut again = Run::with("", Consent::Deny, Some(&run.data()), |_| {});
    again.frames(3);
    assert_eq!(again.lines(), run.lines());
    assert!(matches!(again.shapes()[4], Primitive::Disc { rgba: WHITE, .. }));
}

#[test]
fn input_waits_for_the_drawing() {
    // A press in frame 0 (before the record arrives) draws nothing, nor
    // does dragging on with the button still held.
    let mut run = Run::new("0 pointer 50 50 1\n1 pointer 60 60 1\n2 pointer 70 70 1\n3 pointer 80 80 0\n");
    run.frames(5);
    assert_eq!(run.lines(), []);
}

#[test]
fn undo_and_clear() {
    let script = format!("{TWO_STROKES}14 press u\n16 press u\n18 press u\n20 press 3\n21 click 300 300\n23 press c\n25 press s\n");
    let mut run = Run::new(&script);
    run.frames(15);
    assert_eq!(run.lines().len(), 3, "the dot undone");
    assert_eq!(run.status(), "Undone");
    run.frames(2);
    assert_eq!(run.lines(), []);
    run.frames(2);
    assert_eq!(run.lines(), [], "nothing more to undo");
    run.frames(3);
    assert_eq!(run.lines(), [line(4800, 4800, 4800, 4800, [255, 210, 60, 255])]);
    run.frames(2);
    assert_eq!(run.lines(), []);
    assert_eq!(run.status(), "Cleared");
    run.frames(3);
    assert_eq!(run.status(), "Saved");
    assert_eq!(record(&run.data(), "drawing").unwrap(), b"");
}

#[test]
fn phosphor_toggle() {
    // F works from frame 0, before the drawing has arrived.
    let mut run = Run::new(&format!("0 press f\n{TWO_STROKES}20 press f\n"));
    run.frames(1);
    assert!(run.m.vector().unwrap().phosphor());
    run.frames(19);
    assert!(run.m.vector().unwrap().phosphor());
    assert_eq!(run.lines().len(), 4);
    let list = run.m.vector_list().to_vec();
    run.frames(1);
    assert!(!run.m.vector().unwrap().phosphor());
    assert_eq!(run.m.vector_list(), list, "the style changes nothing else");
}

#[test]
fn publishes_a_document() {
    let script = format!("{TWO_STROKES}14 press p\n20 press p\n");
    let mut run = Run::with(&script, Consent::Allow, None, |_| {});
    run.frames(19);
    assert_eq!(run.status(), "Published as Sketch 1");
    run.frames(6);
    assert_eq!(run.status(), "Published as Sketch 2");
    assert_eq!(record(&run.data(), "published").unwrap(), 2u32.to_le_bytes());
    let store = run.dir.path().join("store");
    let entries = FeedStore::beside_blobs(&store).entries(&identity().feed_key()).unwrap();
    assert_eq!(entries.len(), 2);
    let mut want = header();
    want.extend(two_strokes_data());
    for (i, e) in entries.iter().enumerate() {
        assert_eq!(e.entry.kind, 4, "a document entry");
        assert_eq!(e.entry.title, format!("Sketch {}", i + 1));
        assert_eq!(e.entry.size as usize, want.len());
        assert_eq!(BlobStore::open(&store, None).unwrap().read(&e.entry.blob).unwrap().unwrap(), want);
    }

    // Refused: nothing published, the count stays.
    let mut run = Run::with(&script, Consent::Deny, None, |_| {});
    run.frames(19);
    assert_eq!(run.status(), "Not published");
    assert_eq!(record(&run.data(), "published"), None);
    // An empty drawing is not published.
    let mut run = Run::with("5 press p\n", Consent::Allow, None, |_| {});
    run.frames(7);
    assert_eq!(run.status(), "Nothing to publish");
}

/// A run started with `blob` as its argument (an ID not in the store if
/// `blob` is empty); the record "drawing" holds one green stroke.
fn with_argument(blob: &[u8], script: &str) -> Run {
    let mut id = [7; 32];
    let mut run = Run::with(script, Consent::Deny, None, |store| {
        let data = store.parent().unwrap().join("data");
        Records::in_data_dir(&data).save(&owner(), b"drawing", &stroke(3, &[(10, 10), (20, 20)])).unwrap();
        if !blob.is_empty() {
            id = BlobStore::open(store, None).unwrap().put(blob).unwrap();
        }
    });
    run.m.set_argument(Some(id));
    run
}

const GREEN: [u8; 4] = [80, 220, 100, 255];

#[test]
fn shows_a_document_read_only() {
    let mut doc = header();
    doc.extend(two_strokes_data());
    let mut run = with_argument(&doc, "4 press u\n5 press c\n6 press s\n10 pointer 300 200 1\n11 pointer 320 200 0\n13 press s\n");
    run.frames(3);
    assert_eq!(run.status(), "Viewing a published sketch");
    assert_eq!(run.lines().len(), 4);
    assert_eq!(run.lines()[3], line(3200, 3200, 3200, 3200, RED));
    run.frames(4);
    assert_eq!(run.lines().len(), 4, "U and C do nothing");
    assert_eq!(run.status(), "Draw to edit a copy");
    assert_eq!(record(&run.data(), "drawing").unwrap(), stroke(3, &[(10, 10), (20, 20)]), "S does nothing");
    run.frames(5);
    assert_eq!(run.status(), "Editing a copy; S saves it");
    assert_eq!(run.lines().len(), 6);
    run.frames(3);
    assert_eq!(run.status(), "Saved");
    let mut want = two_strokes_data();
    want.extend(stroke(0, &[(300, 200), (320, 200)]));
    assert_eq!(record(&run.data(), "drawing").unwrap(), want);
}

#[test]
fn a_bad_argument_falls_back_to_the_record() {
    let mut other = b"TDOC\0".to_vec();
    other.extend([0; 32]);
    other.push(5);
    other.extend(b"notes");
    other.extend(two_strokes_data());
    let mut bad_data = header();
    bad_data.extend(stroke(9, &[(1, 1)]));
    for (blob, says) in [
        (other, "Not a sketch document"),
        (b"plain text".to_vec(), "Not a sketch document"),
        (bad_data, "Not a sketch drawing"),
        (Vec::new(), ""),
    ] {
        let mut run = with_argument(&blob, "");
        run.frames(4);
        let status = run.status();
        if says.is_empty() {
            assert!(status == "Document not found" || status == "Could not get the document", "{status}");
        } else {
            assert_eq!(status, says);
        }
        assert_eq!(run.lines(), [line(160, 160, 160, 160, GREEN), line(160, 160, 320, 320, GREEN)], "{says}");
    }
}

/// One stroke dragged back and forth until the data is full (8,192 bytes):
/// 2,047 points. Then S, and a new run shows the same.
#[test]
fn fills_the_data() {
    let mut script = String::from("2 pointer 0 0 1\n");
    for i in 0..2100u32 {
        let f = 3 + i / 50;
        // Short steps, as a hand draws: 2,047 lines of 10 to 14 units stay
        // within the ink cap (SPEC §vector.ink-cap), so all are drawn.
        let (x, y) = (i % 2 * 10 + 10, 10 + (i * 7) % 5 + i / 300 * 20);
        script += &format!("{f} pointer {x} {y} 1\n");
    }
    script += "50 pointer 0 0 0\n52 press s\n";
    let mut run = Run::new(&script);
    run.frames(55);
    assert_eq!(run.status(), "Saved");
    let saved = record(&run.data(), "drawing").unwrap();
    assert_eq!(saved.len(), 3 + 2047 * 4);
    assert_eq!(run.lines().len(), 2047);
    run.frames(1);
    let used = run.used.clone();
    // Loading it takes a few frames (1,024 points a frame).
    let mut again = Run::with("", Consent::Deny, Some(&run.data()), |_| {});
    again.frames(3);
    assert!(again.lines().len() < 2047);
    again.frames(2);
    assert_eq!(again.lines(), run.lines());
    report("fills the data (drawing)", &used);
    report("fills the data (loading)", &again.used);
}

/// Single clicks until the display list is full: 1,090 dots (15 bytes of
/// list each), 7,630 bytes of data.
#[test]
fn fills_the_list() {
    let mut script = String::new();
    for i in 0..1100u32 {
        script += &format!("{} click {} {}\n", 2 + i / 25, 10 + i % 60 * 10, 10 + i / 60 * 20);
    }
    script += "50 press s\n";
    let mut run = Run::new(&script);
    run.frames(48);
    assert_eq!(run.status(), "Full: undo or clear");
    assert_eq!(run.lines().len(), 1090);
    assert!(run.m.vector_list().len() <= 16384);
    run.frames(5);
    assert_eq!(record(&run.data(), "drawing").unwrap().len(), 1090 * 7);
    report("fills the list", &run.used);
    // Undo makes room again.
    let mut more = Run::new(&format!("{script}52 press u\n53 click 5 470\n"));
    more.frames(56);
    assert_eq!(more.lines().len(), 1090);
    assert_eq!(more.lines()[1089], line(80, 7520, 80, 7520, WHITE));
}

/// A record whose list would not fit: the strokes past the limit are
/// dropped when it is loaded.
#[test]
fn loading_caps_the_list() {
    let data = tempfile::tempdir().unwrap();
    let mut d = Vec::new();
    for i in 0..1170u16 {
        d.extend(stroke((i % 6) as u8, &[(i % 600, i / 3)]));
    }
    assert_eq!(d.len(), 8190);
    Records::in_data_dir(data.path()).save(&owner(), b"drawing", &d).unwrap();
    let mut run = Run::with("", Consent::Deny, Some(data.path()), |_| {});
    run.frames(6);
    assert_eq!(run.lines().len(), 1090);
    assert_eq!(run.status(), "");
    report("loading a full list", &run.used);
}

fn report(what: &str, used: &[u64]) {
    let stats = common::FrameStats::new(used);
    eprintln!("sketch {what}: {}", stats.describe());
    assert!(common::pct(stats.busiest) < 60.0, "{what}: busiest frame {} units", stats.busiest);
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(IMAGE, &["sketch/sketch.c"], true, "sketch", RAM_PAGES, &[(2, 0), (3, 0), (4, 0x80)]);
}

