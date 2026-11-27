//! The deny list (README "Running a public relay"; NETWORK 5.3, "Its own
//! policy"): a denied feed's entries refused and removed in batches while
//! requests are answered, its subscribers cut off; a denied blob never
//! stored or served; a denied address refused at the handshake, behind a
//! trusted proxy too; the list read again, kept over a restart, not passed
//! on by mirrors; what the operator sees of the store. Counts and the
//! relay's own clock only.

use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_net::blob::to_hex;
use terse_net::protocol::{self, Reply, SCOPE_ENTRIES, status};
use terse_net::{BlobStore, Entry, Identity, blob_id};
use terse_relay::deny::{self, Item};
use terse_relay::limits::Cidr;
use terse_relay::{Answer, Connection, PURGE_BATCH, Policy, Reason, Relay, admin};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

fn relay(dir: &Path, policy: Policy) -> Arc<Relay> {
    let r = Arc::new(Relay::with_policy(BlobStore::open(dir, None).unwrap(), policy).unwrap());
    // Batches end by count only: the relay's clock stands still.
    r.freeze_clock();
    r
}

fn denying(list: &Path) -> Policy {
    Policy { deny: Some(list.to_path_buf()), ..Policy::unlimited(1024) }
}

fn ask(relay: &Relay, conn: &Connection, message: Vec<u8>) -> Reply {
    match relay.respond(conn, &message) {
        Answer::Reply(r) => protocol::decode_reply(&r).unwrap(),
        _ => panic!("no reply"),
    }
}

fn put(relay: &Relay, conn: &Connection, data: &[u8]) -> u8 {
    match ask(relay, conn, protocol::encode_put(1, data)) {
        Reply::Put { status, .. } => status,
        r => panic!("{r:?}"),
    }
}

fn get(relay: &Relay, conn: &Connection, data: &[u8]) -> Option<Vec<u8>> {
    match ask(relay, conn, protocol::encode_get(1, &blob_id(data))) {
        Reply::Get { status: status::DONE, data, .. } => Some(data),
        Reply::Get { status: status::NOT_FOUND, .. } => None,
        r => panic!("{r:?}"),
    }
}

fn publish(relay: &Relay, conn: &Connection, entry: &Entry, id: &Identity) -> u8 {
    let bytes = entry.clone().sign(id).unwrap().bytes;
    match ask(relay, conn, protocol::encode_publish(1, &bytes)) {
        Reply::Status { status, .. } => status,
        r => panic!("{r:?}"),
    }
}

fn subscribe(relay: &Relay, conn: &Connection, id: &Identity) -> u8 {
    match ask(relay, conn, protocol::encode_subscribe(1, &id.feed_key(), 0)) {
        Reply::Subscribed { .. } => status::DONE,
        Reply::Status { status, .. } => status,
        r => panic!("{r:?}"),
    }
}

fn article(id: &Identity, seq: u32, data: &[u8], title: &str) -> Entry {
    Entry::article(id.feed_key(), seq, 1_800_000_000 + seq, data, None, title)
}

fn on_disk(relay: &Relay, data: &[u8]) -> bool {
    relay.blobs().path(&blob_id(data)).exists()
}

/// Puts the blob and publishes an article naming it.
fn post(relay: &Relay, conn: &Connection, id: &Identity, seq: u32, data: &[u8]) {
    assert_eq!(put(relay, conn, data), status::DONE);
    assert_eq!(publish(relay, conn, &article(id, seq, data, &format!("post {seq}")), id), status::DONE);
}

fn list(dir: &Path) -> PathBuf {
    dir.join("deny")
}

