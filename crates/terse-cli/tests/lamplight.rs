//! programs/lamplight (an illustrated text adventure engine and its
//! placeholder story): every transcript in programs/lamplight/transcripts/
//! played headless, as `terse run` plays it, its replies checked (the
//! walkthrough of the story is one of them); the walkthrough again through
//! `terse run --events`; the frame budget and the scene's ink over the
//! walkthrough; the game without its optional devices (sound, saving);
//! the same input gives the same run; story_data.h is what story.py makes
//! of the story, and the image is up to date with it and lamplight.c.
//!
//! A transcript (format at the top of transcripts/walkthrough.txt) types
//! commands and checks fragments of the replies, read from the engine's
//! own record of its text (found in RAM by its marker, LAMPTEXT), the
//! status line from the terminal, and the scene from the vector list.

mod common;

use std::path::{Path, PathBuf};

use terse_core::script::Script;
use terse_core::{Completion, Event, FrameInput, FrameResult, Grants, Image, Machine, RAM_BASE};
use terse_net::{BlobStore, ContentService, Node, Records};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/lamplight")
}

/// The image, read when a test starts.
static IMAGE_BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| std::fs::read(dir().join("lamplight.terse")).unwrap());
fn image() -> Image {
    Image::parse(&IMAGE_BYTES).unwrap()
}
const RAM_PAGES: u8 = 9;

/// A game being played: the machine, a content service for its saved
/// games, and what each frame used.
struct Game {
    m: Machine,
    content: Option<ContentService>,
    completions: Vec<Completion>,
    _dir: tempfile::TempDir,
    used: Vec<u64>,
    spilled: usize,
    ink: Vec<u64>,
    list_bytes: usize,
    text_at: Option<usize>,
}

impl Game {
    fn new(grants: Option<Grants>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let image = image();
        let m = match grants {
            Some(g) => Machine::with_grants(&image, g),
            None => Machine::new(&image),
        };
        let content = m.has_device(4).then(|| {
            let mut node = Node::new(BlobStore::open(dir.path().join("store"), None).unwrap(), None);
            node.set_records(Records::in_data_dir(&dir.path().join("data")));
            ContentService::start(node)
        });
        let mut g = Game { m, content, completions: vec![], _dir: dir, used: vec![], spilled: 0, ink: vec![], list_bytes: 0, text_at: None };
        g.frames(3);
        g
    }

