//! What a relay keeps grows with what it keeps, not with the traffic it has
//! seen (NETWORK 5.3, "The reference relay's bounds"): the arrival log is
//! compacted to the entries still kept, a batch at a time and safely; a
//! connection's subscriptions, the peer links' subscriptions and the gets
//! asked of the peers are bounded. Counted, by the relay's clock (stopped
//! here), never timed.

use std::path::Path;
use std::sync::Arc;

use terse_net::protocol::{self, Reply, status};
use terse_net::{BlobStore, Entry, FeedKey, Identity, blob_id};
use terse_relay::limits::Group;
use terse_relay::{ARRIVALS_SLACK, Answer, COMPACT_BATCH, Compacting, Connection, MAX_PULLS, PULLS_PER_CONNECTION, Policy, Reason, Relay};

const RECORD: u64 = 36;

fn relay(dir: &Path, policy: Policy) -> Arc<Relay> {
    let r = Arc::new(Relay::with_policy(BlobStore::open(dir, None).unwrap(), policy).unwrap());
    r.freeze_clock();
    r
}

fn entry(id: &Identity, seq: u32) -> Vec<u8> {
    Entry::article(id.feed_key(), seq, 1_800_000_000, &seq.to_le_bytes(), None, "e").sign(id).unwrap().bytes
}

fn ask(relay: &Relay, conn: &Connection, message: Vec<u8>) -> Reply {
    match relay.respond(conn, &message) {
        Answer::Reply(r) => protocol::decode_reply(&r).unwrap(),
        _ => panic!("no reply"),
    }
}

fn publish(relay: &Relay, conn: &Connection, bytes: &[u8]) -> u8 {
    match ask(relay, conn, protocol::encode_publish(1, bytes)) {
        Reply::Status { status, .. } => status,
        r => panic!("{r:?}"),
    }
}

fn subscribe(relay: &Relay, conn: &Connection, feed: &FeedKey) -> u8 {
    match ask(relay, conn, protocol::encode_subscribe(1, feed, 0)) {
        Reply::Subscribed { .. } => status::DONE,
        Reply::Status { status, .. } => status,
        r => panic!("{r:?}"),
    }
}

/// The records of the arrival log on disk, in order.
fn log(dir: &Path) -> Vec<(FeedKey, u32)> {
    let bytes = std::fs::read(dir.join("feeds/arrivals")).unwrap_or_default();
    bytes.chunks_exact(RECORD as usize).map(|r| (r[..32].try_into().unwrap(), u32::from_le_bytes(r[32..].try_into().unwrap()))).collect()
}

fn log_bytes(dir: &Path) -> u64 {
    std::fs::metadata(dir.join("feeds/arrivals")).map_or(0, |m| m.len())
}

/// Three feeds of 1500 entries each, published in turn, on a relay that
/// keeps 256 of each: 4500 records in the log, 768 entries kept.
fn filled(dir: &Path) -> (Arc<Relay>, Vec<Identity>) {
    let r = relay(dir, Policy::unlimited(256));
    let ids: Vec<Identity> = (0..3).map(|n| Identity::from_seed([60 + n; 32])).collect();
    for seq in 1..=1500 {
        for id in &ids {
            assert_eq!(r.publish(&entry(id, seq)), status::DONE);
        }
    }
    assert_eq!(r.kept_entries(), 768);
    assert_eq!(r.arrivals(), (4500, 4500 * RECORD));
    (r, ids)
}

/// The kept entries, in the order they arrived.
fn kept_in_order(ids: &[Identity]) -> Vec<(FeedKey, u32)> {
    (1245..=1500).flat_map(|seq| ids.iter().map(move |id| (id.feed_key(), seq))).collect()
}

