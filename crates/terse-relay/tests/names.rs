//! Profiles and search on the relay (NETWORK 4.6, 4.7): the latest profile
//! is kept through pruning; Find answers from the relay's index; a client
//! asking two relays merges their results.

use std::net::TcpListener;
use std::sync::Arc;

use terse_net::protocol::{SCOPE_ENTRIES, SCOPE_FEEDS};
use terse_net::relay::{DEFAULT_TIMEOUT, RelayClient, RelayReply};
use terse_net::{BlobStore, Done, Entry, Identity, Node, blob_id, entry};
use terse_relay::Relay;

fn relay(dir: &std::path::Path, cap: usize) -> Relay {
    Relay::new(BlobStore::open(dir, None).unwrap(), cap)
}

/// Publishes a profile (and stores its document unless `doc` is `None`).
fn profile(r: &Relay, id: &Identity, seq: u32, name: &str, doc: Option<&[u8]>) {
    let data = doc.unwrap_or(b"listed: yes\n");
    if doc.is_some() {
        r.blobs().put(data).unwrap();
    }
    let e = Entry::profile(id.feed_key(), seq, seq, data, name).sign(id).unwrap();
    assert_eq!(r.publish(&e.bytes), 1);
}

fn article(r: &Relay, id: &Identity, seq: u32, time: u32, title: &str) {
    let e = Entry::article(id.feed_key(), seq, time, title.as_bytes(), None, title).sign(id).unwrap();
    assert_eq!(r.publish(&e.bytes), 1);
}

fn titles(found: Vec<Vec<u8>>) -> Vec<String> {
    found.iter().map(|b| entry::decode(b).unwrap().entry.title).collect()
}

const LISTED: Option<&[u8]> = Some(b"listed: yes\n\nAbout.");
const UNLISTED: Option<&[u8]> = Some(b"listed: no\n");

#[test]
fn latest_profile_is_kept_through_pruning() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), 3);
    let id = Identity::from_seed([1; 32]);
    profile(&r, &id, 1, "First", LISTED);
    profile(&r, &id, 2, "Second", LISTED);
    for seq in 3..=9 {
        article(&r, &id, seq, seq, "post");
    }
    assert_eq!(r.feeds().seqs(&id.feed_key()).unwrap(), [2, 7, 8, 9]);
    assert_eq!(titles(r.find(SCOPE_FEEDS, b"sec").unwrap()), ["Second"]);
    // After a restart too (the index is rebuilt from the store).
    drop(r);
    let r = relay(dir.path(), 3);
    article(&r, &id, 10, 10, "post");
    assert_eq!(r.feeds().seqs(&id.feed_key()).unwrap(), [2, 8, 9, 10]);
    assert_eq!(titles(r.find(SCOPE_FEEDS, b"Second").unwrap()), ["Second"]);
}

#[test]
fn find_feeds_and_entries() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), 1024);
    let ids: Vec<Identity> = (10..16).map(|i| Identity::from_seed([i; 32])).collect();
    profile(&r, &ids[0], 1, "Mariana", LISTED);
    profile(&r, &ids[1], 1, "ANA", LISTED);
    profile(&r, &ids[2], 1, "anabel", LISTED);
    profile(&r, &ids[3], 1, "Ana hidden", UNLISTED);
    profile(&r, &ids[4], 1, "Ana no document", None);
    profile(&r, &ids[5], 1, "Ännа", LISTED);
    // Older profile listed, newer unlisted: unlisted.
    profile(&r, &ids[0], 2, "Mariana", UNLISTED);
    profile(&r, &ids[0], 3, "Mariana", LISTED);

    // Prefix matches first (by feed key), then the others; ASCII folding.
    let found = r.find(SCOPE_FEEDS, b"aNa").unwrap();
    let mut prefix = vec![(ids[1].feed_key(), "ANA"), (ids[2].feed_key(), "anabel")];
    prefix.sort();
    let want: Vec<&str> = prefix.iter().map(|p| p.1).chain(["Mariana"]).collect();
    assert_eq!(titles(found), want);
    // Only A–Z fold: "Ä" is not "a", "А" (Cyrillic) is not "A".
    assert_eq!(titles(r.find(SCOPE_FEEDS, "änn".as_bytes()).unwrap()), Vec::<String>::new());
    assert_eq!(titles(r.find(SCOPE_FEEDS, "Änn".as_bytes()).unwrap()), ["Ännа"]);
    // The document arrives later: listed from then on.
    assert!(titles(r.find(SCOPE_FEEDS, b"no document").unwrap()).is_empty());
    r.blobs().put(b"listed: yes\n").unwrap();
    assert_eq!(titles(r.find(SCOPE_FEEDS, b"no document").unwrap()), ["Ana no document"]);

    // Entries of listed feeds only, articles only, newest stored first
    // (whatever time their authors claim).
    article(&r, &ids[1], 2, 100, "Rust news");
    article(&r, &ids[2], 2, 300, "More NEWS");
    article(&r, &ids[3], 2, 500, "Hidden news");
    article(&r, &ids[2], 3, 200, "Old news");
    article(&r, &ids[1], 1_000_000, 1, "Late news");
    assert_eq!(titles(r.find(SCOPE_ENTRIES, b"news").unwrap()), ["Late news", "Old news", "More NEWS", "Rust news"]);
    // The order is kept across a restart; entries stored with no arrival
    // record (an older store) are recorded at start, by feed key then seq.
    drop(r);
    let r = relay(dir.path(), 1024);
    assert_eq!(titles(r.find(SCOPE_ENTRIES, b"news").unwrap()), ["Late news", "Old news", "More NEWS", "Rust news"]);
    drop(r);
    std::fs::remove_file(dir.path().join("feeds").join("arrivals")).unwrap();
    let r = relay(dir.path(), 1024);
    let mut seeded = vec![
        (ids[1].feed_key(), 2, "Rust news"),
        (ids[1].feed_key(), 1_000_000, "Late news"),
        (ids[2].feed_key(), 2, "More NEWS"),
        (ids[2].feed_key(), 3, "Old news"),
    ];
    seeded.sort();
    let want: Vec<&str> = seeded.iter().rev().map(|s| s.2).collect();
    assert_eq!(titles(r.find(SCOPE_ENTRIES, b"news").unwrap()), want);
    // At most 16 results.
    for seq in 4..30 {
        article(&r, &ids[2], seq, 1000 + seq, "many");
    }
    assert_eq!(r.find(SCOPE_ENTRIES, b"many").unwrap().len(), 16);
    assert_eq!(titles(r.find(SCOPE_ENTRIES, b"many").unwrap())[0], "many");
}

