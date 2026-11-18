//! programs/reader/reader.terse: golden views (programs/reader/expected/:
//! the terminal, then the page as the reference layout shows it, as
//! `terse run --dump --dump-page` prints them; UPDATE_GOLDEN=1 rewrites
//! them) with a store of fixture feeds, the requests that need consent
//! (subscribe, unsubscribe, publish) allowed or refused, and the image is
//! up to date.
//!
//! `timing` prints the list and article latencies and the busiest and
//! typical frame (run with `--nocapture`); other tests print their busiest
//! frame.

mod common;

use terse_core::page::reference_layout;
use terse_core::{Event, FrameInput, FrameResult, Image, Machine, PageInput, RAM_BASE, Script};
use terse_net::{BlobStore, Consent, ContentService, Draft, FeedKey, Identity, Node, Subscriptions, blob_id, to_hex};

const IMAGE: &[u8] = include_bytes!("../../../programs/reader/reader.terse");
const RAM_PAGES: u8 = 22;
/// 2026-11-17 18:00 UTC: entry times count up from here.
const T0: u32 = 1_794_938_400;
/// The article's (and help's) rectangle (reader.c RECT_*).
const RECT: (u16, u16, u16, u16) = (64, 0, 512, 464);
/// The lists' rectangle (reader.c LIST_*).
const LIST: (u16, u16, u16, u16) = (32, 0, 576, 464);
/// Up and Down move this far (reader.c LINE).
const LINE: u32 = 20;
/// One list slot in the reference layout: 8 units of space, a 16-unit line
/// (20 high) and a 12-unit line (15 high).
const SLOT: u32 = 8 + 20 + 15;
/// Entries wholly visible: floor(464 / 43) - 1 (the header slot).
const VISIBLE: usize = 9;
/// The selected entry's title colour (reader.c rgb_sel).
const SELECTED: [u8; 3] = [255, 200, 95];

fn new_machine() -> Machine {
    Machine::new(&Image::parse(IMAGE).unwrap())
}

fn alice() -> Identity {
    Identity::from_seed([11; 32])
}

fn bob() -> Identity {
    Identity::from_seed([12; 32])
}

/// The host's own identity, where a test gives the host one.
fn carol() -> Identity {
    Identity::from_seed([13; 32])
}

/// A feed key's short form, as the reader shows it.
fn short(key: &FeedKey) -> String {
    let h = to_hex(key);
    format!("{}…{}", &h[..6], &h[58..])
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
    /// The host's config directory: identity.key.
    config: tempfile::TempDir,
    author: Node,
    feeds: Vec<FeedKey>,
    time: u32,
}

impl Fixture {
    fn new() -> Self {
        let store = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let author = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
        Fixture { store, config, author, feeds: vec![alice().feed_key(), bob().feed_key()], time: T0 }
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
        std::fs::remove_file(f.store.path().join(to_hex(&blob_id(b"gone")))).unwrap();
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

    /// `n` short entries by alice, "Entry 00" first (so last in the list).
    fn many(n: usize) -> Self {
        let mut f = Fixture::new();
        for i in 0..n {
            f.publish(&alice(), &format!("Entry {i:02}"), format!("Entry number {i}.\n").as_bytes(), None);
        }
        f
    }

    /// Gives the host carol's identity (identity.key in its config).
    fn with_identity(mut self, subscribed: bool) -> Self {
        std::fs::write(self.config.path().join("identity.key"), format!("{}\n", to_hex(&[13; 32]))).unwrap();
        if subscribed {
            self.publish(&carol(), "Carol's first post", b"Hello from carol.\n", None);
            self.feeds.push(carol().feed_key());
        }
        self
    }

    fn content(&self, consent: Consent) -> ContentService {
        let mut node = Node::new(BlobStore::open(self.store.path(), None).unwrap(), None);
        node.set_subscriptions(Subscriptions::in_memory(self.feeds.iter().copied()));
        node.set_identity_path(self.config.path().join("identity.key")).unwrap();
        ContentService::start_with_consent(node, consent)
    }
}

struct Run {
    machine: Machine,
    content: ContentService,
    script: Script,
    page: PageInput,
    completions: Vec<terse_core::Completion>,
    feeds_updated: bool,
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
        Run::with_consent(fixture, script, Consent::Deny)
    }

