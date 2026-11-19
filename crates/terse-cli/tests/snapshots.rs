//! Snapshots of the checked-in programs (STATE.md 4, SPEC 12.7): at several
//! points of scripted runs, serialize → restore → serialize is byte for
//! byte the same, and a restored machine given the inputs the original got
//! from then on has the original's hash after every frame (where no content
//! request was pending; a pending one fails after a restore). Then the
//! shell's sessions: saved, restored in a new run with the same data
//! directory, a missing image, discarded; and `terse run --save-state`.
//!
//! `sizes` prints each program's snapshot file size and the time to take
//! and restore one (run with `--nocapture`).

mod common;

use std::path::Path;
use std::time::Instant;

use terse_core::machines::{failure, state};
use terse_core::{Completion, Controller, Event, FrameInput, Image, Machine, PageInput, ProgramId, Script, Snapshot, System};
use terse_net::{BlobStore, Consent, ContentService, Draft, HeadlessSystem, Identity, Node, Records, Snapshots, Subscriptions, builtin};

const HELLO: &[u8] = include_bytes!("../../../programs/hello/hello.terse");
const READER: &[u8] = include_bytes!("../../../programs/reader/reader.terse");
const SPARKS: &[u8] = include_bytes!("../../../programs/sparks/sparks.terse");
const SKETCH: &[u8] = include_bytes!("../../../programs/sketch/sketch.terse");
const FETCH: &[u8] = include_bytes!("../../../programs/fetch/fetch.terse");
const CONFORM: &[u8] = include_bytes!("../../../programs/conform/conform.terse");
const SHELL: &[u8] = include_bytes!("../../../programs/shell/shell.terse");
const MESSAGE: &[u8] = include_bytes!("../../../programs/fetch/message.txt");
const PICTURES: &str = include_str!("../../terse-host/demo/pictures.txt");
const LONG: &str = include_str!("../../terse-host/demo/long.txt");

/// One frame's inputs, as the machine got them.
struct Input {
    events: Vec<Event>,
    completions: Vec<Completion>,
    controllers: [Controller; 4],
}

/// A machine run as `terse run` runs it (scripted events, page events from
/// the reference layout, every request completed at the next frame), with
/// each frame's inputs and hash kept, and snapshots at chosen frames.
struct Rec {
    image: Image,
    machine: Machine,
    content: Option<ContentService>,
    script: Script,
    page: PageInput,
    completions: Vec<Completion>,
    feeds_updated: bool,
    inputs: Vec<Input>,
    hashes: Vec<[u8; 32]>,
    /// (frames run, the snapshot, whether a content request was pending).
    snapshots: Vec<(usize, Snapshot, bool)>,
    _dirs: Vec<tempfile::TempDir>,
}

impl Rec {
    fn new(image: &[u8], script: &str, content: Option<ContentService>, dirs: Vec<tempfile::TempDir>) -> Self {
        let image = Image::parse(image).unwrap();
        Rec {
            machine: Machine::new(&image),
            image,
            content,
            script: Script::parse(script).unwrap(),
            page: PageInput::default(),
            completions: Vec::new(),
            feeds_updated: false,
            inputs: Vec::new(),
            hashes: Vec::new(),
            snapshots: Vec::new(),
            _dirs: dirs,
        }
    }

    fn frame(&mut self) {
        let f = self.machine.frame();
        let mut events = self.page.events(self.machine.page(), self.script.events(f));
        if std::mem::take(&mut self.feeds_updated) {
            events.push(Event::feeds_updated());
        }
        let completions = std::mem::take(&mut self.completions);
        let controllers = self.script.controllers(f);
        let r = self.machine.run_frame(&FrameInput { events: &events, completions: &completions, controllers, ..Default::default() });
        assert!(self.machine.fault().is_none(), "{:?}", self.machine.fault());
        assert_eq!(r.completions_ignored, 0);
        self.page.after_frame(self.machine.page());
        if let Some(content) = &mut self.content {
            content.submit_machine(0, &mut self.machine);
            self.completions = content.wait_finished();
            self.feeds_updated = content.feeds_updated();
        }
        self.inputs.push(Input { events, completions, controllers });
        self.hashes.push(self.machine.state_hash());
    }

