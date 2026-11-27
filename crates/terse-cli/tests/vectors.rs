//! The core test vectors (`vectors/`, `terse vectors`): the checked-in
//! directory is what the generator writes, and the built-in reader
//! (programs/reader, run headless) shows the article corpus as the
//! reference parser (terse_net::article_parse) says it must.
//!
//! The reader comparison converts the page document the reader submits
//! for an article into the parser's model and compares the two after
//! normalising what ARTICLE.md leaves to the reader:
//! - blocks: kind (heading, subheading, paragraph, item, pre, table,
//!   figure) and order; a preformatted block and a fence are both `pre`
//!   (they are shown alike);
//! - text: tabs as spaces, runs of spaces as one, blanks at a block's (or
//!   cell's) start and end dropped (ARTICLE.md 2.1 lets a reader show a run
//!   of blanks as written or as one space); pre lines exactly but tabs as
//!   spaces;
//! - code spans: their text;
//! - links: kind, the target's short form (first and last six hex digits,
//!   and a program's decoded name), read from the reader's status row as
//!   Tab selects each link in turn, and the shown text: a link with text
//!   shows its text, a bare link the reference reader's short form
//!   (ARTICLE.md 4.1);
//! - tables: columns, header, alignment, every cell (short rows padded);
//! - figures: size and every path that draws something, with its fill,
//!   stroke and commands in sixteenths, exactly; the reader's `fg` is its
//!   body text colour, so a literal colour equal to that would read as `fg`
//!   (the corpus has none).
//! Not compared: styles (sizes, faces, colours of text), spacing, the
//! title and details lines the reader puts above an article.

mod common;

use std::collections::BTreeMap;

use terse_core::page::{Paragraph, Run as PageRun, Segment, parse as parse_page};
use terse_core::{Event, FrameInput, Image, Machine, PageInput, Script};
use terse_net::article_parse::{self, Align, Block, Colour, Inline, Parsed, Seg, Target};
use terse_net::{BlobStore, Consent, ContentService, Draft, Identity, Node, Subscriptions};

/// The reference reader's column, in units (ARTICLE.md 11).
const COLUMN: i32 = 512;

const IMAGE: &[u8] = include_bytes!("../../../programs/reader/reader.terse");

/// Corpus cases the reader does not show as ARTICLE.md says, each with the
/// reason. The test fails if one of these starts to agree, so that it is
/// taken off the list. None at present.
const KNOWN_FAILURES: &[(&str, &str)] = &[];

/// Cases that are not articles: the reader does not show them as text.
const NOT_SHOWN: &[&str] = &["not-an-article-image", "not-an-article-document"];

fn vectors_dir() -> std::path::PathBuf {
    common::workspace().join("vectors")
}