    fn with_consent(fixture: &Fixture, script: &str, consent: Consent) -> Self {
        Run {
            machine: new_machine(),
            content: fixture.content(consent),
            script: Script::parse(script).unwrap(),
            page: PageInput::default(),
            completions: Vec::new(),
            feeds_updated: false,
            unix_time: 0,
            results: Vec::new(),
            screens: Vec::new(),
            heights: Vec::new(),
            submits: Vec::new(),
        }
    }

    /// One frame as `terse run` runs it (with `extra` events and this
    /// run's clock): the page events of the reference layout first, a
    /// feeds-updated event in the frame after the host stored a new entry.
    fn frame(&mut self, extra: &[Event]) {
        let frame = self.machine.frame();
        let mut events = self.script.events(frame).to_vec();
        events.extend_from_slice(extra);
        let mut events = self.page.events(self.machine.page(), &events);
        if std::mem::take(&mut self.feeds_updated) {
            events.push(Event::feeds_updated());
        }
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
        self.feeds_updated = self.content.feeds_updated();
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

    /// A list's entries (title lines after the header slot): the text of
    /// the line and whether it is in the selection's colour.
    fn entries(&self) -> Vec<(String, bool)> {
        let layout = self.layout();
        let titles: Vec<_> = layout.lines.iter().filter(|l| l.spans.first().is_some_and(|s| s.style.size == 16)).collect();
        titles[1..]
            .iter()
            .map(|l| (l.spans.iter().map(|s| s.text.as_str()).collect(), l.spans[0].style.rgb == SELECTED))
            .collect()
    }

    /// The title of the selected entry.
    fn selected(&self) -> String {
        let sel: Vec<String> = self.entries().into_iter().filter(|e| e.1).map(|e| e.0).collect();
        assert_eq!(sel.len(), 1, "one selected entry: {sel:?}");
        sel[0].clone()
    }

    /// The scroll value as the program stored it.
    fn scroll(&self) -> i32 {
        self.machine.page().unwrap().scroll() as i32
    }

    fn status(&self) -> String {
        row(&self.screen(), 29).trim_end().to_string()
    }

    /// The reading position in the status row, if shown.
    fn pct(&self) -> Option<u32> {
        let s = self.status();
        s.split_once('%').map(|(p, _)| p.trim().parse().unwrap_or_else(|_| panic!("{s}")))
    }

    /// Script lines from this frame on (`{f}` is this frame), then `n` frames.
    fn script_now(&mut self, lines: &str, n: u32) {
        let f = self.machine.frame();
        self.script = Script::parse(&lines.replace("{f}", &f.to_string())).unwrap();
        self.frames(n);
    }

    fn press(&mut self, keys: &str) {
        self.script_now(&format!("{{f}} press {keys}\n"), 2);
    }

    /// Text events (one per character), 32 a frame (the queue holds 64
    /// events), as a host spreads a long paste.
    fn text(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let f = self.machine.frame();
        let lines: String =
            chars.chunks(32).enumerate().map(|(i, c)| format!("{} text {}\n", f + i as u32, c.iter().collect::<String>())).collect();
        self.script = Script::parse(&lines).unwrap();
        self.frames(chars.len().div_ceil(32) as u32 + 1);
    }

    /// Clicks the first span of anchor `id`, where the reference layout
    /// puts it (the page scrolled to `scroll_for`).
    fn click_anchor(&mut self, id: u16) {
        let layout = self.layout();
        let page = self.machine.page().unwrap();
        let scroll = page.scroll_for(layout.height);
        let r = page.region();
        let (line, span) = layout
            .lines
            .iter()
            .find_map(|l| l.spans.iter().find(|s| s.style.anchor == id).map(|s| (l, s)))
            .unwrap_or_else(|| panic!("anchor {id}: {}", self.page_text()));
        assert!(line.y >= scroll && line.y + line.height <= scroll + r.h as u32, "anchor {id} in view");
        let (x, y) = (r.x as u32 + span.x0 / 2 + 4, r.y as u32 + line.y - scroll + 4);
        self.script_now(&format!("{{f}} click {x} {y}\n"), 2);
    }

    /// The busiest frame so far, and whether every frame ended in wait_frame.
    fn busiest(&self) -> (u64, bool) {
        (self.results.iter().map(|r| r.used).max().unwrap(), self.results.iter().all(|r| r.waited))
    }
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("reader/expected/{name}.txt"), got);
}