    /// Runs `frames` frames, taking a snapshot (and checking its round
    /// trip) after each frame count in `at` (0: at reset).
    fn run(&mut self, frames: usize, at: &[usize]) {
        for n in 0..=frames {
            if at.contains(&n) {
                self.snapshot();
            }
            if n < frames {
                self.frame();
            }
        }
    }

    fn snapshot(&mut self) {
        let s = Snapshot::of(&self.machine);
        let file = s.encode();
        let back = Snapshot::decode(&file).unwrap();
        assert_eq!(back, s);
        let restored = back.restore(&self.image).unwrap();
        assert_eq!(restored.state(), self.machine.state(), "round trip after {} frames", self.inputs.len());
        let pending = self.machine.content().is_some_and(|c| !c.pending_handles().is_empty());
        self.snapshots.push((self.inputs.len(), s, pending));
    }

    /// Each snapshot without a pending request, restored and given the
    /// original's inputs from then on: the same hash after every frame.
    /// Returns how many were continued.
    fn continue_each(&self) -> usize {
        let mut continued = 0;
        for (k, s, pending) in &self.snapshots {
            if *pending {
                continue;
            }
            let mut m = s.restore(&self.image).unwrap();
            for (i, input) in self.inputs[*k..].iter().enumerate() {
                let frame = FrameInput { events: &input.events, completions: &input.completions, controllers: input.controllers, ..Default::default() };
                m.run_frame(&frame);
                assert_eq!(m.state_hash(), self.hashes[k + i], "restored after {k} frames, frame {}", k + i);
            }
            continued += 1;
        }
        continued
    }
}

fn service(store: &Path, data: &Path, setup: impl FnOnce(&mut Node)) -> ContentService {
    let mut node = Node::new(BlobStore::open(store, None).unwrap(), None);
    setup(&mut node);
    node.set_records(Records::in_data_dir(data));
    ContentService::start_with_consent(node, Consent::Deny)
}

fn alice() -> Identity {
    Identity::from_seed([11; 32])
}

