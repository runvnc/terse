//! A relay's limits (NETWORK 3.1, 5.3): the bytes of a feed, blobs no
//! entry names, rates per address with a fake clock, the write list, the
//! proxy's header, keepalive pings, a graceful close, and what a client is
//! told.

use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_net::blob::to_hex;
use terse_net::protocol::{self, Refusal, Reply, status};
use terse_net::relay::{DEFAULT_TIMEOUT, RelayClient};
use terse_net::{BlobStore, Delivery, Done, Draft, Entry, Identity, Node, blob_id};
use terse_relay::limits::{Cidr, Group, Rate, Rates};
use terse_relay::{Answer, Connection, Policy, Reason, Relay};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

const HOUR: Duration = Duration::from_secs(3600);

fn relay(dir: &Path, policy: Policy) -> Arc<Relay> {
    Arc::new(Relay::with_policy(BlobStore::open(dir, None).unwrap(), policy).unwrap())
}

/// Blobs of `n` bytes no entry names wait an hour; nothing else is limited.
fn expiring(feed_bytes: Option<u64>) -> Policy {
    Policy { feed_bytes, unreferenced_ttl: Some(HOUR), ..Policy::unlimited(1024) }
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

/// The blob, if the relay serves it.
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

fn article(id: &Identity, seq: u32, data: &[u8]) -> Entry {
    Entry::article(id.feed_key(), seq, 1_800_000_000 + seq, data, None, &format!("a{seq}"))
}

fn on_disk(relay: &Relay, data: &[u8]) -> bool {
    relay.blobs().path(&blob_id(data)).exists()
}

fn group(ip: &str) -> Group {
    Group::of(ip.parse().unwrap())
}

/// The quota is reached exactly, then exceeded: the oldest entries go,
/// never the latest profile; one entry over the limit by itself, and one
/// older than all kept, are refused with "feed full".
#[test]
fn feed_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), Policy { feed_bytes: Some(40), ..Policy::unlimited(1024) });
    let c = r.connect();
    let id = Identity::from_seed([1; 32]);
    let feed = id.feed_key();
    let profile = Entry::profile(feed, 1, 1, b"listed: yes\n", "ana");
    assert_eq!(publish(&r, &c, &profile, &id), status::DONE);
    for seq in 2..=4 {
        assert_eq!(publish(&r, &c, &article(&id, seq, &[seq as u8; 10]), &id), status::DONE);
    }
    // 12 + 3 * 10 = 42 > 40: the oldest article went, not the profile.
    assert_eq!(r.feeds().seqs(&feed).unwrap(), [1, 3, 4]);
    assert_eq!(r.feed_bytes(&feed), Some(32));
    // Exactly at the limit: nothing is dropped.
    assert_eq!(publish(&r, &c, &article(&id, 5, &[5; 8]), &id), status::DONE);
    assert_eq!((r.feeds().seqs(&feed).unwrap(), r.feed_bytes(&feed)), (vec![1, 3, 4, 5], Some(40)));
    assert_eq!(publish(&r, &c, &article(&id, 6, &[6; 41]), &id), status::FEED_FULL, "over the limit by itself");
    assert_eq!(publish(&r, &c, &article(&id, 7, &[7; 29]), &id), status::FEED_FULL, "no room beside the profile");
    assert_eq!(publish(&r, &c, &article(&id, 2, &[2; 10]), &id), status::FEED_FULL, "older than all kept");
    assert_eq!(publish(&r, &c, &article(&id, 6, &[6; 28]), &id), status::DONE, "28 + 12: all articles but this go");
    assert_eq!(r.feeds().seqs(&feed).unwrap(), [1, 6]);
    assert_eq!(r.refused(Reason::FeedFull), 3);
    // A restart counts the same bytes.
    drop(c);
    let r = relay(dir.path(), Policy { feed_bytes: Some(40), ..Policy::unlimited(1024) });
    assert_eq!(r.feed_bytes(&feed), Some(40));
}

