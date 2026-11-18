//! Golden tests for programs/reader/reader.terse: a host with a blob store
//! holding two feeds (no relay), the content device served by the real
//! `ContentService`, and scripted events. Every request completes at the
//! start of the next frame, as in `terse run`. Set UPDATE_GOLDEN=1 to
//! rewrite the expected screens in programs/reader/expected/.
//!
//! `timing` prints the measurements quoted in the README (run with
//! `--nocapture`).

use std::path::{Path, PathBuf};

use terse_core::{Event, FrameInput, FrameResult, Image, Machine, Script, TSTATES_PER_FRAME};
use terse_net::{BlobStore, ContentService, Draft, FeedKey, Identity, Node, Subscriptions, blob_id};

const IMAGE: &[u8] = include_bytes!("../../../programs/reader/reader.terse");
/// 2026-11-17 18:00 UTC: entry times count up from here.
const T0: u32 = 1_794_938_400;

fn expected_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/reader/expected")
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
            machine: Machine::new(&Image::parse(IMAGE).unwrap()),
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
    let path = expected_dir().join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(expected_dir()).unwrap();
        std::fs::write(&path, got).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} (run with UPDATE_GOLDEN=1)", path.display()));
    assert_eq!(got, want, "screen differs from {}", path.display());
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
    // Newest first; the selection is on the first entry.
    assert!(row(&screen, 1).contains("Emoji"));
    assert!(row(&screen, 7).contains("日本語の記事：テスト"));
    let reply = row(&screen, 5);
    assert!(reply.contains("✓↳ Re: Welcome"), "{reply}");
    assert!(row(&screen, 4).contains("   An article that is not in the store"), "not local, no marks");
    assert!(row(&screen, 3).ends_with("…"), "cut with an ellipsis: {}", row(&screen, 3));
    let key = terse_net::to_hex(&alice().feed_key());
    assert!(row(&screen, 2).contains(&key[..8]));
    // Selection bar: row 1 has the selected background.
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
    run.machine = Machine::new(&Image::parse(IMAGE).unwrap());
    run.frames(10);
    assert!(row(&run.screen(), 8).starts_with("      3d ago "), "{}", row(&run.screen(), 8));
}

#[test]
fn open_scroll_and_return() {
    let f = Fixture::populated();
    // Select "A long article" (index 5) with Down and a click, open it.
    let mut run = Run::new(&f, "12 press down\n13 press down\n14 click 300 104\n16 press enter\n");
    run.frames(30);
    let opened = run.screen();
    golden("article", &opened);
    assert!(row(&opened, 0).contains("A long article"));
    assert!(row(&opened, 29).contains("lines 1–27 of"));
    // Scrolling: a page, then three lines by the wheel.
    run.script = Script::parse("30 press pagedown\n32 wheel 1\n").unwrap();
    run.frames(10);
    let scrolled = run.screen();
    golden("article-scrolled", &scrolled);
    assert!(row(&scrolled, 29).contains("lines 30–56 of"), "{}", row(&scrolled, 29));
    // Every text row fits: no row is longer than 80 columns, and the long word is split.
    assert!(opened.lines().chain(scrolled.lines()).all(|l| l.chars().count() <= 80));
    // Back to the list, with the same selection.
    run.script = Script::parse("40 press esc\n").unwrap();
    run.frames(8);
    let back = run.screen();
    assert!(row(&back, 6).contains("A long article"));
    let t = run.machine.terminal().unwrap();
    assert_ne!(t.cell(40, 6).bg, t.cell(40, 5).bg, "selected");
    // Esc in the list exits.
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
    // Opening the new entry clears its mark.
    run.script = Script::parse(&format!("{} press home\n{} press enter\n{} press esc\n", 31, 32, 36)).unwrap();
    run.frames(14);
    assert!(row(&run.screen(), 1).starts_with(" ") && row(&run.screen(), 1).contains("Fresh news"));
}

