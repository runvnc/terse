//! programs/reports/reports.terse: gathering performance reports (RECORD.md 5)
//! from the followed feeds and, on F, from a local relay's search; one
//! report per key (a faker's 50 count once; an older report is replaced
//! by a newer); malformed reports counted, other documents ignored; the
//! verdicts of both samples, checked against the reference (terse-perflog's
//! `aggregate`, the computation of `terse perf aggregate`) through the
//! article the program publishes; golden screens; the frame budget and
//! the reports parsed a second; `terse perf report` and `terse perf
//! aggregate` themselves; and the image is up to date. Set
//! UPDATE_GOLDEN=1 to rewrite programs/reports/expected/.

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use terse_core::{BUDGET_PER_FRAME, Image, Machine, Script};
use terse_net::protocol::SCOPE_ENTRIES;
use terse_net::service::Headless;
use terse_net::{BlobStore, Consent, ContentService, Done, Identity, Node, Subscriptions, to_hex};
use terse_perflog::aggregate::{Aggregate, Submission, tally};
use terse_perflog::report::{self, Bench, Report, Start, Tier};
use terse_perflog::Component;
use terse_perflog::env::Environment;

const IMAGE: &[u8] = include_bytes!("../../../programs/reports/reports.terse");
const RAM_PAGES: u8 = 32;
const VECTORS: &str = include_str!("../../terse-perflog/example-reports.txt");

fn id(name: &str, build: &str) -> terse_perflog::Identity {
    terse_perflog::Identity { name: name.into(), version: "0.1.0".into(), build: build.into() }
}

const HOST_A: (&str, &str) = ("terse-host", "b3:a1a1a1a1a1a1a1a1");
const HOST_B: (&str, &str) = ("terse-bus", "b3:b2b2b2b2b2b2b2b2");
const VEC: (&str, &str) = ("terse-dev-vector", "b3:c3c3c3c3c3c3c3c3");
const PAGE: (&str, &str) = ("terse-dev-page", "b3:d4d4d4d4d4d4d4d4");

/// A report about `host` with the vector (dev3) and page (dev7) drivers.
struct Spec {
    host: (&'static str, &'static str),
    cores: u32,
    ram: u32,
    page_halted: bool,
    vec_halted: bool,
    perf: Option<bool>,
    start: f64,
    /// The report names the two drivers.
    devices: bool,
}

fn spec(cores: u32, ram: u32) -> Spec {
    Spec { host: HOST_A, cores, ram, page_halted: false, vec_halted: false, perf: None, start: 400.0, devices: true }
}

fn make(s: &Spec) -> Report {
    let mut r = report::parse_text(b"terse-report 1\nperiod 2026-11-01 2026-11-21\nhost x - -\nenv Linux 6.8.0 x86_64 4 4 software -\nruns 1 0 0 0\nend 5\n").unwrap();
    r.host = id(s.host.0, s.host.1);
    r.env = Environment { os: "Linux".into(), os_version: "6.8.0".into(), arch: "x86_64".into(), cores: s.cores, ram_gib: s.ram, renderer: "gpu".into(), cpu: "Test CPU".into() };
    if s.devices {
        r.devices = vec![(Component::Device(3), vec![3, 9], id(VEC.0, VEC.1)), (Component::Device(7), vec![7], id(PAGE.0, PAGE.1))];
    }
    (r.runs, r.ended, r.frames, r.late_frames) = (12, 12, 600_000, 40);
    r.start = Some(Start { count: 12, median_ms: s.start, worst_ms: s.start + 100.0, cold: 1, cold_worst_ms: Some(s.start + 100.0) });
    for (c, halted) in [(Component::Device(3), s.vec_halted), (Component::Device(7), s.page_halted)] {
        if halted {
            r.late.push((c, [30, 2, 0, 0]));
            r.warn.push((c, 3));
            r.halt.push((c, 1));
        }
    }
    if let Some(fail) = s.perf {
        r.perf = Some((id("terse", "b3:e5e5e5e5e5e5e5e5"), "2026-11-21".into()));
        r.bench = vec![
            Bench { id: "M1a".into(), value: Some(1.0), unit: "frames".into(), requirement: if fail { Tier::Fail } else { Tier::Pass }, ceiling: Tier::None, target: Tier::None },
            Bench { id: "H1".into(), value: Some(22.0), unit: "ms".into(), requirement: Tier::None, ceiling: Tier::None, target: Tier::Pass },
        ];
    }
    r
}

