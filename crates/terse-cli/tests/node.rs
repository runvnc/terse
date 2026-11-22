//! `terse node` (LOCAL.md) from the outside: a small client written from
//! LOCAL.md alone, against the node as a process on a socket in a
//! temporary directory, with relays on ephemeral ports.
//!
//! - `vector_session`: vectors/local-session.txt, replayed;
//! - `every_message_and_both_pushes`: every request against a node that
//!   follows a feed on a relay, Changed and Log, and the connections it
//!   closes;
//! - `held_state_answers_while_the_network_waits`: LOCAL.md 4.5 with a relay
//!   that never answers;
//! - `one_node_per_socket`: a live node is not replaced, a stale socket is,
//!   the socket goes when the node stops, the directory's rules;
//! - with Go (skipped, saying so, when `go` is absent):
//!   `go_thin_reader_through_terse_node` (the Go reader's thin build as
//!   the client) and `terse_node_and_go_node_answer_alike` (this client
//!   against `terse node` and `tersereader node`, LOCAL.md's rules
//!   included: Hello, refusals, hints, titles, Set relays at once).
#![cfg(unix)]

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_net::relay::RelayClient;
use terse_net::{BlobStore, Draft, FeedKey, FeedStore, Identity, Node, Subscriptions, blob_id, to_hex};

const TIMEOUT: Duration = Duration::from_secs(20);

// --- The client -------------------------------------------------------------------

/// A connection to a node: whole messages, pushes kept aside.
struct Client {
    stream: UnixStream,
    pushes: Vec<Vec<u8>>,
}

fn msg(t: u8, req: u32, parts: &[&[u8]]) -> Vec<u8> {
    let mut m = vec![t];
    m.extend_from_slice(&req.to_le_bytes());
    for p in parts {
        m.extend_from_slice(p);
    }
    m
}

fn short(b: &[u8]) -> Vec<u8> {
    assert!(b.len() < 256);
    [&[b.len() as u8][..], b].concat()
}

impl Client {
    fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        Client { stream, pushes: Vec::new() }
    }

    /// Connects and says Hello; returns the client and the Hello flags.
    fn hello(path: &Path) -> (Self, u8) {
        let mut c = Self::connect(path);
        let r = c.call(&msg(0x20, 1, &[&[0], &short(b"")]));
        assert_eq!(&r[..7], &[0xa0, 1, 0, 0, 0, 1, 0], "{r:x?}");
        (c, r[7])
    }

    fn send(&mut self, m: &[u8]) {
        let mut framed = (m.len() as u32).to_le_bytes().to_vec();
        framed.extend_from_slice(m);
        self.stream.write_all(&framed).unwrap();
    }

    /// The next message, push or reply; `None` if the node closed the
    /// connection.
    fn next(&mut self) -> Option<Vec<u8>> {
        let mut len = [0u8; 4];
        self.stream.read_exact(&mut len).ok()?;
        let len = u32::from_le_bytes(len) as usize;
        assert!(len <= 2_097_152);
        let mut m = vec![0; len];
        self.stream.read_exact(&mut m).unwrap();
        Some(m)
    }

    /// The next reply, pushes kept for [`Self::take_pushes`].
    fn reply(&mut self) -> Vec<u8> {
        loop {
            let m = self.next().expect("the node closed the connection");
            assert!(!m.is_empty());
            if m[0] >= 0xc0 {
                check_push(&m);
                self.pushes.push(m);
            } else {
                return m;
            }
        }
    }

    fn call(&mut self, m: &[u8]) -> Vec<u8> {
        self.send(m);
        let r = self.reply();
        assert_eq!((r[0], &r[1..5]), (m[0] | 0x80, &m[1..5]), "a reply to {:x?}: {r:x?}", &m[..5]);
        r
    }

    /// A call's status and the rest.
    fn ask(&mut self, m: &[u8]) -> (u8, Vec<u8>) {
        let r = self.call(m);
        (r[5], r[6..].to_vec())
    }

    /// Waits for a pushed message of type `t` (at most `TIMEOUT`).
    fn wait_push(&mut self, t: u8) -> Vec<u8> {
        if let Some(i) = self.pushes.iter().position(|p| p[0] == t) {
            return self.pushes.remove(i);
        }
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            let m = self.next().expect("closed while waiting for a push");
            assert!(m[0] >= 0xc0, "a reply nobody asked for: {m:x?}");
            check_push(&m);
            if m[0] == t {
                return m;
            }
        }
        panic!("no push {t:x}");
    }

    fn take_pushes(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.pushes)
    }

    /// Whether the node closes the connection (within `TIMEOUT`), pushes
    /// aside.
    fn closed(&mut self) -> bool {
        loop {
            match self.next() {
                None => return true,
                Some(m) if m[0] >= 0xc0 => {}
                Some(m) => panic!("a reply where the connection should close: {m:x?}"),
            }
        }
    }
}

/// A push is Changed (`C0h` alone) or Log (`C1h`, `n2`, text).
fn check_push(m: &[u8]) {
    match m[0] {
        0xc0 => assert_eq!(m.len(), 1),
        0xc1 => {
            let n = u16::from_le_bytes([m[1], m[2]]) as usize;
            assert_eq!(m.len(), 3 + n);
            std::str::from_utf8(&m[3..]).unwrap();
        }
        t => panic!("unknown push {t:x}"),
    }
}

/// Reads a reply's fields.
struct R<'a>(&'a [u8]);

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> &'a [u8] {
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        a
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take(2).try_into().unwrap())
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn text(&mut self) -> String {
        let n = self.u8() as usize;
        String::from_utf8(self.take(n).to_vec()).unwrap()
    }
    fn urls(&mut self) -> Vec<String> {
        (0..self.u8()).map(|_| self.text()).collect()
    }
}

/// A record (LOCAL.md 4.3).
#[derive(Debug, Clone, PartialEq)]
struct Rec {
    arrival: u32,
    flags: u8,
    name: String,
    entry: Vec<u8>,
}

