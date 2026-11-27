//! Feed addresses with hints (NETWORK 4.9), over real loopback WebSockets:
//! an author who publishes only to relay B is found by a reader whose only
//! relay is A when it subscribes with the address `key@B`, and not with the
//! bare key; the hint is not used when A holds the feed, is superseded by
//! the relay lines of the feed's profile, is off with feed relays, and is
//! refused for a loopback address unless the test override is on.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use terse_net::relay::{DEFAULT_TIMEOUT, RelayClient};
use terse_net::{BlobStore, Entry, Identity, Node, blob_id};
use terse_relay::Relay;

fn in_process(dir: &std::path::Path) -> (String, Arc<Relay>) {
    let relay = Arc::new(Relay::new(BlobStore::open(dir, None).unwrap(), 1024));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let r = relay.clone();
    std::thread::spawn(move || terse_relay::serve(listener, r));
    (url, relay)
}

/// A relay serving wss:// with the test certificate, trusted in this
/// process through the test authority: a profile names only encrypted
/// relays (NETWORK 4.6).
fn in_process_tls(dir: &std::path::Path) -> (String, Arc<Relay>) {
    let certs = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../terse-testkit/tls");
    terse_net::tls::trust_ca_file(&certs.join("ca.pem")).unwrap();
    let (url, relay) = in_process(dir);
    relay.set_tls(Some(terse_net::tls::server_config(&certs.join("localhost.pem"), &certs.join("localhost.key")).unwrap()));
    (url.replace("ws://", "wss://"), relay)
}

/// The hint that names a relay at `url` (`ws://127.0.0.1:<port>`).
fn hint(url: &str) -> &str {
    url.strip_prefix("ws://").unwrap()
}