/// One published report: who, which entry, the report (None: malformed).
struct Published {
    key: String,
    seq: u64,
    report: Option<Report>,
    followed: bool,
}

struct World {
    store: tempfile::TempDir,
    author: Node,
    time: u32,
    followed: Vec<Identity>,
    published: Vec<Published>,
    malformed_reports: usize,
}

impl World {
    fn publish(&mut self, who: &Identity, doc: &[u8], title: &str) -> u64 {
        self.author.set_identity(who.clone());
        self.time += 60;
        match self.author.publish_document(doc, title, self.time) {
            Done::Published { seq, .. } => seq as u64,
            other => panic!("{other:?}"),
        }
    }

    fn report(&mut self, who: &Identity, r: &Report) {
        let seq = self.publish(who, &r.document(), &r.title());
        let followed = self.followed.iter().any(|f| f.feed_key() == who.feed_key());
        self.published.push(Published { key: to_hex(&who.feed_key()), seq, report: Some(r.clone()), followed });
    }

    fn profile(&mut self, who: &Identity, name: &str, listed: bool) {
        self.author.set_identity(who.clone());
        self.time += 60;
        assert!(matches!(self.author.publish_profile(name, listed, Some(String::new()), self.time), Done::Published { .. }));
    }
}

fn who(seed: u8) -> Identity {
    Identity::from_seed([seed; 32])
}

/// The vectors' documents (valid and malformed), as the tests of
/// terse-perflog read them.
fn vector_documents() -> Vec<(bool, String, Vec<u8>)> {
    let mut out = Vec::new();
    for rec in VECTORS.split("\n\n").filter(|r| r.starts_with("report ")) {
        let (mut valid, mut reason, mut text, mut header) = (false, String::new(), String::new(), report::header());
        for line in rec.lines().skip(1) {
            let (k, v) = line.trim_start().split_once(' ').unwrap();
            match k {
                "valid" => valid = v == "yes",
                "reason" => reason = v.into(),
                "format" => {
                    header = [&b"TDOC\0"[..], &[0; 32], &[v.len() as u8], v.as_bytes()].concat();
                }
                "header-hex" => header = (0..v.len()).step_by(2).map(|i| u8::from_str_radix(&v[i..i + 2], 16).unwrap()).collect(),
                "text" => {
                    let q = &v[1..v.len() - 1];
                    text = q.replace("\\n", "\n").replace("\\t", "\t").replace("\\\"", "\"").replace("\\\\", "\\");
                }
                _ => {}
            }
        }
        let _ = reason;
        out.push((valid, rec.lines().next().unwrap()[7..].to_string(), [header, text.into_bytes()].concat()));
    }
    out
}