impl Rec {
    fn feed(&self) -> &[u8] {
        &self.entry[..32]
    }
    fn seq(&self) -> u32 {
        u32::from_le_bytes(self.entry[32..36].try_into().unwrap())
    }
    fn kind(&self) -> u8 {
        self.entry[40]
    }
    fn blob(&self) -> [u8; 32] {
        self.entry[44..76].try_into().unwrap()
    }
    fn size(&self) -> u32 {
        u32::from_le_bytes([self.entry[41], self.entry[42], self.entry[43], 0])
    }
    fn title(&self) -> String {
        let n = self.entry[108] as usize;
        String::from_utf8(self.entry[109..109 + n].to_vec()).unwrap()
    }
}

fn records(r: &mut R, count: usize) -> Vec<Rec> {
    (0..count)
        .map(|_| {
            let arrival = r.u32();
            let flags = r.u8();
            let name = r.text();
            let m = r.u16() as usize;
            Rec { arrival, flags, name, entry: r.take(m).to_vec() }
        })
        .collect()
}

/// A feed record (LOCAL.md 4.3).
#[derive(Debug, Clone, PartialEq)]
struct FeedRec {
    key: Vec<u8>,
    flags: u8,
    name: String,
    hints: String,
    hint_urls: Vec<String>,
    hinted: Vec<String>,
    profile_relays: Vec<String>,
    description: String,
}

fn feed_record(r: &mut R) -> FeedRec {
    let key = r.take(32).to_vec();
    let flags = r.u8();
    let name = r.text();
    let hints = r.text();
    let (hint_urls, hinted, profile_relays) = (r.urls(), r.urls(), r.urls());
    let n = r.u16() as usize;
    FeedRec { key, flags, name, hints, hint_urls, hinted, profile_relays, description: String::from_utf8(r.take(n).to_vec()).unwrap() }
}

fn timeline(c: &mut Client, req: u32, before: u32, max: u16) -> Vec<Rec> {
    let (st, rest) = c.ask(&msg(0x22, req, &[&before.to_le_bytes(), &max.to_le_bytes()]));
    assert_eq!(st, 1);
    let mut r = R(&rest);
    let n = r.u16() as usize;
    let out = records(&mut r, n);
    assert!(r.0.is_empty());
    out
}

fn subscriptions(c: &mut Client, req: u32) -> Vec<FeedRec> {
    let (st, rest) = c.ask(&msg(0x23, req, &[]));
    assert_eq!(st, 1);
    let mut r = R(&rest);
    let n = r.u16();
    let out = (0..n).map(|_| feed_record(&mut r)).collect();
    assert!(r.0.is_empty());
    out
}

fn feed(c: &mut Client, req: u32, address: &str) -> (FeedRec, Vec<Rec>) {
    let (st, rest) = c.ask(&msg(0x24, req, &[&[1], &short(address.as_bytes())]));
    assert_eq!(st, 1, "{address}");
    let mut r = R(&rest);
    let f = feed_record(&mut r);
    let n = r.u16() as usize;
    let recs = records(&mut r, n);
    assert!(r.0.is_empty());
    (f, recs)
}

fn relays(c: &mut Client, req: u32) -> Vec<(u8, String)> {
    let (st, rest) = c.ask(&msg(0x28, req, &[]));
    assert_eq!(st, 1);
    let mut r = R(&rest);
    let n = r.u8();
    let out = (0..n).map(|_| (r.u8(), r.text())).collect();
    assert!(r.0.is_empty());
    out
}

fn set_relays(c: &mut Client, req: u32, urls: &[&str]) -> u8 {
    let mut parts = vec![urls.len() as u8];
    for u in urls {
        parts.extend(short(u.as_bytes()));
    }
    c.ask(&msg(0x29, req, &[&parts])).0
}

fn title(c: &mut Client, req: u32, id: &[u8; 32], size: u32) -> Option<String> {
    let (st, rest) = c.ask(&msg(0x2b, req, &[id, &size.to_le_bytes()]));
    match st {
        1 => Some(R(&rest).text()),
        2 => None,
        s => panic!("title status {s}"),
    }
}

// --- Processes and fixtures -------------------------------------------------------

/// A node process, killed on drop.
struct NodeProc {
    child: Child,
    socket: PathBuf,
    log: PathBuf,
}

impl NodeProc {
    fn wait_ready(mut self) -> Self {
        let deadline = Instant::now() + TIMEOUT;
        while UnixStream::connect(&self.socket).is_err() {
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("the node exited ({status}): {}", self.log());
            }
            assert!(Instant::now() < deadline, "the node did not start: {}", self.log());
            std::thread::sleep(Duration::from_millis(20));
        }
        self
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Stops it as Ctrl-C or a service manager would.
    fn terminate(&mut self) -> std::process::ExitStatus {
        // SAFETY: kill with the PID of a child this test started.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        self.child.wait().unwrap()
    }
}

impl Drop for NodeProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `terse node` with its directories in `dir`, a socket at `socket`
/// and `args`; stderr goes to a file.
fn spawn_terse_node(dir: &Path, socket: &Path, args: &[&str]) -> NodeProc {
    let log = dir.join(format!("node-{}.log", std::process::id() ^ rand_u32()));
    let mut cmd = common::terse_command();
    cmd.arg("node")
        .arg("--socket")
        .arg(socket)
        .arg("--store")
        .arg(dir.join("store"))
        .arg("--config")
        .arg(dir.join("config"))
        .args(args)
        .env_remove("TERSE_NODE_SOCKET")
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap());
    NodeProc { child: cmd.spawn().unwrap(), socket: socket.to_path_buf(), log }
}

fn rand_u32() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(Instant::now().elapsed().as_nanos() as u64);
    h.finish() as u32
}

/// An in-process relay on an ephemeral loopback port.
fn start_relay(store: &Path) -> String {
    let relay = Arc::new(terse_relay::Relay::new(BlobStore::open(store, None).unwrap(), terse_relay::DEFAULT_FEED_CAP));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    url
}

/// A port nothing listens on.
fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn alice() -> Identity {
    Identity::from_seed([21; 32])
}

const ARTICLE: &str = "# Through the node\n\nRead with no cryptography: café, 5 €.\n";
const DESCRIPTION: &str = "Notes from the node test.";