/// A content service whose store holds alice's articles, `titles` and
/// `texts`, and which follows her.
fn with_articles(articles: &[(&str, &str)]) -> (ContentService, Vec<tempfile::TempDir>) {
    let (store, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let content = service(store.path(), data.path(), |node| {
        node.set_identity(alice());
        for (i, (title, text)) in articles.iter().enumerate() {
            let draft = Draft { data: text.as_bytes().to_vec(), title: title.to_string(), reply_to: None, time: 1_800_000_000 + i as u32 };
            assert!(matches!(node.publish_article(&draft), terse_net::Done::Published { .. }));
        }
        node.set_subscriptions(Subscriptions::in_memory([alice().feed_key()]));
    });
    (content, vec![store, data])
}

fn plain_service() -> (ContentService, Vec<tempfile::TempDir>) {
    let (store, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    (service(store.path(), data.path(), |_| {}), vec![store, data])
}

#[test]
fn hello() {
    let mut r = Rec::new(HELLO, "", None, Vec::new());
    r.run(40, &[0, 1, 10, 30]);
    assert_eq!(r.continue_each(), 4);
}

#[test]
fn reader_list_article_figures_and_scrolling() {
    let (content, dirs) = with_articles(&[("Pictures in articles", PICTURES), ("A long one", LONG)]);
    // The list, then the newest (the long article) and the pictures,
    // scrolled.
    let script = "20 press down\n24 press enter\n40 press pagedown\n44 press pagedown\n50 press down\n60 press esc\n66 press enter\n";
    let mut r = Rec::new(READER, script, Some(content), dirs);
    // 25: the article just submitted, its height not yet reported.
    r.run(90, &[1, 10, 20, 25, 26, 30, 41, 47, 61, 67, 68, 80]);
    let doc = r.machine.page().unwrap().document().unwrap();
    assert!(terse_core::page::parse(doc).iter().any(|p| p.figure.is_some()), "the article with figures is open");
    assert!(r.continue_each() >= 8, "most points have nothing pending");
}

#[test]
fn reader_scrolled_halfway() {
    let (content, dirs) = with_articles(&[("A long one", LONG)]);
    let mut r = Rec::new(READER, "12 press enter\n30 press pagedown\n31 press pagedown\n32 press down\n", Some(content), dirs);
    r.run(50, &[13, 14, 31, 40]);
    assert!(r.machine.page().unwrap().scroll() > 0);
    assert!(r.snapshots[0].2, "13: the article's get is pending");
    assert_eq!(r.continue_each(), 3);
}

#[test]
fn sparks_plain_and_phosphor() {
    let dir = common::programs().join("sparks/sessions");
    for (name, at) in [("keys.txt", [30, 61, 150, 400]), ("phosphor.txt", [30, 100, 250, 400])] {
        let script = std::fs::read_to_string(dir.join(name)).unwrap();
        let (content, dirs) = plain_service();
        let mut r = Rec::new(SPARKS, &script, Some(content), dirs);
        r.run(450, &at);
        assert!(r.continue_each() >= 3, "{name}");
    }
}

#[test]
fn sketch_with_a_drawing() {
    let script = "3 pointer 100 100 1\n4 pointer 110 100 1\n5 pointer 111 100 1\n6 pointer 120 105 0\n8 press 2\n9 pointer 200 200 1\n10 pointer 200 200 0\n12 press f\n20 pointer 300 300 1\n22 pointer 320 310 1\n24 pointer 330 330 0\n";
    let (content, dirs) = plain_service();
    let mut r = Rec::new(SKETCH, script, Some(content), dirs);
    // 5: in the middle of a stroke, the button held.
    r.run(40, &[5, 11, 13, 21, 30]);
    assert!(r.machine.vector().unwrap().phosphor());
    assert!(r.continue_each() >= 4);
}

#[test]
fn conform_every_ten_frames() {
    let script = std::fs::read_to_string(common::programs().join("conform/conform.events")).unwrap();
    let (content, dirs) = plain_service();
    let mut r = Rec::new(CONFORM, &script, Some(content), dirs);
    let at: Vec<usize> = (0..=200).step_by(10).collect();
    r.run(200, &at);
    let continued = r.continue_each();
    assert!(continued >= 15, "{continued} of {}", at.len());
}

#[test]
fn fetch_pending_request_fails_after_restore() {
    let (store, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    BlobStore::open(store.path(), None).unwrap().put(MESSAGE).unwrap();
    let content = service(store.path(), data.path(), |_| {});
    let mut r = Rec::new(FETCH, "", Some(content), vec![store, data]);
    r.run(10, &[1, 5]);
    let (_, s, pending) = &r.snapshots[0];
    assert!(pending, "the get is pending after frame 0");
    assert!(r.machine.terminal().unwrap().text().contains("done"), "{}", r.machine.terminal().unwrap().text());
    // Restored, it is given what the original got: the get's answer is
    // ignored, for the request failed first.
    let mut m = s.restore(&r.image).unwrap();
    let input = &r.inputs[1];
    let result = m.run_frame(&FrameInput { events: &input.events, completions: &input.completions, ..Default::default() });
    assert_eq!(result.completions_ignored, 1);
    let text = m.terminal().unwrap().text();
    assert!(text.contains("failed"), "{text}");
    assert_eq!(r.continue_each(), 1, "after frame 5 nothing is pending");
}

/// Snapshot file sizes and the time to take and restore one.
#[test]
fn sizes() {
    let mut out = Vec::new();
    for (name, image, script, frames) in [
        ("hello", HELLO, String::new(), 30),
        ("fetch", FETCH, String::new(), 10),
        ("sketch", SKETCH, "3 pointer 100 100 1\n6 pointer 300 200 0\n".to_string(), 20),
        ("sparks", SPARKS, std::fs::read_to_string(common::programs().join("sparks/sessions/keys.txt")).unwrap(), 300),
        ("reader", READER, "12 press enter\n".to_string(), 30),
        ("conform", CONFORM, std::fs::read_to_string(common::programs().join("conform/conform.events")).unwrap(), 200),
    ] {
        let (content, dirs) = with_articles(&[("A long one", LONG)]);
        let mut r = Rec::new(image, &script, Some(content), dirs);
        r.run(frames, &[]);
        let start = Instant::now();
        let file = Snapshot::of(&r.machine).encode();
        let take = start.elapsed();
        let start = Instant::now();
        let m = Snapshot::decode(&file).unwrap().restore(&r.image).unwrap();
        let restore = start.elapsed();
        assert_eq!(m.state_hash(), r.machine.state_hash());
        out.push(format!(
            "{name}: state {} bytes, file {} bytes; take {:.2} ms, restore {:.2} ms",
            r.machine.state().len(),
            file.len(),
            take.as_secs_f64() * 1e3,
            restore.as_secs_f64() * 1e3
        ));
    }
    eprintln!("{}", out.join("\n"));
}

// --- The shell's sessions -------------------------------------------------

/// A system as `terse run --shell` runs it, on a store and data directory
/// that outlive it (so a second run finds the first one's session).
struct Sys {
    system: System,
    headless: HeadlessSystem,
    shell_units: Vec<u64>,
}

impl Sys {
    fn new(store: &Path, data: &Path, script: &str) -> Self {
        Sys::without(store, data, script, None)
    }

    /// With the image `missing` taken out of the store after the built-in
    /// programs are put in.
    fn without(store: &Path, data: &Path, script: &str, missing: Option<[u8; 32]>) -> Self {
        let mut node = Node::new(BlobStore::open(store, None).unwrap(), None);
        node.set_identity(alice());
        node.set_subscriptions(Subscriptions::in_memory([alice().feed_key()]));
        node.set_records(Records::in_data_dir(data));
        node.set_snapshots(Snapshots::in_data_dir(data));
        let ids = builtin::install(node.store(), &[]).unwrap();
        if let Some(id) = missing {
            node.store().remove(&id).unwrap();
        }
        let content = ContentService::start_with_consent(node, Consent::Deny);
        let shell = Image::parse(SHELL).unwrap();
        let mut system = System::new(&shell, true, ids);
        system.set_shell_identity(Some(ProgramId { publisher: [0; 32], name: shell.title.clone() }));
        terse_net::snapshots::load_table(&mut system, data);
        Sys { system, headless: HeadlessSystem::new(Some(content), Script::parse(script).unwrap()), shell_units: Vec::new() }
    }

    /// Runs system frames until the shell's frame counter is `frame`.
    fn run_until(&mut self, frame: u32) {
        while self.system.shell().frame() < frame {
            let r = self.headless.frame(&mut self.system);
            self.shell_units.push(r.shell.unwrap().used);
            assert!(self.system.stopped().is_none());
        }
    }

    fn machine(&self, h: u8) -> &Machine {
        self.system.child(h).and_then(|c| c.machine.as_ref()).unwrap_or_else(|| panic!("machine {h}"))
    }

    fn rects(&self) -> Vec<(u16, u16, u16, u16)> {
        let dev = self.system.shell().machines().unwrap();
        (1..=3).map(|h| dev.slot(h).rect).map(|r| (r.x, r.y, r.w, r.h)).collect()
    }

    fn bar(&self) -> String {
        self.system.shell().terminal().unwrap().text().lines().next().unwrap().trim_end().to_string()
    }

    /// A machine's picture as dumps show it: terminal, shapes, page.
    fn picture(m: &Machine) -> String {
        let page = m.page().map_or(String::new(), |p| {
            let layout = p.document().map(|d| terse_core::page::reference_layout(d, p.region().w)).unwrap_or_default();
            layout.dump(p)
        });
        format!("{}{}{}", m.terminal().map_or(String::new(), |t| t.text()), m.vector().map_or(String::new(), |v| v.dump()), page)
    }

    /// What `terse run --shell --dump` prints.
    fn dump(&self) -> String {
        let mut out = self.system.shell().terminal().unwrap().text();
        let dev = self.system.shell().machines().unwrap();
        for (h, child) in self.system.children() {
            let slot = dev.slot(h);
            let r = slot.rect;
            out += &format!("-- machine {h} {} {} {} {} {} {} --\n", state::name(slot.state), r.x, r.y, r.w, r.h, child.title);
            if let Some(m) = &child.machine {
                out += &m.terminal().map_or(String::new(), |t| t.text());
            }
        }
        out
    }

    fn launcher(&self) -> String {
        let page = self.system.shell().page().unwrap();
        let Some(doc) = page.document() else { return String::new() };
        let layout = terse_core::page::reference_layout(doc, page.region().w.max(1));
        layout.lines.iter().map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>() + "\n").collect()
    }

    fn owner(&self) -> terse_net::Owner {
        terse_net::Owner::of(self.system.shell())
    }
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("shell/expected/{name}.txt"), got);
}

/// From the launcher: the reader (handle 1) opens the pictures article and
/// scrolls; sparks (2) starts a game and thrusts; sketch (3) gets two
/// strokes (its picture is at 331, 253, 298 x 224 in the master layout);
/// the launcher (home), 1 selects the reader, S saves the session.
pub const SESSION: &str = "\
2 press enter
14 press enter
40 press pagedown
50 home
51 press up
51 press up
52 press up
53 press up
54 press up
55 press enter
62 press enter
70 down up
90 up up
100 home
101 press up
101 press up
102 press up
103 press enter
112 pointer 400 300 1
114 pointer 450 330 1
116 pointer 500 360 0
118 press 2
120 pointer 420 420 1
122 pointer 420 420 0
130 home
131 press 1
132 press s
";

struct Saved {
    store: tempfile::TempDir,
    data: tempfile::TempDir,
    /// Each machine's picture, hash and frame counter at the snapshot, and
    /// its hash one frame later.
    pictures: Vec<String>,
    hashes: Vec<[u8; 32]>,
    frames: Vec<u32>,
    next: Vec<[u8; 32]>,
    /// The tiles while a machine had the keyboard (frame 129).
    rects: Vec<(u16, u16, u16, u16)>,
    first: Sys,
}

/// The first run, to just after the session was saved.
fn saved_session() -> Saved {
    let (store, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    {
        let mut node = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
        node.set_identity(alice());
        let draft = Draft { data: PICTURES.as_bytes().to_vec(), title: "Pictures in articles".into(), reply_to: None, time: 1_800_000_000 };
        assert!(matches!(node.publish_article(&draft), terse_net::Done::Published { .. }));
    }
    let mut s = Sys::new(store.path(), data.path(), SESSION);
    s.run_until(130);
    let rects = s.rects();
    assert_eq!(s.system.focus(), 3);
    // Frame 132 has the S; the snapshots are taken at its end.
    s.run_until(133);
    let pictures = (1..=3).map(|h| Sys::picture(s.machine(h))).collect();
    let hashes = (1..=3).map(|h| s.machine(h).state_hash()).collect();
    let frames = (1..=3).map(|h| s.machine(h).frame()).collect();
    s.run_until(134);
    let next = (1..=3).map(|h| s.machine(h).state_hash()).collect();
    Saved { store, data, pictures, hashes, frames, next, rects, first: s }
}

#[test]
fn session_saved_and_restored() {
    let Saved { store, data, pictures, hashes, frames, next, rects, mut first } = saved_session();
    let s = &mut first;
    for (h, title) in [(1, "reader"), (2, "sparks"), (3, "sketch")] {
        assert_eq!(s.system.state(h), state::RUNNING, "machine {h}");
        assert_eq!(s.system.child(h).unwrap().title, title);
    }
    assert!(s.machine(1).page().unwrap().scroll() > 0, "the reader scrolled");
    let doc = s.machine(1).page().unwrap().document().unwrap();
    assert!(terse_core::page::parse(doc).iter().any(|p| p.figure.is_some()), "on the article with figures");
    assert!(s.machine(3).vector_list().len() > 30, "sketch has strokes");
    s.run_until(136);
    assert!(s.bar().starts_with(" ⌂   Session saved "), "{}", s.bar());
    let snapshots = Snapshots::in_data_dir(data.path());
    let owner = s.owner();
    let table = snapshots.table(&owner);
    for slot in 0..3 {
        let h = slot as u8 + 1;
        let info = table[slot].as_ref().unwrap_or_else(|| panic!("slot {slot}"));
        assert_eq!(info.image, *s.machine(h).image_id());
        assert_eq!(info.frames, frames[slot]);
        assert_eq!(info.identity.as_ref().unwrap().name, s.system.child(h).unwrap().title.as_bytes());
        let file = snapshots.load(&owner, slot as u8).unwrap().unwrap();
        assert_eq!(Snapshot::decode(&file).unwrap().hash(), hashes[slot]);
    }
    assert!(table[3..].iter().all(Option::is_none));
    let record = Records::in_data_dir(data.path()).load(&owner, b"session").unwrap().unwrap();
    assert_eq!(record, [1, 1, 1, 0, 3], "version, master layout, before zoom, slot 0 selected, 3 machines");
    let busiest = s.shell_units.iter().copied().max().unwrap();
    eprintln!(
        "shell: busiest frame of the saving run {busiest} units ({:.2}%); the frame of S {}, of the snapshot done events {}",
        common::pct(busiest),
        s.shell_units[132],
        s.shell_units[133]
    );
    drop(first);

    // A new run on the same data: the launcher offers the session; Enter.
    let mut t = Sys::new(store.path(), data.path(), "4 press enter\n20 press pagedown\n");
    t.run_until(4);
    let launcher = t.launcher();
    assert!(launcher.starts_with("PROGRAMS\nRestore last session (3 programs)\nManual\nreader\n"), "{launcher}");
    golden("session-offered", &format!("{}-- launcher --\n{launcher}", t.dump()));
    t.run_until(5);
    assert!((1..=3).all(|h| t.system.state(h) == state::LOADING));
    // Restored at the start of the next frame, each running its first
    // frame in it: as the originals ran theirs after the snapshot.
    t.run_until(6);
    for (i, (h, title)) in [(1, "reader"), (2, "sparks"), (3, "sketch")].into_iter().enumerate() {
        assert_eq!(t.system.state(h), state::RUNNING, "machine {h}");
        assert_eq!(t.system.child(h).unwrap().title, title);
        assert_eq!(t.machine(h).frame(), frames[i] + 1, "the frame counter goes on");
        let slot = t.system.shell().machines().unwrap().slot(h);
        assert_eq!(slot.identity.as_ref().unwrap().name, title.as_bytes());
        assert_eq!(slot.image, *t.machine(h).image_id());
        assert_eq!(t.machine(h).state_hash(), next[i], "{title}: the hash the original had a frame after the snapshot");
    }
    assert_eq!(t.system.focus(), 1, "the selected machine has the keyboard");
    assert_eq!(t.rects(), rects, "the same tiles");
    assert_eq!(Sys::picture(t.machine(1)), pictures[0], "reader");
    assert_eq!(Sys::picture(t.machine(3)), pictures[2], "sketch");
    let launcher = t.launcher();
    golden("session-restored", &t.dump());
    // They go on: sparks moves, the reader scrolls with the keyboard.
    let (sparks, scroll) = (t.machine(2).vector_list().to_vec(), t.machine(1).page().unwrap().scroll());
    eprintln!("reader: scroll {scroll} of a page {} high", terse_core::page::reference_layout(t.machine(1).page().unwrap().document().unwrap(), 512).height);
    t.run_until(24);
    assert_ne!(t.machine(2).vector_list(), &sparks[..], "sparks moves");
    assert!(t.machine(1).page().unwrap().scroll() > scroll, "the reader scrolls");
    assert!(!launcher.contains("Restore"), "offered once");
    eprintln!("shell: the frame of Enter {} units, the next {}, busiest {}", t.shell_units[4], t.shell_units[5], t.shell_units.iter().max().unwrap());
}

#[test]
fn a_session_with_a_missing_image_or_a_bad_snapshot() {
    let Saved { store, data, first, .. } = saved_session();
    let sketch = *first.machine(3).image_id();
    let owner = first.owner();
    drop(first);
    // Sparks's snapshot is damaged; sketch's image is gone and there is no
    // relay.
    let snapshots = Snapshots::in_data_dir(data.path());
    let mut file = snapshots.load(&owner, 1).unwrap().unwrap();
    file[400] ^= 1;
    snapshots.save(&owner, 1, &file).unwrap();
    let mut t = Sys::without(store.path(), data.path(), "4 press enter\n", Some(sketch));
    t.run_until(8);
    assert_eq!(t.system.state(1), state::RUNNING);
    let dev = t.system.shell().machines().unwrap();
    assert_eq!((dev.slot(2).state, dev.slot(2).failure), (state::FAILED, failure::SNAPSHOT));
    assert_eq!((dev.slot(3).state, dev.slot(3).failure), (state::FAILED, failure::NOT_FOUND));
    let dump = t.dump();
    assert!(dump.contains("no usable snapshot") && dump.contains("not found"), "{dump}");
    golden("session-failed", &dump);
}

/// A session saved in the gallery layout comes back in it: the machines on
/// the ring, and the selected one faced with the keyboard once it runs.
#[test]
fn a_session_in_the_gallery() {
    let (store, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    // Reader, then hello; the home key, L twice (grid, gallery), S.
    let script = "2 press enter\n10 home\n11 press up\n11 press up\n12 press up\n13 press enter\n20 home\n21 press l\n22 press l\n30 press s\n";
    let mut s = Sys::new(store.path(), data.path(), script);
    s.run_until(40);
    assert!(s.bar().contains("Session saved"), "{}", s.bar());
    let owner = s.owner();
    let record = Records::in_data_dir(data.path()).load(&owner, b"session").unwrap().unwrap();
    assert_eq!(record, [1, 3, 3, 1, 2], "version, the gallery, before zoom, slot 1 (hello) selected, 2 machines");
    drop(s);
    let mut t = Sys::new(store.path(), data.path(), "4 press enter\n");
    t.run_until(30);
    assert_eq!(t.system.focus(), 2);
    assert!(t.rects().iter().all(|r| r.2 == 0), "no tiles: panels");
    let dev = t.system.shell().machines().unwrap();
    let v = *dev.view();
    assert_eq!((v.handle, v.toward, v.progress(t.system.shell().frame() - 1)), (2, true, 4096));
    let pictures: Vec<_> = t.system.pictures().into_iter().map(|(h, p)| (h, p.x, p.y, p.w, p.h)).collect();
    assert_eq!(pictures, [(2, 17, 23, 605, 453)]);
    assert_eq!(terse_core::vector3::screens_in(t.system.shell().vector3().unwrap().list()), 0b11);
    golden("session-gallery", &format!("{}{}", t.dump(), t.system.shell().vector3().unwrap().dump()));
}

#[test]
fn a_session_discarded() {
    let Saved { store, data, first, .. } = saved_session();
    let owner = first.owner();
    drop(first);
    let mut t = Sys::new(store.path(), data.path(), "4 press d\n");
    t.run_until(8);
    assert!(t.bar().starts_with(" ⌂   Session discarded"), "{}", t.bar());
    assert!(t.launcher().starts_with("PROGRAMS\nManual\nreader\n"), "{}", t.launcher());
    assert_eq!(t.system.children().count(), 0);
    assert!(Snapshots::in_data_dir(data.path()).table(&owner).iter().all(Option::is_none));
    assert_eq!(Records::in_data_dir(data.path()).load(&owner, b"session").unwrap(), None);
    drop(t);
    let mut u = Sys::new(store.path(), data.path(), "");
    u.run_until(4);
    assert!(u.launcher().starts_with("PROGRAMS\nManual\nreader\n"), "not offered again");
}

/// `terse run --save-state` and `--load-state`: the hash at the moment of
/// the snapshot, and the hashes of a continued run.
#[test]
fn cli_save_and_load_state() {
    let dir = tempfile::tempdir().unwrap();
    let p = |n: &str| dir.path().join(n).to_str().unwrap().to_string();
    let sparks = common::programs().join("sparks/sparks.terse");
    let keys = common::programs().join("sparks/sessions/keys.txt");
    let run = |extra: &[&str]| {
        let out = common::terse_command()
            .args(["run", sparks.to_str().unwrap(), "--events", keys.to_str().unwrap()])
            .args(["--store", &p("store"), "--data", &p("data")])
            .args(extra)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    let saved = run(&["--frames", "300", "--hash", "--save-state", &p("s.snap")]);
    let loaded = run(&["--frames", "0", "--hash", "--load-state", &p("s.snap")]);
    assert_eq!(saved, loaded);
    let whole = run(&["--frames", "600", "--hash-every", "100"]);
    let continued = run(&["--frames", "300", "--hash-every", "100", "--load-state", &p("s.snap")]);
    assert_eq!(whole.lines().skip(3).collect::<Vec<_>>(), continued.lines().collect::<Vec<_>>());
    // Another image refuses it.
    let hello = common::programs().join("hello/hello.terse");
    let out = common::terse_command().args(["run", hello.to_str().unwrap(), "--load-state", &p("s.snap")]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("another image"));
    let out = common::terse_command().args(["run", "--shell", "--load-state", &p("s.snap")]).output().unwrap();
    assert!(!out.status.success(), "not for a shell");
}
