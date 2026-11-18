//! programs/reader/reader.terse: golden views (programs/reader/expected/:
//! the terminal, then the page as the reference layout shows it, as
//! `terse run --dump --dump-page` prints them; UPDATE_GOLDEN=1 rewrites
//! them) with a store of fixture feeds, and the image is up to date.
//!
//! `timing` prints the article-open latency and the busiest and typical
//! frame (run with `--nocapture`).

mod common;

use terse_core::page::reference_layout;
use terse_core::{Event, FrameInput, FrameResult, Image, Machine, PageInput, RAM_BASE, Script};
use terse_net::{BlobStore, ContentService, Draft, FeedKey, Identity, Node, Subscriptions, blob_id};

const IMAGE: &[u8] = include_bytes!("../../../programs/reader/reader.terse");
const RAM_PAGES: u8 = 17;
/// 2026-11-17 18:00 UTC: entry times count up from here.
const T0: u32 = 1_794_938_400;
/// The page's rectangle (reader.c).
const RECT: (u16, u16, u16, u16) = (64, 16, 512, 448);
/// Up and Down move this far (reader.c LINE).
const LINE: u32 = 20;

fn new_machine() -> Machine {
    Machine::new(&Image::parse(IMAGE).unwrap())
}

fn alice() -> Identity {
    Identity::from_seed([11; 32])
}

fn bob() -> Identity {
    Identity::from_seed([12; 32])
}

/// A short article as people write them: paragraphs wrapped by hand, a
/// preformatted table (indented lines) and bare links, one in parentheses.
pub const SOURDOUGH_TITLE: &str = "Keeping a sourdough starter alive";
pub const SOURDOUGH: &str = "\
A sourdough starter is nothing more than flour and water that has been
colonised by wild yeast and lactic acid bacteria. Left on a kitchen counter,
the mixture ferments, rises and falls each day, and in return for regular
feeding it will leaven bread for decades. Some bakeries keep starters that
are older than the buildings they work in.

The routine is simple once it settles into a rhythm. Discard most of the
starter, keep a small amount, and feed it with fresh flour and water in equal
weights. Within four to eight hours it should double in volume and smell
pleasantly sour, like yoghurt or green apples. That peak is the moment to
bake with it.

A typical week at room temperature looks like this:

    Day   Keep   Flour   Water   Notes
    Mon   20 g    50 g    50 g   rises in about six hours
    Wed   20 g    50 g    50 g   bake on Thursday morning
    Fri   20 g   100 g   100 g   a double feed for the weekend
    Sun   rests in the fridge until Monday

Temperature matters more than almost anything else. A warm kitchen speeds
fermentation up and makes the starter hungry; a cool one slows it down, so
that a single feeding can last a day or more. A starter kept in the
refrigerator survives a few weeks without attention and usually recovers
after two or three feedings.

The choice of flour changes the character of the culture. Whole wheat and rye
carry more of the wild yeast and the nutrients it feeds on, so a starter begun
with them tends to wake up sooner and smell more strongly. White flour gives a
milder, more predictable starter; many bakers begin with rye and move to white
once the jar rises on schedule.

Nothing has to go to waste. The discard that is poured off before each feeding
still holds plenty of flavour, and it can go into pancakes, crackers, waffles
or pizza dough, where it adds a gentle tang without needing to leaven
anything on its own.