    fn frame_with(&mut self, events: &[Event]) -> FrameResult {
        let completions = std::mem::take(&mut self.completions);
        let input = FrameInput { events, completions: &completions, ..Default::default() };
        let r = self.m.run_frame(&input);
        assert!(self.m.fault().is_none(), "fault: {:?}", self.m.fault());
        if let Some(c) = &mut self.content {
            c.submit_machine(0, &mut self.m);
            self.completions = c.wait_finished();
        }
        self.used.push(r.used);
        if !r.waited && r.stopped.is_none() {
            self.spilled += 1;
        }
        if let Some(v) = self.m.vector() {
            let list = v.list();
            self.list_bytes = self.list_bytes.max(list.len());
            self.ink.push(terse_core::vector::decode(list).map(|p| terse_core::vector::ink(&p)).sum());
        }
        r
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame_with(&[]);
        }
    }

    /// One line of a `terse run` script, given in this frame.
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
        self.row(29).contains("-- more --")
    }

    /// Space while the reply waits at "-- more --".
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

    /// The engine's lines of text: (count ever written, the lines kept).
    fn lines(&mut self) -> (u32, Vec<String>) {
        let ram = self.m.ram();
        let at = *self.text_at.get_or_insert_with(|| ram.windows(8).position(|w| w == b"LAMPTEXT").expect("the text's marker in RAM"));
        let word = |o: usize| u32::from_le_bytes(ram[o..o + 4].try_into().unwrap());
        let ptr = |o: usize| (word(o) - RAM_BASE) as usize;
        let count = word(ptr(at + 8));
        let ring = ptr(at + 12);
        let size = word(at + 16);
        let cols = word(at + 20) as usize;
        let first = count.saturating_sub(size);
        let lines = (first..count)
            .map(|i| {
                let l = &ram[ring + (i % size) as usize * cols..][..cols];
                l[2..2 + l[0] as usize].iter().map(|&b| cp1252(b)).collect()
            })
            .collect();
        (first, lines)
    }

    /// The text written since line `from` (lines ever written).
    fn since(&mut self, from: u32) -> String {
        let (first, lines) = self.lines();
        lines.iter().skip(from.saturating_sub(first) as usize).cloned().collect::<Vec<_>>().join("\n")
    }

    fn count(&mut self) -> u32 {
        let (first, lines) = self.lines();
        first + lines.len() as u32
    }

    /// Types a command and Enter; the reply, all of it.
    fn command(&mut self, cmd: &str) -> String {
        self.through_more();
        let before = self.count();
        self.input(&format!("text {cmd}"));
        self.input("press enter");
        self.frames(3);
        self.through_more();
        self.since(before)
    }

    fn vectors(&self) -> String {
        self.m.vector().unwrap().dump()
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
        0x95 => '•',
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
    let mut fails = Vec::new();
    let mut reply = String::new();
    let mut last = String::from("(the start)");
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
                game.through_more();
                reply = game.since(before);
                last = format!("@ {rest}");
            }
            "<" | "!" => {
                let has = squash(&reply).contains(&squash(rest));
                if has != (kind == "<") {
                    fails.push(format!("{at}: after {last}, the reply {} {rest:?}; it was:\n{reply}\n", if kind == "<" { "lacks" } else { "holds" }));
                }
            }
            "status" => {
                let s = game.row(16);
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
            "vectors" | "novectors" => {
                let v = game.vectors();
                if v.contains(rest) != (kind == "vectors") {
                    fails.push(format!("{at}: after {last}, the scene {} {rest:?}", if kind == "vectors" { "lacks" } else { "holds" }));
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

/// Every transcript, each from a new game: all their checks hold, no frame
/// runs out of budget, and the scene stays far inside the ink cap.
#[test]
fn transcripts_play_as_written() {
    let all = transcripts();
    assert!(all.iter().any(|(n, _)| n == "walkthrough.txt"));
    let mut fails = Vec::new();
    for (name, text) in &all {
        let mut g = Game::new(None);
        let f = play(name, text, &mut g);
        let ink = g.ink.iter().max().copied().unwrap_or(0);
        eprintln!(
            "lamplight {name}: {} frames, busiest {} units ({:.1}%), most ink {ink} ({:.1}% of the cap), list {} bytes, {} failed",
            g.used.len(),
            g.busiest(),
            common::pct(g.busiest()),
            ink as f64 * 100.0 / 230_400.0,
            g.list_bytes,
            f.len()
        );
        assert_eq!(g.spilled, 0, "{name}: a frame's work ran into the next");
        fails.extend(f);
    }
    assert!(fails.is_empty(), "{} failed:\n{}", fails.len(), fails.join("\n"));
}

/// The walkthrough as `terse run --events` plays it: each command typed,
/// with Esc before it (which answers a "more" prompt and otherwise clears
/// the empty input line); the story is won.
#[test]
fn walkthrough_through_terse_run() {
    let text = std::fs::read_to_string(dir().join("transcripts/walkthrough.txt")).unwrap();
    let mut script = String::new();
    let mut f = 10;
    for line in text.lines().filter_map(|l| l.strip_prefix("> ")) {
        for _ in 0..4 {
            script += &format!("{f} press esc\n");
            f += 1;
        }
        script += &format!("{f} text {line}\n{} press enter\n", f + 1);
        f += 4;
    }
    let tmp = tempfile::tempdir().unwrap();
    let events = tmp.path().join("walk.events");
    std::fs::write(&events, &script).unwrap();
    let out = common::terse_command()
        .args(["run", dir().join("lamplight.terse").to_str().unwrap(), "--events", events.to_str().unwrap()])
        .args(["--frames", &(f + 10).to_string(), "--dump", "--relay", "none"])
        .arg("--store")
        .arg(tmp.path().join("store"))
        .arg("--data")
        .arg(tmp.path().join("data"))
        .output()
        .unwrap();
    let screen = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(screen.contains("*** You have won ***") || screen.contains("Type RESTART, RESTORE, UNDO or QUIT."), "{screen}");
    assert!(screen.contains("score 10/10"), "{screen}");
}

/// The walkthrough's frames, every one: well inside the budget (a command
/// with its reply, a new scene, the beam), and the picture far from the
/// ink cap.
#[test]
fn frames_stay_inside_the_budget_and_ink() {
    let text = std::fs::read_to_string(dir().join("transcripts/walkthrough.txt")).unwrap();
    let mut g = Game::new(None);
    assert!(play("walkthrough.txt", &text, &mut g).is_empty());
    // The beam swept over the boathouse, held down for a second each way.
    let mut g2 = Game::new(None);
    for c in ["light lantern", "open box", "take key", "unlock door", "open door", "east"] {
        g2.command(c);
    }
    for k in ["left", "up", "right", "down"] {
        g2.input(&format!("down {k}"));
        g2.frames(60);
        g2.input(&format!("up {k}"));
    }
    for g in [&g, &g2] {
        let stats = common::FrameStats::new(&g.used[2..]);
        let ink = *g.ink.iter().max().unwrap();
        eprintln!("lamplight: {}; most ink {ink}", stats.describe());
        assert!(common::pct(stats.busiest) < 40.0, "busiest frame {} units", stats.busiest);
        assert!(ink < 230_400 / 4, "ink {ink}");
        assert_eq!(g.spilled, 0);
    }
}

/// Without the content device there is no saving, said so; without sound
/// the story plays the same.
#[test]
fn plays_without_the_optional_devices() {
    let mut g = Game::new(Some(Grants(vec![false, false])));
    let r = g.command("save");
    assert!(r.contains("This host has no storage for saved games."), "{r}");
    let text = std::fs::read_to_string(dir().join("transcripts/walkthrough.txt")).unwrap();
    let mut g = Game::new(Some(Grants(vec![false, false])));
    let fails = play("walkthrough.txt", &text, &mut g);
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

/// The same input gives the same machine, frame for frame.
#[test]
fn same_input_same_run() {
    let text = std::fs::read_to_string(dir().join("transcripts/walkthrough.txt")).unwrap();
    let mut a = Game::new(None);
    let mut b = Game::new(None);
    play("walkthrough.txt", &text, &mut a);
    play("walkthrough.txt", &text, &mut b);
    assert_eq!(a.m.state_hash(), b.m.state_hash());
    assert_eq!(a.used, b.used);
}

/// story_data.h is what story.py makes of story/, and lamplight.terse is
/// built from it and lamplight.c as build.sh builds it.
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
            "programs/lamplight/story_data.h is out of date: run programs/lamplight/build.sh"
        );
    }
    common::check_up_to_date(&IMAGE_BYTES, &["lamplight/lamplight.c"], true, "lamplight", RAM_PAGES, &[(2, 0), (3, 0), (4, 0x80), (6, 0x80)]);
}

/// story.py --check finds the mistakes a writer makes, each with its file
/// and line: an unknown object in a rule, a room nothing leads to, an
/// object never placed, a point outside the picture.
#[test]
fn story_check_names_mistakes() {
    let python = std::process::Command::new("python3").arg("--version").output().is_ok_and(|o| o.status.success());
    if !python {
        terse_testkit::skip!("python3", "needs python3 to run story.py");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let story = "game\n  start hall\n\nroom hall \"Hall\"\n  description: A hall.\n  art\n    layer walls\n      line 0 0 700 10\n\n\
                 room attic \"Attic\"\n  description: Dusty.\n\nobject lamp \"lamp\"\n  in nowhere\n\non take lammp\n  say: No.\n";
    std::fs::write(tmp.path().join("bad.story"), story).unwrap();
    let o = std::process::Command::new("python3").arg(dir().join("story.py")).arg("--check").arg(tmp.path()).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{err}");
    for want in [
        "bad.story:8: error: point (700, 10) is outside the picture",
        "bad.story:10: error: room \"attic\" cannot be reached",
        "bad.story:13: error: object \"lamp\" is never placed",
        "bad.story:16: error: unknown object \"lammp\" (did you mean \"lamp\"?)",
    ] {
        assert!(err.contains(want), "missing {want:?} in:\n{err}");
    }
}