/// The world of the tests: nine followed reporters (one below the
/// floor), the page driver halted for five of the eight judged; two
/// reports about a second host; a followed faker with 50 reports from
/// one key; a reporter whose older report is replaced by a newer; the
/// vectors (valid and malformed) and a sketch from a followed feed; 50
/// strangers (listed profiles, not followed) whose reports all have the
/// vector driver halted.
fn world() -> World {
    let store = tempfile::tempdir().unwrap();
    let author = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
    let followed: Vec<Identity> = (1..=11).map(who).collect();
    let mut w = World { store, author, time: 1_794_938_400, followed, published: Vec::new(), malformed_reports: 0 };
    for (i, f) in w.followed.clone().iter().enumerate() {
        w.profile(f, &format!("Reporter {}", i + 1), true);
    }
    // Reporter 2's older report: the page driver halted (replaced below).
    let mut old = spec(8, 16);
    old.page_halted = true;
    w.report(&who(2), &make(&old));
    let envs = [(2, 4), (8, 16), (4, 8), (16, 32), (8, 8), (4, 4), (12, 16), (6, 16), (32, 64)];
    for (i, &(cores, ram)) in envs.iter().enumerate() {
        let mut s = spec(cores, ram);
        s.page_halted = [0, 3, 4, 5, 6, 8].contains(&i); // reporter 1 (i 0) is below the floor
        s.perf = (i % 3 != 2).then_some(i == 4);
        s.start = 300.0 + 10.0 * i as f64;
        w.report(&who(i as u8 + 1), &make(&s));
    }
    // A second host, two reports (its devices built in).
    for k in [1u8, 2] {
        let mut s = spec(8, 16);
        s.host = HOST_B;
        s.devices = false;
        w.report(&who(k), &make(&s));
    }
    // The faker (reporter 10): 50 reports from one key, each saying the
    // vector driver was halted.
    for _ in 0..50 {
        let mut s = spec(8, 16);
        s.vec_halted = true;
        w.report(&who(10), &make(&s));
    }
    // Reporter 11: the vectors and a sketch.
    for (valid, name, doc) in vector_documents() {
        let seq = w.publish(&who(11), &doc, &format!("Performance report: vector {name}"));
        let ours = doc.starts_with(&report::header());
        if valid {
            w.published.push(Published { key: to_hex(&who(11).feed_key()), seq, report: Some(report::parse_document(&doc).unwrap()), followed: true });
        } else if ours {
            w.malformed_reports += 1;
            w.published.push(Published { key: to_hex(&who(11).feed_key()), seq, report: None, followed: true });
        }
    }
    let sketch = [&b"TDOC\0"[..], &[0; 32], &[6], b"sketch", &[0, 1, 0, 10, 0, 10, 0]].concat();
    w.publish(&who(11), &sketch, "Sketch 1");
    // 50 strangers, listed, not followed.
    for k in 0..50u8 {
        let s_id = who(100 + k);
        w.profile(&s_id, &format!("Stranger {k}"), true);
        let mut s = spec(8, 16);
        s.vec_halted = true;
        w.report(&s_id, &make(&s));
    }
    w
}

/// A relay serving a copy of the world's store, on an ephemeral port.
fn relay_for(w: &World) -> (tempfile::TempDir, String) {
    fn copy(from: &Path, to: &Path) {
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
    copy(w.store.path(), dir.path());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let relay = std::sync::Arc::new(terse_relay::Relay::new(BlobStore::open(dir.path(), None).unwrap(), 1024));
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    (dir, url)
}

struct Run {
    m: Machine,
    h: Headless,
    used: Vec<u64>,
}

impl Run {
    fn new(w: &World, relay: Option<&str>, script: &str) -> Self {
        let mut node = Node::new(BlobStore::open(w.store.path(), None).unwrap(), None);
        if let Some(url) = relay {
            node.add_relay(url, terse_net::relay::RelayClient::websocket(url, terse_net::relay::DEFAULT_TIMEOUT));
        }
        node.set_subscriptions(Subscriptions::in_memory(w.followed.iter().map(Identity::feed_key)));
        node.set_identity(who(200));
        let mut content = ContentService::start_with_consent(node, Consent::Allow);
        if relay.is_some() {
            // Wait for the relay to connect, as a host does before its user acts.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match content.wait_feed_event(deadline.saturating_duration_since(std::time::Instant::now())) {
                    Some(terse_net::FeedEvent::Connected(_)) => break,
                    Some(_) => {}
                    None => panic!("the local relay did not connect"),
                }
            }
        }
        let h = Headless::new(Some(content), Script::parse(script).unwrap());
        Run { m: Machine::new(&Image::parse(IMAGE).unwrap()), h, used: Vec::new() }
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            let r = self.h.frame(&mut self.m);
            assert_eq!(r.stopped, None, "stopped at frame {}", self.m.frame());
            assert!(r.waited, "frame {} did not end in wait_frame ({} units; before: {:?})\n{}", self.m.frame(), r.used, self.used, self.m.terminal().unwrap().text());
            self.used.push(r.used);
        }
    }

    fn screen(&self) -> String {
        self.m.terminal().unwrap().text()
    }

    fn row(&self, n: usize) -> String {
        self.screen().lines().nth(n).unwrap_or("").trim_end().to_string()
    }
}

