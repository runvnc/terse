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
const RAM_PAGES: u8 = 42;
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
    /// The host's nicknames for feeds.
    nicknames: Vec<(FeedKey, String)>,
    /// The host's relays (none: no network).
    relays: Vec<String>,
    /// The hints of subscribed feeds (NETWORK 4.9).
    hints: Vec<(FeedKey, String)>,
    /// Hint relays at loopback addresses are used (for relays in this test).
    loopback_hints: bool,
}

impl Fixture {
    fn new() -> Self {
        let store = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let author = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
        Fixture {
            store,
            config,
            author,
            feeds: vec![alice().feed_key(), bob().feed_key()],
            time: T0,
            nicknames: Vec::new(),
            relays: Vec::new(),
            hints: Vec::new(),
            loopback_hints: false,
        }
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

    /// Publishes a profile for `who` into the store.
    fn profile(&mut self, who: &Identity, name: &str, listed: bool) {
        self.author.set_identity(who.clone());
        self.time += 600;
        assert!(matches!(self.author.publish_profile(name, listed, Some(String::new()), self.time), terse_net::Done::Published { .. }));
    }

    fn content(&self, consent: Consent) -> ContentService {
        let mut node = Node::new(BlobStore::open(self.store.path(), None).unwrap(), None);
        for url in &self.relays {
            node.add_relay(url, terse_net::relay::RelayClient::websocket(url, terse_net::relay::DEFAULT_TIMEOUT));
        }
        let mut subscriptions = Subscriptions::in_memory(self.feeds.iter().copied());
        for (feed, hints) in &self.hints {
            subscriptions.set_hints(feed, Some(hints)).unwrap();
        }
        node.set_subscriptions(subscriptions);
        if self.loopback_hints {
            node.set_feed_relays_unrestricted(Box::new(|url| {
                terse_net::relay::RelayClient::websocket(url, terse_net::relay::DEFAULT_TIMEOUT)
            }));
        }
        node.set_identity_path(self.config.path().join("identity.key")).unwrap();
        let mut nicknames = terse_net::profile::Nicknames::in_memory();
        for (feed, name) in &self.nicknames {
            nicknames.set(*feed, Some(name)).unwrap();
        }
        node.set_nicknames(nicknames);
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
    /// Every get the program sent: frame, blob, length.
    gets: Vec<(u32, [u8; 32], u16)>,
    /// Every subscribe the program sent: the feed and its hints.
    subscribes: Vec<([u8; 32], Vec<u8>)>,
    /// The most requests pending at once.
    most_pending: usize,
    pending: usize,
    /// The machine's reading profile (SPEC): content requests outside it
    /// complete with status 05h, without reaching the content service.
    reading: bool,
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
            gets: Vec::new(),
            subscribes: Vec::new(),
            most_pending: 0,
            pending: 0,
            reading: false,
        }
    }

