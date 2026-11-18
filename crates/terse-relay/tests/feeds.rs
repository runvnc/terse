//! Feeds on the relay over real loopback WebSockets (NETWORK 4.3).

use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use terse_net::protocol::{self, PUBLISH_REPLY, SUBSCRIBE_REPLY, UNSUBSCRIBE_REPLY, status};
use terse_net::{BlobStore, Entry, Identity};
use terse_relay::Relay;
use tungstenite::{Message, WebSocket};

type Ws = WebSocket<TcpStream>;

/// Starts a relay on `dir`; it lives until the test process exits.
fn start(dir: &Path, cap: usize) -> String {
    let relay = Arc::new(Relay::new(BlobStore::open(dir, None).unwrap(), cap));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, relay));
    url
}

fn connect(url: &str) -> Ws {
    let stream = TcpStream::connect(url.strip_prefix("ws://").unwrap()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tungstenite::client::client_with_config(url, stream, Some(terse_net::relay::websocket_config())).unwrap().0
}

fn recv(ws: &mut Ws) -> Vec<u8> {
    loop {
        match ws.read().unwrap() {
            Message::Binary(b) => return b.to_vec(),
            Message::Ping(_) | Message::Pong(_) => {}
            m => panic!("unexpected {m:?}"),
        }
    }
}

/// Nothing arrives within a short wait.
fn quiet(ws: &mut Ws) -> bool {
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    let quiet = match ws.read() {
        Err(tungstenite::Error::Io(e)) => matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut),
        other => panic!("expected nothing, got {other:?}"),
    };
    ws.get_ref().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    quiet
}

fn ask(ws: &mut Ws, message: Vec<u8>) -> Vec<u8> {
    ws.send(Message::binary(message)).unwrap();
    recv(ws)
}

fn entry(id: &Identity, seq: u32, title: &str) -> Vec<u8> {
    Entry::article(id.feed_key(), seq, 1_700_000_000 + seq, title.as_bytes(), None, title).sign(id).unwrap().bytes
}

fn publish(ws: &mut Ws, req: u32, entry: &[u8]) -> u8 {
    let reply = ask(ws, protocol::encode_publish(req, entry));
    assert_eq!(reply[..5], protocol::encode_status_reply(PUBLISH_REPLY, req, 0)[..5]);
    reply[5]
}

fn subscribe(ws: &mut Ws, req: u32, id: &Identity, after: u32) -> u8 {
    let reply = ask(ws, protocol::encode_subscribe(req, &id.feed_key(), after));
    assert_eq!(reply.len(), 6);
    assert_eq!(reply[..5], protocol::encode_status_reply(SUBSCRIBE_REPLY, req, 0)[..5]);
    reply[5]
}

fn pushed(ws: &mut Ws) -> Vec<u8> {
    let m = recv(ws);
    assert_eq!(m[0], protocol::ENTRY, "{m:?}");
    m[1..].to_vec()
}

#[test]
fn publish_subscribe_backlog_and_live_push() {
    let dir = tempfile::tempdir().unwrap();
    let url = start(dir.path(), 1024);
    let author = Identity::from_seed([1; 32]);
    let entries: Vec<Vec<u8>> = (1..=4).map(|s| entry(&author, s, &format!("article {s}"))).collect();

    let mut a = connect(&url);
    // Published out of order; the backlog comes in increasing seq order.
    for (req, i) in [(1, 2), (2, 0), (3, 3), (4, 1)] {
        assert_eq!(publish(&mut a, req, &entries[i]), status::DONE);
    }
    let mut b = connect(&url);
    assert_eq!(subscribe(&mut b, 7, &author, 0), status::DONE);
    for e in &entries {
        assert_eq!(&pushed(&mut b), e);
    }
    // `after` filters the backlog.
    let mut c = connect(&url);
    assert_eq!(subscribe(&mut c, 8, &author, 2), status::DONE);
    assert_eq!(pushed(&mut c), entries[2]);
    assert_eq!(pushed(&mut c), entries[3]);
    assert!(quiet(&mut c));

    // A new entry is pushed to both subscribers, once.
    let fifth = entry(&author, 5, "five");
    assert_eq!(publish(&mut a, 9, &fifth), status::DONE);
    assert_eq!(pushed(&mut b), fifth);
    assert_eq!(pushed(&mut c), fifth);
    // Publishing it again succeeds and pushes nothing.
    assert_eq!(publish(&mut a, 10, &fifth), status::DONE);
    assert!(quiet(&mut b));

    // After unsubscribing, c gets nothing more.
    let reply = ask(&mut c, protocol::encode_unsubscribe(11, &author.feed_key()));
    assert_eq!(reply, protocol::encode_status_reply(UNSUBSCRIBE_REPLY, 11, status::DONE));
    let sixth = entry(&author, 6, "six");
    assert_eq!(publish(&mut a, 12, &sixth), status::DONE);
    assert_eq!(pushed(&mut b), sixth);
    assert!(quiet(&mut c));
    // The publisher was not subscribed: nothing pushed to it.
    assert!(quiet(&mut a));

    // A dead subscriber (never reads, then vanishes) does not hold anyone up.
    let mut dead = connect(&url);
    assert_eq!(subscribe(&mut dead, 1, &author, 6), status::DONE);
    drop(dead);
    assert_eq!(publish(&mut a, 13, &entry(&author, 7, "seven")), status::DONE);
    assert_eq!(pushed(&mut b)[32..36], 7u32.to_le_bytes());
}