/// A feed denied while it has entries and a subscriber: its entries are
/// refused, the subscription ends and a new one is refused, its entries
/// and the blobs only it names are removed in batches with requests
/// answered between them, a blob another feed names stays.
#[test]
fn deny_a_feed() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(&dir.path().join("store"), denying(&list(dir.path())));
    let (spam, ana) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let c = r.connect();
    let n = PURGE_BATCH as u32 + 6;
    for seq in 1..=n {
        post(&r, &c, &spam, seq, format!("spam {seq}").as_bytes());
    }
    // A blob both feeds name.
    post(&r, &c, &spam, n + 1, b"shared");
    post(&r, &c, &ana, 1, b"shared");
    post(&r, &c, &ana, 2, b"ana's own");
    let subscriber = r.connect();
    assert_eq!(subscribe(&r, &subscriber, &spam), status::DONE);
    assert_eq!(r.subscribers(&spam.feed_key()), 1);
    let profile = Entry::profile(spam.feed_key(), n + 2, 1, b"listed: yes\n", "spam");
    assert_eq!(put(&r, &c, b"listed: yes\n"), status::DONE);
    assert_eq!(publish(&r, &c, &profile, &spam), status::DONE);
    assert!(r.find(protocol::SCOPE_FEEDS, b"spam").is_some_and(|f| f.len() == 1));

    assert!(deny::add(&list(dir.path()), Item::Feed(spam.feed_key()), "spam, reported 2026-11-22").unwrap());
    r.reload_deny(false);
    // At once: refused, subscription ended, not found by search.
    assert_eq!(r.subscribers(&spam.feed_key()), 0);
    assert_eq!(subscribe(&r, &subscriber, &spam), status::REFUSED);
    assert_eq!(publish(&r, &c, &article(&spam, n + 3, b"more", "more"), &spam), status::REFUSED);
    assert_eq!(r.refused(Reason::DeniedFeed), 1);
    assert!(r.find(protocol::SCOPE_FEEDS, b"spam").is_some_and(|f| f.is_empty()));
    assert!(r.find(SCOPE_ENTRIES, b"post").is_some_and(|f| f.is_empty()));

    // The first batch: PURGE_BATCH entries, not all.
    let before = r.feeds().seqs(&spam.feed_key()).unwrap().len();
    assert_eq!(before, n as usize + 2);
    assert_eq!(r.purge_batch(), (PURGE_BATCH, true));
    assert_eq!(r.feeds().seqs(&spam.feed_key()).unwrap().len(), before - PURGE_BATCH);
    // Requests are answered between batches.
    assert_eq!(get(&r, &c, b"ana's own").as_deref(), Some(&b"ana's own"[..]));
    post(&r, &c, &ana, 3, b"ana again");
    // The rest.
    assert_eq!(r.purge(), before - PURGE_BATCH);
    assert_eq!(r.purge_batch(), (0, false));
    assert!(r.feeds().seqs(&spam.feed_key()).unwrap().is_empty());
    assert!(!on_disk(&r, b"spam 1") && !on_disk(&r, format!("spam {n}").as_bytes()) && !on_disk(&r, b"listed: yes\n"));
    assert!(on_disk(&r, b"shared") && get(&r, &c, b"shared").is_some(), "ana names it too");
    assert_eq!(r.feeds().seqs(&ana.feed_key()).unwrap(), [1, 2, 3]);
    let s = r.status();
    assert!(s.contains("denied 1 feeds, 0 blobs, 0 address ranges") && s.contains("denied-feed 1"), "{s}");
    assert!(s.contains(&format!("removed {} entries and {} blobs", n + 2, n + 1)), "{s}");
}

/// A denied blob that an allowed feed names: not served, removed, never
/// stored again; a new entry naming it is refused; the feed's other
/// entries are untouched.
#[test]
fn deny_a_blob() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(&dir.path().join("store"), denying(&list(dir.path())));
    let ana = Identity::from_seed([3; 32]);
    let c = r.connect();
    post(&r, &c, &ana, 1, b"not allowed here");
    post(&r, &c, &ana, 2, b"fine");
    assert!(deny::add(&list(dir.path()), Item::Blob(blob_id(b"not allowed here")), "notice received").unwrap());
    r.reload_deny(false);
    assert_eq!(get(&r, &c, b"not allowed here"), None, "not served, even before it is removed");
    assert_eq!(r.purge(), 1);
    assert!(!on_disk(&r, b"not allowed here"));
    assert_eq!(put(&r, &c, b"not allowed here"), status::REFUSED);
    assert!(!on_disk(&r, b"not allowed here"));
    assert_eq!(publish(&r, &c, &article(&ana, 3, b"not allowed here", "again"), &ana), status::REFUSED);
    assert_eq!((r.refused(Reason::DeniedBlob), r.refused(Reason::DeniedFeed)), (1, 1));
    assert_eq!(r.feeds().seqs(&ana.feed_key()).unwrap(), [1, 2]);
    assert!(get(&r, &c, b"fine").is_some());
    // Allowed again: it may be put again (what was removed is not restored).
    assert_eq!(deny::remove(&list(dir.path()), Item::Blob(blob_id(b"not allowed here"))).unwrap(), 1);
    r.reload_deny(false);
    assert_eq!(put(&r, &c, b"not allowed here"), status::DONE);
}