/// The entry cap and the byte limit together: the entry cap still drops
/// the oldest, and with an index the feed's bytes follow.
#[test]
fn entry_cap_with_index() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), Policy { feed_entries: 3, ..expiring(Some(1000)) });
    let c = r.connect();
    let id = Identity::from_seed([2; 32]);
    for seq in 1..=5 {
        assert_eq!(put(&r, &c, &[seq as u8; 5]), status::DONE);
        assert_eq!(publish(&r, &c, &article(&id, seq, &[seq as u8; 5]), &id), status::DONE);
    }
    assert_eq!(r.feeds().seqs(&id.feed_key()).unwrap(), [3, 4, 5]);
    assert_eq!(r.feed_bytes(&id.feed_key()), Some(15));
    // The dropped entries' blobs wait, and go after the time.
    assert_eq!(r.waiting().1, 2);
    r.advance_clock(HOUR + Duration::from_secs(1));
    assert_eq!(r.collect(), 2);
    assert!(!on_disk(&r, &[1; 5]) && !on_disk(&r, &[2; 5]) && on_disk(&r, &[3; 5]));
}

/// A blob no entry names: accepted, not served, removed after the time;
/// named by an entry in time, it is held for good. A second put does not
/// add a second charge, and moves the time on.
#[test]
fn unreferenced_blobs_expire() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), expiring(None));
    let c = r.connect();
    let id = Identity::from_seed([3; 32]);
    assert_eq!(put(&r, &c, b"named later"), status::DONE);
    assert_eq!(put(&r, &c, b"never named"), status::DONE);
    assert_eq!(r.waiting(), (22, 2));
    assert_eq!(get(&r, &c, b"never named"), None, "not served while no entry names it");
    assert_eq!(publish(&r, &c, &article(&id, 1, b"named later"), &id), status::DONE);
    assert_eq!(get(&r, &c, b"named later").as_deref(), Some(&b"named later"[..]));
    assert_eq!(r.waiting(), (11, 1));
    // An entry whose size is not the blob's does not hold it (NETWORK 4.4).
    let mut wrong = article(&id, 2, b"never named");
    wrong.size += 1;
    assert_eq!(publish(&r, &c, &wrong, &id), status::DONE);
    assert_eq!(r.waiting(), (11, 1));
    r.advance_clock(Duration::from_secs(1800));
    assert_eq!(put(&r, &c, b"never named"), status::DONE, "a second put moves the time on");
    assert_eq!(r.waiting(), (11, 1));
    r.advance_clock(Duration::from_secs(1801));
    assert_eq!(r.collect(), 0);
    assert!(on_disk(&r, b"never named"));
    r.advance_clock(Duration::from_secs(1800));
    assert_eq!(r.collect(), 1);
    assert!(!on_disk(&r, b"never named") && on_disk(&r, b"named later"));
    assert_eq!(r.waiting(), (0, 0));
    assert!(r.status().contains("collected 1 blobs (11 bytes)"), "{}", r.status());
    // The entry came first, the blob later: held at once.
    assert_eq!(publish(&r, &c, &article(&id, 3, b"blob after entry"), &id), status::DONE);
    assert_eq!(put(&r, &c, b"blob after entry"), status::DONE);
    assert_eq!(r.waiting(), (0, 0));
    assert!(get(&r, &c, b"blob after entry").is_some());
}

/// A blob two feeds name counts for each, and is held until neither does;
/// then it waits and goes.
#[test]
fn shared_blob() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), expiring(Some(30)));
    let c = r.connect();
    let (a, b) = (Identity::from_seed([4; 32]), Identity::from_seed([5; 32]));
    let shared = [7u8; 20];
    assert_eq!(put(&r, &c, &shared), status::DONE);
    assert_eq!(publish(&r, &c, &article(&a, 1, &shared), &a), status::DONE);
    assert_eq!(publish(&r, &c, &article(&b, 1, &shared), &b), status::DONE);
    assert_eq!((r.feed_bytes(&a.feed_key()), r.feed_bytes(&b.feed_key())), (Some(20), Some(20)));
    // A's entry is dropped for its limit: B still names the blob.
    assert_eq!(put(&r, &c, &[8; 20]), status::DONE);
    assert_eq!(publish(&r, &c, &article(&a, 2, &[8; 20]), &a), status::DONE);
    assert_eq!(r.feeds().seqs(&a.feed_key()).unwrap(), [2]);
    assert_eq!(r.waiting().1, 0);
    assert!(get(&r, &c, &shared).is_some());
    // Then B's: the blob waits, is not served, and goes after the time.
    assert_eq!(put(&r, &c, &[9; 20]), status::DONE);
    assert_eq!(publish(&r, &c, &article(&b, 2, &[9; 20]), &b), status::DONE);
    assert_eq!(r.waiting(), (20, 1));
    assert_eq!(get(&r, &c, &shared), None);
    r.advance_clock(HOUR);
    assert_eq!(r.collect(), 1);
    assert!(!on_disk(&r, &shared));
}

