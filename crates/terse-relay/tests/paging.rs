//! Pages of a find (NETWORK 4.7, "Pages"): a relay answers 16 results a
//! page with a cursor while the list goes on; the next page follows the
//! last result of the one before, in the same order, whatever was stored
//! since; a cursor survives the arrival log's compaction while its entry
//! is kept, and is refused (status 3, count 0, c 0) once it cannot be
//! continued from, or when it was not made for that find; a find that
//! ends at its query is answered as before paging. Every page is a find
//! for the relay's allowance.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use terse_net::protocol::{self, Reply, SCOPE_ENTRIES, SCOPE_FEEDS, SCOPE_PROGRAMS, status};
use terse_net::{BlobStore, Entry, FeedKey, Identity, entry};
use terse_relay::{Answer, Compacting, Connection, Policy, Relay};

fn relay(dir: &Path, cap: usize) -> Arc<Relay> {
    let r = Arc::new(Relay::with_policy(BlobStore::open(dir, None).unwrap(), Policy::unlimited(cap)).unwrap());
    r.freeze_clock();
    r
}

fn listed(r: &Relay, id: &Identity, seq: u32, name: &str) {
    let doc = b"listed: yes\n";
    r.blobs().put(doc).unwrap();
    let e = Entry::profile(id.feed_key(), seq, seq, doc, name).sign(id).unwrap();
    assert_eq!(r.publish(&e.bytes), status::DONE);
}

fn article(r: &Relay, id: &Identity, seq: u32, title: &str) {
    let e = Entry::article(id.feed_key(), seq, seq, title.as_bytes(), None, title).sign(id).unwrap();
    assert_eq!(r.publish(&e.bytes), status::DONE);
}

fn keys(found: &[Vec<u8>]) -> Vec<(FeedKey, u32)> {
    found.iter().map(|b| entry::decode(b).unwrap()).map(|s| (s.entry.feed, s.entry.seq)).collect()
}

/// Every page of a find, by `find_page`.
fn all_pages(r: &Relay, scope: u8, query: &[u8]) -> Vec<Vec<Vec<u8>>> {
    let mut pages = Vec::new();
    let mut cursor = Vec::new();
    loop {
        let (found, next) = r.find_page(scope, query, &cursor).expect("a page");
        assert!(found.len() <= 16);
        assert!(next.len() <= 64);
        pages.push(found);
        if next.is_empty() {
            return pages;
        }
        assert_eq!(pages.last().unwrap().len(), 16, "a page with a cursor is full");
        cursor = next;
        assert!(pages.len() < 100);
    }
}

fn ask(r: &Relay, conn: &Connection, message: Vec<u8>) -> Vec<u8> {
    match r.respond(conn, &message) {
        Answer::Reply(m) => m,
        _ => panic!("no reply"),
    }
}

/// Scope 2: 40 matching articles come in pages of 16, 16 and 8, newest
/// stored first, each once; entries stored after the first page are in
/// none of the later ones; scope 1: 40 feeds in their order.
#[test]
fn pages_in_order_and_stable() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), 1024);
    let ids: Vec<Identity> = (0..40u8).map(|i| Identity::from_seed([i + 1; 32])).collect();
    for (i, id) in ids.iter().enumerate() {
        listed(&r, id, 1, if i % 3 == 0 { "Ana" } else { "Mariana" });
    }
    for seq in 2..=11 {
        for id in &ids[..4] {
            article(&r, id, seq, &format!("news {seq}"));
            article(&r, &ids[39], 100 + seq, "other");
        }
    }
    let pages = all_pages(&r, SCOPE_ENTRIES, b"NEWS");
    assert_eq!(pages.iter().map(Vec::len).collect::<Vec<_>>(), [16, 16, 8]);
    let got: Vec<(FeedKey, u32)> = pages.iter().flat_map(|p| keys(p)).collect();
    let newest_first: Vec<(FeedKey, u32)> = (2..=11).rev().flat_map(|seq| ids[..4].iter().rev().map(move |id| (id.feed_key(), seq))).collect();
    assert_eq!(got, newest_first);
    assert_eq!(r.find(SCOPE_ENTRIES, b"news").unwrap(), pages[0], "the first page is the find of before");

    // New entries between pages: not in the pages after the first.
    let (first, cursor) = r.find_page(SCOPE_ENTRIES, b"news", &[]).unwrap();
    for seq in 12..=20 {
        article(&r, &ids[5], seq, "news late");
    }
    let (second, cursor) = r.find_page(SCOPE_ENTRIES, b"news", &cursor).unwrap();
    let (third, end) = r.find_page(SCOPE_ENTRIES, b"news", &cursor).unwrap();
    assert!(end.is_empty());
    assert_eq!([first, second, third].concat(), pages.concat(), "the same pages as before the new entries");
    // A new first page starts with them.
    assert_eq!(keys(&r.find(SCOPE_ENTRIES, b"news").unwrap())[0], (ids[5].feed_key(), 20));

    // Scope 1: names that start with the query first, then by key; a
    // feed listed after the first page is not in the later ones.
    let (first, cursor) = r.find_page(SCOPE_FEEDS, b"ana", &[]).unwrap();
    let late = Identity::from_seed([200; 32]);
    listed(&r, &late, 1, "Ana late");
    let mut rest = Vec::new();
    let mut cursor = cursor;
    while !cursor.is_empty() {
        let (page, next) = r.find_page(SCOPE_FEEDS, b"ana", &cursor).unwrap();
        rest.extend(page);
        cursor = next;
    }
    let got: Vec<FeedKey> = keys(&[first, rest].concat()).into_iter().map(|k| k.0).collect();
    let mut prefix: Vec<FeedKey> = ids.iter().step_by(3).map(Identity::feed_key).collect();
    prefix.sort();
    let mut others: Vec<FeedKey> = ids.iter().enumerate().filter(|(i, _)| i % 3 != 0).map(|(_, id)| id.feed_key()).collect();
    others.sort();
    assert_eq!(got, [prefix, others].concat());
    let all: BTreeSet<FeedKey> = all_pages(&r, SCOPE_FEEDS, b"ana").iter().flat_map(|p| keys(p)).map(|k| k.0).collect();
    assert!(all.contains(&late.feed_key()), "a new search finds it");
}