fn golden(name: &str, got: &str) {
    common::golden(&format!("reports/expected/{name}.txt"), got);
}

/// The verdicts the reference gives, by group label: (you follow, all found).
fn oracle(w: &World, found: &BTreeSet<(String, u64)>) -> Vec<(String, String, String)> {
    let mut a = Aggregate::default();
    for p in &w.published {
        let Some(r) = &p.report else { continue };
        if p.followed || found.contains(&(p.key.clone(), p.seq)) {
            a.add(&Submission { key: p.key.clone(), seq: p.seq, report: r });
        }
    }
    let followed: BTreeSet<String> = w.published.iter().filter(|p| p.followed).map(|p| p.key.clone()).collect();
    a.groups
        .iter()
        .map(|((kind, name, version, build), facts)| {
            let dash = |s: &String| if s.is_empty() { "-".to_string() } else { s.clone() };
            let label = format!("{} {} {} {}", kind.word(), name, dash(version), dash(build));
            let f = tally(facts.iter().filter(|(k, _)| followed.contains(*k)).map(|(_, f)| f)).verdict();
            let all = tally(facts.values()).verdict();
            (label, f, all)
        })
        .collect()
}

/// The table rows of the published aggregate: (label, you follow, all found).
fn article_rows(text: &str) -> Vec<(String, String, String)> {
    let mut rows: Vec<(String, String, String)> = text
        .lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| Identity"))
        .map(|l| {
            let c: Vec<&str> = l.trim_matches('|').split(" | ").map(str::trim).collect();
            (c[0].to_string(), c[1].to_string(), c[2].to_string())
        })
        .collect();
    rows.sort();
    rows
}

