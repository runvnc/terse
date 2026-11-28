//! programs/stillwind (The Stillwind Spire, a painted adventure: scenes
//! in page figures with layers, a walking character, a parser below):
//! every transcript in programs/stillwind/transcripts/ played headless,
//! its replies, scenes and layers checked (the walkthrough of the built
//! part is one of them); the scenes fetched as blobs through the content
//! device, the next ones before she walks into them, so that a scene
//! change never waits, and an honest notice while one has not come (a
//! slow fetch) or cannot come (a missing blob, no content device); the
//! frame budget walking, typing and changing scenes; the same input gives
//! the same run; story_data.h and the blobs are what story.py makes of
//! the story, and the image is up to date with them and stillwind.c.
//!
//! A transcript (format at the top of transcripts/walkthrough.txt) types
//! commands and checks fragments of the replies, read from the engine's
//! own record of its text (found in RAM by its marker, WINDTEXT), the
//! status line from the terminal, the page's layers, and a probe of the
//! scene the engine keeps (her place, the scene shown).

mod common;

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use terse_core::script::Script;
use terse_core::{Completion, Event, FrameInput, FrameResult, Grants, Image, Machine, RAM_BASE};
use terse_net::{BlobStore, ContentService, Node, Records};

/// The program's name: its directory, its C file and its image are named
/// after it (programs/stillwind/build.sh).
const NAME: &str = "stillwind";
const RAM_PAGES: u8 = 28;
const STATUS_ROW: usize = 20;
const INPUT_ROW: usize = 29;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs").join(NAME)
}

static IMAGE_BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| std::fs::read(dir().join(format!("{NAME}.terse"))).unwrap());
fn image() -> Image {
    Image::parse(&IMAGE_BYTES).unwrap()
}

/// The scenes' blobs (blobs/<room>-<hash>.scene): room name, bytes.
fn blobs() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir().join("blobs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "scene"))
        .map(|p| (p.file_name().unwrap().to_string_lossy().split('-').next().unwrap().to_string(), std::fs::read(&p).unwrap()))
        .collect();
    v.sort();
    v
}

/// LAYER_<ROOM>_<NAME> from story_data.h: "room_name" -> layer.
fn layer_names() -> BTreeMap<String, usize> {
    std::fs::read_to_string(dir().join("story_data.h"))
        .unwrap()
        .lines()
        .filter_map(|l| l.strip_prefix("#define LAYER_"))
        .map(|l| {
            let (k, v) = l.split_once(' ').unwrap();
            (k.to_lowercase(), v.trim().parse().unwrap())
        })
        .collect()
}

/// The rooms in story order, as story_data.h numbers them (the blobs'
/// order in scene_len is story order; names from `room` lines).
fn room_index(name: &str) -> u8 {
    let mut rooms = Vec::new();
    for f in ["0-game", "1-meadow", "2-foot", "3-hall"] {
        let t = std::fs::read_to_string(dir().join(format!("story/{f}.story"))).unwrap();
        for l in t.lines() {
            if let Some(r) = l.strip_prefix("room ") {
                rooms.push(r.split_whitespace().next().unwrap().to_string());
            }
        }
    }
    rooms.iter().position(|r| r == name).unwrap_or_else(|| panic!("no room {name}")) as u8
}

/// How the content device answers: at once (as a host with the blobs in
/// its store does, the next frame), or each completion held back some
/// frames (a slow network).
#[derive(Clone, Copy)]
struct Setup {
    content: bool,
    delay: u32,
    leave_out: Option<&'static str>,
}

const NORMAL: Setup = Setup { content: true, delay: 0, leave_out: None };

/// What the engine keeps for the tests (stillwind.c `struct probe`).
#[derive(Debug, Default, Clone, Copy)]
struct Probe {
    busy: bool,
    room: u8,
    shown_room: u8,
    pose: u8,
    scale: u8,
    layers_live: bool,
    fetching: bool,
    x: i16,
    y: i16,
    fetches: u32,
}

struct Game {
    m: Machine,
    content: Option<ContentService>,
    held: VecDeque<(u32, Completion)>,
    delay: u32,
    _dir: tempfile::TempDir,
    used: Vec<u64>,
    spilled: usize,
    submits: Vec<(usize, u32)>,
    text_at: Option<usize>,
}