fn row(screen: &str, n: usize) -> &str {
    screen.lines().nth(n).unwrap()
}

/// Rows 0-27 (and 28 unless a key is being typed) show nothing.
fn blank_rows(screen: &str, rows: usize) {
    for (i, l) in screen.lines().take(rows).enumerate() {
        assert!(l.trim().is_empty(), "row {i} is not blank: {l:?}");
    }
}

#[test]
fn empty_timeline() {
    let mut f = Fixture::new();
    f.feeds.clear();
    let mut run = Run::new(&f, "");
    run.frames(10);
    golden("empty", &run.view());
    let text = run.page_text();
    assert!(text.contains("No articles yet"), "{text}");
    assert!(text.contains("Press F to see your feeds"), "{text}");
    assert!(text.contains("Press T to publish a test article"), "{text}");
    assert!(text.contains("Press ? for help"), "{text}");
    blank_rows(&run.screen(), 29);
    assert!(run.status().contains("F feeds"), "{}", run.status());
    let page = run.machine.page().unwrap();
    assert_eq!(page.background(), [28, 28, 28, 255], "opaque");
    let r = page.region();
    assert_eq!((r.x, r.y, r.w, r.h), RECT);
}

#[test]
fn timeline_list() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "");
    run.frames(10);
    let view = run.view();
    golden("list", &view);
    blank_rows(&run.screen(), 29);
    let text = run.page_text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Timeline");
    assert_eq!(lines[1], "10 articles  ·  2 feeds");
    // Every entry fits: header + 10 slots, the slot height exact.
    assert_eq!(run.layout().height, 11 * SLOT);
    assert_eq!(lines[2], "[Emoji 😀 and tabs in a title]{1}", "a tab in a title is a space");
    let bob = short(&bob().feed_key());
    let alice = short(&alice().feed_key());
    assert!(text.contains(&format!("[{bob}  ·  2026-11-17 19:40  ·  6 bytes]{{1}}")), "{text}");
    assert!(text.contains(&format!("{alice}  ·  2026-11-17 18:50  ·  6.")), "the long article in KB: {text}");
    assert!(text.contains("↳ reply]{5}"), "{text}");
    assert!(text.contains("4 bytes  ·  not downloaded]{4}"), "{text}");
    assert!(text.contains(&format!("[{SOURDOUGH_TITLE}]{{10}}\n")), "{text}");
    let entries = run.entries();
    assert_eq!(entries.len(), 10);
    assert_eq!(entries.iter().filter(|e| e.1).count(), 1);
    assert!(entries[0].1 && entries[0].0.starts_with("Emoji"), "{entries:?}");
    let r = run.machine.page().unwrap().region();
    assert_eq!((r.x, r.y, r.w, r.h), LIST);
    assert!(run.status().contains("1/10"), "{}", run.status());
}

/// More entries than fit: the window follows the selection (one submit per
/// key), PgDn, End, Home and the wheel.
#[test]
fn list_scrolls_with_the_selection() {
    let f = Fixture::many(30);
    // Entry k of the timeline is "Entry {29 - k}".
    let title = |k: usize| format!("Entry {:02}", 29 - k);
    let mut run = Run::new(&f, "");
    run.frames(10);
    let first: Vec<String> = run.entries().into_iter().map(|e| e.0).collect();
    assert_eq!(first, (0..=VISIBLE).map(title).collect::<Vec<_>>(), "visible entries and one more, partly shown");
    assert_eq!(run.layout().height, (VISIBLE as u32 + 2) * SLOT);
    let script: String = (0..12).map(|i| format!("{} press down\n", 20 + i)).collect();
    run.script = Script::parse(&script).unwrap();
    run.until(33);
    assert_eq!(run.submits.iter().filter(|&&f| (20..32).contains(&f)).count(), 12, "one submit per Down: {:?}", run.submits);
    assert_eq!(run.selected(), title(12));
    let shown: Vec<String> = run.entries().into_iter().map(|e| e.0).collect();
    assert_eq!(shown, (4..=4 + VISIBLE).map(title).collect::<Vec<_>>(), "the selection is the last wholly visible entry");
    assert!(run.status().contains("13/30"), "{}", run.status());
    golden("list-scrolled", &run.view());

    run.press("pagedown");
    assert_eq!(run.selected(), title(21));
    assert_eq!(run.entries()[0].0, title(13));
    run.press("pagedown");
    assert_eq!(run.selected(), title(29), "at the end the selection moves instead");
    assert_eq!(run.entries()[0].0, title(21));
    run.press("end");
    assert_eq!(run.selected(), title(29));
    let shown: Vec<String> = run.entries().into_iter().map(|e| e.0).collect();
    assert_eq!(shown, (21..30).map(title).collect::<Vec<_>>(), "the screen stays filled");
    run.press("home");
    assert_eq!(run.selected(), title(0));
    assert_eq!(run.entries()[0].0, title(0));
    run.script_now("{f} wheel 2\n", 2);
    assert_eq!(run.entries()[0].0, title(6));
    assert_eq!(run.selected(), title(6), "the wheel keeps the selection on screen");
    run.press("up");
    assert_eq!(run.entries()[0].0, title(5));
    run.press("pageup");
    assert_eq!(run.entries()[0].0, title(0));
    assert_eq!(run.selected(), title(0));
    let (worst, waited) = run.busiest();
    assert!(waited);
    eprintln!("reader: list of 30, busiest frame {worst} units ({:.2}%)", common::pct(worst));
}