#[test]
fn followed_found_verdicts_and_the_published_aggregate() {
    let w = world();
    let (_relay_dir, url) = relay_for(&w);
    // The groups in the program's order (kind, then label): the page
    // driver's place in the list.
    let mut labels: Vec<(String, String)> = oracle(&w, &BTreeSet::new())
        .into_iter()
        .map(|(l, _, _)| {
            let (kind, rest) = l.split_once(' ').unwrap();
            (["host", "device", "perf"].iter().position(|k| *k == kind).unwrap().to_string(), rest.to_string())
        })
        .collect();
    labels.sort();
    let downs = labels.iter().position(|l| l.1 == "terse-dev-page 0.1.0 b3:d4d4d4d4d4d4d4d4").unwrap() as u32;
    // F at frame 150; the page driver's group at 620; back and P.
    let mut script = String::from("150 press f\n");
    for i in 0..downs {
        script.push_str(&format!("{} press down\n", 620 + i));
    }
    let enter = 620 + downs + 1;
    script.push_str(&format!("{enter} press enter\n{} press escape\n{} press p\n", enter + 20, enter + 30));
    let mut run = Run::new(&w, Some(&url), &script);
    // The timeline: every followed report taken, in how many frames.
    let followed_reports = w.published.iter().filter(|p| p.followed && p.report.is_some()).count();
    let mut done_at = None;
    for f in 0..150u32 {
        run.frames(1);
        if done_at.is_none() && run.row(2).contains(&format!(" {followed_reports} reports,")) {
            done_at = Some(f + 1);
        }
    }
    let done_at = done_at.unwrap_or_else(|| panic!("not every followed report was taken:\n{}", run.screen()));
    let rate = followed_reports as f64 / (done_at as f64 / 60.0);
    eprintln!("reports: {followed_reports} followed reports (and {} other documents) taken in {done_at} frames: {rate:.0} reports a second", w.malformed_reports + 3);
    assert!(rate > 60.0, "{rate}");
    assert!(run.row(2).contains(&format!("{} malformed.", w.malformed_reports)), "{}", run.row(2));
    assert!(run.row(2).starts_with("Followed: 11 reporters."), "the faker counts once: {}", run.row(2));
    golden("followed", &run.screen());
    // The search: three finds, 75 frames apart, then the gets.
    run.frames(470);
    assert!(run.row(28).starts_with("Search done"), "{}", run.screen());
    golden("found", &run.screen());
    // The same searches as the reference sees them.
    let client_dir = tempfile::tempdir().unwrap();
    let mut client = Node::new(BlobStore::open(client_dir.path(), None).unwrap(), None);
    client.add_relay(&url, terse_net::relay::RelayClient::websocket(&url, terse_net::relay::DEFAULT_TIMEOUT));
    let mut found = BTreeSet::new();
    for q in ["performance report", "performance report: terse-bus", "performance report: terse-host"] {
        let Done::Found(entries) = client.find(SCOPE_ENTRIES, q.as_bytes()) else { panic!("{q}") };
        for e in entries {
            found.insert((to_hex(&e.entry.feed), e.entry.seq as u64));
        }
    }
    let strangers: BTreeSet<&String> =
        found.iter().map(|(k, _)| k).filter(|k| !w.followed.iter().any(|f| to_hex(&f.feed_key()) == **k)).collect();
    assert!(strangers.len() > 10, "the relay's search found strangers: {}", strangers.len());
    // Strangers appear only under "all found".
    assert!(run.row(2).contains(&format!("All found: {} reporters", 11 + strangers.len())), "{}", run.row(2));
    // The page driver's group.
    run.frames(enter + 2 - run.m.frame());
    assert!(run.row(0).contains("device terse-dev-page 0.1.0 b3:d4d4d4d4d4d4d4d4"), "{}", run.screen());
    assert!(run.row(13).contains("you follow: NON-CONFORMANT IN SAMPLE (5 of 9 fail: halted 5)"), "{}", run.screen());
    golden("page-driver", &run.screen());
    run.frames(60);
    assert!(run.row(28).starts_with("Published the aggregate as article #"), "{}", run.screen());
    // The article, from the host's own feed, against the reference.
    let feeds = terse_net::FeedStore::beside_blobs(w.store.path());
    let mine = feeds.entries(&who(200).feed_key()).unwrap();
    let entry = &mine.last().unwrap().entry;
    assert_eq!(entry.kind, terse_net::entry::KIND_ARTICLE);
    let text = String::from_utf8(BlobStore::open(w.store.path(), None).unwrap().read(&entry.blob).unwrap().unwrap()).unwrap();
    assert!(text.starts_with("# Performance reports: an aggregate, 1970-01-01\n"), "{text}");
    golden("article", &text);
    let mut want = oracle(&w, &found);
    want.sort();
    assert_eq!(article_rows(&text), want, "the program's verdicts are the reference's");
    // The verdicts the owner asked for, in words.
    let row = |label: &str| want.iter().find(|r| r.0 == label).unwrap_or_else(|| panic!("{label}: {want:?}")).clone();
    assert_eq!(row("device terse-dev-page 0.1.0 b3:d4d4d4d4d4d4d4d4").1, "NON-CONFORMANT IN SAMPLE (5 of 9 fail: halted 5)", "the faker counts once");
    assert_eq!(row("device terse-dev-vector 0.1.0 b3:c3c3c3c3c3c3c3c3").1, "CONFORMANT IN SAMPLE (9 reporters)", "the faker counts once");
    assert!(row("device terse-dev-vector 0.1.0 b3:c3c3c3c3c3c3c3c3").2.starts_with("NON-CONFORMANT IN SAMPLE"), "the strangers outvote");
    assert_eq!(row("host terse-bus 0.1.0 b3:b2b2b2b2b2b2b2b2").1, "TOO FEW REPORTS (2)");
    assert_eq!(row("host terse-host 0.1.0 b3:a1a1a1a1a1a1a1a1").1, "CONFORMANT IN SAMPLE (9 reporters)");
    assert_eq!(row("perf terse 0.1.0 b3:e5e5e5e5e5e5e5e5").1, "CONFORMANT IN SAMPLE (5 reporters)");
    let busiest = run.used.iter().copied().max().unwrap();
    let mut top: Vec<(u64, usize)> = run.used.iter().copied().zip(0..).collect();
    top.sort();
    eprintln!("reports: the busiest frames {:?}", &top[top.len() - 8..]);
    eprintln!("reports: busiest frame {busiest} units ({:.1}% of {BUDGET_PER_FRAME})", common::pct(busiest));
}