/// Alice publishes a profile (listed, with a description), an article
/// and, with `program`, a program called hello, to `relay`.
fn publish(dir: &Path, relay: &str, program: bool) -> FeedKey {
    let mut node = Node::new(BlobStore::open(dir, None).unwrap(), Some(RelayClient::websocket(relay, TIMEOUT)));
    node.set_identity(alice());
    assert!(matches!(node.publish_profile("Alice", true, Some(DESCRIPTION.into()), 1_700_000_000), terse_net::Done::Published { .. }));
    let draft = Draft { data: ARTICLE.as_bytes().to_vec(), title: "Through the node".into(), reply_to: None, time: 1_700_000_100 };
    assert!(matches!(node.publish_article(&draft), terse_net::Done::Published { .. }));
    if program {
        assert!(matches!(node.publish_program(b"not really an image", "hello", 1_700_000_200), terse_net::Done::Published { .. }));
    }
    alice().feed_key()
}

fn subscribe_in_config(config: &Path, feed: FeedKey) {
    std::fs::create_dir_all(config).unwrap();
    Subscriptions::load(config.join("subscriptions")).unwrap().add(feed).unwrap();
}

fn workspace() -> PathBuf {
    common::workspace()
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

// --- The vector session -----------------------------------------------------------

/// The bytes of a valid entry of vectors/entries.txt.
fn vector_entry(name: &str) -> Vec<u8> {
    let text = std::fs::read_to_string(workspace().join("vectors/entries.txt")).unwrap();
    let start = text.find(&format!("\nentry {name}\n")).unwrap();
    let rest = &text[start + 1..];
    let record = &rest[..rest[1..].find("\n\n").map_or(rest.len(), |i| i + 1)];
    hex(record.lines().find_map(|l| l.trim().strip_prefix("bytes ")).unwrap())
}

/// The node of local-session.txt's first comment: no relays, the entries
/// `article` (arrival 1) and `profile` (arrival 2) and their blobs, the
/// test feed subscribed by its bare key, no nicknames.
fn vector_state(dir: &Path) -> FeedKey {
    let store = dir.join("store");
    let blobs = BlobStore::open(&store, None).unwrap();
    let feeds = FeedStore::beside_blobs(&store);
    let mut arrivals = terse_net::timeline::Arrivals::beside_blobs(&store);
    let mut key = [0; 32];
    for (name, blob) in [("article", &b"Hello, Terse!"[..]), ("profile", b"listed: yes\n")] {
        let signed = terse_net::entry::decode(&vector_entry(name)).unwrap();
        feeds.insert(&signed).unwrap();
        arrivals.record(&signed.entry.feed, signed.entry.seq).unwrap();
        blobs.put(blob).unwrap();
        key = signed.entry.feed;
    }
    subscribe_in_config(&dir.join("config"), key);
    key
}

#[test]
fn vector_session() {
    let tmp = tempfile::tempdir().unwrap();
    vector_state(tmp.path());
    let socket = tmp.path().join("sock/node.sock");
    let _node = spawn_terse_node(tmp.path(), &socket, &["--relay", "none"]).wait_ready();
    let text = std::fs::read_to_string(workspace().join("vectors/local-session.txt")).unwrap();
    let mut c = Client::connect(&socket);
    let mut compared = 0;
    for line in text.lines() {
        let Some((dir, h)) = line.split_once(' ').filter(|(d, _)| *d == "c>" || *d == "n>") else { continue };
        let bytes = hex(h);
        match dir {
            "c>" => c.send(&bytes),
            // vectors/README: pushes may come or not; a refusal's reason is
            // not compared, nor the Hello reply's flags (whether this node
            // answers Find and Lookup).
            "n>" if bytes[0] >= 0xc0 => {}
            "n>" => {
                let got = c.reply();
                let n = if bytes[0] == 0xa0 && bytes.len() == 8 {
                    7
                } else if (0xa0..=0xab).contains(&bytes[0]) && bytes[5] == 3 {
                    6
                } else {
                    bytes.len()
                };
                assert_eq!(hex_of(&got[..n.min(got.len())]), hex_of(&bytes[..n]), "after {compared} replies");
                if n == bytes.len() {
                    assert_eq!(got.len(), bytes.len());
                }
                compared += 1;
            }
            _ => {}
        }
    }
    assert_eq!(compared, 17);
}

fn hex_of(b: &[u8]) -> String {
    to_hex_any(b)
}

fn to_hex_any(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// --- Every message ----------------------------------------------------------------

#[test]
fn every_message_and_both_pushes() {
    let tmp = tempfile::tempdir().unwrap();
    let relay = start_relay(&tmp.path().join("relay"));
    let other_relay = start_relay(&tmp.path().join("relay2"));
    let feed_key = publish(&tmp.path().join("alice"), &relay, true);
    let alice_hex = to_hex(&feed_key);
    let socket = tmp.path().join("run/node.sock");
    let node = spawn_terse_node(tmp.path(), &socket, &["--relay", &relay]).wait_ready();
    let (mut c, flags) = Client::hello(&socket);
    assert_eq!(flags, 1, "the reference node answers Find and Lookup");
    let mut req = 1;
    let mut next = || {
        req += 1;
        req
    };

    // Nothing subscribed yet.
    assert!(subscriptions(&mut c, next()).is_empty());
    assert!(timeline(&mut c, next(), 0, 10).is_empty());

    // Subscribe: new, then not new; a Changed follows.
    let r = next();
    assert_eq!(c.ask(&msg(0x25, r, &[&short(alice_hex.as_bytes())])), (1, vec![1]));
    c.wait_push(0xc0);
    let r = next();
    assert_eq!(c.ask(&msg(0x25, r, &[&short(format!("terse:feed/{alice_hex}").as_bytes())])), (1, vec![0]));
    let r = next();
    let (st, why) = c.ask(&msg(0x25, r, &[&short(b"not an address")]));
    assert_eq!(st, 3);
    assert_eq!(why.len(), 1 + why[0] as usize);
    let r = next();
    assert_eq!(c.ask(&msg(0x25, r, &[&short(&[b'0'; 64])])).0, 3, "an invalid key");

    // Sync, then the timeline: the article only, named, held, needing
    // full text (the euro sign).
    let r = next();
    assert_eq!(c.ask(&msg(0x2a, r, &[&20_000u32.to_le_bytes()])).0, 1);
    let tl = timeline(&mut c, next(), 0, 10);
    assert_eq!(tl.len(), 1, "{tl:?}");
    let article = &tl[0];
    assert_eq!((article.feed(), article.kind(), article.title().as_str()), (&feed_key[..], 1, "Through the node"));
    assert_eq!((article.name.as_str(), article.flags), ("Alice", 3));
    assert_eq!(article.blob(), blob_id(ARTICLE.as_bytes()));
    assert!(timeline(&mut c, next(), article.arrival, 10).is_empty());
    assert_eq!(timeline(&mut c, next(), 0, 0), vec![]);

    // Held and Get.
    let id = article.blob();
    let missing = blob_id(b"nobody has this");
    let r = next();
    assert_eq!(c.ask(&msg(0x21, r, &[&id])), (1, ARTICLE.as_bytes().to_vec()));
    let r = next();
    assert_eq!(c.ask(&msg(0x21, r, &[&missing])), (2, vec![]));
    let r = next();
    assert_eq!(c.ask(&msg(0x01, r, &[&id])), (1, ARTICLE.as_bytes().to_vec()));
    let r = next();
    assert_eq!(c.ask(&msg(0x01, r, &[&missing])), (2, vec![]), "the relay has it not");

    // Title.
    assert_eq!(title(&mut c, next(), &id, article.size()).as_deref(), Some("Through the node"));
    assert_eq!(title(&mut c, next(), &id, article.size() + 1), None);

    // Subscriptions and Feed.
    let subs = subscriptions(&mut c, next());
    assert_eq!(subs.len(), 1);
    let want = FeedRec {
        key: feed_key.to_vec(),
        flags: 1,
        name: "Alice".into(),
        hints: String::new(),
        hint_urls: vec![],
        hinted: vec![],
        profile_relays: vec![],
        description: DESCRIPTION.into(),
    };
    assert_eq!(subs[0], want);
    let (f, recs) = feed(&mut c, next(), &alice_hex);
    assert_eq!(f, want);
    assert_eq!(recs.iter().map(|r| (r.seq(), r.kind())).collect::<Vec<_>>(), vec![(1, 2), (2, 1), (3, 3)]);
    assert!(recs.iter().all(|r| r.flags & 1 == 1 && r.name == "Alice"), "all held: {recs:?}");
    assert_eq!(recs[1], *article);
    // A feed not subscribed, with hints: they are not the hints it was
    // subscribed with (LOCAL.md 4.3), so none.
    let bob = Identity::from_seed([22; 32]).feed_key();
    let (f, recs) = feed(&mut c, next(), &format!("{}@relay.example", to_hex(&bob)));
    assert_eq!((f.flags, f.name.as_str(), f.hints.as_str(), recs.len()), (0, "", "", 0));
    let r = next();
    assert_eq!(c.ask(&msg(0x24, r, &[&[0], &short(b"nonsense")])).0, 3);
    // Without entries.
    let r = next();
    let (st, rest) = c.ask(&msg(0x24, r, &[&[0], &short(alice_hex.as_bytes())]));
    assert_eq!(st, 1);
    let mut rd = R(&rest);
    assert_eq!(feed_record(&mut rd), want);
    assert!(rd.0.is_empty());

    // Nickname: set (Changed), shown everywhere, refused if not a display
    // name, removed.
    c.take_pushes();
    let r = next();
    assert_eq!(c.ask(&msg(0x27, r, &[&feed_key, &short(b"Al")])).0, 1);
    c.wait_push(0xc0);
    assert_eq!(timeline(&mut c, next(), 0, 10)[0].name, "Al");
    assert_eq!(subscriptions(&mut c, next())[0].flags, 3);
    let r = next();
    assert_eq!(c.ask(&msg(0x27, r, &[&feed_key, &short(b" Al")])).0, 3);
    let r = next();
    assert_eq!(c.ask(&msg(0x27, r, &[&feed_key, &short(b"")])).0, 1);
    assert_eq!(subscriptions(&mut c, next())[0], want);

    // Find (NETWORK.md 4.7) and Lookup (4.8), through the node.
    let r = next();
    let (st, rest) = c.ask(&msg(0x06, r, &[&[1], &short(b"ali")]));
    assert_eq!((st, rest[0]), (1, 1), "{rest:x?}");
    assert_eq!(&rest[3..35], &feed_key[..], "{rest:x?}");
    let r = next();
    let (st, rest) = c.ask(&msg(0x06, r, &[&[2], &short(b"THROUGH")]));
    assert_eq!((st, rest[0]), (1, 1));
    let r = next();
    assert_eq!(c.ask(&msg(0x06, r, &[&[9], &short(b"x")])), (3, vec![0]));
    let r = next();
    let (st, entry) = c.ask(&msg(0x08, r, &[&feed_key, &[3], &short(b"hello")]));
    assert_eq!((st, &entry[..32], entry[40]), (1, &feed_key[..], 3));
    let r = next();
    assert_eq!(c.ask(&msg(0x08, r, &[&feed_key, &[3], &short(b"nothing")])), (2, vec![]));
    let r = next();
    assert_eq!(c.ask(&msg(0x08, r, &[&feed_key, &[1], &short(b"hello")])).0, 3);

    // Relays and Set relays: the configured list first, in order; a relay
    // that cannot be reached is logged.
    assert_eq!(relays(&mut c, next()), vec![(5, relay.clone())]);
    let dead = format!("ws://127.0.0.1:{}", closed_port());
    c.take_pushes();
    assert_eq!(set_relays(&mut c, next(), &[&other_relay, &relay, &dead]), 1);
    let saved = std::fs::read_to_string(tmp.path().join("config/relays")).unwrap();
    assert_eq!(saved, format!("{other_relay}\n{relay}\n{dead}\n"));
    let mut logs = Vec::new();
    // A Log for the relay that cannot be reached, and one for the new one.
    while !(logs.iter().any(|l: &String| l.contains(&dead)) && logs.iter().any(|l| l.contains(&other_relay))) {
        let log = c.wait_push(0xc1);
        logs.push(String::from_utf8_lossy(&log[3..]).into_owned());
    }
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let list = relays(&mut c, next());
        let urls: Vec<&str> = list.iter().map(|(_, u)| u.as_str()).collect();
        assert_eq!(urls, [other_relay.as_str(), relay.as_str(), dead.as_str()]);
        if list.iter().map(|(f, _)| *f).collect::<Vec<_>>() == [5, 5, 1] {
            break;
        }
        assert!(Instant::now() < deadline, "{list:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(set_relays(&mut c, next(), &["http://relay.example"]), 3);
    let nine: Vec<String> = (0..9).map(|i| format!("ws://r{i}.example")).collect();
    assert_eq!(set_relays(&mut c, next(), &nine.iter().map(String::as_str).collect::<Vec<_>>()), 3);
    assert_eq!(set_relays(&mut c, next(), &[&relay]), 1);
    assert_eq!(relays(&mut c, next()), vec![(5, relay.clone())]);

    // Unsubscribe: twice is fine; the timeline is empty.
    let r = next();
    assert_eq!(c.ask(&msg(0x26, r, &[&feed_key])).0, 1);
    let r = next();
    assert_eq!(c.ask(&msg(0x26, r, &[&feed_key])).0, 1);
    assert!(timeline(&mut c, next(), 0, 10).is_empty());
    assert!(subscriptions(&mut c, next()).is_empty());
    let saved = std::fs::read_to_string(tmp.path().join("config/subscriptions")).unwrap_or_default();
    assert!(!saved.contains(&alice_hex), "{saved}");

    // A type it does not know and relay requests that are not local
    // messages are ignored; the next request is answered.
    c.send(&msg(0x3f, 900, &[]));
    c.send(&msg(0x02, 901, &[b"a put"]));
    assert_eq!(relays(&mut c, next()).len(), 1);

    // A second reader on the same node sees a change the first one makes.
    let (mut d, _) = Client::hello(&socket);
    let r = next();
    assert_eq!(c.ask(&msg(0x25, r, &[&short(alice_hex.as_bytes())])), (1, vec![1]));
    d.wait_push(0xc0);
    assert_eq!(subscriptions(&mut d, 1).len(), 1);

    // Connections the node closes.
    let not_hello = &mut Client::connect(&socket);
    not_hello.send(&msg(0x23, 1, &[]));
    assert!(not_hello.closed(), "a first message that is not Hello");
    let (mut bad, _) = Client::hello(&socket);
    bad.send(&msg(0x23, 2, &[&[0]]));
    assert!(bad.closed(), "a known type with a byte too many");
    let (mut bad, _) = Client::hello(&socket);
    bad.send(&msg(0x22, 2, &[&[0]]));
    assert!(bad.closed(), "a known type cut short");
    let (mut bad, _) = Client::hello(&socket);
    bad.stream.write_all(&(2_097_153u32).to_le_bytes()).unwrap();
    assert!(bad.closed(), "a length over the limit");
    let (mut bad, _) = Client::hello(&socket);
    bad.send(&msg(0x20, 3, &[&[0], &short(b"")]));
    assert!(bad.closed(), "Hello again");
    // A version it does not speak is refused; Hello is still awaited.
    let mut v = Client::connect(&socket);
    let (st, rest) = v.ask(&msg(0x20, 1, &[&[1], &short(b"")]));
    assert_eq!((st, rest.len()), (3, 1 + rest[0] as usize));
    let (st, rest) = v.ask(&msg(0x20, 2, &[&[0], &short(b"")]));
    assert_eq!((st, rest), (1, vec![0, 1]));
    assert_eq!(subscriptions(&mut v, 3).len(), 1);
    drop(node);
}

// --- LOCAL.md 4.5 -----------------------------------------------------------------

/// A "relay" that accepts connections and never answers.
fn silent_relay() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for s in listener.incoming() {
            held.push(s);
        }
    });
    url
}