#[test]
fn click_opens_an_entry() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "");
    run.frames(10);
    run.click_anchor(5);
    let text = run.page_text();
    assert!(text.starts_with("Re: Welcome to the Terse reader\n"), "{text}");
    assert!(text.contains("Thanks, this is a reply."), "{text}");
    run.press("esc");
    assert_eq!(run.selected(), "Re: Welcome to the Terse reader", "the clicked entry is selected");
}

#[test]
fn feeds_view() {
    let f = Fixture::populated().with_identity(true);
    let mut run = Run::new(&f, "10 press f\n");
    run.frames(20);
    let view = run.view();
    golden("feeds", &view);
    blank_rows(&run.screen(), 29);
    let text = run.page_text();
    assert!(text.starts_with(&format!("Feeds\n3 subscriptions  ·  your key {}\n", short(&carol().feed_key()))), "{text}");
    // In key order: bob (0b51…), alice (66be…), carol.
    let mut feeds = vec![(bob(), 5), (alice(), 5), (carol(), 1)];
    feeds.sort_by_key(|(who, _)| who.feed_key());
    for (i, (who, n)) in feeds.iter().enumerate() {
        let id = i + 1;
        let you = if who.feed_key() == carol().feed_key() { format!("[  you]{{{id}}}") } else { String::new() };
        let s = if *n == 1 { "" } else { "s" };
        let want = format!("[{}]{{{id}}}{you}\n[highest #", short(&who.feed_key()));
        let at = text.find(&want).unwrap_or_else(|| panic!("{want}\n{text}"));
        let held = format!("  ·  {n} article{s} held]{{{id}}}\n");
        assert!(text[at..].lines().nth(1).is_some_and(|l| format!("{l}\n").ends_with(&held)), "{held}\n{text}");
    }
    assert_eq!(run.layout().height, 4 * SLOT);
    assert!(run.status().contains("S subscribe"), "{}", run.status());
    run.press("down");
    assert!(run.selected().starts_with(&short(&feeds[1].0.feed_key())), "{}", run.selected());
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n11 articles  ·  3 feeds\n"), "{}", run.page_text());
}

/// A feed key in groups, as the help page shows it (whitespace is ignored).
fn grouped(key: &FeedKey) -> String {
    let h = to_hex(key);
    (0..8).map(|g| &h[g * 8..g * 8 + 8]).collect::<Vec<_>>().join(" ")
}