    /// The same on a host of the reading profile.
    fn reading(fixture: &Fixture, script: &str, consent: Consent) -> Self {
        Run { reading: true, ..Run::with_consent(fixture, script, consent) }
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
        assert_eq!(r.completions_ignored, 0, "frame {frame}: {completions:?} reading {}", self.reading);
        assert!(self.machine.fault().is_none(), "{:?}", self.machine.fault());
        self.page.after_frame(self.machine.page());
        let page = self.machine.page().unwrap();
        if page.generation() != generation && page.document().is_some() {
            self.submits.push(frame);
        }
        let requests = self.machine.take_content_requests();
        for r in &requests {
            if let terse_core::RequestKind::Get { id, length, .. } = r.kind {
                self.gets.push((frame, id, length));
            }
            if let terse_core::RequestKind::Subscribe { feed, hints } = &r.kind {
                self.subscribes.push((*feed, hints.clone()));
            }
        }
        self.pending += requests.len();
        self.most_pending = self.most_pending.max(self.pending);
        let (refused, requests): (Vec<_>, Vec<_>) = requests.into_iter().partition(|r| self.reading && !in_reading_profile(&r.kind));
        self.content.submit(requests);
        self.completions = self.content.wait_finished();
        self.completions.extend(refused.iter().map(|r| terse_core::Completion { handle: r.handle, outcome: terse_core::Outcome::Refused }));
        self.pending -= self.completions.len();
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

/// The content operations every machine host provides (the reading
/// profile): get, subscriptions, timeline, subscribe, unsubscribe,
/// identity and names.
fn in_reading_profile(kind: &terse_core::RequestKind) -> bool {
    use terse_core::RequestKind as K;
    matches!(
        kind,
        K::Get { .. } | K::Subscriptions { .. } | K::Timeline { .. } | K::Subscribe { .. } | K::Unsubscribe { .. } | K::Identity | K::Names { .. }
    )
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
    assert!(text.contains(&format!("[{bob}  ·  2026-11-17 19:40  ·  6 bytes  ·  may need a font]{{1}}")), "an emoji is beyond the full repertoire: {text}");
    assert!(text.contains("41 bytes  ·  may need a font]{7}"), "so is Japanese: {text}");
    assert!(text.contains("6.0 KiB]{6}"), "an ASCII title needs no font: {text}");
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
    assert_eq!(entry.trim_end(), format!(" Address: {}", to_hex(&bob().feed_key())));
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
    assert_eq!(run.status(), " Type or paste a feed key or address (key@relay), or Esc to cancel");
    run.text("abc 123");
    run.press("enter");
    assert_eq!(run.status(), " Not a feed key: 6 hex digits, but a key has 64");
    assert!(row(&run.screen(), 28).starts_with(" Address: abc123"), "still typing: {}", row(&run.screen(), 28));
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
    // Addresses (NETWORK 4.9): malformed hints are named as such.
    for bad in ["@", "@a.example,b.example,c.example", "@a.example:0", "@a_b.example", "@a.example."] {
        run.press("esc");
        run.press("s");
        run.text(&format!("{k}{bad}"));
        run.press("enter");
        assert_eq!(
            run.status(),
            " Not a feed address: after @ give one or two relays, host or host:port",
            "{bad}"
        );
    }
    assert!(run.subscribes.is_empty(), "nothing was asked: {:?}", run.subscribes);
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
fn help_shows_the_own_address() {
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
    assert!(run.page_text().contains("subscribe: type or paste a feed key or address"));
    assert!(run.page_text().contains("Your feed address\n"), "{}", run.page_text());
    assert!(run.page_text().contains("only people who use the same relay as you will find your feed"), "{}", run.page_text());
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

/// A profile of carol's (the host's own feed, NETWORK 4.6) whose relay
/// lines name public relays and a private one, stored as received.
fn carol_profile_with_relays(f: &Fixture) {
    let doc = b"listed: no\nrelay: ws://192.168.1.5:7878\nrelay: ws://relay.example:7878\nrelay: ws://[2001:db8::7]:9000/\n";
    BlobStore::open(f.store.path(), None).unwrap().put(doc).unwrap();
    let e = terse_net::Entry::profile(carol().feed_key(), 9, T0, doc, "Carol").sign(&carol()).unwrap();
    terse_net::FeedStore::beside_blobs(f.store.path()).insert(&e).unwrap();
}

/// NETWORK 4.9: the help page shows the own address with the hints the
/// host gives (identity, SPEC §content.feeds): the public relays of the profile.
#[test]
fn help_shows_the_own_address_with_hints() {
    let f = Fixture::populated().with_identity(false);
    carol_profile_with_relays(&f);
    let mut run = Run::new(&f, "10 text ?\n");
    run.frames(14);
    let view = run.view();
    golden("help-hinted", &view);
    let h = to_hex(&carol().feed_key());
    let group = |g: usize| &h[g * 8..g * 8 + 8];
    let want = format!(
        "{} {} {} {}\n{} {} {} {}\n@relay.example,\n[2001:db8::7]:9000\n",
        group(0),
        group(1),
        group(2),
        group(3),
        group(4),
        group(5),
        group(6),
        group(7)
    );
    assert!(run.page_text().contains(&want), "{}", run.page_text());
    assert!(run.page_text().contains("names your public relays"), "{}", run.page_text());
    // On a host of the reading profile too.
    let mut run = Run::reading(&f, "10 text ?\n", Consent::Deny);
    run.frames(14);
    assert!(run.page_text().contains(&want), "{}", run.page_text());
}

/// NETWORK 4.9: an address pasted into the subscribe entry, as a link,
/// with its hints, which go with the subscribe (SPEC §content.feeds).
#[test]
fn subscribe_by_pasted_address() {
    let mut f = Fixture::populated();
    f.feeds = vec![alice().feed_key()];
    let mut run = Run::with_consent(&f, "10 press f\n12 press s\n", Consent::Allow);
    run.frames(14);
    let address = format!("  terse:feed/{}@relay.example:7000,[2001:db8::1]\n", grouped(&bob().feed_key()));
    run.text(&address);
    let view = run.view();
    golden("feeds-entry-address", &view);
    assert!(row(&view, 28).starts_with(" Address: …"), "{}", row(&view, 28));
    assert!(row(&view, 28).trim_end().ends_with("@relay.example:7000,[2001:db8::1]"), "{}", row(&view, 28));
    run.press("enter");
    run.frames(4);
    assert_eq!(run.subscribes, [(bob().feed_key(), b"relay.example:7000,[2001:db8::1]".to_vec())]);
    assert_eq!(run.status(), format!(" Subscribed to {}", short(&bob().feed_key())));
    assert!(run.page_text().starts_with("Feeds\n2 subscriptions"), "{}", run.page_text());
    // The same on a host of the reading profile, which must carry it out.
    let mut run = Run::reading(&f, "10 press f\n12 press s\n", Consent::Allow);
    run.frames(14);
    run.text(&format!("{}@relay.example", to_hex(&bob().feed_key())));
    run.press("enter");
    run.frames(4);
    assert_eq!(run.subscribes, [(bob().feed_key(), b"relay.example".to_vec())]);
    assert_eq!(run.status(), format!(" Subscribed to {}", short(&bob().feed_key())));
}

/// NETWORK 4.9, ARTICLE.md 4.1: a feed link with hints opens the feed's
/// page, which shows them and passes them on when S subscribes.
#[test]
fn feed_page_from_a_hinted_link() {
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    let text = format!(
        "Follow bob at terse:feed/{}@relay.example:7000,203.0.113.9 (his relays).\n",
        to_hex(&bob().feed_key())
    );
    f.publish(&alice(), "Bob's address", text.as_bytes(), None);
    let mut run = Run::with_consent(&f, "10 press enter\n14 press tab\n", Consent::Allow);
    run.frames(18);
    let shown = format!("[terse:feed/{}@relay.example:7000,203.0.113.9]{{1}}", short(&bob().feed_key()));
    assert!(run.page_text().contains(&shown), "{}", run.page_text());
    assert!(run.status().contains("@relay.example:7000,203.0.113.9  (a feed)"), "{}", run.status());
    run.press("enter");
    run.frames(2);
    let view = run.view();
    golden("feed-page-hinted", &view);
    let text = run.page_text();
    assert!(text.contains("Relays its address names:\nrelay.example:7000\n203.0.113.9\n"), "{text}");
    assert!(text.contains("your host connects to"), "{text}");
    run.press("s");
    run.frames(4);
    assert_eq!(run.subscribes, [(bob().feed_key(), b"relay.example:7000,203.0.113.9".to_vec())]);
    assert_eq!(run.status(), format!(" Subscribed to {}", short(&bob().feed_key())));
    // A feed page reached otherwise has no hints.
    run.press("esc");
    run.press("esc");
    run.press("f");
    run.press("enter");
    run.frames(2);
    assert!(!run.page_text().contains("Relays its address names"), "{}", run.page_text());
}

/// NETWORK 4.9: a feed followed through a relay its address names is
/// marked in the feeds view (names flag 08h, SPEC §content.feeds), over two real
/// relays: the host's own (A), which does not hold bob's feed, and B.
#[test]
fn feeds_view_marks_a_feed_followed_through_a_hint() {
    let dirs: Vec<tempfile::TempDir> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let serve = |dir: &std::path::Path| {
        let relay = std::sync::Arc::new(terse_relay::Relay::new(BlobStore::open(dir, None).unwrap(), 1024));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let r = relay.clone();
        std::thread::spawn(move || terse_relay::serve(listener, r));
        (url, relay)
    };
    let (url_a, _a) = serve(dirs[0].path());
    let (url_b, b) = serve(dirs[1].path());
    let article = b"On B only.\n".to_vec();
    b.blobs().put(&article).unwrap();
    let e = terse_net::Entry::article(bob().feed_key(), 1, T0, &article, None, "Bob on B").sign(&bob()).unwrap();
    assert_eq!(b.publish(&e.bytes), 1);
    let mut f = Fixture::new();
    f.feeds = vec![bob().feed_key()];
    f.relays = vec![url_a];
    f.hints = vec![(bob().feed_key(), url_b.strip_prefix("ws://").unwrap().to_string())];
    f.loopback_hints = true;
    let mut run = Run::new(&f, "10 press f\n");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !run.page_text().contains("@ through a relay its address names") {
        assert!(std::time::Instant::now() < deadline, "no mark: {}", run.page_text());
        run.frames(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(run.page_text().contains("@ through a relay its address names  ·  highest #1"), "{}", run.page_text());
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
    assert!(text.contains("This is a test article, published from the Tersenet reader"), "{text}");
    assert!(text.contains("[the manual]{1}"), "{text}");
    assert!(text.contains("Some shape\n") && text.contains("• and a figure:\n"), "{text}");
    assert!(text.replace('\n', " ").trim_end().ends_with("which opens when pressed."), "not cut: {text}");
    let figs = figures(&run);
    assert_eq!(figs.len(), 1, "{}", run.view());
    assert_eq!((figs[0].width, figs[0].height), (120, 32));
    all_drawn(&figs[0]);
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

    // Web addresses are ordinary text (ARTICLE.md): no anchors, and Tab
    // finds no link.
    run.press("pagedown");
    assert!(run.layout().lines.iter().flat_map(|l| &l.spans).all(|s| s.style.anchor == 0), "{}", run.page_text());
    assert!(run.page_text().contains("(https://en.wikipedia.org/wiki/Lactobacillus).\n"), "{}", run.page_text());
    run.press("tab");
    assert_eq!(run.status(), " No links in this article");
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
    assert!(run.page_text().contains(&format!("#9  ·  2026-11-17 19:30 UTC  ·  {:.1} KiB", size as f64 / 1024.0)), "{}", run.page_text());
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
    for i in 0..60 {
        text.push_str(&format!("terse:feed/{i:064x} "));
    }
    text.push_str(&"terse:".repeat(1000));
    text.push_str(&"http://a.example/ ".repeat(100));
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
    assert!(text.contains("[terse:feed/000000…00003b]{60}") && !text.contains("{61}"), "60 links marked: {text}");
    assert!(text.contains("http://a.example/ http://a.example/"), "not links: {text}");
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

/// 600 links: the first 512 are anchors in the link colour, the rest are
/// shown the same way but are not links.
#[test]
fn links_capped_at_512() {
    let mut f = Fixture::new();
    let text: String = (0..600).map(|i| format!("terse:feed/{i:064x} ")).collect();
    assert!(text.len() < 48 * 1024);
    f.publish(&alice(), "Very many links", text.as_bytes(), None);
    let mut run = Run::new(&f, "12 press enter\n");
    run.frames(80);
    let text = run.page_text();
    assert!(text.contains("[terse:feed/000000…0001ff]{512}") && !text.contains("{513}"), "512 links marked: {text}");
    assert!(text.contains(" terse:feed/000000…000257"), "{text}");
    assert!(run.results.iter().all(|r| r.waited));
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
    assert!(text.contains("Not found: neither this host nor its relays have this article."), "{text}");
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
    common::check_up_to_date(IMAGE, &["reader/reader.c"], true, "reader", RAM_PAGES, &[(2, 0), (4, 0), (7, 0), (10, 0)]);
}

/// An article with every construct of the article format (ARTICLE.md).
pub const FORMATTED_TITLE: &str = "Every construct";
pub const FORMATTED: &str = "\
# A heading
A paragraph right under the heading: its lines
join with a space,
   and blanks at the start of a line are dropped.

## A subheading

- A list item, whose text goes on
  on the next line, with a link terse:feed/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
  (a web address, https://example.com/item, is text).
* A second item
- A third item
and a lazy continuation of it.

    Indented by four spaces:
\tor by a tab, both preformatted.
A paragraph after the preformatted lines.

```
A fenced block keeps
   its lines

and its blank lines.
```

```path 160 80
fill #20304a stroke none M0 0 H160 V80 H0 Z
fill none stroke #ffcc66 2 M8 72 L40 30 72 50 104 12 152 20
stroke fg M8 8 v64 h144
fill #6aa84f80 stroke #6aa84f 1.5 m-32 -22 c10 0 15 5 15 15 s-5 15 -15 15 q-10 0 -12 -8 t2 -15 z
```

```python
print(\"a fence with an info string\")
```

# A heading that is a link: terse:feed/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
The end.
";

/// Opens the only article of a fixture with `text` (Enter in frame 12) and
/// runs until `frames` frames after that.
fn open_only(title: &str, text: &str, frames: u32) -> Run {
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    f.publish(&alice(), title, text.as_bytes(), None);
    let mut run = Run::new(&f, "12 press enter\n");
    run.frames(13 + frames);
    run
}

/// The figures of the page's document.
fn figures(run: &Run) -> Vec<terse_core::page::Figure> {
    let doc = run.machine.page().unwrap().document().unwrap();
    terse_core::page::parse(doc).into_iter().filter_map(|p| p.figure).collect()
}

/// Every path command of a figure is drawn: none is cut by the caps.
fn all_drawn(fig: &terse_core::page::Figure) {
    let (b, mut at, mut drawing) = (&fig.path, 0, 0);
    while at < b.len() {
        let n = match b[at] {
            1 => 4,
            2 => 6,
            3 => 4,
            4 => 4,
            5 => 8,
            6 => 12,
            7 => 0,
            op => panic!("opcode {op} at {at}"),
        };
        if (4..=7).contains(&b[at]) {
            drawing += 1;
        }
        at += 1 + n;
    }
    assert_eq!(at, b.len(), "whole commands");
    let drawn: usize = fig.paths().iter().map(|p| p.segments.iter().filter(|s| !matches!(s, terse_core::page::Segment::Move(..))).count()).sum();
    assert_eq!(drawn, drawing, "every segment drawn, within the caps");
}

/// Fences that name a language, coloured by the reader (its choice, not
/// the format's): C, Rust, Go, command lines, and fences that are not
/// coloured; a link in a comment stays a link.
const CODE: &str = "\
```c examples/program/c/hello.c
#include \"terse.h\"
/* a comment
   on two lines */
static uint8_t n = 0x2A; // see terse:feed/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
char c = '\\n';
```

```rust
#![no_std]
fn main() -> Result<(), String> {
    println!(\"{}\", 1.5); // a [link](terse:feed/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef)
    let s: &'static str = r\"raw\";
}
```

```go
package main
func f(b []byte) error { return nil }
var s = `a raw
string`
```

```sh
X=1 terse run a.terse | grep \"x\" # a comment
xxd -r -p > f <<EOF
5445
EOF
```

```text
int not_coloured;
```

A paragraph after them.
";

/// A page's lines with each run that is not in the plain code colour or
/// the body colour as `class[text]` (k keyword, t type, s string, n
/// number or meta, c comment, l link).
fn coloured_lines(run: &Run) -> String {
    let mut out = String::new();
    for line in &run.layout().lines {
        for span in &line.spans {
            let class = match span.style.rgb {
                [190, 205, 165] | [218, 218, 218] => "",
                [207, 162, 238] => "k",
                [124, 199, 196] => "t",
                [230, 176, 122] => "s",
                [240, 145, 155] => "n",
                [150, 150, 140] => "c",
                [110, 170, 255] => "l",
                _ => "?",
            };
            match class {
                "" => out.push_str(&span.text),
                c => out.push_str(&format!("{c}[{}]", span.text)),
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn article_code_colours() {
    let run = open_only("Code", CODE, 10);
    let got = coloured_lines(&run);
    golden("article-code", &got);
    assert!(got.contains("t[uint8_t ]") && got.contains("k[fn ]") && got.contains("k[func ]") && got.contains("k[terse ]"), "{got}");
    assert!(got.contains("int not_coloured;\nA paragraph after them.\n"), "{got}");
    let text = run.page_text();
    assert!(text.contains("// see [terse:feed/012345…abcdef]{1}\n"), "{text}");
    assert!(text.contains("// a [link]{2}\n"), "{text}");
}

#[test]
fn article_format() {
    let mut run = open_only(FORMATTED_TITLE, FORMATTED, 10);
    run.press("end");
    let view = run.view();
    golden("article-format", &view);
    let run = open_only(FORMATTED_TITLE, FORMATTED, 10);
    let top = run.view();
    golden("article-format-top", &top);
    assert!(top.contains("sans21b |A heading\n"), "{top}");
    assert!(top.contains("sans15 |A paragraph right under the heading: its lines join with a space,\n"), "{top}");
    assert!(top.contains("sans15 |and blanks at the start of a line are dropped.\n"), "{top}");
    assert!(top.contains("sans17b |A subheading\n"), "{top}");
    assert!(top.contains("   10   sans15 |• A list item, whose text goes on on the next line, with a link\n"), "{top}");
    assert!(top.contains("|• A third item and a lazy continuation of it.\n"), "{top}");
    assert!(top.contains("mono13 |    Indented by four spaces:\n"), "{top}");
    let text = run.page_text();
    assert!(text.contains("[terse:feed/012345…abcdef]{1} (a web address, https://example.com/item,\nis text).\n"), "{text}");
    assert!(text.contains("A fenced block keeps\n   its lines\n\nand its blank lines.\n"), "{text}");
    assert!(text.contains("print(\"a fence with an info string\")\n"), "{text}");
    assert!(!text.contains("```"), "{text}");
    assert!(text.contains("A heading that is a link:\n[terse:feed/012345…abcdef]{2}\nThe end.\n"), "{text}");
    let figs = figures(&run);
    assert_eq!(figs.len(), 1);
    assert_eq!((figs[0].width, figs[0].height), (160, 80));
    assert_eq!(figs[0].rgb, [218, 218, 218], "the initial stroke: the body text colour");
    all_drawn(&figs[0]);
    // The path's commands, byte for byte.
    let mut want: Vec<u8> = Vec::new();
    let pt = |w: &mut Vec<u8>, op: u8, xy: &[i32]| {
        w.push(op);
        for v in xy {
            w.extend_from_slice(&(*v as i16).to_le_bytes());
        }
    };
    want.extend([1, 0x20, 0x30, 0x4a, 255]);
    want.extend([2, 0, 0, 0, 0, 16, 0]);
    let s = |v: i32| v * 16;
    pt(&mut want, 3, &[0, 0]);
    pt(&mut want, 4, &[s(160), 0]);
    pt(&mut want, 4, &[s(160), s(80)]);
    pt(&mut want, 4, &[0, s(80)]);
    want.push(7);
    want.extend([1, 0, 0, 0, 0]);
    want.extend([2, 0xff, 0xcc, 0x66, 255, 32, 0]);
    pt(&mut want, 3, &[s(8), s(72)]);
    for (x, y) in [(40, 30), (72, 50), (104, 12), (152, 20)] {
        pt(&mut want, 4, &[s(x), s(y)]);
    }
    // "stroke fg" with no width after "stroke #ffcc66 2": 1 unit wide, not
    // the earlier 2 (ARTICLE.md section 6.1).
    want.extend([2, 218, 218, 218, 255, 16, 0]);
    pt(&mut want, 3, &[s(8), s(8)]);
    pt(&mut want, 4, &[s(8), s(72)]);
    pt(&mut want, 4, &[s(152), s(72)]);
    want.extend([1, 0x6a, 0xa8, 0x4f, 0x80]);
    want.extend([2, 0x6a, 0xa8, 0x4f, 255, 24, 0]);
    // m-32 -22 (from (152, 72)) c10 0 15 5 15 15 s-5 15 -15 15 q-10 0 -12 -8 t2 -15 z
    pt(&mut want, 3, &[s(120), s(50)]);
    pt(&mut want, 6, &[s(130), s(50), s(135), s(55), s(135), s(65)]);
    // s: the first control point reflects (135, 55) about (135, 65).
    pt(&mut want, 6, &[s(135), s(75), s(130), s(80), s(120), s(80)]);
    pt(&mut want, 5, &[s(110), s(80), s(108), s(72)]);
    // t: the control point reflects (110, 80) about (108, 72).
    pt(&mut want, 5, &[s(106), s(64), s(110), s(57)]);
    want.push(7);
    assert_eq!(figs[0].path, want);
}

/// Figures the reader cannot draw are shown as the text of their fence.
pub const MALFORMED: &str = "\
An arc:

```path 100 50
M10 10 A 5 5 0 0 1 20 20
```

An exponent, a colour of four digits, a number with no command, a point
out of range, a word with no colour, numbers left over:

```path 100 50
M1e2 0
```

```path 100 50
fill #abcd M0 0 L1 1
```

```path 100 50
10 10 M0 0
```

```path 100 50
M0 0 L2048 0
```

```path 100 50
M0 0 stroke
```

```path 100 50
M0 0 L1 1 2
```

Command letters with no numbers:

```path 100 50
M0 0 L Z
```

```path 100 50
M0 0 L
```

```path 100 50
M
```

A bad size, and a figure that is not closed:

```path 0 50
M0 0
```

```path 100 50
M0 0 L10 10
";

#[test]
fn malformed_figures() {
    let mut run = open_only("Malformed figures", MALFORMED, 10);
    assert!(figures(&run).is_empty(), "no figure");
    let text = run.page_text();
    for pre in ["M10 10 A 5 5 0 0 1 20 20", "M1e2 0", "fill #abcd M0 0 L1 1", "10 10 M0 0", "M0 0 L2048 0", "M0 0 stroke", "M0 0 L1 1 2", "M0 0 L Z", "M0 0 L\n", "M\n", "M0 0\n", "M0 0 L10 10"] {
        assert!(text.contains(pre), "{pre}: {text}");
    }
    run.press("end");
    golden("article-malformed", &run.view());
    assert!(run.view().contains("mono13 |M0 0 L10 10\n"), "the unclosed figure, as preformatted text");
}

/// Z after a word in the middle of a subpath: the word started a new
/// subpath at the current point (10, 10), so Z returns there, as the page
/// device's Close does (ARTICLE.md).
#[test]
fn close_after_a_word() {
    let text = "Z:\n\n```path 100 50\nM0 0 L10 10 stroke #f00 L20 20 Z l5 5\n```\n";
    let run = open_only("Close", text, 10);
    let figs = figures(&run);
    assert_eq!(figs.len(), 1, "{}", run.page_text());
    let p = &figs[0].path;
    let s = |v: i16| (v * 16).to_le_bytes();
    let mut tail = vec![7, 3];
    tail.extend(s(10));
    tail.extend(s(10));
    tail.push(4);
    tail.extend(s(15));
    tail.extend(s(15));
    assert!(p.ends_with(&tail), "{p:?}");
}

/// Z right after a word (no subpath open) does nothing and does not move
/// the current point: the next line starts a subpath at (10, 10), where
/// the last segment ended (ARTICLE.md 6.3).
#[test]
fn close_with_no_subpath_open() {
    let text = "Z:\n\n```path 50 50\nM0 0 L10 10 fill #f00 Z L20 20\n```\n";
    let run = open_only("Close", text, 10);
    let figs = figures(&run);
    assert_eq!(figs.len(), 1, "{}", run.page_text());
    let p = &figs[0].path;
    let s = |v: i16| (v * 16).to_le_bytes();
    let mut tail = vec![3];
    tail.extend(s(10));
    tail.extend(s(10));
    tail.push(4);
    tail.extend(s(20));
    tail.extend(s(20));
    assert!(p.ends_with(&tail), "{p:?}");
}

/// A figure fence with no lines (an empty path string) is an empty box
/// W x H, the same as one with only words (ARTICLE.md 2.4, 6.1).
#[test]
fn empty_figure() {
    let text = "Empty:\n\n```path 40 20\n```\n\n```path 40 20\nfill #fff stroke none\n```\n";
    let run = open_only("Empty", text, 10);
    let figs = figures(&run);
    assert_eq!(figs.len(), 2, "{}", run.page_text());
    for f in &figs {
        assert_eq!((f.width, f.height), (40, 20));
    }
}

/// Text without spaces (CJK) with bare links in it, long enough to be
/// built over several frames: wherever a frame's share ends, no link is
/// cut (each is found whole).
#[test]
fn links_in_text_without_spaces() {
    // Shifted by 0 to 29 bytes, so that the frames' shares end at many
    // places relative to the links.
    for shift in 0..30 {
        let mut text = "x".repeat(shift);
        for i in 0..400 {
            text.push_str(&"日本語".repeat(1 + i % 3));
            text.push_str(&format!("terse:feed/{i:064x}"));
        }
        text.push('\n');
        assert!(text.len() < 48 * 1024);
        let run = open_only("No spaces", &text, 60);
        assert!(run.submits.len() >= 2, "built over several frames: {:?}", run.submits);
        let layout = run.layout();
        let mut ids: Vec<u16> = layout.lines.iter().flat_map(|l| &l.spans).map(|s| s.style.anchor).filter(|&a| a != 0).collect();
        ids.dedup();
        assert_eq!(ids, (1..=400).collect::<Vec<u16>>(), "shift {shift}: every link whole");
    }
}

/// A figure wider than the column is scaled down to its width.
#[test]
fn wide_figure() {
    let text = "Wide:\n\n```path 1024 300\nM1024 300 L0 0 stroke #fff 3 h-0.5 V-1.03 Q1023.97 100 1000 299.99\n```\n";
    let run = open_only("A wide figure", text, 10);
    let view = run.view();
    golden("article-wide", &view);
    assert!(view.contains("-- figure 512x150 --"), "{view}");
    let figs = figures(&run);
    assert_eq!((figs[0].width, figs[0].height), (512, 150));
    all_drawn(&figs[0]);
    // Scaled by 512/1024 after rounding to sixteenths (halves away from
    // zero): -0.5 units is -8, so 0 - 8 = -8 sixteenths, -4 scaled;
    // -1.03 is -16.48, so -16, -8 scaled; 1023.97 is 16383.52, so 16384,
    // 8192 scaled; 299.99 is 4799.84, so 4800, 2400 scaled.
    // First the initial stroke, its width scaled: 1 unit, 8 sixteenths.
    let mut want = vec![2u8, 218, 218, 218, 255, 8, 0, 3];
    for v in [8192i16, 2400, 0, 0] {
        want.extend(v.to_le_bytes());
        if want.len() == 12 {
            want.push(4);
        }
    }
    want.extend([2, 255, 255, 255, 255, 24, 0]); // 3 units, 48 sixteenths, scaled
    want.push(3);
    want.extend([0i16, 0].iter().flat_map(|v| v.to_le_bytes()));
    want.push(4);
    want.extend([-4i16, 0].iter().flat_map(|v| v.to_le_bytes()));
    want.push(4);
    want.extend([-4i16, -8].iter().flat_map(|v| v.to_le_bytes()));
    want.push(5);
    want.extend([8192i16, 800, 8000, 2400].iter().flat_map(|v| v.to_le_bytes()));
    assert_eq!(figs[0].path, want);
}

/// A line chart of about 4 KB of path string (340 points with fractions).
fn chart_article() -> String {
    let mut path = String::from("fill #1d2a3a stroke none M0 0 H500 V260 H0 Z fill none stroke #8ab4f8 1.5 M10 130");
    for i in 1..340 {
        let y = 130 + ((i * 37) % 101) as i32 - 50;
        path.push_str(&format!(" {}.{} {}.{}", 10 + i * 14 / 10, i % 10, y, (i * 7) % 10));
    }
    format!("A chart:\n\n```path 500 260\n{path}\n```\n\nAfter the chart.\n")
}

/// The ~4 KB path: the busiest frame of opening it.
#[test]
fn figure_frames() {
    let text = chart_article();
    assert!((3800..4600).contains(&text.len()), "{}", text.len());
    let run = open_only("A chart", &text, 10);
    let figs = figures(&run);
    assert_eq!(figs.len(), 1);
    all_drawn(&figs[0]);
    assert!(run.page_text().ends_with("After the chart.\n"), "{}", run.page_text());
    let (worst, waited) = run.busiest();
    assert!(waited);
    let article: Vec<u32> = run.submits.iter().copied().filter(|&f| f > 12).collect();
    eprintln!(
        "reader: a {}-byte path string ({} bytes of commands): submits in frames {article:?} (Enter in 12), busiest frame {worst} units ({:.2}%)",
        text.len(),
        figs[0].path.len(),
        common::pct(worst)
    );
}

/// The costliest path strings, 40 KB of them in short figures and long
/// ones: built over many frames, none out of budget, none much over the
/// building share (the estimate is never below the real cost).
#[test]
fn figure_cost() {
    let mut text = String::new();
    // Many tiny figures.
    for i in 0..150 {
        text.push_str(&format!("```path 8 8\nM0 0 l1 1 z h{i}.5\n```\n"));
    }
    // Long ones: separators, numbers of many digits, words, relative
    // commands and reflections.
    let mut path = String::new();
    while path.len() < 7000 {
        path.push_str(",,  \n L0001.000001,-0.000001 q1 2 3 4 t 5 6 s1 2 3 4 c1 2 3 4 5 6 fill #123 stroke #12345678 0.5 m1 1 h1v1z\n");
    }
    for _ in 0..4 {
        text.push_str(&format!("```path 100 100\nM0 0 {path}```\n\n"));
    }
    // Malformed at the very end.
    text.push_str(&format!("```path 100 100\nM0 0 {path} A\n```\n"));
    assert!(text.len() > 36000, "{}", text.len());
    let run = open_only("Costly figures", &text, 60);
    let figs = figures(&run);
    assert_eq!(figs.len(), 154, "every well-formed figure");
    let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
    let building: Vec<u64> = used[13..].iter().copied().filter(|&u| u > 1000).collect();
    let worst = *used.iter().max().unwrap();
    eprintln!("reader: 36 KB of path strings: built in {} frames; busiest frame {worst} units ({:.2}%)", building.len(), common::pct(worst));
    assert!(run.results.iter().all(|r| r.waited), "{used:?}");
    assert!(worst < 90_000, "the estimate is honest: {used:?}");
    assert!(run.page_text().contains(" A\n"), "the malformed one as text");
}

/// Every kind of line in bulk (40 KB of it): no frame out of budget, none
/// much over the building share.
#[test]
fn line_kinds_frames() {
    let near = format!("terse:{}g terse:feed/{}g\n", "a".repeat(63), "a".repeat(63));
    let link = format!("terse:{} ", "a".repeat(64));
    for unit in ["  \t\n", "\n", "a\n\n", "- a\n", "# a\n", "    a\n", "a\n    b\n", "```\na\n```\n", &near, &link] {
        let run = open_only("Lines", &unit.repeat(40000 / unit.len()), 60);
        let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
        assert!(run.results.iter().all(|r| r.waited), "{unit:?}: {used:?}");
        assert!(*used.iter().max().unwrap() < 90_000, "{unit:?}: the estimate is honest: {used:?}");
    }
}

/// The articles `terse-host --demo` publishes (crates/terse-host/demo):
/// their figures are drawn whole, inside the column, and backquotes show
/// only where they are meant to be read (in prose, and the indented
/// sample).
#[test]
fn demo_articles() {
    let demos = [
        ("welcome", include_str!("../../terse-host/demo/welcome.txt"), 0, 1),
        ("long", include_str!("../../terse-host/demo/long.txt"), 0, 0),
        ("languages", include_str!("../../terse-host/demo/languages.txt"), 0, 0),
        ("pictures", include_str!("../../terse-host/demo/pictures.txt"), 3, 2),
    ];
    for (name, text, n, fences) in demos {
        let run = open_only(name, text, 10);
        let figs = figures(&run);
        assert_eq!(figs.len(), n, "{name}");
        for fig in &figs {
            assert!(fig.width <= RECT.2, "{name}: {}", fig.width);
            all_drawn(fig);
        }
        let page = run.page_text();
        assert_eq!(page.matches("```").count(), fences, "{name}: {page}");
        if name == "pictures" {
            golden("article-pictures", &run.view());
            assert!(page.contains("    ```path 120 60\n    fill #e06c75 stroke fg 2 M10 50 L60 10 110 50 Z\n    ```\n"), "{page}");
        }
        if name == "long" {
            assert!(run.view().contains("sans21b |The machine\n"), "{}", run.view());
        }
    }
}

/// CR LF line ends: the markers, fences and path strings work the same.
#[test]
fn crlf_article() {
    let text = "# Title\r\n- item\r\n\r\n```path 20 10\r\nM0 0 L20 10\r\n```\r\nAfter.\r\n";
    let run = open_only("CR LF", text, 10);
    let figs = figures(&run);
    assert_eq!(figs.len(), 1, "{}", run.view());
    assert_eq!(figs[0].path, [3, 0, 0, 0, 0, 4, 64, 1, 160, 0]);
    let view = run.view();
    assert!(view.contains("sans21b |Title\n") && view.contains("|• item\n") && view.contains("sans15 |After.\n"), "{view}");
}

/// ARTICLE.md 1: a byte order mark first is not shown and the first line's
/// marker counts (here a heading that is the title, so it is not shown
/// twice); a lone CR is a blank shown as a space, in a paragraph, a
/// preformatted line and a table cell, where it waits for the next byte
/// before it is decided.
#[test]
fn byte_order_mark_and_lone_cr() {
    let text = "\u{feff}# Notes\r\nA lone\rCR.\r\n\r\n    pre\rline\r\n\r\n| a\rb |\r\n";
    let run = open_only("Notes", text, 10);
    let view = run.view();
    assert!(view.contains("sans26b |Notes\n") && !view.contains("sans21b |Notes"), "{view}");
    assert!(!view.contains('\u{feff}'), "{view}");
    assert!(view.contains("|A lone CR.\n") && view.contains("|    pre line\n"), "{view}");
    let doc = run.machine.page().unwrap().document().unwrap();
    let cells: Vec<String> = terse_core::page::parse(doc)
        .iter()
        .filter_map(|p| p.table.as_ref())
        .flat_map(|t| t.cells.iter().map(|c| c.iter().map(|r| r.text.as_str()).collect::<String>()))
        .collect();
    assert_eq!(cells, ["a b"]);
}

// --- Names, search, links ------------------------------------------------

/// The short form of a blob ID (the title of an article known only by it).
fn short_id(id: &[u8; 32]) -> String {
    short(id)
}

/// The colour of the first span of anchor `id` on the page.
fn anchor_rgb(run: &Run, id: u16) -> [u8; 3] {
    run.layout().lines.iter().flat_map(|l| &l.spans).find(|s| s.style.anchor == id).unwrap().style.rgb
}

/// Alice has a listed profile "Alice", bob is "Bobby" by nickname, carol
/// (the host) has none.
fn named() -> Fixture {
    let mut f = Fixture::populated().with_identity(true);
    f.profile(&alice(), "Alice", true);
    f.nicknames.push((bob().feed_key(), "Bobby".into()));
    f
}

#[test]
fn names_in_timeline_and_feeds() {
    let f = named();
    let mut run = Run::new(&f, "");
    run.frames(10);
    golden("names-list", &run.view());
    let text = run.page_text();
    let (a, b, c) = (short(&alice().feed_key()), short(&bob().feed_key()), short(&carol().feed_key()));
    assert!(text.contains(&format!("[Alice ({a})  ·  2026-11-17 18:50")), "{text}");
    assert!(text.contains(&format!("[Bobby ({b})  ·  2026-11-17 19:40")), "{text}");
    assert!(text.contains(&format!("[{c}  ·  ")), "no name: the key alone: {text}");
    assert!(!text.contains("Alice  ·") && !text.contains("Bobby  ·"), "never a name without the key: {text}");
    run.press("f");
    run.frames(4);
    golden("names-feeds", &run.view());
    let text = run.page_text();
    assert!(text.contains(&format!("[Alice]{{2}}[  {a}]{{2}}")), "{text}");
    assert!(text.contains(&format!("[Bobby]{{1}}[  {b}]{{1}}")), "{text}");
    assert!(text.contains(&format!("[{c}]")) && text.contains("  you]"), "{text}");
    assert!(text.contains("[your nickname  ·  highest #"), "{text}");
    // The article's details line.
    run.press("esc");
    run.press("home");
    run.press("down");
    run.press("enter");
    run.frames(2);
    assert!(run.page_text().contains(&format!("Bobby ({b})  ·  #10")), "{}", run.page_text());
}

/// N in the feeds view: the nickname entry, allowed and refused, a
/// malformed one, and removing one.
#[test]
fn nickname_flow() {
    let f = named();
    // Key order: bob (0b51…), alice (66be…), carol.
    let mut run = Run::with_consent(&f, "10 press f\n12 press down\n14 press n\n", Consent::Allow);
    run.frames(16);
    assert!(row(&run.screen(), 28).starts_with(&format!(" Nickname for {}:", short(&alice().feed_key()))), "{}", run.screen());
    run.text(" Al");
    run.press("enter");
    assert_eq!(run.status(), " A nickname cannot start or end with a space");
    for _ in 0..3 {
        run.press("backspace");
    }
    run.text("Ally ✓");
    golden("nickname-entry", &run.view());
    run.press("enter");
    run.frames(3);
    assert_eq!(run.status(), format!(" Nickname for {} set to “Ally ✓”", short(&alice().feed_key())));
    assert!(row(&run.screen(), 28).trim().is_empty());
    let text = run.page_text();
    assert!(text.contains(&format!("[Ally ✓]{{2}}[  {}]{{2}}", short(&alice().feed_key()))), "{text}");
    // Empty removes it: the declared name is back.
    run.press("n");
    run.press("enter");
    run.frames(3);
    assert_eq!(run.status(), format!(" Nickname for {} removed", short(&alice().feed_key())));
    assert!(run.page_text().contains(&format!("[Alice]{{2}}[  {}]{{2}}", short(&alice().feed_key()))), "{}", run.page_text());
    // Too long.
    run.press("n");
    run.text(&"x".repeat(49));
    run.press("enter");
    assert_eq!(run.status(), " A nickname has at most 48 bytes of UTF-8; this one has 49");
    // Refused.
    let mut run = Run::with_consent(&f, "10 press f\n14 press n\n", Consent::Deny);
    run.frames(16);
    run.text("Nope");
    run.press("enter");
    run.frames(3);
    assert_eq!(run.status(), format!(" Nickname for {}: refused", short(&bob().feed_key())));
    assert!(run.page_text().contains(&format!("[Bobby]{{1}}[  {}]{{1}}", short(&bob().feed_key()))), "{}", run.page_text());
}

/// P in the feeds view: the public name, the listing question, allowed
/// (listed and not) and refused.
#[test]
fn public_name_flow() {
    let f = named();
    let mut run = Run::with_consent(&f, "10 press f\n14 press p\n", Consent::Allow);
    run.frames(16);
    assert!(row(&run.screen(), 28).starts_with(" Your public name:"), "{}", run.screen());
    run.text("Carol W");
    run.press("enter");
    golden("public-name-question", &run.view());
    assert_eq!(run.status(), " Let others find “Carol W” by search?   Y: yes   N: no   Esc: cancel");
    run.press("y");
    run.frames(4);
    assert!(run.status().starts_with(" Your public name is now “Carol W”, and others can find it by search (#"), "{}", run.status());
    let text = run.page_text();
    assert!(text.contains(&format!("[Carol W]{{3}}[  {}]{{3}}[  you]{{3}}", short(&carol().feed_key()))), "{text}");
    let feeds = terse_net::FeedStore::beside_blobs(f.store.path());
    let p = feeds.latest_profile(&carol().feed_key()).unwrap().unwrap();
    assert_eq!(p.entry.title, "Carol W");
    // N: not listed.
    run.press("p");
    run.text("Carol");
    run.press("enter");
    run.press("n");
    run.frames(4);
    assert!(run.status().starts_with(" Your public name is now “Carol”, not listed for search (#"), "{}", run.status());
    // Esc at the question: nothing.
    run.press("p");
    run.text("Carol");
    run.press("enter");
    run.press("esc");
    assert!(run.status().contains("S subscribe"), "{}", run.status());
    // A malformed name.
    run.press("p");
    run.press("enter");
    assert_eq!(run.status(), " Type the name others will see beside your feed key, or Esc to cancel");
    run.text("Carol ");
    run.press("enter");
    assert_eq!(run.status(), " A name cannot start or end with a space");
    // Refused.
    let mut run = Run::with_consent(&f, "10 press f\n14 press p\n", Consent::Deny);
    run.frames(16);
    run.text("Carol W");
    run.press("enter");
    run.press("y");
    run.frames(3);
    assert_eq!(run.status(), " Setting your public name was refused");
}

/// A relay serving a copy of the fixture's store, on an ephemeral port.
fn relay_for(f: &Fixture) -> (tempfile::TempDir, String) {
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &to.join(e.file_name()));
            } else {
                std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    copy(f.store.path(), dir.path());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let relay = std::sync::Arc::new(terse_relay::Relay::new(BlobStore::open(dir.path(), None).unwrap(), 1024));
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    (dir, url)
}

/// Bob gets a listed profile too ("Bob the baker"); carol is not
/// subscribed to bob.
fn searchable() -> (Fixture, tempfile::TempDir) {
    let mut f = named();
    f.profile(&bob(), "Bob the baker", true);
    f.feeds.retain(|k| *k != bob().feed_key());
    let (dir, url) = relay_for(&f);
    f.relays.push(url);
    (f, dir)
}

#[test]
fn search_finds_feeds_and_articles() {
    let (f, _relay) = searchable();
    let mut run = Run::with_consent(&f, "10 text /\n", Consent::Allow);
    run.frames(12);
    assert!(row(&run.screen(), 28).starts_with(" Search:"), "{}", run.screen());
    assert!(run.page_text().starts_with("Search\n"), "{}", run.page_text());
    // "b" matches bob's name and titles with a b.
    run.text("b");
    let enter = run.machine.frame();
    run.press("enter");
    run.frames(2);
    let view = run.view();
    golden("search", &view);
    let text = run.page_text();
    assert!(text.contains("Feeds\n1 found\n"), "{text}");
    // The name found, then the nickname bob has though not subscribed
    // (names records list it, flagged); bob's article under the nickname.
    let bob_row = 3 + text.lines().filter(|l| l.contains("   Alice (") || l.contains("   Bobby (") || l.contains("   carol")).count();
    assert!(
        text.contains(&format!("[Bob the baker]{{{bob_row}}}\n[{}  ·  your nickname: Bobby]{{{bob_row}}}", short(&bob().feed_key()))),
        "{text}"
    );
    assert!(text.contains("Articles on your relays\n"), "{text}");
    assert!(text.contains("[A big article (over 48 KiB)]"), "{text}");
    assert!(text.contains(&format!("[Bobby ({})  ·  2026-11-17 19:40  ·  6 bytes]", short(&bob().feed_key()))), "{text}");
    assert_eq!(run.status(), " 1 feed and 2 articles found on your relays");
    // The relays answered before this host's first match was found: the
    // feed is selected, and stays so as this host's results come in above.
    assert_eq!(run.selected(), "Bob the baker");
    // Results in frames: Enter's frame counted as 1.
    let first = run.submits.iter().copied().filter(|&s| s > enter).nth(0).unwrap();
    eprintln!("reader: search: results submitted {} frames after Enter (Enter's frame is 1)", first - enter + 1);
    // Down crosses the articles' heading; Up comes back.
    run.press("down");
    assert!(run.selected().starts_with("A big article") || !run.selected().is_empty());
    run.press("up");
    assert_eq!(run.selected(), "Bob the baker");
    // Enter: the feed's page; S subscribes.
    run.press("enter");
    let page = run.page_text();
    assert!(page.starts_with("Bobby\nyour nickname for the feed"), "{page}");
    assert!(page.contains("You are not subscribed to this feed."), "{page}");
    run.press("s");
    run.frames(4);
    assert_eq!(run.status(), format!(" Subscribed to {}", short(&bob().feed_key())));
    assert!(run.page_text().contains("You are subscribed to this feed."), "{}", run.page_text());
    run.press("esc");
    assert!(run.page_text().contains("subscribed]"), "the marker: {}", run.page_text());
    // An article result opens; Esc comes back to the search.
    run.press("down");
    run.press("enter");
    run.frames(3);
    assert!(!run.page_text().starts_with("Search"), "{}", run.page_text());
    run.press("esc");
    assert!(run.page_text().starts_with("Search\n"), "{}", run.page_text());
    let (worst, waited) = run.busiest();
    assert!(waited);
    eprintln!("reader: search, busiest frame {worst} units ({:.2}%)", common::pct(worst));
}

#[test]
fn search_without_results_relay_or_permission() {
    let (f, _relay) = searchable();
    let mut run = Run::new(&f, "10 text /\n");
    run.frames(12);
    run.text("zzz");
    run.press("enter");
    run.frames(2);
    assert_eq!(run.status(), " Nothing on your relays for “zzz” (only listed feeds can be found)");
    let text = run.page_text();
    assert!(text.contains("Feeds\nnone found\n") && text.contains("Articles on your relays\nnone found\n"), "{text}");
    golden("search-none", &run.view());
    // A third search within a second: the relay allows 4 finds a second.
    run.text("/");
    run.text("zzz");
    run.press("enter");
    run.frames(2);
    run.text("/");
    run.text("zzz");
    run.press("enter");
    run.frames(2);
    assert_eq!(run.status(), " Relay search refused (busy relays, or not on this host): your articles only");
    // No relay.
    let f = named();
    let mut run = Run::new(&f, "10 press f\n12 text /\n");
    run.frames(14);
    run.text("Alice");
    run.press("enter");
    run.frames(2);
    assert_eq!(run.status(), " No relay could be reached: your articles only");
    golden("search-no-relay", &run.view());
    run.press("esc");
    assert!(run.page_text().starts_with("Feeds\n"), "back where it came from: {}", run.page_text());
}

/// An article with links of every kind, published newest (so first).
fn linked(f: &mut Fixture) -> [u8; 32] {
    let sour = to_hex(&blob_id(SOURDOUGH.as_bytes()));
    let missing = to_hex(&blob_id(b"not anywhere"));
    let text = format!(
        "Links of every kind.\n\nThe sourdough article: terse:{sour}\n\nOne that is nowhere: terse:{missing}\n\n\
         Bob's feed: terse:feed/{}\n\nThe web is not linked: https://example.com/x\n\nNot links: terse:{}0 and xterse:{sour}\n",
        to_hex(&bob().feed_key()),
        &sour
    );
    f.publish(&alice(), "Links", text.as_bytes(), None);
    blob_id(text.as_bytes())
}

#[test]
fn tab_moves_through_links() {
    let mut f = named();
    linked(&mut f);
    let mut run = Run::new(&f, "12 press enter\n");
    run.frames(16);
    let text = run.page_text();
    // Bare terse: links shortened to their first and last six digits.
    let sour = blob_id(SOURDOUGH.as_bytes());
    assert!(text.contains(&format!("The sourdough article: [terse:{}]{{1}}\n", short(&sour))), "{text}");
    assert!(text.contains(&format!("Bob's feed: [terse:feed/{}]{{3}}\n", short(&bob().feed_key()))), "{text}");
    assert!(text.contains("The web is not linked: https://example.com/x\n"), "{text}");
    assert!(!text.contains("{4}"), "the near misses and the web address are not links: {text}");
    assert_eq!(anchor_rgb(&run, 1), [110, 170, 255]);
    run.press("tab");
    run.frames(1);
    assert_eq!(anchor_rgb(&run, 1), SELECTED, "the current link");
    assert_eq!(run.status(), format!(" Link 1: terse:{}  (an article)   Enter: open", short(&blob_id(SOURDOUGH.as_bytes()))));
    golden("article-tab", &run.view());
    let feed = format!(" Link 3: terse:feed/{}  (a feed)   Enter: open", short(&bob().feed_key()));
    run.press("tab");
    run.press("tab");
    assert_eq!(run.status(), feed);
    assert_eq!(anchor_rgb(&run, 3), SELECTED);
    assert_eq!(anchor_rgb(&run, 1), [110, 170, 255]);
    run.press("tab");
    assert!(run.status().starts_with(" Link 1: "), "wraps: {}", run.status());
    run.script_now("{f} press tab shift\n", 2);
    assert_eq!(run.status(), feed, "Shift+Tab goes back");
    // A press of another button on a link does nothing (the anchor
    // event's buttons, c bits 3-5); the primary button follows it.
    let layout = run.layout();
    let (line, span) =
        layout.lines.iter().find_map(|l| l.spans.iter().find(|s| s.style.anchor == 1).map(|s| (l, s))).unwrap();
    let r = run.machine.page().unwrap().region();
    let (x, y) = (r.x as u32 + span.x0 / 2 + 4, r.y as u32 + line.y + 4);
    run.script_now(&format!("{{f}} pointer {x} {y} 2\n{{f}} pointer {x} {y} 0\n"), 4);
    assert!(run.page_text().starts_with("Links\n"), "{}", run.page_text());
    run.click_anchor(3);
    assert!(run.page_text().starts_with("Bobby\n"), "a click follows the link: {}", run.page_text());
}

/// Links with text (ARTICLE.md): shown as their text in the link colour,
/// the target on the status row, followed and prefetched; near misses are
/// ordinary text, in which bare links count; a web address is no target
/// (nor a bare link), so "[the web](https://…)" is shown as written.
#[test]
fn links_with_text() {
    let mut f = named();
    let sour = to_hex(&blob_id(SOURDOUGH.as_bytes()));
    let bob_key = to_hex(&bob().feed_key());
    let feed = format!("terse:feed/{bob_key}");
    let (long, ok) = ("x".repeat(201), "y".repeat(200));
    let text = format!(
        "Links with text.\n\nAn [article](terse:{sour}), a [feed]({feed}) and [the web](https://example.com/a?b=1).\n\n\
         Misses: [two\nlines]({feed}) [](https://example.com/3) [a [b] c]({feed}) \
         [x] ({feed}) [y]({feed} ) [z](terse:{sour}0) [w](ftp://example.com/7) \
         [{long}]({feed}) [see {feed}]({feed})\n\n\
         [{ok}]({feed})\n\n    pre [text]({feed})\n\n\
         Brackets in prose [like this] (and this) stay.\n"
    );
    f.publish(&alice(), "Links with text", text.as_bytes(), None);
    let mut run = Run::new(&f, "12 press enter\n");
    run.frames(16);
    let page = run.page_text();
    assert!(page.contains("An [article]{1}, a [feed]{2} and [the web](https://example.com/a?b=1).\n"), "{page}");
    let anchors: Vec<(u16, String)> = {
        let layout = run.layout();
        let mut v: Vec<(u16, String)> = Vec::new();
        for s in layout.lines.iter().flat_map(|l| &l.spans).filter(|s| s.style.anchor != 0) {
            match v.last_mut() {
                Some(last) if last.0 == s.style.anchor => last.1.push_str(&s.text),
                _ => v.push((s.style.anchor, s.text.clone())),
            }
        }
        v
    };
    let texts: Vec<&str> = anchors.iter().map(|a| a.1.as_str()).collect();
    let bare = format!("terse:feed/{}", short(&bob().feed_key()));
    let want = ["article", "feed", &bare, &bare, &bare, &bare, &bare, &bare, &bare, &ok, "text"];
    assert_eq!(texts, want, "{page}");
    assert!(page.contains("[z](terse:") && page.contains("[w](ftp://example.com/7)"), "no link: {page}");
    assert!(page.contains("[](https://example.com/3)"), "no link: {page}");
    assert!(page.contains("Brackets in prose [like this] (and this) stay."), "{page}");
    assert_eq!(anchor_rgb(&run, 1), [110, 170, 255]);
    // The target shows on the status row; Enter follows it.
    run.press("tab");
    run.press("tab");
    run.press("tab");
    assert!(run.status().starts_with(" Link 3: terse:feed/"), "{}", run.status());
    run.script_now("{f} press tab shift\n", 2);
    assert_eq!(run.status(), format!(" Link 2: terse:feed/{}  (a feed)   Enter: open", short(&bob().feed_key())));
    run.script_now("{f} press tab shift\n", 2);
    assert_eq!(run.status(), format!(" Link 1: terse:{}  (an article)   Enter: open", short(&blob_id(SOURDOUGH.as_bytes()))));
    assert!(run.gets.iter().any(|g| g.1 == blob_id(SOURDOUGH.as_bytes()) && g.2 == 1), "prefetched: {:?}", run.gets);
    run.press("enter");
    run.frames(3);
    assert!(run.page_text().starts_with(&format!("{SOURDOUGH_TITLE}\n")), "{}", run.page_text());
}

/// A long article full of links with text, in paragraphs of 50 lines,
/// built over several frames: a frame's share never ends inside one (each
/// is found whole, whatever the cut).
#[test]
fn links_with_text_across_frames() {
    let text: String = (1..=320)
        .map(|i| {
            let end = if i % 40 == 0 { "\n\n" } else { "\n" };
            format!("Line {i:03} of a long article: [a link with text for line {i:03}](terse:feed/{i:064x}), done.{end}")
        })
        .collect();
    assert!(text.len() > 40 * 1024 && text.len() < 48 * 1024);
    let run = open_only("Many links", &text, 30);
    assert!(run.submits.len() >= 3, "built over several frames: {:?}", run.submits);
    let layout = run.layout();
    let spans: Vec<&terse_core::page::Span> = layout.lines.iter().flat_map(|l| &l.spans).collect();
    let anchored: Vec<u16> = spans.iter().filter(|s| s.style.anchor != 0).map(|s| s.style.anchor).collect();
    assert_eq!(anchored.iter().max(), Some(&320));
    for s in &spans {
        assert!(!s.text.contains("](") && !s.text.contains("[a link"), "a link with text cut: {:?}", s.text);
        if s.style.anchor != 0 {
            assert!(s.text.starts_with("a link with text for line "), "{:?}", s.text);
        }
    }
    assert_eq!(run.page_text().matches("for line").count(), 320);
}

/// A long title in a list ends in an ellipsis (paragraph flag, SPEC §page.document)
/// within the list's width.
#[test]
fn list_titles_end_in_an_ellipsis() {
    let f = Fixture::populated();
    let mut run = Run::new(&f, "");
    run.frames(10);
    let long = run.entries().into_iter().map(|e| e.0).find(|t| t.starts_with("A title long enough")).unwrap();
    assert!(long.ends_with(",…"), "{long}");
    assert!(long.chars().count() * 8 <= LIST.2 as usize, "{long}");
    assert!(run.entries().iter().any(|e| e.0 == "Keeping a sourdough starter alive"), "short titles are whole");
}

/// On a host of the machine's reading profile (only get, subscriptions,
/// timeline, subscribe, unsubscribe, identity and names; every other
/// content request completes with 05h) the reader starts and shows the
/// same views as on a full host, follows links, subscribes and opens the
/// manual; what needs an operation the host lacks ends in a short message
/// and leaves the reader free for the next request.
#[test]
fn reading_profile() {
    let mut f = named();
    linked(&mut f);
    terse_net::builtin::install_data(f.author.store()).unwrap();
    let script = "12 press enter\n";
    let mut full = Run::with_consent(&f, script, Consent::Allow);
    let mut run = Run::reading(&f, script, Consent::Allow);
    full.frames(16);
    run.frames(16);
    assert_eq!(run.view(), full.view(), "the same article");
    // Links: the feed's page.
    run.press("tab");
    run.press("tab");
    run.press("tab");
    run.press("enter");
    assert!(run.page_text().starts_with("Bobby\n"), "{}", run.page_text());
    run.press("esc");
    run.press("esc");
    // T: no put on this host.
    run.press("t");
    run.frames(2);
    assert_eq!(run.status(), " Could not store the test article: this host does not allow it");
    run.press("t");
    run.frames(2);
    assert_eq!(run.status(), " Could not store the test article: this host does not allow it", "not busy");
    // Search: refused.
    run.text("/");
    run.text("Alice");
    run.press("enter");
    run.frames(2);
    assert_eq!(run.status(), " Relay search refused (busy relays, or not on this host): your articles only");
    run.press("esc");
    // Nickname and public name: refused.
    run.press("f");
    run.press("n");
    run.text("Nope");
    run.press("enter");
    run.frames(2);
    assert_eq!(run.status(), format!(" Nickname for {}: refused", short(&bob().feed_key())));
    run.press("p");
    run.text("Carol");
    run.press("enter");
    run.press("y");
    run.frames(2);
    assert_eq!(run.status(), " Setting your public name was refused");
    // Unsubscribe (in the profile) still works; then the manual.
    run.press("u");
    run.frames(3);
    assert_eq!(run.status(), format!(" Unsubscribed from {}", short(&bob().feed_key())));
    run.press("esc");
    run.press("m");
    run.frames(6);
    assert!(run.view().contains("sans26b |Tersenet\n"), "{}", run.view());
    let (worst, waited) = run.busiest();
    assert!(waited, "every frame ended in wait_frame ({worst} units)");
}

/// Four links far apart, in paragraphs 3, 30, 55 and 58 of 60.
fn far_apart() -> Fixture {
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    let mut text = String::from("Links far apart\n\n");
    let at = [3, 30, 55, 58];
    for i in 1..=60 {
        text.push_str(&format!("Paragraph {i}. The quick brown fox jumps over the lazy dog, again and again, until the line wraps once or twice"));
        if let Some(k) = at.iter().position(|&a| a == i) {
            text.push_str(&format!(", and here is [link {}](terse:feed/{:064x})", k + 1, k + 1));
        }
        text.push_str(".\n\n");
    }
    f.publish(&alice(), "Links far apart", text.as_bytes(), None);
    f
}

/// A reader snapshotted in the frame where Tab asked the page where a link
/// is (the answer due in the next frame) and restored (pending locates are
/// not part of the state, STATE.md 4): it asks again when the restored page
/// is laid out, and scrolls to the link as the original did. Both a plain
/// Tab and a step of the search for the first link in view.
#[test]
fn locate_asked_again_after_restore() {
    let f = far_apart();
    let image = Image::parse(IMAGE).unwrap();
    for search in [false, true] {
        let mut run = Run::new(&f, "12 press enter\n");
        run.frames(20);
        run.press("tab");
        if search {
            run.press("end");
        }
        // The frame of the Tab: the locate is asked; snapshot after it.
        run.script_now("{f} press tab\n", 1);
        let snap = terse_core::Snapshot::of(&run.machine);
        assert!(snap.restore(&image).is_ok());
        let mut original = Run::new(&f, "");
        std::mem::swap(&mut original.machine, &mut run.machine);
        original.page = std::mem::take(&mut run.page);
        original.completions = std::mem::take(&mut run.completions);
        original.frames(10);
        let want = (original.scroll(), original.status());
        assert!(want.1.starts_with(if search { " Link 3: " } else { " Link 2: " }), "{want:?}");
        assert!(want.0 > 0, "scrolled to the link");
        // Restored: a new machine and a new page (no layout yet).
        run.machine = snap.restore(&image).unwrap();
        run.page = PageInput::default();
        run.frames(10);
        assert_eq!((run.scroll(), run.status()), want, "search: {search}");
    }
}

/// An article with links far apart: Tab and Shift+Tab scroll each just
/// into view (located by the page, SPEC §page.calls); after the user scrolls, Tab
/// starts from the first link in view, Shift+Tab from the last.
#[test]
fn tab_scrolls_links_into_view() {
    let f = far_apart();
    let mut run = Run::new(&f, "12 press enter\n");
    run.frames(20);
    const MARGIN: i32 = 8;
    let place = |run: &Run, id: u16| -> (i32, i32) {
        let (y, h) = run.layout().locate(id).unwrap();
        (y as i32, h as i32)
    };
    let in_view = |run: &Run, id: u16| {
        let (y, h) = place(run, id);
        let s = run.scroll();
        s <= y - MARGIN && y + h + MARGIN <= s + RECT.3 as i32
    };
    assert_eq!(run.scroll(), 0);
    assert!(in_view(&run, 1));
    // Tab: link 1 is in view already: no scrolling.
    run.press("tab");
    run.frames(1);
    assert!(run.status().starts_with(" Link 1: "), "{}", run.status());
    assert_eq!(run.scroll(), 0);
    // Tab: link 2 is far down: scrolled just enough to show its line.
    run.press("tab");
    run.frames(1);
    assert!(run.status().starts_with(" Link 2: "), "{}", run.status());
    let (y, h) = place(&run, 2);
    assert_eq!(run.scroll(), y + h + MARGIN - RECT.3 as i32, "just enough: the line at the bottom");
    golden("article-tab-far", &run.view());
    // Shift+Tab: back up, the line at the top.
    run.script_now("{f} press tab shift\n", 3);
    assert!(run.status().starts_with(" Link 1: "), "{}", run.status());
    assert_eq!(run.scroll(), place(&run, 1).0 - MARGIN);
    // End, then Tab: the first link at or below the top of the view (3),
    // not the next after the current one (2).
    run.press("end");
    let s = run.scroll();
    assert!(place(&run, 2).0 < s && place(&run, 3).0 >= s, "link 2 above the view, 3 in it");
    run.press("tab");
    run.frames(8);
    assert!(run.status().starts_with(" Link 3: "), "{}", run.status());
    assert!(in_view(&run, 3));
    // Home, then Shift+Tab: the last link above the bottom of the view (1).
    run.press("home");
    run.script_now("{f} press tab shift\n", 10);
    assert!(run.status().starts_with(" Link 1: "), "{}", run.status());
    assert_eq!(run.scroll(), 0);
    // Scrolled past link 2, two Tabs in consecutive frames: the first link
    // in view, then the next (the second press waits for the search).
    while run.scroll() <= place(&run, 2).0 {
        run.press("pagedown");
    }
    let s = run.scroll();
    let first = (1..=4).find(|&id| place(&run, id).0 >= s).unwrap();
    assert_eq!(first, 3, "simple order would give 2, then 3");
    run.script_now("{f} press tab\n", 1);
    run.script_now("{f} press tab\n", 10);
    assert!(run.status().starts_with(&format!(" Link {}: ", first + 1)), "{} (first in view {first}, scroll {s})", run.status());
    assert!(in_view(&run, first + 1));
    let (worst, waited) = run.busiest();
    assert!(waited);
    eprintln!("reader: Tab to links far apart, busiest frame {worst} units ({:.2}%)", common::pct(worst));
}

#[test]
fn following_links_and_back() {
    let mut f = named();
    linked(&mut f);
    let mut run = Run::new(&f, "12 press enter\n");
    run.frames(16);
    run.press("tab");
    run.press("enter");
    run.frames(2);
    let view = run.view();
    golden("article-followed", &view);
    assert!(run.page_text().starts_with(&format!("{SOURDOUGH_TITLE}\nAlice ({})  ·  #1", short(&alice().feed_key()))), "the timeline's title: {}", run.page_text());
    run.press("end");
    run.press("esc");
    run.frames(2);
    assert!(run.page_text().starts_with("Links\n"), "back: {}", run.page_text());
    // A missing article: its short hash as the title.
    run.press("tab");
    run.press("tab");
    run.press("enter");
    run.frames(2);
    let missing = blob_id(b"not anywhere");
    let view = run.view();
    golden("article-link-missing", &view);
    let text = run.page_text();
    assert!(text.starts_with(&format!("{}\nterse:{}  ·  not in your timeline", short_id(&missing), short_id(&missing))), "{text}");
    assert!(text.contains("Not found: neither this host nor its relays have this article."), "{text}");
    assert!(text.contains("Esc: back to the previous article"), "{text}");
    run.press("esc");
    run.frames(2);
    assert!(run.page_text().starts_with("Links\n"), "{}", run.page_text());
    // A feed link: the feed page; S subscribes; Esc back to the article.
    run.press("tab");
    run.press("tab");
    run.press("tab");
    run.press("enter");
    let view = run.view();
    golden("feed-page", &view);
    let text = run.page_text();
    assert!(text.starts_with(&format!("Bobby\nyour nickname for the feed  ·  {}", short(&bob().feed_key()))), "{text}");
    assert!(text.contains("You are subscribed to this feed."), "{text}");
    run.press("esc");
    run.frames(2);
    assert!(run.page_text().starts_with("Links\n"), "{}", run.page_text());
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n"), "{}", run.page_text());
    let (worst, waited) = run.busiest();
    assert!(waited);
    eprintln!("reader: following links, busiest frame {worst} units ({:.2}%)", common::pct(worst));
}

/// Ten articles, each linking to the next: the back stack keeps the last
/// eight, then Esc goes to the timeline. Every article's first eight links
/// are prefetched, at most two at a time.
#[test]
fn back_stack_and_prefetch() {
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    let mut texts = vec![String::from("The end of the chain.\n")];
    for i in (0..9).rev() {
        texts.insert(0, format!("Article {i}. Next: terse:{}\n", to_hex(&blob_id(texts[0].as_bytes()))));
    }
    for (i, t) in texts.iter().enumerate().rev() {
        f.publish(&alice(), &format!("Chain {i}"), t.as_bytes(), None);
    }
    let mut run = Run::new(&f, "12 press enter\n");
    run.frames(16);
    assert!(run.page_text().starts_with("Chain 0\n"), "{}", run.page_text());
    for i in 1..10 {
        run.press("tab");
        run.press("enter");
        run.frames(2);
        assert!(run.page_text().starts_with(&format!("Chain {i}\n")), "{i}: {}", run.page_text());
    }
    for i in (2..9).rev() {
        run.press("esc");
        run.frames(2);
        assert!(run.page_text().starts_with(&format!("Chain {i}\n")), "{i}: {}", run.page_text());
    }
    run.press("esc");
    run.frames(2);
    assert!(run.page_text().starts_with("Chain 1\n"), "{}", run.page_text());
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n"), "eight back, then the timeline: {}", run.page_text());
    // Prefetches: one-byte gets of each next article.
    let pre: Vec<&(u32, [u8; 32], u16)> = run.gets.iter().filter(|g| g.2 == 1).collect();
    assert!(pre.len() >= 9, "{pre:?}");
    assert!(run.most_pending <= 8);

    // Many links: only the first eight distinct ones, two at a time.
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    let ids: Vec<[u8; 32]> = (0..12).map(|i| blob_id(format!("elsewhere {i}").as_bytes())).collect();
    let mut text = String::from("Many links.\n\n");
    for id in ids.iter().chain(&ids[..3]) {
        text.push_str(&format!("terse:{}\n", to_hex(id)));
    }
    f.publish(&alice(), "Many", text.as_bytes(), None);
    let mut run = Run::new(&f, "12 press enter\n");
    run.frames(30);
    let pre: Vec<[u8; 32]> = run.gets.iter().filter(|g| g.2 == 1).map(|g| g.1).collect();
    assert_eq!(pre, ids[..8], "the first eight distinct links, in order");
    let mut per_frame = std::collections::HashMap::new();
    for g in run.gets.iter().filter(|g| g.2 == 1) {
        *per_frame.entry(g.0).or_insert(0) += 1;
    }
    assert!(per_frame.values().all(|&n| n <= 2), "{per_frame:?}");
}

/// A link to an article over the reader's 48 KiB: a page says so.
#[test]
fn link_to_a_large_article() {
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    f.publish(&alice(), "Big", big_article().as_bytes(), None);
    let text = format!("A link to a big one: terse:{}\n", to_hex(&blob_id(big_article().as_bytes())));
    f.publish(&alice(), "Pointer", text.as_bytes(), None);
    let mut run = Run::new(&f, "12 press enter\n");
    run.frames(16);
    run.press("tab");
    run.press("enter");
    run.frames(3);
    let text = run.page_text();
    assert!(text.starts_with("Big\n"), "{text}");
    assert!(text.contains("Too large: this article is 50.8 KiB, and this reader shows articles\nof up to 48 KiB."), "{text}");
}

impl Fixture {
    /// Publishes a document (entry kind 4) into the store.
    fn publish_document(&mut self, who: &Identity, title: &str, data: &[u8]) {
        self.author.set_identity(who.clone());
        self.time += 600;
        self.author.start_publish_document(0, data, title, self.time);
        loop {
            if let Some((_, done)) = self.author.poll().into_iter().find(|(t, _)| *t == 0) {
                assert!(matches!(done, terse_net::Done::Published { .. }), "{done:?}");
                return;
            }
        }
    }

    /// Puts a blob into the store (no entry).
    fn put(&mut self, data: &[u8]) -> [u8; 32] {
        assert!(!matches!(self.author.publish(data), terse_net::Done::Failed));
        blob_id(data)
    }
}

/// A document blob of format `format` (a built-in program's: owner key 0).
fn document(format: &str, data: &[u8]) -> Vec<u8> {
    let mut d = b"TDOC\0".to_vec();
    d.extend_from_slice(&[0; 32]);
    d.push(format.len() as u8);
    d.extend_from_slice(format.as_bytes());
    d.extend_from_slice(data);
    d
}

/// The open request the machine made: its kind and the rest.
fn open_request(run: &Run) -> Option<(u8, Vec<u8>)> {
    run.machine.open_request().map(|(b, _)| (b[0], b[1..].to_vec()))
}

/// Started with an argument (a blob ID from the shell): the reader shows
/// that article at once, titled by its timeline entry; Esc goes to the
/// timeline. An ID the timeline does not have is titled by its short form.
#[test]
fn argument_opens_an_article() {
    let mut f = Fixture::populated();
    let elsewhere = f.put(b"An article without an entry.\n");
    let mut run = Run::new(&f, "");
    run.machine.set_argument(Some(blob_id(TEA.as_bytes())));
    run.frames(6);
    let text = run.page_text();
    assert!(text.starts_with(&format!("{TEA_TITLE}\n{}  ·  #", short(&bob().feed_key()))), "the entry's title: {text}");
    assert!(text.contains("Tea is drunk almost everywhere"), "{text}");
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n"), "{}", run.page_text());

    let mut run = Run::new(&f, "");
    run.machine.set_argument(Some(elsewhere));
    run.frames(6);
    let text = run.page_text();
    assert!(text.starts_with(&format!("{}\nterse:{}  ·  not in your timeline", short_id(&elsewhere), short_id(&elsewhere))), "{text}");
    assert!(text.contains("An article without an entry."), "{text}");
    let (worst, waited) = run.busiest();
    assert!(waited);
    eprintln!("reader: an argument, busiest frame {worst} units ({:.2}%)", common::pct(worst));

    // A document as the argument: passed back to the shell; the timeline.
    let doc = f.put(&document("sketch", b"lines"));
    let mut run = Run::new(&f, "");
    run.machine.set_argument(Some(doc));
    run.frames(6);
    assert!(run.page_text().starts_with("Timeline\n"), "{}", run.page_text());
    assert_eq!(run.status(), " This needs the shell: run terse-host with the link, or with no program");
}

/// O opens a link the user types or pastes (blanks and line breaks left
/// out): an article by its ID (Esc goes back to where O was pressed), a
/// feed's page with the hints of its address, a program asked of the
/// shell; a malformed link is named as such, and one that cannot be
/// fetched says so.
#[test]
fn open_a_pasted_link() {
    let mut f = Fixture::populated();
    let id = f.put(b"# Given by a friend\n\nPasted into the reader.\n");
    let mut run = Run::new(&f, "10 press o\n");
    run.machine.set_in_system(true);
    run.frames(12);
    assert!(row(&run.screen(), 28).starts_with(" Link:"), "{}", row(&run.screen(), 28));
    assert_eq!(run.status(), " Paste or type a terse: link (article, feed, program)  Enter: open  Esc: cancel");
    // A paste's blanks are left out.
    run.text(&format!("terse: {}", grouped(&id)));
    assert_eq!(row(&run.screen(), 28).trim_end(), format!(" Link: terse:{}", to_hex(&id)));
    run.press("enter");
    run.frames(10);
    assert!(run.page_text().contains("Pasted into the reader."), "{}", run.page_text());
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n"), "back where O was pressed: {}", run.page_text());

    // Not links.
    for (typed, said) in [
        ("", " Type or paste a terse: link, or Esc to cancel"),
        ("https://example.com", " Not a link: a Tersenet link begins with terse:"),
        ("terse:1234", " Not a link: after terse: comes an article ID, feed/key or program/key/name"),
        ("terse:feed/abc", " Not a feed link: terse:feed/ is followed by a key, 64 hex digits"),
        (&format!("terse:feed/{}@a_b.example", to_hex(&bob().feed_key())), " Not a feed address: after @ give one or two relays, host or host:port"),
        (&format!("terse:program/{}/", to_hex(&bob().feed_key())), " Not a program link: after the key's / comes the program's name"),
        ("terse:program/abc/x", " Not a program link: after program/ come a key, 64 hex digits, and /"),
        (&format!("terse:program/{}/a%zz", to_hex(&bob().feed_key())), " Not a program link: after the key's / comes the program's name"),
    ] {
        run.press("o");
        run.text(typed);
        run.press("enter");
        assert_eq!(run.status(), said, "{typed}");
        run.press("esc");
    }

    // A feed: its page, with the address's hints.
    let carol = Identity::from_seed([99; 32]).feed_key();
    run.press("o");
    run.text(&format!("terse:feed/{}@relay.example", to_hex(&carol)));
    run.press("enter");
    run.frames(2);
    assert_eq!(run.status(), " S subscribe  Esc back");
    assert!(run.page_text().contains("relay.example"), "the address's hint: {}", run.page_text());
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n"), "{}", run.page_text());

    // A program: asked of the shell.
    run.press("o");
    run.text(&format!("terse:program/{}/Sketch%20pad", to_hex(&alice().feed_key())));
    run.press("enter");
    let mut want = alice().feed_key().to_vec();
    want.push(10);
    want.extend_from_slice(b"Sketch pad");
    want.resize(81, 0);
    assert_eq!(open_request(&run), Some((2, want)));
    assert_eq!(run.status(), " Asked the shell to open it");

    // An article nobody has.
    run.press("o");
    run.text(&format!("terse:{}", to_hex(&[7; 32])));
    run.press("enter");
    run.frames(10);
    assert!(run.page_text().contains("Not found: neither this host nor its relays have this article."), "{}", run.page_text());
}

/// A program's key and a name with a space and a non-ASCII character,
/// percent-encoded.
fn program_links(f: &mut Fixture) -> String {
    let key = to_hex(&alice().feed_key());
    let text = format!(
        "Programs.\n\nA bare one: terse:program/{key}/Sketch%20pad%20%C3%a9 and with text: \
         [the pad](terse:program/{key}/Sketch%20pad%20%c3%A9). Then one at a sentence's end: terse:program/{key}/Pad.\n"
    );
    f.publish(&alice(), "Programs", text.as_bytes(), None);
    key
}

/// A program link: shown with its name decoded, the status row names the
/// program and its feed, Enter asks the shell (OPEN_PROGRAM) and the
/// article stays; without a shell the status row says so.
#[test]
fn program_links_ask_the_shell() {
    let mut f = named();
    let key = program_links(&mut f);
    let mut run = Run::new(&f, "12 press enter\n");
    run.machine.set_in_system(true);
    run.frames(16);
    let text = run.page_text().replace('\n', " ");
    let shown = format!("terse:program/{}…{}/Sketch pad é", &key[..6], &key[58..]);
    assert!(text.contains(&format!("A bare one: [{shown}]{{1}} and with text: [the pad]{{2}}.")), "{text}");
    assert!(text.contains(&format!("[terse:program/{}…{}/Pad]{{3}}.", &key[..6], &key[58..])), "the trailing dot is not the link's: {text}");
    run.press("tab");
    assert_eq!(run.status(), format!(" Link 1: program “Sketch pad é” by Alice ({})   Enter: open", short(&alice().feed_key())));
    run.press("enter");
    let mut want = alice().feed_key().to_vec();
    want.push("Sketch pad é".len() as u8);
    want.extend_from_slice("Sketch pad é".as_bytes());
    want.resize(81, 0);
    assert_eq!(open_request(&run), Some((2, want)));
    assert_eq!(run.status(), " Asked the shell to open it");
    assert!(run.page_text().starts_with("Programs\n"), "the article stays: {}", run.page_text());
    assert!(!run.gets.iter().any(|g| g.2 == 1), "program links are not prefetched: {:?}", run.gets);
    // The link with text: the same request; the third: "Pad".
    run.machine.clear_open_request();
    run.press("tab");
    run.press("tab");
    run.press("enter");
    let (kind, rest) = open_request(&run).unwrap();
    assert_eq!((kind, &rest[..32], rest[32], &rest[33..36]), (2, &alice().feed_key()[..], 3, &b"Pad"[..]));
    // Standalone (no shell).
    let mut run = Run::new(&f, "12 press enter\n");
    run.frames(16);
    run.press("tab");
    run.press("enter");
    assert_eq!(run.status(), " This needs the shell: run terse-host with the link, or with no program");
    assert!(open_request(&run).is_none());
    assert!(run.page_text().starts_with("Programs\n"));
}

/// Program links that are not: the key not 64 lowercase hex digits, a
/// name that is empty, starts with a bad escape, is not UTF-8, holds a
/// control character, starts with a space or is over 48 bytes.
#[test]
fn malformed_program_links() {
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    let key = to_hex(&alice().feed_key());
    let bad = [
        format!("terse:program/{}/Name", key.to_uppercase()),
        format!("terse:program/{}/Name", &key[1..]),
        format!("terse:program/{key}Name"),
        format!("terse:program/{key}/"),
        format!("terse:program/{key}/%G1name"),
        format!("terse:program/{key}/%C3"),
        format!("terse:program/{key}/a%0Ab"),
        format!("terse:program/{key}/%20name"),
        format!("terse:program/{key}/name%20"),
        format!("terse:program/{key}/{}", "x".repeat(49)),
        format!("terse:program/{key}/{}", "%C3%A9".repeat(25)),
    ];
    let text = format!("Not links.\n\n{}\n", bad.iter().map(|b| format!("{b} .")).collect::<Vec<_>>().join("\n\n"));
    let run = open_only("Not links", &text, 16);
    let page = run.page_text().replace('\n', "");
    assert!(!page.contains("{1}"), "no links: {page}");
    for b in &bad {
        assert!(page.contains(b.as_str()), "{b} as text: {page}");
    }
    // 48 bytes is a name.
    let ok = format!("terse:program/{key}/{}", "%C3%A9".repeat(24));
    let run = open_only("A link", &format!("{ok}\n"), 16);
    assert!(run.page_text().contains(&format!("/{}]{{1}}", "é".repeat(24))), "{}", run.page_text());
}

/// A terse: link to a program image or a document: the first bytes are
/// got, the shell asked to open the blob (OPEN_BLOB) and nothing of it is
/// shown: the article stays.
#[test]
fn links_to_images_and_documents() {
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    let image = f.put(b"TERS\x01not really an image, but its magic");
    let doc = f.put(&document("sketch", b"some lines"));
    let text = format!("Two links.\n\nAn image: terse:{}\n\nA [document](terse:{}).\n", to_hex(&image), to_hex(&doc));
    f.publish(&alice(), "Two links", text.as_bytes(), None);
    let mut run = Run::new(&f, "12 press enter\n");
    run.machine.set_in_system(true);
    run.frames(16);
    run.press("tab");
    run.press("enter");
    run.frames(2);
    assert_eq!(open_request(&run), Some((1, [&image[..], &[0; 49][..]].concat())));
    assert_eq!(run.status(), " Asked the shell to open it");
    assert!(run.page_text().starts_with("Two links\n"), "{}", run.page_text());
    assert!(run.gets.iter().any(|g| g.1 == image && g.2 == 4), "the first bytes: {:?}", run.gets);
    run.machine.clear_open_request();
    run.press("tab");
    run.press("enter");
    run.frames(2);
    assert_eq!(open_request(&run), Some((1, [&doc[..], &[0; 49][..]].concat())));
    assert!(run.page_text().starts_with("Two links\n"), "{}", run.page_text());
    assert!(!run.page_text().contains("some lines"));
    // Esc: the timeline (nothing went on the back stack).
    run.press("esc");
    assert!(run.page_text().starts_with("Timeline\n"), "{}", run.page_text());
    let (worst, waited) = run.busiest();
    assert!(waited);
    eprintln!("reader: links to images and documents, busiest frame {worst} units ({:.2}%)", common::pct(worst));
}

/// A document's entry in the timeline: a mark before its title, its
/// format name (from the blob's head) on the details line; Enter or a
/// click asks the shell to open it.
#[test]
fn documents_in_the_timeline() {
    let mut f = named();
    let data = document("sketch", b"strokes");
    f.publish_document(&alice(), "My drawing", &data);
    let mut run = Run::new(&f, "");
    run.machine.set_in_system(true);
    run.frames(12);
    let entries = run.entries();
    assert_eq!(entries[0].0, "◆ My drawing", "{entries:?}");
    let text = run.page_text();
    assert!(text.contains(&format!("[◆ My drawing]{{1}}\n[sketch document  ·  Alice ({})  ·  ", short(&alice().feed_key()))), "{text}");
    assert!(run.gets.iter().any(|g| g.1 == blob_id(&data) && g.2 == 86), "the head: {:?}", run.gets);
    golden("timeline-document", &run.view());
    run.press("enter");
    assert_eq!(open_request(&run), Some((1, [&blob_id(&data)[..], &[0; 49][..]].concat())));
    assert_eq!(run.status(), " Asked the shell to open it");
    assert!(run.page_text().starts_with("Timeline\n"));
    run.machine.clear_open_request();
    run.click_anchor(1);
    assert!(open_request(&run).is_some_and(|r| r.0 == 1), "a click too");
    let (worst, waited) = run.busiest();
    assert!(waited);
    eprintln!("reader: documents in the timeline, busiest frame {worst} units ({:.2}%)", common::pct(worst));
}

/// The runs of a cell or paragraph as (text, face, bold, anchor).
fn runs(r: &[terse_core::page::Run]) -> Vec<(String, u8, bool, u16)> {
    r.iter().map(|r| (r.text.clone(), r.style.face, r.style.bold, r.style.anchor)).collect()
}

const TABLES: &str = "\
A paragraph with `code` and `two spans`. A split `code
span` is none.
Two together `` are none.
But `x` is one.

| Key | What | Notes |
|:--|:-:|--:|
| `M` | the manual | [the index](terse:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa) |
| a \\| b | `x \\| y` | [e \\| f](terse:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa) | dropped |
|short
 | not a table: a paragraph

|---|---|
| no | header |\\|

# A heading with `code`
";

/// Tables and code spans (ARTICLE.md): cells, escapes, alignment rows,
/// ragged rows, near misses.
#[test]
fn tables_and_code_spans() {
    use terse_core::page::{Align, parse};
    let run = open_only("Tables", TABLES, 10);
    golden("article-tables", &run.view());
    let doc = run.machine.page().unwrap().document().unwrap();
    let ps = parse(doc);
    let text: Vec<_> = ps.iter().filter(|p| p.has_text()).map(|p| runs(&p.runs)).collect();
    // The title is the entry's; then the details line, then the paragraph.
    let para = &text[2];
    let mono: Vec<&str> = para.iter().filter(|r| r.1 == 2).map(|r| r.0.as_str()).collect();
    assert_eq!(mono, ["code", "two spans", "x"], "{para:?}");
    let plain: String = para.iter().filter(|r| r.1 == 0).map(|r| r.0.as_str()).collect();
    assert!(plain.contains("A split `code span` is none. Two together `` are none. But "), "{plain}");
    let tables: Vec<_> = ps.iter().filter_map(|p| p.table.as_ref()).collect();
    assert_eq!(tables.len(), 2);
    let t = tables[0];
    assert_eq!((t.columns, t.header, t.align.clone()), (3, true, vec![Align::Start, Align::Centre, Align::End]));
    let cells: Vec<_> = t.cells.iter().map(|c| runs(c)).collect();
    assert_eq!(cells.len(), 12);
    assert_eq!(cells[0], [("Key".to_string(), 0, true, 0)]);
    assert_eq!(cells[3], [("M".to_string(), 2, false, 0)]);
    assert_eq!(cells[5], [("the index".to_string(), 0, false, 1)], "a link in a cell");
    assert_eq!(cells[6], [("a | b".to_string(), 0, false, 0)], "an escaped bar");
    assert_eq!(cells[7], [("x | y".to_string(), 2, false, 0)], "an escaped bar in a code span");
    assert_eq!(cells[8], [("e | f".to_string(), 0, false, 2)], "a link with text whose text has an escaped bar");
    assert_eq!(cells[9], [("short".to_string(), 0, false, 0)]);
    assert!(cells[10].is_empty() && cells[11].is_empty(), "a short row has empty cells");
    assert!(text.iter().any(|p| p.iter().map(|r| r.0.as_str()).collect::<String>() == "| not a table: a paragraph"));
    // No alignment row: "|---|---|" is the first row; "\|" at the end is a
    // cell's text, not its end.
    let t = tables[1];
    assert_eq!((t.columns, t.header), (2, false));
    let cells: Vec<String> = t.cells.iter().map(|c| c.iter().map(|r| r.text.as_str()).collect()).collect();
    assert_eq!(cells, ["---", "---", "no", "header"]);
    let heading = text.last().unwrap();
    assert_eq!(heading.iter().map(|r| (r.0.as_str(), r.1)).collect::<Vec<_>>(), [("A heading with ", 0), ("code", 2)]);
}

/// A table-heavy 48 KiB article: built over frames within the budget.
#[test]
fn table_frames() {
    let row = "| `0Ah` | Cell | starts the next cell, in row order; see [the page](terse:feed/00000000000000000000000000000000000000000000000000000000000000aa) |\n";
    let mut text = String::new();
    let mut n = 0;
    while text.len() < 48 * 1024 - 400 {
        if n % 20 == 0 {
            text.push_str("\n| Opcode | Command | Effect |\n|---|---|---|\n");
        }
        text.push_str(row);
        n += 1;
    }
    let run = open_only("Tables", &text, 60);
    let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
    let worst = *used.iter().max().unwrap();
    let doc = run.machine.page().unwrap().document().unwrap();
    let cells: usize = terse_core::page::parse(doc).iter().filter_map(|p| p.table.as_ref()).map(|t| t.cells.len()).sum();
    eprintln!(
        "reader: a {} byte article of tables ({cells} cells in the document, {} bytes): busiest frame {worst} units ({:.1}%)",
        text.len(),
        doc.len(),
        common::pct(worst)
    );
    assert!(run.results.iter().all(|r| r.waited), "{used:?}");
    assert!(worst < 90_000, "the estimate is honest: {used:?}");
    assert!(cells > 1_000);
    // Lines of only bars and cells of only blanks or escapes.
    // And cells of one long word (the costliest bytes to cut into cells).
    let long = format!("| {} | {} |\n", "x".repeat(80), "y".repeat(60));
    for unit in ["|\n", "||||||||||\n", "|   |   |   |\n", "| \\| | \\\\|\n", "| `a` | `b` |\n", &long] {
        let run = open_only("Bars", &unit.repeat(40000 / unit.len()), 60);
        let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
        assert!(run.results.iter().all(|r| r.waited), "{unit:?}: {used:?}");
        assert!(*used.iter().max().unwrap() < 90_000, "{unit:?}: the estimate is honest: {used:?}");
    }
    // Code spans in prose.
    let run = open_only("Code", &"some `code` and `more` text ".repeat(1500), 60);
    let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
    assert!(*used.iter().max().unwrap() < 90_000, "code spans: {used:?}");
    // terse: links in prose, bare and with text, of every kind.
    let key = "0123456789abcdef".repeat(4);
    for unit in [
        format!("see terse:feed/{key} and "),
        format!("see [the page](terse:feed/{key}) and "),
        format!("[a](terse:{key}) "),
        format!("terse:program/{key}/Sketch%20pad "),
    ] {
        let run = open_only("Links", &unit.repeat(40000 / unit.len()), 80);
        let used: Vec<u64> = run.results.iter().map(|r| r.used).collect();
        assert!(run.results.iter().all(|r| r.waited), "{unit:?}: {used:?}");
        eprintln!("reader: links in prose {unit:?}: busiest frame {} units", used.iter().max().unwrap());
        assert!(*used.iter().max().unwrap() < 90_000, "{unit:?}: the estimate is honest: {used:?}");
    }
}

/// M on the timeline opens the manual's index, which every host puts in
/// its store; the index's heading is its title, not shown twice.
#[test]
fn manual_key_opens_the_index() {
    let mut f = Fixture::new();
    f.feeds.clear();
    terse_net::builtin::install_data(f.author.store()).unwrap();
    let mut run = Run::new(&f, "12 press m\n");
    run.frames(20);
    let view = run.view();
    golden("manual-index", &view);
    assert!(view.contains("sans26b |Tersenet\n"), "{view}");
    assert!(!view.contains("sans21b |Tersenet\n"), "the heading is the title: {view}");
    assert!(view.contains("[Install and try Tersenet]"), "{view}");
    assert!(view.contains("[Write your own Tersenet reader]"), "{view}");
    assert!(view.contains("[Contents]"), "{view}");
    // Following the first link opens the user guide's first page; Esc comes back.
    run.press("tab");
    run.press("enter");
    run.frames(6);
    assert!(run.view().contains("sans26b |Install Tersenet\n"), "{}", run.view());
    run.press("esc");
    run.frames(4);
    assert!(run.view().contains("sans26b |Tersenet\n"));
}

/// The reader holds the ID of the manual's current index: a change of the
/// manual without a rebuild of the reader fails here (no compiler needed).
#[test]
fn reader_knows_the_manual_index() {
    let (name, _, text) = terse_net::builtin::MANUAL[0];
    assert_eq!(name, "index");
    let id = blob_id(text);
    let rom = &Image::parse(IMAGE).unwrap().rom;
    assert!(rom.windows(32).any(|w| w == id), "programs/reader/reader.terse is stale: run programs/reader/build.sh");
    let header = include_str!("../../../programs/reader/manual_id.h");
    let bytes: String = id.iter().map(|b| format!("0x{b:02x}")).collect::<Vec<_>>().join(", ");
    assert!(header.contains(&bytes), "programs/reader/manual_id.h is stale: run programs/reader/build.sh");
}

/// Every article of the built manual is shown whole by the reader: each
/// ```path fence is a figure drawn in full within the column, each table
/// is a table (never preformatted or paragraph text), and nothing is cut.
#[test]
fn manual_articles_are_valid() {
    for &(name, title, text) in terse_net::builtin::MANUAL {
        let text = std::str::from_utf8(text).unwrap();
        let run = open_only(title, text, 20);
        let doc = run.machine.page().unwrap().document().unwrap();
        let ps = terse_core::page::parse(doc);
        let figs: Vec<_> = ps.iter().filter_map(|p| p.figure.as_ref()).collect();
        let tables = ps.iter().filter(|p| p.table.is_some()).count();
        let (mut want_figs, mut want_tables, mut in_fence, mut prev_bar) = (0, 0, false, false);
        for line in text.lines() {
            if line.starts_with("```") {
                if !in_fence && line.starts_with("```path") {
                    want_figs += 1;
                }
                in_fence = !in_fence;
                prev_bar = false;
                continue;
            }
            let bar = !in_fence && line.starts_with('|');
            if bar && !prev_bar {
                want_tables += 1;
            }
            prev_bar = bar;
        }
        assert_eq!(figs.len(), want_figs, "{name}: every figure is drawn");
        assert_eq!(tables, want_tables, "{name}: every table is a table");
        for fig in figs {
            assert!(fig.width <= RECT.2, "{name}: {}", fig.width);
            all_drawn(fig);
        }
        let page = run.page_text();
        assert!(!page.lines().any(|l| l.starts_with("```")), "{name}: no fence shown as text: {page}");
        assert!(!page.contains("the article continues"), "{name}: shown whole");
        let flat = page.replace('\n', " ");
        assert!(flat.starts_with(&format!("{title} ")), "{name}: titled {title:?}");
        // (The title's words may well recur in the text; the heading
        // itself, in the heading style, must not.)
        assert!(!run.view().contains(&format!("sans21b |{title}\n")), "{name}: the heading is not repeated");
        assert!(text.len() <= 16_384, "{name}: under the automatic download limit");
    }
}


// --- Searching the articles on this host ------------------------------------

/// Prose of about `len` bytes (paragraphs of hand-wrapped lines, words from a
/// fixed list), different for each `seed`; never holds the words the search
/// tests look for ("zebra", "boundary", "köln", "needle").
fn prose(seed: u64, len: usize) -> String {
    const WORDS: &[&str] = &[
        "the", "of", "and", "a", "to", "in", "is", "that", "it", "was", "for", "on", "are", "as", "with", "his", "they",
        "at", "be", "this", "from", "have", "or", "by", "one", "had", "not", "but", "what", "all", "were", "when", "we",
        "there", "can", "an", "your", "which", "their", "said", "if", "do", "will", "each", "about", "how", "up", "out",
        "them", "then", "she", "many", "some", "so", "these", "would", "other", "into", "has", "more", "her", "two",
        "like", "him", "see", "time", "could", "no", "make", "than", "first", "been", "its", "who", "now", "people",
        "my", "made", "over", "did", "down", "only", "way", "find", "use", "may", "water", "long", "little", "very",
        "after", "words", "called", "just", "where", "most", "know", "garden", "river", "bread", "letter", "winter",
        "machine", "window", "harbour", "station", "morning", "village", "kitchen", "library", "mountain", "ferment",
    ];
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let mut next = || {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (x >> 33) as usize
    };
    let (mut out, mut line, mut sentence) = (String::new(), 0, 0);
    while out.len() < len {
        let mut w = WORDS[next() % WORDS.len()].to_string();
        if sentence == 0 {
            w[..1].make_ascii_uppercase();
        }
        sentence += 1;
        out.push_str(&w);
        line += w.len() + 1;
        if sentence > 6 && next() % 5 == 0 {
            out.push('.');
            sentence = 0;
        }
        if line > 70 {
            out.push('\n');
            line = 0;
            if next() % 6 == 0 {
                out.push('\n');
            }
        } else {
            out.push(' ');
        }
    }
    out.push('\n');
    out
}

/// A library of `n` articles by alice, "Article 0000" first (so last in the
/// timeline), each `len` bytes of prose; `edit` may change article i's text.
fn library(n: usize, len: usize, edit: impl Fn(usize, String) -> String) -> Fixture {
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    for i in 0..n {
        let text = edit(i, prose(i as u64, len));
        f.publish(&alice(), &format!("Article {i:04}"), text.as_bytes(), None);
    }
    f
}

/// The search view's account of this host's articles: the line under the
/// heading, and the results shown (title line, excerpt line).
fn local(run: &Run) -> (String, Vec<(String, String)>) {
    let text = run.page_text();
    let lines: Vec<&str> = text.lines().collect();
    // The list may be scrolled past the heading: then the results start
    // after the header slot.
    let (meta, h) = match lines.iter().position(|l| *l == "In your articles") {
        Some(h) => (lines[h + 1].to_string(), h),
        None => (String::new(), 0),
    };
    let strip = |s: &str| {
        let mut out = String::new();
        let mut skip = false;
        for c in s.chars() {
            match c {
                '[' => {}
                ']' => skip = true,
                '{' if skip => {}
                '}' if skip => skip = false,
                _ if skip && c.is_ascii_digit() => {}
                _ => {
                    skip = false;
                    out.push(c)
                }
            }
        }
        out
    };
    let mut results = Vec::new();
    let mut i = h + 2;
    while i + 1 < lines.len() && lines[i] != "Feeds" {
        results.push((strip(lines[i]), strip(lines[i + 1])));
        i += 2;
    }
    (meta, results)
}

/// Types `query` in the search view (opened with /) and presses Enter, in
/// one frame each; returns the frame Enter arrived in.
fn search_for(run: &mut Run, query: &str) -> u32 {
    run.text("/");
    run.text(query);
    let enter = run.machine.frame();
    run.script_now("{f} press enter\n", 1);
    enter
}

/// Runs frames until the search of this host's articles has ended (or
/// `max` frames): the frame it ended in.
fn until_searched(run: &mut Run, max: u32) -> u32 {
    for _ in 0..max {
        let (meta, _) = local(run);
        if meta.contains(" found in ") || meta.starts_with("more than") || meta.starts_with("stopped") || meta.starts_with("no articles") {
            return run.machine.frame();
        }
        run.frames(1);
    }
    panic!("the search did not end: {}", run.page_text());
}

#[test]
fn local_search_title_body_case_and_bytes() {
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    f.publish(&alice(), "Body only", "Nothing in the title.\n\nBut the text mentions a Zebra crossing\nnear the station.\n".as_bytes(), None);
    f.publish(&alice(), "A zebra in the title", b"The text says nothing of it.\n", None);
    f.publish(&alice(), "Both: ZEBRA", b"And a zebra here too, on the second line\nof the text.\n", None);
    f.publish(&alice(), "Greetings", "Grüße aus Köln, und aus Bonn.\n".as_bytes(), None);
    f.publish(&alice(), "Not downloaded, zebra", b"zebra text that is not in the store", None);
    std::fs::remove_file(f.store.path().join(to_hex(&blob_id(b"zebra text that is not in the store")))).unwrap();
    f.publish(&alice(), "Unrelated", b"Just words.\n", None);
    let mut run = Run::new(&f, "");
    run.frames(10);
    search_for(&mut run, "zebra");
    until_searched(&mut run, 60);
    let (meta, results) = local(&run);
    assert_eq!(meta, "4 found in 6 articles  ·  1 skipped (not downloaded)");
    let titles: Vec<&str> = results.iter().map(|r| r.0.split("   ").next().unwrap()).collect();
    assert_eq!(titles, ["Not downloaded, zebra", "Both: ZEBRA", "A zebra in the title", "Body only"], "newest first");
    assert_eq!(results[0].1, "in the title (not downloaded: its text was not searched)");
    assert_eq!(results[1].1, "And a zebra here too, on the second line of the text.", "folded: the first match in the text");
    assert_eq!(results[2].1, "in the title");
    assert_eq!(results[3].1, "…the text mentions a Zebra crossing near the station.", "from a word's start; line breaks as spaces");
    assert_eq!(results[3].0, format!("Body only   {}  ·  2026-11-17 18:10", short(&alice().feed_key())));
    golden("search-local", &run.view());
    // The match is in the match colour, the text around it is not.
    let layout = run.layout();
    let zebra = layout.lines.iter().flat_map(|l| l.spans.iter()).find(|s| s.text == "Zebra").unwrap();
    assert_eq!(zebra.style.rgb, [100, 220, 220]);
    // Non-ASCII bytes match only themselves: "KÖLN" is not "Köln", "kÖLN"
    // neither; "köln" is (K and k fold).
    for (q, n) in [("köln", 1), ("KÖLN", 0), ("kÖln", 0), ("GRÜSSE", 0), ("grüße", 1)] {
        search_for(&mut run, q);
        until_searched(&mut run, 60);
        let (meta, results) = local(&run);
        assert_eq!(results.len(), n, "{q}: {meta}");
        if n == 0 {
            assert_eq!(meta, "none found in 6 articles  ·  1 skipped (not downloaded)", "{q}");
        }
    }
    golden("search-local-none", &run.view());
}

/// A match across two chunks: the query straddles byte 16,384 of a 40 KB
/// article, at every split of its bytes between the chunks.
#[test]
fn local_search_across_a_chunk_boundary() {
    let needle = "Boundary";
    for split in 1..needle.len() {
        let mut f = Fixture::new();
        f.feeds = vec![alice().feed_key()];
        let mut text = prose(7, 16_384 - split - 1);
        text.truncate(16_384 - split - 1);
        text.push(' ');
        text.push_str(needle);
        text.push_str(" and then more ");
        text.push_str(&prose(8, 24_000));
        assert_eq!(&text[16_384 - split..16_384 + needle.len() - split], needle);
        f.publish(&alice(), "Long", text.as_bytes(), None);
        let mut run = Run::new(&f, "");
        run.frames(10);
        search_for(&mut run, "boundary");
        until_searched(&mut run, 60);
        let (meta, results) = local(&run);
        assert_eq!(meta, "1 found in 1 article", "split {split}");
        assert!(results[0].1.contains(" Boundary and then more "), "split {split}: {:?}", results[0]);
        // Two chunks were asked for, at most 16,384 bytes each, no more.
        let gets: Vec<u16> = run.gets.iter().map(|g| g.2).collect();
        assert!(gets.iter().all(|&n| n as u32 <= 16_384), "{gets:?}");
    }
}

#[test]
fn local_search_cap_and_cancel() {
    // 70 matches: the first 64 are kept, newest first, and the search
    // stops there.
    let f = library(70, 600, |i, t| format!("Number {i} has the needle.\n{t}"));
    let mut run = Run::new(&f, "");
    run.frames(10);
    search_for(&mut run, "Needle");
    until_searched(&mut run, 200);
    let (meta, results) = local(&run);
    assert!(meta.starts_with("more than 64 matches; refine the search  ·  searched "), "{meta}");
    assert_eq!(results[0].0.split("   ").next().unwrap(), "Article 0069");
    assert!(results[0].1.starts_with("Number 69 has the needle. "), "{:?}", results[0]);
    // End: the 64th result is article 6.
    run.press("end");
    let (_, results) = local(&run);
    assert_eq!(results.last().unwrap().0.split("   ").next().unwrap(), "Article 0006");

    // Typing stops a search at once: no request is sent after the key.
    let f = library(400, 3000, |_, t| t);
    let mut run = Run::new(&f, "");
    run.frames(10);
    search_for(&mut run, "zebra");
    run.frames(5);
    run.text("/");
    let before = run.gets.len();
    let typed = run.machine.frame();
    run.script_now("{f} text x\n", 1);
    assert!(row(&run.screen(), 28).contains("Search: x"), "echoed in the frame of the key: {}", run.screen());
    run.frames(3);
    assert!(run.gets.iter().skip(before).all(|g| g.0 < typed), "no get after the key");
    run.press("esc");
    let (meta, _) = local(&run);
    assert!(meta.starts_with("stopped after "), "{meta}");
    // A new search waits for the requests of the old one, then runs whole.
    search_for(&mut run, "zebra");
    until_searched(&mut run, 400);
    assert_eq!(local(&run).0, "none found in 400 articles");
}

/// A result opens its article at the match: the match is anchored and in
/// the match colour, and the view is scrolled to it.
#[test]
fn local_search_opens_at_the_match() {
    let mut f = Fixture::new();
    f.feeds = vec![alice().feed_key()];
    let text = format!("{}\nHere is the zebra we are looking for, deep in the text.\n\n{}", prose(3, 9000), prose(4, 3000));
    f.publish(&alice(), "Deep", text.as_bytes(), None);
    f.publish(&alice(), "Title zebra", b"Nothing in the text.\n", None);
    let mut run = Run::new(&f, "");
    run.frames(10);
    search_for(&mut run, "zebra");
    until_searched(&mut run, 60);
    // The title match is first; Down and Enter: the deep one.
    run.press("down");
    run.press("enter");
    run.frames(6);
    let layout = run.layout();
    let (line, span) = layout
        .lines
        .iter()
        .find_map(|l| l.spans.iter().find(|s| s.style.anchor == 1000).map(|s| (l, s)))
        .unwrap_or_else(|| panic!("the match is anchored: {}", run.page_text()));
    assert_eq!(span.text, "zebra");
    assert_eq!(span.style.rgb, [100, 220, 220]);
    let scroll = run.scroll() as u32;
    assert!(scroll > 0 && line.y >= scroll && line.y + line.height <= scroll + RECT.3 as u32, "in view: {} at {scroll}", line.y);
    golden("search-open", &run.view());
    // Esc: back to the results, which stay; the search had ended.
    run.press("esc");
    assert_eq!(local(&run).0, "2 found in 2 articles");
    // The title match opens at the top.
    run.press("up");
    run.press("enter");
    run.frames(4);
    assert_eq!(run.scroll(), 0);
}

/// The reading profile has no find: the relays' sections say it was
/// refused, and this host's articles are searched all the same.
#[test]
fn local_search_on_the_reading_profile() {
    let f = library(30, 2000, |i, t| if i == 11 { format!("{t}A zebra.\n") } else { t });
    let mut run = Run::reading(&f, "", Consent::Allow);
    run.frames(10);
    search_for(&mut run, "zebra");
    until_searched(&mut run, 100);
    let (meta, results) = local(&run);
    assert_eq!(meta, "1 found in 30 articles");
    assert_eq!(results[0].0.split("   ").next().unwrap(), "Article 0011");
    assert!(run.page_text().contains("Feeds\nthe relays refused the search\n"), "{}", run.page_text());
    let (worst, waited) = run.busiest();
    assert!(waited, "every frame ended in wait_frame ({worst} units)");
}

/// Searches library `label` (n articles of `len` bytes, `big` of them 48 KiB)
/// for a word in every 97th article and for one in none, checks that no
/// frame ran out of budget and prints: frames (and seconds at 60 Hz) from
/// Enter to the first result and to the end, the bytes of text got per
/// frame, and the busiest frame.
fn measure_search(label: &str, n: usize, len: usize, big: usize) {
    let f = library(n, len, |i, t| {
        let t = if big > 0 && i % (n / big) == n / big / 2 { prose(i as u64 + 9999, 49_152) } else { t };
        if i % 97 == 50 { format!("{t}\nThe zebra.\n") } else { t }
    });
    let matches = (0..n).filter(|i| i % 97 == 50).count();
    for query in ["zebra", "xylophone"] {
        let mut run = Run::new(&f, "");
        run.frames(10);
        let enter = search_for(&mut run, query);
        let gets_before = run.gets.len();
        let mut first = None;
        let mut f_end = None;
        while f_end.is_none() {
            assert!(run.machine.frame() < enter + 5000, "{label}: the search ends");
            let (meta, results) = local(&run);
            if first.is_none() && !results.is_empty() {
                first = Some(run.machine.frame() - 1);
            }
            if meta.contains(" found in ") {
                f_end = Some(run.machine.frame() - 1);
                let want = if query == "zebra" { matches } else { 0 };
                let found = if want == 0 { "none".to_string() } else { want.to_string() };
                assert!(meta.starts_with(&format!("{found} found in ")), "{label} {query}: {meta}");
            }
            run.frames(1);
        }
        let end = f_end.unwrap();
        let searching = &run.results[enter as usize..=end as usize];
        let busiest = searching.iter().map(|r| r.used).max().unwrap();
        assert!(searching.iter().all(|r| r.waited), "{label}: every frame ended in wait_frame");
        let bytes: u64 = run.gets[gets_before..].iter().filter(|g| g.2 > 64).map(|g| g.2 as u64).sum();
        let mut per_frame = std::collections::BTreeMap::new();
        for g in &run.gets[gets_before..] {
            *per_frame.entry(g.0).or_insert(0u64) += g.2 as u64;
        }
        let frames = end - enter + 1;
        let secs = |fr: u32| fr as f64 / 60.0;
        eprintln!(
            "reader: search {label} for {query:?}: first result after {} frames{}, done after {frames} frames ({:.1} s at 60 Hz); \
             {:.0} bytes of text got a frame on average, {} at most; busiest frame {busiest} units ({:.1}%); {} requests pending at most",
            first.map(|f| (f - enter + 1).to_string()).unwrap_or("-".into()),
            first.map(|f| format!(" ({:.2} s)", secs(f - enter + 1))).unwrap_or_default(),
            secs(frames),
            bytes as f64 / frames as f64,
            per_frame.values().max().unwrap(),
            common::pct(busiest),
            run.most_pending,
        );
    }
}

/// What the search of this host's articles costs: it never runs a frame out
/// of budget, and stays responsive (a key moves the selection in its own
/// frame). Run with --nocapture to see the numbers; local_search_5000 (run
/// with --ignored, best with --release) measures 5,000 articles.
#[test]
fn local_search_budget() {
    measure_search("1,000 x 3 KB", 1000, 3000, 0);
    measure_search("6 x 48 KiB in 100 x 3 KB", 100, 3000, 6);
    // Responsive while it runs: Down moves the selection, and its list is
    // submitted, in the frame of the key; typed text shows in its frame.
    let f = library(1000, 3000, |i, t| if i % 50 == 0 { format!("{t}A zebra.\n") } else { t });
    let mut run = Run::new(&f, "");
    run.frames(10);
    search_for(&mut run, "zebra");
    run.frames(40);
    for _ in 0..5 {
        let k = run.machine.frame();
        run.script_now("{f} press down\n", 1);
        assert!(run.submits.contains(&k), "the list in the frame of the key");
        run.frames(3);
    }
    assert!(local(&run).0.starts_with("searched "), "still searching: {}", local(&run).0);
    let (worst, waited) = run.busiest();
    assert!(waited, "{worst}");
    eprintln!("reader: Down during a search: the list submitted in the frame of the key; busiest frame {worst} units");
}

#[test]
#[ignore]
fn local_search_5000() {
    measure_search("5,000 x 3 KB", 5000, 3000, 0);
}