#[test]
fn refuses_invalid_and_conflicting_entries() {
    let dir = tempfile::tempdir().unwrap();
    let url = start(dir.path(), 1024);
    let author = Identity::from_seed([2; 32]);
    let mut ws = connect(&url);

    let mut forged = entry(&author, 1, "forged");
    forged[50] ^= 1;
    assert_eq!(publish(&mut ws, 1, &forged), status::REFUSED, "bad signature");
    assert_eq!(publish(&mut ws, 2, b"short"), status::REFUSED, "malformed");
    let good = entry(&author, 1, "first");
    assert_eq!(publish(&mut ws, 3, &good), status::DONE);
    assert_eq!(publish(&mut ws, 4, &good), status::DONE, "duplicate");
    assert_eq!(publish(&mut ws, 5, &entry(&author, 1, "rival")), status::REFUSED, "conflicting seq");

    let mut sub = connect(&url);
    assert_eq!(subscribe(&mut sub, 1, &author, 0), status::DONE);
    assert_eq!(pushed(&mut sub), good);
    assert!(quiet(&mut sub));
    // Malformed and invalid subscriptions are refused.
    assert_eq!(ask(&mut sub, protocol::encode_subscribe(2, &[0; 32], 0)[..40].to_vec()), [SUBSCRIBE_REPLY, 2, 0, 0, 0, 3]);
    assert_eq!(ask(&mut sub, protocol::encode_subscribe(3, &[0; 32], 0)), [SUBSCRIBE_REPLY, 3, 0, 0, 0, 3]);
}

#[test]
fn keeps_the_most_recent_entries_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let cap = terse_relay::MIN_FEED_CAP;
    let url = start(dir.path(), cap);
    let author = Identity::from_seed([3; 32]);
    let mut ws = connect(&url);
    // A subscriber that never reads does not slow publishing or the
    // subscriber that does.
    let mut slow = connect(&url);
    assert_eq!(subscribe(&mut slow, 1, &author, 0), status::DONE);
    let mut reader = connect(&url);
    assert_eq!(subscribe(&mut reader, 1, &author, 0), status::DONE);
    let total = cap as u32 + 10;
    for seq in 1..=total {
        assert_eq!(publish(&mut ws, seq, &entry(&author, seq, "x")), status::DONE);
        assert_eq!(pushed(&mut reader)[32..36], seq.to_le_bytes());
    }
    let feeds = terse_net::FeedStore::beside_blobs(dir.path());
    let held = feeds.seqs(&author.feed_key()).unwrap();
    assert_eq!(held.len(), cap);
    assert_eq!(held[0], 11);
    // An entry older than everything kept is not stored.
    assert_eq!(publish(&mut ws, 999, &entry(&author, 3, "late")), status::REFUSED);

    // A second relay on the same directory (a restart) serves the same feed.
    let restarted = start(dir.path(), cap);
    let mut sub = connect(&restarted);
    assert_eq!(subscribe(&mut sub, 1, &author, total - 2), status::DONE);
    assert_eq!(pushed(&mut sub), entry(&author, total - 1, "x"));
    assert_eq!(pushed(&mut sub), entry(&author, total, "x"));
    assert!(quiet(&mut sub));
    let mut ws = connect(&restarted);
    assert_eq!(publish(&mut ws, 1, &entry(&author, total, "different")), status::REFUSED, "seq held after restart");
}