When something goes wrong it is rarely fatal. A grey liquid on top, often
called hooch, only means the starter is hungry: pour it off and feed it. Pink
or orange streaks are another matter, and a sign to start again. The history
of the method, from ancient Egypt to the San Francisco gold rush, is told at
https://en.wikipedia.org/wiki/Sourdough and the microbes themselves are
described in detail (https://en.wikipedia.org/wiki/Lactobacillus).

Most of all, be patient. A new starter can take ten days before it rises
reliably, and the bread it makes in its first month is rarely its best. The
flavour deepens as the culture settles, which is one more reason people keep
the same jar going for years.
";

/// Paragraphs in English, Japanese and Arabic, and one that mixes them.
pub const TEA_TITLE: &str = "Tea in three languages: お茶, الشاي";
pub const TEA: &str = "\
Tea is drunk almost everywhere, but every culture has made it its own. This
page describes the same simple ritual in three languages, each paragraph laid
out in its own direction.

日本では、緑茶は毎日の生活に欠かせない飲み物です。朝の一杯は心を落ち着かせ、一日の始まりを静かに告げてくれます。急須でいれたお茶は、ペットボトルのお茶とはまったく違う香りがします。

الشاي جزء أساسي من الحياة اليومية في كثير من البلدان العربية. يُقدَّم للضيوف ترحيبًا بهم، وغالبًا ما يُحضَّر بالنعناع الطازج أو الهيل. وفي المساء تجتمع العائلة حول إبريق الشاي لتتحدث عن أحداث اليوم.

In Japanese the word for tea is お茶 (ocha); in Arabic it is شاي (shay). Both
words travelled along the old trade routes from China, where the plant was
first grown.

茶道では、一つひとつの動作に意味があります。お湯を注ぐ音さえも、もてなしの一部なのです。

وفي المغرب يُصبّ الشاي من ارتفاع عالٍ ليتكوّن على سطحه زبد خفيف.
";

/// About 6 KB: paragraphs, a tab, a word longer than a line, wide characters.
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

/// Over 48 KiB (800 paragraphs of 66 bytes): the reader needs four gets,
/// holds the first 48 KiB and says so at the end.
fn big_article() -> String {
    (1..=800).map(big_line).collect()
}

fn big_line(i: usize) -> String {
    format!("Line {i:03} of the big article, which needs more than one request.\n\n")
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

    /// Two feeds, ten entries, newest last; in the list (newest first):
    /// 1 Emoji, 2 big, 3 long title, 4 not in the store, 5 reply, 6 long,
    /// 7 Japanese, 8 Welcome, 9 tea, 10 sourdough.
    fn populated() -> Self {
        let mut f = Fixture::new();
        f.publish(&alice(), SOURDOUGH_TITLE, SOURDOUGH.as_bytes(), None);
        f.publish(&bob(), TEA_TITLE, TEA.as_bytes(), None);
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
        f.publish(&alice(), "A big article (over 48 KiB)", big_article().as_bytes(), None);
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
    page: PageInput,
    completions: Vec<terse_core::Completion>,
    unix_time: u32,
    results: Vec<FrameResult>,
    screens: Vec<String>,
    /// (frame, height) of every height event delivered.
    heights: Vec<(u32, u32)>,
    /// The frames that ended with a newly submitted document.
    submits: Vec<u32>,
}

impl Run {
    fn new(fixture: &Fixture, script: &str) -> Self {
        Run {
            machine: new_machine(),
            content: fixture.content(),
            script: Script::parse(script).unwrap(),
            page: PageInput::default(),
            completions: Vec::new(),
            unix_time: 0,
            results: Vec::new(),
            screens: Vec::new(),
            heights: Vec::new(),
            submits: Vec::new(),
        }
    }

    /// One frame as `terse run` runs it (with `extra` events and this
    /// run's clock): the page events of the reference layout first.
    fn frame(&mut self, extra: &[Event]) {
        let frame = self.machine.frame();
        let mut events = self.script.events(frame).to_vec();
        events.extend_from_slice(extra);
        let events = self.page.events(self.machine.page(), &events);
        for e in &events {
            if e.kind == Event::PAGE_LAID_OUT {
                self.heights.push((frame, e.a as u32 | (e.b as u32) << 8 | (e.c as u32) << 16));
            }
        }
        let generation = self.machine.page().unwrap().generation();
        let completions = std::mem::take(&mut self.completions);
        let input = FrameInput { events: &events, completions: &completions, unix_time: self.unix_time, ..Default::default() };
        let r = self.machine.run_frame(&input);
        assert_eq!(r.completions_ignored, 0);
        assert!(self.machine.fault().is_none(), "{:?}", self.machine.fault());
        self.page.after_frame(self.machine.page());
        let page = self.machine.page().unwrap();
        if page.generation() != generation && page.document().is_some() {
            self.submits.push(frame);
        }
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

    /// Runs frames until frame `f` has run.
    fn until(&mut self, f: u32) -> &mut Self {
        while self.machine.frame() <= f {
            self.frame(&[]);
        }
        self
    }

    fn screen(&self) -> String {
        self.machine.terminal().unwrap().text()
    }

    fn layout(&self) -> terse_core::Layout {
        let page = self.machine.page().unwrap();
        page.document().map(|d| reference_layout(d, page.region().w)).unwrap_or_default()
    }

    /// The terminal, then the page (`terse run --dump --dump-page`).
    fn view(&self) -> String {
        format!("{}{}", self.screen(), self.layout().dump(self.machine.page().unwrap()))
    }

    /// The text of every line of the page, anchors as `[text]{id}`.
    fn page_text(&self) -> String {
        let mut out = String::new();
        for line in &self.layout().lines {
            for span in &line.spans {
                match span.style.anchor {
                    0 => out.push_str(&span.text),
                    id => out.push_str(&format!("[{}]{{{id}}}", span.text)),
                }
            }
            out.push('\n');
        }
        out
    }

    /// The scroll value as the program stored it.
    fn scroll(&self) -> i32 {
        self.machine.page().unwrap().scroll() as i32
    }

    fn status(&self) -> String {
        row(&self.screen(), 29).to_string()
    }

    fn press(&mut self, keys: &str) {
        let f = self.machine.frame();
        self.script = Script::parse(&format!("{f} press {keys}\n")).unwrap();
        self.frames(2);
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
    golden("empty", &run.view());
    assert!(screen.contains("terse feed subscribe <feed key>"));
    assert!(row(&screen, 0).contains("0 entries  ·  0 subscriptions"));
}

#[test]
fn list_of_two_feeds() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "");
    run.frames(10);
    let screen = run.screen();
    golden("list", &run.view());
    assert!(row(&screen, 0).contains("10 entries  ·  2 subscriptions"));
    assert!(row(&screen, 1).contains("Emoji"));
    assert!(row(&screen, 7).contains("日本語の記事：テスト"));
    let reply = row(&screen, 5);
    assert!(reply.contains("✓↳ Re: Welcome"), "{reply}");
    assert!(row(&screen, 4).contains("   An article that is not in the store"), "not local, no marks");
    assert!(row(&screen, 3).ends_with("…"), "cut with an ellipsis: {}", row(&screen, 3));
    assert!(row(&screen, 10).contains(SOURDOUGH_TITLE));
    let key = terse_net::to_hex(&alice().feed_key());
    assert!(row(&screen, 2).contains(&key[..8]));
    let t = run.machine.terminal().unwrap();
    assert_ne!(t.cell(40, 1).bg, t.cell(40, 2).bg);
    assert!(!run.machine.page().unwrap().region().visible(), "no page in the list");
}

/// The relative age, when the host gives the time.
#[test]
fn list_with_a_clock() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "");
    run.unix_time = T0 + 10 * 600 + 7200 + 59;
    run.frames(10);
    let screen = run.screen();
    assert!(row(&screen, 1).starts_with("      2h ago "), "{}", row(&screen, 1));
    run.unix_time = T0 + 600 + 3 * 86400;
    run.machine = new_machine();
    run.page = PageInput::default();
    run.frames(10);
    assert!(row(&run.screen(), 10).starts_with("      3d ago "), "{}", row(&run.screen(), 10));
}

