//! `terse perf`: the responsiveness scenarios of PERFORMANCE.md, run on
//! this machine against the reference implementation.
//!
//! Every scenario runs headless, in temporary directories, with relays
//! started here on ephemeral loopback ports: nothing reads or writes the
//! user's config, cache or data directories, and nothing uses port 7878.
//! Requirements are in frames, round trips or "no network access", and
//! print PASS or FAIL; targets are in milliseconds on a named machine and
//! print as information. A run exits with 1 if any requirement failed.

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

pub const USAGE: &str = "terse perf [--only <text>] [--runs <n>] [--programs <dir>] [--host <terse-host>] [--reader <tersereader>]\n       terse perf --write-worst <dir>   (the W rows' images, PERFORMANCE.md R10)";

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
        rows.push(Row {
            id,
            what: format!("{label}: submit to content-done (fetch)"),
            measured: format!("{measured}; service {}; relay asked {asked} times; {status}", ms(latency)),
            rule: format!("R6: ≤ {HELD_FRAMES} frames; R1: no relay asked"),
            verdict,
        });
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
    rows.push(Row {
        id: "M2",
        what: "get not held, on a connected local relay".into(),
        measured: format!("{trips} Get; service {}; fetch {}", spread(lat, "ms"), spread(frames, "frames")),
        rule: "R2: 1 round trip".into(),
        verdict: if trips == 1 { Verdict::Pass } else { Verdict::Fail },
    });

    // M3: not held and no relay: not found at once.
    let tmp = Tmp::new("m3")?;
    let store = tmp.path("store");
    BlobStore::open(&store, None).map_err(|e| e.to_string())?;
    let (at, status) = fetch_frames(service(&store, &Net::None, None)?)?;
    let (measured, verdict) = frames_rule(at, HELD_FRAMES);
    rows.push(Row {
        id: "M3",
        what: "get not held, no relay: not found".into(),
        measured: format!("{measured}; {status}"),
        rule: format!("R6: ≤ {HELD_FRAMES} frames"),
        verdict,
    });

    // M4: the timeline from the store while the only relay never answers.
    let tmp = Tmp::new("m4")?;
    let store = held_store(&tmp)?;
    let key = Identity::from_seed([7; 32]).feed_key();
    let mut svc = service(&store, &Net::Silent(&silent), Some(Subscriptions::in_memory([key])))?;
    let (latency, outcome) = service_latency(&mut svc, RequestKind::Timeline { offset: 0, length: 4096 });
    let ok = matches!(outcome, Some(Outcome::Records { .. })) && latency < FRAME * HELD_FRAMES;
    rows.push(Row {
        id: "M4",
        what: "timeline request, subscribed, relay never answers".into(),
        measured: ms(latency),
        rule: format!("R3, R6: ≤ {HELD_FRAMES} frames"),
        verdict: if ok { Verdict::Pass } else { Verdict::Fail },
    });

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
    rows.push(Row {
        id: "M10",
        what: "programs/conform-perf in the real-time loop".into(),
        measured: lines.iter().map(|l| short(l)).collect::<Vec<_>>().join("; "),
        rule: "R6, R8: every line PASS".into(),
        verdict: if !lines.is_empty() && lines.iter().all(|l| l.contains("PASS")) { Verdict::Pass } else { Verdict::Fail },
    });

    shell_rows(rows, runs)?;
    reader_rows(rows)?;
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
    rows.push(Row {
        id: "M5",
        what: "shell start to the launcher on screen".into(),
        measured: spread(launcher, "frames"),
        rule: "the shell's own frames".into(),
        verdict: Verdict::Info,
    });
    let worst = start.iter().copied().fold(0.0, f64::max);
    rows.push(Row {
        id: "M6",
        what: "program start (held image): Enter on fetch to fetch's first frame".into(),
        measured: spread(start, "frames"),
        rule: format!("R9: ≤ {HELD_FRAMES} frames"),
        verdict: if worst <= HELD_FRAMES as f64 { Verdict::Pass } else { Verdict::Fail },
    });
    rows.push(Row {
        id: "M7",
        what: "Enter on fetch to its message on screen (emulation only)".into(),
        measured: format!("{}; {}", spread(text, "frames"), spread(wall, "ms")),
        rule: "T: ≤ 6 frames".into(),
        verdict: Verdict::Info,
    });
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
fn reader_rows(rows: &mut Vec<Row>) -> Result<(), String> {
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
    rows.push(Row {
        id: "M8",
        what: "reader: M to the manual's index on the page (held)".into(),
        measured: opened.map_or("never".into(), |f| format!("{} frames", f - 10)),
        rule: "T: ≤ 6 frames".into(),
        verdict: Verdict::Info,
    });
    rows.push(Row {
        id: "M9",
        what: "reader: Enter on a link to the article on the page (held)".into(),
        measured: followed.map_or("never".into(), |f| format!("{} frames", f.saturating_sub(6))),
        rule: "T: ≤ 6 frames".into(),
        verdict: Verdict::Info,
    });
    Ok(())
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

fn client_rows(rows: &mut Vec<Row>, runs: usize, reader: Option<PathBuf>) -> Result<(), String> {
    let terse = std::env::current_exe().map_err(|e| e.to_string())?;
    let relay = LocalRelay::start()?;
    let silent = SilentRelay::start()?;
    let (key, article) = publish_feed(&relay)?;
    let key = to_hex(&key);

    // The reference command line as a reading client.
    let home = Tmp::new("cli")?;
    let (store, config) = (home.path("store"), home.path("cfg"));
    let net = |c: &mut Command, url: &str| {
        c.args(["--relay", url, "--store"]).arg(&store).arg("--config").arg(&config).arg("--no-feed-relays");
    };
    let mut c = isolated(&terse, &home);
    c.args(["feed", "subscribe", &key]);
    net(&mut c, &relay.url);
    timed(&mut c)?;
    let (subs, gets) = (relay.relay.received(SUBSCRIBE), relay.gets());
    let mut c = isolated(&terse, &home);
    c.args(["feed", "sync"]);
    net(&mut c, &relay.url);
    let (took, _) = timed(&mut c)?;
    rows.push(Row {
        id: "C1",
        what: format!("terse feed sync: {SYNC_ENTRIES} new entries and articles from a local relay"),
        measured: format!("{}; {} Subscribe, {} Get", ms(took), relay.relay.received(SUBSCRIBE) - subs, relay.gets() - gets),
        rule: "R4: 1 Subscribe per feed; T: ≤ 500 ms".into(),
        verdict: if relay.relay.received(SUBSCRIBE) - subs == 1 { Verdict::Pass } else { Verdict::Fail },
    });
    let mut held = Vec::new();
    let gets = relay.gets();
    for _ in 0..runs {
        let mut c = isolated(&terse, &home);
        c.args(["get", &to_hex(&article), "-o"]).arg(home.path("out"));
        net(&mut c, &relay.url);
        held.push(timed(&mut c)?.0.as_secs_f64() * 1000.0);
    }
    let asked = relay.gets() - gets;
    rows.push(Row {
        id: "C2",
        what: "terse get of a held article (process start to exit), relay connected".into(),
        measured: format!("{}; relay asked {asked} times", spread(held, "ms")),
        rule: "R1: no relay asked; T: ≤ 50 ms".into(),
        verdict: if asked == 0 { Verdict::Pass } else { Verdict::Fail },
    });
    let mut c = isolated(&terse, &home);
    c.args(["get", &to_hex(&article), "-o"]).arg(home.path("out"));
    net(&mut c, &silent.url);
    let (took, _) = timed(&mut c)?;
    rows.push(Row {
        id: "C3",
        what: "terse get of a held article, relay never answers".into(),
        measured: ms(took),
        rule: "R1: no wait for the relay (< 1 s; it times out at 10 s)".into(),
        verdict: if took < Duration::from_secs(1) { Verdict::Pass } else { Verdict::Fail },
    });
    let mut c = isolated(&terse, &home);
    c.args(["feed", "list"]);
    net(&mut c, &silent.url);
    let (took, out) = timed(&mut c)?;
    rows.push(Row {
        id: "C4",
        what: "terse feed list (the timeline from the store), relay never answers".into(),
        measured: format!("{}; {} entries", ms(took), out.lines().filter(|l| l.contains("Article")).count()),
        rule: "R3: no wait for the relay (< 1 s)".into(),
        verdict: if took < Duration::from_secs(1) { Verdict::Pass } else { Verdict::Fail },
    });
    let mut not_held = Vec::new();
    let mut trips = 0;
    for run in 0..runs {
        let blob = format!("C5 {run} {}", std::process::id()).into_bytes();
        relay.relay.blobs().put(&blob).map_err(|e| e.to_string())?;
        let before = relay.gets();
        let mut c = isolated(&terse, &home);
        c.args(["get", &to_hex(&blob_id(&blob)), "-o"]).arg(home.path("out"));
        net(&mut c, &relay.url);
        not_held.push(timed(&mut c)?.0.as_secs_f64() * 1000.0);
        trips = relay.gets() - before;
    }
    rows.push(Row {
        id: "C5",
        what: "terse get of an article not held, from a local relay".into(),
        measured: format!("{}; {trips} Get", spread(not_held, "ms")),
        rule: "R2: 1 round trip".into(),
        verdict: if trips == 1 { Verdict::Pass } else { Verdict::Fail },
    });

    // The Go reader, an independent implementation, if it is built.
    let Some(reader) = reader else {
        rows.push(Row {
            id: "G",
            what: "Go reader (native/reader-go/tersereader)".into(),
            measured: String::new(),
            rule: String::new(),
            verdict: Verdict::Skipped("not built (cd native/reader-go && go build -o tersereader .)".into()),
        });
        return Ok(());
    };
    let home = Tmp::new("go")?;
    let dir = home.path("reader");
    let go = |args: &[&str], url: &str| {
        let mut c = isolated(&reader, &home);
        c.arg("--dir").arg(&dir).args(["--node", "in-process", "--relay", url, "--no-feed-relays"]).args(args);
        c
    };
    timed(&mut go(&["subscribe", &key], &relay.url))?;
    let (subs, gets) = (relay.relay.received(SUBSCRIBE), relay.gets());
    let (took, _) = timed(&mut go(&["sync", "--timeout", "20s"], &relay.url))?;
    rows.push(Row {
        id: "G1",
        what: format!("tersereader sync: {SYNC_ENTRIES} new entries and articles from a local relay"),
        measured: format!("{}; {} Subscribe, {} Get", ms(took), relay.relay.received(SUBSCRIBE) - subs, relay.gets() - gets),
        rule: "R4: 1 Subscribe per feed; T: ≤ 500 ms".into(),
        verdict: if relay.relay.received(SUBSCRIBE) - subs == 1 { Verdict::Pass } else { Verdict::Fail },
    });
    let mut dump = Vec::new();
    let gets = relay.gets();
    for _ in 0..runs {
        dump.push(timed(&mut go(&["--dump", &to_hex(&article), "--plain", "--width", "80"], &relay.url))?.0.as_secs_f64() * 1000.0);
    }
    let asked = relay.gets() - gets;
    rows.push(Row {
        id: "G2",
        what: "tersereader --dump of a held article, relay connected".into(),
        measured: format!("{}; relay asked {asked} times", spread(dump, "ms")),
        rule: "R1: no relay asked; T: ≤ 50 ms".into(),
        verdict: if asked == 0 { Verdict::Pass } else { Verdict::Fail },
    });
    let (took, _) = timed(&mut go(&["--dump", &to_hex(&article), "--plain"], &silent.url))?;
    rows.push(Row {
        id: "G3",
        what: "tersereader --dump of a held article, relay never answers".into(),
        measured: ms(took),
        rule: "R1: no wait for the relay (< 1 s)".into(),
        verdict: if took < Duration::from_secs(1) { Verdict::Pass } else { Verdict::Fail },
    });
    let (took, out) = timed(&mut go(&["list"], &silent.url))?;
    rows.push(Row {
        id: "G4",
        what: "tersereader list (the timeline from the store), relay never answers".into(),
        measured: format!("{}; {} entries", ms(took), out.lines().filter(|l| l.contains("Article")).count()),
        rule: "R3: no wait for the relay (< 1 s)".into(),
        verdict: if took < Duration::from_secs(1) { Verdict::Pass } else { Verdict::Fail },
    });

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
    node.arg("--dir").arg(&dir).args(["--relay", &silent.url, "--no-feed-relays", "--socket"]).arg(&socket).arg("node");
    let mut child = node.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let via_node = |args: &[&str]| {
        let mut c = isolated(&reader, &home);
        c.args(["--node", "socket", "--socket"]).arg(&socket).args(args);
        timed(&mut c)
    };
    let result = (|| {
        let mut dump = Vec::new();
        for _ in 0..runs {
            dump.push(via_node(&["--dump", &to_hex(&article), "--plain", "--width", "80"])?.0.as_secs_f64() * 1000.0);
        }
        let (list, out) = via_node(&["list"])?;
        Ok::<_, String>((dump, list, out))
    })();
    let _ = child.kill();
    let _ = child.wait();
    let (dump, list, out) = result?;
    let worst = dump.iter().copied().fold(list.as_secs_f64() * 1000.0, f64::max);
    rows.push(Row {
        id: "G5",
        what: "tersereader via its node on the local socket: --dump held, list; relay never answers".into(),
        measured: format!("--dump {}; list {} ({} entries)", spread(dump, "ms"), ms(list), out.lines().filter(|l| l.contains("Article")).count()),
        rule: "R5: no wait for the relay (< 1 s)".into(),
        verdict: if worst < 1000.0 { Verdict::Pass } else { Verdict::Fail },
    });
    Ok(())
}

/// `--bench-render`'s line: "... p50 X ms, p99 Y ms, max Z ms, ...".
fn bench_numbers(out: &str) -> Option<(f64, f64, f64)> {
    let line = out.lines().find(|l| l.contains(" p50 "))?;
    let num = |key: &str| -> Option<f64> { line.split(key).nth(1)?.split_whitespace().next()?.parse().ok() };
    Some((num(" p50 ")?, num(" p99 ")?, num(" max ")?))
}

fn host_rows(rows: &mut Vec<Row>, runs: usize, host: Option<PathBuf>, programs: Option<PathBuf>) -> Result<(), String> {
    let Some(host) = host else {
        rows.push(Row {
            id: "H",
            what: "terse-host (rendering)".into(),
            measured: String::new(),
            rule: String::new(),
            verdict: Verdict::Skipped("terse-host not found beside terse (--host)".into()),
        });
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
    let mut start = Vec::new();
    let mut first_cjk = Vec::new();
    for _ in 0..runs {
        let (took, _) = timed(&mut bench(&hello, &["--warmup", "0", "--frames", "1"]))?;
        start.push(took.as_secs_f64() * 1000.0);
        let (_, out) = timed(&mut bench(&fetch, &["--warmup", "0", "--frames", "8"]))?;
        first_cjk.push(bench_numbers(&out).ok_or("no --bench-render line")?.2);
    }
    rows.push(Row {
        id: "H1",
        what: "terse-host start to hello's first frame drawn (software, offscreen)".into(),
        measured: spread(start, "ms"),
        rule: "T: ≤ 500 ms".into(),
        verdict: Verdict::Info,
    });
    rows.push(Row {
        id: "H2",
        what: "fetch: slowest frame drawn (its first CJK and Hangul text, 50 ms after start)".into(),
        measured: spread(first_cjk, "ms"),
        rule: "T: ≤ 150 ms (waits for the font map, built in the first 0.1–0.2 s)".into(),
        verdict: Verdict::Info,
    });
    let conform = programs.as_ref().map(|p| p.join("conform/conform.terse")).filter(|p| p.exists());
    match conform {
        None => rows.push(Row {
            id: "H3",
            what: "programs/conform's stress scenes".into(),
            measured: String::new(),
            rule: String::new(),
            verdict: Verdict::Skipped("programs/conform not found (--programs)".into()),
        }),
        Some(conform) => {
            let events = conform.with_extension("events");
            for threads in ["1", "0"] {
                // The stress phase: frames 49-108 (programs/conform/conform.c).
                let (_, out) = timed(
                    bench(&conform, &["--warmup", "48", "--frames", "61", "--threads", threads, "--events"]).arg(&events),
                )?;
                let (p50, p99, max) = bench_numbers(&out).ok_or("no --bench-render line")?;
                rows.push(Row {
                    id: "H3",
                    what: format!(
                        "programs/conform's stress frames 49–108, software, {}",
                        if threads == "1" { "1 thread" } else { "all threads" }
                    ),
                    measured: format!("p50 {p50:.2} ms, p99 {p99:.2} ms, max {max:.2} ms"),
                    rule: "T: ≤ 16.7 ms (a worst legal case)".into(),
                    verdict: Verdict::Info,
                });
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
        let median = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let (p50, max) = (median(p50s), median(maxes));
        let capped = enforced.ink2.is_some() || enforced.ink3.is_some();
        rows.push(Row {
            id: "W",
            what: format!("{}: {}", s.id, s.what),
            measured: format!("1 thread p50 {p50:.1} ms, max {max:.1} ms; {enforced}"),
            rule: if capped { "R10: within the caps; T: ≤ 16.7 ms".into() } else { "T: ≤ 16.7 ms".into() },
            verdict: match (capped, enforced.within()) {
                (false, _) => Verdict::Info,
                (true, true) => Verdict::Pass,
                (true, false) => Verdict::Fail,
            },
        });
    }
    // R11: a shell and 8 machines, each at its worst, tiled, overlapping
    // and as panels, with the system ink (SPEC §machines.ink) and without.
    let all = image("all.terse", &terse_core::worst::combined_image())?;
    let all = all.to_string_lossy().into_owned();
    for s in terse_core::worst::system_scenarios(builtin::PROGRAMS.len() as u32) {
        let img = image(&format!("{}.terse", s.id), &s.shell)?;
        let p50 = |off: bool| -> Result<f64, String> {
            let mut v = Vec::new();
            for _ in 0..runs {
                let mut c = bench(&img, &["--shell", "--builtin", &all, "--warmup", "12", "--frames", "30", "--threads", "1"]);
                if off {
                    c.env("TERSE_SYSTEM_INK", "off");
                }
                let (_, out) = timed(&mut c)?;
                v.push(bench_numbers(&out).ok_or("no --bench-render line")?.0);
            }
            v.sort_by(f64::total_cmp);
            Ok(v[v.len() / 2])
        };
        let (on, off) = (p50(false)?, p50(true)?);
        rows.push(Row {
            id: "W",
            what: format!("{}: {}", s.id, s.what),
            measured: format!("1 thread p50 {on:.1} ms (without the system ink {off:.1} ms)"),
            rule: "R11: within the system ink by construction; T: ≤ 2 × the single machine's".into(),
            verdict: Verdict::Info,
        });
    }
    // The phosphor look at the caps: shed by ink (terse-host's default)
    // and in full (--no-shed); presentation, the host's choice.
    let mut parts = Vec::new();
    for s in terse_core::worst::scenarios().into_iter().filter(|s| s.id.ends_with("-phosphor")) {
        let img = image(&format!("{}.terse", s.id), &s.image)?;
        let one = |extra: &[&str]| -> Result<f64, String> {
            let mut v = Vec::new();
            for _ in 0..runs {
                let mut args = vec!["--warmup", "3", "--frames", "30", "--threads", "1"];
                args.extend_from_slice(extra);
                let (_, out) = timed(&mut bench(&img, &args))?;
                v.push(bench_numbers(&out).ok_or("no --bench-render line")?.0);
            }
            v.sort_by(f64::total_cmp);
            Ok(v[v.len() / 2])
        };
        parts.push(format!("{} {:.1} ({:.1})", s.id.trim_end_matches("-phosphor"), one(&[])?, one(&["--no-shed"])?));
    }
    rows.push(Row {
        id: "W",
        what: "phosphor look at the caps, shed by ink (in full, --no-shed), 1 thread p50 ms".into(),
        measured: parts.join(", "),
        rule: "T: ≤ 16.7 ms shed".into(),
        verdict: Verdict::Info,
    });
    Ok(())
}

/// A sibling of this executable (`target/release/terse-host` beside
/// `target/release/terse`).
fn sibling(name: &str) -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.with_file_name(name);
    p.exists().then_some(p)
}

pub fn perf(args: &[String]) -> Result<ExitCode, String> {
    let (mut only, mut runs, mut programs, mut host, mut reader) = (None, 5usize, None, sibling("terse-host"), None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a value\nusage: {USAGE}"));
        match a.as_str() {
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
    println!("terse perf: {} cores; load average {}", std::thread::available_parallelism().map_or(0, |n| n.get()), load());
    if want("machine") {
        machine_rows(&mut rows, runs)?;
    }
    if want("client") {
        client_rows(&mut rows, runs, reader)?;
    }
    if want("host") {
        host_rows(&mut rows, runs.min(3), host, programs)?;
    }
    let mut failed = 0;
    let width = rows.iter().map(|r| r.what.chars().count()).max().unwrap_or(0).min(72);
    println!("{:<4} {:<width$}  {:<44}  {:<38}  result", "id", "scenario", "measured", "requirement (R) or target (T)");
    for r in &rows {
        let verdict = match &r.verdict {
            Verdict::Pass => "PASS".to_string(),
            Verdict::Fail => {
                failed += 1;
                "FAIL".to_string()
            }
            Verdict::Info => "info".to_string(),
            Verdict::Skipped(why) => format!("skipped: {why}"),
        };
        println!("{:<4} {:<width$}  {:<44}  {:<38}  {verdict}", r.id, r.what, r.measured, r.rule);
    }
    println!("{} requirements, {failed} failed. Targets (T) are for the reference machine of PERFORMANCE.md.", rows.iter().filter(|r| matches!(r.verdict, Verdict::Pass | Verdict::Fail)).count());
    Ok(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

fn load() -> String {
    std::fs::read_to_string("/proc/loadavg").map_or("unknown".into(), |s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
}