#[test]
fn held_state_answers_while_the_network_waits() {
    let tmp = tempfile::tempdir().unwrap();
    let key = vector_state(tmp.path());
    let socket = tmp.path().join("run/node.sock");
    let silent = silent_relay();
    let _node = spawn_terse_node(tmp.path(), &socket, &["--relay", &silent]).wait_ready();
    let (mut c, _) = Client::hello(&socket);
    let hello = blob_id(b"Hello, Terse!");
    // A Get the relay will never answer, and a Sync that cannot finish.
    c.send(&msg(0x01, 100, &[&blob_id(b"not held")]));
    c.send(&msg(0x2a, 101, &[&60_000u32.to_le_bytes()]));
    let start = Instant::now();
    let mut slowest = Duration::ZERO;
    for round in 0..20u32 {
        let t = Instant::now();
        let r = 1000 + round * 10;
        assert_eq!(c.ask(&msg(0x21, r, &[&hello])).0, 1);
        assert_eq!(timeline(&mut c, r + 1, 0, 10).len(), 1);
        assert_eq!(subscriptions(&mut c, r + 2).len(), 1);
        assert_eq!(feed(&mut c, r + 3, &to_hex(&key)).1.len(), 2);
        assert_eq!(relays(&mut c, r + 4), vec![(1, silent.clone())]);
        assert_eq!(title(&mut c, r + 5, &hello, 13).as_deref(), Some("Hello"));
        assert_eq!(c.ask(&msg(0x01, r + 6, &[&hello])).0, 1);
        slowest = slowest.max(t.elapsed());
    }
    eprintln!("7 requests answered at once 20 times in {:?} (slowest round {slowest:?}) while a Get and a Sync wait", start.elapsed());
    assert!(slowest < Duration::from_secs(1), "{slowest:?}");
}