#[test]
fn subscribe_by_key() {
    let mut f = Fixture::populated();
    f.feeds = vec![alice().feed_key()];
    let mut run = Run::with_consent(&f, "10 press f\n12 press s\n", Consent::Allow);
    run.frames(14);
    assert!(run.page_text().starts_with("Feeds\n1 subscription  ·  no key of your own yet\n"), "{}", run.page_text());
    // The S's own text event is not typed; pasted spaces are ignored.
    run.text(&format!("s{}", grouped(&bob().feed_key())));
    let view = run.view();
    golden("feeds-entry", &view);
    let entry = row(&view, 28);
    assert_eq!(entry.trim_end(), format!(" Feed key: {}", to_hex(&bob().feed_key())));
    assert_eq!(run.machine.page().unwrap().region().h, 448, "the page leaves row 28 to the entry");
    blank_rows(&view, 28);
    run.press("enter");
    run.frames(4);
    assert_eq!(run.status(), format!(" Subscribed to {}", short(&bob().feed_key())));
    assert!(row(&run.screen(), 28).trim().is_empty());
    assert!(run.page_text().starts_with("Feeds\n2 subscriptions"), "{}", run.page_text());
    assert!(run.page_text().contains("highest #10  ·  5 articles held"), "{}", run.page_text());
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n10 articles  ·  2 feeds\n"), "the timeline reloaded: {}", run.page_text());
    // Again: already subscribed, nothing asked.
    run.press("f");
    run.press("s");
    run.text(&to_hex(&bob().feed_key()));
    run.press("enter");
    assert_eq!(run.status(), format!(" Already subscribed to {}", short(&bob().feed_key())));
}

#[test]
fn subscribe_with_a_malformed_key() {
    let f = Fixture::populated();
    let mut run = Run::with_consent(&f, "10 press f\n12 press s\n", Consent::Allow);
    run.frames(14);
    run.press("enter");
    assert_eq!(run.status(), " Type or paste a feed key (64 hex digits), or Esc to cancel");
    run.text("abc 123");
    run.press("enter");
    assert_eq!(run.status(), " Not a feed key: 6 hex digits, but a key has 64");
    assert!(row(&run.screen(), 28).starts_with(" Feed key: abc123"), "still typing: {}", row(&run.screen(), 28));
    run.press("backspace");
    run.text("xyz");
    run.press("enter");
    assert_eq!(run.status(), " Not a feed key: a key has only the hex digits 0-9 and a-f");
    let k = to_hex(&bob().feed_key());
    run.press("esc");
    assert!(row(&run.screen(), 28).trim().is_empty(), "cancelled");
    assert_eq!(run.machine.page().unwrap().region().h, 464);
    run.press("s");
    run.text(&format!("{k}0"));
    run.press("enter");
    assert_eq!(run.status(), " Not a feed key: 65 hex digits, but a key has 64");
    run.press("esc");
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n10 articles  ·  2 feeds"));
}

#[test]
fn subscribe_refused() {
    let mut f = Fixture::populated();
    f.feeds = vec![alice().feed_key()];
    let mut run = Run::with_consent(&f, "10 press f\n12 press s\n", Consent::Deny);
    run.frames(14);
    run.text(&to_hex(&bob().feed_key()));
    run.press("enter");
    run.frames(2);
    assert_eq!(run.status(), format!(" Subscribing to {} refused", short(&bob().feed_key())));
    assert!(run.page_text().starts_with("Feeds\n1 subscription"), "{}", run.page_text());
    // Not an Ed25519 key: refused too.
    run.press("s");
    run.text(&"0".repeat(63));
    run.text("2");
    run.press("enter");
    run.frames(2);
    assert!(run.status().ends_with(" refused"), "{}", run.status());
}

#[test]
fn unsubscribe() {
    let f = Fixture::populated();
    let mut run = Run::with_consent(&f, "10 press f\n12 press down\n14 press u\n", Consent::Allow);
    run.frames(20);
    // Selected: the second feed in key order, alice.
    assert_eq!(run.status(), format!(" Unsubscribed from {}", short(&alice().feed_key())));
    assert!(run.page_text().starts_with("Feeds\n1 subscription"), "{}", run.page_text());
    assert_eq!(run.selected(), short(&bob().feed_key()));
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n5 articles  ·  1 feed\n"), "{}", run.page_text());
    // Refused.
    let mut run = Run::with_consent(&f, "10 press f\n14 press u\n", Consent::Deny);
    run.frames(20);
    assert_eq!(run.status(), format!(" Unsubscribing from {} refused", short(&bob().feed_key())));
}