/// Every file under `dir`, by its path relative to `dir`.
fn files(dir: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// `vectors/` is what `terse vectors` writes, file for file.
#[test]
fn vectors_are_up_to_date() {
    let tmp = tempfile::tempdir().unwrap();
    let out = common::terse_command()
        .args(["vectors", "--dir", tmp.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let fresh = files(tmp.path());
    let generated: BTreeMap<String, Vec<u8>> = terse_net::vectors::generate().into_iter().map(|f| (f.path, f.bytes)).collect();
    assert_eq!(fresh.keys().collect::<Vec<_>>(), generated.keys().collect::<Vec<_>>());
    let checked_in = files(&vectors_dir());
    for (path, bytes) in &fresh {
        assert!(checked_in.get(path) == Some(bytes), "vectors/{path} is stale or missing: run `terse vectors`");
    }
    for path in checked_in.keys() {
        assert!(fresh.contains_key(path), "vectors/{path} is no longer generated: run `terse vectors`");
    }
    // Generating twice gives the same bytes.
    terse_net::vectors::write(tmp.path()).unwrap();
    assert_eq!(files(tmp.path()), fresh);
}

// --- Running the reader ------------------------------------------------------

struct Reader {
    machine: Machine,
    content: ContentService,
    script: Script,
    page: PageInput,
    completions: Vec<terse_core::Completion>,
    feeds_updated: bool,
    _store: tempfile::TempDir,
    _config: tempfile::TempDir,
}

impl Reader {
    /// The reader with one article, `text`, in its timeline, opened.
    fn open(text: &[u8]) -> Reader {
        let store = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let alice = Identity::from_seed([11; 32]);
        let mut author = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
        author.set_identity(alice.clone());
        let draft = Draft { data: text.to_vec(), title: "Corpus".into(), reply_to: None, time: 1_794_938_400 };
        assert!(matches!(author.publish_article(&draft), terse_net::Done::Published { .. }));
        let mut node = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
        node.set_subscriptions(Subscriptions::in_memory([alice.feed_key()]));
        node.set_identity_path(config.path().join("identity.key")).unwrap();
        let mut r = Reader {
            machine: Machine::new(&Image::parse(IMAGE).unwrap()),
            content: ContentService::start_with_consent(node, Consent::Deny),
            script: Script::parse("12 press enter\n").unwrap(),
            page: PageInput::default(),
            completions: Vec::new(),
            feeds_updated: false,
            _store: store,
            _config: config,
        };
        r.frames(40);
        r
    }

    fn frame(&mut self) {
        let frame = self.machine.frame();
        let events = self.script.events(frame).to_vec();
        let mut events = self.page.events(self.machine.page(), &events);
        if std::mem::take(&mut self.feeds_updated) {
            events.push(Event::feeds_updated());
        }
        let completions = std::mem::take(&mut self.completions);
        let input = FrameInput { events: &events, completions: &completions, ..Default::default() };
        self.machine.run_frame(&input);
        assert!(self.machine.fault().is_none(), "{:?}", self.machine.fault());
        self.page.after_frame(self.machine.page());
        let requests = self.machine.take_content_requests();
        self.content.submit(requests);
        self.completions = self.content.wait_finished();
        self.feeds_updated = self.content.feeds_updated();
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame();
        }
    }

    fn press(&mut self, key: &str) {
        let f = self.machine.frame();
        self.script = Script::parse(&format!("{f} press {key}\n")).unwrap();
        self.frames(3);
    }

    fn status(&self) -> String {
        let screen = self.machine.terminal().unwrap().text();
        screen.lines().nth(29).unwrap_or("").trim_end().to_string()
    }

    fn paragraphs(&self) -> Vec<Paragraph> {
        parse_page(self.machine.page().unwrap().document().expect("a page document"))
    }
}

// --- The comparable form -------------------------------------------------------

/// A link's target as the reference reader shows it in its status row and
/// in a bare link: kind and short form.
fn short(h: &[u8]) -> String {
    let x: String = h.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}…{}", &x[..6], &x[58..])
}

fn target_key(t: &Target) -> String {
    match t {
        Target::Article(id) => format!("article {}", short(id)),
        Target::Feed(k, h) if h.is_empty() => format!("feed {}", short(k)),
        Target::Feed(k, h) => format!("feed {}@{h}", short(k)),
        Target::Program(k, name) => format!("program {} {name}", short(k)),
    }
}

/// What the reference reader shows for a bare link (ARTICLE.md 4.1).
fn bare_text(t: &Target) -> String {
    match t {
        Target::Article(id) => format!("terse:{}", short(id)),
        Target::Feed(k, h) if h.is_empty() => format!("terse:feed/{}", short(k)),
        Target::Feed(k, h) => format!("terse:feed/{}@{h}", short(k)),
        Target::Program(k, name) => format!("terse:program/{}/{name}", short(k)),
    }
}

/// Text with tabs as spaces and runs of spaces as one.
fn squeeze(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().map(|c| if c == '\t' { ' ' } else { c }) {
        if !(c == ' ' && out.ends_with(' ')) {
            out.push(c);
        }
    }
    out
}

/// Inline content as comparable lines, `flow`: text squeezed and trimmed
/// at the ends.
fn inline_lines(xs: &[(char, String, String)], flow: bool, out: &mut Vec<String>, indent: &str) {
    // Merge adjacent text, squeeze, trim the ends.
    let mut merged: Vec<(char, String, String)> = Vec::new();
    for (k, a, b) in xs {
        match (merged.last_mut(), k) {
            (Some(('t', t, _)), 't') => t.push_str(a),
            _ => merged.push((*k, a.clone(), b.clone())),
        }
    }
    let n = merged.len();
    for (i, (k, a, b)) in merged.iter_mut().enumerate() {
        let tabs = a.replace('\t', " ");
        *a = if *k == 't' && flow { squeeze(a) } else { tabs };
        if *k == 't' && flow && i == 0 {
            *a = a.trim_start_matches(' ').to_string();
        }
        if *k == 't' && flow && i + 1 == n {
            *a = a.trim_end_matches(' ').to_string();
        }
        let _ = b;
    }
    for (k, a, b) in merged {
        match k {
            't' if a.is_empty() => {}
            't' => out.push(format!("{indent}text {}", article_parse::quote(&a))),
            'c' => out.push(format!("{indent}code {}", article_parse::quote(&a))),
            _ => out.push(format!("{indent}link {b} shown {}", article_parse::quote(&a))),
        }
    }
}

fn from_inlines(xs: &[Inline]) -> Vec<(char, String, String)> {
    xs.iter()
        .map(|x| match x {
            Inline::Text(t) => ('t', t.clone(), String::new()),
            Inline::Code(t) => ('c', t.clone(), String::new()),
            Inline::Link { target, text } => ('l', text.clone().unwrap_or_else(|| bare_text(target)), target_key(target)),
        })
        .collect()
}

fn colour(c: Option<Colour>) -> String {
    match c {
        None => "none".into(),
        Some(Colour::Fg) => "fg".into(),
        Some(Colour::Rgba(c)) => format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]),
    }
}