/// Scope 3 pages hold current versions only.
#[test]
fn program_pages() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), 1024);
    let id = Identity::from_seed([90; 32]);
    listed(&r, &id, 1, "Tools");
    for seq in 2..=41 {
        // Each name twice: the second version is the current one.
        let name = format!("tool {}", (seq - 2) / 2);
        let e = Entry::program(id.feed_key(), seq, seq, b"image", &name).sign(&id).unwrap();
        assert_eq!(r.publish(&e.bytes), status::DONE);
    }
    let pages = all_pages(&r, SCOPE_PROGRAMS, b"tool");
    let got: Vec<u32> = pages.iter().flat_map(|p| keys(p)).map(|k| k.1).collect();
    assert_eq!(got, (2..=41).rev().step_by(2).collect::<Vec<_>>());
}

/// A cursor survives a compaction of the arrival log while its entry is
/// kept, and is refused once it is not; a cursor of another find, a
/// malformed one and a cursor of another scope are refused.
#[test]
fn cursors_after_compaction_and_bad_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), 64);
    let (a, b) = (Identity::from_seed([80; 32]), Identity::from_seed([81; 32]));
    listed(&r, &a, 1, "A");
    listed(&r, &b, 1, "B");
    // 1400 entries of A, 64 kept; then 40 matching entries of B.
    for seq in 2..=1401 {
        article(&r, &a, seq, "filler");
    }
    for seq in 2..=41 {
        article(&r, &b, seq, "match");
    }
    let (first, cursor) = r.find_page(SCOPE_ENTRIES, b"match", &[]).unwrap();
    assert_eq!(first.len(), 16);
    assert!(r.arrivals().0 > r.arrivals_bound());
    let before = r.compactions();
    while !matches!(r.compact_batch(), Compacting::Done { .. }) {}
    assert_eq!(r.compactions(), before + 1);
    let (second, _) = r.find_page(SCOPE_ENTRIES, b"match", &cursor).expect("the cursor's entry is kept");
    assert_eq!(keys(&second), (10..=25).rev().map(|s| (b.feed_key(), s)).collect::<Vec<_>>());

    // B's entries are pruned past its cap: the cursor's entry goes, and
    // once the log is compacted the cursor is refused.
    let (_, cursor) = r.find_page(SCOPE_ENTRIES, b"match", &[]).unwrap();
    for seq in 42..=1500 {
        article(&r, &b, seq, "filler");
    }
    while !matches!(r.compact_batch(), Compacting::Done { .. } | Compacting::Idle) {}
    assert_eq!(r.find_page(SCOPE_ENTRIES, b"match", &cursor), None);

    // Cursors not made for the find.
    let (_, cursor) = r.find_page(SCOPE_ENTRIES, b"filler", &[]).unwrap();
    assert!(!cursor.is_empty());
    assert!(r.find_page(SCOPE_ENTRIES, b"filler", &cursor).is_some());
    assert_eq!(r.find_page(SCOPE_ENTRIES, b"fillers", &cursor), None, "another query");
    assert_eq!(r.find_page(SCOPE_PROGRAMS, b"filler", &cursor), None, "another scope");
    assert_eq!(r.find_page(SCOPE_ENTRIES, b"filler", &cursor[..cursor.len() - 1]), None, "cut short");
    let mut bent = cursor.clone();
    bent[1] ^= 1;
    assert_eq!(r.find_page(SCOPE_ENTRIES, b"filler", &bent), None, "another query's tag");
    assert_eq!(r.find_page(SCOPE_ENTRIES, b"filler", &[0; 65]), None);

    // On the wire: refused with count 0 and c 0, not counted as a find.
    let conn = r.connect();
    let m = ask(&r, &conn, protocol::encode_find(5, SCOPE_ENTRIES, b"fillers", Some(&cursor)));
    assert_eq!(m, [protocol::FIND_REPLY, 5, 0, 0, 0, status::REFUSED, 0, 0]);
}