#[test]
fn help_shows_the_own_key() {
    let f = Fixture::populated().with_identity(false);
    let mut run = Run::new(&f, "10 text ?\n");
    run.frames(14);
    let view = run.view();
    golden("help", &view);
    blank_rows(&run.screen(), 29);
    let h = to_hex(&carol().feed_key());
    let group = |g: usize| &h[g * 8..g * 8 + 8];
    let want = format!("{} {} {} {}\n{} {} {} {}\n", group(0), group(1), group(2), group(3), group(4), group(5), group(6), group(7));
    assert!(run.page_text().contains(&want), "{}", run.page_text());
    assert!(view.contains("mono15"), "the key in the mono face");
    assert!(run.page_text().contains("subscribe: type or paste a feed key"));
    // It is longer than the screen: it scrolls, with its position shown.
    assert_eq!(run.pct(), Some(0), "{}", run.status());
    run.press("end");
    assert_eq!(run.pct(), Some(100), "{}", run.status());
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline"));
    // No identity: F1 says how to get one.
    let f = Fixture::populated();
    let mut run = Run::new(&f, "10 press f1\n");
    run.frames(14);
    assert!(run.page_text().contains("This host has no feed key yet. Press T to publish a test article"), "{}", run.page_text());
}

#[test]
fn publish_a_test_article() {
    let f = Fixture::populated();
    let mut run = Run::with_consent(&f, "10 press t\n", Consent::Allow);
    run.frames(20);
    assert_eq!(run.status(), " Published “Test article 1” as #1 and subscribed to your own feed");
    let text = run.page_text();
    assert!(text.starts_with("Timeline\n11 articles  ·  3 feeds\n[Test article 1]{1}\n"), "{text}");
    run.press("home");
    run.press("enter");
    run.frames(2);
    let text = run.page_text();
    assert!(text.starts_with("Test article 1\n"), "{text}");
    assert!(text.contains("This is a test article, published from the Terse reader"), "{text}");
    assert!(text.contains("[https://example.com/terse]{1}"), "{text}");
    run.press("esc");
    // The identity it created, on the help page; a second test article.
    run.text("?");
    let key = run.layout().lines.iter().filter(|l| l.spans.first().is_some_and(|s| s.style.face == 2)).count();
    assert_eq!(key, 2, "the key in two lines: {}", run.page_text());
    run.press("t");
    run.frames(4);
    assert_eq!(run.status(), " Published “Test article 2” as #2", "already subscribed");
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n12 articles  ·  3 feeds\n[Test article 2]{1}\n"), "{}", run.page_text());
    run.press("f");
    run.frames(4);
    let text = run.page_text();
    assert!(text.contains("  you]") && text.contains("[highest #2  ·  2 articles held]"), "{text}");
    let (worst, waited) = run.busiest();
    assert!(waited);
    eprintln!("reader: publishing, busiest frame {worst} units ({:.2}%)", common::pct(worst));

    let mut run = Run::with_consent(&f, "10 press t\n", Consent::Deny);
    run.frames(20);
    assert_eq!(run.status(), " Publishing refused");
}