/// The sourdough article: opened, scrolled every way, a link pressed, and
/// back to the list.
#[test]
fn open_scroll_and_return() {
    let f = Fixture::populated();
    // End selects the last entry (the sourdough article); Enter in frame 14.
    let mut run = Run::new(&f, "12 press end\n14 press enter\n");
    run.until(14);
    assert_eq!(run.submits, [] as [u32; 0]);
    run.until(15);
    assert_eq!(run.submits, [15], "the whole article is in the document at once");
    assert_eq!(run.status().trim_start().chars().next(), Some('↑'), "no percentage before the height: {}", run.status());
    run.until(16);
    let height = run.layout().height;
    assert_eq!(run.heights, [(16, height)]);
    let opened = run.view();
    golden("article", &opened);
    assert!(row(&opened, 0).contains(SOURDOUGH_TITLE));
    assert!(opened.contains("sans26b |Keeping a sourdough starter alive"));
    assert!(opened.contains("-- rule --"));
    assert!(opened.contains("mono13 |    Day   Keep   Flour   Water   Notes"), "preformatted");
    assert!(opened.contains("colonised by wild yeast"));
    let pct = |run: &Run| -> u32 {
        let s = run.status();
        s.split('%').next().unwrap().trim().parse().unwrap_or_else(|_| panic!("{s}"))
    };
    let max = height as i32 - RECT.3 as i32;
    assert!(max > 0, "taller than the rectangle: {height}");
    assert_eq!(pct(&run), (RECT.3 as u32) * 100 / height);
    // Rows 1-28 are blank under the page.
    assert!(opened.lines().skip(1).take(28).all(|l| l.trim().is_empty()));

    run.press("down");
    assert_eq!(run.scroll(), LINE as i32);
    assert_eq!(pct(&run), (LINE + RECT.3 as u32) * 100 / height);
    run.press("pagedown");
    assert_eq!(run.scroll(), (LINE + RECT.3 as u32 - LINE) as i32);
    let f = run.machine.frame();
    run.script = Script::parse(&format!("{f} wheel 1\n")).unwrap();
    run.frames(2);
    assert_eq!(run.scroll(), (RECT.3 as u32 + 3 * LINE) as i32);
    golden("article-scrolled", &run.view());
    run.press("end");
    assert_eq!(run.scroll(), max);
    assert_eq!(pct(&run), 100);
    assert!(run.view().contains("the same jar going for years."));
    run.press("down");
    assert_eq!(run.scroll(), max, "clamped at the end");
    let f = run.machine.frame();
    run.script = Script::parse(&format!("{f} wheel 5\n")).unwrap();
    run.frames(2);
    assert_eq!(run.scroll(), max, "clamped at the end");
    run.press("pageup");
    assert_eq!(run.scroll(), max - (RECT.3 as i32 - LINE as i32));
    run.press("home");
    assert_eq!(run.scroll(), 0);
    run.press("up");
    assert_eq!(run.scroll(), 0, "clamped at the top");
    assert_eq!(pct(&run), (RECT.3 as u32) * 100 / height);

    // Press the first link, where the reference layout puts it.
    run.press("pagedown");
    let layout = run.layout();
    let scroll = run.machine.page().unwrap().scroll_for(layout.height);
    let (line, span) = layout
        .lines
        .iter()
        .find_map(|l| l.spans.iter().find(|s| s.style.anchor == 1).map(|s| (l, s)))
        .expect("an anchored link");
    assert_eq!(span.text, "https://en.wikipedia.org/wiki/Sourdough");
    assert!(line.y >= scroll && line.y + line.height <= scroll + RECT.3 as u32, "in view");
    let (x, y) = (RECT.0 as u32 + span.x0 / 2 + 8, RECT.1 as u32 + line.y - scroll + 4);
    let f = run.machine.frame();
    run.script = Script::parse(&format!("{f} click {x} {y}\n")).unwrap();
    run.frames(2);
    let linked = run.view();
    golden("article-link", &linked);
    assert_eq!(row(&linked, 29).trim_end(), " Link: https://en.wikipedia.org/wiki/Sourdough");
    // The second link: the parenthesis and full stop are not part of it.
    assert!(run.page_text().contains("([https://en.wikipedia.org/wiki/Lactobacillus]{2}).\n"), "{}", run.page_text());
    run.press("down");
    assert!(run.status().contains('%'), "a key puts the percentage back");

    run.press("esc");
    let back = run.view();
    assert!(row(&back, 10).contains(SOURDOUGH_TITLE));
    let t = run.machine.terminal().unwrap();
    assert_ne!(t.cell(40, 10).bg, t.cell(40, 9).bg, "selected");
    let page = run.machine.page().unwrap();
    assert!(page.document().is_none() && !page.region().visible(), "the page is cleared and hidden");
    run.press("esc");
    assert_eq!(run.machine.exit_code(), Some(0));
}