/// The waiting room's limits: in all and per address; a refusal says when
/// to try again.
#[test]
fn waiting_room_limits() {
    let dir = tempfile::tempdir().unwrap();
    let policy = Policy { unreferenced_bytes: 5 * 4096, unreferenced_bytes_per_address: 2 * 4096, ..expiring(None) };
    let r = relay(dir.path(), policy);
    let (one, two, three) = (r.connect_from(group("192.0.2.1")), r.connect_from(group("192.0.2.2")), r.connect_from(group("192.0.2.3")));
    assert_eq!(put(&r, &one, b"a"), status::DONE);
    assert_eq!(put(&r, &one, b"b"), status::DONE);
    let refused = put(&r, &one, b"c");
    assert_eq!(Refusal::of_status(refused), Refusal::TryLater(HOUR), "until its first blob goes");
    assert_eq!(put(&r, &one, b"a"), status::DONE, "one already waiting costs nothing");
    assert_eq!(put(&r, &two, b"c"), status::DONE);
    assert_eq!(put(&r, &two, b"d"), status::DONE);
    assert_eq!(put(&r, &three, b"e"), status::DONE);
    assert!(matches!(Refusal::of_status(put(&r, &three, b"f")), Refusal::TryLater(_)), "the room is full");
    // In-process connections are counted in the room, not per address.
    let local = r.connect();
    assert!(matches!(Refusal::of_status(put(&r, &local, b"f")), Refusal::TryLater(_)));
    assert_eq!(r.refused(Reason::WaitingRoom), 3);
    r.advance_clock(HOUR);
    r.collect();
    assert_eq!(put(&r, &three, b"f"), status::DONE);
}

/// With a write list, an address none of the listed feeds published from
/// has a small share of the room until one does; entries of other feeds
/// are refused; the file is read again when it changes.
#[test]
fn write_list() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("writers");
    let (ana, bo) = (Identity::from_seed([6; 32]), Identity::from_seed([7; 32]));
    std::fs::write(&list, format!("# the writers\n{} ana\n", to_hex(&ana.feed_key()))).unwrap();
    let policy = Policy { writers: Some(list.clone()), stranger_bytes_per_address: 4096, ..expiring(None) };
    let r = relay(&dir.path().join("store"), policy);
    let c = r.connect_from(group("198.51.100.1"));
    assert_eq!(put(&r, &c, &[1; 100]), status::DONE);
    assert!(matches!(Refusal::of_status(put(&r, &c, &[2; 100])), Refusal::TryLater(_)), "a stranger's share");
    assert_eq!(publish(&r, &c, &article(&bo, 1, &[1; 100]), &bo), status::NOT_WRITER);
    assert_eq!(publish(&r, &c, &article(&ana, 1, &[1; 100]), &ana), status::DONE);
    // Now known as a writer's address: the whole allowance.
    for n in 2..10u8 {
        assert_eq!(put(&r, &c, &[n; 100]), status::DONE);
    }
    assert_eq!(r.refused(Reason::NotWriter), 1);
    // The file changes: read again (the maintenance thread does it every
    // 2 s; here by hand); a bad file keeps the list in force.
    std::fs::write(&list, format!("{}\n{}\n", to_hex(&ana.feed_key()), to_hex(&bo.feed_key()))).unwrap();
    r.reload_writers(false);
    assert_eq!(publish(&r, &c, &article(&bo, 1, &[1; 100]), &bo), status::DONE);
    std::fs::write(&list, "not a key\n").unwrap();
    r.reload_writers(false);
    assert_eq!(publish(&r, &c, &article(&bo, 2, &[2; 100]), &bo), status::DONE);
    // Reading stays open to all.
    let reader = r.connect_from(group("203.0.113.9"));
    assert!(get(&r, &reader, &[1; 100]).is_some());
}

