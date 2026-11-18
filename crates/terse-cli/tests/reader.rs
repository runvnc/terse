//! programs/reader/reader.terse: golden screens (programs/reader/expected/;
//! UPDATE_GOLDEN=1 rewrites them) with a store of fixture feeds, and the
//! image is up to date.
//!
//! `timing` prints the article-open latency and the busiest and typical
//! frame (run with `--nocapture`).

mod common;

use terse_core::{Event, FrameInput, FrameResult, Image, Machine, RAM_BASE, Script};
use terse_net::{BlobStore, ContentService, Draft, FeedKey, Identity, Node, Subscriptions, blob_id};

const IMAGE: &[u8] = include_bytes!("../../../programs/reader/reader.terse");
const RAM_PAGES: u8 = 9;
/// 2026-11-17 18:00 UTC: entry times count up from here.
const T0: u32 = 1_794_938_400;

fn new_machine() -> Machine {
    Machine::new(&Image::parse(IMAGE).unwrap())
}

fn alice() -> Identity {
    Identity::from_seed([11; 32])
}

fn bob() -> Identity {
    Identity::from_seed([12; 32])
}

/// About 6 KB: paragraphs, a tab, a word longer than a row, wide characters.
fn long_article() -> String {
    let mut text = String::from("A long article\n\n");
    for p in 1..=20 {
        text.push_str(&format!(
            "Paragraph {p}. The quick brown fox jumps over the lazy dog while the reader wraps each line at a \
             space so that no word is split unless it is longer than the row itself. 日本語の文章も二桁の幅で\
             折り返されます。\tA tab, then more words to fill the paragraph out to a few hundred bytes.\n\n"
        ));
    }
    text.push_str(&"x".repeat(100));
    text.push_str("\nThe end.\n");
    text
}

/// Over 16 KiB, so the reader needs two gets.
fn big_article() -> String {
    (1..=400).map(|i| format!("Line {i:03} of the big article, which is longer than one 16 KiB request.\n")).collect()
}

struct Fixture {
    store: tempfile::TempDir,
    author: Node,
    feeds: Vec<FeedKey>,
    time: u32,
}

impl Fixture {
    fn new() -> Self {
        let store = tempfile::tempdir().unwrap();
        let author = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
        Fixture { store, author, feeds: vec![alice().feed_key(), bob().feed_key()], time: T0 }
    }

    /// Publishes into the store, as `terse feed publish` without a relay does.
    fn publish(&mut self, who: &Identity, title: &str, data: &[u8], reply_to: Option<[u8; 32]>) {
        self.author.set_identity(who.clone());
        self.time += 600;
        let draft = Draft { data: data.to_vec(), title: title.into(), reply_to, time: self.time };
        assert!(matches!(self.author.publish_article(&draft), terse_net::Done::Published { .. }));
    }

    /// Two feeds, eight entries, newest last.
    fn populated() -> Self {
        let mut f = Fixture::new();
        let welcome = b"Welcome to the Terse reader.\n\nThis article is short.\n".to_vec();
        f.publish(&alice(), "Welcome to the Terse reader", &welcome, None);
        f.publish(&bob(), "日本語の記事：テスト", "日本語の本文です。\n二行目。\n".as_bytes(), None);
        f.publish(&alice(), "A long article", long_article().as_bytes(), None);
        f.publish(&bob(), "Re: Welcome to the Terse reader", b"Thanks, this is a reply.\n", Some(blob_id(&welcome)));
        f.publish(&alice(), "An article that is not in the store", b"gone", None);
        std::fs::remove_file(f.store.path().join(terse_net::to_hex(&blob_id(b"gone")))).unwrap();
        f.publish(
            &bob(),
            "A title long enough that the list has to cut it short with an ellipsis, wide characters: 長い題名",
            b"Short.\n",
            None,
        );
        f.publish(&alice(), "A big article (over 16 KiB)", big_article().as_bytes(), None);
        f.publish(&bob(), "Emoji 😀 and tabs\tin a title", b"Text.\n", None);
        f
    }