/// Measurements for the README: frames and T-states to draw the list, the
/// busiest frame of a session, and Enter to a local article on screen.
#[test]
fn timing() {
    let f = Fixture::populated();
    // Open "A long article" (index 5), page through it, go back, open the
    // big one (index 1), go to its end, go back.
    let script = "40 press down\n41 press down\n42 press down\n43 press down\n44 press down\n50 press enter\n\
                  90 press pagedown\n95 wheel 2\n100 press end\n110 press esc\n120 press home\n121 press down\n\
                  123 press enter\n160 press end\n170 press esc\n";
    let mut run = Run::new(&f, script);
    run.frames(200);
    // The first frame from which rows 0-28 (or, with `status`, all rows)
    // stay as they are at frame `to`.
    let settled = |from: usize, to: usize, status: bool| {
        let rows = |s: &str| s.lines().take(if status { 30 } else { 29 }).collect::<Vec<_>>().join("\n");
        let last = rows(&run.screens[to]);
        (from..=to).find(|&i| run.screens[i..=to].iter().all(|s| rows(s) == last)).unwrap()
    };
    let list_done = settled(0, 39, true);
    let tstates: u64 = run.results[..=list_done].iter().map(|r| r.tstates).sum();
    let worst = run.results.iter().enumerate().max_by_key(|(_, r)| r.tstates).unwrap();
    let spilled = run.results.iter().filter(|r| !r.halted_early).count();
    eprintln!(
        "reader: list of 8 entries drawn by frame {list_done} ({tstates} T-states from reset)\n\
         reader: Enter (frame 50) to the ~6 KB article's first page on screen: {} frames, fully wrapped: {} frames\n\
         reader: Enter (frame 123) to the >16 KiB article's first page: {} frames, fully wrapped: {} frames\n\
         reader: busiest frame {} with {} T-states ({:.0}% of {TSTATES_PER_FRAME}); frames not ending in HALT: {spilled}",
        settled(50, 89, false) - 50 + 1,
        settled(50, 89, true) - 50 + 1,
        settled(123, 159, false) - 123 + 1,
        settled(123, 159, true) - 123 + 1,
        worst.0,
        worst.1.tstates,
        worst.1.tstates as f64 * 100.0 / TSTATES_PER_FRAME as f64,
    );
    // The stack grows down from FFFFh into zeroed RAM: its deepest point is
    // where the bytes below are untouched (a run of 256 zeros).
    let ram = &run.machine.ram()[..0x8000];
    let mut low = ram.len();
    while low > 256 && ram[low - 256..low].iter().any(|&b| b != 0) {
        low -= 1;
    }
    let deepest = (low..ram.len()).find(|&i| ram[i] != 0).unwrap();
    eprintln!("reader: stack reached {:04X}h ({} bytes)", 0x8000 + deepest, ram.len() - deepest);
    assert_eq!(spilled, 0, "a frame's work ran into the next frame");
}

/// The costliest text: wide characters only, wrapped and redrawn page by
/// page. No frame's work may spill into the next.
#[test]
fn wide_text_stays_within_each_frame() {
    let mut f = Fixture::new();
    let text: String = (0..80).map(|i| format!("{}\n", "日本語の文章も二桁の幅で折り返されます。".repeat(1 + i % 4))).collect();
    f.publish(&alice(), "日本語".repeat(13).as_str(), text.as_bytes(), None);
    let script = "14 press enter\n40 press pagedown\n41 press pagedown\n42 press end\n50 press home\n51 wheel 5\n";
    let mut run = Run::new(&f, script);
    run.frames(70);
    let worst = run.results.iter().map(|r| r.tstates).max().unwrap();
    eprintln!("reader: wide text, busiest frame {worst} T-states");
    assert!(run.results.iter().all(|r| r.halted_early), "{:?}", run.results.iter().map(|r| r.tstates).collect::<Vec<_>>());
    assert!(row(&run.screen(), 29).contains("lines 16–42 of"), "{}", run.screen());
}