/// The list is read again when its file changes, or by force (SIGHUP); a
/// bad file keeps the list in force.
#[test]
fn reload() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(&dir.path().join("store"), denying(&list(dir.path())));
    let bo = Identity::from_seed([4; 32]);
    let c = r.connect();
    std::fs::write(list(dir.path()), format!("feed {} # by hand\n", to_hex(&bo.feed_key()))).unwrap();
    r.reload_deny(true);
    assert_eq!(publish(&r, &c, &article(&bo, 1, b"x", "x"), &bo), status::REFUSED);
    std::fs::write(list(dir.path()), "feed nonsense\n").unwrap();
    r.reload_deny(false);
    assert_eq!(publish(&r, &c, &article(&bo, 1, b"x", "x"), &bo), status::REFUSED, "the list in force stays");
    std::fs::write(list(dir.path()), "# nothing denied\n").unwrap();
    r.reload_deny(true);
    assert_eq!(publish(&r, &c, &article(&bo, 1, b"x", "x"), &bo), status::DONE);
}

/// A restart: what was denied and is still on disk is removed, and the
/// denial holds.
#[test]
fn restart_keeps_the_denial() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    let spam = Identity::from_seed([5; 32]);
    {
        let r = relay(&store, Policy::unlimited(1024));
        let c = r.connect();
        post(&r, &c, &spam, 1, b"before the list");
    }
    deny::add(&list(dir.path()), Item::Feed(spam.feed_key()), "").unwrap();
    let r = relay(&store, denying(&list(dir.path())));
    assert_eq!(r.purge(), 1);
    assert!(r.feeds().seqs(&spam.feed_key()).unwrap().is_empty() && !on_disk(&r, b"before the list"));
    drop(r);
    let r = relay(&store, denying(&list(dir.path())));
    let c = r.connect();
    assert_eq!(subscribe(&r, &c, &spam), status::REFUSED);
    assert_eq!(publish(&r, &c, &article(&spam, 2, b"after", "after"), &spam), status::REFUSED);
}

struct Served {
    url: String,
    relay: Arc<Relay>,
    _dir: tempfile::TempDir,
}

fn serve(policy: impl FnOnce(&Path) -> Policy) -> Served {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(Relay::with_policy(BlobStore::open(&dir.path().join("store"), None).unwrap(), policy(dir.path())).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let r = relay.clone();
    std::thread::spawn(move || terse_relay::serve(listener, r));
    Served { url, relay, _dir: dir }
}

fn ws_with(url: &str, headers: &[(&'static str, &str)]) -> Result<WebSocket<TcpStream>, u16> {
    let stream = TcpStream::connect(url.strip_prefix("ws://").unwrap()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut request = url.into_client_request().unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(*name, value.parse().unwrap());
    }
    match tungstenite::client::client_with_config(request, stream, None) {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(r))) => Err(r.status().as_u16()),
        Err(e) => panic!("{e}"),
    }
}

/// An address denied behind a trusted proxy: by the proxy's header; an
/// open connection from it is closed when the list is read again; the
/// client is told why.
#[test]
fn deny_an_address_behind_a_proxy() {
    let cf = "cf-connecting-ip";
    let s = serve(|dir| {
        std::fs::write(list(dir), "address 198.51.100.0/24 flood\n").unwrap();
        Policy { trust_proxy: vec![Cidr::parse("127.0.0.0/8").unwrap()], ..denying(&list(dir)) }
    });
    assert_eq!(ws_with(&s.url, &[(cf, "198.51.100.9")]).err(), Some(403));
    assert_eq!(ws_with(&s.url, &[("x-forwarded-for", "198.51.100.10")]).err(), Some(403));
    let mut open = ws_with(&s.url, &[(cf, "203.0.113.5")]).unwrap();
    let _other = ws_with(&s.url, &[(cf, "203.0.114.5")]).unwrap();
    assert_eq!(s.relay.refused(Reason::DeniedAddress), 2);
    // Denied while connected: closed.
    deny::add(&s.relay.policy().deny.clone().unwrap(), Item::parse("address", "203.0.113.5").unwrap(), "").unwrap();
    s.relay.reload_deny(true);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match open.read() {
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => assert!(Instant::now() < deadline, "still open"),
        }
    }
    assert_eq!(ws_with(&s.url, &[(cf, "203.0.113.5")]).err(), Some(403));
    // The proxy itself, without a header, is not denied.
    assert!(terse_net::relay::WsTransport::connect(&s.url, Duration::from_secs(5)).is_ok());
}