/// English, Japanese and Arabic paragraphs (laid out left to right by the
/// reference layout; a real host follows each paragraph's direction).
#[test]
fn mixed_languages() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "12 press end\n13 press up\n14 press enter\n");
    run.frames(20);
    let view = run.view();
    golden("article-tea", &view);
    assert!(row(&view, 0).contains("Tea in three languages"));
    assert!(view.contains("日本では、緑茶は毎日の生活に欠かせない飲み物です。"));
    assert!(view.contains("الشاي جزء أساسي"));
    assert!(row(&view, 29).starts_with(" 100%"), "it fits: {}", row(&view, 29));
}

#[test]
fn article_over_48_kib() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "12 press down\n14 press enter\n20 press end\n");
    run.frames(60);
    let view = run.view();
    golden("article-big", &view);
    let size = big_article().len();
    assert!(size > 49152);
    assert!(view.contains(&format!("{size} bytes")));
    // The document ends where the 48 KiB do, in a paragraph.
    let per = big_line(1).len();
    assert!(view.contains(&format!("[… the article continues: this reader shows 49152 of its {size} bytes]")), "{view}");
    assert!(view.contains(&format!("|{}\n", big_line(49152 / per).trim_end())), "{view}");
    assert!(view.contains(&format!("|{}\n", &big_line(49152 / per + 1)[..49152 % per])), "{view}");
    assert!(row(&view, 29).starts_with(" 100%"), "{}", row(&view, 29));
    assert_eq!(run.submits.len(), 2, "a part at once, then the whole: {:?}", run.submits);
    let max = run.layout().height - RECT.3 as u32;
    assert_eq!(run.scroll(), max as i32, "End before the whole document was laid out still reaches its end");
}

