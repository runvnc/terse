//! `terse perf`: the responsiveness scenarios of PERFORMANCE.md, run on
//! this machine against the reference implementation.
//!
//! Every scenario runs headless, in temporary directories, with relays
//! started here on ephemeral loopback ports: nothing reads or writes the
//! user's config, cache or data directories, and nothing uses port 7878.
//!
//! Three tiers (PERFORMANCE.md §1): requirements are in frames, round
//! trips or "no network access", and pass or fail; ceilings are wall-clock
//! limits far above any correct implementation on any reasonable computer,
//! and fail on any machine; targets are the reference machine's numbers,
//! information unless `--strict`. Ceilings and targets judge the median of
//! the runs. A run exits with 1 if anything failed.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_core::page::reference_layout;
use terse_core::{Image, Machine, Outcome, Request, RequestKind, Script, System};
use terse_net::protocol::{GET, SUBSCRIBE};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::{
    BlobStore, Consent, ContentService, Draft, FeedEvent, HeadlessSystem, Identity, Node, Subscriptions, blob_id, builtin,
    to_hex,
};

/// A frame at 60 frames a second (SPEC §system `wait_frame`).
const FRAME: Duration = Duration::from_micros(16_667);

/// The frame bound for content that needs no network (PERFORMANCE.md M1).
const HELD_FRAMES: u32 = 2;

/// How many frames a scenario may run before it counts as never finishing.
const GIVE_UP: u32 = 600;

/// Entries published for the sync scenarios.
const SYNC_ENTRIES: u32 = 50;

pub const USAGE: &str = "terse perf [--only <text>] [--runs <n>] [--strict] [--programs <dir>] [--host <terse-host>] [--reader <tersereader>]\n       terse perf --write-worst <dir>   (the W rows' images, PERFORMANCE.md R10)";

/// A frame's time in milliseconds.
const FRAME_MS: f64 = 1000.0 / 60.0;

/// What a measurement counts, and how its limits are shown.
#[derive(Clone, Copy, PartialEq)]
enum Unit {
    /// Milliseconds.
    Ms,
    /// Milliseconds of the host's drawing, the limits shown in frame times.
    FrameTimes,
    /// The machine's frames, in the real-time loop.
    Frames,
}

/// A timed measurement's target and ceiling (PERFORMANCE.md §1): the
/// target is the reference machine's, information unless `--strict`; the
/// ceiling fails on any machine. Both judge the median of the runs.
#[derive(Clone, Copy, PartialEq)]
struct Limit {
    target: f64,
    ceiling: f64,
    unit: Unit,
}

/// A held article or the timeline shown by a whole process (`terse get`,
/// `terse feed list`, the Go reader): C2–C5, G2–G5.
const HELD_PROCESS: Limit = Limit { target: 25.0, ceiling: 250.0, unit: Unit::Ms };
/// A feed of 50 entries and their articles synced from a local relay: C1, G1.
const SYNC: Limit = Limit { target: 400.0, ceiling: 2000.0, unit: Unit::Ms };
/// A key to what it shows, everything held, in the real-time loop: M7–M9.
const SHOWN: Limit = Limit { target: 3.0, ceiling: 15.0, unit: Unit::Frames };
/// The host's start to its first frame drawn, software renderer: H1, H5.
const START: Limit = Limit { target: 50.0, ceiling: 1500.0, unit: Unit::Ms };
/// The same, cold: the first run on this machine as a new user has it (H7).
const COLD_START: Limit = Limit { target: 100.0, ceiling: 1500.0, unit: Unit::Ms };
/// The same on the GPU renderer, which starts the graphics driver (H8).
const GPU_START: Limit = Limit { target: 500.0, ceiling: 1500.0, unit: Unit::Ms };
/// A frame of the host's worst legal drawing on one thread (H2–H4, W rows,
/// the phosphor look shed): within a frame on the reference, 4 anywhere.
const WORST_FRAME: Limit = Limit { target: FRAME_MS, ceiling: 4.0 * FRAME_MS, unit: Unit::FrameTimes };
/// 1,200 CJK glyphs never drawn before, each frame, from a listed font (H6).
const NEW_GLYPHS: Limit = Limit { target: 2.0 * FRAME_MS, ceiling: 6.0 * FRAME_MS, unit: Unit::FrameTimes };
/// A shell and 8 machines, every device at its worst, one thread (W sys-*).
const SYSTEM: Limit = Limit { target: 4.0 * FRAME_MS, ceiling: 12.0 * FRAME_MS, unit: Unit::FrameTimes };

impl Limit {
    fn show(&self, v: f64) -> String {
        match self.unit {
            Unit::Ms => format!("{v:.0} ms"),
            Unit::Frames => format!("{v:.0} frames"),
            Unit::FrameTimes => {
                let n = (v / FRAME_MS).round();
                format!("{n:.0} frame{} ({v:.1} ms)", if n == 1.0 { "" } else { "s" })
            }
        }
    }

    /// "C ≤ 250 ms, T ≤ 25 ms".
    fn rule(&self) -> String {
        format!("C ≤ {}, T ≤ {}", self.show(self.ceiling), self.show(self.target))
    }
}

/// Several runs of one measurement, judged by their median.
struct Measure {
    /// What it is, if the row has several ("" if not).
    name: String,
    samples: Vec<f64>,
    limit: Limit,
}

impl Measure {
    fn new(name: &str, samples: Vec<f64>, limit: Limit) -> Self {
        Measure { name: name.to_string(), samples, limit }
    }

    fn median(&self) -> f64 {
        median(self.samples.clone())
    }

    fn show(&self) -> String {
        let unit = if self.limit.unit == Unit::Frames { "frames" } else { "ms" };
        let s = spread(self.samples.clone(), unit);
        if self.name.is_empty() { s } else { format!("{} {s}", self.name) }
    }
}

enum Verdict {
    Pass,
    Fail,
    Info,
    Skipped(String),
}

struct Row {
    id: &'static str,
    what: String,
    measured: String,
    rule: String,
    verdict: Verdict,
    /// Timed measurements with a ceiling and a target.
    measures: Vec<Measure>,
}

/// A row with no timed measurement.
fn row(id: &'static str, what: impl Into<String>, measured: impl Into<String>, rule: impl Into<String>, verdict: Verdict) -> Row {
    Row { id, what: what.into(), measured: measured.into(), rule: rule.into(), verdict, measures: Vec::new() }
}