/// The sourdough article: opened, scrolled every way, a link pressed, and
/// back to the list.
#[test]
fn open_scroll_and_return() {
    let f = Fixture::populated();
    // End selects the last entry (the sourdough article); Enter in frame 14.
    let mut run = Run::new(&f, "12 press end\n14 press enter\n");
    run.until(14);
    assert!(run.page_text().starts_with(&format!("{SOURDOUGH_TITLE}\n")), "a stand-in while it is read");
    assert!(run.page_text().contains("Reading the article…"));
    let submits = run.submits.len();
    run.until(15);
    assert_eq!(run.submits.len(), submits + 1, "the whole article is in the document at once");
    assert_eq!(run.pct(), None, "no position before the height: {}", run.status());
    run.until(16);
    let height = run.layout().height;
    assert_eq!(run.heights.last(), Some(&(16, height)));
    let opened = run.view();
    golden("article", &opened);
    assert!(opened.contains("sans26b |Keeping a sourdough starter alive"));
    assert!(opened.contains("-- rule --"));
    assert!(opened.contains("mono13 |    Day   Keep   Flour   Water   Notes"), "preformatted");
    assert!(opened.contains("colonised by wild yeast"));
    assert!(run.page_text().contains(&format!("{}  ·  #1  ·  2026-11-17 18:10 UTC  ·  2.", short(&alice().feed_key()))));
    let max = height as i32 - RECT.3 as i32;
    assert!(max > 0, "taller than the rectangle: {height}");
    assert_eq!(run.pct(), Some(0), "0% at the top");
    blank_rows(&opened, 29);

    run.press("down");
    assert_eq!(run.scroll(), LINE as i32);
    assert_eq!(run.pct(), Some(LINE * 100 / max as u32));
    run.press("pagedown");
    assert_eq!(run.scroll(), (LINE + RECT.3 as u32 - LINE) as i32);
    run.script_now("{f} wheel 1\n", 2);
    assert_eq!(run.scroll(), (RECT.3 as u32 + 3 * LINE) as i32);
    golden("article-scrolled", &run.view());
    run.press("end");
    assert_eq!(run.scroll(), max);
    assert_eq!(run.pct(), Some(100));
    assert!(run.view().contains("the same jar going for years."));
    run.press("down");
    assert_eq!(run.scroll(), max, "clamped at the end");
    run.script_now("{f} wheel 5\n", 2);
    assert_eq!(run.scroll(), max, "clamped at the end");
    run.press("pageup");
    assert_eq!(run.scroll(), max - (RECT.3 as i32 - LINE as i32));
    run.press("home");
    assert_eq!(run.scroll(), 0);
    run.press("up");
    assert_eq!(run.scroll(), 0, "clamped at the top");
    assert_eq!(run.pct(), Some(0));

    // Press the first link, where the reference layout puts it.
    run.press("pagedown");
    assert_eq!(
        run.layout().lines.iter().flat_map(|l| &l.spans).find(|s| s.style.anchor == 1).unwrap().text,
        "https://en.wikipedia.org/wiki/Sourdough"
    );
    run.click_anchor(1);
    let linked = run.view();
    golden("article-link", &linked);
    assert_eq!(row(&linked, 29).trim_end(), " Link: https://en.wikipedia.org/wiki/Sourdough");
    // The second link: the parenthesis and full stop are not part of it.
    assert!(run.page_text().contains("([https://en.wikipedia.org/wiki/Lactobacillus]{2}).\n"), "{}", run.page_text());
    run.press("down");
    assert!(run.status().contains('%'), "a key puts the position back");

    run.press("esc");
    assert_eq!(run.selected(), SOURDOUGH_TITLE);
    let r = run.machine.page().unwrap().region();
    assert_eq!((r.x, r.y, r.w, r.h), LIST);
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
    assert!(run.page_text().starts_with("Tea in three languages"));
    assert!(view.contains("日本では、緑茶は毎日の生活に欠かせない飲み物です。"));
    assert!(view.contains("الشاي جزء أساسي"));
    assert!(run.layout().height <= RECT.3 as u32);
    assert_eq!(run.pct(), None, "it fits: no position: {}", run.status());
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
    assert!(run.page_text().contains(&format!("#9  ·  2026-11-17 19:30 UTC  ·  {:.1} KB", size as f64 / 1024.0)), "{}", run.page_text());
    // The document ends where the 48 KiB do, in a paragraph.
    let per = big_line(1).len();
    assert!(view.contains(&format!("[… the article continues: this reader shows 49152 of its {size} bytes]")), "{view}");
    assert!(view.contains(&format!("|{}\n", big_line(49152 / per).trim_end())), "{view}");
    assert!(view.contains(&format!("|{}\n", &big_line(49152 / per + 1)[..49152 % per])), "{view}");
    assert_eq!(run.pct(), Some(100), "{}", run.status());
    let article: Vec<u32> = run.submits.iter().copied().filter(|&f| f > 14).collect();
    assert_eq!(article.len(), 2, "a part at once, then the whole: {:?}", run.submits);
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
    assert_eq!(run.pct(), Some(100), "{}", run.status());
    let worst = run.results.iter().map(|r| r.used).max().unwrap();
    let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
    assert!(run.results.iter().all(|r| r.waited), "a frame ran out of budget ({worst} units): {used:?}");
    eprintln!(
        "reader: a full document of the costliest text: built in {} frames, busiest frame {worst} units ({:.2}%)",
        run.submits.last().unwrap() - 12,
        common::pct(worst)
    );
}

#[test]
fn article_not_found() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "");
    run.frames(12);
    run.click_anchor(4);
    run.frames(2);
    let view = run.view();
    golden("not-found", &view);
    let text = run.page_text();
    assert!(text.starts_with("An article that is not in the store\n"), "{text}");
    assert!(text.contains("Not found: neither this host nor its relay has the article."), "{text}");
    assert!(text.contains("Esc: back to the timeline"), "{text}");
    blank_rows(&view, 29);
}