impl Game {
    fn new(s: Setup) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let image = image();
        let m = if s.content { Machine::new(&image) } else { Machine::with_grants(&image, Grants(vec![false])) };
        let store = BlobStore::open(dir.path().join("store"), None).unwrap();
        for (room, bytes) in blobs() {
            if Some(room.as_str()) != s.leave_out {
                store.put(&bytes).unwrap();
            }
        }
        let content = m.has_device(4).then(|| {
            let mut node = Node::new(store, None);
            node.set_records(Records::in_data_dir(&dir.path().join("data")));
            ContentService::start(node)
        });
        let mut g = Game { m, content, held: VecDeque::new(), delay: s.delay, _dir: dir, used: vec![], spilled: 0, submits: vec![], text_at: None };
        g.frames(3);
        g
    }

    fn frame_with(&mut self, events: &[Event]) -> FrameResult {
        let now = self.m.frame();
        let mut completions = Vec::new();
        while self.held.front().is_some_and(|(at, _)| *at <= now) {
            completions.push(self.held.pop_front().unwrap().1);
        }
        let generation = self.m.page().map(|p| p.generation()).unwrap_or(0);
        let input = FrameInput { events, completions: &completions, ..Default::default() };
        let r = self.m.run_frame(&input);
        assert!(self.m.fault().is_none(), "fault: {:?}", self.m.fault());
        if self.m.page().map(|p| p.generation()).unwrap_or(0) != generation {
            self.submits.push((self.used.len(), now));
        }
        if let Some(c) = &mut self.content {
            c.submit_machine(0, &mut self.m);
            let at = self.m.frame() + self.delay;
            for c in c.wait_finished() {
                self.held.push_back((at, c));
            }
        }
        self.used.push(r.used);
        if !r.waited && r.stopped.is_none() {
            self.spilled += 1;
        }
        r
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame_with(&[]);
        }
    }

    fn input(&mut self, line: &str) {
        let f = self.m.frame();
        let script = Script::parse(&format!("{f} {line}\n")).unwrap_or_else(|e| panic!("{line}: {e}"));
        let events = script.events(f).to_vec();
        self.frame_with(&events);
    }

    fn screen(&self) -> String {
        self.m.terminal().unwrap().text()
    }

    fn row(&self, n: usize) -> String {
        self.screen().lines().nth(n).unwrap_or("").to_string()
    }

    fn more(&self) -> bool {
        self.row(INPUT_ROW).contains("-- more --")
    }

    fn through_more(&mut self) {
        for _ in 0..40 {
            if !self.more() {
                return;
            }
            self.input("press space");
            self.frames(1);
        }
        panic!("still at -- more --:\n{}", self.screen());
    }

    fn view(&mut self) -> usize {
        let ram = self.m.ram();
        *self.text_at.get_or_insert_with(|| ram.windows(8).position(|w| w == b"WINDTEXT").expect("the text's marker in RAM"))
    }

    fn word(&self, o: usize) -> u32 {
        u32::from_le_bytes(self.m.ram()[o..o + 4].try_into().unwrap())
    }

    fn lines(&mut self) -> (u32, Vec<String>) {
        let at = self.view();
        let ptr = |g: &Self, o: usize| (g.word(o) - RAM_BASE) as usize;
        let count = self.word(ptr(self, at + 8));
        let ring = ptr(self, at + 12);
        let size = self.word(at + 16);
        let cols = self.word(at + 20) as usize;
        let ram = self.m.ram();
        let first = count.saturating_sub(size);
        let lines = (first..count)
            .map(|i| {
                let l = &ram[ring + (i % size) as usize * cols..][..cols];
                l[2..2 + l[0] as usize].iter().map(|&b| cp1252(b)).collect()
            })
            .collect();
        (first, lines)
    }

    fn probe(&mut self) -> Probe {
        let at = self.view();
        let p = (self.word(at + 28) - RAM_BASE) as usize;
        let r = &self.m.ram()[p..p + 24];
        Probe {
            busy: r[0] != 0,
            room: r[1],
            shown_room: r[2],
            pose: r[4],
            scale: r[5],
            layers_live: r[6] != 0,
            fetching: r[7] != 0,
            x: i16::from_le_bytes([r[8], r[9]]),
            y: i16::from_le_bytes([r[10], r[11]]),
            fetches: u32::from_le_bytes(r[12..16].try_into().unwrap()),
        }
    }

    fn since(&mut self, from: u32) -> String {
        let (first, lines) = self.lines();
        lines.iter().skip(from.saturating_sub(first) as usize).cloned().collect::<Vec<_>>().join("\n")
    }

    fn count(&mut self) -> u32 {
        let (first, lines) = self.lines();
        first + lines.len() as u32
    }

    /// Frames until she has stopped walking, no command waits and the
    /// scene is shown with its layers (at most 20 seconds).
    fn settle(&mut self) {
        for _ in 0..1200 {
            if !self.more() && !self.probe().busy {
                return;
            }
            if self.more() {
                self.input("press space");
            }
            self.frames(1);
        }
        panic!("still busy: {:?}\n{}", self.probe(), self.screen());
    }

    fn command(&mut self, cmd: &str) -> String {
        self.through_more();
        let before = self.count();
        self.input(&format!("text {cmd}"));
        self.input("press enter");
        self.frames(3);
        self.settle();
        self.since(before)
    }

    fn layer(&self, id: usize) -> terse_core::page::LayerState {
        self.m.page().unwrap().layers()[id]
    }

    fn document(&self) -> Vec<u8> {
        self.m.page().and_then(|p| p.document()).map(<[u8]>::to_vec).unwrap_or_default()
    }

    fn busiest(&self) -> u64 {
        *self.used.iter().skip(2).max().unwrap_or(&0)
    }
}