impl Row {
    fn with(mut self, m: Measure) -> Self {
        self.measures.push(m);
        self
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// A directory removed when dropped.
struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Result<Self, String> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("terse-perf-{}-{n}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Tmp(dir))
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.0.join(sub)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A relay started in this process on an ephemeral loopback port.
struct LocalRelay {
    url: String,
    relay: Arc<terse_relay::Relay>,
    _dir: Tmp,
}

impl LocalRelay {
    fn start() -> Result<Self, String> {
        let dir = Tmp::new("relay")?;
        let store = BlobStore::open(&dir.0, None).map_err(|e| e.to_string())?;
        let relay = Arc::new(terse_relay::Relay::new(store, terse_relay::DEFAULT_FEED_CAP));
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let url = format!("ws://{}", listener.local_addr().map_err(|e| e.to_string())?);
        let serving = relay.clone();
        std::thread::spawn(move || terse_relay::serve(listener, serving));
        Ok(LocalRelay { url, relay, _dir: dir })
    }

    fn gets(&self) -> usize {
        self.relay.received(GET)
    }
}

/// A "relay" that accepts TCP connections and never answers the WebSocket
/// handshake: a client stays connecting until its timeout (10 s).
struct SilentRelay {
    url: String,
    _listener: TcpListener,
}

impl SilentRelay {
    fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let url = format!("ws://{}", listener.local_addr().map_err(|e| e.to_string())?);
        Ok(SilentRelay { url, _listener: listener })
    }
}

/// The relays of a scenario.
enum Net<'a> {
    None,
    Connected(&'a LocalRelay),
    Silent(&'a SilentRelay),
}

impl Net<'_> {
    fn urls(&self) -> Vec<String> {
        match self {
            Net::None => Vec::new(),
            Net::Connected(r) => vec![r.url.clone()],
            Net::Silent(r) => vec![r.url.clone()],
        }
    }
}

/// A content service on `store`, with `net`'s relays, waiting until a
/// connected relay is connected.
fn service(store: &Path, net: &Net, subscriptions: Option<Subscriptions>) -> Result<ContentService, String> {
    let mut node = Node::open(Some(store.to_path_buf()), &net.urls(), DEFAULT_TIMEOUT)?;
    node.set_feed_relays(false);
    if let Some(s) = subscriptions {
        node.set_subscriptions(s);
    }
    let mut service = ContentService::start_with_consent(node, Consent::Deny);
    if let Net::Connected(_) = net {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match service.wait_feed_event(deadline.saturating_duration_since(Instant::now())) {
                Some(FeedEvent::Connected(_)) => break,
                Some(_) => {}
                None => return Err("the local relay did not connect".into()),
            }
        }
    }
    Ok(service)
}

/// Runs frames at 60 a second until `done` says so (given the frame just
/// run, counted from 0) or [`GIVE_UP`]; returns the frame it was true in.
fn paced(mut frame: impl FnMut(u32) -> bool) -> Option<u32> {
    let mut next = Instant::now();
    for f in 0..GIVE_UP {
        if frame(f) {
            return Some(f);
        }
        next += FRAME;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    None
}

/// `fetch` (programs/fetch) in a real-time loop: the frame in which its
/// get's status is on the screen (it submits the get in frame 0).
fn fetch_frames(content: ContentService) -> Result<(Option<u32>, String), String> {
    let image = Image::parse(builtin::PROGRAMS[2]).map_err(|e| e.to_string())?;
    let mut machine = Machine::new(&image);
    let mut headless = terse_net::service::Headless::new(Some(content), Script::default()).realtime();
    let at = paced(|_| {
        headless.frame(&mut machine);
        machine.terminal().is_some_and(|t| t.text().contains("fetching… status"))
    });
    let text = machine.terminal().map(|t| t.text()).unwrap_or_default();
    let status = text.lines().find(|l| l.contains("fetching… status")).unwrap_or("").replace("fetching… ", "");
    Ok((at, status))
}

/// How long the service takes to finish one request, waiting for it
/// without frames (µs), and its outcome.
fn service_latency(content: &mut ContentService, kind: RequestKind) -> (Duration, Option<Outcome>) {
    let start = Instant::now();
    content.submit(vec![Request { handle: 1, kind }]);
    while start.elapsed() < Duration::from_secs(15) {
        if let Some(c) = content.take_finished().pop() {
            return (start.elapsed(), Some(c.outcome));
        }
        std::thread::yield_now();
    }
    (start.elapsed(), None)
}

fn frames_rule(at: Option<u32>, max: u32) -> (String, Verdict) {
    match at {
        Some(f) => (format!("{f} frames"), if f <= max { Verdict::Pass } else { Verdict::Fail }),
        None => (format!("not in {GIVE_UP} frames"), Verdict::Fail),
    }
}

fn ms(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1.0 { format!("{:.0} µs", ms * 1000.0) } else { format!("{ms:.1} ms") }
}

/// Median and spread of several runs, as "median (min–max)".
fn spread(mut v: Vec<f64>, unit: &str) -> String {
    v.sort_by(f64::total_cmp);
    let mid = v[v.len() / 2];
    if v.len() == 1 { format!("{mid:.1} {unit}") } else { format!("{mid:.1} {unit} ({:.1}–{:.1})", v[0], v[v.len() - 1]) }
}

/// A store holding the built-in programs' data (fetch's message) and the
/// manual.
fn held_store(tmp: &Tmp) -> Result<PathBuf, String> {
    let dir = tmp.path("store");
    let store = BlobStore::open(&dir, None).map_err(|e| e.to_string())?;
    builtin::install(&store, &[])?;
    Ok(dir)
}

fn machine_rows(rows: &mut Vec<Row>, runs: usize) -> Result<(), String> {
    let message = include_bytes!("../../../programs/fetch/message.txt");
    let relay = LocalRelay::start()?;
    let silent = SilentRelay::start()?;

    // M1: held, no relay; with a connected relay (which must not be asked);
    // with a relay that never answers.
    for (id, label, net) in [
        ("M1a", "held get, no relay", Net::None),
        ("M1b", "held get, local relay connected", Net::Connected(&relay)),
        ("M1c", "held get, relay never answers", Net::Silent(&silent)),
    ] {
        let tmp = Tmp::new("m1")?;
        let store = held_store(&tmp)?;
        let gets = relay.gets();
        let mut svc = service(&store, &net, None)?;
        let (latency, _) = service_latency(&mut svc, RequestKind::Get { id: blob_id(message), offset: 0, length: 16 });
        let (at, status) = fetch_frames(svc)?;
        let asked = relay.gets() - gets;
        let (measured, mut verdict) = frames_rule(at, HELD_FRAMES);
        if asked > 0 || !status.contains("done") {
            verdict = Verdict::Fail;
        }
        rows.push(row(
            id,
            format!("{label}: submit to content-done (fetch)"),
            format!("{measured}; service {}; relay asked {asked} times; {status}", ms(latency)),
            format!("R6: ≤ {HELD_FRAMES} frames; R1: no relay asked"),
            verdict,
        ));
    }

    // M2: not held, on a connected relay: one round trip.
    let mut lat = Vec::new();
    let mut frames = Vec::new();
    let mut trips = 0;
    for run in 0..runs {
        let tmp = Tmp::new("m2")?;
        let store = tmp.path("store");
        BlobStore::open(&store, None).map_err(|e| e.to_string())?;
        let blob = format!("not held {run} {}", std::process::id()).into_bytes();
        relay.relay.blobs().put(&blob).map_err(|e| e.to_string())?;
        let mut svc = service(&store, &Net::Connected(&relay), None)?;
        let before = relay.gets();
        let (latency, outcome) = service_latency(&mut svc, RequestKind::Get { id: blob_id(&blob), offset: 0, length: 16 });
        trips = relay.gets() - before;
        if !matches!(outcome, Some(Outcome::Got { .. })) {
            return Err(format!("M2: the relay's blob was not delivered: {outcome:?}"));
        }
        lat.push(latency.as_secs_f64() * 1000.0);
        // The same through fetch: its message on the relay only.
        let tmp = Tmp::new("m2f")?;
        let store = tmp.path("store");
        BlobStore::open(&store, None).map_err(|e| e.to_string())?;
        relay.relay.blobs().put(message).map_err(|e| e.to_string())?;
        let (at, _) = fetch_frames(service(&store, &Net::Connected(&relay), None)?)?;
        frames.push(at.unwrap_or(GIVE_UP) as f64);
    }
    rows.push(row(
        "M2",
        "get not held, on a connected local relay",
        format!("{trips} Get; service {}; fetch {}", spread(lat, "ms"), spread(frames, "frames")),
        "R2: 1 round trip",
        if trips == 1 { Verdict::Pass } else { Verdict::Fail },
    ));

    // M3: not held and no relay: not found at once.
    let tmp = Tmp::new("m3")?;
    let store = tmp.path("store");
    BlobStore::open(&store, None).map_err(|e| e.to_string())?;
    let (at, status) = fetch_frames(service(&store, &Net::None, None)?)?;
    let (measured, verdict) = frames_rule(at, HELD_FRAMES);
    rows.push(row("M3", "get not held, no relay: not found", format!("{measured}; {status}"), format!("R6: ≤ {HELD_FRAMES} frames"), verdict));

    // M4: the timeline from the store while the only relay never answers.
    let tmp = Tmp::new("m4")?;
    let store = held_store(&tmp)?;
    let key = Identity::from_seed([7; 32]).feed_key();
    let mut svc = service(&store, &Net::Silent(&silent), Some(Subscriptions::in_memory([key])))?;
    let (latency, outcome) = service_latency(&mut svc, RequestKind::Timeline { offset: 0, length: 4096 });
    let ok = matches!(outcome, Some(Outcome::Records { .. })) && latency < FRAME * HELD_FRAMES;
    rows.push(row(
        "M4",
        "timeline request, subscribed, relay never answers",
        ms(latency),
        format!("R3, R6: ≤ {HELD_FRAMES} frames"),
        if ok { Verdict::Pass } else { Verdict::Fail },
    ));

    // M10: programs/conform-perf in the real-time loop: its own PASS
    // lines, and the frame rate against the clock (about 6 s). (No relay:
    // its put would wait for one that never answers, NETWORK.md 5.1.)
    let tmp = Tmp::new("conform-perf")?;
    let store = tmp.path("store");
    BlobStore::open(&store, None).map_err(|e| e.to_string())?;
    let image = Image::parse(include_bytes!("../../../programs/conform-perf/conform-perf.terse")).map_err(|e| e.to_string())?;
    let mut machine = Machine::new(&image);
    let mut headless = terse_net::service::Headless::new(Some(service(&store, &Net::None, None)?), Script::default()).realtime();
    paced(|_| {
        headless.frame(&mut machine);
        machine.terminal().is_some_and(|t| t.text().contains("requirement"))
    });
    let text = machine.terminal().map(|t| t.text()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("PASS") || l.contains("FAIL") || l.contains("not checked")).collect();
    let short = |l: &str| l.split("  ").next().unwrap_or(l).trim().to_string();
    rows.push(row(
        "M10",
        "programs/conform-perf in the real-time loop",
        lines.iter().map(|l| short(l)).collect::<Vec<_>>().join("; "),
        "R6, R8: every line PASS",
        if !lines.is_empty() && lines.iter().all(|l| l.contains("PASS")) { Verdict::Pass } else { Verdict::Fail },
    ));

    shell_rows(rows, runs)?;
    reader_rows(rows, runs)?;
    Ok(())
}

/// The shell in a real-time loop: frames to the launcher, and from Enter
/// on `fetch` to fetch's first frame and to its text.
fn shell_rows(rows: &mut Vec<Row>, runs: usize) -> Result<(), String> {
    let mut launcher = Vec::new();
    let (mut start, mut text, mut wall) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..runs {
        let tmp = Tmp::new("shell")?;
        let dir = tmp.path("store");
        let store = BlobStore::open(&dir, None).map_err(|e| e.to_string())?;
        let builtins = builtin::install(&store, &[])?;
        let content = service(&dir, &Net::None, None)?;
        let image = Image::parse(builtin::SHELL).map_err(|e| e.to_string())?;
        let mut system = System::new(&image, true, builtins);
        let script = Script::parse("2 press down\n3 press down\n4 press enter\n")?;
        let mut headless = HeadlessSystem::new(Some(content), script).realtime();
        let (mut shown, mut first, mut done) = (None, None, None);
        let mut enter_at = None;
        paced(|f| {
            if f == 4 {
                enter_at = Some(Instant::now());
            }
            headless.frame(&mut system);
            let screen = system.shell().terminal().map(|t| t.text()).unwrap_or_default();
            if shown.is_none() && screen.contains("fetch") {
                shown = Some(f);
            }
            let fetch = system.children().find(|(_, c)| c.title == "fetch").and_then(|(_, c)| c.machine.as_ref());
            if first.is_none() && fetch.is_some_and(|m| m.frame() > 0) {
                first = Some(f);
            }
            if fetch.and_then(|m| m.terminal()).is_some_and(|t| t.text().contains("fetching… status")) {
                done = Some(f);
                if let Some(t) = enter_at {
                    wall.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                return true;
            }
            false
        });
        launcher.push(shown.unwrap_or(GIVE_UP) as f64);
        start.push(first.map_or(GIVE_UP as f64, |f| f as f64 - 4.0));
        text.push(done.map_or(GIVE_UP as f64, |f| f as f64 - 4.0));
    }
    rows.push(row("M5", "shell start to the launcher on screen", spread(launcher, "frames"), "the shell's own frames", Verdict::Info));
    let worst = start.iter().copied().fold(0.0, f64::max);
    rows.push(row(
        "M6",
        "program start (held image): Enter on fetch to fetch's first frame",
        spread(start, "frames"),
        format!("R9: ≤ {HELD_FRAMES} frames"),
        if worst <= HELD_FRAMES as f64 { Verdict::Pass } else { Verdict::Fail },
    ));
    rows.push(
        row("M7", "Enter on fetch to its message on screen (emulation only)", spread(wall, "ms"), "", Verdict::Info)
            .with(Measure::new("", text, SHOWN)),
    );
    Ok(())
}

/// The page's text as the reference layout lays it out.
fn page_text(m: &Machine) -> String {
    let Some(page) = m.page() else { return String::new() };
    let layout = page.document().map(|d| reference_layout(d, page.region().w)).unwrap_or_default();
    layout.lines.iter().map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>() + "\n").collect()
}

/// The reader (programs/reader) in a real-time loop: M opens the manual's
/// index; Tab and Enter follow its first link. Both articles are held.
fn reader_rows(rows: &mut Vec<Row>, runs: usize) -> Result<(), String> {
    let (mut opened, mut followed) = (Vec::new(), Vec::new());
    for _ in 0..runs {
        let (o, f) = reader_run()?;
        opened.push(o.map_or(GIVE_UP as f64, |f| f.saturating_sub(10) as f64));
        followed.push(f.map_or(GIVE_UP as f64, |f| f.saturating_sub(6) as f64));
    }
    rows.push(
        row("M8", "reader: M to the manual's index on the page (held)", "", "", Verdict::Info)
            .with(Measure::new("", opened, SHOWN)),
    );
    rows.push(
        row("M9", "reader: Enter on a link to the article on the page (held)", "", "", Verdict::Info)
            .with(Measure::new("", followed, SHOWN)),
    );
    Ok(())
}

/// One run of [`reader_rows`]: the frames in which the index and the
/// article were on the page.
fn reader_run() -> Result<(Option<u32>, Option<u32>), String> {
    let tmp = Tmp::new("reader")?;
    let store = held_store(&tmp)?;
    let content = service(&store, &Net::None, None)?;
    let image = Image::parse(builtin::PROGRAMS[0]).map_err(|e| e.to_string())?;
    let mut machine = Machine::new(&image);
    let index_title = builtin::MANUAL[0].1;
    let script = Script::parse("10 press m\n")?;
    let mut headless = terse_net::service::Headless::new(Some(content), script).realtime();
    let opened = paced(|f| {
        headless.frame(&mut machine);
        f > 10 && page_text(&machine).contains(index_title)
    });
    // Let the index settle, then follow its first link.
    let mut index = String::new();
    paced(|_| {
        headless.frame(&mut machine);
        let now = page_text(&machine);
        let same = now == index;
        index = now;
        same
    });
    let base = machine.frame();
    let script = Script::parse(&format!("{} press tab\n{} press enter\n", base + 2, base + 6))?;
    let content = headless_content(headless);
    let mut headless = terse_net::service::Headless::new(content, script).realtime();
    let followed = paced(|_| {
        headless.frame(&mut machine);
        let now = page_text(&machine);
        machine.frame() > base + 6 && !now.is_empty() && now != index
    });
    Ok((opened, followed))
}

fn headless_content(h: terse_net::service::Headless) -> Option<ContentService> {
    h.into_content()
}

/// Runs a command, returning its wall time and output; fails on a
/// non-zero status.
fn timed(cmd: &mut Command) -> Result<(Duration, String), String> {
    let start = Instant::now();
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| format!("{cmd:?}: {e}"))?;
    let took = start.elapsed();
    if !out.status.success() {
        return Err(format!("{cmd:?}: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok((took, String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)))
}

/// A command that sees only temporary directories as its home.
fn isolated(program: &Path, home: &Tmp) -> Command {
    let mut c = Command::new(program);
    for (var, sub) in [("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"), ("XDG_DATA_HOME", "data"), ("XDG_RUNTIME_DIR", "run")] {
        let dir = home.path(sub);
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        c.env(var, dir);
    }
    c.env_remove("TERSE_NODE_SOCKET").env_remove("TERSEREADER_DIR");
    c
}

/// Publishes [`SYNC_ENTRIES`] small articles of a new feed to `relay`;
/// returns the feed key and the first article's blob ID.
fn publish_feed(relay: &LocalRelay) -> Result<([u8; 32], [u8; 32]), String> {
    let tmp = Tmp::new("author")?;
    let mut node = Node::open(Some(tmp.path("store")), std::slice::from_ref(&relay.url), DEFAULT_TIMEOUT)?;
    let author = Identity::from_seed([42; 32]);
    node.set_identity(author.clone());
    let mut first = None;
    for i in 0..SYNC_ENTRIES {
        let text = format!("# Article {i}\n\nA small article for terse perf, number {i}.\n");
        let draft = Draft { data: text.clone().into_bytes(), title: format!("Article {i}"), reply_to: None, time: 1_700_000_000 + i };
        match node.publish_article(&draft) {
            terse_net::Done::Published { .. } => first.get_or_insert(blob_id(text.as_bytes())),
            done => return Err(format!("publishing to the local relay: {done:?}")),
        };
    }
    Ok((author.feed_key(), first.unwrap()))
}

/// `terse` with `home`'s store and config and `url` as its only relay.
fn cli(terse: &Path, home: &Tmp, url: &str, args: &[&str]) -> Command {
    let mut c = isolated(terse, home);
    c.args(args).args(["--relay", url, "--store"]).arg(home.path("store")).arg("--config").arg(home.path("cfg")).arg("--no-feed-relays");
    c
}

/// `tersereader` with its directory in `home` and `url` as its only relay.
fn go(reader: &Path, home: &Tmp, url: &str, args: &[&str]) -> Command {
    let mut c = isolated(reader, home);
    c.arg("--dir").arg(home.path("reader")).args(["--node", "in-process", "--relay", url, "--no-feed-relays"]).args(args);
    c
}

/// Runs `cmd` `runs` times; the times in milliseconds and the last output.
fn repeat(runs: usize, mut cmd: impl FnMut() -> Command) -> Result<(Vec<f64>, String), String> {
    let mut v = Vec::new();
    let mut out = String::new();
    for _ in 0..runs {
        let (took, o) = timed(&mut cmd())?;
        v.push(took.as_secs_f64() * 1000.0);
        out = o;
    }
    Ok((v, out))
}

/// The worst of several runs is under a second: "no wait" for a relay that
/// never answers (it would time out after 10 s).
fn no_wait(v: &[f64]) -> Verdict {
    if v.iter().all(|&t| t < 1000.0) { Verdict::Pass } else { Verdict::Fail }
}

fn client_rows(rows: &mut Vec<Row>, runs: usize, reader: Option<PathBuf>) -> Result<(), String> {
    let terse = std::env::current_exe().map_err(|e| e.to_string())?;
    let relay = LocalRelay::start()?;
    let silent = SilentRelay::start()?;
    let (key, article) = publish_feed(&relay)?;
    let (key, article) = (to_hex(&key), to_hex(&article));

    // The reference command line as a reading client. Each sync starts
    // from an empty store; the last one's is used for the rest.
    let (mut sync, mut subscribes, mut gets) = (Vec::new(), Vec::new(), 0);
    let mut home = None;
    for _ in 0..runs {
        let h = Tmp::new("cli")?;
        timed(&mut cli(&terse, &h, &relay.url, &["feed", "subscribe", &key]))?;
        let (subs, before) = (relay.relay.received(SUBSCRIBE), relay.gets());
        let (took, _) = timed(&mut cli(&terse, &h, &relay.url, &["feed", "sync"]))?;
        sync.push(took.as_secs_f64() * 1000.0);
        subscribes.push(relay.relay.received(SUBSCRIBE) - subs);
        gets = relay.gets() - before;
        home = Some(h);
    }
    let home = home.ok_or("no runs")?;
    let out = home.path("out");
    let out = out.to_str().ok_or("a temporary path that is not UTF-8")?;
    rows.push(
        row(
            "C1",
            format!("terse feed sync: {SYNC_ENTRIES} new entries and articles from a local relay"),
            format!("{} Subscribe, {gets} Get", subscribes.iter().max().unwrap_or(&0)),
            "R4: 1 Subscribe per feed",
            if subscribes.iter().all(|&n| n == 1) { Verdict::Pass } else { Verdict::Fail },
        )
        .with(Measure::new("", sync, SYNC)),
    );
    let before = relay.gets();
    let (held, _) = repeat(runs, || cli(&terse, &home, &relay.url, &["get", &article, "-o", out]))?;
    let asked = relay.gets() - before;
    rows.push(
        row(
            "C2",
            "terse get of a held article (process start to exit), relay connected",
            format!("relay asked {asked} times"),
            "R1: no relay asked",
            if asked == 0 { Verdict::Pass } else { Verdict::Fail },
        )
        .with(Measure::new("", held, HELD_PROCESS)),
    );
    let (v, _) = repeat(runs, || cli(&terse, &home, &silent.url, &["get", &article, "-o", out]))?;
    rows.push(
        row("C3", "terse get of a held article, relay never answers", "", "R1: no wait for the relay (< 1 s; it times out at 10 s)", no_wait(&v))
            .with(Measure::new("", v, HELD_PROCESS)),
    );
    let (v, list) = repeat(runs, || cli(&terse, &home, &silent.url, &["feed", "list"]))?;
    rows.push(
        row(
            "C4",
            "terse feed list (the timeline from the store), relay never answers",
            format!("{} entries", list.lines().filter(|l| l.contains("Article")).count()),
            "R3: no wait for the relay (< 1 s)",
            no_wait(&v),
        )
        .with(Measure::new("", v, HELD_PROCESS)),
    );
    let mut not_held = Vec::new();
    let mut trips = Vec::new();
    for run in 0..runs {
        let blob = format!("C5 {run} {}", std::process::id()).into_bytes();
        relay.relay.blobs().put(&blob).map_err(|e| e.to_string())?;
        let before = relay.gets();
        not_held.push(timed(&mut cli(&terse, &home, &relay.url, &["get", &to_hex(&blob_id(&blob)), "-o", out]))?.0.as_secs_f64() * 1000.0);
        trips.push(relay.gets() - before);
    }
    rows.push(
        row(
            "C5",
            "terse get of an article not held, from a local relay",
            format!("{} Get", trips.iter().max().unwrap_or(&0)),
            "R2: 1 round trip",
            if trips.iter().all(|&n| n == 1) { Verdict::Pass } else { Verdict::Fail },
        )
        .with(Measure::new("", not_held, HELD_PROCESS)),
    );

    // The Go reader, an independent implementation, if it is built.
    let Some(reader) = reader else {
        rows.push(row(
            "G",
            "Go reader (native/reader-go/tersereader)",
            "",
            "",
            Verdict::Skipped("not built (cd native/reader-go && go build -o tersereader .)".into()),
        ));
        return Ok(());
    };
    let (mut sync, mut subscribes, mut gets) = (Vec::new(), Vec::new(), 0);
    let mut home = None;
    for _ in 0..runs {
        let h = Tmp::new("go")?;
        timed(&mut go(&reader, &h, &relay.url, &["subscribe", &key]))?;
        let (subs, before) = (relay.relay.received(SUBSCRIBE), relay.gets());
        let (took, _) = timed(&mut go(&reader, &h, &relay.url, &["sync", "--timeout", "20s"]))?;
        sync.push(took.as_secs_f64() * 1000.0);
        subscribes.push(relay.relay.received(SUBSCRIBE) - subs);
        gets = relay.gets() - before;
        home = Some(h);
    }
    let home = home.ok_or("no runs")?;
    rows.push(
        row(
            "G1",
            format!("tersereader sync: {SYNC_ENTRIES} new entries and articles from a local relay"),
            format!("{} Subscribe, {gets} Get", subscribes.iter().max().unwrap_or(&0)),
            "R4: 1 Subscribe per feed",
            if subscribes.iter().all(|&n| n == 1) { Verdict::Pass } else { Verdict::Fail },
        )
        .with(Measure::new("", sync, SYNC)),
    );
    let before = relay.gets();
    let (dump, _) = repeat(runs, || go(&reader, &home, &relay.url, &["--dump", &article, "--plain", "--width", "80"]))?;
    let asked = relay.gets() - before;
    rows.push(
        row(
            "G2",
            "tersereader --dump of a held article, relay connected",
            format!("relay asked {asked} times"),
            "R1: no relay asked",
            if asked == 0 { Verdict::Pass } else { Verdict::Fail },
        )
        .with(Measure::new("", dump, HELD_PROCESS)),
    );
    let (v, _) = repeat(runs, || go(&reader, &home, &silent.url, &["--dump", &article, "--plain"]))?;
    rows.push(
        row("G3", "tersereader --dump of a held article, relay never answers", "", "R1: no wait for the relay (< 1 s)", no_wait(&v))
            .with(Measure::new("", v, HELD_PROCESS)),
    );
    let (v, list) = repeat(runs, || go(&reader, &home, &silent.url, &["list"]))?;
    rows.push(
        row(
            "G4",
            "tersereader list (the timeline from the store), relay never answers",
            format!("{} entries", list.lines().filter(|l| l.contains("Article")).count()),
            "R3: no wait for the relay (< 1 s)",
            no_wait(&v),
        )
        .with(Measure::new("", v, HELD_PROCESS)),
    );

    // The same through the local node protocol (LOCAL.md): the Go node on
    // a socket of its own, a reader process asking it.
    // LOCAL.md 2: the socket's directory is the user's alone.
    let sockets = home.path("sockets");
    std::fs::create_dir_all(&sockets).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sockets, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    let socket = sockets.join("node.sock");
    let mut node = isolated(&reader, &home);
    node.arg("--dir").arg(home.path("reader")).args(["--relay", &silent.url, "--no-feed-relays", "--socket"]).arg(&socket).arg("node");
    let child = node.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let _child = Killed(child);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let via_node = |args: &[&str]| {
        let mut c = isolated(&reader, &home);
        c.args(["--node", "socket", "--socket"]).arg(&socket).args(args);
        c
    };
    let (dump, _) = repeat(runs, || via_node(&["--dump", &article, "--plain", "--width", "80"]))?;
    let (list, out) = repeat(runs, || via_node(&["list"]))?;
    let verdict = match (no_wait(&dump), no_wait(&list)) {
        (Verdict::Pass, Verdict::Pass) => Verdict::Pass,
        _ => Verdict::Fail,
    };
    rows.push(
        row(
            "G5",
            "tersereader via its node on the local socket: --dump held, list; relay never answers",
            format!("{} entries", out.lines().filter(|l| l.contains("Article")).count()),
            "R5: no wait for the relay (< 1 s)",
            verdict,
        )
        .with(Measure::new("--dump", dump, HELD_PROCESS))
        .with(Measure::new("list", list, HELD_PROCESS)),
    );
    Ok(())
}

/// A child process killed and reaped when dropped.
struct Killed(std::process::Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `--bench-render`'s line: "... p50 X ms, p99 Y ms, max Z ms, ...".
fn bench_numbers(out: &str) -> Option<(f64, f64, f64)> {
    let line = out.lines().find(|l| l.contains(" p50 "))?;
    let num = |key: &str| -> Option<f64> { line.split(key).nth(1)?.split_whitespace().next()?.parse().ok() };
    Some((num(" p50 ")?, num(" p99 ")?, num(" max ")?))
}

fn host_rows(rows: &mut Vec<Row>, runs: usize, host: Option<PathBuf>, programs: Option<PathBuf>) -> Result<(), String> {
    let Some(host) = host else {
        rows.push(row("H", "terse-host (rendering)", "", "", Verdict::Skipped("terse-host not found beside terse (--host)".into())));
        return Ok(());
    };
    let home = Tmp::new("host")?;
    let images = Tmp::new("images")?;
    let image = |name: &str, bytes: &[u8]| -> Result<PathBuf, String> {
        let p = images.path(name);
        std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
        Ok(p)
    };
    let fetch = image("fetch.terse", builtin::PROGRAMS[2])?;
    let hello = image("hello.terse", builtin::PROGRAMS[3])?;
    let store = held_store(&home)?;
    let bench = |img: &Path, extra: &[&str]| {
        let mut c = isolated(&host, &home);
        c.arg(img).args(["--bench-render", "--renderer", "software", "--store"]).arg(&store);
        c.arg("--config").arg(home.path("cfg")).arg("--data").arg(home.path("dat")).args(extra);
        c
    };
    // One frame drawn, then the host exits: the whole process is timed.
    const FIRST: [&str; 4] = ["--warmup", "0", "--frames", "1"];
    let mut start = Vec::new();
    let mut first_cjk = Vec::new();
    for _ in 0..runs {
        let (took, _) = timed(&mut bench(&hello, &FIRST))?;
        start.push(took.as_secs_f64() * 1000.0);
        let (_, out) = timed(&mut bench(&fetch, &["--warmup", "0", "--frames", "8"]))?;
        first_cjk.push(bench_numbers(&out).ok_or("no --bench-render line")?.2);
    }
    rows.push(
        row("H1", "terse-host start to hello's first frame drawn (software, offscreen)", "", "R13", Verdict::Info)
            .with(Measure::new("", start, START)),
    );
    rows.push(
        row("H2", "fetch: slowest frame drawn (its first Greek, Cyrillic and symbols, 50 ms after start)", "", "R11", Verdict::Info)
            .with(Measure::new("", first_cjk, WORST_FRAME)),
    );
    cold_start(rows, runs, &host)?;
    // The GPU renderer: its start includes the graphics driver's.
    let mut gpu = Vec::new();
    let mut failed = None;
    for _ in 0..runs {
        let mut args = FIRST.to_vec();
        args.extend(["--renderer", "gpu"]);
        match timed(&mut bench(&hello, &args)) {
            Ok((took, _)) => gpu.push(took.as_secs_f64() * 1000.0),
            Err(e) => {
                failed = Some(e.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string());
                break;
            }
        }
    }
    let what = "terse-host start to hello's first frame drawn, GPU renderer (offscreen)";
    rows.push(match failed {
        Some(e) => row("H8", what, "", "", Verdict::Skipped(format!("no GPU renderer here: {e}"))),
        None => row("H8", what, "", "R13", Verdict::Info).with(Measure::new("", gpu, GPU_START)),
    });
    // R11: the frame that first shows 2,400 of the full repertoire's glyphs
    // (term-full's first frame), against term-basic's first frame (90 ASCII
    // glyphs), both on one thread; and start-up and the CJK frames with a
    // 19 MB CJK collection listed as an extra font, if this system has one.
    let worst = terse_core::worst::scenarios();
    let first_frame = |id: &str, extra: &[&str]| -> Result<Vec<f64>, String> {
        let s = worst.iter().find(|s| s.id == id).ok_or("no scenario")?;
        let img = image(&format!("{id}.terse"), &s.image)?;
        let mut firsts = Vec::new();
        for _ in 0..runs {
            let mut args = vec!["--warmup", "0", "--frames", "2", "--threads", "1"];
            args.extend_from_slice(extra);
            let (_, out) = timed(&mut bench(&img, &args))?;
            firsts.push(bench_numbers(&out).ok_or("no --bench-render line")?.2);
        }
        Ok(firsts)
    };
    rows.push(
        row(
            "H4",
            "first frame showing 2,400 new glyphs of the full repertoire, 1 thread (term-full)",
            format!("90 ASCII glyphs (term-basic): {}", spread(first_frame("term-basic", &[])?, "ms")),
            "R11 (no font found or opened)",
            Verdict::Info,
        )
        .with(Measure::new("", first_frame("term-full", &[])?, WORST_FRAME)),
    );
    let cjk = ["/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file());
    match cjk {
        None => rows.push(row(
            "H5",
            "start-up and CJK with a CJK font listed",
            "",
            "",
            Verdict::Skipped("no NotoSansCJK-Regular.ttc on this system".into()),
        )),
        Some(cjk) => {
            let cfg = home.path("cfg-cjk");
            std::fs::create_dir_all(&cfg).map_err(|e| e.to_string())?;
            std::fs::write(cfg.join("fonts"), format!("extra {}\n", cjk.display())).map_err(|e| e.to_string())?;
            let cfg = cfg.display().to_string();
            let mut start = Vec::new();
            for _ in 0..runs {
                let (took, _) = timed(&mut bench(&hello, &["--warmup", "0", "--frames", "1", "--config", &cfg]))?;
                start.push(took.as_secs_f64() * 1000.0);
            }
            rows.push(
                row(
                    "H5",
                    format!("terse-host start to hello's first frame, {} listed as an extra font", cjk.display()),
                    "",
                    "R11, R13",
                    Verdict::Info,
                )
                .with(Measure::new("", start, START)),
            );
            let s = worst.iter().find(|s| s.id == "term-cjk").ok_or("no scenario")?;
            let img = image("term-cjk-font.terse", &s.image)?;
            let (mut p50s, mut maxes) = (Vec::new(), Vec::new());
            for _ in 0..runs {
                let (_, out) = timed(&mut bench(&img, &["--warmup", "0", "--frames", "30", "--threads", "1", "--config", &cfg]))?;
                let (p50, _, max) = bench_numbers(&out).ok_or("no --bench-render line")?;
                p50s.push(p50);
                maxes.push(max);
            }
            rows.push(
                row(
                    "H6",
                    "term-cjk with that font listed: 1,200 new CJK glyphs a frame, 1 thread, p50",
                    format!("max {}", spread(maxes, "ms")),
                    "",
                    Verdict::Info,
                )
                .with(Measure::new("p50", p50s, NEW_GLYPHS)),
            );
        }
    }
    let conform = programs.as_ref().map(|p| p.join("conform/conform.terse")).filter(|p| p.exists());
    match conform {
        None => rows.push(row(
            "H3",
            "programs/conform's stress scenes",
            "",
            "",
            Verdict::Skipped("programs/conform not found (--programs)".into()),
        )),
        Some(conform) => {
            let events = conform.with_extension("events");
            for threads in ["1", "0"] {
                // The stress phase: frames 49-108 (programs/conform/conform.c).
                let (mut p50s, mut maxes) = (Vec::new(), Vec::new());
                for _ in 0..runs {
                    let (_, out) = timed(
                        bench(&conform, &["--warmup", "48", "--frames", "61", "--threads", threads, "--events"]).arg(&events),
                    )?;
                    let (p50, _, max) = bench_numbers(&out).ok_or("no --bench-render line")?;
                    p50s.push(p50);
                    maxes.push(max);
                }
                rows.push(
                    row(
                        "H3",
                        format!(
                            "programs/conform's stress frames 49–108, software, {}: slowest frame",
                            if threads == "1" { "1 thread" } else { "all threads" }
                        ),
                        format!("p50 {}", spread(p50s, "ms")),
                        "",
                        Verdict::Info,
                    )
                    .with(Measure::new("max", maxes, WORST_FRAME)),
                );
            }
        }
    }
    // R10: each device's worst legal frame (terse_core::worst), drawn on one
    // thread; the verdict is whether the caps held, the time a target.
    for s in terse_core::worst::scenarios() {
        let img = image(&format!("{}.terse", s.id), &s.image)?;
        let enforced = terse_core::worst::Enforced::of(&s.image, 4)?;
        let mut p50s = Vec::new();
        let mut maxes = Vec::new();
        for _ in 0..runs {
            let (_, out) = timed(&mut bench(&img, &["--warmup", "3", "--frames", "30", "--threads", "1"]))?;
            let (p50, _, max) = bench_numbers(&out).ok_or("no --bench-render line")?;
            p50s.push(p50);
            maxes.push(max);
        }
        let capped = enforced.ink2.is_some() || enforced.ink3.is_some();
        rows.push(
            row(
                "W",
                format!("{}: {}", s.id, s.what),
                format!("max {:.1} ms; {enforced}", median(maxes)),
                if capped { "R10: within the caps" } else { "" },
                match (capped, enforced.within()) {
                    (false, _) => Verdict::Info,
                    (true, true) => Verdict::Pass,
                    (true, false) => Verdict::Fail,
                },
            )
            .with(Measure::new("1 thread p50", p50s, WORST_FRAME)),
        );
    }
    // R12: a shell and 8 machines, each at its worst, tiled, overlapping
    // and as panels, with the system ink (SPEC §machines.ink) and without.
    let all = image("all.terse", &terse_core::worst::combined_image())?;
    let all = all.to_string_lossy().into_owned();
    for s in terse_core::worst::system_scenarios(builtin::PROGRAMS.len() as u32) {
        let img = image(&format!("{}.terse", s.id), &s.shell)?;
        let p50s = |off: bool| -> Result<Vec<f64>, String> {
            let mut v = Vec::new();
            for _ in 0..runs {
                let mut c = bench(&img, &["--shell", "--builtin", &all, "--warmup", "12", "--frames", "30", "--threads", "1"]);
                if off {
                    c.env("TERSE_SYSTEM_INK", "off");
                }
                let (_, out) = timed(&mut c)?;
                v.push(bench_numbers(&out).ok_or("no --bench-render line")?.0);
            }
            Ok(v)
        };
        let (on, off) = (p50s(false)?, p50s(true)?);
        rows.push(
            row(
                "W",
                format!("{}: {}", s.id, s.what),
                format!("without the system ink {:.1} ms", median(off)),
                "R12: within the system ink by construction",
                Verdict::Info,
            )
            .with(Measure::new("1 thread p50", on, SYSTEM)),
        );
    }
    // The phosphor look at the caps: shed by ink (terse-host's default)
    // and in full (--no-shed); presentation, the host's choice.
    let mut full = Vec::new();
    let mut shed = Vec::new();
    for s in terse_core::worst::scenarios().into_iter().filter(|s| s.id.ends_with("-phosphor")) {
        let img = image(&format!("{}.terse", s.id), &s.image)?;
        let one = |extra: &[&str]| -> Result<Vec<f64>, String> {
            let mut v = Vec::new();
            for _ in 0..runs {
                let mut args = vec!["--warmup", "3", "--frames", "30", "--threads", "1"];
                args.extend_from_slice(extra);
                let (_, out) = timed(&mut bench(&img, &args))?;
                v.push(bench_numbers(&out).ok_or("no --bench-render line")?.0);
            }
            Ok(v)
        };
        let name = s.id.trim_end_matches("-phosphor");
        full.push(format!("{name} {:.1}", median(one(&["--no-shed"])?)));
        shed.push(Measure::new(name, one(&[])?, WORST_FRAME));
    }
    let mut r = row(
        "W",
        "phosphor look at the caps, shed by ink, 1 thread p50",
        format!("in full (--no-shed, presentation: no limit): {} ms", full.join(", ")),
        "",
        Verdict::Info,
    );
    r.measures = shed;
    rows.push(r);
    Ok(())
}

/// H7: the built-in shell's first frame as a new user's first start has it.
/// Each run is a first run: empty store, config and data directories and a
/// new HOME (so the host installs the built-in programs and the manual), and
/// terse-host's own pages dropped from the page cache just before it is
/// started, checked to be gone. The shared libraries it links (libc and the
/// like) are left: every desktop's processes keep them resident. Then the
/// same start again, warm, for comparison.
fn cold_start(rows: &mut Vec<Row>, runs: usize, host: &Path) -> Result<(), String> {
    let (mut cold, mut warm, mut resident) = (Vec::new(), Vec::new(), (0, 0));
    let mut dropped = true;
    for _ in 0..runs {
        let home = Tmp::new("cold")?;
        let start = || {
            let mut c = isolated(host, &home);
            c.args(["--bench-render", "--renderer", "software", "--warmup", "0", "--frames", "1", "--store"]).arg(home.path("store"));
            c.arg("--config").arg(home.path("cfg")).arg("--data").arg(home.path("dat"));
            c
        };
        // Pages of a process that has just exited can stay mapped for a
        // moment (they cannot be dropped then): try again for up to 2 s.
        let mut tries = 0;
        let dropping = loop {
            let r = page_cache::drop_pages(host);
            tries += 1;
            match r {
                Ok((left, _)) if left > 0 && tries < 40 => std::thread::sleep(Duration::from_millis(50)),
                r => break r,
            }
        };
        match dropping {
            Ok((left, pages)) => {
                resident = (resident.0.max(left), pages);
                // A file system that keeps them (tmpfs) gives no cold start.
                dropped &= left * 10 <= pages;
            }
            Err(_) => dropped = false,
        }
        cold.push(timed(&mut start())?.0.as_secs_f64() * 1000.0);
        warm.push(timed(&mut start())?.0.as_secs_f64() * 1000.0);
    }
    let (what, measured) = if dropped {
        (
            "cold start: the built-in shell's first frame on a first run (empty store, config, data, HOME; terse-host not in the page cache)",
            format!("at most {} of terse-host's {} pages resident at start; warm, the same: {}", resident.0, resident.1, spread(warm, "ms")),
        )
    } else {
        (
            "first run: the built-in shell's first frame (empty store, config, data, HOME); terse-host could not be dropped from the page cache here",
            format!("(not cold: {} of {} pages stayed resident); warm, the same: {}", resident.0, resident.1, spread(warm, "ms")),
        )
    };
    rows.push(row("H7", what, measured, "R13", Verdict::Info).with(Measure::new("", cold, COLD_START)));
    Ok(())
}

/// Dropping one file's pages from the page cache, without root and
/// without touching any other file's.
mod page_cache {
    use std::path::Path;

    /// Drops `path`'s cached pages (`posix_fadvise(POSIX_FADV_DONTNEED)`:
    /// clean pages not mapped by a process, as an executable's are when it
    /// is not running) and counts those still resident (`mincore`): the
    /// number left and the file's pages.
    #[cfg(target_os = "linux")]
    pub fn drop_pages(path: &Path) -> Result<(usize, usize), String> {
        use std::os::fd::AsRawFd;
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len() as usize;
        let fd = file.as_raw_fd();
        // SAFETY: plain system calls on a file this function holds open; the
        // map is read only, never dereferenced, and unmapped before return.
        unsafe {
            let e = libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED);
            if e != 0 {
                return Err(format!("posix_fadvise: error {e}"));
            }
            let page = libc::sysconf(libc::_SC_PAGESIZE).max(1) as usize;
            let pages = len.div_ceil(page);
            if pages == 0 {
                return Ok((0, 0));
            }
            let map = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd, 0);
            if map == libc::MAP_FAILED {
                return Err("mmap failed".into());
            }
            let mut v = vec![0u8; pages];
            let r = libc::mincore(map, len, v.as_mut_ptr());
            libc::munmap(map, len);
            if r != 0 {
                return Err("mincore failed".into());
            }
            Ok((v.iter().filter(|&&b| b & 1 != 0).count(), pages))
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn drop_pages(_: &Path) -> Result<(usize, usize), String> {
        Err("no way to drop one file's pages on this system".into())
    }
}

/// A sibling of this executable (`target/release/terse-host` beside
/// `target/release/terse`).
fn sibling(name: &str) -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.with_file_name(name);
    p.exists().then_some(p)
}

pub fn perf(args: &[String]) -> Result<ExitCode, String> {
    let (mut only, mut runs, mut programs, mut host, mut reader) = (None, 5usize, None, sibling("terse-host"), None);
    let mut strict = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a value\nusage: {USAGE}"));
        match a.as_str() {
            "--strict" => strict = true,
            "--only" => only = Some(value()?),
            "--runs" => runs = value()?.parse().ok().filter(|&n| n > 0).ok_or("bad --runs")?,
            "--programs" => programs = Some(PathBuf::from(value()?)),
            "--host" => host = Some(PathBuf::from(value()?)),
            "--reader" => reader = Some(PathBuf::from(value()?)),
            "--write-worst" => {
                let dir = PathBuf::from(value()?);
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                for s in terse_core::worst::scenarios() {
                    std::fs::write(dir.join(format!("{}.terse", s.id)), &s.image).map_err(|e| e.to_string())?;
                    println!("{}\t{}", s.id, s.what);
                }
                // The systems: run as terse-host --shell <id>.terse --builtin all.terse.
                for s in terse_core::worst::system_scenarios(builtin::PROGRAMS.len() as u32) {
                    std::fs::write(dir.join(format!("{}.terse", s.id)), &s.shell).map_err(|e| e.to_string())?;
                    println!("{}\t{} (--shell {}.terse --builtin all.terse)", s.id, s.what, s.id);
                }
                return Ok(ExitCode::SUCCESS);
            }
            _ => return Err(format!("usage: {USAGE}")),
        }
    }
    // The repository's programs/ and the Go reader, from the working
    // directory or from where this executable was built.
    let repo = |sub: &str| {
        let here = PathBuf::from(sub);
        if here.exists() {
            return Some(here);
        }
        let built = std::env::current_exe().ok()?.parent()?.parent()?.parent()?.join(sub);
        built.exists().then_some(built)
    };
    let programs = programs.or_else(|| repo("programs"));
    let reader = reader.or_else(|| repo("native/reader-go/tersereader"));
    let want = |layer: &str| only.as_deref().is_none_or(|o| layer.contains(o));

    let mut rows = Vec::new();
    println!(
        "terse perf: {} cores; load average {}; {runs} runs of each timed scenario, ceilings (C) and targets (T) judged by the median{}",
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        load(),
        if strict { "; --strict: a missed target fails" } else { "" }
    );
    if want("machine") {
        machine_rows(&mut rows, runs)?;
    }
    if want("client") {
        client_rows(&mut rows, runs, reader)?;
    }
    if want("host") {
        host_rows(&mut rows, runs, host, programs)?;
    }
    let mut tally = Tally::default();
    let width = rows.iter().map(|r| r.what.chars().count()).max().unwrap_or(0).min(72);
    println!("{:<4} {:<width$}  {:<44}  {:<38}  result", "id", "scenario", "measured: median (range)", "requirement (R), ceiling (C), target (T)");
    for r in &rows {
        let result = tally.judge(r, strict);
        let measured =
            r.measures.iter().map(Measure::show).chain((!r.measured.is_empty()).then(|| r.measured.clone())).collect::<Vec<_>>().join("; ");
        let mut limits: Vec<Limit> = r.measures.iter().map(|m| m.limit).collect();
        limits.dedup();
        let rule = (!r.rule.is_empty()).then(|| r.rule.clone()).into_iter().chain(limits.iter().map(Limit::rule)).collect::<Vec<_>>().join("; ");
        println!("{:<4} {:<width$}  {:<44}  {:<38}  {result}", r.id, r.what, measured, rule);
    }
    println!("{}", tally.summary(strict));
    Ok(if tally.failures() == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

/// What was judged, and what failed.
#[derive(Default)]
struct Tally {
    requirements: usize,
    requirements_failed: usize,
    ceilings: usize,
    ceilings_failed: usize,
    targets: usize,
    targets_missed: usize,
    /// Whether a missed target counts as a failure (`--strict`).
    strict: bool,
}

impl Tally {
    /// Counts `r`'s requirement, ceilings and targets; its result column.
    fn judge(&mut self, r: &Row, strict: bool) -> String {
        self.strict = strict;
        let mut failed = Vec::new();
        match &r.verdict {
            Verdict::Pass => self.requirements += 1,
            Verdict::Fail => {
                self.requirements += 1;
                self.requirements_failed += 1;
                failed.push("requirement");
            }
            Verdict::Skipped(why) => return format!("skipped: {why}"),
            Verdict::Info => {}
        }
        let (mut over, mut missed) = (false, false);
        for m in &r.measures {
            let v = m.median();
            self.ceilings += 1;
            self.targets += 1;
            if v > m.limit.ceiling {
                self.ceilings_failed += 1;
                over = true;
            }
            if v > m.limit.target {
                self.targets_missed += 1;
                missed = true;
            }
        }
        if over {
            failed.push("ceiling");
        }
        if missed && strict {
            failed.push("target");
        }
        if !failed.is_empty() {
            format!("FAIL ({})", failed.join(", "))
        } else if matches!(r.verdict, Verdict::Pass) || !r.measures.is_empty() {
            if missed { "PASS; target missed".into() } else { "PASS".into() }
        } else {
            "info".into()
        }
    }

    fn failures(&self) -> usize {
        self.requirements_failed + self.ceilings_failed + if self.strict { self.targets_missed } else { 0 }
    }

    fn summary(&self, strict: bool) -> String {
        format!(
            "{} requirements, {} ceilings, {} targets: {} failures ({} requirements, {} ceilings{}); {} targets missed{}. Targets (T) are the reference machine's, PERFORMANCE.md.",
            self.requirements,
            self.ceilings,
            self.targets,
            self.failures(),
            self.requirements_failed,
            self.ceilings_failed,
            if strict { format!(", {} targets", self.targets_missed) } else { String::new() },
            self.targets_missed,
            if strict { "" } else { " (information; --strict makes them failures)" },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The median is judged, not the worst run: one slow run on a busy
    /// machine does not fail a ceiling, a slow majority does.
    #[test]
    fn ceilings_judge_the_median() {
        let judge = |samples: &[f64], strict: bool| {
            let mut t = Tally::default();
            let r = row("H1", "", "", "", Verdict::Info).with(Measure::new("", samples.to_vec(), START));
            (t.judge(&r, strict), t.failures())
        };
        assert_eq!(judge(&[20.0, 21.0, 19.0, 22.0, 2000.0], false), ("PASS".into(), 0));
        assert_eq!(judge(&[2000.0, 1600.0, 1700.0, 20.0, 20.0], false), ("FAIL (ceiling)".into(), 1));
        assert_eq!(judge(&[60.0, 70.0, 80.0], false), ("PASS; target missed".into(), 0));
        assert_eq!(judge(&[60.0, 70.0, 80.0], true), ("FAIL (target)".into(), 1));
        let mut t = Tally::default();
        assert_eq!(t.judge(&row("R", "", "", "", Verdict::Fail).with(Measure::new("", vec![1.0], START)), true), "FAIL (requirement)");
        assert_eq!(t.failures(), 1);
    }

    /// Every ceiling is a generous multiple of its target: at least 3 times
    /// (PERFORMANCE.md §1, the hardware floor).
    #[test]
    fn ceilings_are_far_above_targets() {
        for l in [HELD_PROCESS, SYNC, SHOWN, START, COLD_START, GPU_START, WORST_FRAME, NEW_GLYPHS, SYSTEM] {
            assert!(l.ceiling >= 3.0 * l.target, "{}", l.rule());
        }
        assert_eq!(START.ceiling, 1500.0, "the owner's start-up ceiling");
        assert_eq!(WORST_FRAME.rule(), "C ≤ 4 frames (66.7 ms), T ≤ 1 frame (16.7 ms)");
    }
}

fn load() -> String {
    std::fs::read_to_string("/proc/loadavg").map_or("unknown".into(), |s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
}