    fn content(&self) -> ContentService {
        let mut node = Node::new(BlobStore::open(self.store.path(), None).unwrap(), None);
        node.set_subscriptions(Subscriptions::in_memory(self.feeds.iter().copied()));
        ContentService::start(node)
    }
}

struct Run {
    machine: Machine,
    content: ContentService,
    script: Script,
    completions: Vec<terse_core::Completion>,
    unix_time: u32,
    results: Vec<FrameResult>,
    screens: Vec<String>,
}

impl Run {
    fn new(fixture: &Fixture, script: &str) -> Self {
        Run {
            machine: new_machine(),
            content: fixture.content(),
            script: Script::parse(script).unwrap(),
            completions: Vec::new(),
            unix_time: 0,
            results: Vec::new(),
            screens: Vec::new(),
        }
    }

    /// One frame as `terse run` runs it (with `extra` events and this
    /// run's clock).
    fn frame(&mut self, extra: &[Event]) {
        let mut events = self.script.events(self.machine.frame()).to_vec();
        events.extend_from_slice(extra);
        let completions = std::mem::take(&mut self.completions);
        let input = FrameInput { events: &events, completions: &completions, unix_time: self.unix_time, ..Default::default() };
        let r = self.machine.run_frame(&input);
        assert_eq!(r.completions_ignored, 0);
        assert!(self.machine.fault().is_none(), "{:?}", self.machine.fault());
        self.content.submit(self.machine.take_content_requests());
        self.completions = self.content.wait_finished();
        self.results.push(r);
        self.screens.push(self.screen());
    }

    fn frames(&mut self, n: u32) -> &mut Self {
        for _ in 0..n {
            self.frame(&[]);
        }
        self
    }

    fn screen(&self) -> String {
        self.machine.terminal().unwrap().text()
    }
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("reader/expected/{name}.txt"), got);
}

fn row(screen: &str, n: usize) -> &str {
    screen.lines().nth(n).unwrap()
}

#[test]
fn empty_state() {
    let mut f = Fixture::new();
    f.feeds.clear();
    let mut run = Run::new(&f, "");
    run.frames(10);
    let screen = run.screen();
    golden("empty", &screen);
    assert!(screen.contains("terse feed subscribe <feed key>"));
    assert!(row(&screen, 0).contains("0 entries  ·  0 subscriptions"));
}

#[test]
fn list_of_two_feeds() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "");
    run.frames(10);
    let screen = run.screen();
    golden("list", &screen);
    assert!(row(&screen, 0).contains("8 entries  ·  2 subscriptions"));
    assert!(row(&screen, 1).contains("Emoji"));
    assert!(row(&screen, 7).contains("日本語の記事：テスト"));
    let reply = row(&screen, 5);
    assert!(reply.contains("✓↳ Re: Welcome"), "{reply}");
    assert!(row(&screen, 4).contains("   An article that is not in the store"), "not local, no marks");
    assert!(row(&screen, 3).ends_with("…"), "cut with an ellipsis: {}", row(&screen, 3));
    let key = terse_net::to_hex(&alice().feed_key());
    assert!(row(&screen, 2).contains(&key[..8]));
    let t = run.machine.terminal().unwrap();
    assert_ne!(t.cell(40, 1).bg, t.cell(40, 2).bg);
}

/// The relative age, when the host gives the time.
#[test]
fn list_with_a_clock() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "");
    run.unix_time = T0 + 8 * 600 + 7200 + 59;
    run.frames(10);
    let screen = run.screen();
    assert!(row(&screen, 1).starts_with("      2h ago "), "{}", row(&screen, 1));
    run.unix_time = T0 + 600 + 3 * 86400;
    run.machine = new_machine();
    run.frames(10);
    assert!(row(&run.screen(), 8).starts_with("      3d ago "), "{}", row(&run.screen(), 8));
}