/// The log is compacted to the entries kept, in their order, a batch of at
/// most `COMPACT_BATCH` records at a time (the clock stands still, so the
/// count ends each batch); between batches the relay takes and records new
/// entries, which the compacted log keeps.
#[test]
fn log_is_compacted_in_batches() {
    let dir = tempfile::tempdir().unwrap();
    let (r, ids) = filled(dir.path());
    assert!(4500 > r.arrivals_bound());
    let c = r.connect();
    let late = Identity::from_seed([70; 32]);
    let mut steps = Vec::new();
    let mut seq = 0;
    loop {
        let step = r.compact_batch();
        steps.push(step);
        if let Compacting::Done { .. } | Compacting::Idle | Compacting::Failed = step {
            break;
        }
        // Answered between batches, on this thread: the log's lock is
        // free (one still held would never be let go).
        seq += 1;
        assert_eq!(publish(&r, &c, &entry(&late, seq)), status::DONE);
        assert!(steps.len() < 100);
    }
    let full = COMPACT_BATCH;
    // 4500 records and one more each batch.
    assert_eq!(
        steps,
        [
            Compacting::Started,
            Compacting::Copied(full),
            Compacting::Copied(full),
            Compacting::Copied(full),
            Compacting::Copied(full),
            Compacting::Copied(4500 + 5 - 4 * full),
            Compacting::Synced,
            Compacting::Done { before: 4500, after: 768 + 7 },
        ]
    );
    let mut expected = kept_in_order(&ids);
    // The entries recorded meanwhile, in their places.
    expected.extend((1..=7).map(|s| (late.feed_key(), s)));
    assert_eq!(log(dir.path()), expected);
    assert_eq!(log_bytes(dir.path()), (768 + 7) * RECORD);
    assert_eq!(r.arrivals(), (775, 775 * RECORD));
    assert_eq!(r.compactions(), 1);
    assert!(!dir.path().join("feeds/arrivals.compact").exists());
    // Within its bound: nothing more to do; recording goes on in the new
    // file.
    assert_eq!(r.compact_batch(), Compacting::Idle);
    assert_eq!(publish(&r, &c, &entry(&late, 8)), status::DONE);
    assert_eq!(log(dir.path()).last(), Some(&(late.feed_key(), 8)));
    // A restart reads the same log.
    drop(c);
    drop(r);
    let r = relay(dir.path(), Policy::unlimited(256));
    assert_eq!(r.arrivals(), (776, 776 * RECORD));
    assert!(r.bounds().contains("arrival-log 776 records, 27936 bytes; bound 2576 records (2 x 776 entries kept + 1024); 0 compactions"), "{}", r.bounds());
}

/// A crash after the copy was written and synced, before the rename,
/// leaves the log as it was; the next start removes the copy and a
/// compaction then runs to its end. A crash after the rename that lost the
/// end of the copy (a torn record, records missing) leaves a log the next
/// start makes whole: the torn record is cut off, and the entries kept
/// that it lacks are recorded again.
#[test]
fn crash_during_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let (r, ids) = filled(dir.path());
    let before = log(dir.path());
    loop {
        match r.compact_batch() {
            Compacting::Synced => break,
            Compacting::Started | Compacting::Copied(_) => {}
            s => panic!("{s:?}"),
        }
    }
    let copy = dir.path().join("feeds/arrivals.compact");
    assert_eq!(std::fs::metadata(&copy).unwrap().len(), 768 * RECORD, "the copy is written");
    // The crash: the relay is gone, the rename never happened.
    drop(r);
    assert_eq!(log(dir.path()), before);
    let r = relay(dir.path(), Policy::unlimited(256));
    assert!(!copy.exists(), "the copy is removed at the start");
    assert_eq!(r.arrivals(), (4500, 4500 * RECORD));
    assert_eq!(r.compact(), Some((4500, 768)));
    assert_eq!(log(dir.path()), kept_in_order(&ids));
    drop(r);

    // After the rename, the end of the copy lost: 100 records and a torn
    // one.
    let path = dir.path().join("feeds/arrivals");
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_len(668 * RECORD + 20).unwrap();
    drop(f);
    let r = relay(dir.path(), Policy::unlimited(256));
    assert_eq!(log_bytes(dir.path()) % RECORD, 0, "the torn record is cut off");
    let records = log(dir.path());
    assert_eq!(records.len(), 768);
    assert_eq!(records[..668], kept_in_order(&ids)[..668], "the order of what was kept is kept");
    let mut all = records.clone();
    all.sort();
    let mut expected = kept_in_order(&ids);
    expected.sort();
    assert_eq!(all, expected, "every entry kept is in the log once");
    assert_eq!(r.compact(), None, "within its bound");
}