fn poll_until(node: &mut Node, what: &str, mut ok: impl FnMut(&Node) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ok(node) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        node.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Polls for a while, for things that must not happen.
fn idle(node: &mut Node) {
    let until = Instant::now() + Duration::from_millis(600);
    while Instant::now() < until {
        node.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// An article of `author`, `seq`, put and published on `relay`.
fn publish(relay: &Relay, author: &Identity, seq: u32, text: &[u8]) {
    relay.blobs().put(text).unwrap();
    let e = Entry::article(author.feed_key(), seq, seq, text, None, "An article").sign(author).unwrap();
    assert_eq!(relay.publish(&e.bytes), 1);
}

struct Setup {
    url_a: String,
    a: Arc<Relay>,
    url_b: String,
    b: Arc<Relay>,
    author: Identity,
    article: Vec<u8>,
    node: Node,
    _dirs: Vec<tempfile::TempDir>,
}

/// Relays A and B; the author's article only on B; a reader node whose
/// only relay is A, with loopback feed relays allowed (`unrestricted`).
fn setup(unrestricted: bool) -> Setup {
    let dirs: Vec<tempfile::TempDir> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let (url_a, a) = in_process(dirs[0].path());
    let (url_b, b) = in_process(dirs[1].path());
    let author = Identity::from_seed([61; 32]);
    let article = b"Published on relay B only.".to_vec();
    publish(&b, &author, 1, &article);
    let mut node = Node::open(Some(dirs[2].path().to_path_buf()), std::slice::from_ref(&url_a), DEFAULT_TIMEOUT).unwrap();
    if unrestricted {
        node.set_feed_relays_unrestricted(Box::new(|url| RelayClient::websocket(url, DEFAULT_TIMEOUT)));
    }
    Setup { url_a, a, url_b, b, author, article, node, _dirs: dirs }
}

#[test]
fn an_address_finds_a_feed_on_another_relay() {
    let mut s = setup(true);
    let feed = s.author.feed_key();
    let address = hint(&s.url_b).to_string();
    s.node.subscribe_with_hints(feed, Some(&address)).unwrap();
    assert_eq!(s.node.subscriptions().hints(&feed), Some(address.as_str()));
    poll_until(&mut s.node, "the backlog from B", |n| n.feed_store().seqs(&feed).unwrap() == [1]);
    poll_until(&mut s.node, "the article from B", |n| n.store().read(&blob_id(&s.article)).unwrap().is_some());
    assert_eq!(s.node.connected_relays(), [s.url_a.clone(), s.url_b.clone()]);
    assert!(s.node.followed_by_hint(&feed));
    assert_eq!(s.node.held_on_own_relays(&feed), Some(false));
    // The names record says so (SPEC §content.feeds, flag 08h).
    let (_, names) = s.node.name_records(0, 1000);
    assert_eq!(names[1] & terse_core::content::NAME_HINTED, terse_core::content::NAME_HINTED);
    // Unsubscribing drops the hint relay.
    s.node.unsubscribe(&feed).unwrap();
    poll_until(&mut s.node, "B to be dropped", |n| n.connected_relays() == [s.url_a.clone()]);
    assert!(!s.node.followed_by_hint(&feed));
}

#[test]
fn a_bare_key_finds_nothing_there() {
    let mut s = setup(true);
    let feed = s.author.feed_key();
    s.node.subscribe(feed).unwrap();
    poll_until(&mut s.node, "A's answer", |n| n.held_on_own_relays(&feed) == Some(false));
    idle(&mut s.node);
    assert!(s.node.feed_store().seqs(&feed).unwrap().is_empty());
    assert_eq!(s.node.connected_relays(), [s.url_a.clone()]);
}

#[test]
fn the_hint_is_not_used_when_an_own_relay_holds_the_feed() {
    let mut s = setup(true);
    let feed = s.author.feed_key();
    publish(&s.a, &s.author, 2, b"On A.");
    s.node.subscribe_with_hints(feed, Some(hint(&s.url_b))).unwrap();
    poll_until(&mut s.node, "the entry from A", |n| n.feed_store().seqs(&feed).unwrap() == [2]);
    idle(&mut s.node);
    assert_eq!(s.node.feed_store().seqs(&feed).unwrap(), [2], "nothing from B");
    assert_eq!(s.node.connected_relays(), [s.url_a.clone()]);
    assert_eq!(s.node.held_on_own_relays(&feed), Some(true));
    assert!(!s.node.followed_by_hint(&feed));
}

#[test]
fn the_profile_supersedes_the_hint() {
    let mut s = setup(true);
    let feed = s.author.feed_key();
    let dir_c = tempfile::tempdir().unwrap();
    let (url_c, c) = in_process_tls(dir_c.path());
    // On B, after the article: a profile that names C; on C, another article.
    let doc = format!("listed: no\nrelay: {url_c}\n").into_bytes();
    s.b.blobs().put(&doc).unwrap();
    let profile = Entry::profile(feed, 2, 2, &doc, "Author").sign(&s.author).unwrap();
    assert_eq!(s.b.publish(&profile.bytes), 1);
    publish(&c, &s.author, 3, b"On C, the relay the profile names.");

    s.node.subscribe_with_hints(feed, Some(hint(&s.url_b))).unwrap();
    poll_until(&mut s.node, "the entry from C", |n| n.feed_store().seqs(&feed).unwrap() == [1, 2, 3]);
    poll_until(&mut s.node, "B to be replaced by C", |n| n.connected_relays() == [s.url_a.clone(), url_c.clone()]);
    assert!(!s.node.followed_by_hint(&feed), "followed through the profile's relay now");
    assert_eq!(s.node.subscriptions().hints(&feed), Some(hint(&s.url_b)), "the hints are kept");
}

#[test]
fn no_hints_without_feed_relays() {
    let mut s = setup(true);
    let feed = s.author.feed_key();
    s.node.set_feed_relays(false);
    s.node.subscribe_with_hints(feed, Some(hint(&s.url_b))).unwrap();
    poll_until(&mut s.node, "A's answer", |n| n.held_on_own_relays(&feed) == Some(false));
    idle(&mut s.node);
    assert!(s.node.feed_store().seqs(&feed).unwrap().is_empty());
    assert_eq!(s.node.connected_relays(), [s.url_a.clone()]);
}

#[test]
fn a_loopback_hint_is_refused() {
    let mut s = setup(false);
    let feed = s.author.feed_key();
    s.node.subscribe_with_hints(feed, Some(hint(&s.url_b))).unwrap();
    poll_until(&mut s.node, "A's answer", |n| n.held_on_own_relays(&feed) == Some(false));
    idle(&mut s.node);
    assert!(s.node.feed_store().seqs(&feed).unwrap().is_empty());
    assert_eq!(s.node.connected_relays(), [s.url_a.clone()]);
    assert!(!s.node.followed_by_hint(&feed));
}

#[test]
fn malformed_hints_are_ignored() {
    let mut s = setup(true);
    let feed = s.author.feed_key();
    let three = format!("{0},{0},{0}", hint(&s.url_b));
    for bad in [three.as_str(), "", "a b", "127.0.0.1:0"] {
        s.node.subscribe_with_hints(feed, Some(bad)).unwrap();
        assert_eq!(s.node.subscriptions().hints(&feed), None, "{bad}");
    }
    idle(&mut s.node);
    assert!(s.node.feed_store().seqs(&feed).unwrap().is_empty());
    assert_eq!(s.node.connected_relays(), [s.url_a.clone()]);
}