fn rates(uploads: f64, bytes: f64, new_feeds: f64, finds: f64) -> Rates {
    let m = Duration::from_secs(60);
    Rates { uploads: Rate::new(uploads, m), upload_bytes: Rate::new(bytes, m), new_feeds: Rate::new(new_feeds, m), finds: Rate::new(finds, m) }
}

/// Rates per address, with the relay's clock moved by hand; an IPv6
/// client is one /64.
#[test]
fn rate_limits() {
    let dir = tempfile::tempdir().unwrap();
    let policy = Policy { rates: Some(rates(3.0, 2_000_000.0, 2.0, 100.0)), ..Policy::unlimited(1024) };
    let r = relay(dir.path(), policy);
    let c = r.connect_from(group("2001:db8:1:2::1"));
    let same64 = r.connect_from(group("2001:db8:1:2:ffff::9"));
    let other64 = r.connect_from(group("2001:db8:1:3::1"));
    assert_eq!(put(&r, &c, b"1"), status::DONE);
    assert_eq!(put(&r, &same64, b"2"), status::DONE);
    assert_eq!(put(&r, &c, b"3"), status::DONE);
    assert_eq!(Refusal::of_status(put(&r, &same64, b"4")), Refusal::TryLater(Duration::from_secs(20)), "3 a minute: one in 20 s");
    assert_eq!(put(&r, &other64, b"4"), status::DONE);
    r.advance_clock(Duration::from_secs(20));
    assert_eq!(put(&r, &c, b"4"), status::DONE);
    assert_eq!(r.refused(Reason::Uploads), 1);
    // Bytes: 2,000,000 a minute.
    r.advance_clock(Duration::from_secs(60));
    assert_eq!(put(&r, &c, &vec![1; 1_000_000]), status::DONE);
    assert_eq!(put(&r, &c, &vec![2; 1_000_000]), status::DONE);
    assert!(matches!(Refusal::of_status(put(&r, &c, b"x")), Refusal::TryLater(_)));
    assert_eq!(r.refused(Reason::UploadBytes), 1);
    // New feeds: 2 a minute per address; a feed already held is not new.
    r.advance_clock(Duration::from_secs(60));
    let ids: Vec<Identity> = (10..14).map(|n| Identity::from_seed([n; 32])).collect();
    assert_eq!(publish(&r, &other64, &article(&ids[0], 1, b"a"), &ids[0]), status::DONE);
    assert_eq!(publish(&r, &other64, &article(&ids[1], 1, b"a"), &ids[1]), status::DONE);
    assert_eq!(publish(&r, &other64, &article(&ids[0], 2, b"b"), &ids[0]), status::DONE);
    assert!(matches!(Refusal::of_status(publish(&r, &other64, &article(&ids[2], 1, b"a"), &ids[2])), Refusal::TryLater(_)));
    assert_eq!(r.refused(Reason::NewFeeds), 1);
    // In-process and outgoing peer connections are not limited.
    let local = r.connect();
    for n in 0..20u8 {
        assert_eq!(put(&r, &local, &[n, 1]), status::DONE);
    }
}