/// The reference parser's blocks in comparable form.
fn reference(blocks: &[Block]) -> Vec<String> {
    let mut out = Vec::new();
    for b in blocks {
        match b {
            Block::Heading(x) | Block::Subheading(x) | Block::Paragraph(x) | Block::Item(x) => {
                out.push(
                    match b {
                        Block::Heading(_) => "heading",
                        Block::Subheading(_) => "subheading",
                        Block::Paragraph(_) => "paragraph",
                        _ => "item",
                    }
                    .into(),
                );
                inline_lines(&from_inlines(x), true, &mut out, "  ");
            }
            Block::Pre(lines, _) => {
                out.push("pre".into());
                for l in lines {
                    out.push("  line".into());
                    inline_lines(&from_inlines(l), false, &mut out, "    ");
                }
            }
            Block::Table(t) => {
                let align: Vec<&str> = t.align.iter().map(|a| match a {
                    Align::Start => "start",
                    Align::Centre => "centre",
                    Align::End => "end",
                }).collect();
                out.push(format!("table {} header {} align {}", t.columns, t.header, align.join(" ")));
                for row in &t.rows {
                    out.push("  row".into());
                    for cell in row {
                        out.push("    cell".into());
                        inline_lines(&from_inlines(cell), true, &mut out, "      ");
                    }
                }
            }
            Block::Figure(f) => {
                // Wider than the column: scaled down to it, as the reference
                // reader rounds (ARTICLE.md 6.2 allows scaling; the rounding
                // is the reader's).
                let w = f.width as i32;
                let sc = |v: i32| if w > COLUMN { (v * COLUMN + if v < 0 { -(w / 2) } else { w / 2 }) / w } else { v };
                let (fw, fh) = if w > COLUMN { (COLUMN, ((f.height as i32 * COLUMN + w / 2) / w).max(1)) } else { (w, f.height as i32) };
                out.push(format!("figure {fw} {fh}"));
                for p in &f.paths {
                    let width = |v: u32| if v != 0 && sc(v as i32) == 0 { 1 } else { sc(v as i32) };
                    let stroke = p.stroke.map_or("none".into(), |(c, v)| format!("{} {}", colour(Some(c)), width(v)));
                    out.push(format!("  path fill {} stroke {stroke}", colour(p.fill)));
                    for s in &p.segs {
                        out.push(format!(
                            "    {}",
                            seg(match *s {
                                Seg::Move(x, y) => Seg::Move(sc(x), sc(y)),
                                Seg::Line(x, y) => Seg::Line(sc(x), sc(y)),
                                Seg::Quad(a, b, x, y) => Seg::Quad(sc(a), sc(b), sc(x), sc(y)),
                                Seg::Cubic(a, b, c, d, x, y) => Seg::Cubic(sc(a), sc(b), sc(c), sc(d), sc(x), sc(y)),
                                Seg::Close => Seg::Close,
                            })
                        ));
                    }
                }
            }
        }
    }
    out
}

