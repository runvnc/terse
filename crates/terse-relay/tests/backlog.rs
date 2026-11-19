//! A subscriber that arrives late gets the whole backlog (NETWORK 4.3,
//! 4.4): an author publishes five articles before the subscriber exists;
//! the subscriber's program then subscribes with op 05h (SPEC 9.5, consent
//! allowed), receives all five entries and downloads the articles.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_core::{Completion, Outcome, Request, RequestKind};
use terse_net::relay::{DEFAULT_TIMEOUT, RelayClient};
use terse_net::{Article, BlobStore, Consent, ContentService, Done, Draft, FeedEvent, Identity, Node, Subscriptions};
use terse_relay::{DEFAULT_FEED_CAP, Relay};

#[test]
fn late_subscriber_gets_the_backlog_and_the_articles() {
    let (relay_dir, author_dir, reader_dir, config) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let relay = Arc::new(Relay::new(BlobStore::open(relay_dir.path(), None).unwrap(), DEFAULT_FEED_CAP));
    std::thread::spawn(move || terse_relay::serve(listener, relay));

    // The author publishes five articles; nobody is subscribed yet.
    let author = Identity::from_seed([21; 32]);
    let mut node = Node::new(BlobStore::open(author_dir.path(), None).unwrap(), Some(RelayClient::websocket(&url, DEFAULT_TIMEOUT)));
    node.set_identity(author.clone());
    let articles: Vec<Vec<u8>> = (1..=5).map(|i| format!("Article {i}.\n\n{}\n", "Some text. ".repeat(i * 40)).into_bytes()).collect();
    for (i, data) in articles.iter().enumerate() {
        let draft = Draft { data: data.clone(), title: format!("Article {}", i + 1), reply_to: None, time: 1_800_000_000 + i as u32 };
        assert!(matches!(node.publish_article(&draft), Done::Published { seq, .. } if seq == i as u32 + 1));
    }
    drop(node);

    // The subscriber starts with nothing and its program subscribes.
    let mut node = Node::new(BlobStore::open(reader_dir.path(), None).unwrap(), Some(RelayClient::websocket(&url, DEFAULT_TIMEOUT)));
    node.set_subscriptions(Subscriptions::load(config.path().join("subscriptions")).unwrap());
    let mut service = ContentService::start_with_consent(node, Consent::Allow);
    service.submit(vec![Request { handle: 1, kind: RequestKind::Subscribe { feed: author.feed_key(), hints: Vec::new() } }]);
    assert_eq!(service.wait_finished(), [Completion { handle: 1, outcome: Outcome::Done }]);

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut got = Vec::new();
    while got.len() < 5 {
        match service.wait_feed_event(deadline.saturating_duration_since(Instant::now())) {
            Some(FeedEvent::Entry { entry, article }) => got.push((entry.seq, entry.title, article)),
            Some(_) => {}
            None => panic!("only {} entries arrived: {got:?}", got.len()),
        }
    }
    got.sort_by_key(|g| g.0);
    let want: Vec<(u32, String, Article)> = (1..=5).map(|i| (i, format!("Article {i}"), Article::Local)).collect();
    assert_eq!(got, want);
    // Every article is in the subscriber's store, so reading never waits.
    let store = BlobStore::open(reader_dir.path(), None).unwrap();
    for data in &articles {
        assert_eq!(store.read(&terse_net::blob_id(data)).unwrap().as_deref(), Some(&data[..]));
    }
    // And the program's timeline has all five, marked local, newest first.
    service.submit(vec![Request { handle: 2, kind: RequestKind::Timeline { offset: 0, length: 16384 } }]);
    let done = service.wait_finished();
    let Outcome::Records { total, data } = &done[0].outcome else { panic!("{done:?}") };
    assert_eq!(*total, 5);
    let (mut at, mut seqs) = (0, Vec::new());
    while at < data.len() {
        assert_eq!(data[at + 1], 1, "local");
        seqs.push(u32::from_le_bytes(data[at + 2 + 32..at + 2 + 36].try_into().unwrap()));
        at += 1 + data[at] as usize;
    }
    assert_eq!(seqs, [5, 4, 3, 2, 1]);
    // The subscription was saved.
    let saved = std::fs::read_to_string(config.path().join("subscriptions")).unwrap();
    assert_eq!(saved.trim(), terse_net::to_hex(&author.feed_key()));
}