/// Denials are a relay's own: a mirror that denies a feed refuses what its
/// peer sends of it, once (no retry), and the peer keeps it.
#[test]
fn mirrors_do_not_pass_denials_on() {
    let spam = Identity::from_seed([6; 32]);
    let a = serve(|dir| {
        std::fs::write(list(dir), format!("feed {}\n", to_hex(&spam.feed_key()))).unwrap();
        denying(&list(dir))
    });
    let b = serve(|_| Policy::unlimited(1024));
    terse_relay::connect_peers(b.relay.clone(), vec![a.url.clone()]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while a.relay.peer_count() == 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let c = b.relay.connect();
    post(&b.relay, &c, &spam, 1, b"elsewhere");
    let deadline = Instant::now() + Duration::from_secs(10);
    while a.relay.refused(Reason::DeniedFeed) == 0 {
        assert!(Instant::now() < deadline, "the entry never reached the mirror");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(b.relay.feeds().seqs(&spam.feed_key()).unwrap(), [1], "the peer keeps it");
    assert!(a.relay.feeds().seqs(&spam.feed_key()).unwrap().is_empty());
    // Sent once and refused once: the peer does not send it again.
    assert_eq!(b.relay.peer_messages(protocol::PUBLISH), 1);
    assert_eq!(a.relay.refused(Reason::DeniedFeed), 1);
}

/// What the operator sees of a store: feeds by size and by arrival, one
/// feed's entries.
#[test]
fn operator_view() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    let (big, small) = (Identity::from_seed([7; 32]), Identity::from_seed([8; 32]));
    {
        let r = relay(&store, Policy::unlimited(1024));
        let c = r.connect();
        post(&r, &c, &big, 1, &[1; 3000]);
        post(&r, &c, &big, 2, &[2; 3000]);
        post(&r, &c, &small, 1, b"tiny\x1b[31m");
    }
    let by_size = admin::feeds(&store, admin::Order::Size).unwrap();
    assert_eq!(by_size.iter().map(|f| (f.key, f.entries, f.stored)).collect::<Vec<_>>(), [(big.feed_key(), 2, 6000), (small.feed_key(), 1, 9)]);
    let by_arrival = admin::feeds(&store, admin::Order::Arrival).unwrap();
    assert_eq!(by_arrival[0].key, small.feed_key());
    let entries = admin::entries(&store, &big.feed_key()).unwrap();
    assert_eq!(entries.iter().map(|e| (e.seq, e.kind, e.size, e.stored)).collect::<Vec<_>>(), [(1, "article", 3000, true), (2, "article", 3000, true)]);
    assert_eq!(entries[0].title, "post 1");
    assert_eq!(admin::printable("a\x1b[31m"), "a\u{fffd}[31m");
}

/// The worst case of memory, and `--memory`'s connections.
#[test]
fn memory() {
    use terse_relay::{BASE_MEMORY, CONNECTION_MEMORY, MIB, PUBLIC_MAX_CONNECTIONS};
    assert_eq!(Policy::public().max_connections, Some(PUBLIC_MAX_CONNECTIONS));
    let worst = BASE_MEMORY + PUBLIC_MAX_CONNECTIONS as u64 * CONNECTION_MEMORY;
    assert!(worst < 600 * MIB, "{worst}");
    assert_eq!(Policy::connections_for_memory(600 * MIB, 0), 178);
    assert_eq!(Policy::connections_for_memory(32 * MIB, 0), 0);
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), Policy::public());
    assert!(r.status().contains(&format!("memory at worst {} MiB", worst / MIB)), "{}", r.status());
}