fn seg(s: Seg) -> String {
    match s {
        Seg::Move(x, y) => format!("M {x} {y}"),
        Seg::Line(x, y) => format!("L {x} {y}"),
        Seg::Quad(a, b, x, y) => format!("Q {a} {b} {x} {y}"),
        Seg::Cubic(a, b, c, d, x, y) => format!("C {a} {b} {c} {d} {x} {y}"),
        Seg::Close => "Z".into(),
    }
}

/// The reader's runs as inline content: mono runs are code spans (but in
/// pre blocks), anchored runs links (merged by anchor).
fn from_runs(runs: &[PageRun], pre: bool, links: &BTreeMap<u16, String>) -> Vec<(char, String, String)> {
    let mut out: Vec<(char, String, String)> = Vec::new();
    let mut last_anchor = 0;
    for r in runs {
        if r.style.anchor != 0 {
            if r.style.anchor == last_anchor
                && let Some(l) = out.last_mut()
            {
                l.1.push_str(&r.text);
                continue;
            }
            last_anchor = r.style.anchor;
            let t = links.get(&r.style.anchor).cloned().unwrap_or_else(|| format!("? (anchor {})", r.style.anchor));
            out.push(('l', r.text.clone(), t));
            continue;
        }
        last_anchor = 0;
        let k = if !pre && r.style.face == terse_core::page::face::MONO { 'c' } else { 't' };
        match out.last_mut() {
            Some((lk, t, _)) if *lk == k && k == 't' => t.push_str(&r.text),
            _ => out.push((k, r.text.clone(), String::new())),
        }
    }
    out
}

/// The reader's page document in comparable form: the paragraphs after the
/// rule that ends the article's head.
fn reader_view(ps: &[Paragraph], fg: [u8; 3], links: &BTreeMap<u16, String>) -> Vec<String> {
    let start = ps.iter().position(|p| p.rule.is_some()).expect("the head's rule") + 1;
    let mut out = Vec::new();
    for p in &ps[start..] {
        if let Some(t) = &p.table {
            let align: Vec<&str> = t.align.iter().map(|a| match a {
                terse_core::page::Align::Start => "start",
                terse_core::page::Align::Centre => "centre",
                terse_core::page::Align::End => "end",
            }).collect();
            out.push(format!("table {} header {} align {}", t.columns, t.header, align.join(" ")));
            for r in 0..t.rows() {
                out.push("  row".into());
                for c in 0..t.columns as usize {
                    out.push("    cell".into());
                    inline_lines(&from_runs(t.cell(r, c), false, links), true, &mut out, "      ");
                }
            }
        } else if let Some(f) = &p.figure {
            out.push(format!("figure {} {}", f.width, f.height));
            let c = |rgba: [u8; 4]| if rgba == [fg[0], fg[1], fg[2], 255] { Colour::Fg } else { Colour::Rgba(rgba) };
            for path in f.paths() {
                let stroke = path.stroke.map_or("none".into(), |(rgba, w)| format!("{} {w}", colour(Some(c(rgba)))));
                out.push(format!("  path fill {} stroke {stroke}", colour(path.fill.map(c))));
                for s in &path.segments {
                    let v = |x: i16| x as i32;
                    out.push(format!(
                        "    {}",
                        seg(match *s {
                            Segment::Move(x, y) => Seg::Move(v(x), v(y)),
                            Segment::Line(x, y) => Seg::Line(v(x), v(y)),
                            Segment::Quad(a, b, x, y) => Seg::Quad(v(a), v(b), v(x), v(y)),
                            Segment::Cubic(a, b, c, d, x, y) => Seg::Cubic(v(a), v(b), v(c), v(d), v(x), v(y)),
                            Segment::Close => Seg::Close,
                        })
                    ));
                }
            }
        } else if p.rule.is_some() {
            out.push("rule".into());
        } else if p.nowrap {
            out.push("pre".into());
            let text: String = p.runs.iter().map(|r| r.text.as_str()).collect();
            // Split the runs at line breaks, keeping links.
            let mut line: Vec<PageRun> = Vec::new();
            let mut lines = Vec::new();
            for r in &p.runs {
                let mut parts = r.text.split('\n').peekable();
                while let Some(part) = parts.next() {
                    line.push(PageRun { text: part.to_string(), style: r.style });
                    if parts.peek().is_some() {
                        lines.push(std::mem::take(&mut line));
                    }
                }
            }
            lines.push(line);
            let _ = text;
            for l in lines {
                out.push("  line".into());
                inline_lines(&from_runs(&l, true, links), false, &mut out, "    ");
            }
        } else {
            let mut runs = p.runs.clone();
            let kind = if p.indent == 10 {
                if let Some(first) = runs.first_mut() {
                    first.text = first.text.trim_start_matches("• ").to_string();
                }
                "item"
            } else if p.space == 18 {
                "heading"
            } else if p.space == 14 {
                "subheading"
            } else {
                "paragraph"
            };
            out.push(kind.into());
            inline_lines(&from_runs(&runs, false, links), true, &mut out, "  ");
        }
    }
    out
}