/// A get of a blob while it is dropped and collected answers with the
/// whole blob or "not found", never a part.
#[test]
fn expiry_during_gets() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), expiring(Some(64 * 1024)));
    let id = Identity::from_seed([20; 32]);
    let blob = |seq: u32| vec![seq as u8; 32 * 1024];
    let c = r.connect();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let (r, stop) = (r.clone(), stop.clone());
        std::thread::spawn(move || {
            let c = r.connect();
            let (mut whole, mut missing) = (0, 0);
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                for seq in 1..=40u32 {
                    match get(&r, &c, &blob(seq)) {
                        Some(d) => {
                            assert_eq!(d, blob(seq));
                            whole += 1;
                        }
                        None => missing += 1,
                    }
                }
            }
            (whole, missing)
        })
    };
    for seq in 1..=40u32 {
        assert_eq!(put(&r, &c, &blob(seq)), status::DONE);
        assert_eq!(publish(&r, &c, &article(&id, seq, &blob(seq)), &id), status::DONE);
        r.advance_clock(HOUR);
        r.collect();
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let (whole, missing) = reader.join().unwrap();
    assert!(whole > 0 && missing > 0, "{whole} {missing}");
    assert_eq!(r.feeds().seqs(&id.feed_key()).unwrap(), [39, 40]);
    assert_eq!(r.waiting().1, 0);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().filter(|e| e.as_ref().unwrap().file_type().unwrap().is_file()).count(), 2, "the two blobs held");
}

/// A restart: the index is rebuilt from the disk; a waiting blob keeps its
/// time from its file; a blob whose entry was removed by a crash in the
/// middle waits; a temporary file a crash left is removed.
#[test]
fn restart() {
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::from_seed([21; 32]);
    {
        let r = relay(dir.path(), expiring(Some(1000)));
        let c = r.connect();
        assert_eq!(put(&r, &c, b"held"), status::DONE);
        assert_eq!(publish(&r, &c, &article(&id, 1, b"held"), &id), status::DONE);
        assert_eq!(put(&r, &c, b"orphan"), status::DONE);
        assert_eq!(put(&r, &c, b"entry lost"), status::DONE);
        assert_eq!(publish(&r, &c, &article(&id, 2, b"entry lost"), &id), status::DONE);
    }
    // Killed after removing an entry, before its blob was put to wait.
    r_remove_entry(dir.path(), &id.feed_key(), 2);
    std::fs::write(dir.path().join(".partial.1.2.tmp"), b"half").unwrap();
    let r = relay(dir.path(), expiring(Some(1000)));
    assert!(!dir.path().join(".partial.1.2.tmp").exists());
    assert_eq!(r.waiting(), (16, 2));
    assert_eq!(r.collect(), 0, "their time is not up");
    r.advance_clock(HOUR + Duration::from_secs(5));
    assert_eq!(r.collect(), 2);
    assert!(on_disk(&r, b"held") && !on_disk(&r, b"orphan") && !on_disk(&r, b"entry lost"));
    // Without expiry, nothing is removed.
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), Policy::unlimited(1024));
    let c = r.connect();
    assert_eq!(put(&r, &c, b"kept"), status::DONE);
    r.advance_clock(HOUR * 100);
    assert_eq!(r.collect(), 0);
    assert!(get(&r, &c, b"kept").is_some(), "served though no entry names it");
}

fn r_remove_entry(dir: &Path, feed: &[u8; 32], seq: u32) {
    std::fs::remove_file(dir.join("feeds").join(to_hex(feed)).join(format!("{seq:010}"))).unwrap();
}

struct Served {
    url: String,
    relay: Arc<Relay>,
    _dir: tempfile::TempDir,
}

fn serve(policy: Policy) -> Served {
    let dir = tempfile::tempdir().unwrap();
    let relay = relay(dir.path(), policy);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let r = relay.clone();
    std::thread::spawn(move || terse_relay::serve(listener, r));
    Served { url, relay, _dir: dir }
}

/// A WebSocket with these headers; the HTTP status if the handshake is
/// refused.
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