/// The costliest text to turn into a document: more links than the
/// reader marks, colons that are almost links, blank lines, and paragraphs
/// of one letter, so many that the formatting does not fit in 64 KiB. Cut
/// cleanly, built over many frames, none of them out of budget.
#[test]
fn document_full() {
    let mut f = Fixture::new();
    let mut text = String::new();
    for i in 0..600 {
        text.push_str(&format!("http://a.example/{i} "));
    }
    text.push_str(&"http:".repeat(1000));
    text.push_str(&":".repeat(4000));
    text.push_str(&"\n".repeat(2000));
    text.push_str(&"  \t\n".repeat(1000));
    text.push_str(&"a\n\n".repeat(6000));
    f.publish(&alice(), "Very many paragraphs", text.as_bytes(), None);
    let mut run = Run::new(&f, "12 press enter\n30 press end\n");
    run.frames(100);
    let page = run.machine.page().unwrap();
    let doc = page.document().unwrap();
    assert!(doc.len() <= 65536 && doc.len() > 65400, "{}", doc.len());
    let view = run.view();
    assert!(view.contains("[… the article continues: this reader shows "), "{view}");
    let text = run.page_text();
    assert!(text.contains("[http://a.example/511]{512}") && !text.contains("{513}"), "512 links marked: {text}");
    assert!(text.contains("http://a.example/599"), "{text}");
    assert!(row(&view, 29).starts_with(" 100%"), "{}", row(&view, 29));
    let worst = run.results.iter().map(|r| r.used).max().unwrap();
    let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
    assert!(run.results.iter().all(|r| r.waited), "a frame ran out of budget ({worst} units): {used:?}");
    eprintln!(
        "reader: a full document of the costliest text: built in {} frames, busiest frame {worst} units",
        run.submits.last().unwrap() - 12
    );
}