/// The status row for link `n` (Tab selects links in order) as a target
/// key: " Link 3: terse:012345…abcdef  (an article)   Enter: open".
fn status_target(s: &str) -> String {
    let rest = s.split_once(": ").map_or(s, |x| x.1);
    if let Some(r) = rest.strip_prefix("terse:feed/") {
        format!("feed {}", r.split("  ").next().unwrap())
    } else if let Some(r) = rest.strip_prefix("terse:") {
        format!("article {}", r.split("  ").next().unwrap())
    } else if let Some(r) = rest.strip_prefix("program \u{201c}") {
        let (name, by) = r.split_once("\u{201d} by ").unwrap_or((r, ""));
        format!("program {} {name}", by.split("   ").next().unwrap())
    } else {
        format!("? {s}")
    }
}

/// Opens `text` in the reader and returns its view in comparable form.
fn reader_shows(text: &[u8]) -> Vec<String> {
    let mut r = Reader::open(text);
    let anchors: Vec<u16> = {
        let mut a: Vec<u16> = Vec::new();
        for p in r.paragraphs() {
            let runs: Vec<&PageRun> = p.runs.iter().chain(p.table.iter().flat_map(|t| t.cells.iter().flatten())).collect();
            for run in runs {
                if run.style.anchor != 0 && !a.contains(&run.style.anchor) {
                    a.push(run.style.anchor);
                }
            }
        }
        a
    };
    let mut links = BTreeMap::new();
    for _ in &anchors {
        r.press("tab");
        let s = r.status();
        let n: u16 = s.trim_start().strip_prefix("Link ").and_then(|x| x.split(':').next()).and_then(|x| x.parse().ok()).unwrap_or(0);
        links.insert(n, status_target(&s));
    }
    let ps = r.paragraphs();
    let fg = [218, 218, 218];
    reader_view(&ps, fg, &links)
}

/// The reader shows every corpus article as the reference parser says.
#[test]
fn reader_agrees_with_the_reference_parser() {
    let mut failed = Vec::new();
    let mut report = String::new();
    for (name, text) in terse_net::vectors::corpus() {
        if NOT_SHOWN.contains(&name) {
            continue;
        }
        let Parsed::Article(blocks) = article_parse::parse(&text) else { panic!("{name}") };
        let want = reference(&blocks);
        let got = reader_shows(&text);
        if std::env::var("VECTORS_SHOW").is_ok_and(|v| v == name) {
            eprintln!("{}\n---\n{}", want.join("\n"), got.join("\n"));
        }
        if want != got {
            failed.push(name);
            report.push_str(&format!("=== {name}\n"));
            let n = want.len().max(got.len());
            for i in 0..n {
                let (w, g) = (want.get(i).map_or("", String::as_str), got.get(i).map_or("", String::as_str));
                report.push_str(&format!("{} {w:<60} | {g}\n", if w == g { ' ' } else { '!' }));
            }
        }
    }
    eprintln!("{report}");
    let known: Vec<&str> = KNOWN_FAILURES.iter().map(|k| k.0).collect();
    let unexpected: Vec<&&str> = failed.iter().filter(|n| !known.contains(n)).collect();
    let fixed: Vec<&&str> = known.iter().filter(|n| !failed.contains(n)).collect();
    assert!(unexpected.is_empty() && fixed.is_empty(), "disagree: {unexpected:?}; known failures that now agree: {fixed:?}\n{report}");
}