fn serve(r: Relay) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, Arc::new(r)));
    url
}

/// A client asks two relays and merges their answers (NETWORK 4.7).
#[test]
fn find_merges_two_relays() {
    let (da, db, dn) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (relay(da.path(), 1024), relay(db.path(), 1024));
    let (x, y, z) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]), Identity::from_seed([3; 32]));
    profile(&a, &x, 1, "Writer X", LISTED);
    profile(&b, &y, 1, "Writer Y", LISTED);
    profile(&a, &z, 1, "Writer Z", LISTED);
    profile(&b, &z, 2, "Writer Zed", LISTED);
    let (ua, ub) = (serve(a), serve(b));
    let mut node = Node::new(BlobStore::open(dn.path(), None).unwrap(), None);
    node.add_relay(&ua, RelayClient::websocket(&ua, DEFAULT_TIMEOUT));
    node.add_relay(&ub, RelayClient::websocket(&ub, DEFAULT_TIMEOUT));
    let Done::Found(found) = node.find(SCOPE_FEEDS, b"writer") else { panic!() };
    let mut want = vec![(x.feed_key(), "Writer X", 1), (y.feed_key(), "Writer Y", 1), (z.feed_key(), "Writer Zed", 2)];
    want.sort();
    let got: Vec<(_, &str, u32)> = found.iter().map(|s| (s.entry.feed, s.entry.title.as_str(), s.entry.seq)).collect();
    assert_eq!(got, want);
    assert!(found.iter().all(|s| s.entry.blob == blob_id(b"listed: yes\n\nAbout.")));
}

/// A connection that searches more than 4 times a second is refused
/// (NETWORK 4.7); a refused find does not count. The relay's clock stands
/// still unless the test moves it, so the finds are all in one second
/// however slowly they are sent.
#[test]
fn find_rate_limit() {
    let dir = tempfile::tempdir().unwrap();
    let r = Arc::new(relay(dir.path(), 1024));
    r.freeze_clock();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let served = r.clone();
    std::thread::spawn(move || terse_relay::serve(listener, served));
    let mut client = RelayClient::websocket(&url, DEFAULT_TIMEOUT);
    client.connect().unwrap();
    let ask = |client: &mut RelayClient, n: usize| {
        let reqs: Vec<u32> = (0..n).map(|_| client.find(SCOPE_FEEDS, b"x")).collect();
        let mut replies = Vec::new();
        while replies.len() < n {
            replies.extend(client.poll());
        }
        replies.sort_by_key(|r| reqs.iter().position(|q| *q == r.0));
        replies.into_iter().map(|r| r.1).collect::<Vec<_>>()
    };
    let got = ask(&mut client, 6);
    let found = RelayReply::Found(Vec::new(), Vec::new());
    assert_eq!(got[..4], [found.clone(), found.clone(), found.clone(), found.clone()]);
    // NETWORK 3.1: refused with a wait of at most a second.
    let later = RelayReply::Refused(terse_net::protocol::Refusal::TryLater(std::time::Duration::from_secs(1)));
    assert_eq!(got[4..], [later.clone(), later.clone()]);
    // Just short of a second later, still refused; a second after the
    // first finds, they have left the window.
    r.advance_clock(std::time::Duration::from_millis(999));
    assert_eq!(ask(&mut client, 1), [later]);
    r.advance_clock(std::time::Duration::from_millis(1));
    assert_eq!(ask(&mut client, 1), [found]);
    // Another connection has its own allowance.
    let mut other = RelayClient::websocket(&url, DEFAULT_TIMEOUT);
    other.connect().unwrap();
    assert_eq!(ask(&mut other, 4).iter().filter(|r| matches!(r, RelayReply::Refused(_))).count(), 0);
}