/// Connections per address, by the proxy's header only from a trusted
/// proxy; an IPv6 client is one /64.
#[test]
fn proxy_header_and_connections() {
    let cf = "cf-connecting-ip";
    let trusted = serve(Policy {
        connections_per_address: Some(1),
        trust_proxy: vec![Cidr::parse("127.0.0.0/8").unwrap()],
        ..Policy::unlimited(1024)
    });
    let a = ws_with(&trusted.url, &[(cf, "198.51.100.1")]).unwrap();
    let b = ws_with(&trusted.url, &[(cf, "198.51.100.2")]).unwrap();
    assert_eq!(ws_with(&trusted.url, &[(cf, "198.51.100.1")]).err(), Some(429));
    assert_eq!(ws_with(&trusted.url, &[("x-forwarded-for", "203.0.113.5, 198.51.100.1")]).err(), Some(429));
    let v6 = ws_with(&trusted.url, &[(cf, "2001:db8:5:6::1")]).unwrap();
    assert_eq!(ws_with(&trusted.url, &[(cf, "2001:db8:5:6:7::2")]).err(), Some(429), "the same /64");
    let _other64 = ws_with(&trusted.url, &[(cf, "2001:db8:5:7::1")]).unwrap();
    assert_eq!(trusted.relay.refused(Reason::Connections), 3);
    // A closed connection frees its place.
    drop(a);
    let deadline = Instant::now() + Duration::from_secs(10);
    while ws_with(&trusted.url, &[(cf, "198.51.100.1")]).is_err() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    drop((b, v6));
    // Not a trusted proxy: the header is ignored, every connection is
    // 127.0.0.1.
    let open = serve(Policy { connections_per_address: Some(1), ..Policy::unlimited(1024) });
    let _a = ws_with(&open.url, &[(cf, "198.51.100.1")]).unwrap();
    assert_eq!(ws_with(&open.url, &[(cf, "198.51.100.2")]).err(), Some(429));
    // The relay's total.
    let few = serve(Policy { max_connections: Some(1), ..Policy::unlimited(1024) });
    let _a = ws_with(&few.url, &[]).unwrap();
    assert_eq!(ws_with(&few.url, &[]).err(), Some(503));
}