/// relay-session.txt: a Terse relay that holds nothing answers the client
/// messages with exactly the relay messages of the file.
#[test]
fn relay_session_matches_the_relay() {
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;
    let store = tempfile::tempdir().unwrap();
    let relay = std::sync::Arc::new(terse_relay::Relay::new(BlobStore::open(store.path(), None).unwrap(), terse_relay::DEFAULT_FEED_CAP));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    let connect = || {
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tungstenite::client::client(format!("ws://{addr}"), stream).unwrap().0
    };
    let mut ws = connect();
    let text = std::fs::read_to_string(vectors_dir().join("relay-session.txt")).unwrap();
    let mut note = "";
    let (mut sent, mut received) = (0, 0);
    for line in text.lines() {
        if let Some(n) = line.strip_prefix("# ") {
            note = n;
            continue;
        }
        let Some((dir, hex)) = line.split_once(' ') else { continue };
        let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        match dir {
            "c>" => {
                ws.send(tungstenite::Message::binary(bytes)).unwrap();
                sent += 1;
            }
            "r>" => {
                let got = loop {
                    match ws.read().unwrap_or_else(|e| panic!("{note}: {e}")) {
                        tungstenite::Message::Binary(b) => break b.to_vec(),
                        _ => continue,
                    }
                };
                assert_eq!(got, bytes, "{note}");
                received += 1;
            }
            "p>" => {
                // A peer link: no reply, and the connection stays open.
                let mut peer = connect();
                peer.send(tungstenite::Message::binary(bytes)).unwrap();
                peer.send(tungstenite::Message::binary(terse_net::protocol::encode_get(1, &[0; 32]))).unwrap();
                let reply = loop {
                    if let tungstenite::Message::Binary(b) = peer.read().unwrap() {
                        break b.to_vec();
                    }
                };
                assert_eq!(reply, terse_net::protocol::encode_get_reply(1, 2, b""), "{note}");
            }
            d => panic!("unknown direction {d}"),
        }
    }
    assert!(sent >= 10 && received >= 10, "{sent} sent, {received} received");
}

/// entries.txt: the reference decoder and a Terse relay, as a black box,
/// accept every valid entry and refuse every invalid one (strict
/// verification included).
#[test]
fn relay_judges_entries_as_the_vectors_say() {
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;
    let store = tempfile::tempdir().unwrap();
    let relay = std::sync::Arc::new(terse_relay::Relay::new(BlobStore::open(store.path(), None).unwrap(), terse_relay::DEFAULT_FEED_CAP));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut ws = tungstenite::client::client(format!("ws://{addr}"), stream).unwrap().0;
    let text = std::fs::read_to_string(vectors_dir().join("entries.txt")).unwrap();
    let (mut name, mut valid) = (String::new(), None);
    let (mut req, mut checked) = (0u32, 0);
    for line in text.lines() {
        if let Some(n) = line.strip_prefix("entry ") {
            (name, valid) = (n.to_string(), None);
        } else if let Some(v) = line.strip_prefix("  valid ") {
            valid = Some(v == "yes");
        } else if let Some(hex) = line.strip_prefix("  bytes ") {
            let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
            let valid = valid.unwrap_or_else(|| panic!("{name}: no valid line"));
            assert_eq!(terse_net::entry::decode(&bytes).is_ok(), valid, "decode: {name}");
            req += 1;
            ws.send(tungstenite::Message::binary(terse_net::protocol::encode_publish(req, &bytes))).unwrap();
            let got = loop {
                if let tungstenite::Message::Binary(b) = ws.read().unwrap() {
                    break b.to_vec();
                }
            };
            let status = if valid { 1 } else { 3 };
            assert_eq!(got, terse_net::protocol::encode_status_reply(terse_net::protocol::PUBLISH_REPLY, req, status), "relay: {name}");
            checked += 1;
        }
    }
    assert!(checked >= 34, "{checked} entries");
}