/// On the wire: a find with `c` gets `c`; one that ends at its query gets
/// a reply that ends at its last entry (an older client reads it); each
/// page counts as a find: the fifth in a second is refused.
#[test]
fn old_style_finds_and_the_allowance() {
    let dir = tempfile::tempdir().unwrap();
    let r = relay(dir.path(), 1024);
    let id = Identity::from_seed([95; 32]);
    listed(&r, &id, 1, "Writer");
    for seq in 2..=40 {
        article(&r, &id, seq, "post");
    }
    let conn = r.connect();
    let old = ask(&r, &conn, protocol::encode_find(1, SCOPE_ENTRIES, b"post", None));
    let Some(Reply::Found { status: status::DONE, entries, cursor, .. }) = protocol::decode_reply(&old) else { panic!() };
    assert_eq!((entries.len(), cursor.len()), (16, 0));
    // Ends at the last entry: as before paging.
    let last = entries.last().unwrap();
    assert!(old.ends_with(last));
    let mut cursor = Vec::new();
    let mut got = Vec::new();
    for page in 0..3 {
        let m = ask(&r, &conn, protocol::encode_find(2 + page, SCOPE_ENTRIES, b"post", Some(&cursor)));
        let Some(Reply::Found { status: status::DONE, entries, cursor: next, .. }) = protocol::decode_reply(&m) else { panic!("{m:?}") };
        assert_eq!(m[m.len() - 1 - next.len()..], [&[next.len() as u8][..], &next].concat(), "c and the cursor end the reply");
        got.extend(entries);
        cursor = next;
    }
    assert!(cursor.is_empty());
    assert_eq!(got.len(), 39);
    // Four finds in this second already: the next page is refused with a
    // "try again" status, count 0 and c 0.
    let m = ask(&r, &conn, protocol::encode_find(9, SCOPE_ENTRIES, b"post", Some(&[])));
    assert_eq!(m.len(), 8);
    assert!(m[5] >= 64, "try again: {}", m[5]);
    assert_eq!(&m[6..], [0, 0]);
}

/// A machine pages through a find (SPEC §content.feeds, find more): the
/// host keeps the relay's cursor; a completion says in `total`'s
/// `FIND_MORE` whether there are more; a find more with nothing left is
/// not found.
#[test]
fn a_machine_pages_with_find_more() {
    use terse_core::content::FIND_MORE;
    use terse_core::{Outcome, Request, RequestKind};
    let dir = tempfile::tempdir().unwrap();
    let r = relay(&dir.path().join("relay"), 1024);
    let id = Identity::from_seed([96; 32]);
    listed(&r, &id, 1, "Writer");
    for seq in 2..=41 {
        article(&r, &id, seq, "news");
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || terse_relay::serve(listener, r));
    let client = BlobStore::open(&dir.path().join("client"), None).unwrap();
    let node = terse_net::Node::new(client, Some(terse_net::relay::RelayClient::websocket(&url, terse_net::relay::DEFAULT_TIMEOUT)));
    let mut service = terse_net::ContentService::start(node);
    let mut ask = |kind: RequestKind| {
        service.submit(vec![Request { handle: 1, kind }]);
        service.wait_finished().remove(0).outcome
    };
    let mut seen = BTreeSet::new();
    let mut results = |o: Outcome| -> (u32, bool) {
        let Outcome::Records { total, data } = o else { panic!("{o:?}") };
        let mut at = 0;
        while at < data.len() {
            seen.insert(data[at + 2..at + 2 + 36].to_vec());
            at += 1 + data[at] as usize;
        }
        (total & !FIND_MORE, total & FIND_MORE != 0)
    };
    assert_eq!(results(ask(RequestKind::Find { scope: SCOPE_ENTRIES, query: b"news".to_vec(), length: 16384 })), (16, true));
    assert_eq!(results(ask(RequestKind::FindMore { scope: SCOPE_ENTRIES, length: 16384 })), (16, true));
    assert_eq!(results(ask(RequestKind::FindMore { scope: SCOPE_ENTRIES, length: 16384 })), (8, false));
    assert_eq!(seen.len(), 40, "each result once");
    assert_eq!(ask(RequestKind::FindMore { scope: SCOPE_ENTRIES, length: 16384 }), Outcome::NotFound);
    assert_eq!(ask(RequestKind::FindMore { scope: SCOPE_FEEDS, length: 16384 }), Outcome::NotFound, "no find of that scope");
}