/// Without a relay, or on a host that refuses find and publish: the
/// followed feeds still count; F and P say why they cannot.
#[test]
fn no_relay() {
    let w = world();
    let mut run = Run::new(&w, None, "150 press f\n");
    run.frames(160);
    assert!(run.row(28).starts_with("Search failed") || run.row(28).starts_with("Search done"), "{}", run.screen());
    assert!(run.row(2).starts_with("Followed: 11 reporters."), "{}", run.row(2));
}

/// The most reporters the program keeps, all followed, each with one
/// report: every frame ends within the budget, also the one that lists
/// them all (Enter on the host).
#[test]
fn many_reporters_stay_within_the_budget() {
    let store = tempfile::tempdir().unwrap();
    let author = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
    let followed: Vec<Identity> = (0..250u32).map(|i| Identity::from_seed([(i % 250) as u8, (i / 250) as u8 + 1, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 9])).collect();
    let mut w = World { store, author, time: 1_794_938_400, followed: followed.clone(), published: Vec::new(), malformed_reports: 0 };
    for (i, f) in followed.iter().enumerate() {
        let mut s = spec(4 + (i as u32 % 3) * 4, 8 << (i % 3));
        s.page_halted = i % 4 == 0;
        s.perf = Some(i % 7 == 0);
        w.report(f, &make(&s));
    }
    // Enter on the first group (the host) once all are in, then scroll.
    let mut run = Run::new(&w, None, "400 press enter\n410 press down\n411 press down\n420 press escape\n");
    run.frames(430);
    assert!(run.row(2).starts_with("Followed: 250 reporters."), "{}", run.row(2));
    let busiest = run.used.iter().copied().max().unwrap();
    eprintln!("reports: 250 reporters, busiest frame {busiest} units ({:.1}%)", common::pct(busiest));
    assert!(busiest < BUDGET_PER_FRAME as u64 * 9 / 10, "{busiest}");
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(IMAGE, &["reports/reports.c"], true, "reports", RAM_PAGES, &[(2, 0), (4, 0), (10, 0x80)]);
}

// --- terse perf report and terse perf aggregate --------------------------