#[test]
fn article_not_found() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "12 click 300 72\n13 click 300 72\n");
    run.frames(20);
    let view = run.view();
    golden("not-found", &view);
    assert!(view.contains("Not found: neither this host nor its relay has the article."));
    assert!(run.machine.page().unwrap().document().is_none());
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
    golden("refreshed", &run.view());
    assert!(row(&screen, 0).contains("11 entries"));
    assert!(row(&screen, 1).starts_with("•") && row(&screen, 1).contains("Fresh news"), "{}", row(&screen, 1));
    let t = run.machine.terminal().unwrap();
    assert!(row(&screen, 4).contains("A title long enough"));
    assert_ne!(t.cell(40, 4).bg, t.cell(40, 3).bg, "the selection moved down with its entry");
    run.script = Script::parse(&format!("{} press home\n{} press enter\n{} press esc\n", 31, 32, 36)).unwrap();
    run.frames(14);
    assert!(row(&run.screen(), 1).starts_with(" ") && row(&run.screen(), 1).contains("Fresh news"));
}

/// Measurements: frames to draw the list, Enter to the document submitted,
/// to its first height event and to the height of the whole document, the
/// busiest and typical frame, and the deepest stack.
#[test]
fn timing() {
    let f = Fixture::populated();
    let script = "40 press down\n41 press down\n42 press down\n43 press down\n44 press down\n50 press enter\n\
                  90 press pagedown\n95 wheel 2\n100 press end\n110 press esc\n120 press home\n121 press down\n\
                  123 press enter\n160 press end\n170 press esc\n180 press end\n182 press enter\n190 press esc\n";
    let mut run = Run::new(&f, script);
    let start = std::time::Instant::now();
    run.frames(200);
    let host = start.elapsed();
    // screens[i] is the screen after frame i (frames count from 0). The
    // first i from which the rows stay as they are until `to`.
    let settled = |from: usize, to: usize| {
        let last = &run.screens[to];
        (from..=to).find(|&i| run.screens[i..=to].iter().all(|s| s == last)).unwrap()
    };
    let list_done = settled(0, 39);
    let units: u64 = run.results[..=list_done].iter().map(|r| r.used).sum();
    let spilled = run.results.iter().filter(|r| !r.waited).count();
    let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
    let stats = common::FrameStats::new(&used);
    let ram_end = RAM_BASE + run.machine.ram().len() as u32;
    let min_sp = run.machine.cpu().min_sp;
    // Frames from the one Enter arrives in to the one after which the
    // document is submitted, the first height event arrives and the whole
    // document's height arrives, counting both (1: at once).
    let open = |enter: u32, until: u32| {
        let submits: Vec<u32> = run.submits.iter().copied().filter(|&f| f >= enter && f < until).collect();
        let heights: Vec<u32> = run.heights.iter().map(|&(f, _)| f).filter(|&f| f > enter && f < until).collect();
        let n = |f: u32| f - enter + 1;
        format!(
            "submitted after {} frames ({} submits), first height in frame {}, whole document's height in frame {}",
            n(submits[0]),
            submits.len(),
            n(heights[0]),
            n(*heights.last().unwrap())
        )
    };
    let busy = |from: usize, to: usize| run.results[from..to].iter().map(|r| r.used).max().unwrap();
    eprintln!(
        "reader: list of 10 entries drawn in {} frames ({units} units from reset)\n\
         reader: Enter to the ~6 KB article: {}; busiest frame {} units\n\
         reader: Enter to the >48 KiB article: {}; busiest frame {} units\n\
         reader: Enter to the ~3 KB article: {}\n\
         reader: {}; frames not ending in wait_frame: {spilled}\n\
         reader: deepest stack {} bytes; host {:.1} µs per frame (machine and content service)",
        list_done + 1,
        open(50, 90),
        busy(50, 90),
        open(123, 160),
        busy(123, 160),
        open(182, 190),
        stats.describe(),
        ram_end - min_sp,
        host.as_secs_f64() * 1e6 / 200.0,
    );
    assert_eq!(spilled, 0, "a frame's work ran into the next frame");
}

/// The costliest text: wide characters only. Measured, not limited: a frame
/// that runs out of budget simply continues in the next.
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
    assert_eq!(run.scroll(), 5 * 3 * LINE as i32);
    assert!(run.status().contains('%'), "{}", run.status());
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(IMAGE, &["reader/reader.c"], true, "reader", RAM_PAGES, &[(2, 0), (4, 0), (7, 0)]);
}