// --- The socket -------------------------------------------------------------------

#[test]
fn one_node_per_socket() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("run/terse/node.sock");
    let mut first = spawn_terse_node(tmp.path(), &socket, &["--relay", "none"]).wait_ready();
    let meta = std::fs::metadata(&socket).unwrap();
    assert_eq!(meta.mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(socket.parent().unwrap()).unwrap().mode() & 0o777, 0o700);

    // A second node on the same socket does not start.
    let out = common::terse_command()
        .args(["node", "--relay", "none", "--socket"])
        .arg(&socket)
        .arg("--store")
        .arg(tmp.path().join("store"))
        .arg("--config")
        .arg(tmp.path().join("config"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("already running"), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(UnixStream::connect(&socket).is_ok(), "the first node still serves");

    // Stopped, it removes its socket.
    assert!(first.terminate().success(), "{}", first.log());
    assert!(!socket.exists());

    // A stale socket (nobody listens) is replaced.
    drop(UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    let second = spawn_terse_node(tmp.path(), &socket, &["--relay", "none"]).wait_ready();
    let (mut c, _) = Client::hello(&socket);
    assert!(subscriptions(&mut c, 2).is_empty());
    assert!(second.log().contains("stale"), "{}", second.log());
    drop(second);

    // A file that is not a socket is left alone.
    let file = tmp.path().join("run/terse/file.sock");
    std::fs::write(&file, "mine").unwrap();
    let out = common::terse_command().args(["node", "--relay", "none", "--socket"]).arg(&file).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "mine");

    // A directory other users can enter is refused.
    let open = tmp.path().join("open");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = common::terse_command().args(["node", "--relay", "none", "--socket"]).arg(open.join("node.sock")).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("open to other users"), "{}", String::from_utf8_lossy(&out.stderr));

    // The default path: $XDG_RUNTIME_DIR/terse/node.sock, the directory made 0700.
    let runtime = tmp.path().join("xdg");
    std::fs::create_dir(&runtime).unwrap();
    let log = tmp.path().join("default.log");
    let child = common::terse_command()
        .args(["node", "--relay", "none", "--store"])
        .arg(tmp.path().join("store"))
        .arg("--config")
        .arg(tmp.path().join("config"))
        .env_remove("TERSE_NODE_SOCKET")
        .env("XDG_RUNTIME_DIR", &runtime)
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let default = NodeProc { child, socket: runtime.join("terse/node.sock"), log }.wait_ready();
    assert_eq!(std::fs::metadata(runtime.join("terse")).unwrap().mode() & 0o777, 0o700);
    drop(default);
    // TERSE_NODE_SOCKET overrides it.
    let env_socket = tmp.path().join("run/terse/env.sock");
    let log = tmp.path().join("env.log");
    let child = common::terse_command()
        .args(["node", "--relay", "none", "--store"])
        .arg(tmp.path().join("store"))
        .arg("--config")
        .arg(tmp.path().join("config"))
        .env("TERSE_NODE_SOCKET", &env_socket)
        .env("XDG_RUNTIME_DIR", &runtime)
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    drop(NodeProc { child, socket: env_socket, log }.wait_ready());
}