#[test]
fn perf_report_prints_checks_and_publishes_only_when_told() {
    let dir = tempfile::tempdir().unwrap();
    let record = common::workspace().join("crates/terse-perflog/example-record.txt");
    let config = dir.path().join("config");
    let store = dir.path().join("store");
    let out = dir.path().join("r.doc");
    let base = |extra: &[&str]| {
        let mut c = common::terse_command();
        c.args(["perf", "report", "--record"]).arg(&record).args(["--to", "2026-11-21"]).args(extra);
        c
    };
    let o = base(&["--out", out.to_str().unwrap()]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(text.starts_with("terse-report 1\nperiod 2026-11-21 2026-11-21\nhost terse-bus 0.1.0"), "{text}");
    assert!(text.ends_with("end 20\n"), "{text}");
    let doc = std::fs::read(&out).unwrap();
    assert_eq!(report::parse_document(&doc).unwrap().text(), text);
    let o = common::terse_command().args(["perf", "report", "--check"]).arg(&out).output().unwrap();
    assert!(o.status.success() && String::from_utf8_lossy(&o.stdout).contains("valid: a performance report"));
    // Publishing: the whole text, the warnings, a question; "no" publishes nothing.
    let o = common::terse_command().args(["key", "new", "--config"]).arg(&config).args(["--store"]).arg(&store).output().unwrap();
    assert!(o.status.success());
    let publish = |answer: &str| {
        let mut c = base(&["--publish", "--relay", "none", "--config", config.to_str().unwrap(), "--store", store.to_str().unwrap()]);
        c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let mut child = c.spawn().unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), answer.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    };
    let o = publish("no\n");
    let said = String::from_utf8(o.stdout).unwrap();
    for want in ["It is PUBLIC", "It CANNOT BE DELETED", "signed with your feed key", "Type yes to publish", "Not published."] {
        assert!(said.contains(want), "{want}\n{said}");
    }
    let feeds = terse_net::FeedStore::beside_blobs(&store);
    let key = Identity::load(&config.join("identity.key")).unwrap().feed_key();
    assert!(feeds.entries(&key).unwrap().is_empty(), "nothing published");
    let o = publish("yes\n");
    assert!(String::from_utf8(o.stdout).unwrap().contains("Published: seq 1"));
    let held = feeds.entries(&key).unwrap();
    assert_eq!(held[0].entry.kind, terse_net::entry::KIND_DOCUMENT);
    assert_eq!(held[0].entry.title, "Performance report: terse-bus 0.1.0, 2026-11-21 to 2026-11-21");
}

#[test]
fn perf_aggregate_counts_one_per_key() {
    let dir = tempfile::tempdir().unwrap();
    let key = |n: u8| format!("{:064x}", n);
    let mut follow = String::new();
    // Five followed reporters, two halting the page driver; a sixth key
    // with three reports (the newest says nothing halted); a malformed one.
    for n in 1..=5u8 {
        let mut s = spec(8, 16);
        s.page_halted = n <= 2;
        std::fs::write(dir.path().join(format!("{}-1.txt", key(n))), make(&s).document()).unwrap();
        follow.push_str(&key(n));
        follow.push('\n');
    }
    for seq in 1..=3 {
        let mut s = spec(8, 16);
        s.page_halted = seq < 3;
        std::fs::write(dir.path().join(format!("{}-{seq}.txt", key(9))), make(&s).text()).unwrap();
    }
    std::fs::write(dir.path().join("broken.txt"), "terse-report 1\nend 0\n").unwrap();
    let keys_dir = tempfile::tempdir().unwrap();
    let keys = keys_dir.path().join("follow");
    std::fs::write(&keys, follow).unwrap();
    let o = common::terse_command().args(["perf", "aggregate", "--follow"]).arg(&keys).arg(dir.path()).output().unwrap();
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(o.status.success(), "{text}");
    assert!(text.contains("malformed: ") && text.contains("broken.txt: end-count"), "{text}");
    let page = text.split("\ndevice terse-dev-page").nth(1).unwrap();
    assert!(page.contains("feeds you follow: NON-CONFORMANT IN SAMPLE (2 of 5 fail: halted 2)"), "{text}");
    assert!(page.contains("all found: CONFORMANT IN SAMPLE (6 reporters)"), "the newest of key 9 counts: {text}");
}