fn cp1252(b: u8) -> char {
    match b {
        0x85 => '…',
        0x91 => '‘',
        0x92 => '’',
        0x93 => '“',
        0x94 => '”',
        0x96 => '–',
        0x97 => '—',
        _ => b as char,
    }
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Plays a transcript; the failures, each with the reply it looked at.
fn play(name: &str, text: &str, game: &mut Game) -> Vec<String> {
    let layers = layer_names();
    let mut fails = Vec::new();
    let mut reply = String::new();
    let mut last = String::from("(the start)");
    game.settle();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end();
        let at = format!("{name}:{}", n + 1);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (kind, rest) = line.split_once(' ').unwrap_or((line, ""));
        match kind {
            ">" => {
                reply = game.command(rest);
                last = format!("> {rest}");
            }
            "@" => {
                game.through_more();
                let before = game.count();
                game.input(rest);
                game.frames(3);
                reply = game.since(before);
                last = format!("@ {rest}");
            }
            "settle" => {
                let before = game.count();
                game.settle();
                reply += &game.since(before);
            }
            "<" | "!" => {
                let has = squash(&reply).contains(&squash(rest));
                if has != (kind == "<") {
                    fails.push(format!("{at}: after {last}, the reply {} {rest:?}; it was:\n{reply}\n", if kind == "<" { "lacks" } else { "holds" }));
                }
            }
            "status" => {
                let s = game.row(STATUS_ROW);
                if !s.contains(rest) {
                    fails.push(format!("{at}: after {last}, the status line lacks {rest:?}: {s:?}"));
                }
            }
            "screen" => {
                let s = game.screen();
                if !squash(&s).contains(&squash(rest)) {
                    fails.push(format!("{at}: after {last}, the screen lacks {rest:?}:\n{s}"));
                }
            }
            "shown" | "hidden" => {
                let id = *layers.get(rest).unwrap_or_else(|| panic!("{at}: no layer {rest} (story_data.h LAYER_*)"));
                if game.layer(id).shown != (kind == "shown") {
                    fails.push(format!("{at}: after {last}, layer {rest} ({id}) is not {kind}"));
                }
            }
            "scene" => {
                let p = game.probe();
                let want = room_index(rest);
                if p.room != want || p.shown_room != want || !p.layers_live {
                    fails.push(format!("{at}: after {last}, the scene is not {rest}'s: {p:?}"));
                }
            }
            "near" => {
                let v: Vec<i16> = rest.split_whitespace().map(|w| w.parse().unwrap()).collect();
                let p = game.probe();
                let tol = v.get(2).copied().unwrap_or(4);
                if (p.x - v[0]).abs() > tol || (p.y - v[1]).abs() > tol {
                    fails.push(format!("{at}: after {last}, her feet are at ({}, {}), not near ({}, {})", p.x, p.y, v[0], v[1]));
                }
            }
            "wait" => game.frames(rest.parse().unwrap()),
            _ => fails.push(format!("{at}: not a transcript line: {line:?}")),
        }
    }
    fails
}

