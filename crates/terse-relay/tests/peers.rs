//! Relays that mirror each other (NETWORK 5), over real loopback
//! WebSockets: two peered relays, three in a line, and a triangle of three
//! fully peered relays where one publish must not set off a storm.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_net::protocol;
use terse_net::relay::{DEFAULT_TIMEOUT, RelayClient};
use terse_net::{Article, BlobStore, ContentService, Done, Draft, Entry, FeedEvent, Identity, Node, Subscriptions, blob_id};
use terse_relay::Relay;

struct Started {
    url: String,
    relay: Arc<Relay>,
    _dir: tempfile::TempDir,
}

/// A relay on an ephemeral port, linked to `peers`.
fn start(peers: &[&str]) -> Started {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(Relay::new(BlobStore::open(dir.path(), None).unwrap(), 1024));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let r = relay.clone();
    std::thread::spawn(move || terse_relay::serve(listener, r));
    terse_relay::connect_peers(relay.clone(), peers.iter().map(|s| s.to_string()).collect());
    Started { url, relay, _dir: dir }
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn node(dir: &std::path::Path, url: &str) -> Node {
    Node::new(BlobStore::open(dir, None).unwrap(), Some(RelayClient::websocket(url, DEFAULT_TIMEOUT)))
}

fn draft(title: &str, data: Vec<u8>) -> Draft {
    Draft { data, title: title.into(), reply_to: None, time: 1_800_000_000 }
}

/// A subscriber that follows `feed` on `url` only.
fn follower(dir: &std::path::Path, url: &str, feed: [u8; 32]) -> ContentService {
    let mut n = node(dir, url);
    n.set_subscriptions(Subscriptions::in_memory([feed]));
    ContentService::start(n)
}

fn next_entry(service: &mut ContentService) -> (u32, String, Article) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match service.wait_feed_event(deadline.saturating_duration_since(Instant::now())) {
            Some(FeedEvent::Entry { entry, article }) => return (entry.seq, entry.title, article),
            Some(_) => {}
            None => panic!("no entry arrived"),
        }
    }
}

#[test]
fn two_peered_relays() {
    let a = start(&[]);
    let b = start(&[&a.url]);
    wait_until("the peer link", || a.relay.peer_count() == 1 && b.relay.peer_count() == 1);
    let author = Identity::from_seed([40; 32]);
    let (author_dir, reader_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());

    // An article published at A before anyone follows it on B.
    let mut writer = node(author_dir.path(), &a.url);
    writer.set_identity(author.clone());
    let small = b"A small article.".to_vec();
    assert!(matches!(writer.publish_article(&draft("Small", small.clone())), Done::Published { seq: 1, .. }));
    wait_until("the entry and blob on B", || {
        b.relay.feeds().seqs(&author.feed_key()).unwrap() == [1] && b.relay.blobs().read(&blob_id(&small)).unwrap().is_some()
    });

    // The reader is on B only: the backlog, then a live entry.
    let mut reader = follower(reader_dir.path(), &b.url, author.feed_key());
    assert_eq!(next_entry(&mut reader), (1, "Small".into(), Article::Local));
    let big: Vec<u8> = (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect();
    assert!(matches!(writer.publish_article(&draft("Big", big.clone())), Done::Published { seq: 2, .. }));
    assert_eq!(next_entry(&mut reader), (2, "Big".into(), Article::NotFetched));
    assert!(b.relay.blobs().read(&blob_id(&big)).unwrap().is_none(), "blobs over 16,384 bytes are not pushed");
    drop(reader);
    // Fetched through B, which asks A, stores it and serves it.
    let mut fetcher = node(reader_dir.path(), &b.url);
    assert_eq!(fetcher.fetch(blob_id(&big)), Done::Got(big.clone()));
    assert_eq!(b.relay.blobs().read(&blob_id(&big)).unwrap(), Some(big));
    // A blob neither has: not found, after asking A.
    assert_eq!(fetcher.fetch(blob_id(b"nowhere")), Done::NotFound);
}

#[test]
fn three_relays_in_a_line() {
    let a = start(&[]);
    let c = start(&[]);
    let b = start(&[&a.url, &c.url]);
    wait_until("the peer links", || a.relay.peer_count() == 1 && b.relay.peer_count() == 2 && c.relay.peer_count() == 1);
    let author = Identity::from_seed([41; 32]);
    let (author_dir, reader_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut reader = follower(reader_dir.path(), &c.url, author.feed_key());
    let mut writer = node(author_dir.path(), &a.url);
    writer.set_identity(author);
    assert!(matches!(writer.publish_article(&draft("Far", b"From A to C.".to_vec())), Done::Published { .. }));
    assert_eq!(next_entry(&mut reader), (1, "Far".into(), Article::Local));
}

#[test]
fn no_storm_in_a_triangle() {
    let a = start(&[]);
    let b = start(&[&a.url]);
    let c = start(&[&a.url, &b.url]);
    let relays = [&a, &b, &c];
    wait_until("the peer links", || relays.iter().all(|r| r.relay.peer_count() == 2));
    let count = |kind: u8| relays.iter().map(|r| r.relay.peer_messages(kind)).sum::<usize>();
    let kinds = [protocol::PUT, protocol::PUT_REPLY, protocol::PUBLISH, protocol::PUBLISH_REPLY, protocol::ENTRY];
    let before: Vec<usize> = kinds.iter().map(|&k| count(k)).collect();

    let author = Identity::from_seed([42; 32]);
    let data = b"One article.".to_vec();
    let entry = Entry::article(author.feed_key(), 1, 1, &data, None, "One").sign(&author).unwrap();
    let mut client = RelayClient::websocket(&a.url, DEFAULT_TIMEOUT);
    client.connect().unwrap();
    client.put(&data);
    client.publish(&entry.bytes);
    let mut replies = Vec::new();
    wait_until("the replies", || {
        replies.extend(client.poll());
        replies.len() == 2
    });
    wait_until("the entry everywhere", || relays.iter().all(|r| r.relay.feeds().seqs(&author.feed_key()).unwrap() == [1]));
    std::thread::sleep(Duration::from_millis(500));
    let sent: Vec<usize> = kinds.iter().zip(&before).map(|(&k, b)| count(k) - b).collect();
    eprintln!("peer messages for one put and one publish (put, put reply, publish, publish reply, entry): {sent:?}");
    // Each relay sends what it newly stored to the peers it did not get it
    // from: A to B and C; B and C each to the other; the second copy is a
    // duplicate and goes no further.
    assert_eq!(sent, [4, 4, 4, 4, 0]);
    assert!(replies.iter().all(|(_, r)| matches!(r, terse_net::relay::RelayReply::Put(_) | terse_net::relay::RelayReply::Done)));
}