// --- With Go ----------------------------------------------------------------------

/// Builds the Go reader (`thin`: its thin build) into `dir`; `None`, saying
/// so, when Go is not installed.
fn build_go_reader(dir: &Path, thin: bool) -> Option<PathBuf> {
    if !Command::new("go").arg("version").output().is_ok_and(|o| o.status.success()) {
        terse_testkit::skip!("go", "needs Go (`go` not found) to build native/reader-go");
        return None;
    }
    let out = dir.join(if thin { "tersereader-thin" } else { "tersereader" });
    let mut cmd = Command::new("go");
    cmd.args(["build", "-p", "4"]);
    if thin {
        cmd.args(["-tags", "thin"]);
    }
    let built = cmd.arg("-o").arg(&out).arg(".").current_dir(workspace().join("native/reader-go")).env("CGO_ENABLED", "0").output().unwrap();
    assert!(built.status.success(), "go build: {}", String::from_utf8_lossy(&built.stderr));
    Some(out)
}

/// The Go reader with every directory in `home`.
fn go_reader(exe: &Path, home: &Path, socket: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg-config"))
        .env("XDG_CACHE_HOME", home.join("xdg-cache"))
        .env("XDG_DATA_HOME", home.join("xdg-data"))
        .env("XDG_RUNTIME_DIR", home.join("xdg-run"))
        .env("TERSE_NODE_SOCKET", socket);
    cmd
}

fn run_ok(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{cmd:?}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap() + &String::from_utf8_lossy(&out.stderr)
}

#[test]
fn go_thin_reader_through_terse_node() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(thin) = build_go_reader(tmp.path(), true) else { return };
    let relay = start_relay(&tmp.path().join("relay"));
    let key = publish(&tmp.path().join("alice"), &relay, false);
    let socket = tmp.path().join("run/node.sock");
    let node = spawn_terse_node(tmp.path(), &socket, &["--relay", &relay]).wait_ready();
    let reader = |args: &[&str]| run_ok(go_reader(&thin, tmp.path(), &socket).args(args));

    reader(&["subscribe", &to_hex(&key)]);
    reader(&["sync", "--timeout", "20s"]);
    let list = reader(&["list"]);
    assert!(list.contains("Through the node") && list.contains("Alice (") && list.contains("[needs full text]"), "{list}");
    let id = to_hex(&blob_id(ARTICLE.as_bytes()));
    let plain = reader(&["--dump", &id, "--plain"]);
    assert!(plain.contains("Read with no cryptography: café, 5 €."), "{plain}");
    let feeds = reader(&["feeds"]);
    assert!(feeds.contains("Alice"), "{feeds}");
    let one = reader(&["feed", &to_hex(&key)]);
    assert!(one.contains(DESCRIPTION) && one.contains("Through the node"), "{one}");
    reader(&["nickname", &to_hex(&key), "Al"]);
    // The change is the node's: this client sees it, and so does the
    // reference configuration.
    let (mut c, _) = Client::hello(&socket);
    assert_eq!(subscriptions(&mut c, 2)[0].name, "Al");
    assert!(std::fs::read_to_string(tmp.path().join("config/nicknames")).unwrap().contains("Al"));
    reader(&["relay", "list"]);
    reader(&["unsubscribe", &to_hex(&key)]);
    assert!(subscriptions(&mut c, 3).is_empty());
    drop(node);
}

/// An article with an empty title (LOCAL.md 4.3, Title).
const UNTITLED: &str = "No title on this one.\n";

/// Alice publishes, after [`publish`], [`ARTICLE`] again under another
/// title (a Title of several entries is the first stored) and
/// [`UNTITLED`] with an empty title.
fn publish_more(dir: &Path, relay: &str) {
    let mut node = Node::new(BlobStore::open(dir, None).unwrap(), Some(RelayClient::websocket(relay, TIMEOUT)));
    node.set_identity(alice());
    for (data, title, time) in [(ARTICLE, "Stored second", 1_700_000_300), (UNTITLED, "", 1_700_000_400)] {
        let draft = Draft { data: data.as_bytes().to_vec(), title: title.into(), reply_to: None, time };
        assert!(matches!(node.publish_article(&draft), terse_net::Done::Published { .. }));
    }
}