fn transcripts() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_dir(dir().join("transcripts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap()))
        .collect();
    v.sort();
    v
}

/// The most any frame may use, percent of the budget: Low Bell's bound
/// (the busiest frame is a new room's text, as there).
const BUSIEST: f64 = 46.0;

/// Every transcript, each from a new game: all their checks hold, and no
/// frame uses more than BUSIEST of its budget (start, scene changes and
/// fetches included).
#[test]
fn transcripts_play_as_written() {
    let all = transcripts();
    assert!(all.iter().any(|(n, _)| n == "walkthrough.txt"));
    let mut fails = Vec::new();
    for (name, text) in &all {
        let mut g = Game::new(NORMAL);
        let f = play(name, text, &mut g);
        eprintln!(
            "stillwind {name}: {} frames, busiest {} units ({:.1}%), {} page submits, {} failed",
            g.used.len(),
            g.busiest(),
            common::pct(g.busiest()),
            g.submits.len(),
            f.len()
        );
        if g.spilled != 0 {
            fails.push(format!("{name}: a frame's work ran into the next"));
        }
        let (at, worst) = g.used.iter().copied().enumerate().max_by_key(|&(_, u)| u).unwrap();
        if common::pct(worst) >= BUSIEST {
            fails.push(format!("{name}: frame {at} used {worst} units ({:.1}%)", common::pct(worst)));
        }
        fails.extend(f);
    }
    assert!(fails.is_empty(), "{} failed:\n{}", fails.len(), fails.join("\n"));
}

/// The costs of the walkthrough's kinds of frame: walking (only `layer`
/// calls), typing (the input line), a command and its reply, a scene
/// change (a submit of a scene already in RAM), a fetch (16 KiB delivered).
#[test]
fn costs_of_walking_typing_and_scene_changes() {
    let mut g = Game::new(NORMAL);
    g.settle();
    let mark = g.used.len();
    g.input("down left");
    g.frames(60);
    g.input("up left");
    g.frames(2);
    let walking = common::FrameStats::new(&g.used[mark + 1..mark + 60]);
    let mark = g.used.len();
    for c in "listen".chars() {
        g.input(&format!("text {c}"));
    }
    let typing = common::FrameStats::new(&g.used[mark..]);
    g.input("press enter");
    let command = g.used[g.used.len() - 1];
    g.settle();
    // Walk up the path into the next scene: the submit frame.
    let before = g.submits.len();
    g.command("north");
    let p = g.probe();
    assert_eq!(p.room, room_index("foot"));
    let (frame, _) = g.submits[before];
    let change = g.used[frame];
    let fetch = *g.used[frame + 1..].iter().max().unwrap();
    let arrival = *g.used[frame.saturating_sub(4)..frame].iter().max().unwrap();
    eprintln!(
        "stillwind costs: walking {}; typing {}; a command {} units ({:.2}%); the new room's text {} units ({:.2}%); a scene change {} units ({:.2}%); the frames after it, fetching the next scenes, at most {} ({:.2}%)",
        walking.describe(),
        typing.describe(),
        command,
        common::pct(command),
        arrival,
        common::pct(arrival),
        change,
        common::pct(change),
        fetch,
        common::pct(fetch)
    );
    assert!(common::pct(walking.busiest) < 3.0, "walking: {}", walking.describe());
    assert!(common::pct(typing.busiest) < 3.0, "typing: {}", typing.describe());
    assert!(common::pct(change) < 25.0, "scene change {change}");
}

/// The scenes the exits lead to are fetched while she is in a room: when
/// she walks into the next one, its scene is in RAM and is submitted the
/// frame after the room's text, without a notice in between, and without
/// a frame over a third of the budget.
#[test]
fn scene_change_after_preload_never_waits() {
    let mut g = Game::new(NORMAL);
    g.settle();
    g.frames(30);
    let p = g.probe();
    assert!(!p.fetching && p.fetches >= 2, "the meadow and the Spire's foot fetched: {p:?}");
    let before = g.submits.len();
    let text = g.command("north");
    assert!(text.contains("The Spire's Foot"), "{text}");
    let docs: Vec<usize> = g.submits[before..].iter().map(|s| s.0).collect();
    assert_eq!(docs.len(), 1, "one submit, the scene itself (no notice): {docs:?}");
    let foot = &blobs().into_iter().find(|(r, _)| r == "foot").unwrap().1;
    assert_eq!(&g.document(), foot);
    assert!(common::pct(g.busiest()) < BUSIEST);
}

/// A scene not yet fetched (the content device answers 90 frames late):
/// the room's text comes at once, a short notice stands in the picture's
/// place, and the scene replaces it when it comes; no frame over a third
/// of the budget, nothing waits.
#[test]
fn scene_not_yet_fetched_shows_a_notice() {
    let mut g = Game::new(Setup { delay: 90, ..NORMAL });
    g.frames(2);
    let doc = g.document();
    assert!(String::from_utf8_lossy(&doc).contains("Fetching the scene"), "{doc:?}");
    let p = g.probe();
    assert!(p.busy && p.shown_room == 0xFF, "{p:?}");
    assert!(g.screen().contains("The Meadow Path"), "{}", g.screen());
    g.settle();
    assert_eq!(g.probe().shown_room, room_index("meadow"));
    // Straight on north before the Spire's foot has come: the notice again.
    let text = g.command("north");
    assert!(text.contains("The Spire's Foot"), "{text}");
    assert!(common::pct(g.busiest()) < BUSIEST, "busiest {}", g.busiest());
    assert_eq!(g.spilled, 0);
}

/// A scene whose blob nobody has: the notice says so, and the story goes
/// on in words (she can still take the exits by name).
#[test]
fn a_missing_scene_says_so() {
    let mut g = Game::new(Setup { leave_out: Some("foot"), ..NORMAL });
    g.settle();
    let text = g.command("north");
    assert!(text.contains("The Spire's Foot"), "{text}");
    g.frames(10);
    let doc = String::from_utf8_lossy(&g.document()).to_string();
    assert!(doc.contains("could not be fetched"), "{doc}");
    let text = g.command("read note");
    assert!(text.contains("Gone down the mere"), "{text}");
    let text = g.command("south");
    assert!(text.contains("The Meadow Path"), "{text}");
    g.settle();
    assert_eq!(g.probe().shown_room, room_index("meadow"));
}

/// Without the content device there are no scenes and no saving; the
/// story is the same in words.
#[test]
fn plays_without_the_content_device() {
    let mut g = Game::new(Setup { content: false, ..NORMAL });
    g.settle();
    let doc = String::from_utf8_lossy(&g.document()).to_string();
    assert!(doc.contains("no content device"), "{doc}");
    let r = g.command("save");
    assert!(r.contains("This host has no storage for saved games."), "{r}");
    for (c, want) in [("north", "The Spire's Foot"), ("knock apple with staff", "drops into the grass"), ("take apple", "Taken")] {
        let r = g.command(c);
        assert!(squash(&r).contains(want), "{c}: {r}");
    }
}

/// The same input gives the same machine, frame for frame.
#[test]
fn same_input_same_run() {
    let text = std::fs::read_to_string(dir().join("transcripts/walkthrough.txt")).unwrap();
    let mut a = Game::new(NORMAL);
    let mut b = Game::new(NORMAL);
    play("walkthrough.txt", &text, &mut a);
    play("walkthrough.txt", &text, &mut b);
    assert_eq!(a.m.state_hash(), b.m.state_hash());
    assert_eq!(a.used, b.used);
}

/// story_data.h and the blobs are what story.py makes of story/ and
/// scenes/, and stillwind.terse is built from them and stillwind.c as
/// build.sh builds it.
#[test]
fn image_is_up_to_date() {
    let python = std::process::Command::new("python3").arg("--version").output().is_ok_and(|o| o.status.success());
    if !python {
        terse_testkit::skip!("python3", "needs python3 to run story.py");
    } else {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("story_data.h");
        let o = std::process::Command::new("python3")
            .arg(dir().join("story.py"))
            .arg(dir().join("story"))
            .arg("-o")
            .arg(&out)
            .output()
            .unwrap();
        assert!(o.status.success(), "story.py: {}", String::from_utf8_lossy(&o.stderr));
        assert!(
            std::fs::read(&out).unwrap() == std::fs::read(dir().join("story_data.h")).unwrap(),
            "programs/stillwind/story_data.h is out of date: run programs/stillwind/build.sh"
        );
        for (room, bytes) in blobs() {
            let fresh = std::fs::read_dir(tmp.path().join("blobs")).unwrap().map(|e| e.unwrap().path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{room}-")));
            assert_eq!(fresh.map(|p| std::fs::read(p).unwrap()), Some(bytes), "programs/stillwind/blobs/{room}-*.scene is out of date: run build.sh");
        }
    }
    // The image lists the scenes' blobs (SPEC §image.blobs), in file name
    // order, after the program's own ROM.
    let img = image();
    let listed: Vec<([u8; 32], u32)> = img.blob_list().iter().map(|b| (b.id, b.size)).collect();
    let want: Vec<([u8; 32], u32)> = blobs().iter().map(|(_, b)| (terse_net::blob_id(b), b.len() as u32)).collect();
    assert_eq!(listed, want, "the image's blob list is not the scenes': run programs/stillwind/build.sh");
    let code = &img.rom[..img.rom.len() - 4 - 36 * listed.len()];
    let devices: Vec<terse_core::DeviceReq> = img.devices.clone();
    let without = Image::build(NAME, img.ram_pages, &devices, code);
    common::check_up_to_date(&without, &[&format!("{NAME}/{NAME}.c")], true, NAME, RAM_PAGES, &[(2, 0), (3, 0), (7, 0), (4, 0x80)]);
}