#[test]
fn open_scroll_and_return() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "12 press down\n13 press down\n14 click 300 104\n16 press enter\n");
    // Enter in frame 16; the golden is the screen once the article is
    // wholly wrapped (the status bar shows the line count without "+").
    run.frames(17);
    while !row(&run.screen(), 29).contains('%') {
        run.frames(1);
    }
    let wrapped = run.machine.frame();
    assert!(wrapped <= 20, "wrapped after frame {wrapped}");
    let opened = run.screen();
    golden("article", &opened);
    run.frames(30 - wrapped);
    assert_eq!(run.screen(), opened);
    assert!(row(&opened, 0).contains("A long article"));
    assert!(row(&opened, 29).starts_with(" lines 1–27 of 105  25%"), "{}", row(&opened, 29));
    run.script = Script::parse("30 press pagedown\n32 wheel 1\n").unwrap();
    run.frames(10);
    let scrolled = run.screen();
    golden("article-scrolled", &scrolled);
    assert!(row(&scrolled, 29).contains("lines 30–56 of"), "{}", row(&scrolled, 29));
    assert!(opened.lines().chain(scrolled.lines()).all(|l| l.chars().count() <= 80));
    run.script = Script::parse("40 press esc\n").unwrap();
    run.frames(8);
    let back = run.screen();
    assert!(row(&back, 6).contains("A long article"));
    let t = run.machine.terminal().unwrap();
    assert_ne!(t.cell(40, 6).bg, t.cell(40, 5).bg, "selected");
    run.script = Script::parse("48 press esc\n").unwrap();
    run.frames(2);
    assert_eq!(run.machine.exit_code(), Some(0));
}

#[test]
fn article_over_16_kib() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "12 press down\n14 press enter\n20 press end\n");
    run.frames(60);
    let screen = run.screen();
    golden("article-big", &screen);
    assert!(row(&screen, 1).contains(&format!("{} bytes", big_article().len())));
    assert!(row(&screen, 28).contains("Line 400 of the big article"), "{screen}");
    assert!(row(&screen, 29).contains("of 400  100%"), "{}", row(&screen, 29));
}

#[test]
fn article_not_found() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "12 click 300 72\n13 click 300 72\n");
    run.frames(20);
    let screen = run.screen();
    golden("not-found", &screen);
    assert!(screen.contains("Not found: neither this host nor its relay has the article."));
}

/// A feeds-updated refresh keeps the selection on its entry and marks the
/// new one; opening it clears the mark.
#[test]
fn refresh_keeps_the_selection() {
    let mut f = Fixture::populated();
    let mut run = Run::new(&f, "12 press down\n13 press down\n");
    run.frames(20);
    assert!(row(&run.screen(), 3).contains("A title long enough"));
    f.publish(&bob(), "Fresh news", b"Just in.\n", None);
    run.frame(&[Event::feeds_updated()]);
    run.frames(10);
    let screen = run.screen();
    golden("refreshed", &screen);
    assert!(row(&screen, 0).contains("9 entries"));
    assert!(row(&screen, 1).starts_with("•") && row(&screen, 1).contains("Fresh news"), "{}", row(&screen, 1));
    let t = run.machine.terminal().unwrap();
    assert!(row(&screen, 4).contains("A title long enough"));
    assert_ne!(t.cell(40, 4).bg, t.cell(40, 3).bg, "the selection moved down with its entry");
    run.script = Script::parse(&format!("{} press home\n{} press enter\n{} press esc\n", 31, 32, 36)).unwrap();
    run.frames(14);
    assert!(row(&run.screen(), 1).starts_with(" ") && row(&run.screen(), 1).contains("Fresh news"));
}