/// A limited connection may subscribe to at most so many feeds; an
/// in-process one is not limited.
#[test]
fn subscriptions_per_connection() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), Policy { subscriptions_per_connection: Some(3), ..Policy::unlimited(256) });
    let feeds: Vec<FeedKey> = (0..5).map(|n| Identity::from_seed([80 + n; 32]).feed_key()).collect();
    let c = r.connect_from(Group::of("203.0.113.9".parse().unwrap()));
    for f in &feeds[..3] {
        assert_eq!(subscribe(&r, &c, f), status::DONE);
    }
    assert_eq!(subscribe(&r, &c, &feeds[3]), status::REFUSED);
    assert_eq!(r.refused(Reason::Subscriptions), 1);
    // Subscribing again to a feed it has is not a new subscription.
    assert_eq!(subscribe(&r, &c, &feeds[0]), status::DONE);
    assert!(matches!(ask(&r, &c, protocol::encode_unsubscribe(1, &feeds[0])), Reply::Status { status: status::DONE, .. }));
    assert_eq!(subscribe(&r, &c, &feeds[3]), status::DONE);
    assert!(r.bounds().contains("subscriptions 3 on 1 client connections, for 3 feeds; bound 3 per connection"), "{}", r.bounds());
    let local = r.connect();
    for f in &feeds {
        assert_eq!(subscribe(&r, &local, f), status::DONE);
    }
}

/// A peer link is subscribed to a feed while some client subscribes to it,
/// and unsubscribed when none does any more (by unsubscribing or
/// disconnecting), so what a peer link is subscribed to is bounded by what
/// clients subscribe to now.
#[test]
fn peer_subscriptions_follow_clients() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), Policy::unlimited(256));
    let peer = r.connect();
    assert!(matches!(r.respond(&peer, &protocol::encode_peer()), Answer::Nothing));
    let feeds: Vec<FeedKey> = (0..2).map(|n| Identity::from_seed([90 + n; 32]).feed_key()).collect();
    let sent = |peer: &Connection| -> Vec<(u8, FeedKey)> { peer.take_queued().iter().map(|m| (m[0], m[5..37].try_into().unwrap())).collect() };
    let (a, b) = (r.connect(), r.connect());
    assert_eq!(subscribe(&r, &a, &feeds[0]), status::DONE);
    assert_eq!(subscribe(&r, &b, &feeds[0]), status::DONE);
    assert_eq!(subscribe(&r, &b, &feeds[1]), status::DONE);
    assert_eq!(sent(&peer), [(protocol::SUBSCRIBE, feeds[0]), (protocol::SUBSCRIBE, feeds[1])]);
    // Another client still subscribes to the first.
    assert!(matches!(ask(&r, &a, protocol::encode_unsubscribe(1, &feeds[0])), Reply::Status { status: status::DONE, .. }));
    assert_eq!(sent(&peer), []);
    r.disconnect(&b);
    let mut gone = sent(&peer);
    gone.sort();
    assert_eq!(gone, [(protocol::UNSUBSCRIBE, feeds[0]), (protocol::UNSUBSCRIBE, feeds[1])]);
    assert!(r.bounds().contains("peer-subscriptions 0;"), "{}", r.bounds());
    // A new client's subscription is repeated on the link again.
    assert_eq!(subscribe(&r, &a, &feeds[1]), status::DONE);
    assert_eq!(sent(&peer), [(protocol::SUBSCRIBE, feeds[1])]);
}

/// Gets asked of the peers: at most `PULLS_PER_CONNECTION` for one
/// connection at once (the rest are answered from the store: not found),
/// and forgotten, with the requests sent for them, when the client goes.
#[test]
fn pulls_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), Policy::unlimited(256));
    let peer = r.connect();
    assert!(matches!(r.respond(&peer, &protocol::encode_peer()), Answer::Nothing));
    let c = r.connect();
    let get = |n: u32| r.respond(&c, &protocol::encode_get(n, &blob_id(&n.to_le_bytes())));
    for n in 0..PULLS_PER_CONNECTION as u32 {
        assert!(matches!(get(n), Answer::Nothing), "asked of the peer");
    }
    let Answer::Reply(over) = get(99) else { panic!("over the bound: answered at once") };
    assert!(matches!(protocol::decode_reply(&over), Some(Reply::Get { status: status::NOT_FOUND, .. })));
    assert_eq!(peer.take_queued().len(), PULLS_PER_CONNECTION);
    assert!(r.bounds().contains(&format!("pulls {PULLS_PER_CONNECTION} waiting for peers ({PULLS_PER_CONNECTION} requests); bound {PULLS_PER_CONNECTION} per connection, {MAX_PULLS} in all")), "{}", r.bounds());
    r.disconnect(&c);
    assert!(r.bounds().contains("pulls 0 waiting for peers (0 requests)"), "{}", r.bounds());
    let _ = ARRIVALS_SLACK;
}