/// A feeds-updated refresh keeps the selection on its entry.
#[test]
fn refresh_keeps_the_selection() {
    let mut f = Fixture::populated();
    let mut run = Run::new(&f, "12 press down\n13 press down\n");
    run.frames(20);
    assert!(run.selected().starts_with("A title long enough"));
    f.publish(&bob(), "Fresh news", b"Just in.\n", None);
    run.frame(&[Event::feeds_updated()]);
    run.frames(10);
    golden("refreshed", &run.view());
    assert!(run.page_text().starts_with("Timeline\n11 articles  ·  2 feeds\n[Fresh news]{1}\n"), "{}", run.page_text());
    assert!(run.selected().starts_with("A title long enough"), "the selection stays with its entry");
    assert!(run.status().contains("4/11"), "{}", run.status());
}

/// Measurements: frames to show the list, Down to the new document and its
/// height, Enter to the document submitted, to its first height event and
/// to the height of the whole document, the busiest and typical frame, and
/// the deepest stack.
#[test]
fn timing() {
    let f = Fixture::populated();
    let script = "40 press down\n41 press down\n42 press down\n43 press down\n44 press down\n50 press enter\n\
                  90 press pagedown\n95 wheel 2\n100 press end\n110 press esc\n120 press home\n121 press down\n\
                  123 press enter\n160 press end\n170 press esc\n180 press end\n182 press enter\n190 press esc\n\
                  195 press f\n205 text ?\n215 press esc\n";
    let mut run = Run::new(&f, script);
    let start = std::time::Instant::now();
    run.frames(230);
    let host = start.elapsed();
    let list_done = *run.submits.iter().filter(|&&f| f < 40).last().unwrap();
    let units: u64 = run.results[..=list_done as usize].iter().map(|r| r.used).sum();
    let spilled = run.results.iter().filter(|r| !r.waited).count();
    let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
    let stats = common::FrameStats::new(&used);
    let ram_end = RAM_BASE + run.machine.ram().len() as u32;
    let min_sp = run.machine.cpu().min_sp;
    // Each Down (frames 40-44): its submit and height, counting the frame
    // the key arrives in as 1.
    for f in 40..45 {
        assert!(run.submits.contains(&f), "Down in frame {f} submits in that frame: {:?}", run.submits);
        assert!(run.heights.iter().any(|&(h, _)| h == f + 1), "its height in the next frame");
    }
    // Frames from the one Enter arrives in to the one after which the
    // document is submitted, the first height event arrives and the whole
    // document's height arrives, counting both (1: at once).
    let open = |enter: u32, until: u32| {
        let submits: Vec<u32> = run.submits.iter().copied().filter(|&f| f > enter && f < until).collect();
        let heights: Vec<u32> = run.heights.iter().map(|&(f, _)| f).filter(|&f| f > enter + 1 && f < until).collect();
        let n = |f: u32| f - enter + 1;
        format!(
            "article submitted after {} frames ({} submits), first height in frame {}, whole document's height in frame {}",
            n(submits[0]),
            submits.len(),
            n(heights[0]),
            n(*heights.last().unwrap())
        )
    };
    let busy = |from: usize, to: usize| run.results[from..to].iter().map(|r| r.used).max().unwrap();
    eprintln!(
        "reader: list of 10 entries shown after {} frames ({units} units from reset)\n\
         reader: Down to its document submitted: 1 frame (the frame of the key); its height: frame 2; busiest Down frame {} units\n\
         reader: Enter to the ~6 KB article: {}; busiest frame {} units\n\
         reader: Enter to the >48 KiB article: {}; busiest frame {} units\n\
         reader: Enter to the ~3 KB article: {}\n\
         reader: feeds and help: busiest frame {} units\n\
         reader: {}; frames not ending in wait_frame: {spilled}\n\
         reader: deepest stack {} bytes; host {:.1} µs per frame (machine and content service)",
        list_done + 1,
        busy(40, 45),
        open(50, 90),
        busy(50, 90),
        open(123, 160),
        busy(123, 160),
        open(182, 190),
        busy(195, 230),
        stats.describe(),
        ram_end - min_sp,
        host.as_secs_f64() * 1e6 / 230.0,
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