/// A reply reduced for comparing: a refusal to its status, a Hello reply
/// without its flags (whether the node answers Find and Lookup).
fn reduced(r: Vec<u8>) -> Vec<u8> {
    if (0xa0..=0xab).contains(&r[0]) && r[5] == 3 {
        r[..6].to_vec()
    } else if r[0] == 0xa0 && r.len() == 8 {
        r[..7].to_vec()
    } else {
        r
    }
}

/// Asks Relays until `done` holds for the list (at most `TIMEOUT`), and
/// returns that reply.
fn relays_until(c: &mut Client, req: u32, done: impl Fn(&[(u8, String)]) -> bool) -> Vec<u8> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let r = c.call(&msg(0x28, req, &[]));
        let mut f = R(&r[6..]);
        let list: Vec<(u8, String)> = (0..f.u8()).map(|_| (f.u8(), f.text())).collect();
        if done(&list) {
            return r;
        }
        assert!(Instant::now() < deadline, "relays: {list:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The same questions to two nodes holding the same feed, with `relays`
/// the node's relay and another; the replies, for comparing ([`reduced`]).
/// A connection the node closes is recorded as the reply "closed".
///
/// Not compared, because the Go node does not offer them (Hello flags):
/// Find and Lookup. Not here, because they need thousands of entries:
/// replies over 2 MiB (each node's own tests).
fn script(socket: &Path, key: &FeedKey, relays: [&str; 2]) -> Vec<(String, Vec<u8>)> {
    let (mut c, _) = Client::hello(socket);
    let key_hex = to_hex(key);
    let id = blob_id(ARTICLE.as_bytes());
    let bob = Identity::from_seed([22; 32]).feed_key();
    let invalid = [0xffu8; 32]; // not a point's encoding (y >= p)
    let mut out = Vec::new();
    fn ask(out: &mut Vec<(String, Vec<u8>)>, c: &mut Client, what: &str, m: Vec<u8>) {
        let r = reduced(c.call(&m));
        out.push((what.to_string(), r));
    }
    assert_eq!(c.ask(&msg(0x2a, 2, &[&20_000u32.to_le_bytes()])).0, 1, "sync");
    ask(&mut out, &mut c, "subscriptions", msg(0x23, 3, &[]));
    ask(&mut out, &mut c, "timeline", msg(0x22, 4, &[&0u32.to_le_bytes(), &10u16.to_le_bytes()]));
    ask(&mut out, &mut c, "timeline before 2", msg(0x22, 5, &[&2u32.to_le_bytes(), &10u16.to_le_bytes()]));
    ask(&mut out, &mut c, "feed with entries", msg(0x24, 6, &[&[1], &short(key_hex.as_bytes())]));
    ask(&mut out, &mut c, "feed not subscribed", msg(0x24, 7, &[&[1], &short(to_hex(&bob).as_bytes())]));
    ask(&mut out, &mut c, "feed malformed", msg(0x24, 8, &[&[1], &short(b"nonsense")]));
    ask(&mut out, &mut c, "held", msg(0x21, 9, &[&id]));
    ask(&mut out, &mut c, "held missing", msg(0x21, 10, &[&blob_id(b"missing")]));
    ask(&mut out, &mut c, "get", msg(0x01, 11, &[&id]));
    ask(&mut out, &mut c, "get missing", msg(0x01, 12, &[&blob_id(b"missing")]));
    ask(&mut out, &mut c, "title", msg(0x2b, 13, &[&id, &(ARTICLE.len() as u32).to_le_bytes()]));
    ask(&mut out, &mut c, "title wrong size", msg(0x2b, 14, &[&id, &1u32.to_le_bytes()]));
    ask(&mut out, &mut c, "relays", msg(0x28, 15, &[]));
    ask(&mut out, &mut c, "set relays not a URL", msg(0x29, 16, &[&[1], &short(b"http://relay.example")]));
    ask(&mut out, &mut c, "nickname", msg(0x27, 17, &[key, &short(b"Al")]));
    ask(&mut out, &mut c, "nickname not a name", msg(0x27, 18, &[key, &short(b" Al")]));
    ask(&mut out, &mut c, "subscriptions with a nickname", msg(0x23, 19, &[]));
    ask(&mut out, &mut c, "nickname removed", msg(0x27, 20, &[key, &short(b"")]));
    ask(&mut out, &mut c, "subscribe again", msg(0x25, 21, &[&short(key_hex.as_bytes())]));
    ask(&mut out, &mut c, "subscribe malformed", msg(0x25, 22, &[&short(b"not an address")]));
    ask(&mut out, &mut c, "subscribe bob", msg(0x25, 23, &[&short(to_hex(&bob).as_bytes())]));
    ask(&mut out, &mut c, "unsubscribe bob", msg(0x26, 24, &[&bob]));
    ask(&mut out, &mut c, "unsubscribe bob again", msg(0x26, 25, &[&bob]));
    ask(&mut out, &mut c, "subscriptions after bob", msg(0x23, 26, &[]));

    // LOCAL.md 3: a nickname for an invalid key is refused; unsubscribing
    // from one succeeds (it is not subscribed).
    ask(&mut out, &mut c, "nickname for an invalid key", msg(0x27, 27, &[&invalid, &short(b"Al")]));
    ask(&mut out, &mut c, "unsubscribe an invalid key", msg(0x26, 28, &[&invalid]));
    // Feed: the hints the feed was subscribed with, not the address's.
    ask(&mut out, &mut c, "feed with other hints", msg(0x24, 29, &[&[0], &short(format!("{key_hex}@relay.example:1").as_bytes())]));
    ask(&mut out, &mut c, "feed not subscribed, with hints", msg(0x24, 30, &[&[0], &short(format!("{}@relay.example:1", to_hex(&bob)).as_bytes())]));
    // (A loopback hint, which a node never follows: NETWORK.md 5.1.)
    ask(&mut out, &mut c, "subscribe bob with a hint", msg(0x25, 50, &[&short(format!("{}@127.0.0.1:1", to_hex(&bob)).as_bytes())]));
    ask(&mut out, &mut c, "feed subscribed with a hint, asked with another", msg(0x24, 51, &[&[0], &short(format!("{}@relay.example:1", to_hex(&bob)).as_bytes())]));
    ask(&mut out, &mut c, "unsubscribe bob with a hint", msg(0x26, 52, &[&bob]));
    // Title: an empty one is status 1 with n 0 (of several, "title" above
    // is the first stored).
    ask(&mut out, &mut c, "title empty", msg(0x2b, 31, &[&blob_id(UNTITLED.as_bytes()), &(UNTITLED.len() as u32).to_le_bytes()]));

    // Set relays, at once: a URL twice counts once; the new relay is
    // connected; one dropped goes; with none, Sync is refused and a Get not
    // held is not found.
    let [relay, other] = relays;
    let urls = |list: &[&str]| -> Vec<u8> {
        let mut m = vec![list.len() as u8];
        for u in list {
            m.extend(short(u.as_bytes()));
        }
        m
    };
    ask(&mut out, &mut c, "set relays, one twice", msg(0x29, 32, &[&urls(&[relay, other, relay])]));
    let r = relays_until(&mut c, 33, |l| l.len() == 2 && l.iter().all(|(f, _)| *f == 5));
    out.push(("relays, both connected".into(), r));
    ask(&mut out, &mut c, "set relays, the first dropped", msg(0x29, 34, &[&urls(&[other])]));
    let r = relays_until(&mut c, 35, |l| l == [(5, other.to_string())]);
    out.push(("relays, the first gone".into(), r));
    ask(&mut out, &mut c, "set no relays", msg(0x29, 36, &[&urls(&[])]));
    let r = relays_until(&mut c, 37, |l| l.is_empty());
    out.push(("relays, none".into(), r));
    ask(&mut out, &mut c, "sync with no relay", msg(0x2a, 38, &[&1000u32.to_le_bytes()]));
    ask(&mut out, &mut c, "get not held, no relay", msg(0x01, 39, &[&blob_id(b"missing")]));
    ask(&mut out, &mut c, "set relays back", msg(0x29, 40, &[&urls(&[relay])]));
    let r = relays_until(&mut c, 41, |l| l == [(5, relay.to_string())]);
    out.push(("relays, back".into(), r));
    ask(&mut out, &mut c, "subscriptions at the end", msg(0x23, 42, &[]));

    // LOCAL.md 3 and 4.1, each on a connection of its own: a first message
    // that is not Hello, and a second Hello, close it; a version the node
    // does not speak is refused and Hello is awaited again.
    let closed = |c: &mut Client| if c.closed() { b"closed".to_vec() } else { b"open".to_vec() };
    let mut d = Client::connect(socket);
    d.send(&msg(0x28, 1, &[]));
    out.push(("not Hello first".into(), closed(&mut d)));
    let (mut d, _) = Client::hello(socket);
    d.send(&msg(0x20, 2, &[&[0], &short(b"")]));
    out.push(("a second Hello".into(), closed(&mut d)));
    let mut d = Client::connect(socket);
    out.push(("Hello, version 1".into(), reduced(d.call(&msg(0x20, 1, &[&[1], &short(b"")])))));
    out.push(("then Hello, version 0".into(), reduced(d.call(&msg(0x20, 2, &[&[0], &short(b"")])))));
    out.push(("and a request".into(), reduced(d.call(&msg(0x23, 3, &[])))));
    out
}

#[test]
fn terse_node_and_go_node_answer_alike() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(go) = build_go_reader(tmp.path(), false) else { return };
    let relay = start_relay(&tmp.path().join("relay"));
    let other = start_relay(&tmp.path().join("other"));
    let key = publish(&tmp.path().join("alice"), &relay, false);
    publish_more(&tmp.path().join("alice"), &relay);

    // The reference node, subscribed through its configuration.
    let ours = tmp.path().join("ours");
    subscribe_in_config(&ours.join("config"), key);
    let socket = tmp.path().join("run/terse.sock");
    let _terse = spawn_terse_node(&ours, &socket, &["--relay", &relay]).wait_ready();

    // The Go node, subscribed through its own commands.
    let theirs = tmp.path().join("theirs");
    std::fs::create_dir_all(&theirs).unwrap();
    let go_socket = tmp.path().join("run/go.sock");
    let go_store = theirs.join("store");
    run_ok(go_reader(&go, &theirs, &go_socket).arg("--dir").arg(&go_store).args(["--node", "in-process", "relay", "add", &relay]));
    run_ok(go_reader(&go, &theirs, &go_socket).arg("--dir").arg(&go_store).args(["--node", "in-process", "subscribe", &to_hex(&key)]));
    let log = theirs.join("node.log");
    let child = go_reader(&go, &theirs, &go_socket)
        .arg("--dir")
        .arg(&go_store)
        .arg("node")
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let _go = NodeProc { child, socket: go_socket.clone(), log }.wait_ready();

    let a = script(&socket, &key, [&relay, &other]);
    let b = script(&go_socket, &key, [&relay, &other]);
    assert_eq!(a.len(), b.len());
    let mut differ = Vec::new();
    for ((what, x), (_, y)) in a.iter().zip(&b) {
        if x != y {
            differ.push(format!("{what}:\n  terse node      {}\n  tersereader node {}", to_hex_any(x), to_hex_any(y)));
        }
    }
    assert!(differ.is_empty(), "the nodes answer differently:\n{}", differ.join("\n"));
    // And what LOCAL.md says they answer.
    let reply = |what: &str| a.iter().find(|(w, _)| w == what).unwrap().1.clone();
    let st = |what: &str| reply(what)[5];
    assert_eq!(R(&reply("title")[6..]).text(), "Through the node", "of several, the first stored");
    assert_eq!(&reply("title empty")[5..], &[1, 0]);
    assert_eq!(feed_record(&mut R(&reply("feed with other hints")[6..])).hints, "");
    assert_eq!(feed_record(&mut R(&reply("feed subscribed with a hint, asked with another")[6..])).hints, "127.0.0.1:1");
    assert_eq!(feed_record(&mut R(&reply("feed not subscribed, with hints")[6..])).hints, "");
    for (what, want) in [
        ("nickname for an invalid key", 3),
        ("unsubscribe an invalid key", 1),
        ("set relays, one twice", 1),
        ("sync with no relay", 3),
        ("get not held, no relay", 2),
        ("Hello, version 1", 3),
        ("then Hello, version 0", 1),
    ] {
        assert_eq!(st(what), want, "{what}");
    }
    for what in ["not Hello first", "a second Hello"] {
        assert_eq!(reply(what), b"closed", "{what}");
    }
    eprintln!("terse node and tersereader node gave the same {} replies", a.len());
}