/// Measurements: frames to draw the list, Enter to the article's first
/// page on screen and to its last line wrapped, the busiest and typical
/// frame, and the deepest stack.
#[test]
fn timing() {
    let f = Fixture::populated();
    let script = "40 press down\n41 press down\n42 press down\n43 press down\n44 press down\n50 press enter\n\
                  90 press pagedown\n95 wheel 2\n100 press end\n110 press esc\n120 press home\n121 press down\n\
                  123 press enter\n160 press end\n170 press esc\n";
    let mut run = Run::new(&f, script);
    let start = std::time::Instant::now();
    run.frames(200);
    let host = start.elapsed();
    // screens[i] is the screen after frame i (frames count from 0). The
    // first i from which the rows stay as they are until `to`: with the
    // status bar, or without it.
    let settled = |from: usize, to: usize, status: bool| {
        let rows = |s: &str| s.lines().take(if status { 30 } else { 29 }).collect::<Vec<_>>().join("\n");
        let last = rows(&run.screens[to]);
        (from..=to).find(|&i| run.screens[i..=to].iter().all(|s| rows(s) == last)).unwrap()
    };
    let list_done = settled(0, 39, true);
    let units: u64 = run.results[..=list_done].iter().map(|r| r.used).sum();
    let spilled = run.results.iter().filter(|r| !r.waited).count();
    let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
    let stats = common::FrameStats::new(&used);
    let ram_end = RAM_BASE + run.machine.ram().len() as u32;
    let min_sp = run.machine.cpu().min_sp;
    // Frames from the one Enter arrives in to the one after which the
    // article's first page is on screen, counting both (1: at once).
    let open = settled(50, 89, false) - 50 + 1;
    eprintln!(
        "reader: list of 8 entries drawn in {} frames ({units} units from reset)\n\
         reader: article-open latency (Enter to the ~6 KB article's first page on screen): {open} frames; \
         fully wrapped: {} frames\n\
         reader: Enter to the >16 KiB article's first page: {} frames, fully wrapped: {} frames\n\
         reader: {}; frames not ending in wait_frame: {spilled}\n\
         reader: deepest stack {} bytes; host {:.1} µs per frame (machine and content service)",
        list_done + 1,
        settled(50, 89, true) - 50 + 1,
        settled(123, 159, false) - 123 + 1,
        settled(123, 159, true) - 123 + 1,
        stats.describe(),
        ram_end - min_sp,
        host.as_secs_f64() * 1e6 / 200.0,
    );
    assert_eq!(spilled, 0, "a frame's work ran into the next frame");
}

/// The costliest text: wide characters only, wrapped and redrawn page by
/// page. Measured, not limited: a frame that runs out of budget simply
/// continues in the next.
#[test]
fn wide_text_frames() {
    let mut f = Fixture::new();
    let text: String = (0..80).map(|i| format!("{}\n", "日本語の文章も二桁の幅で折り返されます。".repeat(1 + i % 4))).collect();
    f.publish(&alice(), "日本語".repeat(13).as_str(), text.as_bytes(), None);
    let script = "14 press enter\n40 press pagedown\n41 press pagedown\n42 press end\n50 press home\n51 wheel 5\n";
    let mut run = Run::new(&f, script);
    run.frames(70);
    let worst = run.results.iter().map(|r| r.used).max().unwrap();
    let spilled = run.results.iter().filter(|r| !r.waited).count();
    let busy: Vec<(usize, u64)> =
        run.results.iter().map(|r| r.used).enumerate().filter(|&(_, t)| t > 20_000).collect();
    eprintln!(
        "reader: wide text, busiest frame {worst} units ({:.2}%); frames not ending in wait_frame: {spilled}; \
         frames over 20,000 units: {busy:?}",
        common::pct(worst)
    );
    assert!(row(&run.screen(), 29).contains("lines 16–42 of"), "{}", run.screen());
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(IMAGE, &["reader/reader.c"], true, "reader", RAM_PAGES, &[(2, 0), (4, 0)]);
}