/// A silent connection gets pings; a client that answers them stays
/// connected; a graceful close closes every connection.
#[test]
fn pings_and_close() {
    let s = serve(Policy { ping_every: Some(Duration::from_millis(100)), ..Policy::unlimited(1024) });
    let mut ws = ws_with(&s.url, &[]).unwrap();
    let start = Instant::now();
    let mut pings = 0;
    while start.elapsed() < Duration::from_millis(550) {
        if let Ok(Message::Ping(_)) = ws.read() {
            pings += 1;
        }
    }
    assert!((3..=6).contains(&pings), "{pings} pings");
    // The ordinary client answers them and goes on working.
    let mut client = RelayClient::websocket(&s.url, DEFAULT_TIMEOUT);
    client.connect().unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let req = client.put(b"after pings");
    let mut got = Vec::new();
    while got.is_empty() {
        got = client.poll();
    }
    assert_eq!(got, [(req, terse_net::relay::RelayReply::Put(blob_id(b"after pings")))]);
    s.relay.close();
    loop {
        match ws.read() {
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => break,
            Ok(_) => {}
            Err(e) => panic!("{e}"),
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while s.relay.connection_count() > 0 {
        assert!(Instant::now() < deadline, "{} connections left", s.relay.connection_count());
        client.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A publishing client is told each relay's reason (NETWORK 3.1): the
/// entry's, and the blob's when the relay took the entry but not the blob.
#[test]
fn client_is_told_why() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("writers");
    let me = Identity::from_seed([30; 32]);
    std::fs::write(&list, "").unwrap();
    let closed = serve(Policy { writers: Some(list.clone()), ..Policy::unlimited(1024) });
    let small = serve(Policy { feed_bytes: Some(10), ..Policy::unlimited(1024) });
    let open = serve(Policy::unlimited(1024));
    let mut node = Node::open(Some(dir.path().join("client")), &[closed.url.clone(), small.url.clone(), open.url.clone()], DEFAULT_TIMEOUT).unwrap();
    node.set_identity(me);
    node.set_deliver_to_all(true);
    let done = node.publish_article(&Draft { data: b"twenty bytes of text".to_vec(), title: "hi".into(), reply_to: None, time: 1 });
    assert!(matches!(done, Done::Published { seq: 1, .. }), "{done:?}");
    let delivery: std::collections::HashMap<String, Delivery> = node.take_delivery().into_iter().collect();
    assert_eq!(delivery[&closed.url], Delivery::Refused(Refusal::NotWriter));
    assert_eq!(delivery[&small.url], Delivery::Refused(Refusal::FeedFull));
    assert_eq!(delivery[&open.url], Delivery::Accepted);
    assert_eq!(Refusal::NotWriter.describe(), "this feed is not on the relay's list of writers");

    // A relay that refuses the blob (its waiting room is full) but takes
    // the entry.
    let room = serve(Policy { unreferenced_bytes: 0, ..expiring(None) });
    let mut node = Node::open(Some(dir.path().join("client2")), &[room.url.clone(), open.url.clone()], DEFAULT_TIMEOUT).unwrap();
    node.set_identity(Identity::from_seed([31; 32]));
    node.set_deliver_to_all(true);
    let done = node.publish_article(&Draft { data: b"another article".to_vec(), title: "hi".into(), reply_to: None, time: 1 });
    assert!(matches!(done, Done::Published { .. }), "{done:?}");
    let delivery: std::collections::HashMap<String, Delivery> = node.take_delivery().into_iter().collect();
    assert!(matches!(delivery[&room.url], Delivery::BlobRefused(Refusal::TryLater(_))), "{delivery:?}");
    // Every relay refuses the blob: the publish fails and says why.
    let mut node = Node::open(Some(dir.path().join("client3")), std::slice::from_ref(&room.url), DEFAULT_TIMEOUT).unwrap();
    node.set_identity(Identity::from_seed([32; 32]));
    node.set_deliver_to_all(true);
    let done = node.publish_article(&Draft { data: b"a third".to_vec(), title: "hi".into(), reply_to: None, time: 1 });
    assert_eq!(done, Done::Failed);
    assert!(matches!(node.take_delivery()[..], [(_, Delivery::Refused(Refusal::TryLater(_)))]));
}

/// Mirrors: a write list applies to what a peer pushes; a mirror keeps
/// what its peer later drops (each relay's limits are its own).
#[test]
fn mirrors() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("writers");
    let (ana, bo) = (Identity::from_seed([40; 32]), Identity::from_seed([41; 32]));
    std::fs::write(&list, to_hex(&ana.feed_key())).unwrap();
    let a = serve(Policy { feed_bytes: Some(20), ..Policy::unlimited(1024) });
    let b = serve(Policy { writers: Some(list), ..Policy::unlimited(1024) });
    terse_relay::connect_peers(b.relay.clone(), vec![a.url.clone()]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while a.relay.peer_count() == 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let c = a.relay.connect();
    for seq in 1..=3 {
        assert_eq!(put(&a.relay, &c, &[seq as u8; 10]), status::DONE);
        assert_eq!(publish(&a.relay, &c, &article(&ana, seq, &[seq as u8; 10]), &ana), status::DONE);
    }
    assert_eq!(publish(&a.relay, &c, &article(&bo, 1, b"bo"), &bo), status::DONE);
    let deadline = Instant::now() + Duration::from_secs(10);
    while b.relay.feeds().seqs(&ana.feed_key()).unwrap().len() < 3 {
        assert!(Instant::now() < deadline, "{:?}", b.relay.feeds().seqs(&ana.feed_key()));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(a.relay.feeds().seqs(&ana.feed_key()).unwrap(), [2, 3], "the peer dropped the first");
    std::thread::sleep(Duration::from_millis(200));
    assert!(b.relay.feeds().seqs(&bo.feed_key()).unwrap().is_empty(), "not on the mirror's list");
    assert!(b.relay.blobs().read(&blob_id(&[1; 10])).unwrap().is_some(), "the mirror keeps it");
    let _ = IpAddr::from([127, 0, 0, 1]);
}

/// The status line counts what happened.
#[test]
fn status_line() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), Policy { feed_bytes: Some(5), ..expiring(None) });
    let c = r.connect();
    let id = Identity::from_seed([50; 32]);
    assert_eq!(put(&r, &c, b"waiting"), status::DONE);
    assert_eq!(publish(&r, &c, &article(&id, 1, b"too big"), &id), status::FEED_FULL);
    let s = r.status();
    assert!(s.contains("unreferenced 7 bytes in 1 blobs"), "{s}");
    assert!(s.contains("refused: feed-full 1"), "{s}");
}
